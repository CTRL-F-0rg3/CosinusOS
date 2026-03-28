// cache.zig — LRU block cache
//
// MMIO reads są drogie — trzymamy N ostatnio używanych bloków w pamięci.
// Implementacja: fixed-size array + doubly-linked list (bez alokacji).
//
// Cache line = jeden sektor dysku (block_size bajtów, typowo 512).
// Rozmiar cache: CACHE_LINES * block_size bajtów.

const std = @import("std");
const block = @import("block.zig");

// ─── Stałe ────────────────────────────────────────────────────────────────────

pub const CACHE_LINES: usize = 256; // 256 * 512 = 128 KB
pub const BLOCK_SIZE: usize = 512;

// ─── CacheLine ───────────────────────────────────────────────────────────────

const CacheLine = struct {
    lba: u64,
    dirty: bool,
    valid: bool,
    data: [BLOCK_SIZE]u8,
    prev: u16, // index w tablicy (CACHE_LINES = invalid sentinel)
    next: u16,
};

const INVALID_IDX: u16 = 0xFFFF;

// ─── BlockCache ──────────────────────────────────────────────────────────────

pub const BlockCache = struct {
    lines: [CACHE_LINES]CacheLine,
    // LRU linked list — head = most recently used
    lru_head: u16,
    lru_tail: u16,
    // lookup: lba → line index (linear scan, wystarczy dla 256 linii)
    dev: block.BlockDevice,

    pub fn init(dev: block.BlockDevice) BlockCache {
        var c = BlockCache{
            .lines = undefined,
            .lru_head = INVALID_IDX,
            .lru_tail = INVALID_IDX,
            .dev = dev,
        };
        for (&c.lines, 0..) |*l, i| {
            l.* = .{
                .lba = 0,
                .dirty = false,
                .valid = false,
                .data = [_]u8{0} ** BLOCK_SIZE,
                .prev = if (i == 0) INVALID_IDX else @intCast(i - 1),
                .next = if (i == CACHE_LINES - 1) INVALID_IDX else @intCast(i + 1),
            };
        }
        // Zbuduj initial LRU list (0 = MRU, CACHE_LINES-1 = LRU)
        c.lru_head = 0;
        c.lru_tail = CACHE_LINES - 1;
        return c;
    }

    // ── Lookup ──────────────────────────────────────────────────────────────

    fn findLine(self: *BlockCache, lba: u64) ?u16 {
        for (&self.lines, 0..) |*l, i| {
            if (l.valid and l.lba == lba) return @intCast(i);
        }
        return null;
    }

    // ── LRU list ops ────────────────────────────────────────────────────────

    fn detach(self: *BlockCache, idx: u16) void {
        const l = &self.lines[idx];
        if (l.prev != INVALID_IDX) self.lines[l.prev].next = l.next;
        if (l.next != INVALID_IDX) self.lines[l.next].prev = l.prev;
        if (self.lru_head == idx) self.lru_head = l.next;
        if (self.lru_tail == idx) self.lru_tail = l.prev;
        l.prev = INVALID_IDX;
        l.next = INVALID_IDX;
    }

    fn pushFront(self: *BlockCache, idx: u16) void {
        const l = &self.lines[idx];
        l.prev = INVALID_IDX;
        l.next = self.lru_head;
        if (self.lru_head != INVALID_IDX) self.lines[self.lru_head].prev = idx;
        self.lru_head = idx;
        if (self.lru_tail == INVALID_IDX) self.lru_tail = idx;
    }

    fn promoteToFront(self: *BlockCache, idx: u16) void {
        if (self.lru_head == idx) return;
        self.detach(idx);
        self.pushFront(idx);
    }

    // ── Evict LRU ────────────────────────────────────────────────────────────

    fn evict(self: *BlockCache) u16 {
        const idx = self.lru_tail;
        // jeśli dirty — writeback przed eviction
        if (self.lines[idx].dirty) {
            self.writebackLine(idx) catch {};
        }
        self.lines[idx].valid = false;
        self.lines[idx].dirty = false;
        self.detach(idx);
        return idx;
    }

    // ── MMIO read/write ──────────────────────────────────────────────────────
    // Prawdziwy odczyt przez MMIO — zapisujemy lba i czekamy na DMA done.
    // W CosinusOS userspace: to będzie syscall do sterownika dysku,
    // tutaj mamy bezpośredni dostęp (FS server działa ring-0 lub ma mapping).

    fn mmioRead(self: *BlockCache, lba: u64, buf: []u8) void {
        const base: usize = self.dev.info.mmio_base;
        // Protokół: zapisz LBA do rejestru cmd, poczekaj na status
        const cmd_reg: *volatile u64 = @ptrFromInt(base + 0x00);
        const status_reg: *volatile u32 = @ptrFromInt(base + 0x08);
        const data_reg: *volatile u8 = @ptrFromInt(base + 0x10);

        cmd_reg.* = lba | (0 << 48); // bit48=0: read

        // Busy wait — w docelowej wersji zastąpić IRQ/event
        var timeout: u32 = 1_000_000;
        while (status_reg.* == 0 and timeout > 0) : (timeout -= 1) {
            asm volatile ("pause" ::: .{ .memory = true });
        }

        // Kopiuj dane z MMIO data window
        const window: [*]volatile u8 = @ptrCast(data_reg);
        for (buf, 0..) |*b, i| b.* = window[i];
    }

    fn mmioWrite(self: *BlockCache, lba: u64, buf: []const u8) void {
        const base: usize = self.dev.info.mmio_base;
        const cmd_reg: *volatile u64 = @ptrFromInt(base + 0x00);
        const status_reg: *volatile u32 = @ptrFromInt(base + 0x08);
        const data_reg: *volatile u8 = @ptrFromInt(base + 0x10);

        const window: [*]volatile u8 = @ptrCast(data_reg);
        for (buf, 0..) |b, i| window[i] = b;

        cmd_reg.* = lba | (@as(u64, 1) << 48); // bit48=1: write

        var timeout: u32 = 1_000_000;
        while (status_reg.* == 0 and timeout > 0) : (timeout -= 1) {
            asm volatile ("pause" ::: .{ .memory = true });
        }
    }

    fn writebackLine(self: *BlockCache, idx: u16) !void {
        const l = &self.lines[idx];
        self.mmioWrite(l.lba, &l.data);
        l.dirty = false;
    }

    // ── Publiczne API ────────────────────────────────────────────────────────

    pub const CacheError = error{
        ReadFailed,
        WriteFailed,
    };

    /// Zwróć wskaźnik do danych bloku (read-only).
    pub fn readBlock(self: *BlockCache, lba: u64) CacheError!*const [BLOCK_SIZE]u8 {
        if (self.findLine(lba)) |idx| {
            self.promoteToFront(idx);
            return &self.lines[idx].data;
        }
        // Cache miss — evict + load
        const idx = self.evict();
        const l = &self.lines[idx];
        l.lba = lba;
        l.valid = true;
        l.dirty = false;
        self.mmioRead(lba, &l.data);
        self.pushFront(idx);
        return &l.data;
    }

    /// Zwróć mutable slice do bloku — caller musi wywołać markDirty().
    pub fn getWritable(self: *BlockCache, lba: u64) CacheError!*[BLOCK_SIZE]u8 {
        if (self.findLine(lba)) |idx| {
            self.promoteToFront(idx);
            return &self.lines[idx].data;
        }
        const idx = self.evict();
        const l = &self.lines[idx];
        l.lba = lba;
        l.valid = true;
        l.dirty = false;
        self.mmioRead(lba, &l.data);
        self.pushFront(idx);
        return &l.data;
    }

    pub fn markDirty(self: *BlockCache, lba: u64) void {
        if (self.findLine(lba)) |idx| {
            self.lines[idx].dirty = true;
        }
    }

    /// Flush wszystkich dirty bloków na dysk.
    pub fn flushAll(self: *BlockCache) void {
        for (&self.lines, 0..) |*l, i| {
            if (l.valid and l.dirty) {
                self.writebackLine(@intCast(i)) catch {};
            }
        }
    }

    /// Invalidate (wyrzuć z cache bez writeback) — np. po unmount.
    pub fn invalidateAll(self: *BlockCache) void {
        for (&self.lines) |*l| {
            l.valid = false;
            l.dirty = false;
        }
    }
};
