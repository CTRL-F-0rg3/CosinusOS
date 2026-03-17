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
        asm!("cli", options(nomem, nostack));

        cls(); debug::serial_init();
        set_col(col::attr(col::LCYAN, col::BLACK));
        print(" ===========================\n  CosinusOS Microkernel v3.5\n ===========================\n\n");
        set_col(col::WHITE);
        serial_print("=== CosinusOS v3.5 boot ===\n");

        mm::mm_init(0x0100_0000, 0x0F00_0000);
        mm::vmm_init(0x1000);
        debug::log_ok("PMM + VMM", true);

        perm::init_gdt(); debug::log_ok("GDT", true);
        perm::init_pic(); debug::log_ok("PIC", true);
        perm::init_idt(); asm!("cli", options(nomem, nostack));
        debug::log_ok("IDT + IRQ1 + IRQ12", true);

        threading::sched_init(); debug::log_ok("Scheduler (idle thread)", true);
        perm::init_pit(); asm!("cli", options(nomem, nostack));
        debug::log_ok("PIT 100Hz", true);

        input::init_ps2(); asm!("cli", options(nomem, nostack));
        debug::log_ok("PS/2 keyboard + mouse", true);

        let disp_ok = display::display_init();
        debug::log_ok("Display HDMI/DP", disp_ok);

        let usb_ok = usb::usb_init();
        debug::log_ok("USB XHCI/EHCI/OHCI + HID", usb_ok);
        if usb_ok { spawn_k("usb\0", usb::usb_thread as *const () as u64, 0); }

        // Kernel terminal — spawnuj ale nie uruchamiaj jeszcze
        spawn_k("kterminal\0", kterminal::run as *const () as u64, 0);
        debug::log_ok("Kernel terminal (PS/2 + COM1)", true);

        // Załaduj userspace (zapisuje entry/stack/cr3, nie spawnuje wątku)
        print("\n"); printc("=== Userspace ===\n", col::YELLOW);
        let loaded = 'load: {
            if mb_magic == userspace_loader::MB2_OK {
                debug::log_ok("MB2 magic", true);
                if let Some((s, e)) = userspace_loader::mb2_module(mb_info) {
                    debug::log_ok("Modul MB2", true);
                    { let mut b=[0u8;18]; print(hex_str(s,&mut b)); }
                    print(" - ");
                    { let mut b=[0u8;18]; print(hex_str(e,&mut b)); }
                    print("\n");
                    let ok = userspace_loader::load_userspace(s, e);
                    debug::log_ok("Zaladowanie MB2", ok);
                    if ok { break 'load true; }
                }
                debug::log_ok("Modul MB2", false);
            } else { debug::log_ok("MB2 magic", false); }
            let ok = userspace_loader::load_embedded();
            debug::log_ok("Embedded", ok);
            break 'load ok;
        };

        print("\n");
        { let mut b=[0u8;24]; print(num_str(mm_free_kb(),&mut b)); }
        print(" KB free  ");
        { let mut b=[0u8;24]; print(num_str(NTHREADS.load(Ordering::Relaxed),&mut b)); }
        print(" threads\n");
        set_col(col::attr(col::BLACK, col::LGREEN)); print(" [ COMPLETE ] \n");
        set_col(col::WHITE); print("\n");
        serial_print("[OK] boot complete\n");

        if loaded {
            // Userspace załadowany — skocz bezpośrednio do ring-3
            // Scheduler zacznie działać przez timer IRQ po wejściu do userspace
            serial_print("[OK] launching userspace directly\n");
            userspace_loader::run_userspace_direct();
        } else {
            // Brak userspace — uruchom kterminal przez scheduler
            serial_print("[OK] no userspace, starting kterminal\n");
            threading::jump_to_scheduler();
        }
    }
}