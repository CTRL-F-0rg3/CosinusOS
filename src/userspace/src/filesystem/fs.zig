// fs.zig — VFS + implementacje FAT32 / ext2-like / CSFS
//
// VFS = tagged union dispatch: każde wywołanie idzie do właściwego drivera.
// Nie ma vtable ani heap allocation — comptime dispatch przez switch na FsTag.

const std = @import("std");
const inode = @import("inode.zig");
const cache = @import("cache.zig");
const io = @import("io.zig");
const utils = @import("utils.zig");

pub const FsError = error{
    NotFound,
    NotADirectory,
    NotAFile,
    AlreadyExists,
    NoSpace,
    Corrupt,
    ReadOnly,
    InvalidArgument,
    NotSupported,
};

// =============================================================================
//  VFS — wspólny interfejs
// =============================================================================

pub const Filesystem = union(inode.FsTag) {
    fat32: Fat32,
    ext2: Ext2,
    csfs: Csfs,

    pub fn lookup(
        self: *Filesystem,
        parent: *const inode.Inode,
        name: []const u8,
        c: *cache.BlockCache,
    ) FsError!inode.Inode {
        return switch (self.*) {
            .fat32 => |*f| f.lookup(parent, name, c),
            .ext2 => |*f| f.lookup(parent, name, c),
            .csfs => |*f| f.lookup(parent, name, c),
        };
    }

    pub fn readDir(
        self: *Filesystem,
        dir: *const inode.Inode,
        c: *cache.BlockCache,
        cb: *const fn (*const inode.DirEntry, ?*anyopaque) bool,
        userdata: ?*anyopaque,
    ) FsError!void {
        return switch (self.*) {
            .fat32 => |*f| f.readDir(dir, c, cb, userdata),
            .ext2 => |*f| f.readDir(dir, c, cb, userdata),
            .csfs => |*f| f.readDir(dir, c, cb, userdata),
        };
    }

    pub fn getRoot(self: *Filesystem, c: *cache.BlockCache) FsError!inode.Inode {
        return switch (self.*) {
            .fat32 => |*f| f.getRoot(c),
            .ext2 => |*f| f.getRoot(c),
            .csfs => |*f| f.getRoot(c),
        };
    }

    pub fn read(
        self: *Filesystem,
        in: *const inode.Inode,
        offset: u64,
        buf: []u8,
        c: *cache.BlockCache,
    ) FsError!usize {
        return switch (self.*) {
            .fat32 => |*f| f.read(in, offset, buf, c),
            .ext2 => |*f| f.read(in, offset, buf, c),
            .csfs => |*f| f.read(in, offset, buf, c),
        };
    }
};

// =============================================================================
//  FAT32
// =============================================================================
//
//  Layout dysku:
//    LBA 0:    Boot Record (BPB)
//    LBA 1:    FS Info sector
//    LBA fat_start .. fat_start+sectors_per_fat*num_fats-1: FAT tables
//    LBA data_start ..: Cluster 2 = root directory, reszta = data
//
//  Superblock parsujemy z BPB przy mount.

pub const Fat32Sb = struct {
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    fat_size_32: u32,
    root_cluster: u32,
    // Wyliczone
    fat_lba: u64,
    data_lba: u64,
    cluster_size: u32, // bajty
};

