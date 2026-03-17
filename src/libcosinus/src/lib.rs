// libcosinus — biblioteka systemowa CosinusOS
// Linkowana statycznie do każdego procesu userspace.
// Odpowiednik glibc/musl dla CosinusOS.
//
// Struktura:
//   syscall  — niskopoziomowe wrappery int 0x80
//   io       — print/println/eprint
//   proc     — exit, spawn, wait, yield, sleep
//   mem      — alloc/free (mmap/munmap)
//   ipc      — send/recv/poll
//   thread   — TID, priority
//   time     — ticks, uptime
//   fs       — open/close/read/write/seek (gdy VFS gotowy)
//   signal   — signal/kill/sigret

#![no_std]
#![allow(dead_code)]

// ── Numery syscalli (identyczne z kernel/syscall_api.rs::nr) ─────────────────
pub mod nr {
    pub const EXIT:            u64 = 0;
    pub const WRITE:           u64 = 1;
    pub const READ:            u64 = 2;
    pub const YIELD:           u64 = 3;
    pub const SPAWN:           u64 = 4;
    pub const SLEEP:           u64 = 5;
    pub const MEM_ALLOC:       u64 = 6;
    pub const MEM_FREE:        u64 = 7;
    pub const IPC_SEND:        u64 = 8;
    pub const IPC_RECV:        u64 = 9;
    pub const IPC_POLL:        u64 = 10;
    pub const THREAD_ID:       u64 = 11;
    pub const TIME:            u64 = 12;
    pub const DEBUG_PRINT:     u64 = 13;
    pub const THREAD_SET_PRIO: u64 = 14;
    pub const WAIT:            u64 = 15;
    pub const OPEN:            u64 = 20;
    pub const CLOSE:           u64 = 21;
    pub const SEEK:            u64 = 22;
    pub const FSTAT:           u64 = 23;
    pub const IOCTL:           u64 = 24;
    pub const MMAP:            u64 = 30;
    pub const MUNMAP:          u64 = 31;
    pub const MPROTECT:        u64 = 32;
    pub const SIGNAL:          u64 = 40;
    pub const KILL:            u64 = 41;
    pub const SIGRET:          u64 = 42;
    pub const GETCWD:          u64 = 50;
    pub const CHDIR:           u64 = 51;
    pub const PIPE:            u64 = 60;
}

// ── Kody błędów ──────────────────────────────────────────────────────────────
pub mod err {
    pub const OK:       i64 =  0;
    pub const INVAL:    i64 = -1;
    pub const NOMEM:    i64 = -2;
    pub const NOSLOT:   i64 = -3;
    pub const FAULT:    i64 = -4;
    pub const AGAIN:    i64 = -5;
    pub const NOSYS:    i64 = -6;
    pub const PERM:     i64 = -7;
    pub const NOENT:    i64 = -8;
    pub const BADF:     i64 = -9;
    pub const BUSY:     i64 = -10;
    pub const OVERFLOW: i64 = -11;
    pub const NOSUP:    i64 = -12;
    pub const ALIGN:    i64 = -13;
    pub const EXIST:    i64 = -14;
}

// ── Struktury ABI (repr(C) — identyczne z kernel) ────────────────────────────
#[repr(C)] pub struct SpawnArgs {
    pub entry:    u64,
    pub arg:      u64,
    pub stack_sz: u32,
    pub flags:    u32,
    pub name:     [u8; 16],
}
pub mod spawn_flags {
    pub const USER:   u32 = 1 << 0;
    pub const DETACH: u32 = 1 << 1;
}

#[repr(C)] pub struct IpcMsg {
    pub from:  u32,
    pub to:    u32,
    pub tag:   u32,
    pub _pad:  u32,
    pub data:  [u64; 4],
    pub ptr:   u64,
    pub len:   u32,
    pub _pad2: u32,
}

#[repr(C)] pub struct MmapArgs {
    pub hint:   u64,
    pub length: u64,
    pub prot:   u32,
    pub flags:  u32,
    pub fd:     i32,
    pub _pad:   u32,
    pub offset: u64,
}
pub mod mmap_prot  { pub const READ: u32=1; pub const WRITE: u32=2; pub const EXEC: u32=4; }
pub mod mmap_flags { pub const ANON: u32=1; pub const FIXED: u32=2; pub const PRIVATE: u32=8; }

