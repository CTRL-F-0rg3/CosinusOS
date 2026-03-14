// CosinusOS Microkernel v3.5.2
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(unsafe_op_in_unsafe_fn)]

use core::{arch::asm, panic::PanicInfo, sync::atomic::Ordering};

pub mod sync;
pub mod debug;
pub mod mm;
pub mod perm;
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

// ── 8042 PS/2 controller init ─────────────────────────────────────────────────
unsafe fn init_ps2() {
    use debug::{inb, outb, serial_print, hex_str};

    let status = inb(0x64);
    serial_print("[8042] status=");
    { let mut b = [0u8;18]; serial_print(hex_str(status as u64, &mut b)); }
    serial_print("\n");

    if status == 0xFF {
        serial_print("[8042] brak kontrolera PS/2\n");
        return;
    }

    // Opróżnij output buffer
    if status & 0x01 != 0 { let _ = inb(0x60); }

    // Disable port 1 na czas konfiguracji
    while inb(0x64) & 0x02 != 0 {}
    outb(0x64, 0xAD);

    // Odczytaj Configuration Byte
    while inb(0x64) & 0x02 != 0 {}
    outb(0x64, 0x20);
    while inb(0x64) & 0x01 == 0 {}
    let mut cfg = inb(0x60);
    serial_print("[8042] cfg=");
    { let mut b = [0u8;18]; serial_print(hex_str(cfg as u64, &mut b)); }
    serial_print("\n");

    // IRQ1 enable (bit0=1), wyłącz translation (bit6=0)
    cfg |= 0x01;
    cfg &= !0x40;

    // Zapisz Configuration Byte
    while inb(0x64) & 0x02 != 0 {}
    outb(0x64, 0x60);
    while inb(0x64) & 0x02 != 0 {}
    outb(0x60, cfg);

    // Enable port 1
    while inb(0x64) & 0x02 != 0 {}
    outb(0x64, 0xAE);

    // Reset klawiatury
    while inb(0x64) & 0x02 != 0 {}
    outb(0x60, 0xFF);

    // Czekaj na BAT (0xAA)
    let mut tries = 0usize;
    loop {
        if inb(0x64) & 0x01 != 0 {
            let r = inb(0x60);
            serial_print("[8042] resp=");
            { let mut b = [0u8;18]; serial_print(hex_str(r as u64, &mut b)); }
            serial_print("\n");
            if r == 0xAA { break; }
        }
        tries += 1;
        if tries > 100_000 { serial_print("[8042] timeout\n"); break; }
        for _ in 0..10 { core::hint::spin_loop(); }
    }

    serial_print("[8042] OK\n");
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
        perm::init_idt(); debug::log_ok("IDT + IRQ1 keyboard", true);

        // ── 4. Scheduler ─────────────────────────────────────────────────────
        threading::sched_init();  debug::log_ok("Scheduler (idle thread)", true);
        perm::init_pit();         debug::log_ok("PIT 100Hz", true);

        // ── 5. PS/2 (po sched_init żeby TSS.rsp0 był ustawiony) ──────────────
        init_ps2();
        debug::log_ok("PS/2 8042", true);

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

        if mb_magic == userspace_loader::MB2_OK {
            debug::log_ok("MB2 magic", true);
            match userspace_loader::mb2_module(mb_info) {
                Some((s, e)) => {
                    debug::log_ok("Modul userspace", true);
                    print("  Adres: ");
                    { let mut b = [0u8;18]; print(hex_str(s, &mut b)); }
                    print(" - ");
                    { let mut b = [0u8;18]; print(hex_str(e, &mut b)); }
                    print("\n");
                    let ok = userspace_loader::load_userspace(s, e);
                    debug::log_ok("Uruchomienie userspace", ok);
                }
                None => {
                    debug::log_ok("Modul userspace", false);
                    printc("  Dodaj do grub.cfg: module2 /boot/userspace.bin\n", col::YELLOW);
                }
            }
        } else {
            debug::log_ok("MB2 magic", false);
            print("  Otrzymano: ");
            { let mut b = [0u8;18]; print(hex_str(mb_magic, &mut b)); }
            print("\n");
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