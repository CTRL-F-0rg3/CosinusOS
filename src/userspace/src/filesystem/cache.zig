// cache.zig — LRU block cache
//
// Sits between fs.zig and block.zig.
// All hardware access goes through BlockDevice.readSectors / writeSectors
// which call into the Odin ATA driver — no direct MMIO here.

const block = @import("block.zig");

pub const CACHE_LINES: usize = 256; // 256 * 512 = 128 KB resident
pub const BLOCK_SIZE: usize = 512;

// ── CacheLine ─────────────────────────────────────────────────────────────────

const INVALID_IDX: u16 = 0xFFFF;

const CacheLine = struct {
    lba: u64,
    dirty: bool,
    valid: bool,
    data: [BLOCK_SIZE]u8,
    prev: u16,
    next: u16,
};

// ── BlockCache ────────────────────────────────────────────────────────────────

pub const CacheError = error{
    ReadFailed,
    WriteFailed,
};

pub const BlockCache = struct {
    lines: [CACHE_LINES]CacheLine,
    lru_head: u16,
    lru_tail: u16,
    dev: block.BlockDevice,
    hits: u64, // stats
    misses: u64,

    pub fn init(dev: block.BlockDevice) BlockCache {
        var c = BlockCache{
            .lines = undefined,
            .lru_head = INVALID_IDX,
            .lru_tail = INVALID_IDX,
            .dev = dev,
            .hits = 0,
            .misses = 0,
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
        c.lru_head = 0;
        c.lru_tail = CACHE_LINES - 1;
        return c;
    }

    // ── LRU list ──────────────────────────────────────────────────────────────

    fn findLine(self: *BlockCache, lba: u64) ?u16 {
        for (&self.lines, 0..) |*l, i| {
            if (l.valid and l.lba == lba) return @intCast(i);
        }
        return null;
    }

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

    fn evict(self: *BlockCache) CacheError!u16 {
        const idx = self.lru_tail;
        if (self.lines[idx].dirty) {
            try self.writebackLine(idx);
        }
        self.lines[idx].valid = false;
        self.lines[idx].dirty = false;
        self.detach(idx);
        return idx;
    }

    // ── Physical I/O — goes through block.zig → Odin driver ──────────────────

    fn writebackLine(self: *BlockCache, idx: u16) CacheError!void {
        const l = &self.lines[idx];
        self.dev.writeSector(l.lba, &l.data) catch return CacheError.WriteFailed;
        l.dirty = false;
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Returns read-only pointer to cached sector data.
    pub fn readBlock(self: *BlockCache, lba: u64) CacheError!*const [BLOCK_SIZE]u8 {
        if (self.findLine(lba)) |idx| {
            self.promoteToFront(idx);
            self.hits += 1;
            return &self.lines[idx].data;
        }

        self.misses += 1;
        const idx = try self.evict();
        const l = &self.lines[idx];
        l.lba = lba;
        l.valid = true;
        l.dirty = false;

        self.dev.readSector(lba, &l.data) catch return CacheError.ReadFailed;

        self.pushFront(idx);
        return &l.data;
    }

    /// Returns mutable pointer to cached sector; caller must call markDirty().
    pub fn getWritable(self: *BlockCache, lba: u64) CacheError!*[BLOCK_SIZE]u8 {
        if (self.findLine(lba)) |idx| {
            self.promoteToFront(idx);
            return &self.lines[idx].data;
        }
        const idx = try self.evict();
        const l = &self.lines[idx];
        l.lba = lba;
        l.valid = true;
        l.dirty = false;

        self.dev.readSector(lba, &l.data) catch return CacheError.ReadFailed;

        self.pushFront(idx);
        return &l.data;
    }

    pub fn markDirty(self: *BlockCache, lba: u64) void {
        if (self.findLine(lba)) |idx| {
            self.lines[idx].dirty = true;
        }
    }

    /// Flush all dirty sectors to disk via block.zig.
    pub fn flushAll(self: *BlockCache) void {
        for (&self.lines, 0..) |*l, i| {
            if (l.valid and l.dirty) {
                self.writebackLine(@intCast(i)) catch {};
            }
        }
    }

    pub fn invalidateAll(self: *BlockCache) void {
        for (&self.lines) |*l| {
            l.valid = false;
            l.dirty = false;
        }
    }

    /// Cache hit rate as integer percentage (0–100).
    pub fn hitRate(self: *const BlockCache) u64 {
        const total = self.hits + self.misses;
        if (total == 0) return 0;
        return self.hits * 100 / total;
    }
};