#[repr(C)] pub struct ThreadInfo { pub tid: u32, pub prio: u8, pub state: u8, pub _pad: [u8;2] }
#[repr(C)] pub struct TimeInfo   { pub ticks: u64, pub uptime: u64 }
#[repr(C)] pub struct PipeFds    { pub read_fd: i32, pub write_fd: i32 }

pub mod sig {
    pub const KILL: u32 = 1;
    pub const TERM: u32 = 2;
    pub const USR1: u32 = 16;
    pub const USR2: u32 = 17;
    pub const MAX:  u32 = 32;
}

// ── Niskopoziomowe wrappery ───────────────────────────────────────────────────
#[inline(always)]
pub unsafe fn syscall0(n: u64) -> i64 {
    let r: i64;
    core::arch::asm!("int 0x80", inout("rax") n as i64 => r, options(nostack, preserves_flags));
    r
}
#[inline(always)]
pub unsafe fn syscall1(n: u64, a1: u64) -> i64 {
    let r: i64;
    core::arch::asm!("int 0x80", inout("rax") n as i64 => r, in("rdi") a1, options(nostack, preserves_flags));
    r
}
#[inline(always)]
pub unsafe fn syscall2(n: u64, a1: u64, a2: u64) -> i64 {
    let r: i64;
    core::arch::asm!("int 0x80", inout("rax") n as i64 => r, in("rdi") a1, in("rsi") a2, options(nostack, preserves_flags));
    r
}
#[inline(always)]
pub unsafe fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let r: i64;
    core::arch::asm!("int 0x80", inout("rax") n as i64 => r, in("rdi") a1, in("rsi") a2, in("rdx") a3, options(nostack, preserves_flags));
    r
}

// ── I/O ──────────────────────────────────────────────────────────────────────
pub fn write(fd: u64, s: &str) -> i64 {
    unsafe { syscall3(nr::WRITE, fd, s.as_ptr() as u64, s.len() as u64) }
}
pub fn print(s: &str)   { write(1, s); }
pub fn println(s: &str) { write(1, s); write(1, "\n"); }
pub fn eprint(s: &str)  { write(2, s); }

pub fn debug(s: &str) {
    unsafe { syscall3(nr::DEBUG_PRINT, 0, s.as_ptr() as u64, s.len() as u64); }
}

// fmt::Write dla format! w no_std
pub struct Writer;
impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { print(s); Ok(()) }
}
pub struct EWriter;
impl core::fmt::Write for EWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { eprint(s); Ok(()) }
}

#[macro_export] macro_rules! cos_print {
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::Writer, $($a)*); }};
}
#[macro_export] macro_rules! cos_println {
    ()          => { $crate::print("\n"); };
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::Writer, $($a)*); $crate::print("\n"); }};
}
#[macro_export] macro_rules! cos_eprint {
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::EWriter, $($a)*); }};
}

