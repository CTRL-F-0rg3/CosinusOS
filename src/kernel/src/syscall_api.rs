// CosinusOS — syscall_api.rs

pub mod nr {
    pub const EXIT:         u64 = 0;
    pub const WRITE:        u64 = 1;
    pub const READ:         u64 = 2;
    pub const YIELD:        u64 = 3;
    pub const SPAWN:        u64 = 4;
    pub const SLEEP:        u64 = 5;
    pub const MEM_ALLOC:    u64 = 6;
    pub const MEM_FREE:     u64 = 7;
    pub const IPC_SEND:     u64 = 8;
    pub const IPC_RECV:     u64 = 9;
    pub const IPC_POLL:     u64 = 10;
    pub const THREAD_ID:    u64 = 11;
    pub const TIME:         u64 = 12;
    pub const DEBUG_PRINT:  u64 = 13;
    pub const GET_FB_INFO:  u64 = 14; // query framebuffer layout + map into userspace
}

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

// ── Shared structs ────────────────────────────────────────────────────────────

#[repr(C)]
pub struct SpawnArgs {
    pub entry:    u64,
    pub arg:      u64,
    pub stack_sz: u32,
    pub flags:    u32,
    pub name:     [u8; 16],
}

pub mod spawn_flags {
    pub const KERNEL: u32 = 0;
    pub const USER:   u32 = 1 << 0;
    pub const DETACH: u32 = 1 << 1;
}

#[repr(C)]
pub struct IpcMsg {
    pub from:  u32,
    pub to:    u32,
    pub tag:   u32,
    pub _pad:  u32,
    pub data:  [u64; 4],
    pub ptr:   u64,
    pub len:   u32,
    pub _pad2: u32,
}

#[repr(C)]
pub struct ThreadInfo {
    pub tid:  u32,
    pub prio: u8,
    pub _pad: [u8; 3],
}

#[repr(C)]
pub struct TimeInfo {
    pub ticks:  u64,
    pub uptime: u64,
}

/// Framebuffer descriptor written into userspace by GET_FB_INFO.
/// The kernel maps the physical FB pages into the calling process and
/// fills this struct with the layout information.
///
/// After a successful call the process can write pixels directly to
/// `virt_addr` as a flat BGRX / XRGB 32-bpp buffer.
#[repr(C)]
pub struct FbInfo {
    /// Virtual address at which the framebuffer is mapped in userspace.
    pub virt_addr: u64,
    /// Physical base address of the framebuffer (informational).
    pub phys_addr: u64,
    /// Width in pixels.
    pub width:     u32,
    /// Height in pixels.
    pub height:    u32,
    /// Bytes per row (may be larger than width * 4 due to hardware padding).
    pub pitch:     u32,
    /// Bits per pixel (always 32 for the current GRUB linear FB).
    pub bpp:       u32,
    /// Total size of the framebuffer in bytes.
    pub size:      u64,
}

pub type SyscallFn = unsafe fn(*mut crate::perm::TF) -> i64;

// ── Dispatcher ────────────────────────────────────────────────────────────────

#[no_mangle]
pub unsafe fn syscall_dispatch_v2(tf: *mut crate::perm::TF) {
    let num = (*tf).rax;
    let cs  = (*tf).cs;

    if let Err(e) = crate::root_policy::check_syscall(num, cs) {
        (*tf).rax = e as u64;
        return;
    }

    let ret: i64 = match num {
        nr::EXIT        => sys_exit(tf),
        nr::WRITE       => sys_write(tf),
        nr::READ        => sys_read(tf),
        nr::YIELD       => sys_yield(tf),
        nr::SPAWN       => sys_spawn(tf),
        nr::SLEEP       => sys_sleep(tf),
        nr::MEM_ALLOC   => sys_mem_alloc(tf),
        nr::MEM_FREE    => sys_mem_free(tf),
        nr::IPC_SEND    => crate::ipc::sys_ipc_send(tf),
        nr::IPC_RECV    => crate::ipc::sys_ipc_recv(tf),
        nr::IPC_POLL    => crate::ipc::sys_ipc_poll(tf),
        nr::THREAD_ID   => sys_thread_id(tf),
        nr::TIME        => sys_time(tf),
        nr::DEBUG_PRINT => sys_debug_print(tf),
        nr::GET_FB_INFO => sys_get_fb_info(tf),
        _               => err::NOSYS,
    };
    (*tf).rax = ret as u64;
}

