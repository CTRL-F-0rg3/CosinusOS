// libcosinus — lib.rs
// Userspace standard library for CosinusOS.
// Syscall ABI: int 0x80, rax=nr, rdi/rsi/rdx=args, rax=return value.
// Only syscalls actually implemented in kernel syscall_api.rs are exposed.

#![no_std]
#![allow(dead_code)]

// ── Syscall numbers (must match kernel syscall_api.rs) ────────────────────────
pub mod nr {
    pub const EXIT:        u64 = 0;
    pub const WRITE:       u64 = 1;
    pub const READ:        u64 = 2;
    pub const YIELD:       u64 = 3;
    pub const SPAWN:       u64 = 4;
    pub const SLEEP:       u64 = 5;
    pub const MEM_ALLOC:   u64 = 6;
    pub const MEM_FREE:    u64 = 7;
    pub const IPC_SEND:    u64 = 8;
    pub const IPC_RECV:    u64 = 9;
    pub const IPC_POLL:    u64 = 10;
    pub const THREAD_ID:   u64 = 11;
    pub const TIME:        u64 = 12;
    pub const DEBUG_PRINT: u64 = 13;
    pub const GET_FB_INFO: u64 = 14;  // query framebuffer + map into userspace
}

// ── Error codes (must match kernel syscall_api.rs) ────────────────────────────
pub mod err {
    pub const OK:    i64 =  0;
    pub const INVAL: i64 = -1;
    pub const NOMEM: i64 = -2;
    pub const NOSLOT:i64 = -3;
    pub const FAULT: i64 = -4;
    pub const AGAIN: i64 = -5;
    pub const NOSYS: i64 = -6;
    pub const PERM:  i64 = -7;
}

pub type CosResult<T> = Result<T, i64>;

// ── Framebuffer descriptor ────────────────────────────────────────────────────

/// Filled by the GET_FB_INFO syscall.
/// After a successful call, `virt_addr` points to the linear 32-bpp pixel buffer.
#[repr(C)]
pub struct FbInfo {
    /// Virtual address at which the FB is mapped in this process.
    pub virt_addr: u64,
    /// Physical base address (informational).
    pub phys_addr: u64,
    /// Width in pixels.
    pub width:     u32,
    /// Height in pixels.
    pub height:    u32,
    /// Bytes per scan line.
    pub pitch:     u32,
    /// Bits per pixel (always 32 = BGRX).
    pub bpp:       u32,
    /// Total framebuffer size in bytes.
    pub size:      u64,
}

impl FbInfo {
    pub const fn zeroed() -> Self {
        Self {
            virt_addr: 0, phys_addr: 0,
            width: 0, height: 0, pitch: 0, bpp: 0, size: 0,
        }
    }

    /// Return a mutable pixel slice over the entire framebuffer.
    /// Each element is a 32-bit BGRX pixel (blue in low byte).
    ///
    /// # Safety
    /// Valid only after a successful `get_fb_info()` call.
    pub unsafe fn pixels_mut(&self) -> &mut [u32] {
        core::slice::from_raw_parts_mut(
            self.virt_addr as *mut u32,
            (self.pitch / 4) as usize * self.height as usize,
        )
    }

    /// Write one pixel at (x, y). Clips silently if out of bounds.
    #[inline]
    pub unsafe fn put_pixel(&self, x: u32, y: u32, rgb: u32) {
        if x >= self.width || y >= self.height { return; }
        let off = y as usize * (self.pitch / 4) as usize + x as usize;
        *((self.virt_addr as *mut u32).add(off)) = rgb;
    }

    /// Fill a rectangle with a solid colour.
    pub unsafe fn fill_rect(&self, x: u32, y: u32, w: u32, h: u32, rgb: u32) {
        let x2 = (x + w).min(self.width);
        let y2 = (y + h).min(self.height);
        let stride = (self.pitch / 4) as usize;
        let base   = self.virt_addr as *mut u32;
        for row in y..y2 {
            for col in x..x2 {
                *base.add(row as usize * stride + col as usize) = rgb;
            }
        }
    }
}

// ── Syscall wrapper ───────────────────────────────────────────────────────────

/// Query the kernel for framebuffer info and map the FB into this process.
pub fn get_fb_info(info: &mut FbInfo) -> CosResult<()> {
    unsafe {
        ok(syscall1(nr::GET_FB_INFO, info as *mut FbInfo as u64)).map(|_| ())
    }
}

