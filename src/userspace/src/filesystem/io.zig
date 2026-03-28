// io.zig — I/O helpers nad cache
//
// Operacje wyższego poziomu: czytanie pliku po bajtach,
// zapis z automatycznym alokacją bloków, scatter-gather przez indirect ptrs.

const std = @import("std");
const cache = @import("cache.zig");
const inode = @import("inode.zig");
const utils = @import("utils.zig");

pub const BLOCK_SIZE = cache.BLOCK_SIZE;

pub const IoError = error{
    OutOfBounds,
    ReadError,
    WriteError,
    NotImplemented,
};

// ─── Flat block read/write (LBA bezpośredni) ──────────────────────────────────

/// Kopiuj `len` bajtów z LBA `lba`, offset `off` wewnątrz bloku → `buf`
pub fn readPartial(
    c: *cache.BlockCache,
    lba: u64,
    off: usize,
    buf: []u8,
) IoError!void {
    if (off + buf.len > BLOCK_SIZE) return IoError.OutOfBounds;
    const data = c.readBlock(lba) catch return IoError.ReadError;
    @memcpy(buf, data[off .. off + buf.len]);
}

/// Kopiuj `len` bajtów z `buf` → LBA `lba`, offset `off` wewnątrz bloku
pub fn writePartial(
    c: *cache.BlockCache,
    lba: u64,
    off: usize,
    buf: []const u8,
) IoError!void {
    if (off + buf.len > BLOCK_SIZE) return IoError.OutOfBounds;
    const data = c.getWritable(lba) catch return IoError.WriteError;
    @memcpy(data[off .. off + buf.len], buf);
    c.markDirty(lba);
}

/// Zeruj cały blok (dla nowo alokowanych bloków)
pub fn zeroBlock(c: *cache.BlockCache, lba: u64) IoError!void {
    const data = c.getWritable(lba) catch return IoError.WriteError;
    @memset(data, 0);
    c.markDirty(lba);
}

// ─── File I/O przez inode block pointers (ext2/CSFS style) ───────────────────
// Dla FAT32 ten schemat jest inny (cluster chain) — FAT driver obsługuje
// to osobno w fs.zig. Tutaj: bloki bezpośrednie + jeden indirect level.

/// Przelicz logiczny numer bloku pliku → fizyczny LBA.
/// Zwraca 0 jeśli blok nie jest zaalokowany (sparse file).
pub fn logicalToPhysical(
    in: *const inode.Inode,
    logical_blk: u64,
    c: *cache.BlockCache,
) IoError!u64 {
    const direct_count = inode.MAX_DIRECT;

    if (logical_blk < direct_count) {
        return in.blocks.direct[logical_blk];
    }

    // Single indirect
    const ptrs_per_block = BLOCK_SIZE / @sizeOf(u64);
    const indirect_idx = logical_blk - direct_count;

    if (indirect_idx < ptrs_per_block) {
        if (in.blocks.indirect == 0) return 0;
        const ind_block = c.readBlock(in.blocks.indirect) catch return IoError.ReadError;
        const ptr_off = indirect_idx * @sizeOf(u64);
        return utils.readU64LE(ind_block, ptr_off);
    }

    // Double indirect
    const dbl_idx = indirect_idx - ptrs_per_block;
    if (in.blocks.dbl_indirect == 0) return 0;

    const dbl_block = c.readBlock(in.blocks.dbl_indirect) catch return IoError.ReadError;
    const l1_idx = dbl_idx / ptrs_per_block;
    const l2_idx = dbl_idx % ptrs_per_block;
    const l1_lba = utils.readU64LE(dbl_block, l1_idx * @sizeOf(u64));
    if (l1_lba == 0) return 0;

    const l2_block = c.readBlock(l1_lba) catch return IoError.ReadError;
    return utils.readU64LE(l2_block, l2_idx * @sizeOf(u64));
}

