// CosinusOS — kterminal.rs
// Kernel debug terminal: PS/2 klawiatura + COM1 serial
// Uruchamiany jako wątek kernelowy spawn_k("kterminal")

use core::sync::atomic::Ordering;
use core::arch::asm;
use crate::debug::{col, print, printc, set_col, cls, num_str, hex_str,
                   VGA_LOCK, CUR_X, putc, com_write, com_read, cursor_hw_pub};
use crate::mm::{mm_free_kb, mm_used_kb, mm_total_kb};
use crate::threading::{THREADS, CUR, NTHREADS, MAX_THREADS, TS, thread_yield};
use crate::perm::{kb_pop, TICK};
use crate::userspace_loader;
use crate::mm::new_user_p4;
use crate::threading::spawn_user_on_cr3;

static mut LINE: [u8; 256] = [0u8; 256];
static mut LEN:  usize     = 0;

unsafe fn prompt() {
    printc("\n#$> ", col::LGREEN);
}

unsafe fn process_cmd() {
    let line = core::str::from_utf8_unchecked(&LINE[..LEN]);
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
            print("  usb        - status USB/HID\n");
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
            for i in 0..MAX_THREADS {
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
            for i in 0..MAX_THREADS {
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
                    if tid >= 0 {
                        printc("  Watek #", col::LGREEN);
                        pnum!(tid as usize); print(" OK\n");
                    } else {
                        printc("  Brak slotow!\n", col::LRED);
                    }
                } else {
                    printc("Brak zaladowanego userspace\n", col::LRED);
                }
            }
        }

        "ticks"  => { print("Ticks: "); pnum!(TICK as usize); print("\n"); }

        "uptime" => {
            print("Uptime: "); pnum!((TICK / 100) as usize);
            print("s ("); pnum!(TICK as usize); print(" ticks)\n");
        }

        "cr3" => {
            let cr3: u64;
            asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
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

        "usb" => {
            printc("=== USB/HID ===\n", col::YELLOW);
            let ok = crate::usb::usb_ok();
            print("  Kontroler: ");
            if ok { printc("OK\n", col::LGREEN); } else { printc("brak\n", col::LRED); }
            let n = crate::usb::usb_hid_count();
            print("  HID devices: "); pnum!(n); print("\n");
        }

        "clear" => { cls(); }

        "panic" => { crate::panic_no_dyn("Test panic z terminala"); }

        "" => {}

        _ => {
            printc("Nieznana komenda: ", col::LRED);
            print(cmd); print("\nWpisz 'help'\n");
        }
    }

    LEN = 0;
}

unsafe fn handle_char(c: char) {
    match c {
        '\n' | '\r' => { process_cmd(); prompt(); }
        '\x08' => {
            if LEN > 0 {
                LEN -= 1;
                VGA_LOCK.lock();
                if CUR_X > 0 { CUR_X -= 1; }
                putc(' ');
                if CUR_X > 0 { CUR_X -= 1; }
                cursor_hw_pub();
                VGA_LOCK.unlock();
                com_write('\x08'); com_write(' '); com_write('\x08');
            }
        }
        c if (c as u32) >= 0x20 && (c as u32) < 0x7F => {
            if LEN < 255 {
                LINE[LEN] = c as u8; LEN += 1;
                VGA_LOCK.lock(); putc(c); VGA_LOCK.unlock();
                com_write(c);
            }
        }
        _ => {}
    }
}

pub unsafe extern "C" fn run(_: u64) -> ! {
    printc("\n=== Kernel Terminal ===\n", col::YELLOW);
    print("  PS/2 + COM1 (115200). Wpisz 'help'.\n");
    prompt();
    loop {
        let mut got = false;
        while let Some(c) = kb_pop()   { handle_char(c); got = true; }
        while let Some(c) = com_read() { com_write(c); handle_char(c); got = true; }
        if !got { thread_yield(); }
    }
}

// Lokalne makra (nie kolidują z makrami w lib.rs)
macro_rules! pnum {
    ($v:expr) => {{ let mut b = [0u8;24]; print(num_str($v as usize, &mut b)); }};
}
macro_rules! phex {
    ($v:expr) => {{ let mut b = [0u8;18]; print(hex_str($v as u64, &mut b)); }};
}
use pnum;
use phex;