pub const Fat32 = struct {
    sb: Fat32Sb,
    lba_base: u64, // LBA początku partycji

    pub fn mount(lba_base: u64, c: *cache.BlockCache) FsError!Fat32 {
        const boot = c.readBlock(lba_base) catch return FsError.Corrupt;

        // BPB offsets (FAT32 standard)
        const bytes_per_sector = utils.readU16LE(boot, 11);
        const sectors_per_cluster = boot[13];
        const reserved_sectors = utils.readU16LE(boot, 14);
        const num_fats = boot[16];
        const fat_size_32 = utils.readU32LE(boot, 36);
        const root_cluster = utils.readU32LE(boot, 44);

        // Sprawdź signature
        if (boot[510] != 0x55 or boot[511] != 0xAA) return FsError.Corrupt;

        const fat_lba = lba_base + reserved_sectors;
        const data_lba = fat_lba + @as(u64, num_fats) * fat_size_32;

        return Fat32{
            .lba_base = lba_base,
            .sb = .{
                .bytes_per_sector = bytes_per_sector,
                .sectors_per_cluster = sectors_per_cluster,
                .reserved_sectors = reserved_sectors,
                .num_fats = num_fats,
                .fat_size_32 = fat_size_32,
                .root_cluster = root_cluster,
                .fat_lba = fat_lba,
                .data_lba = data_lba,
                .cluster_size = @as(u32, sectors_per_cluster) * bytes_per_sector,
            },
        };
    }

    fn clusterToLba(self: *const Fat32, cluster: u32) u64 {
        const spc = self.sb.sectors_per_cluster;
        return self.sb.data_lba + @as(u64, cluster - 2) * spc;
    }

    fn nextCluster(self: *const Fat32, cluster: u32, c: *cache.BlockCache) FsError!u32 {
        const fat_byte = @as(u64, cluster) * 4;
        const sec = self.sb.fat_lba + fat_byte / 512;
        const off = @as(usize, @intCast(fat_byte % 512));
        const data = c.readBlock(sec) catch return FsError.Corrupt;
        return utils.readU32LE(data, off) & 0x0FFFFFFF;
    }

    fn makeFat32Inode(self: *const Fat32, cluster: u32, is_dir: bool, size: u32) inode.Inode {
        var in: inode.Inode = undefined;
        in.ino = cluster;
        in.ftype = if (is_dir) .directory else .regular;
        in.perm = if (is_dir) inode.Perm.DIR_DEFAULT else inode.Perm.OWNER_RW;
        in.uid = 0;
        in.gid = 0;
        in.size = size;
        in.nlinks = 1;
        in.ts = .{};
        in.fs_tag = .fat32;
        in.fs_data = .{ .fat32 = .{
            .first_cluster = cluster,
            .dir_entry_cluster = 0,
            .dir_entry_offset = 0,
            ._pad = [_]u8{0} ** 52,
        } };
        in.blocks = .{};
        in.blocks.direct[0] = self.clusterToLba(cluster);
        return in;
    }

    pub fn getRoot(self: *Fat32, _: *cache.BlockCache) FsError!inode.Inode {
        return self.makeFat32Inode(self.sb.root_cluster, true, 0);
    }

    pub fn lookup(
        self: *Fat32,
        parent: *const inode.Inode,
        name: []const u8,
        c: *cache.BlockCache,
    ) FsError!inode.Inode {
        var result: ?inode.Inode = null;
        // Pełna implementacja lookup przez readDir
        try self.readDir(parent, c, &fat32LookupCb, @ptrCast(@constCast(&FatLookupState{
            .name = name,
            .result = &result,
            .fat = self,
        })));
        return result orelse FsError.NotFound;
    }

    pub fn readDir(
        self: *Fat32,
        dir: *const inode.Inode,
        c: *cache.BlockCache,
        cb: *const fn (*const inode.DirEntry, ?*anyopaque) bool,
        ud: ?*anyopaque,
    ) FsError!void {
        const cluster = dir.fs_data.fat32.first_cluster;
        var cur_cluster = cluster;
        const EOC: u32 = 0x0FFFFFF8;

        while (cur_cluster < EOC and cur_cluster >= 2) {
            const lba = self.clusterToLba(cur_cluster);
            const spc = self.sb.sectors_per_cluster;

            var sec: u8 = 0;
            while (sec < spc) : (sec += 1) {
                const data = c.readBlock(lba + sec) catch return FsError.Corrupt;
                var off: usize = 0;
                while (off + 32 <= 512) : (off += 32) {
                    const first = data[off];
                    if (first == 0x00) return; // koniec katalogu
                    if (first == 0xE5) continue; // usunięty wpis
                    if (data[off + 11] == 0x0F) continue; // LFN wpis (skip)

                    var entry: inode.DirEntry = undefined;
                    // Dekoduj 8.3 name
                    var name_buf: [13]u8 = undefined;
                    const nlen = utils.fat83ToString(data[off .. off + 11][0..11], &name_buf);
                    entry.setName(name_buf[0..nlen]);

                    const attr = data[off + 11];
                    const is_dir = (attr & 0x10) != 0;
                    entry.ftype = if (is_dir) .directory else .regular;

                    const cluster_hi = utils.readU16LE(data, off + 20);
                    const cluster_lo = utils.readU16LE(data, off + 26);
                    const file_cluster = (@as(u32, cluster_hi) << 16) | cluster_lo;
                    entry.ino = file_cluster;

                    if (!cb(&entry, ud)) return;
                }
            }
            cur_cluster = try self.nextCluster(cur_cluster, c);
        }
    }

    pub fn read(
        self: *Fat32,
        in: *const inode.Inode,
        offset: u64,
        buf: []u8,
        c: *cache.BlockCache,
    ) FsError!usize {
        if (offset >= in.size) return 0;
        const to_read = @min(buf.len, in.size - offset);

        var cur_cluster = in.fs_data.fat32.first_cluster;
        const cluster_size = self.sb.cluster_size;
        const EOC: u32 = 0x0FFFFFF8;

        // Skip klastrów do offset
        var skip = offset / cluster_size;
        while (skip > 0 and cur_cluster < EOC) : (skip -= 1) {
            cur_cluster = try self.nextCluster(cur_cluster, c);
        }

        var read_total: usize = 0;
        var f_off = offset % cluster_size;

        while (read_total < to_read and cur_cluster >= 2 and cur_cluster < EOC) {
            const lba = self.clusterToLba(cur_cluster);
            const spc = self.sb.sectors_per_cluster;
            var sec_off = f_off / 512;
            var byte_off = f_off % 512;

            while (sec_off < spc and read_total < to_read) {
                const chunk = @min(to_read - read_total, 512 - byte_off);
                const data = c.readBlock(lba + sec_off) catch return FsError.Corrupt;
                @memcpy(buf[read_total .. read_total + chunk], data[byte_off .. byte_off + chunk]);
                read_total += chunk;
                byte_off = 0;
                sec_off += 1;
            }
            f_off = 0;
            cur_cluster = try self.nextCluster(cur_cluster, c);
        }

        return read_total;
    }
};

