// init/main.rs — CosinusOS init process (PID 1)
//
// Boot sequence:
//   _arg = PID of FS server, started by kernel before init
//   1. Store FS server PID, allocate shared memory window
//   2. Wait for TAG_FS_READY from FS server
//   3. Read /etc/init.conf, spawn processes listed there
//   4. Supervisor loop — reap dead children, restart critical ones

#![no_std]
#![no_main]

extern crate alloc;

// ─── Global allocator — wraps kernel mem_alloc/mem_free ──────────────────────

use core::alloc::{GlobalAlloc, Layout};

struct KernelAlloc;

unsafe impl GlobalAlloc for KernelAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pages = (layout.size() + 0xFFF) / 0x1000;
        libcosinus::mem_alloc(pages.max(1))
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let pages = (layout.size() + 0xFFF) / 0x1000;
        let _ = libcosinus::mem_free(ptr, pages.max(1));
    }
}

#[global_allocator]
static ALLOCATOR: KernelAlloc = KernelAlloc;

use alloc::vec::Vec;
use libcosinus::{
    cos_dbg, cos_println,
    err,
    ipc_recv_blocking, ipc_send,
    mem_alloc_at,
    sched_yield, sleep,
    spawn, spawn_flags,
    thread_id, ticks,
    IpcMsg, SpawnArgs,
};

// ─── FS IPC protocol (must match filesystem/main.zig) ────────────────────────

#[allow(dead_code)]
mod fs_ipc {
    // IpcMsg.tag values
    pub const TAG_FS_REQUEST:  u32 = 0x4653_0001;
    pub const TAG_FS_RESPONSE: u32 = 0x4653_0002;
    pub const TAG_FS_READY:    u32 = 0x4653_00FF;

    // Opcodes carried in IpcMsg.data[0]
    pub const OP_OPEN:    u64 = 1;
    pub const OP_READ:    u64 = 2;
    pub const OP_WRITE:   u64 = 3;
    pub const OP_CLOSE:   u64 = 4;
    pub const OP_READDIR: u64 = 5;
    pub const OP_STAT:    u64 = 6;

    // Error codes returned in IpcMsg.data[0] on response
    pub const ERR_OK:       i64 =  0;
    pub const ERR_NOTFOUND: i64 = -1;
    pub const ERR_IO:       i64 = -4;
    pub const ERR_BADFD:    i64 = -6;

    // Shared memory layout (must match Zig FS server)
    pub const SHM_BASE:      usize = 0x0000_7000_0000_0000;
    pub const SHM_PATH_OFF:  usize = 0x1000;
    pub const SHM_DATA_OFF:  usize = 0x2000;
    pub const SHM_DATA_SIZE: usize = 0x10000; // 64 KB transfer window
    pub const SHM_PAGES:     usize = (SHM_DATA_OFF + SHM_DATA_SIZE) / 0x1000 + 1;
}

// ─── Globals ──────────────────────────────────────────────────────────────────

static mut FS_SERVER_PID: u32    = 0;
static mut SHM_BASE:      *mut u8 = core::ptr::null_mut();

fn fs_pid() -> u32 { unsafe { FS_SERVER_PID } }

fn shm_path_buf() -> *mut u8 {
    unsafe { SHM_BASE.add(fs_ipc::SHM_PATH_OFF) }
}
fn shm_data_buf() -> *mut u8 {
    unsafe { SHM_BASE.add(fs_ipc::SHM_DATA_OFF) }
}

// ─── Shared memory setup ─────────────────────────────────────────────────────

fn setup_shm() -> bool {
    let ptr = mem_alloc_at(fs_ipc::SHM_PAGES, fs_ipc::SHM_BASE as u64);
    if ptr.is_null() {
        cos_dbg!("[init] FATAL: cannot alloc shm at {:#x}\n", fs_ipc::SHM_BASE);
        return false;
    }
    unsafe { SHM_BASE = ptr; }
    cos_dbg!("[init] shm @ {:#x}, {} pages\n", ptr as u64, fs_ipc::SHM_PAGES);
    true
}

// ─── Wait for FS server ready signal ─────────────────────────────────────────

fn wait_fs_ready(fs_tid: u32) -> bool {
    let deadline = ticks() + 5000;
    loop {
        if ticks() > deadline {
            cos_dbg!("[init] TIMEOUT waiting for FS ready\n");
            return false;
        }
        let mut msg = IpcMsg::zeroed();
        match libcosinus::ipc_recv(&mut msg) {
            Ok(()) if msg.tag == fs_ipc::TAG_FS_READY && msg.from == fs_tid => {
                cos_dbg!("[init] FS server ready (TID {})\n", fs_tid);
                return true;
            }
            Ok(()) => sched_yield(), // unrelated message — yield and retry
            Err(e) if e == err::AGAIN => sleep(10),
            Err(_) => sched_yield(),
        }
    }
}

// ─── FsHandle — open file via FS server IPC ──────────────────────────────────

