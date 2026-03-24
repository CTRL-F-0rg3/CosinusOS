// libcosinus — panic.rs
// Panic handler i abort dla procesów userspace.
//
// Przy panicu: drukuje lokalizację na stderr + wywołuje exit(101).
// Nie używa alokacji — pisze bezpośrednio przez syscall write.

use core::panic::PanicInfo;
use crate::fmt::FmtBuf;

/// Wywoływany przez Rust przy każdym panic!().
/// Drukuje komunikat na stderr i kończy proces kodem 101.
#[panic_handler]
fn panic_handler(info: &PanicInfo) -> ! {
    // Drukujemy bez alokacji — FmtBuf na stosie
    let mut buf = FmtBuf::<256>::new();

    buf.push_str("\n[PANIC]");

    if let Some(loc) = info.location() {
        buf.push_str(" ");
        buf.push_str(loc.file());
        buf.push_str(":");
        buf.push_u64(loc.line() as u64);
    }

    if let Some(msg) = info.message().as_str() {
        buf.push_str(" — ");
        buf.push_str(msg);
    }

    buf.push_str("\n");
    crate::eprint(buf.as_str());

    abort(101)
}

/// Zakończ proces podanym kodem błędu. Nigdy nie wraca.
pub fn abort(code: i32) -> ! {
    crate::exit(code)
}

/// Zakończ z kodem 0 (sukces).
pub fn quit() -> ! {
    crate::exit(0)
}

/// assert! który działa w no_std przez panic!
#[macro_export]
macro_rules! cos_assert {
    ($cond:expr) => {
        if !$cond { panic!(concat!("assertion failed: ", stringify!($cond))); }
    };
    ($cond:expr, $msg:literal) => {
        if !$cond { panic!($msg); }
    };
}

/// unwrap() z czytelnym komunikatem zamiast "called unwrap on None"
#[macro_export]
macro_rules! cos_unwrap {
    ($expr:expr) => {
        match $expr {
            Some(v) => v,
            None    => panic!(concat!("unwrap failed: ", stringify!($expr))),
        }
    };
    ($expr:expr, $msg:literal) => {
        match $expr {
            Some(v) => v,
            None    => panic!($msg),
        }
    };
}

/// unwrap() dla Result
#[macro_export]
macro_rules! cos_try {
    ($expr:expr) => {
        match $expr {
            Ok(v)  => v,
            Err(e) => {
                let mut buf = $crate::fmt::FmtBuf::<128>::new();
                buf.push_str("error: ");
                buf.push_i64(e as i64);
                panic!("{}", buf.as_str())
            }
        }
    };
}
