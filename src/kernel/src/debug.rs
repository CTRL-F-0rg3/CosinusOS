// CosinusOS — debug.rs


use core::arch::asm;
use crate::sync::Spinlock;

// ── Port I/O ─────────────────────────────────────────────────────────────────
#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("al") val, in("dx") port, options(nostack));
}
#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let r: u8;
    asm!("in al, dx", out("al") r, in("dx") port, options(nostack));
    r
}
pub fn io_wait() { unsafe { outb(0x80, 0); } }

pub mod col {
    pub const BLACK:   u8 = 0x00; pub const BLUE:    u8 = 0x01;
    pub const GREEN:   u8 = 0x02; pub const CYAN:    u8 = 0x03;
    pub const RED:     u8 = 0x04; pub const MAGENTA: u8 = 0x05;
    pub const BROWN:   u8 = 0x06; pub const LGREY:   u8 = 0x07;
    pub const DGREY:   u8 = 0x08; pub const LBLUE:   u8 = 0x09;
    pub const LGREEN:  u8 = 0x0A; pub const LCYAN:   u8 = 0x0B;
    pub const LRED:    u8 = 0x0C; pub const LMAG:    u8 = 0x0D;
    pub const YELLOW:  u8 = 0x0E; pub const WHITE:   u8 = 0x0F;
    pub const fn attr(fg: u8, bg: u8) -> u8 { (bg << 4) | (fg & 0xF) }
}

const VGA_W: usize    = 80;
const VGA_H: usize    = 25;
const VGA:   *mut u16 = 0xB8000 as *mut u16;

pub static mut CUR_X:  usize = 0;
pub static mut CUR_Y:  usize = 0;
pub static mut VCOLOR: u8    = col::WHITE;
pub static VGA_LOCK: Spinlock = Spinlock::new();

unsafe fn cursor_hw() {
    let pos = CUR_Y * VGA_W + CUR_X;
    outb(0x3D4, 0x0F); outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E); outb(0x3D5, (pos >> 8)   as u8);
}

pub unsafe fn cls() {
    for i in 0..(VGA_W * VGA_H) {
        *VGA.add(i) = ((VCOLOR as u16) << 8) | 0x20;
    }
    CUR_X = 0; CUR_Y = 0; cursor_hw();
}

unsafe fn scroll() {
    for i in 0..(VGA_H - 1) * VGA_W {
        *VGA.add(i) = *VGA.add(i + VGA_W);
    }
    for i in 0..VGA_W {
        *VGA.add((VGA_H - 1) * VGA_W + i) = ((VCOLOR as u16) << 8) | 0x20;
    }
    CUR_Y = VGA_H - 1;
}

pub unsafe fn putc(c: char) {
    match c {
        '\n' => { CUR_X = 0; CUR_Y += 1; }
        '\r' => { CUR_X = 0; }
        '\t' => { CUR_X = (CUR_X + 8) & !7; }
        '\x08' => {
            if CUR_X > 0 {
                CUR_X -= 1;
                *VGA.add(CUR_Y * VGA_W + CUR_X) = ((VCOLOR as u16) << 8) | 0x20;
            }
        }
        _ => {
            *VGA.add(CUR_Y * VGA_W + CUR_X) = ((VCOLOR as u16) << 8) | (c as u16 & 0xFF);
            CUR_X += 1;
        }
    }
    if CUR_X >= VGA_W { CUR_X = 0; CUR_Y += 1; }
    if CUR_Y >= VGA_H { scroll(); }
    cursor_hw();
}

pub unsafe fn putc_raw(c: char) { putc(c); }

pub unsafe fn print(s: &str) {
    VGA_LOCK.lock();
    for c in s.chars() { putc(c); }
    VGA_LOCK.unlock();
}

pub unsafe fn print_raw(s: &str) {
    for c in s.chars() { putc(c); }
}

pub unsafe fn printc(s: &str, color: u8) {
    VGA_LOCK.lock();
    let prev = VCOLOR; VCOLOR = color;
    for c in s.chars() { putc(c); }
    VCOLOR = prev;
    VGA_LOCK.unlock();
}

pub unsafe fn set_col(c: u8) { VCOLOR = c; }

pub unsafe fn log_ok(label: &str, ok: bool) {
    let prev = VCOLOR; VCOLOR = col::WHITE;
    print("  "); print(label);
    let n = label.len() + 2;
    VCOLOR = col::DGREY; for _ in n..60 { putc('.'); }
    VCOLOR = col::WHITE; putc('[');
    if ok { VCOLOR = col::LGREEN; print(" OK "); }
    else  { VCOLOR = col::LRED;   print("ERR!"); }
    VCOLOR = col::WHITE; putc(']'); putc('\n');
    VCOLOR = prev;
}

pub fn num_str<'a>(mut v: usize, buf: &'a mut [u8; 24]) -> &'a str {
    if v == 0 {
        buf[23] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[23..]) };
    }
    let mut i = 23usize;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if i == 0 { break; } else { i -= 1; }
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i + 1..]) }
}

pub fn hex_str<'a>(mut v: u64, buf: &'a mut [u8; 18]) -> &'a str {
    const H: &[u8] = b"0123456789ABCDEF";
    buf[0] = b'0'; buf[1] = b'x';
    for i in (2..18).rev() { buf[i] = H[(v & 0xF) as usize]; v >>= 4; }
    unsafe { core::str::from_utf8_unchecked(buf) }
}

#[macro_export]
macro_rules! pnum {
    ($v:expr) => {{ let mut b = [0u8; 24]; crate::debug::print(crate::debug::num_str($v as usize, &mut b)); }};
}
#[macro_export]
macro_rules! phex {
    ($v:expr) => {{ let mut b = [0u8; 18]; crate::debug::print(crate::debug::hex_str($v as u64, &mut b)); }};
}

const COM1: u16 = 0x3F8;

pub unsafe fn serial_init() {
    outb(COM1+1, 0x00); outb(COM1+3, 0x80); outb(COM1+0, 0x03);
    outb(COM1+1, 0x00); outb(COM1+3, 0x03); outb(COM1+2, 0xC7); outb(COM1+4, 0x0B);
}

pub unsafe fn com_write(c: char) {
    while (inb(COM1 + 5) & 0x20) == 0 {}
    outb(COM1, c as u8);
}

pub unsafe fn com_read() -> Option<char> {
    if inb(COM1 + 5) & 1 != 0 { Some(inb(COM1) as char) } else { None }
}

pub unsafe fn serial_print(s: &str) {
    for c in s.chars() { com_write(c); }
}

pub unsafe fn serial_hex(v: u64) {
    let mut b = [0u8; 18];
    serial_print(hex_str(v, &mut b));
}


pub unsafe fn cursor_hw_pub() { cursor_hw(); }