pub struct FsHandle {
    fd:     u16,
    offset: u64,
    size:   u64,
}

impl FsHandle {
    pub fn open(path: &[u8], flags: u32) -> Option<Self> {
        let path_len = path.len().min(4095);
        unsafe {
            core::ptr::copy_nonoverlapping(path.as_ptr(), shm_path_buf(), path_len);
            *shm_path_buf().add(path_len) = 0;
        }
        let mut req = IpcMsg::zeroed();
        req.to      = fs_pid();
        req.tag     = fs_ipc::TAG_FS_REQUEST;
        req.data[0] = fs_ipc::OP_OPEN;
        req.data[1] = flags as u64;
        req.ptr     = shm_path_buf() as u64;
        req.len     = path_len as u32;

        ipc_send(&req).ok()?;
        let mut resp = IpcMsg::zeroed();
        ipc_recv_blocking(&mut resp).ok()?;

        let status = resp.data[0] as i64;
        if status < 0 { return None; }
        Some(FsHandle { fd: status as u16, offset: 0, size: resp.data[1] })
    }

    pub fn read(&mut self, buf: &mut [u8]) -> i64 {
        let count = buf.len().min(fs_ipc::SHM_DATA_SIZE);
        let mut req = IpcMsg::zeroed();
        req.to      = fs_pid();
        req.tag     = fs_ipc::TAG_FS_REQUEST;
        req.data[0] = fs_ipc::OP_READ;
        req.data[1] = self.fd as u64;
        req.data[2] = count as u64;
        req.data[3] = self.offset;
        req.ptr     = shm_data_buf() as u64;
        req.len     = count as u32;

        if ipc_send(&req).is_err() { return -1; }
        let mut resp = IpcMsg::zeroed();
        if ipc_recv_blocking(&mut resp).is_err() { return -1; }

        let n = resp.data[0] as i64;
        if n > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    shm_data_buf(),
                    buf.as_mut_ptr(),
                    (n as usize).min(buf.len()),
                );
            }
            self.offset += n as u64;
        }
        n
    }

    pub fn close(self) {
        let mut req = IpcMsg::zeroed();
        req.to      = fs_pid();
        req.tag     = fs_ipc::TAG_FS_REQUEST;
        req.data[0] = fs_ipc::OP_CLOSE;
        req.data[1] = self.fd as u64;
        let _ = ipc_send(&req); // fire-and-forget
    }

    pub fn size(&self) -> u64 { self.size }
}

// ─── Stat ─────────────────────────────────────────────────────────────────────

pub struct StatResult {
    pub ino:   u64,
    pub size:  u64,
    pub ftype: u8,
}

fn fs_stat(path: &[u8]) -> Option<StatResult> {
    let path_len = path.len().min(4095);
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), shm_path_buf(), path_len);
        *shm_path_buf().add(path_len) = 0;
    }
    let mut req = IpcMsg::zeroed();
    req.to      = fs_pid();
    req.tag     = fs_ipc::TAG_FS_REQUEST;
    req.data[0] = fs_ipc::OP_STAT;
    req.ptr     = shm_path_buf() as u64;
    req.len     = path_len as u32;

    ipc_send(&req).ok()?;
    let mut resp = IpcMsg::zeroed();
    ipc_recv_blocking(&mut resp).ok()?;

    if (resp.data[0] as i64) < 0 { return None; }
    Some(StatResult { ino: resp.data[1], size: resp.data[2], ftype: resp.data[3] as u8 })
}

// ─── Child process table ──────────────────────────────────────────────────────

struct ChildProc {
    tid:      u32,
    name:     [u8; 16],
    entry:    u64,
    arg:      u64,
    critical: bool, // restart on crash
    alive:    bool,
}

const MAX_CHILDREN: usize = 16;

// Use a raw static array — no heap needed for the supervisor table itself
static mut CHILDREN: [Option<ChildProc>; MAX_CHILDREN] = [
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
];

fn add_child(child: ChildProc) {
    unsafe {
        for slot in &raw mut CHILDREN {
            // SAFETY: single-threaded init process, no concurrent access
            let slot = &mut *slot;
            for s in slot.iter_mut() {
                if s.is_none() { *s = Some(child); return; }
            }
        }
    }
    cos_dbg!("[init] WARNING: children table full\n");
}

// ─── Supervisor loop ──────────────────────────────────────────────────────────

