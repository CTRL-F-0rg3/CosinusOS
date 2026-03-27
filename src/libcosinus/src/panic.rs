// libcosinus — panic.rs


use core::panic::PanicInfo;
use crate::fmt::FmtBuf;


#[panic_handler]
fn panic_handler(info: &PanicInfo) -> ! {
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

pub fn abort(code: i32) -> ! {
    crate::exit(code)
}

pub fn quit() -> ! {
    crate::exit(0)
}

#[macro_export]
macro_rules! cos_assert {
    ($cond:expr) => {
        if !$cond { panic!(concat!("assertion failed: ", stringify!($cond))); }
    };
    ($cond:expr, $msg:literal) => {
        if !$cond { panic!($msg); }
    };
}

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