// ── Proces / wątki ────────────────────────────────────────────────────────────
pub fn exit(code: i32) -> ! {
    unsafe { syscall1(nr::EXIT, code as u64); }
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

pub fn sched_yield() { unsafe { syscall0(nr::YIELD); } }

pub fn sleep(ticks: u64) { unsafe { syscall1(nr::SLEEP, ticks); } }
pub fn sleep_ms(ms: u64) { sleep((ms * 100) / 1000); } // ~100Hz

pub fn thread_id() -> u32 {
    unsafe { syscall1(nr::THREAD_ID, 0) as u32 }
}

pub fn thread_info(out: &mut ThreadInfo) -> i64 {
    unsafe { syscall1(nr::THREAD_ID, out as *mut ThreadInfo as u64) }
}

pub fn set_priority(prio: u8) -> i64 {
    unsafe { syscall1(nr::THREAD_SET_PRIO, prio as u64) }
}

pub fn wait(tid: u32) -> i64 {
    unsafe { syscall1(nr::WAIT, tid as u64) }
}

pub fn spawn(args: &SpawnArgs) -> i64 {
    unsafe { syscall1(nr::SPAWN, args as *const SpawnArgs as u64) }
}

pub fn spawn_fn(entry: unsafe extern "C" fn(u64) -> !, name: &[u8; 16], arg: u64) -> i64 {
    let args = SpawnArgs {
        entry: entry as u64, arg,
        stack_sz: 0,
        flags: spawn_flags::USER,
        name: *name,
    };
    spawn(&args)
}

// ── Pamięć ────────────────────────────────────────────────────────────────────
pub fn mmap(length: usize, prot: u32) -> *mut u8 {
    let args = MmapArgs {
        hint: 0, length: length as u64,
        prot, flags: mmap_flags::ANON | mmap_flags::PRIVATE,
        fd: -1, _pad: 0, offset: 0,
    };
    let r = unsafe { syscall1(nr::MMAP, &args as *const MmapArgs as u64) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

pub fn munmap(ptr: *mut u8, length: usize) -> i64 {
    unsafe { syscall2(nr::MUNMAP, ptr as u64, length as u64) }
}

pub fn mprotect(ptr: *mut u8, length: usize, prot: u32) -> i64 {
    unsafe { syscall3(nr::MPROTECT, ptr as u64, length as u64, prot as u64) }
}

// ── IPC ──────────────────────────────────────────────────────────────────────
pub fn ipc_send(msg: &IpcMsg) -> i64 {
    unsafe { syscall1(nr::IPC_SEND, msg as *const IpcMsg as u64) }
}
pub fn ipc_recv(msg: &mut IpcMsg) -> i64 {
    unsafe { syscall2(nr::IPC_RECV, msg as *mut IpcMsg as u64, 0) }
}
pub fn ipc_recv_blocking(msg: &mut IpcMsg) -> i64 {
    unsafe { syscall2(nr::IPC_RECV, msg as *mut IpcMsg as u64, 1) }
}
pub fn ipc_poll() -> usize {
    let r = unsafe { syscall1(nr::IPC_POLL, 0) };
    if r < 0 { 0 } else { r as usize }
}

// ── Czas ─────────────────────────────────────────────────────────────────────
pub fn ticks() -> u64 { unsafe { syscall0(nr::TIME) as u64 } }
pub fn uptime_secs() -> u64 { ticks() / 100 }
pub fn time_info(out: &mut TimeInfo) -> i64 {
    unsafe { syscall1(nr::TIME, out as *mut TimeInfo as u64) }
}

// ── Stdin (read line) ─────────────────────────────────────────────────────────
pub fn read_stdin(buf: &mut [u8]) -> i64 {
    unsafe { syscall3(nr::READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

pub fn read_line(buf: &mut [u8]) -> usize {
    let mut total = 0usize;
    loop {
        if total >= buf.len() { break; }
        let r = read_stdin(&mut buf[total..total+1]);
        if r == err::AGAIN as i64 { sched_yield(); continue; }
        if r <= 0 { break; }
        total += 1;
        if buf[total-1] == b'\n' { break; }
    }
    total
}

// ── Filesystem (stub — implementacja gdy VFS gotowy) ─────────────────────────
pub fn open(path: &str, flags: u32) -> i64 {
    unsafe { syscall3(nr::OPEN, path.as_ptr() as u64, path.len() as u64, flags as u64) }
}
pub fn close(fd: i64) -> i64 { unsafe { syscall1(nr::CLOSE, fd as u64) } }
pub fn seek(fd: i64, off: i64, whence: u32) -> i64 {
    unsafe { syscall3(nr::SEEK, fd as u64, off as u64, whence as u64) }
}
pub fn getcwd(buf: &mut [u8]) -> i64 {
    unsafe { syscall2(nr::GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64) }
}
pub fn chdir(path: &str) -> i64 {
    unsafe { syscall2(nr::CHDIR, path.as_ptr() as u64, path.len() as u64) }
}

// ── Sygnały ───────────────────────────────────────────────────────────────────
pub fn signal(signum: u32, handler: u64) -> i64 {
    unsafe { syscall2(nr::SIGNAL, signum as u64, handler) }
}
pub fn kill(tid: u32, signum: u32) -> i64 {
    unsafe { syscall2(nr::KILL, tid as u64, signum as u64) }
}