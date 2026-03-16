// CosinusOS Microkernel v3.5.2
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(unsafe_op_in_unsafe_fn)]

use core::{arch::asm, panic::PanicInfo, sync::atomic::Ordering};

pub mod sync;
pub mod debug;
pub mod mm;
pub mod valloc;
pub mod perm;
pub mod input;
pub mod threading;
pub mod userspace_loader;
pub mod syscall_api;
pub mod ipc;
pub mod usb;
pub mod display;
pub mod kterminal;

pub use mm::{PhysAddr, VirtAddr, PAGE_SIZE, PTE_W, PTE_U};
pub use mm::{mm_alloc, mm_free_phys, mm_free_kb, mm_used_kb, mm_total_kb};
pub use mm::{vmap, vunmap, virt_to_phys, valid_user, valid_buf, new_user_p4};
pub use debug::{col, print, printc, set_col, cls, serial_print, num_str, hex_str};
pub use threading::{spawn_k, spawn_user_on_cr3, thread_yield, TS, Thread, THREADS, CUR, NTHREADS};
pub use perm::{kb_pop, tss_rsp0, TICK};

// ── Panic ─────────────────────────────────────────────────────────────────────
pub fn panic_no_dyn(msg: &str) -> ! {
    unsafe {
        asm!("cli", options(nomem, nostack));
        debug::VCOLOR = col::attr(col::WHITE, col::RED);
        debug::print_raw("\n  *** KERNEL PANIC ***  \n  ");
        debug::print_raw(msg);
        debug::print_raw("  \n");
        debug::VCOLOR = col::WHITE;
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        asm!("cli", options(nomem, nostack));
        debug::VCOLOR = col::attr(col::WHITE, col::RED);
        debug::print_raw("\n  *** KERNEL PANIC ***  \n  ");
        if let Some(s) = info.message().as_str() { debug::print_raw(s); }
        else { debug::print_raw("(no message)"); }
        if let Some(l) = info.location() {
            debug::print_raw(" @ "); debug::print_raw(l.file());
        }
        debug::print_raw("  \n");
        debug::VCOLOR = col::WHITE;
    }
    loop { unsafe { asm!("hlt", options(nomem, nostack)); } }
}

// ── kernel_main ───────────────────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn kernel_main(mb_magic: u64, mb_info: u64) -> ! {
    unsafe {
        // ── 1. Podstawowe I/O ────────────────────────────────────────────────
        cls();
        debug::serial_init();

        set_col(col::attr(col::LCYAN, col::BLACK));
        print(" ===========================\n");
        print("  CosinusOS Microkernel v3.5\n");
        print(" ===========================\n\n");
        set_col(col::WHITE);
        serial_print("=== CosinusOS v3.5 boot ===\n");

        // ── 2. Pamięć ────────────────────────────────────────────────────────
        mm::mm_init(0x0100_0000, 0x0F00_0000);
        mm::vmm_init(0x1000);
        debug::log_ok("PMM + VMM", true);

        // ── 3. CPU ───────────────────────────────────────────────────────────
        perm::init_gdt(); debug::log_ok("GDT", true);
        perm::init_pic(); debug::log_ok("PIC", true);
        perm::init_idt(); debug::log_ok("IDT + IRQ1 + IRQ12", true);

        // ── 4. Scheduler ─────────────────────────────────────────────────────
        threading::sched_init(); debug::log_ok("Scheduler (idle thread)", true);
        perm::init_pit();        debug::log_ok("PIT 100Hz", true);

        // ── 5. PS/2 — po sched_init żeby TSS.rsp0 był ustawiony ──────────────
        input::init_ps2();
        debug::log_ok("PS/2 keyboard + mouse", true);

        // ── 6. Display ───────────────────────────────────────────────────────
        let disp_ok = display::display_init();
        debug::log_ok("Display HDMI/DP", disp_ok);

        // ── 7. USB ───────────────────────────────────────────────────────────
        let usb_ok = usb::usb_init();
        debug::log_ok("USB XHCI/EHCI/OHCI + HID", usb_ok);
        if usb_ok {
            spawn_k("usb\0", usb::usb_thread as *const () as u64, 0);
        }

        // ── 8. Kernel terminal ───────────────────────────────────────────────
        spawn_k("kterminal\0", kterminal::run as *const () as u64, 0);
        debug::log_ok("Kernel terminal (PS/2 + COM1)", true);

        // ── 9. Userspace ─────────────────────────────────────────────────────
        print("\n");
        printc("=== Userspace ===\n", col::YELLOW);

        // ── Próba załadowania userspace ───────────────────────────────────────
        // Kolejność priorytetu:
        //   1. Moduł Multiboot2 (GRUB module2 /boot/userspace.bin)
        //   2. Embedded blob w obrazie kernela (_userspace_blob_start/end)
        //   3. Brak — kernel działa tylko z kterminalem

        let loaded: bool = 'load: {
            // ── 1. Multiboot2 moduł ────────────────────────────────────────────
            if mb_magic == userspace_loader::MB2_OK {
                debug::log_ok("MB2 magic", true);
                if let Some((s, e)) = userspace_loader::mb2_module(mb_info) {
                    debug::log_ok("Modul MB2", true);
                    print("  Adres: ");
                    { let mut b = [0u8;18]; print(hex_str(s, &mut b)); }
                    print(" - ");
                    { let mut b = [0u8;18]; print(hex_str(e, &mut b)); }
                    print("\n");
                    let ok = userspace_loader::load_userspace(s, e);
                    debug::log_ok("Zaladowanie MB2", ok);
                    if ok { break 'load true; }
                    printc("  MB2 load failed — probuje embedded\n", col::YELLOW);
                } else {
                    debug::log_ok("Modul MB2", false);
                    printc("  Brak modulu. Dodaj do grub.cfg:\n", col::YELLOW);
                    printc("    module2 /boot/userspace.bin\n", col::YELLOW);
                    printc("  Probuje embedded...\n", col::YELLOW);
                }
            } else {
                debug::log_ok("MB2 magic", false);
                print("  Magic=");
                { let mut b=[0u8;18]; print(hex_str(mb_magic, &mut b)); }
                print(" (brak GRUB) — probuje embedded\n");
            }

            // ── 2. Embedded blob ──────────────────────────────────────────────
            let ok = userspace_loader::load_embedded();
            debug::log_ok("Zaladowanie embedded", ok);
            break 'load ok;
        };

        if !loaded {
            printc("  UWAGA: userspace nie zaladowany!\n", col::LRED);
            printc("  Kernel dziala tylko w trybie kterminal.\n", col::YELLOW);
            printc("  Aby uruchomic userspace:\n", col::YELLOW);
            printc("    QEMU:  -initrd build/userspace.bin\n", col::LGREY);
            printc("    GRUB:  module2 /boot/userspace.bin\n", col::LGREY);
        }

        // ── 10. Info systemowe ───────────────────────────────────────────────
        print("\n");
        printc("=== system ===\n", col::YELLOW);
        print("  memory: ");
        { let mut b = [0u8;24]; print(num_str(mm_free_kb(), &mut b)); }
        print(" KB\n");
        print("  Watki: ");
        { let mut b = [0u8;24]; print(num_str(NTHREADS.load(Ordering::Relaxed), &mut b)); }
        print("\n");

        print("\n");
        set_col(col::attr(col::BLACK, col::LGREEN));
        print(" [ COMPLETE ] \n");
        set_col(col::attr(col::YELLOW, col::BLACK));
        print("########################################################\n");
        set_col(col::WHITE);
        print("\n\n");
        serial_print("[OK] boot complete\n");

        threading::schedule();
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}