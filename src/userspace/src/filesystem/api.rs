// filesystem/api.rs — cosinus-api filesystem layer
//
// Trzy warstwy:
//   1. ABI typy (identyczne z main.zig FsRequest/FsResponse)
//   2. Syscall wrappers — userspace wysyła requesty do FS serwera
//   3. Kernel-side handler stubs — kernel dispatchu do FS serwera przez IPC
//
// Zasada: ZERO breaking changes. Każde rozszerzenie tylko addytywne.
// Nowe operacje → nowe stałe FS_OP_*, stare kody zostają na zawsze.

#![no_std]

// ─── FS opcodes (identyczne z main.zig) ──────────────────────────────────────

pub const FS_OP_OPEN:    u8 = 1;
pub const FS_OP_READ:    u8 = 2;
pub const FS_OP_WRITE:   u8 = 3;
pub const FS_OP_CLOSE:   u8 = 4;
pub const FS_OP_READDIR: u8 = 5;
pub const FS_OP_STAT:    u8 = 6;
pub const FS_OP_MKDIR:   u8 = 7;
pub const FS_OP_UNLINK:  u8 = 8;
// Addytywne rozszerzenia — zawsze na końcu
pub const FS_OP_RENAME:  u8 = 9;
pub const FS_OP_CHMOD:   u8 = 10;
pub const FS_OP_TRUNCATE: u8 = 11;

// ─── Error codes ─────────────────────────────────────────────────────────────

pub const FS_ERR_OK:       i32 = 0;
pub const FS_ERR_NOTFOUND: i32 = -1;
pub const FS_ERR_NOTDIR:   i32 = -2;
pub const FS_ERR_NOENT:    i32 = -3;
pub const FS_ERR_IO:       i32 = -4;
pub const FS_ERR_NOTSUP:   i32 = -5;
pub const FS_ERR_BADFD:    i32 = -6;
pub const FS_ERR_NOSPACE:  i32 = -7;
pub const FS_ERR_PERM:     i32 = -8;
pub const FS_ERR_EXIST:    i32 = -9;

// ─── Open flags ──────────────────────────────────────────────────────────────

pub const O_RDONLY: u32 = 0x0000;
pub const O_WRONLY: u32 = 0x0001;
pub const O_RDWR:   u32 = 0x0002;
pub const O_CREAT:  u32 = 0x0040;
pub const O_TRUNC:  u32 = 0x0200;
pub const O_APPEND: u32 = 0x0400;
pub const O_DIRECT: u32 = 0x4000;

// ─── FileType (identyczna z inode.zig) ───────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Unknown   = 0,
    Regular   = 1,
    Directory = 2,
    Symlink   = 3,
    Device    = 4,
    Pipe      = 5,
    Socket    = 6,
}

impl FileType {
    pub fn from_u8(v: u8) Self {
        match v {
            1 => Self::Regular,
            2 => Self::Directory,
            3 => Self::Symlink,
            4 => Self::Device,
            5 => Self::Pipe,
            6 => Self::Socket,
            _ => Self::Unknown,
        }
    }
}

// ─── ABI structs (repr C, identyczne z Zig) ──────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FsRequest {
    pub seq:      u32,
    pub op:       u8,
    pub flags:    u8,
    pub fd:       u16,
    pub pid:      u32,
    pub arg0:     u64,
    pub arg1:     u64,
    pub path_len: u16,
    pub _pad:     [6u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FsResponse {
    pub seq:    u32,
    pub status: i32,
    pub ino:    u64,
    pub size:   u64,
    pub ftype:  u8,
    pub _pad:   [7u8; 7],
}

impl FsResponse {
    pub fn is_ok(&self) -> bool {
        self.status >= 0
    }

    pub fn bytes_read(&self) -> Option<usize> {
        if self.status >= 0 { Some(self.status as usize) } else { None }
    }

    pub fn file_type(&self) -> FileType {
        FileType::from_u8(self.ftype)
    }
}

// ─── Stat ────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub ino:      u64,
    pub size:     u64,
    pub ftype:    u8,
    pub perm:     u16,
    pub uid:      u32,
    pub gid:      u32,
    pub nlinks:   u32,
    pub atime:    u64,
    pub mtime:    u64,
    pub ctime:    u64,
}

// ─── DirEntry (userspace facing) ─────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DirEntry {
    pub ino:      u64,
    pub ftype:    u8,
    pub name_len: u16,
    pub name:     [256u8; 256],
}