// Helper types dla FAT lookup
const FatLookupState = struct {
    name: []const u8,
    result: *?inode.Inode,
    fat: *Fat32,
};

fn fat32LookupCb(entry: *const inode.DirEntry, ud: ?*anyopaque) bool {
    const state: *FatLookupState = @ptrCast(@alignCast(ud));
    if (utils.streqCaseInsensitive(entry.getName(), state.name)) {
        const cluster: u32 = @intCast(entry.ino);
        state.result.* = state.fat.makeFat32Inode(
            cluster,
            entry.ftype == .directory,
            0, // rozmiar nieznany z DirEntry bez dodatkowego odczytu
        );
        return false;
    }
    return true;
}

// =============================================================================
//  Ext2-like (CosinusExt)
// =============================================================================
//
//  Layout: superblock @ LBA 2 (bajt 1024), block groups, inode tables.
//  Uproszczenia względem ext2: brak journalu, 64-bit block numbers.

pub const Ext2Sb = struct {
    inode_count: u32,
    block_count: u64,
    free_blocks: u64,
    free_inodes: u32,
    first_data_block: u32,
    block_size: u32, // 1024 << log_block_size
    blocks_per_group: u32,
    inodes_per_group: u32,
    inode_size: u16,
    magic: u16,
    lba_base: u64,
};

const EXT2_MAGIC: u16 = 0xEF53;
const EXT2_SB_LBA: u64 = 2; // superblock @ offset 1024 w sektorze 2
const EXT2_SB_OFFSET: usize = 0; // już wyrównany do sektora