fn supervisor_loop() -> ! {
    cos_dbg!("[init] supervisor loop\n");
    const TAG_EXIT_NOTIFY: u32 = 0xDEAD_0001;

    loop {
        let mut msg = IpcMsg::zeroed();
        match libcosinus::ipc_recv(&mut msg) {
            Ok(()) if msg.tag == TAG_EXIT_NOTIFY => {
                let dead_tid = msg.data[0] as u32;
                cos_dbg!("[init] child exit TID={}\n", dead_tid);
                unsafe {
                    // SAFETY: single-threaded
                    for s in (&raw mut CHILDREN as *mut [Option<ChildProc>; MAX_CHILDREN])
                        .as_mut().unwrap().iter_mut()
                    {
                        if let Some(ref mut c) = s {
                            if c.tid == dead_tid { c.alive = false; }
                        }
                    }
                }
            }
            Ok(()) => {}
            Err(e) if e == err::AGAIN => sleep(50),
            Err(_) => sched_yield(),
        }

        // Restart critical dead children
        unsafe {
            for s in (&raw mut CHILDREN as *mut [Option<ChildProc>; MAX_CHILDREN])
                .as_mut().unwrap().iter_mut()
            {
                let Some(ref mut child) = s else { continue };
                if child.alive || !child.critical { continue; }
                cos_dbg!("[init] restarting critical process\n");
                let args = SpawnArgs {
                    entry:    child.entry,
                    arg:      child.arg,
                    stack_sz: 0,
                    flags:    spawn_flags::USER | spawn_flags::DETACH,
                    name:     child.name,
                };
                if let Ok(new_tid) = spawn(&args) {
                    child.tid   = new_tid;
                    child.alive = true;
                }
            }
        }
    }
}

// ─── /etc/init.conf parser ────────────────────────────────────────────────────

fn read_init_conf() -> Option<Vec<u8>> {
    let mut f = FsHandle::open(b"/etc/init.conf", 0)?;
    let size = f.size().min(4096) as usize;
    let mut buf = Vec::with_capacity(size);
    buf.resize(size, 0u8);
    let n = f.read(&mut buf);
    f.close();
    if n <= 0 { return None; }
    buf.truncate(n as usize);
    Some(buf)
}

fn parse_and_exec_conf(conf: &[u8]) {
    for line in conf.split(|&b| b == b'\n') {
        let line = trim(line);
        if line.is_empty() || line[0] == b'#' { continue; }
        if let Some(rest) = strip_prefix(line, b"spawn ") {
            spawn_named(rest);
        } else if line == b"mount_check" {
            run_mount_check();
        }
    }
}

fn trim(s: &[u8]) -> &[u8] {
    let s = match s.iter().position(|&b| !matches!(b, b' ' | b'\t' | b'\r')) {
        Some(i) => &s[i..],
        None    => return &[],
    };
    match s.iter().rposition(|&b| !matches!(b, b' ' | b'\t' | b'\r')) {
        Some(i) => &s[..i + 1],
        None    => s,
    }
}

fn strip_prefix<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    s.strip_prefix(prefix)
}

fn spawn_named(name: &[u8]) {
    let name = trim(name);
    cos_dbg!("[init] spawning: {}\n", core::str::from_utf8(name).unwrap_or("?"));

    // Build /bin/<name> path on the stack
    let mut path = [0u8; 64];
    path[0] = b'/'; path[1] = b'b'; path[2] = b'i'; path[3] = b'n'; path[4] = b'/';
    let nlen = name.len().min(58);
    path[5..5 + nlen].copy_from_slice(&name[..nlen]);

    match fs_stat(&path[..5 + nlen]) {
        Some(s) if s.ftype == 1 => {
            cos_dbg!("[init] found, size={}\n", s.size);
            // TODO: sys_exec once kernel ELF loader is ready
        }
        Some(_) => cos_dbg!("[init] not a regular file\n"),
        None    => cos_dbg!("[init] not found: /bin/{}\n",
            core::str::from_utf8(name).unwrap_or("?")),
    }
}

fn run_mount_check() {
    match FsHandle::open(b"/etc/version", 0) {
        None => cos_dbg!("[init] /etc/version not found\n"),
        Some(mut f) => {
            let mut buf = [0u8; 64];
            let n = f.read(&mut buf);
            f.close();
            if n > 0 {
                let s = core::str::from_utf8(&buf[..n as usize]).unwrap_or("(invalid utf8)");
                cos_dbg!("[init] version: {}\n", s);
                cos_println!("CosinusOS {}", s);
            }
        }
    }
}

// ─── Entry point ──────────────────────────────────────────────────────────────
//
// _arg = FS server PID, passed by kernel before jumping to init

#[no_mangle]
pub extern "C" fn _start(arg: u64) -> ! {
    let my_tid = thread_id();
    cos_dbg!("[init] started TID={} fs_pid={}\n", my_tid, arg);
    cos_println!("CosinusOS init");

    // Kernel passes FS server PID in arg
    unsafe { FS_SERVER_PID = arg as u32; }

    // Allocate shared memory window for FS IPC
    if !setup_shm() { libcosinus::exit(1); }

    // Wait for FS server to signal readiness
    if !wait_fs_ready(arg as u32) {
        cos_dbg!("[init] FATAL: FS server not ready\n");
        libcosinus::exit(1);
    }
    cos_println!("FS ready.");

    // Read and execute /etc/init.conf
    match read_init_conf() {
        Some(conf) => parse_and_exec_conf(&conf),
        None => {
            cos_dbg!("[init] no init.conf, spawning shell\n");
            spawn_named(b"shell");
        }
    }

    supervisor_loop()
}