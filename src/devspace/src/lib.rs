// devspace/src/lib.rs — DevSpace Ring-1 driver library
//
// DevSpace runs as a privileged ELF process at Ring-1 (IOPL=1).
// This means IN/OUT instructions work directly without syscalls.
// The kernel sets IOPL=1 in EFLAGS before jumping to DevSpace entry.
//
// Module layout:
//   drivers/drive/  — ATA PIO disk driver (Rust + Forth + ASM)
//   drivers/gpu/    — GPU drivers (future)
//   drivers/usb/    — USB drivers (future)
//   ipc             — IPC channel to Ring-0 kernel and Ring-3 userspace

#![no_std]
#![allow(dead_code)]

// ── Panic handler ─────────────────────────────────────────────────────────────

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Best-effort serial print, then halt
    if let Some(loc) = info.location() {
        serial_print(b"[DEVSPACE PANIC] ");
        serial_print(loc.file().as_bytes());
        serial_print(b":");
        serial_print_u32(loc.line());
        serial_print(b"\n");
    } else {
        serial_print(b"[DEVSPACE PANIC]\n");
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nostack, nomem)); }
    }
}

// ── Serial debug output (port 0xE9 — QEMU debug port) ────────────────────────

#[inline(always)]
pub fn serial_putc(c: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xE9u16,
            in("al") c,
            options(nostack, nomem)
        );
    }
}

pub fn serial_print(s: &[u8]) {
    for &b in s { serial_putc(b); }
}

pub fn serial_print_u32(mut v: u32) {
    if v == 0 { serial_putc(b'0'); return; }
    let mut buf = [0u8; 10];
    let mut i = 10usize;
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    serial_print(&buf[i..]);
}

pub fn serial_print_u64(mut v: u64) {
    if v == 0 { serial_putc(b'0'); return; }
    let mut buf = [0u8; 20];
    let mut i = 20usize;
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    serial_print(&buf[i..]);
}

pub fn serial_print_hex(v: u64) {
    const HEX: &[u8] = b"0123456789abcdef";
    serial_print(b"0x");
    for shift in (0..16).rev() {
        serial_putc(HEX[((v >> (shift * 4)) & 0xF) as usize]);
    }
}

// ── Port I/O — Ring-1 has IOPL=1 so IN/OUT work without syscalls ─────────────

#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nostack, nomem));
    v
}

#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nostack, nomem));
}

#[inline(always)]
pub unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx", out("ax") v, in("dx") port, options(nostack, nomem));
    v
}

#[inline(always)]
pub unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nostack, nomem));
}

// ── IPC shared memory layout ──────────────────────────────────────────────────
// Kernel maps these pages into DevSpace address space at startup.
// Ring-3 userspace maps the same pages (read/write depending on direction).

pub const IPC_DEVSPACE_BASE: usize = 0x0000_6000_0000_0000;
pub const IPC_PAGE_SIZE:     usize = 0x1000;

// Page 0: Ring-3 → DevSpace request ring
pub const IPC_REQ_RING_ADDR: usize = IPC_DEVSPACE_BASE;
// Page 1: DevSpace → Ring-3 response slot (single response, latched)
pub const IPC_RESP_ADDR:     usize = IPC_DEVSPACE_BASE + IPC_PAGE_SIZE;

// ── Driver modules ────────────────────────────────────────────────────────────

pub mod drivers {
    pub mod drive;
    // pub mod gpu;   // future
    // pub mod usb;   // future
}