pub const Ext2 = struct {
    sb: Ext2Sb,

    pub fn mount(lba_base: u64, c: *cache.BlockCache) FsError!Ext2 {
        // Superblock jest w bajcie 1024 od początku FS = LBA base+2 (przy 512B/sektor)
        const data = c.readBlock(lba_base + EXT2_SB_LBA) catch return FsError.Corrupt;

        const magic = utils.readU16LE(data, 56);
        if (magic != EXT2_MAGIC) return FsError.Corrupt;

        const log_bs = utils.readU32LE(data, 24);
        const block_size: u32 = @as(u32, 1024) << @intCast(log_bs);

        return Ext2{ .sb = .{
            .inode_count = utils.readU32LE(data, 0),
            .block_count = utils.readU32LE(data, 4),
            .free_blocks = utils.readU32LE(data, 12),
            .free_inodes = utils.readU32LE(data, 16),
            .first_data_block = utils.readU32LE(data, 20),
            .block_size = block_size,
            .blocks_per_group = utils.readU32LE(data, 32),
            .inodes_per_group = utils.readU32LE(data, 40),
            .inode_size = utils.readU16LE(data, 88),
            .magic = magic,
            .lba_base = lba_base,
        } };
    }

    fn sectorsPerBlock(self: *const Ext2) u64 {
        return self.sb.block_size / 512;
    }

    fn blockToLba(self: *const Ext2, block_num: u64) u64 {
        return self.sb.lba_base + block_num * self.sectorsPerBlock();
    }

    fn groupDescLba(self: *const Ext2, group: u32) u64 {
        // Block group descriptor table jest w bloku po superbloku
        const sb_block = if (self.sb.block_size == 1024) 1 else 0;
        return self.blockToLba(sb_block + 1) + @as(u64, group) * 32 / 512;
    }

    pub fn readInode(self: *Ext2, ino: u32, c: *cache.BlockCache) FsError!inode.Inode {
        if (ino == 0) return FsError.InvalidArgument;

        const group = (ino - 1) / self.sb.inodes_per_group;
        const local = (ino - 1) % self.sb.inodes_per_group;

        // Odczytaj group descriptor → inode table block
        const gd_lba = self.groupDescLba(group);
        const gd_off = @as(usize, @intCast((@as(u64, group) * 32) % 512));
        const gd_data = c.readBlock(gd_lba) catch return FsError.Corrupt;
        const it_block = utils.readU32LE(gd_data, gd_off + 8); // inode table block

        // Inode w tablicy
        const inode_byte_off = @as(u64, local) * self.sb.inode_size;
        const inode_lba = self.blockToLba(it_block) + inode_byte_off / 512;
        const inode_off = @as(usize, @intCast(inode_byte_off % 512));

        const idata = c.readBlock(inode_lba) catch return FsError.Corrupt;

        // Parsuj raw ext2 inode
        const mode = utils.readU16LE(idata, inode_off + 0);
        const size_lo = utils.readU32LE(idata, inode_off + 4);
        const size_hi = utils.readU32LE(idata, inode_off + 108);
        const size = @as(u64, size_hi) << 32 | size_lo;

        const ftype: inode.FileType = switch (mode & 0xF000) {
            0x8000 => .regular,
            0x4000 => .directory,
            0xA000 => .symlink,
            else => .unknown,
        };

        var in: inode.Inode = undefined;
        in.ino = ino;
        in.ftype = ftype;
        in.perm = @bitCast(@as(u16, mode & 0x0FFF));
        in.uid = utils.readU16LE(idata, inode_off + 2);
        in.gid = utils.readU16LE(idata, inode_off + 24);
        in.size = size;
        in.nlinks = utils.readU16LE(idata, inode_off + 26);
        in.ts = .{
            .accessed = utils.readU32LE(idata, inode_off + 8),
            .created = utils.readU32LE(idata, inode_off + 12),
            .modified = utils.readU32LE(idata, inode_off + 16),
        };
        in.fs_tag = .ext2;
        in.fs_data = .{ .ext2 = .{ .raw_inode_num = ino, .group = group, ._pad = [_]u8{0} ** 56 } };

        // Block pointers (ext2 direct: offsets 40–87 w raw inode, 12×4 bajtów)
        for (0..inode.MAX_DIRECT) |i| {
            in.blocks.direct[i] = utils.readU32LE(idata, inode_off + 40 + i * 4);
        }
        in.blocks.indirect = utils.readU32LE(idata, inode_off + 88);
        in.blocks.dbl_indirect = utils.readU32LE(idata, inode_off + 92);

        return in;
    }

    pub fn getRoot(self: *Ext2, c: *cache.BlockCache) FsError!inode.Inode {
        return self.readInode(inode.ROOT_INO, c);
    }

    pub fn lookup(
        self: *Ext2,
        parent: *const inode.Inode,
        name: []const u8,
        c: *cache.BlockCache,
    ) FsError!inode.Inode {
        var found_ino: u32 = 0;
        const state = Ext2LookupState{ .name = name, .found = &found_ino };
        try self.readDir(parent, c, &ext2LookupCb, @ptrCast(@constCast(&state)));
        if (found_ino == 0) return FsError.NotFound;
        return self.readInode(found_ino, c);
    }

    pub fn readDir(
        _: *Ext2,
        dir: *const inode.Inode,
        c: *cache.BlockCache,
        cb: *const fn (*const inode.DirEntry, ?*anyopaque) bool,
        ud: ?*anyopaque,
    ) FsError!void {
        if (!dir.isDir()) return FsError.NotADirectory;

        var file_off: u64 = 0;
        while (file_off < dir.size) {
            var buf: [512]u8 = undefined;
            const n = io.readFile(dir, file_off, &buf, c) catch return FsError.Corrupt;
            if (n == 0) break;

            var off: usize = 0;
            while (off + 8 <= n) {
                const de_ino = utils.readU32LE(buf[0..], off);
                const rec_len = utils.readU16LE(buf[0..], off + 4);
                const name_len = buf[off + 6];
                if (rec_len < 8 or off + rec_len > n) break;

                if (de_ino != 0 and name_len > 0) {
                    var entry: inode.DirEntry = undefined;
                    entry.ino = de_ino;
                    entry.ftype = switch (buf[off + 7]) {
                        1 => .regular,
                        2 => .directory,
                        7 => .symlink,
                        else => .unknown,
                    };
                    entry.setName(buf[off + 8 .. off + 8 + name_len]);
                    if (!cb(&entry, ud)) return;
                }
                off += rec_len;
            }
            file_off += n;
        }
    }

    pub fn read(
        _: *Ext2,
        in: *const inode.Inode,
        offset: u64,
        buf: []u8,
        c: *cache.BlockCache,
    ) FsError!usize {
        return io.readFile(in, offset, buf, c) catch return FsError.Corrupt;
    }
};