/// Odczyt `buf.len` bajtów z pliku, startując od `file_offset`.
/// Obsługuje multi-block reads i partial blocks na początku/końcu.
pub fn readFile(
    in: *const inode.Inode,
    file_offset: u64,
    buf: []u8,
    c: *cache.BlockCache,
) IoError!usize {
    if (file_offset >= in.size) return 0;

    const to_read = @min(buf.len, in.size - file_offset);
    var remaining = to_read;
    var buf_off: usize = 0;
    var f_off: u64 = file_offset;

    while (remaining > 0) {
        const logical_blk = f_off / BLOCK_SIZE;
        const blk_off = @as(usize, @intCast(f_off % BLOCK_SIZE));
        const chunk = @min(remaining, BLOCK_SIZE - blk_off);

        const phys_lba = try logicalToPhysical(in, logical_blk, c);

        if (phys_lba == 0) {
            // Sparse — zero fill
            @memset(buf[buf_off .. buf_off + chunk], 0);
        } else {
            try readPartial(c, phys_lba, blk_off, buf[buf_off .. buf_off + chunk]);
        }

        buf_off += chunk;
        f_off += chunk;
        remaining -= chunk;
    }

    return to_read;
}

/// Zapis `buf` do pliku od `file_offset`.
/// Nie alokuje nowych bloków — zakłada, że są już zaalokowane przez FS driver.
pub fn writeFile(
    in: *const inode.Inode,
    file_offset: u64,
    buf: []const u8,
    c: *cache.BlockCache,
) IoError!usize {
    var remaining = buf.len;
    var buf_off: usize = 0;
    var f_off: u64 = file_offset;

    while (remaining > 0) {
        const logical_blk = f_off / BLOCK_SIZE;
        const blk_off = @as(usize, @intCast(f_off % BLOCK_SIZE));
        const chunk = @min(remaining, BLOCK_SIZE - blk_off);

        const phys_lba = try logicalToPhysical(in, logical_blk, c);
        if (phys_lba == 0) return IoError.WriteError; // nie zaalokowany

        try writePartial(c, phys_lba, blk_off, buf[buf_off .. buf_off + chunk]);

        buf_off += chunk;
        f_off += chunk;
        remaining -= chunk;
    }

    return buf.len;
}

// ─── Cluster chain reader (FAT32) ─────────────────────────────────────────────
// FAT32 używa klastrów (zwykle 4KB = 8 sektorów × 512B).
// Łańcuch klastrów jest w FAT table — tu dostarczamy helper dla fs.zig.

pub const ClusterChainIter = struct {
    fat_lba_start: u64, // LBA pierwszego sektora FAT
    sectors_per_fat: u32,
    cluster_size: u32, // bajty na klaster
    data_lba: u64, // LBA początku data region
    current: u32, // bieżący numer klastra

    const FAT32_EOC: u32 = 0x0FFFFFF8; // end-of-chain marker
    const FAT32_FREE: u32 = 0x00000000;

    pub fn init(
        fat_lba: u64,
        sectors_per_fat: u32,
        cluster_size: u32,
        data_lba: u64,
        first_cluster: u32,
    ) ClusterChainIter {
        return .{
            .fat_lba_start = fat_lba,
            .sectors_per_fat = sectors_per_fat,
            .cluster_size = cluster_size,
            .data_lba = data_lba,
            .current = first_cluster,
        };
    }

    /// Zwraca LBA bieżącego klastra i przesuwa do następnego.
    /// Zwraca null jeśli koniec łańcucha.
    pub fn next(self: *ClusterChainIter, c: *cache.BlockCache) IoError!?u64 {
        if (self.current >= FAT32_EOC or self.current < 2) return null;

        // LBA danych tego klastra
        const sectors_per_cluster = self.cluster_size / BLOCK_SIZE;
        const data_lba = self.data_lba + @as(u64, self.current - 2) * sectors_per_cluster;

        // Odczytaj następny klaster z FAT
        const fat_byte_off = @as(u64, self.current) * 4;
        const fat_sec = self.fat_lba_start + fat_byte_off / BLOCK_SIZE;
        const fat_off = @as(usize, @intCast(fat_byte_off % BLOCK_SIZE));

        const fat_data = c.readBlock(fat_sec) catch return IoError.ReadError;
        const next_cluster = utils.readU32LE(fat_data, fat_off) & 0x0FFFFFFF;
        self.current = next_cluster;

        return data_lba;
    }
};