impl DirEntry {
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

// ─── IPC addresses (muszą zgadzać się z main.zig) ────────────────────────────

pub const IPC_RING_ADDR:  usize = 0x0000_7000_0000_0000;
pub const IPC_RING_SIZE:  usize = 4096;
pub const IPC_PATH_ADDR:  usize = IPC_RING_ADDR + IPC_RING_SIZE;
pub const IPC_PATH_SIZE:  usize = 4096;
pub const IPC_DATA_ADDR:  usize = IPC_PATH_ADDR + IPC_PATH_SIZE;
pub const IPC_DATA_SIZE:  usize = 65536;

// ─── Syscall numbers (CosinusOS ABI) ─────────────────────────────────────────

pub const SYS_IPC_RECV: u64 = 0x30;
pub const SYS_IPC_SEND: u64 = 0x31;
pub const SYS_YIELD:    u64 = 0x10;
pub const SYS_EXIT:     u64 = 0x01;

// ─── Low-level syscall ───────────────────────────────────────────────────────

#[inline(always)]
unsafe fn syscall2(num: u64, a0: u64, a1: u64) u64 {
    let ret: u64;
    core::arch::asm!(
        "int $0x80",
        inlateout("rax") num => ret,
        in("rdi") a0,
        in("rsi") a1,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(num: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int $0x80",
        inlateout("rax") num => ret,
        in("rdi") a0,
        in("rsi") a1,
        in("rdx") a2,
        options(nostack, preserves_flags),
    );
    ret
}

// ─── FsError — Rust-idiomatic error type ─────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    NotADirectory,
    NoEntry,
    Io,
    NotSupported,
    BadFd,
    NoSpace,
    PermissionDenied,
    AlreadyExists,
    Unknown(i32),
}

impl FsError {
    pub fn from_code(code: i32) Result<usize, Self> {
        if code >= 0 { return Ok(code as usize); }
        Err(match code {
            FS_ERR_NOTFOUND => Self::NotFound,
            FS_ERR_NOTDIR   => Self::NotADirectory,
            FS_ERR_NOENT    => Self::NoEntry,
            FS_ERR_IO       => Self::Io,
            FS_ERR_NOTSUP   => Self::NotSupported,
            FS_ERR_BADFD    => Self::BadFd,
            FS_ERR_NOSPACE  => Self::NoSpace,
            FS_ERR_PERM     => Self::PermissionDenied,
            FS_ERR_EXIST    => Self::AlreadyExists,
            other           => Self::Unknown(other),
        })
    }
}

// ─── FsClient — userspace API do FS serwera ──────────────────────────────────
// Używa shared memory IPC ring buffer.

pub struct FsClient {
    seq: u32,
    fs_server_pid: u32,
}

impl FsClient {
    pub fn new(fs_server_pid: u32) Self {
        Self { seq: 1, fs_server_pid }
    }

    fn next_seq(&mut self) -> u32 {
        let s = self.seq;
        self.seq = self.seq.wrapping_add(1);
        s
    }

    unsafe fn send_request(&mut self, req: &FsRequest, path: &[u8]) -> &'static FsResponse {
        // Zapisz path do shared buffer
        let path_buf = IPC_PATH_ADDR as *mut u8;
        let path_len = path.len().min(IPC_PATH_SIZE - 1);
        core::ptr::copy_nonoverlapping(path.as_ptr(), path_buf, path_len);
        *path_buf.add(path_len) = 0;

        // Zapisz request do ring buffer
        let ring = IPC_RING_ADDR as *mut FsRequest;
        let write_idx_ptr = (IPC_RING_ADDR + IPC_RING_SIZE - 8) as *mut u32;
        let wi = (*write_idx_ptr) as usize;
        *ring.add(wi) = *req;
        *write_idx_ptr = ((wi + 1) % (IPC_RING_SIZE / core::mem::size_of::<FsRequest>())) as u32;

        // Powiadom FS serwer i czekaj na odpowiedź
        syscall2(SYS_IPC_SEND, self.fs_server_pid as u64, req.seq as u64);

        // Odpowiedź w drugiej połowie ring buffer
        let resp_ptr = (IPC_RING_ADDR + IPC_RING_SIZE / 2) as *const FsResponse;
        &*resp_ptr
    }

    pub unsafe fn open(&mut self, path: &[u8], flags: u32) -> Result<u16, FsError> {
        let seq = self.next_seq();
        let req = FsRequest {
            seq,
            op: FS_OP_OPEN,
            flags: flags as u8,
            fd: 0,
            pid: get_current_pid(),
            arg0: 0,
            arg1: 0,
            path_len: path.len() as u16,
            _pad: [0; 6],
        };
        let resp = self.send_request(&req, path);
        FsError::from_code(resp.status).map(|fd| fd as u16)
    }

    pub unsafe fn read(&mut self, fd: u16, buf: &mut [u8]) -> Result<usize, FsError> {
        let seq = self.next_seq();
        let req = FsRequest {
            seq,
            op: FS_OP_READ,
            flags: 0,
            fd,
            pid: get_current_pid(),
            arg0: 0,
            arg1: buf.len() as u64,
            path_len: 0,
            _pad: [0; 6],
        };
        let resp = self.send_request(&req, &[]);

        // Kopiuj dane z shared data buffer do buf
        if resp.status > 0 {
            let n = resp.status as usize;
            let data_src = IPC_DATA_ADDR as *const u8;
            core::ptr::copy_nonoverlapping(data_src, buf.as_mut_ptr(), n.min(buf.len()));
        }

        FsError::from_code(resp.status)
    }