const Ext2LookupState = struct { name: []const u8, found: *u32 };
fn ext2LookupCb(entry: *const inode.DirEntry, ud: ?*anyopaque) bool {
    const state: *Ext2LookupState = @ptrCast(@alignCast(ud));
    if (utils.streq(entry.getName(), state.name)) {
        state.found.* = @intCast(entry.ino);
        return false;
    }
    return true;
}

// =============================================================================
//  CSFS — Cosinus File System
// =============================================================================
//
//  Własny FS zaprojektowany pod CosinusOS.
//  Superblock @ LBA 0, inode table fixed-size, extent-based bloki.
//
//  Superblock (512 bajtów):
//    0x00  u32  magic    = 0xC05F5500
//    0x04  u32  version  = 1
//    0x08  u64  block_count
//    0x10  u64  inode_count
//    0x18  u64  inode_table_lba
//    0x20  u64  data_bitmap_lba
//    0x28  u64  root_ino
//    0x30  u16  inode_size  (256)
//    ...

pub const CSFS_MAGIC: u32 = 0xC05F5500;
pub const CSFS_INO_SIZE: u64 = 256;
pub const CSFS_ROOT_INO: u64 = 1;

pub const CsfsSb = struct {
    magic: u32,
    version: u32,
    block_count: u64,
    inode_count: u64,
    inode_table_lba: u64,
    data_bitmap_lba: u64,
    root_ino: u64,
    inode_size: u16,
    lba_base: u64,
};