// ── sys_get_fb_info ───────────────────────────────────────────────────────────

/// GET_FB_INFO — rdi = pointer to FbInfo struct in userspace.
///
/// The kernel:
///   1. Reads the physical FB address and dimensions from display::fb.
///   2. Maps every FB page into the calling process at a fixed user VA
///      (FB_USER_VADDR = 0x0000_6000_0000_0000).
///   3. Fills the FbInfo struct and returns 0.
///
/// The userspace virtual address is deterministic so the process can
/// call this multiple times safely (vmap is idempotent for the same VA).
unsafe fn sys_get_fb_info(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{valid_buf, vmap, PAGE_SIZE, PTE_W, PTE_U};
    use crate::display::fb::{FB_PHYS, FB_WIDTH, FB_HEIGHT, FB_PITCH, FB_BPP};
    use core::sync::atomic::Ordering;
    use core::mem::size_of;

    let ptr = (*tf).rdi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    // Validate the output pointer
    if !valid_buf(p4, ptr, size_of::<FbInfo>()) { return err::FAULT; }

    let phys   = FB_PHYS;
    let width  = FB_WIDTH;
    let height = FB_HEIGHT;
    let pitch  = FB_PITCH;
    let bpp    = FB_BPP;

    if phys == 0 || width == 0 || height == 0 { return err::INVAL; }

    let fb_bytes  = pitch as u64 * height as u64;
    let fb_pages  = ((fb_bytes + PAGE_SIZE as u64 - 1) / PAGE_SIZE as u64) as usize;

    // Fixed user-space VA for the framebuffer mapping
    const FB_USER_VADDR: u64 = 0x0000_6000_0000_0000;

    // Map physical FB pages into the calling process (read-write, user)
    for i in 0..fb_pages {
        let va   = FB_USER_VADDR + i as u64 * PAGE_SIZE as u64;
        let pa   = phys          + i as u64 * PAGE_SIZE as u64;
        // PTE_W | PTE_U — no PTE_NX so the CPU can prefetch through it
        vmap(p4, va, pa, PTE_W | PTE_U);
    }

    // Fill in the FbInfo struct in userspace memory
    let info      = &mut *(ptr as *mut FbInfo);
    info.virt_addr = FB_USER_VADDR;
    info.phys_addr = phys;
    info.width     = width;
    info.height    = height;
    info.pitch     = pitch;
    info.bpp       = bpp;
    info.size      = fb_bytes;

    err::OK
}

// ── Existing syscall implementations (unchanged) ──────────────────────────────

unsafe fn sys_exit(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR, TS, NTHREADS, schedule};
    use core::sync::atomic::Ordering;
    let c = CUR.load(Ordering::Relaxed);
    THREADS[c].state = TS::Dead;
    NTHREADS.fetch_sub(1, Ordering::Relaxed);
    schedule();
    err::OK
}