    pub unsafe fn close(&mut self, fd: u16) -> Result<(), FsError> {
        let seq = self.next_seq();
        let req = FsRequest {
            seq, op: FS_OP_CLOSE, flags: 0, fd,
            pid: get_current_pid(),
            arg0: 0, arg1: 0, path_len: 0, _pad: [0; 6],
        };
        let resp = self.send_request(&req, &[]);
        FsError::from_code(resp.status).map(|_| ())
    }

    pub unsafe fn stat(&mut self, path: &[u8]) -> Result<Stat, FsError> {
        let seq = self.next_seq();
        let req = FsRequest {
            seq, op: FS_OP_STAT, flags: 0, fd: 0,
            pid: get_current_pid(),
            arg0: 0, arg1: 0,
            path_len: path.len() as u16,
            _pad: [0; 6],
        };
        let resp = self.send_request(&req, path);
        if resp.status < 0 { return Err(FsError::from_code(resp.status).unwrap_err()); }

        Ok(Stat {
            ino:    resp.ino,
            size:   resp.size,
            ftype:  resp.ftype,
            ..Stat::default()
        })
    }
}

// ─── Kernel-side syscall handler stubs ───────────────────────────────────────
// Wywoływane przez kernel gdy userspace robi sys_open/read/close przez int 0x80.
// Kernel dispatchu do FS serwera przez IPC.
//
// Sygnatury zgodne z CosinusOS syscall convention:
//   rax = syscall number
//   rdi, rsi, rdx, r10, r8, r9 = args
//   Zwraca: rax = wynik lub negacja errno

pub mod kernel {
    use super::*;

    /// SYS_OPEN: rdi = path_ptr, rsi = path_len, rdx = flags
    /// Zwraca: fd >= 0 lub < 0 = błąd
    pub unsafe fn sys_open(
        path_ptr:  *const u8,
        path_len:  usize,
        flags:     u32,
        fs_server_pid: u32,
    ) -> i64 {
        let path = core::slice::from_raw_parts(path_ptr, path_len);
        let mut client = FsClient::new(fs_server_pid);
        match client.open(path, flags) {
            Ok(fd)  => fd as i64,
            Err(_)  => -1,
        }
    }

    /// SYS_READ: rdi = fd, rsi = buf_ptr, rdx = count
    pub unsafe fn sys_read(
        fd:       u16,
        buf_ptr:  *mut u8,
        count:    usize,
        fs_server_pid: u32,
    ) -> i64 {
        let buf = core::slice::from_raw_parts_mut(buf_ptr, count);
        let mut client = FsClient::new(fs_server_pid);
        match client.read(fd, buf) {
            Ok(n)  => n as i64,
            Err(_) => -1,
        }
    }

    /// SYS_CLOSE: rdi = fd
    pub unsafe fn sys_close(fd: u16, fs_server_pid: u32) -> i64 {
        let mut client = FsClient::new(fs_server_pid);
        match client.close(fd) {
            Ok(_)  => 0,
            Err(_) => -1,
        }
    }

    /// SYS_STAT: rdi = path_ptr, rsi = path_len, rdx = stat_ptr
    pub unsafe fn sys_stat(
        path_ptr:  *const u8,
        path_len:  usize,
        stat_out:  *mut Stat,
        fs_server_pid: u32,
    ) -> i64 {
        let path = core::slice::from_raw_parts(path_ptr, path_len);
        let mut client = FsClient::new(fs_server_pid);
        match client.stat(path) {
            Ok(s)  => { *stat_out = s; 0 }
            Err(_) => -1,
        }
    }
}

// ─── FFI exports dla Zig (extern "C") ────────────────────────────────────────
// Zig może wywołać te funkcje przez @cImport lub extern blok.

#[no_mangle]
pub unsafe extern "C" fn cosfs_open(
    path: *const u8,
    path_len: usize,
    flags: u32,
    server_pid: u32,
) -> i64 {
    kernel::sys_open(path, path_len, flags, server_pid)
}

#[no_mangle]
pub unsafe extern "C" fn cosfs_read(
    fd: u16,
    buf: *mut u8,
    count: usize,
    server_pid: u32,
) -> i64 {
    kernel::sys_read(fd, buf, count, server_pid)
}

#[no_mangle]
pub unsafe extern "C" fn cosfs_close(fd: u16, server_pid: u32) -> i64 {
    kernel::sys_close(fd, server_pid)
}

#[no_mangle]
pub unsafe extern "C" fn cosfs_stat(
    path: *const u8,
    path_len: usize,
    stat_out: *mut Stat,
    server_pid: u32,
) -> i64 {
    kernel::sys_stat(path, path_len, stat_out, server_pid)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

unsafe fn get_current_pid() -> u32 {
    // SYS_GETPID = 0x27 (CosinusOS ABI)
    syscall2(0x27, 0, 0) as u32
}

// ─── Compile-time layout checks ──────────────────────────────────────────────

const _: () = {
    assert!(core::mem::size_of::<FsRequest>()  == 32);
    assert!(core::mem::size_of::<FsResponse>() == 32);
    assert!(core::mem::align_of::<FsRequest>() == 8);
};