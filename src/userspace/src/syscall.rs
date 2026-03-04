// CosinusOS Userspace — syscall.rs
// Interfejs syscalli + podstawowe I/O (print/println/exit)

#[repr(usize)]
#[derive(Copy, Clone)]
pub enum Syscall { Exit = 0, Write = 1, Read = 2 }

#[inline(always)]
pub unsafe fn syscall0(num: Syscall) -> usize {
    let ret: usize;
    core::arch::asm!("int 0x80", in("rax") num as usize, lateout("rax") ret, options(nostack));
    ret
}

#[inline(always)]
pub unsafe fn syscall1(num: Syscall, arg1: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "int 0x80",
        in("rax") num as usize, in("rdi") arg1,
        lateout("rax") ret, options(nostack)
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "int 0x80",
        in("rax") num as usize, in("rdi") arg1, in("rsi") arg2, in("rdx") arg3,
        lateout("rax") ret, options(nostack)
    );
    ret
}

pub fn print(s: &str) {
    unsafe { syscall3(Syscall::Write, 1, s.as_ptr() as usize, s.len()); }
}

pub fn println(s: &str) { print(s); print("\n"); }

pub fn exit(code: i32) -> ! {
    unsafe { syscall1(Syscall::Exit, code as usize); }
    loop { unsafe { core::arch::asm!("hlt"); } }
}

// fmt::Write wrapper — potrzebny przez print_fmt!/println_fmt!
pub struct Writer;

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { print(s); Ok(()) }
}
