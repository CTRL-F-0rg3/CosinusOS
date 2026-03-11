// CosinusOS Microkernel v3.5.2
// Modularny split: mm / debug / perm / sync / threading / userspace_loader
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(unsafe_op_in_unsafe_fn)]

use core::{arch::asm, panic::PanicInfo, sync::atomic::Ordering};
pub mod ipc;
pub mod usb;
pub mod syscall_api;
pub mod sync;
pub mod debug;
pub mod mm;
pub mod perm;
pub mod threading;
pub mod userspace_loader;

// Re-eksport publicznego API kernela (używane przez userspace przez syscalle,
// tutaj jako dokumentacja interfejsu)
pub use mm::{PhysAddr, VirtAddr, PAGE_SIZE, PTE_W, PTE_U};
pub use mm::{mm_alloc, mm_free_phys, mm_free_kb, mm_used_kb, mm_total_kb};
pub use mm::{vmap, vunmap, virt_to_phys, valid_user, valid_buf, new_user_p4};
pub use debug::{col, print, printc, set_col, cls, serial_print, num_str, hex_str};
pub use threading::{spawn_k, spawn_user_on_cr3, thread_yield, TS, Thread, THREADS, CUR, NTHREADS};
pub use perm::{kb_pop, tss_rsp0, TICK};

// ── Terminal kernelowy ────────────────────────────────────────────────────────
static mut TERM_LINE: [u8; 256] = [0u8; 256];
static mut TERM_LEN:  usize     = 0;

unsafe fn term_prompt() {
    printc("\n#$> ", col::LGREEN);
}

unsafe fn term_process_cmd() {
    let line = core::str::from_utf8_unchecked(&TERM_LINE[..TERM_LEN]);
    print("\n");
    let cmd = line.trim_ascii();
    match cmd {
        "help" => {
            printc("=== CosinusOS Kernel Terminal ===\n", col::YELLOW);
            print("  help       - ta pomoc\n");
            print("  mem        - pamiec fizyczna\n");
            print("  threads    - lista watkow\n");
            print("  userspace  - uruchom/sprawdz userspace\n");
            print("  ticks      - licznik tickow\n");
            print("  uptime     - czas pracy\n");
            print("  cr3        - aktualny CR3\n");
            print("  regs       - rejestry CPU\n");
            print("  clear      - wyczysc ekran\n");
            print("  panic      - test kernel panic\n");
        }
        "mem" => {
            printc("=== Pamiec ===\n", col::YELLOW);
            print("  Wolne: "); pnum!(mm_free_kb()); print(" KB\n");
            print("  Uzyte: "); pnum!(mm_used_kb()); print(" KB\n");
            print("  Razem: "); pnum!(mm_total_kb()); print(" KB\n");
        }
        "threads" => {
            printc("=== Watki ===\n", col::YELLOW);
            let cur = CUR.load(Ordering::Relaxed);
            for i in 0..threading::MAX_THREADS {
                let t = &THREADS[i];
                if t.state == TS::Dead { continue; }
                let (ss, sc) = match t.state {
                    TS::Run   => (" RUN  ", col::LGREEN),
                    TS::Ready => (" READY", col::LCYAN),
                    TS::Block => (" BLOCK", col::YELLOW),
                    TS::Dead  => (" DEAD ", col::DGREY),
                };
                print(if i == cur { "  * #" } else { "    #" });
                pnum!(i); print(" ");
                print(t.name_str());
                printc(ss, sc);
                print(" ticks="); pnum!(t.ticks as usize); print("\n");
            }
        }
        "userspace" => {
            let mut found = false;
            for i in 0..threading::MAX_THREADS {
                if THREADS[i].state == TS::Dead { continue; }
                if THREADS[i].name_str().trim_end_matches('\0') == "userspace" {
                    printc("Userspace dziala jako watek #", col::LGREEN);
                    pnum!(i); print("\n");
                    found = true; break;
                }
            }
            if !found {
                let entry = userspace_loader::US_ENTRY;
                if entry != 0 {
                    printc("Uruchamiam userspace @ ", col::LCYAN);
                    phex!(entry); print("\n");
                    let cr3 = new_user_p4();
                    let tid = spawn_user_on_cr3("userspace\0", entry, 0, cr3);
                    if tid >= 0 { printc("  Watek #", col::LGREEN); pnum!(tid as usize); print(" OK\n"); }
                    else        { printc("  Brak slotow!\n", col::LRED); }
                } else {
                    printc("Brak zaladowanego userspace (brak modulu MB2)\n", col::LRED);
                }
            }
        }
        "ticks"  => { print("Ticks: "); pnum!(TICK as usize); print("\n"); }
        "uptime" => {
            print("Uptime: "); pnum!((TICK / 100) as usize);
            print("s ("); pnum!(TICK as usize); print(" ticks)\n");
        }
        "cr3" => {
            let cr3: u64; asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
            print("CR3="); phex!(cr3); print("\n");
        }
        "regs" => {
            let (mut rsp, mut rbp, mut cr3, mut cr2, mut rfl): (u64,u64,u64,u64,u64);
            asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack));
            asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack));
            asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
            asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack));
            asm!("pushfq; pop {}", out(reg) rfl, options(nomem));
            printc("=== Rejestry ===\n", col::YELLOW);
            print("  RSP="); phex!(rsp); print("  RBP="); phex!(rbp); print("\n");
            print("  CR3="); phex!(cr3); print("  CR2="); phex!(cr2); print("\n");
            print("  RFLAGS="); phex!(rfl); print("\n");
        }
        "clear" => { cls(); }
        "panic" => { panic_no_dyn("Test panic z terminala"); }
        "" => {}
        _ => { printc("Nieznana: ", col::LRED); print(cmd); print("\nWpisz 'help'\n"); }
    }
    TERM_LEN = 0;
}