pub const Csfs = struct {
    sb: CsfsSb,

    pub fn mount(lba_base: u64, c: *cache.BlockCache) FsError!Csfs {
        const data = c.readBlock(lba_base) catch return FsError.Corrupt;

        const magic = utils.readU32LE(data, 0);
        if (magic != CSFS_MAGIC) return FsError.Corrupt;

        return Csfs{ .sb = .{
            .magic = magic,
            .version = utils.readU32LE(data, 4),
            .block_count = utils.readU64LE(data, 8),
            .inode_count = utils.readU64LE(data, 16),
            .inode_table_lba = utils.readU64LE(data, 24),
            .data_bitmap_lba = utils.readU64LE(data, 32),
            .root_ino = utils.readU64LE(data, 40),
            .inode_size = utils.readU16LE(data, 48),
            .lba_base = lba_base,
        } };
    }

    pub fn readInode(self: *const Csfs, ino: u64, c: *cache.BlockCache) FsError!inode.Inode {
        const byte_off = (ino - 1) * CSFS_INO_SIZE;
        const lba = self.sb.inode_table_lba + byte_off / 512;
        const off = @as(usize, @intCast(byte_off % 512));

        const data = c.readBlock(lba) catch return FsError.Corrupt;

        const ftype_raw = data[off + 0];
        const ftype: inode.FileType = @enumFromInt(ftype_raw);
        const perm_raw = utils.readU16LE(data, off + 2);
        const size = utils.readU64LE(data, off + 8);
        const uid = utils.readU32LE(data, off + 16);
        const gid = utils.readU32LE(data, off + 20);
        const nlinks = utils.readU32LE(data, off + 24);
        const ts_c = utils.readU64LE(data, off + 32);
        const ts_m = utils.readU64LE(data, off + 40);
        const ts_a = utils.readU64LE(data, off + 48);

        var in: inode.Inode = undefined;
        in.ino = ino;
        in.ftype = ftype;
        in.perm = @bitCast(perm_raw);
        in.uid = uid;
        in.gid = gid;
        in.size = size;
        in.nlinks = nlinks;
        in.ts = .{ .created = ts_c, .modified = ts_m, .accessed = ts_a };
        in.fs_tag = .csfs;
        in.fs_data = .{ .csfs = .{
            .extent_tree_root = utils.readU64LE(data, off + 64),
            .flags = utils.readU32LE(data, off + 72),
            ._pad = [_]u8{0} ** 52,
        } };

        // Bezpośrednie bloki @ offset 80 (12 × 8 bajtów)
        for (0..inode.MAX_DIRECT) |i| {
            in.blocks.direct[i] = utils.readU64LE(data, off + 80 + i * 8);
        }
        in.blocks.indirect = utils.readU64LE(data, off + 176);
        in.blocks.dbl_indirect = utils.readU64LE(data, off + 184);

        return in;
    }

    pub fn getRoot(self: *Csfs, c: *cache.BlockCache) FsError!inode.Inode {
        return self.readInode(self.sb.root_ino, c);
    }

    pub fn lookup(
        self: *Csfs,
        parent: *const inode.Inode,
        name: []const u8,
        c: *cache.BlockCache,
    ) FsError!inode.Inode {
        var found_ino: u64 = 0;
        const state = CsfsLookupState{ .name = name, .found = &found_ino };
        try self.readDir(parent, c, &csfsLookupCb, @ptrCast(@constCast(&state)));
        if (found_ino == 0) return FsError.NotFound;
        return self.readInode(found_ino, c);
    }

    pub fn readDir(
        _: *Csfs,
        dir: *const inode.Inode,
        c: *cache.BlockCache,
        cb: *const fn (*const inode.DirEntry, ?*anyopaque) bool,
        ud: ?*anyopaque,
    ) FsError!void {
        if (!dir.isDir()) return FsError.NotADirectory;

        // CSFS dir entry format (fixed 272 bajtów):
        //   0x00  u64  ino
        //   0x08  u8   ftype
        //   0x09  u8   name_len
        //   0x0A  u8[256] name
        const ENTRY_SIZE: u64 = 272;
        const entries = dir.size / ENTRY_SIZE;

        var i: u64 = 0;
        while (i < entries) : (i += 1) {
            var raw: [272]u8 = undefined;
            _ = io.readFile(dir, i * ENTRY_SIZE, &raw, c) catch return FsError.Corrupt;

            const de_ino = utils.readU64LE(&raw, 0);
            if (de_ino == 0) continue;

            var entry: inode.DirEntry = undefined;
            entry.ino = de_ino;
            entry.ftype = @enumFromInt(raw[8]);
            const nlen = raw[9];
            entry.setName(raw[10 .. 10 + nlen]);

            if (!cb(&entry, ud)) return;
        }
    }

    pub fn read(
        _: *Csfs,
        in: *const inode.Inode,
        offset: u64,
        buf: []u8,
        c: *cache.BlockCache,
    ) FsError!usize {
        return io.readFile(in, offset, buf, c) catch return FsError.Corrupt;
    }
};

const CsfsLookupState = struct { name: []const u8, found: *u64 };
fn csfsLookupCb(entry: *const inode.DirEntry, ud: ?*anyopaque) bool {
    const state: *CsfsLookupState = @ptrCast(@alignCast(ud));
    if (utils.streq(entry.getName(), state.name)) {
        state.found.* = entry.ino;
        return false;
    }
    return true;
}