unsafe fn sys_write(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use crate::debug::{putc, VGA_LOCK};
    use core::sync::atomic::Ordering;

    let fd  = (*tf).rdi;
    let ptr = (*tf).rsi;
    let len = (*tf).rdx as usize;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if fd != 1 && fd != 2 { return err::INVAL; }
    if len == 0 { return err::OK; }
    if len > 65536 { return err::INVAL; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    VGA_LOCK.lock();
    for i in 0..len { putc(*(ptr as *const u8).add(i) as char); }
    VGA_LOCK.unlock();

    len as i64
}

unsafe fn sys_read(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let fd  = (*tf).rdi;
    let ptr = (*tf).rsi;
    let len = (*tf).rdx as usize;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if fd != 0 { return err::INVAL; }
    if len == 0 { return err::OK; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    let mut n = 0usize;
    let buf = core::slice::from_raw_parts_mut(ptr as *mut u8, len);
    while n < len {
        match crate::perm::kb_pop() {
            Some(c) => { buf[n] = c as u8; n += 1; if c == '\n' { break; } }
            None    => break,
        }
    }
    if n == 0 { err::AGAIN } else { n as i64 }
}

unsafe fn sys_yield(_tf: *mut crate::perm::TF) -> i64 {
    crate::threading::thread_yield();
    err::OK
}

unsafe fn sys_spawn(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;
    use crate::syscall_api::SpawnArgs;

    let ptr = (*tf).rdi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    let sz = core::mem::size_of::<SpawnArgs>();
    if !valid_buf(p4, ptr, sz) { return err::FAULT; }

    let args = &*(ptr as *const SpawnArgs);
    let name = core::str::from_utf8(&args.name)
        .unwrap_or("thread")
        .trim_end_matches('\0');

    let flags = args.flags;
    if flags & spawn_flags::USER != 0 {
        let new_cr3 = crate::mm::new_user_p4();
        let tid = crate::threading::spawn_user_on_cr3(name, args.entry, args.arg, new_cr3);
        if tid >= 0 { tid as i64 } else { err::NOSLOT }
    } else {
        err::PERM
    }
}

unsafe fn sys_sleep(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR, TS};
    use core::sync::atomic::Ordering;

    let ticks = (*tf).rdi;
    if ticks == 0 { return err::OK; }

    let c = CUR.load(Ordering::Relaxed);
    THREADS[c].wake_tick = crate::perm::TICK + ticks;
    THREADS[c].state = TS::Block;
    crate::threading::thread_yield();
    err::OK
}

unsafe fn sys_mem_alloc(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{mm_alloc, vmap, PTE_W, PTE_U, PAGE_SIZE};
    use core::sync::atomic::Ordering;

    let pages = (*tf).rdi as usize;
    let hint  = (*tf).rsi;

    if pages == 0 || pages > 512 { return err::INVAL; }

    let p4   = THREADS[CUR.load(Ordering::Relaxed)].cr3;
    let base = if hint != 0 && hint >= 0x1000 { hint } else { 0x1000_0000u64 };
    let vbase = (base + 0xFFF) & !0xFFF;

    for i in 0..pages {
        let vaddr = vbase + i as u64 * PAGE_SIZE as u64;
        let phys  = mm_alloc();
        if vmap(p4, vaddr, phys, PTE_W | PTE_U) != 0 {
            return if i > 0 { vbase as i64 } else { err::NOMEM };
        }
        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
    }
    vbase as i64
}

unsafe fn sys_mem_free(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{virt_to_phys, vunmap, mm_free_phys, PAGE_SIZE};
    use core::sync::atomic::Ordering;

    let ptr   = (*tf).rdi;
    let pages = (*tf).rsi as usize;

    if ptr == 0 || ptr & 0xFFF != 0 { return err::INVAL; }
    if pages == 0 || pages > 512    { return err::INVAL; }

    let p4 = THREADS[CUR.load(Ordering::Relaxed)].cr3;
    for i in 0..pages {
        let vaddr = ptr + i as u64 * PAGE_SIZE as u64;
        if let Some(phys) = virt_to_phys(p4, vaddr) {
            mm_free_phys(phys);
            vunmap(p4, vaddr);
        }
    }
    err::OK
}

unsafe fn sys_thread_id(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;
    let c   = CUR.load(Ordering::Relaxed);

    if ptr != 0 {
        if !valid_buf(p4, ptr, core::mem::size_of::<ThreadInfo>()) { return err::FAULT; }
        let info = &mut *(ptr as *mut ThreadInfo);
        info.tid  = THREADS[c].id;
        info.prio = THREADS[c].prio;
        info._pad = [0; 3];
    }
    THREADS[c].id as i64
}

unsafe fn sys_time(tf: *mut crate::perm::TF) -> i64 {
    use crate::mm::valid_buf;
    use crate::threading::{THREADS, CUR};
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if ptr != 0 {
        if !valid_buf(p4, ptr, core::mem::size_of::<TimeInfo>()) { return err::FAULT; }
        let info = &mut *(ptr as *mut TimeInfo);
        info.ticks  = crate::perm::TICK;
        info.uptime = crate::perm::TICK / 100;
    }
    crate::perm::TICK as i64
}

unsafe fn sys_debug_print(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use crate::debug::serial_print;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rsi;
    let len = (*tf).rdx as usize;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if len == 0 { return err::OK; }
    if len > 4096 { return err::INVAL; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    let bytes = core::slice::from_raw_parts(ptr as *const u8, len);
    if let Ok(s) = core::str::from_utf8(bytes) {
        serial_print("[US] ");
        serial_print(s);
    }
    len as i64
}