#[inline(always)]
fn ok(r: i64) -> CosResult<i64> {
    if r >= 0 { Ok(r) } else { Err(r) }
}

// ── Raw syscall wrappers ──────────────────────────────────────────────────────
#[inline(always)]
pub unsafe fn syscall0(n: u64) -> i64 {
    let r: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") n as i64 => r,
        options(nostack, preserves_flags)
    );
    r
}

#[inline(always)]
pub unsafe fn syscall1(n: u64, a1: u64) -> i64 {
    let r: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") n as i64 => r,
        in("rdi") a1,
        options(nostack, preserves_flags)
    );
    r
}

#[inline(always)]
pub unsafe fn syscall2(n: u64, a1: u64, a2: u64) -> i64 {
    let r: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") n as i64 => r,
        in("rdi") a1,
        in("rsi") a2,
        options(nostack, preserves_flags)
    );
    r
}

#[inline(always)]
pub unsafe fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let r: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") n as i64 => r,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        options(nostack, preserves_flags)
    );
    r
}

// ── Process ───────────────────────────────────────────────────────────────────

pub fn exit(code: i32) -> ! {
    unsafe { syscall1(nr::EXIT, code as u64); }
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

pub fn sched_yield() { unsafe { syscall0(nr::YIELD); } }
pub fn sleep(ticks: u64) { unsafe { syscall1(nr::SLEEP, ticks); } }
pub fn thread_id() -> u32 { unsafe { syscall1(nr::THREAD_ID, 0) as u32 } }

// ── I/O ───────────────────────────────────────────────────────────────────────

/// Write bytes to fd 1 (stdout) or fd 2 (stderr).
pub fn write(fd: u64, s: &str) -> CosResult<usize> {
    unsafe {
        ok(syscall3(nr::WRITE, fd, s.as_ptr() as u64, s.len() as u64))
            .map(|n| n as usize)
    }
}

pub fn print(s: &str)    { let _ = write(1, s); }
pub fn println(s: &str)  { let _ = write(1, s); let _ = write(1, "\n"); }
pub fn eprint(s: &str)   { let _ = write(2, s); }
pub fn eprintln(s: &str) { let _ = write(2, s); let _ = write(2, "\n"); }

/// Send a string to kernel serial output (bypasses VGA, always visible).
pub fn debug(s: &str) {
    unsafe { syscall3(nr::DEBUG_PRINT, 0, s.as_ptr() as u64, s.len() as u64); }
}

/// Read up to `buf.len()` bytes from stdin (fd 0).
/// Returns err::AGAIN if no input is available yet.
pub fn read(buf: &mut [u8]) -> CosResult<usize> {
    unsafe {
        ok(syscall3(nr::READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64))
            .map(|n| n as usize)
    }
}

/// Blocking readline — yields until a newline or buffer full.
pub fn read_line(buf: &mut [u8]) -> usize {
    let mut total = 0usize;
    loop {
        if total >= buf.len() { break; }
        match read(&mut buf[total..total + 1]) {
            Err(e) if e == err::AGAIN => { sched_yield(); continue; }
            Err(_) | Ok(0) => break,
            Ok(_) => {
                total += 1;
                if buf[total - 1] == b'\n' { break; }
            }
        }
    }
    total
}

// ── Memory ────────────────────────────────────────────────────────────────────

/// Allocate `pages` pages at a kernel-chosen address (hint = 0).
pub fn mem_alloc(pages: usize) -> *mut u8 {
    let r = unsafe { syscall2(nr::MEM_ALLOC, pages as u64, 0) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

/// Allocate `pages` pages at a specific virtual address hint.
pub fn mem_alloc_at(pages: usize, hint: u64) -> *mut u8 {
    let r = unsafe { syscall2(nr::MEM_ALLOC, pages as u64, hint) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

pub fn mem_free(ptr: *mut u8, pages: usize) -> CosResult<()> {
    unsafe { ok(syscall2(nr::MEM_FREE, ptr as u64, pages as u64)).map(|_| ()) }
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct SpawnArgs {
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

pub fn spawn(args: &SpawnArgs) -> CosResult<u32> {
    unsafe { ok(syscall1(nr::SPAWN, args as *const SpawnArgs as u64)).map(|t| t as u32) }
}

pub fn spawn_thread(entry: unsafe extern "C" fn(u64) -> !, name: &[u8; 16], arg: u64) -> CosResult<u32> {
    spawn(&SpawnArgs {
        entry: entry as u64,
        arg,
        stack_sz: 0,
        flags: spawn_flags::USER,
        name: *name,
    })
}

// ── IPC ───────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct IpcMsg {
    pub from:  u32, pub to:   u32,
    pub tag:   u32, pub _pad: u32,
    pub data:  [u64; 4],
    pub ptr:   u64,
    pub len:   u32, pub _pad2: u32,
}

impl IpcMsg {
    pub const fn zeroed() -> Self {
        Self { from:0, to:0, tag:0, _pad:0, data:[0;4], ptr:0, len:0, _pad2:0 }
    }
}

pub fn ipc_send(msg: &IpcMsg) -> CosResult<()> {
    unsafe { ok(syscall1(nr::IPC_SEND, msg as *const IpcMsg as u64)).map(|_| ()) }
}

pub fn ipc_recv(msg: &mut IpcMsg) -> CosResult<()> {
    unsafe { ok(syscall2(nr::IPC_RECV, msg as *mut IpcMsg as u64, 0)).map(|_| ()) }
}

pub fn ipc_recv_blocking(msg: &mut IpcMsg) -> CosResult<()> {
    unsafe { ok(syscall2(nr::IPC_RECV, msg as *mut IpcMsg as u64, 1)).map(|_| ()) }
}

pub fn ipc_poll() -> usize {
    unsafe { syscall1(nr::IPC_POLL, 0).max(0) as usize }
}

// ── Time ──────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct TimeInfo { pub ticks: u64, pub uptime: u64 }

pub fn ticks() -> u64 { unsafe { syscall0(nr::TIME) as u64 } }

// ── Thread info ───────────────────────────────────────────────────────────────

#[repr(C)]
pub struct ThreadInfo { pub tid: u32, pub prio: u8, pub _pad: [u8; 3] }

// ── fmt::Write bridge ─────────────────────────────────────────────────────────

pub struct Stdout;
pub struct Stderr;
pub struct Serial;

impl core::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { print(s); Ok(()) }
}
impl core::fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { eprint(s); Ok(()) }
}
impl core::fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { debug(s); Ok(()) }
}

#[macro_export] macro_rules! cos_print {
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::Stdout, $($a)*); }};
}
#[macro_export] macro_rules! cos_println {
    ()          => { $crate::print("\n"); };
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = writeln!($crate::Stdout, $($a)*); }};
}
#[macro_export] macro_rules! cos_eprint {
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::Stderr, $($a)*); }};
}
#[macro_export] macro_rules! cos_eprintln {
    ()          => { $crate::eprint("\n"); };
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = writeln!($crate::Stderr, $($a)*); }};
}
#[macro_export] macro_rules! cos_dbg {
    ($($a:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::Serial, $($a)*); }};
}

