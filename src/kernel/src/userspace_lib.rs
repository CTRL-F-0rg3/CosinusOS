// CosinusOS — userspace_lib.rs

#![no_std]
#![allow(unused_variables)]
pub use crate::syscall_api::{IpcMsg, SpawnArgs, ThreadInfo, TimeInfo, spawn_flags};
pub use crate::syscall_api::{nr, err};

#[inline(always)]
pub unsafe fn syscall0(nr: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") nr as i64 => ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") nr as i64 => ret,
        in("rdi") a1,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") nr as i64 => ret,
        in("rdi") a1,
        in("rsi") a2,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") nr as i64 => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline]
pub unsafe fn exit() -> ! {
    syscall0(nr::EXIT);
    core::hint::unreachable_unchecked()
}

#[inline]
pub unsafe fn write(fd: u64, buf: &[u8]) -> i64 {
    syscall3(nr::WRITE, fd, buf.as_ptr() as u64, buf.len() as u64)
}

#[inline]
pub unsafe fn read(fd: u64, buf: &mut [u8]) -> i64 {
    syscall3(nr::READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64)
}

#[inline]
pub unsafe fn sched_yield() {
    syscall0(nr::YIELD);
}

#[inline]
pub unsafe fn sleep_ticks(ticks: u64) {
    syscall1(nr::SLEEP, ticks);
}

#[inline]
pub unsafe fn spawn(args: &SpawnArgs) -> i64 {
    syscall1(nr::SPAWN, args as *const SpawnArgs as u64)
}

pub unsafe fn spawn_simple(entry: unsafe extern "C" fn(u64) -> !, name: &[u8; 16], arg: u64) -> i64 {
    let mut args = SpawnArgs {
        entry:    entry as u64,
        arg,
        stack_sz: 0,
        flags:    spawn_flags::USER,
        name:     *name,
    };
    spawn(&args)
}

#[inline]
pub unsafe fn mem_alloc(pages: usize) -> i64 {
    syscall2(nr::MEM_ALLOC, pages as u64, 0)
}

#[inline]
pub unsafe fn mem_alloc_at(pages: usize, hint: u64) -> i64 {
    syscall2(nr::MEM_ALLOC, pages as u64, hint)
}

#[inline]
pub unsafe fn mem_free(ptr: *mut u8, pages: usize) -> i64 {
    syscall2(nr::MEM_FREE, ptr as u64, pages as u64)
}

#[inline]
pub unsafe fn ipc_send(msg: &IpcMsg) -> i64 {
    syscall1(nr::IPC_SEND, msg as *const IpcMsg as u64)
}

#[inline]
pub unsafe fn ipc_recv(msg: &mut IpcMsg) -> i64 {
    syscall2(nr::IPC_RECV, msg as *mut IpcMsg as u64, 0)
}

#[inline]
pub unsafe fn ipc_recv_blocking(msg: &mut IpcMsg) -> i64 {
    syscall2(nr::IPC_RECV, msg as *mut IpcMsg as u64, 1)
}

#[inline]
pub unsafe fn ipc_poll() -> usize {
    let r = syscall1(nr::IPC_POLL, 0);
    if r < 0 { 0 } else { r as usize }
}

#[inline]
pub unsafe fn thread_id() -> u32 {
    syscall1(nr::THREAD_ID, 0) as u32
}

#[inline]
pub unsafe fn thread_info(out: &mut ThreadInfo) -> i64 {
    syscall1(nr::THREAD_ID, out as *mut ThreadInfo as u64)
}

#[inline]
pub unsafe fn time_ticks() -> u64 {
    syscall0(nr::TIME) as u64
}

#[inline]
pub unsafe fn time_info(out: &mut TimeInfo) -> i64 {
    syscall1(nr::TIME, out as *mut TimeInfo as u64)
}

#[inline]
pub unsafe fn debug_print(s: &[u8]) -> i64 {
    syscall3(nr::DEBUG_PRINT, 0, s.as_ptr() as u64, s.len() as u64)
}


#[macro_export]
macro_rules! print {
    ($s:literal) => {
        unsafe { $crate::userspace_lib::write(1, $s.as_bytes()); }
    };
}


#[macro_export]
macro_rules! println {
    ($s:literal) => {
        unsafe {
            $crate::userspace_lib::write(1, $s.as_bytes());
            $crate::userspace_lib::write(1, b"\n");
        }
    };
}

