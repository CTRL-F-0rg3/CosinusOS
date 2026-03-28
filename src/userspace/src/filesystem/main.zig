// main.zig — Userspace FS Server (ring-3)
//
// Architektura:
//   1. Mount block device przez MMIO (block.zig)
//   2. Detekcja FS (FAT32 / ext2 / CSFS) i mount
//   3. Event loop — odbiera IPC requesty z kernela przez shared memory queue
//   4. Dispatch → operacja FS → odpowiedź przez IPC
//
// IPC protokół: cosinus int 0x80 syscall ABI
//   Kernel mapuje shared ring buffer do przestrzeni FS serwera.
//   Każdy request = FsRequest (32 bajty), response = FsResponse (32 bajty).

const block = @import("block.zig");
const cache = @import("cache.zig");
const fs = @import("fs.zig");
const inode = @import("inode.zig");
const utils = @import("utils.zig");
const io = @import("io.zig");

// ─── IPC struktury (muszą być identyczne z api.rs) ────────────────────────────

const FS_OP_OPEN: u8 = 1;
const FS_OP_READ: u8 = 2;
const FS_OP_WRITE: u8 = 3;
const FS_OP_CLOSE: u8 = 4;
const FS_OP_READDIR: u8 = 5;
const FS_OP_STAT: u8 = 6;
const FS_OP_MKDIR: u8 = 7;
const FS_OP_UNLINK: u8 = 8;

const FS_ERR_OK: i32 = 0;
const FS_ERR_NOTFOUND: i32 = -1;
const FS_ERR_NOTDIR: i32 = -2;
const FS_ERR_NOENT: i32 = -3;
const FS_ERR_IO: i32 = -4;
const FS_ERR_NOTSUP: i32 = -5;
const FS_ERR_BADFD: i32 = -6;

pub const FsRequest = extern struct {
    seq: u32, // sequence number — do matching request/response
    op: u8,
    flags: u8,
    fd: u16, // file descriptor (dla read/write/close)
    pid: u32, // PID procesu requestującego
    arg0: u64, // op-zależne: offset, ino, ...
    arg1: u64, // op-zależne: length, buf_addr, ...
    path_len: u16,
    _pad: [6]u8,
    // path follows inline w shared buffer (osobny wskaźnik)
};

pub const FsResponse = extern struct {
    seq: u32,
    status: i32, // FS_ERR_* lub >= 0 = bytes read/written
    ino: u64,
    size: u64,
    ftype: u8,
    _pad: [7]u8,
};

// ─── File Descriptor Table ────────────────────────────────────────────────────

const MAX_FDS: usize = 256;

const FdEntry = struct {
    used: bool,
    in: inode.Inode,
    offset: u64,
    flags: u32,
};

var fd_table: [MAX_FDS]FdEntry = [_]FdEntry{.{
    .used = false,
    .in = undefined,
    .offset = 0,
    .flags = 0,
}} ** MAX_FDS;

fn allocFd(in: inode.Inode, flags: u32) ?u16 {
    for (&fd_table, 0..) |*e, i| {
        if (!e.used) {
            e.* = .{ .used = true, .in = in, .offset = 0, .flags = flags };
            return @intCast(i);
        }
    }
    return null;
}

fn getFd(fd: u16) ?*FdEntry {
    if (fd >= MAX_FDS) return null;
    if (!fd_table[fd].used) return null;
    return &fd_table[fd];
}

fn freeFd(fd: u16) void {
    if (fd < MAX_FDS) fd_table[fd].used = false;
}

// ─── Mounted filesystem ───────────────────────────────────────────────────────

var g_fs: fs.Filesystem = undefined;
var g_cache: cache.BlockCache = undefined;
var g_mounted: bool = false;

// ─── Path resolution ──────────────────────────────────────────────────────────

fn resolve(path: []const u8) fs.FsError!inode.Inode {
    var current = try g_fs.getRoot(&g_cache);
    var iter = utils.PathIter.init(path);

    while (iter.next()) |component| {
        if (!current.isDir()) return fs.FsError.NotADirectory;
        current = try g_fs.lookup(&current, component, &g_cache);
    }

    return current;
}