// ── Panic handler ─────────────────────────────────────────────────────────────

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut buf = [0u8; 256];
    let mut pos = 0usize;

    let push = |buf: &mut [u8; 256], pos: &mut usize, s: &str| {
        for &b in s.as_bytes() {
            if *pos >= buf.len() { break; }
            buf[*pos] = b;
            *pos += 1;
        }
    };

    push(&mut buf, &mut pos, "\n[PANIC]");
    if let Some(loc) = info.location() {
        push(&mut buf, &mut pos, " ");
        push(&mut buf, &mut pos, loc.file());
        push(&mut buf, &mut pos, ":");
        let line = loc.line();
        let mut tmp = [0u8; 10];
        let mut i = 10usize;
        let mut n = line;
        if n == 0 { i -= 1; tmp[i] = b'0'; } else {
            while n > 0 { i -= 1; tmp[i] = b'0' + (n % 10) as u8; n /= 10; }
        }
        push(&mut buf, &mut pos, core::str::from_utf8(&tmp[i..]).unwrap_or("?"));
    }
    if let Some(msg) = info.message().as_str() {
        push(&mut buf, &mut pos, " — ");
        push(&mut buf, &mut pos, msg);
    }
    push(&mut buf, &mut pos, "\n");

    if let Ok(s) = core::str::from_utf8(&buf[..pos]) {
        eprint(s);
    }
    exit(101)
}