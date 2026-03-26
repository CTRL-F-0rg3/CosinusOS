#![no_std]
#![no_main]

use libcosinus::{print, debug, sched_yield};

#[no_mangle]
pub extern "C" fn _start(_arg: u64) -> ! {
    print("hello from userspace\n");
    debug("debug: userspace alive\n");
    loop { sched_yield(); }
}