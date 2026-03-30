// devspace/src/main.rs — DevSpace Ring-1 ELF entry point
//
// Boot sequence:
//   1. Kernel loads devspace.elf at its link address via multiboot2 module
//   2. Kernel sets IOPL=1 in EFLAGS, then jumps here
//   3. We init all drivers and enter the IPC event loop
//
// Memory map (kernel sets up before jumping here):
//   IPC_DEVSPACE_BASE  = req ring  (R/W, shared with Ring-3)
//   IPC_DEVSPACE_BASE+PAGE = resp slot (R/W, shared with Ring-3)

#![no_std]
#![no_main]

use devspace::{
    serial_print, serial_print_u32, serial_print_hex,
    IPC_REQ_RING_ADDR, IPC_RESP_ADDR, IPC_PAGE_SIZE,
};
use devspace::drivers::drive::api::{
    DiskResponse, IpcRing, ERR_UNSUPPORTED,
};
use devspace::drivers::drive::AtaDriver;

// ── Global driver instance ────────────────────────────────────────────────────

static mut ATA: Option<AtaDriver> = None;

// ── Entry point ───────────────────────────────────────────────────────────────
// Kernel jumps here after setting IOPL=1.
// _arg: u64 passed in rdi — reserved for future use (e.g. boot info pointer)

#[no_mangle]
pub extern "C" fn _start(_arg: u64) -> ! {
    serial_print(b"[DS] DevSpace starting\n");

    // ── Init ATA driver ───────────────────────────────────────────────────────
    let mut drv = AtaDriver::new();
    let disk_ok = drv.init();

    serial_print(b"[DS] ATA init: ");
    serial_print(if disk_ok { b"OK\n" } else { b"no drive\n" });

    unsafe { ATA = Some(drv); }

    // ── IPC ring pointers ─────────────────────────────────────────────────────
    let ring     = unsafe { &mut *(IPC_REQ_RING_ADDR as *mut IpcRing) };
    let resp_ptr = IPC_RESP_ADDR as *mut DiskResponse;

    serial_print(b"[DS] IPC loop ready\n");

    // ── Event loop ────────────────────────────────────────────────────────────
    let mut last_read: u32 = 0;

    loop {
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

        let wi = ring.write_idx;
        if wi == last_read {
            // No pending requests — yield (Ring-1 still yields through kernel)
            unsafe {
                core::arch::asm!(
                    "int 0x80",
                    in("rax") 3u64,   // SYS_YIELD
                    options(nostack)
                );
            }
            continue;
        }

        // Drain all pending slots
        while last_read != wi {
            let slot = (last_read as usize) % 60;
            let req  = ring.slots[slot];
            last_read = last_read.wrapping_add(1);

            serial_print(b"[DS] req op=");
            serial_print_u32(req.req_type as u32);
            serial_print(b" lba=");
            serial_print_u32(req.lba as u32);
            serial_print(b"\n");

            let resp = unsafe {
                match ATA.as_mut() {
                    Some(d) => d.handle_request(req),
                    None    => DiskResponse::err(req.req_id, ERR_UNSUPPORTED),
                }
            };

            unsafe { *resp_ptr = resp; }
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        }
    }
}