unsafe fn term_handle_char(c: char) {
    use debug::{VGA_LOCK, CUR_X, putc, com_write};
    match c {
        '\n' | '\r' => { term_process_cmd(); term_prompt(); }
        '\x08' => {
            if TERM_LEN > 0 {
                TERM_LEN -= 1;
                VGA_LOCK.lock();
                if CUR_X > 0 { CUR_X -= 1; }
                putc(' ');
                if CUR_X > 0 { CUR_X -= 1; }
                debug::cursor_hw_pub();
                VGA_LOCK.unlock();
                com_write('\x08'); com_write(' '); com_write('\x08');
            }
        }
        c if (c as u32) >= 0x20 && (c as u32) < 0x7F => {
            if TERM_LEN < 255 {
                TERM_LINE[TERM_LEN] = c as u8; TERM_LEN += 1;
                VGA_LOCK.lock(); putc(c); VGA_LOCK.unlock();
                com_write(c);
            }
        }
        _ => {}
    }
}

unsafe extern "C" fn kernel_terminal(_: u64) -> ! {
    use debug::com_read;
    printc("\n=== CosinusOS Kernel Terminal ===\n", col::YELLOW);
    print("  Klawiatura PS/2 + COM1 (115200). Wpisz 'help'.\n");
    term_prompt();
    loop {
        let mut got = false;
        while let Some(c) = kb_pop()   { term_handle_char(c); got = true; }
        while let Some(c) = com_read() { debug::com_write(c); term_handle_char(c); got = true; }
        if !got { thread_yield(); }
    }
}

// ── Panic ────────────────────────────────────────────────────────────────────
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
        if let Some(l) = info.location() { debug::print_raw(" @ "); debug::print_raw(l.file()); }
        debug::print_raw("  \n");
        debug::VCOLOR = col::WHITE;
    }
    loop { unsafe { asm!("hlt", options(nomem, nostack)); } }
}

// ── Makra lokalne (używają debug:: ścieżki) ──────────────────────────────────
macro_rules! pnum {
    ($v:expr) => {{ let mut b = [0u8; 24]; print(num_str($v as usize, &mut b)); }};
}
macro_rules! phex {
    ($v:expr) => {{ let mut b = [0u8; 18]; print(hex_str($v as u64, &mut b)); }};
}

// ── kernel_main ──────────────────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn kernel_main(mb_magic: u64, mb_info: u64) -> ! {
    unsafe {
        cls();
        debug::serial_init();
        let usb_ok = usb::usb_init();
        debug::log_ok("USB XHCI/EHCI+HID", usb_ok);
        spawn_k("usb\0", usb::usb_thread as *const () as u64, 0);
        set_col(col::attr(col::LCYAN, col::BLACK));
        print(" ===========================\n");
        print("  CosinusOS Microkernel v3.5\n");
        print(" ===========================\n\n");
        set_col(col::WHITE);
        serial_print("=== CosinusOS v3.5 boot ===\n");

        mm::mm_init(0x0100_0000, 0x0F00_0000);
        mm::vmm_init(0x1000);
        debug::log_ok("PMM + VMM", true);

        perm::init_gdt(); debug::log_ok("GDT", true);
        perm::init_pic(); debug::log_ok("PIC", true);
        perm::init_idt(); debug::log_ok("IDT + IRQ1 keyboard", true);

        threading::sched_init(); debug::log_ok("Scheduler (idle thread)", true);
        perm::init_pit();        debug::log_ok("PIT 100Hz", true);

        print("\n");
        printc("=== Userspace ===\n", col::YELLOW);

        if mb_magic == userspace_loader::MB2_OK {
            debug::log_ok("MB2 magic", true);
            match userspace_loader::mb2_module(mb_info) {
                Some((s, e)) => {
                    debug::log_ok("Modul userspace", true);
                    print("  Adres: "); phex!(s); print(" - "); phex!(e); print("\n");
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
            print("  Otrzymano: "); phex!(mb_magic); print("\n");
        }

        print("\n");
        printc("=== Kernel Terminal ===\n", col::YELLOW);
        //let t = spawn_k("kterminal\0", kernel_terminal as *const () as u64, 0);
        //debug::log_ok("Kernel debug terminal (PS/2 + COM1)", t >= 0);

        print("\n");
        printc("=== system ===\n", col::YELLOW);
        print("  memory: "); pnum!(mm_free_kb()); print(" KB\n");
        print("  Watki: "); pnum!(NTHREADS.load(Ordering::Relaxed)); print("\n");

        print("\n");
        set_col(col::attr(col::BLACK, col::LGREEN));
        print(" [ COMPLETE ] \n");
        set_col(col::attr(col::YELLOW, col::BLACK));
        print("########################################################\n");

        set_col(col::WHITE); print("\n\n");
        serial_print("[OK] boot complete\n");

        threading::schedule();
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}