// ─── Syscall helpers (no_std, CosinusOS ABI) ──────────────────────────────────

// Wbudowane syscalle przez int 0x80.
// Numery z cosinus-api/src/syscall.rs.
const SYS_IPC_RECV: u64 = 0x30;
const SYS_IPC_SEND: u64 = 0x31;
const SYS_YIELD: u64 = 0x10;
const SYS_EXIT: u64 = 0x01;

// Shared ring buffer addres — kernel mapuje to do naszego VAS przy starcie
const IPC_RING_ADDR: usize = 0x0000_7000_0000_0000;
const IPC_RING_SIZE: usize = 4096; // 4KB ring, 64 requestów po 64B
const IPC_PATH_ADDR: usize = IPC_RING_ADDR + IPC_RING_SIZE;
const IPC_PATH_SIZE: usize = 4096;

inline fn syscall2(num: u64, a0: u64, a1: u64) u64 {
    return asm volatile ("int $0x80"
        : [ret] "={rax}" (-> u64),
        : [num] "{rax}" (num),
          [a0] "{rdi}" (a0),
          [a1] "{rsi}" (a1),
        : .{ .memory = true, .rcx = true, .r11 = true }
    );
}

inline fn syscall0(num: u64) u64 {
    return asm volatile ("int $0x80"
        : [ret] "={rax}" (-> u64),
        : [num] "{rax}" (num),
        : .{ .memory = true, .rcx = true, .r11 = true }
    );
}

// ─── Request dispatch ─────────────────────────────────────────────────────────

fn handleOpen(req: *const FsRequest, path: []const u8) FsResponse {
    var resp = FsResponse{
        .seq = req.seq,
        .status = FS_ERR_NOTFOUND,
        .ino = 0,
        .size = 0,
        .ftype = 0,
        ._pad = [_]u8{0} ** 7,
    };

    const in = resolve(path) catch |err| {
        resp.status = switch (err) {
            fs.FsError.NotFound => FS_ERR_NOTFOUND,
            fs.FsError.NotADirectory => FS_ERR_NOTDIR,
            else => FS_ERR_IO,
        };
        return resp;
    };

    const fd = allocFd(in, @intCast(req.flags)) orelse {
        resp.status = FS_ERR_IO;
        return resp;
    };

    resp.status = @intCast(fd);
    resp.ino = in.ino;
    resp.size = in.size;
    resp.ftype = @intFromEnum(in.ftype);
    return resp;
}

fn handleRead(req: *const FsRequest, user_buf: []u8) FsResponse {
    var resp = FsResponse{ .seq = req.seq, .status = FS_ERR_BADFD, .ino = 0, .size = 0, .ftype = 0, ._pad = [_]u8{0} ** 7 };

    const entry = getFd(req.fd) orelse return resp;
    const count = @min(@as(usize, @intCast(req.arg1)), user_buf.len);

    const n = g_fs.read(&entry.in, entry.offset, user_buf[0..count], &g_cache) catch {
        resp.status = FS_ERR_IO;
        return resp;
    };

    entry.offset += n;
    resp.status = @intCast(n);
    return resp;
}

fn handleClose(req: *const FsRequest) FsResponse {
    freeFd(req.fd);
    return .{ .seq = req.seq, .status = FS_ERR_OK, .ino = 0, .size = 0, .ftype = 0, ._pad = [_]u8{0} ** 7 };
}

fn handleStat(req: *const FsRequest, path: []const u8) FsResponse {
    var resp = FsResponse{ .seq = req.seq, .status = FS_ERR_NOTFOUND, .ino = 0, .size = 0, .ftype = 0, ._pad = [_]u8{0} ** 7 };
    const in = resolve(path) catch return resp;
    resp.status = FS_ERR_OK;
    resp.ino = in.ino;
    resp.size = in.size;
    resp.ftype = @intFromEnum(in.ftype);
    return resp;
}

