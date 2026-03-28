// inode.zig — unified inode, FS-agnostic
//
// Każdy FS (FAT32 / ext2-like / CSFS) mapuje swoje struktury na ten typ.
// Kernel/userspace nie wie z jakiego FS pochodzi inode — VFS to ukrywa.

const std = @import("std");

// ─── Stałe ────────────────────────────────────────────────────────────────────

pub const MAX_NAME_LEN: usize = 255;
pub const MAX_DIRECT: usize = 12; // direct block pointers
pub const INVALID_INO: u64 = 0;
pub const ROOT_INO: u64 = 1;

// ─── FileType ─────────────────────────────────────────────────────────────────

pub const FileType = enum(u8) {
    unknown = 0,
    regular = 1,
    directory = 2,
    symlink = 3,
    device = 4,
    pipe = 5,
    socket = 6,
};

// ─── Permissions ──────────────────────────────────────────────────────────────

pub const Perm = packed struct(u16) {
    other_x: bool = false,
    other_w: bool = false,
    other_r: bool = false,
    group_x: bool = false,
    group_w: bool = false,
    group_r: bool = false,
    owner_x: bool = false,
    owner_w: bool = false,
    owner_r: bool = false,
    sticky: bool = false,
    sgid: bool = false,
    suid: bool = false,
    _pad: u4 = 0,

    pub const OWNER_RW: Perm = .{ .owner_r = true, .owner_w = true };
    pub const ALL_R: Perm = .{ .owner_r = true, .group_r = true, .other_r = true };
    pub const DIR_DEFAULT: Perm = .{
        .owner_r = true,
        .owner_w = true,
        .owner_x = true,
        .group_r = true,
        .group_x = true,
        .other_r = true,
        .other_x = true,
    };
};

// ─── Timestamps (Unix seconds, u64 wystarczy do 2554 roku) ───────────────────

pub const Timestamps = struct {
    created: u64 = 0,
    modified: u64 = 0,
    accessed: u64 = 0,
};

// ─── BlockPointers ───────────────────────────────────────────────────────────
// Dla FAT32: direct[0] = pierwszy cluster, reszta nieużywana (FAT chain).
// Dla ext2/CSFS: classic direct + indirect scheme.

pub const BlockPointers = struct {
    direct: [MAX_DIRECT]u64 = [_]u64{0} ** MAX_DIRECT,
    indirect: u64 = 0, // blok z tablicą bloków
    dbl_indirect: u64 = 0, // blok z tablicą indirect bloków
};

// ─── Inode ────────────────────────────────────────────────────────────────────

pub const Inode = struct {
    ino: u64,
    ftype: FileType,
    perm: Perm,
    uid: u32,
    gid: u32,
    size: u64,
    nlinks: u32,
    ts: Timestamps,
    blocks: BlockPointers,

    // Które FS posiada ten inode — do dispatchu w VFS
    fs_tag: FsTag,
    // Opaque FS-specific data (max 64 bajty, unikamy alokacji)
    fs_data: FsPrivate,

    pub fn isDir(self: *const Inode) bool {
        return self.ftype == .directory;
    }

    pub fn isFile(self: *const Inode) bool {
        return self.ftype == .regular;
    }

    /// Czy inode jest poprawny (nie zerowy sentinel)
    pub fn isValid(self: *const Inode) bool {
        return self.ino != INVALID_INO;
    }
};

// ─── FsTag ───────────────────────────────────────────────────────────────────

pub const FsTag = enum(u8) {
    fat32 = 1,
    ext2 = 2,
    csfs = 3,
};

// ─── FsPrivate — FS-specific opaque data per inode ───────────────────────────

pub const FsPrivate = extern union {
    fat32: Fat32Private,
    ext2: Ext2Private,
    csfs: CsfsPrivate,
    raw: [64]u8,
};

pub const Fat32Private = extern struct {
    first_cluster: u32,
    dir_entry_cluster: u32, // cluster katalogu zawierającego ten wpis
    dir_entry_offset: u32, // offset w katalogu
    _pad: [52]u8,
};

pub const Ext2Private = extern struct {
    raw_inode_num: u32, // numer inode na dysku (może różnić się od ino)
    group: u32, // numer block group
    _pad: [56]u8,
};

pub const CsfsPrivate = extern struct {
    extent_tree_root: u64, // blok z korzeniem extent tree
    flags: u32,
    _pad: [52]u8,
};

// ─── DirEntry — wpis w katalogu ───────────────────────────────────────────────

pub const DirEntry = struct {
    ino: u64,
    ftype: FileType,
    name: [MAX_NAME_LEN + 1]u8, // null-terminated
    name_len: u16,

    pub fn getName(self: *const DirEntry) []const u8 {
        return self.name[0..self.name_len];
    }

    pub fn setName(self: *DirEntry, name: []const u8) void {
        const len = @min(name.len, MAX_NAME_LEN);
        @memcpy(self.name[0..len], name[0..len]);
        self.name[len] = 0;
        self.name_len = @intCast(len);
    }
};