// ─── Mount — autodetekcja FS ──────────────────────────────────────────────────

fn autoMount(dev: block.BlockDevice) !void {
    g_cache = cache.BlockCache.init(dev);

    // Próbuj CSFS (własny magic @ LBA 0)
    if (fs.Csfs.mount(0, &g_cache)) |csfs| {
        g_fs = .{ .csfs = csfs };
        g_mounted = true;
        return;
    } else |_| {}

    // Próbuj FAT32 (signature 0x55AA @ offset 510)
    if (fs.Fat32.mount(0, &g_cache)) |fat| {
        g_fs = .{ .fat32 = fat };
        g_mounted = true;
        return;
    } else |_| {}

    // Próbuj ext2
    if (fs.Ext2.mount(0, &g_cache)) |ext| {
        g_fs = .{ .ext2 = ext };
        g_mounted = true;
        return;
    } else |_| {}

    return error.UnknownFilesystem;
}

// ─── Main — event loop ────────────────────────────────────────────────────────

pub fn main() noreturn {
    // 1. Init block device
    const dev = block.BlockDevice.init() catch {
        _ = syscall0(SYS_EXIT);
        unreachable;
    };

    // 2. Mount filesystem
    autoMount(dev) catch {
        _ = syscall0(SYS_EXIT);
        unreachable;
    };

    // 3. IPC ring pointers
    const ring: [*]volatile u8 = @ptrFromInt(IPC_RING_ADDR);
    const path_buf: [*]volatile u8 = @ptrFromInt(IPC_PATH_ADDR);
    // Shared read buffer dla danych (mapped przez kernel)
    const data_buf_addr: usize = IPC_PATH_ADDR + IPC_PATH_SIZE;
    const data_buf: [*]u8 = @ptrFromInt(data_buf_addr);
    const DATA_BUF_SIZE: usize = 65536; // 64KB transfer window

    var ring_read: usize = 0;
    const RING_CAP: usize = IPC_RING_SIZE / @sizeOf(FsRequest);

    // 4. Event loop
    while (true) {
        // Sprawdź czy jest nowy request (busy-wait → w docelowej wersji IRQ)
        const write_idx_ptr: *volatile u32 = @ptrCast(@alignCast(&ring[IPC_RING_SIZE - 8]));
        const write_idx = write_idx_ptr.*;

        if (@as(usize, write_idx) == ring_read) {
            // Brak requestów — yield do schedulera
            _ = syscall0(SYS_YIELD);
            continue;
        }

        // Odczytaj request
        const req_ptr: *const FsRequest = @ptrCast(@alignCast(&ring[ring_read * @sizeOf(FsRequest)]));
        const req = req_ptr.*;
        ring_read = (ring_read + 1) % RING_CAP;

        // Path ze shared buffer
        const path_len = @min(@as(usize, req.path_len), IPC_PATH_SIZE - 1);
        var path_copy: [256]u8 = undefined;
        for (0..path_len) |i| path_copy[i] = path_buf[i];
        const path = path_copy[0..path_len];

        // Dispatch
        const resp: FsResponse = switch (req.op) {
            FS_OP_OPEN => handleOpen(&req, path),
            FS_OP_READ => handleRead(&req, data_buf[0..DATA_BUF_SIZE]),
            FS_OP_CLOSE => handleClose(&req),
            FS_OP_STAT => handleStat(&req, path),
            else => FsResponse{ .seq = req.seq, .status = FS_ERR_NOTSUP, .ino = 0, .size = 0, .ftype = 0, ._pad = [_]u8{0} ** 7 },
        };

        // Wyślij odpowiedź przez IPC
        const resp_ptr: *volatile FsResponse = @ptrFromInt(IPC_RING_ADDR + IPC_RING_SIZE / 2);
        resp_ptr.* = resp;
        _ = syscall2(SYS_IPC_SEND, req.pid, req.seq);
    }
}
