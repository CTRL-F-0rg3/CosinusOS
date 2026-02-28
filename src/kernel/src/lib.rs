// CosinusOS Microkernel v3.5
// Bazuje na działającym v3.4 + ELF loader z prawdziwym mapowaniem + fix terminala
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![allow(unsafe_op_in_unsafe_fn)]

use core::{
    arch::{asm, naked_asm},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    panic::PanicInfo,
};

type PhysAddr = u64;
type VirtAddr = u64;

const PAGE_SIZE:               usize = 0x1000;
const MAX_FRAMES:              usize = 0x10000;
const MAX_THREADS:             usize = 64;
const KERNEL_STACK_SIZE:       usize = 0x8000;  // 32KB
const USER_STACK_SIZE:         usize = 0x4000;  // 16KB
const DOUBLE_FAULT_STACK_SIZE: usize = 0x4000;

// ============================================================================
// PORT I/O
// ============================================================================
#[inline(always)] unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("al") val, in("dx") port, options(nostack));
}
#[inline(always)] unsafe fn inb(port: u16) -> u8 {
    let r: u8; asm!("in al, dx", out("al") r, in("dx") port, options(nostack)); r
}
fn io_wait() { unsafe { outb(0x80, 0); } }

// ============================================================================
// SPINLOCK
// ============================================================================
pub struct Spinlock { locked: AtomicBool }
impl Spinlock {
    pub const fn new() -> Self { Self { locked: AtomicBool::new(false) } }
    pub fn lock(&self) {
        while self.locked.swap(true, Ordering::Acquire) {
            while self.locked.load(Ordering::Relaxed) { core::hint::spin_loop(); }
        }
    }
    pub fn unlock(&self) { self.locked.store(false, Ordering::Release); }
}

// ============================================================================
// KOLORY VGA
// ============================================================================
pub mod col {
    pub const BLACK:  u8 = 0x00; pub const BLUE:   u8 = 0x01;
    pub const GREEN:  u8 = 0x02; pub const CYAN:   u8 = 0x03;
    pub const RED:    u8 = 0x04; pub const MAGENTA:u8 = 0x05;
    pub const BROWN:  u8 = 0x06; pub const LGREY:  u8 = 0x07;
    pub const DGREY:  u8 = 0x08; pub const LBLUE:  u8 = 0x09;
    pub const LGREEN: u8 = 0x0A; pub const LCYAN:  u8 = 0x0B;
    pub const LRED:   u8 = 0x0C; pub const LMAG:   u8 = 0x0D;
    pub const YELLOW: u8 = 0x0E; pub const WHITE:  u8 = 0x0F;
    pub const fn attr(fg: u8, bg: u8) -> u8 { (bg << 4) | (fg & 0xF) }
}

// ============================================================================
// VGA
// ============================================================================
const VGA_W: usize    = 80;
const VGA_H: usize    = 25;
const VGA:   *mut u16 = 0xB8000 as *mut u16;

static mut CUR_X:  usize = 0;
static mut CUR_Y:  usize = 0;
static mut VCOLOR: u8    = col::WHITE;
static VGA_LOCK: Spinlock = Spinlock::new();

unsafe fn cursor_hw() {
    let pos = CUR_Y * VGA_W + CUR_X;
    outb(0x3D4, 0x0F); outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E); outb(0x3D5, (pos >> 8)   as u8);
}
pub unsafe fn cls() {
    for i in 0..(VGA_W * VGA_H) { *VGA.add(i) = ((VCOLOR as u16) << 8) | 0x20; }
    CUR_X = 0; CUR_Y = 0; cursor_hw();
}
unsafe fn scroll() {
    for i in 0..(VGA_H - 1) * VGA_W { *VGA.add(i) = *VGA.add(i + VGA_W); }
    for i in 0..VGA_W { *VGA.add((VGA_H - 1) * VGA_W + i) = ((VCOLOR as u16) << 8) | 0x20; }
    CUR_Y = VGA_H - 1;
}
unsafe fn putc(c: char) {
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
pub unsafe fn print(s: &str) { VGA_LOCK.lock(); for c in s.chars() { putc(c); } VGA_LOCK.unlock(); }
pub unsafe fn print_raw(s: &str) { for c in s.chars() { putc(c); } }
pub unsafe fn printc(s: &str, c: u8) {
    VGA_LOCK.lock(); let p = VCOLOR; VCOLOR = c;
    for ch in s.chars() { putc(ch); }
    VCOLOR = p; VGA_LOCK.unlock();
}
pub unsafe fn set_col(c: u8) { VCOLOR = c; }

// ============================================================================
// FORMATOWANIE
// ============================================================================
pub fn num_str<'a>(mut v: usize, buf: &'a mut [u8; 24]) -> &'a str {
    if v == 0 { buf[23] = b'0'; return unsafe { core::str::from_utf8_unchecked(&buf[23..]) }; }
    let mut i = 23usize;
    while v > 0 { buf[i] = b'0' + (v % 10) as u8; v /= 10; if i == 0 { break; } else { i -= 1; } }
    unsafe { core::str::from_utf8_unchecked(&buf[i + 1..]) }
}
pub fn hex_str<'a>(mut v: u64, buf: &'a mut [u8; 18]) -> &'a str {
    const H: &[u8] = b"0123456789ABCDEF";
    buf[0] = b'0'; buf[1] = b'x';
    for i in (2..18).rev() { buf[i] = H[(v & 0xF) as usize]; v >>= 4; }
    unsafe { core::str::from_utf8_unchecked(buf) }
}
macro_rules! pnum { ($v:expr) => {{ let mut b = [0u8; 24]; print(num_str($v as usize, &mut b)); }}; }
macro_rules! phex { ($v:expr) => {{ let mut b = [0u8; 18]; print(hex_str($v as u64, &mut b)); }}; }

// ============================================================================
// STATUS LOG
// ============================================================================
unsafe fn log_ok(label: &str, ok: bool) {
    let prev = VCOLOR; VCOLOR = col::WHITE;
    print("  "); print(label);
    let n = label.len() + 2;
    VCOLOR = col::DGREY; for _ in n..60 { putc('.'); }
    VCOLOR = col::WHITE; putc('[');
    if ok { VCOLOR = col::LGREEN; print(" OK "); } else { VCOLOR = col::LRED; print("ERR!"); }
    VCOLOR = col::WHITE; putc(']'); putc('\n');
    VCOLOR = prev;
}

// ============================================================================
// SERIAL COM1
// ============================================================================
const COM1: u16 = 0x3F8;
unsafe fn serial_init() {
    outb(COM1+1, 0x00); outb(COM1+3, 0x80); outb(COM1+0, 0x03);
    outb(COM1+1, 0x00); outb(COM1+3, 0x03); outb(COM1+2, 0xC7); outb(COM1+4, 0x0B);
}
unsafe fn com_write(c: char) { while (inb(COM1 + 5) & 0x20) == 0 {} outb(COM1, c as u8); }
unsafe fn com_read() -> Option<char> {
    if inb(COM1 + 5) & 1 != 0 { Some(inb(COM1) as char) } else { None }
}
unsafe fn serial_print(s: &str) { for c in s.chars() { com_write(c); } }

// ============================================================================
// PMM
// ============================================================================
static MM_LOCK: Spinlock = Spinlock::new();
static mut FRAME_BM:  [u64; MAX_FRAMES / 64] = [0u64; MAX_FRAMES / 64];
static mut MEM_BASE:  PhysAddr = 0;
static mut MEM_SIZE:  usize    = 0;
static mut HINT:      usize    = 0;

unsafe fn fi(p: PhysAddr) -> usize { ((p - MEM_BASE) / PAGE_SIZE as u64) as usize }
unsafe fn fp(i: usize) -> PhysAddr { MEM_BASE + i as u64 * PAGE_SIZE as u64 }
unsafe fn is_free(i: usize) -> bool { (FRAME_BM[i / 64] & (1u64 << (i % 64))) == 0 }
unsafe fn mark_used(i: usize) { FRAME_BM[i / 64] |=  1u64 << (i % 64); }
unsafe fn mark_free(i: usize) { FRAME_BM[i / 64] &= !(1u64 << (i % 64)); if i / 64 < HINT { HINT = i / 64; } }

pub unsafe fn mm_init(base: PhysAddr, size: usize) {
    MEM_BASE = base; MEM_SIZE = size;
    core::ptr::write_bytes(&raw mut FRAME_BM as *mut u8, 0, core::mem::size_of_val(&FRAME_BM));
    mark_used(0); HINT = 0;
    vprint_c("[PMM] "); pnum_raw(size / 1024 / 1024); vprint_c(" MiB dostepne\n");
}
// Wewnętrzna alokacja bez locka (dla użytku gdy MM_LOCK już trzymany)
unsafe fn mm_alloc_nolock() -> PhysAddr {
    for pass in 0..2 {
        let (s, e) = if pass == 0 { (HINT, FRAME_BM.len()) } else { (0, HINT) };
        for w in s..e {
            if FRAME_BM[w] == !0u64 { continue; }
            for bit in 0..64 {
                let idx = w * 64 + bit;
                if idx >= MAX_FRAMES { continue; }
                if is_free(idx) { mark_used(idx); HINT = w; return fp(idx); }
            }
        }
    }
    panic_no_dyn("OOM");
}
pub unsafe fn mm_alloc() -> PhysAddr {
    MM_LOCK.lock();
    let p = mm_alloc_nolock();
    MM_LOCK.unlock();
    p
}
unsafe fn mm_free_nolock(p: PhysAddr) {
    if p < MEM_BASE { return; }
    let i = fi(p); if i >= MAX_FRAMES { return; }
    mark_free(i);
}
pub unsafe fn mm_free_phys(p: PhysAddr) {
    MM_LOCK.lock(); mm_free_nolock(p); MM_LOCK.unlock();
}
pub unsafe fn mm_free_kb()  -> usize { mm_cnt(true)  * PAGE_SIZE / 1024 }
pub unsafe fn mm_used_kb()  -> usize { mm_cnt(false) * PAGE_SIZE / 1024 }
pub unsafe fn mm_total_kb() -> usize { (MEM_SIZE / PAGE_SIZE) * PAGE_SIZE / 1024 }
unsafe fn mm_cnt(free: bool) -> usize {
    let t = MEM_SIZE / PAGE_SIZE; let mut n = 0;
    for i in 0..t { if is_free(i) == free { n += 1; } } n
}
// Pomocnicze print bez locka dla mm_init
unsafe fn vprint_c(s: &str) { for c in s.chars() { putc(c); } }
unsafe fn pnum_raw(mut v: usize) {
    if v == 0 { putc('0'); return; }
    let mut buf = [0u8; 24]; let mut i = 23usize;
    while v > 0 { buf[i] = b'0' + (v % 10) as u8; v /= 10; if i > 0 { i -= 1; } else { break; } }
    for b in &buf[i + 1..] { putc(*b as char); }
}

// ============================================================================
// VMM (4-level paging, identity map)
// ============================================================================
const PTE_P: u64 = 1 << 0;
const PTE_W: u64 = 1 << 1;
const PTE_U: u64 = 1 << 2;
const PTE_ADDR: u64 = 0x000F_FFFF_FFFF_F000;

fn pte(p: PhysAddr, f: u64) -> u64 { (p & PTE_ADDR) | f | PTE_P }
fn pe(e: u64) -> bool { e & PTE_P != 0 }
fn pu(e: u64) -> bool { e & PTE_U != 0 }
fn pa(e: u64) -> PhysAddr { e & PTE_ADDR }

#[repr(C, align(4096))] struct PT { e: [u64; 512] }
unsafe fn pt(p: PhysAddr) -> *mut PT { p as *mut PT }
// zpg bez locka - dla użytku wewnątrz vmap/vunmap (które trzymają MM_LOCK)
unsafe fn zpg() -> PhysAddr {
    let p = mm_alloc_nolock();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}
// zpg z lockiem - dla użytku na zewnątrz (new_user_p4 itp.)
unsafe fn zpg_locked() -> PhysAddr {
    let p = mm_alloc();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}
// Pobierz lub utwórz entry w tablicy stron
unsafe fn goc(tab: PhysAddr, idx: usize, flags: u64) -> PhysAddr {
    let t = &mut *pt(tab);
    if !pe(t.e[idx]) {
        // Brak wpisu - alokuj nową tablicę
        let c = zpg(); t.e[idx] = pte(c, flags);
    } else if t.e[idx] & (1 << 7) != 0 {
        // PS bit (bit 7) = huge page (2MB) - rozkładamy na 512 × 4KB
        let huge_phys = t.e[idx] & 0x000F_FFFF_FFE0_0000; // adres bazowy 2MB
        let c = zpg(); // nowa P1 tablica (już wyzerowana przez zpg)
        let p1 = &mut *pt(c);
        for j in 0..512usize {
            let phys = huge_phys + j as u64 * PAGE_SIZE as u64;
            p1.e[j] = pte(phys, PTE_W);
        }
        t.e[idx] = pte(c, flags); // podmień: huge page → P1 pointer
        // KRYTYCZNE: flush całego TLB przez reload CR3
        // (invlpg nie wystarczy bo stary huge page mógł być w TLB)
        let cr3: u64;
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    }
    pa(t.e[idx])
}
unsafe fn pt_empty(p: PhysAddr) -> bool { (*pt(p)).e.iter().all(|&e| e == 0) }

static mut K_P4:    PhysAddr = 0;
static mut US_ENTRY: VirtAddr = 0; // entry point userspace (dla komendy 'userspace')

pub unsafe fn vmm_init(boot_cr3: PhysAddr) {
    // Tworzymy nowy P4 jako kopię boot P4
    // Kopiujemy wszystkie 512 wpisów 1:1 (zachowując PS/huge bity)
    // Nowy P4 wskazuje na te same P3/P2 co boot P4
    // Dzięki temu goc() może bezpiecznie modyfikować wpisy
    // nie dotykając oryginalnych boot struktur
    let new_p4 = zpg_locked();
    let boot = &*pt(boot_cr3);
    let new  = &mut *pt(new_p4);
    for i in 0..512 { new.e[i] = boot.e[i]; }
    // Przełącz na nowy P4
    core::arch::asm!("mov cr3, {}", in(reg) new_p4, options(nostack));
    K_P4 = new_p4;
}

pub unsafe fn vmap(p4: PhysAddr, v: VirtAddr, p: PhysAddr, f: u64) -> i32 {
    if v & 0xFFF != 0 || p & 0xFFF != 0 || p4 == 0 { return -1; }
    MM_LOCK.lock();
    let p3 = goc(p4, ((v >> 39) & 0x1FF) as usize, PTE_W | PTE_U);
    let p2 = goc(p3, ((v >> 30) & 0x1FF) as usize, PTE_W | PTE_U);
    let p1 = goc(p2, ((v >> 21) & 0x1FF) as usize, PTE_W | PTE_U);
    (*pt(p1)).e[((v >> 12) & 0x1FF) as usize] = pte(p, f);
    asm!("invlpg [{}]", in(reg) v, options(nostack, preserves_flags));
    MM_LOCK.unlock(); 0
}

pub unsafe fn vunmap(p4: PhysAddr, v: VirtAddr) {
    if p4 == 0 { return; }
    MM_LOCK.lock();
    let p4i = ((v >> 39) & 0x1FF) as usize; let p3i = ((v >> 30) & 0x1FF) as usize;
    let p2i = ((v >> 21) & 0x1FF) as usize; let p1i = ((v >> 12) & 0x1FF) as usize;
    let t4 = &mut *pt(p4); if !pe(t4.e[p4i]) { MM_LOCK.unlock(); return; }
    let p3p = pa(t4.e[p4i]); let t3 = &mut *pt(p3p);
    if !pe(t3.e[p3i]) { MM_LOCK.unlock(); return; }
    let p2p = pa(t3.e[p3i]); let t2 = &mut *pt(p2p);
    if !pe(t2.e[p2i]) { MM_LOCK.unlock(); return; }
    let p1p = pa(t2.e[p2i]);
    (*pt(p1p)).e[p1i] = 0;
    asm!("invlpg [{}]", in(reg) v, options(nostack, preserves_flags));
    if pt_empty(p1p) { mm_free_nolock(p1p); t2.e[p2i] = 0;
        if pt_empty(p2p) { mm_free_nolock(p2p); t3.e[p3i] = 0;
            if pt_empty(p3p) && p4i < 256 { mm_free_nolock(p3p); t4.e[p4i] = 0; }}}
    MM_LOCK.unlock();
}

// Przetłumacz virt → phys w przestrzeni adresowej p4
pub unsafe fn virt_to_phys(p4: PhysAddr, v: VirtAddr) -> Option<PhysAddr> {
    if p4 == 0 { return None; }
    macro_rules! walk { ($tab:expr, $idx:expr) => {{
        let e = (*pt($tab)).e[$idx];
        if !pe(e) { return None; }
        pa(e)
    }};}
    let p3 = walk!(p4, ((v >> 39) & 0x1FF) as usize);
    let p2 = walk!(p3, ((v >> 30) & 0x1FF) as usize);
    let p1 = walk!(p2, ((v >> 21) & 0x1FF) as usize);
    let pte_val = (*pt(p1)).e[((v >> 12) & 0x1FF) as usize];
    if !pe(pte_val) { return None; }
    Some(pa(pte_val) | (v & 0xFFF))
}

pub unsafe fn valid_user(p4: PhysAddr, v: VirtAddr) -> bool {
    if p4 == 0 { return false; }
    macro_rules! chk { ($p:expr, $i:expr) => {{
        let e = (*pt($p)).e[$i]; if !pe(e) || !pu(e) { return false; } pa(e)
    }};}
    let p3 = chk!(p4, ((v >> 39) & 0x1FF) as usize);
    let p2 = chk!(p3, ((v >> 30) & 0x1FF) as usize);
    let p1 = chk!(p2, ((v >> 21) & 0x1FF) as usize);
    let e = (*pt(p1)).e[((v >> 12) & 0x1FF) as usize];
    pe(e) && pu(e)
}
pub unsafe fn valid_buf(p4: PhysAddr, ptr: VirtAddr, len: usize) -> bool {
    if len == 0 { return true; }
    let mut pg = ptr & !(PAGE_SIZE as u64 - 1);
    while pg < ptr + len as u64 { if !valid_user(p4, pg) { return false; } pg += PAGE_SIZE as u64; }
    true
}
pub unsafe fn new_user_p4() -> PhysAddr {
    // Kopiuj CAŁY P4 (wszystkie 512 wpisów)
    // Kernel code/stack/data jest pod 0x101000 = P4[0] więc musimy
    // mieć P4[0..255] tak samo jak K_P4
    // Izolacja user/kernel przez prawa dostępu (PTE_U), nie przez osobne P4
    let n = zpg_locked();
    let src = &*pt(K_P4); let dst = &mut *pt(n);
    for i in 0..512 { dst.e[i] = src.e[i]; }
    n
}

// ============================================================================
// TRAPFRAME
// ============================================================================
#[repr(C, align(16))]
pub struct TF {
    pub r15:u64, pub r14:u64, pub r13:u64, pub r12:u64,
    pub r11:u64, pub r10:u64, pub r9:u64,  pub r8:u64,
    pub rdi:u64, pub rsi:u64, pub rdx:u64, pub rcx:u64,
    pub rbx:u64, pub rbp:u64, pub rax:u64,
    pub rip:u64, pub cs:u64,  pub rflags:u64, pub rsp:u64, pub ss:u64,
}

// ============================================================================
// TSS
// ============================================================================
#[repr(C, packed)]
pub struct Tss {
    _r0:u32, pub rsp0:u64, pub rsp1:u64, pub rsp2:u64,
    _r1:u64, pub ist1:u64, _ist:[u64;6], _r2:u64, _r3:u16, pub iomap:u16,
}
impl Tss { pub const fn new() -> Self {
    Self{_r0:0,rsp0:0,rsp1:0,rsp2:0,_r1:0,ist1:0,_ist:[0;6],_r2:0,_r3:0,
         iomap:core::mem::size_of::<Tss>() as u16}
}}
static mut TSS:      Tss                           = Tss::new();
static mut DF_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0u8; DOUBLE_FAULT_STACK_SIZE];
pub unsafe fn tss_rsp0(v: VirtAddr) { TSS.rsp0 = v; }

// ============================================================================
// GDT
// ============================================================================
#[repr(C, packed)] #[derive(Clone, Copy)]
struct GdtE { ll:u16, lb:u16, mb:u8, acc:u8, gr:u8, hb:u8 }
impl GdtE {
    const fn null() -> Self { Self{ll:0,lb:0,mb:0,acc:0,gr:0,hb:0} }
    fn seg(base:u64, lim:u64, acc:u8, gr:u8) -> Self {
        Self{ ll:(lim&0xFFFF)as u16, lb:(base&0xFFFF)as u16,
              mb:((base>>16)&0xFF)as u8, acc,
              gr:(((lim>>16)&0xF)as u8)|(gr&0xF0), hb:((base>>24)&0xFF)as u8 }
    }
}
#[repr(C, packed)] struct GdtTable { e:[GdtE;6], tss_hi:u64 }
#[repr(C, packed)] struct GdtPtr   { lim:u16, base:u64 }
static mut GDT:     GdtTable = GdtTable{e:[GdtE::null();6], tss_hi:0};
static mut GDT_PTR: GdtPtr   = GdtPtr{lim:0, base:0};
unsafe fn init_gdt() {
    TSS.ist1 = DF_STACK.as_ptr() as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
    let tb = &raw const TSS as u64;
    let tl = (core::mem::size_of::<Tss>() - 1) as u64;
    GDT.e[0] = GdtE::null();
    GDT.e[1] = GdtE::seg(0, 0xFFFFF, 0x9A, 0x20); // 0x08 kern code
    GDT.e[2] = GdtE::seg(0, 0xFFFFF, 0x92, 0x00); // 0x10 kern data
    GDT.e[3] = GdtE::seg(0, 0xFFFFF, 0xFA, 0x20); // 0x18 user code
    GDT.e[4] = GdtE::seg(0, 0xFFFFF, 0xF2, 0x00); // 0x20 user data
    GDT.e[5] = GdtE::seg(tb, tl, 0x89, 0x00);      // 0x28 TSS
    GDT.tss_hi = tb >> 32;
    GDT_PTR.lim = (core::mem::size_of::<GdtTable>() - 1) as u16;
    GDT_PTR.base = &raw const GDT as u64;
    asm!("lgdt [{}]", in(reg) &raw const GDT_PTR, options(preserves_flags));
    asm!(
        "push 0x08", "lea rax,[rip+2f]", "push rax", "retfq", "2:",
        "mov ax,0x10","mov ds,ax","mov es,ax","mov fs,ax","mov gs,ax","mov ss,ax",
        out("rax") _, options(preserves_flags)
    );
    asm!("ltr ax", in("ax") 0x28u16, options(nostack, preserves_flags));
}

// ============================================================================
// IDT
// ============================================================================
#[repr(C, packed)] #[derive(Clone, Copy)]
struct IdtE { lo:u16, sel:u16, ist:u8, attr:u8, mi:u16, hi:u32, _z:u32 }
impl IdtE {
    const fn null() -> Self { Self{lo:0,sel:0,ist:0,attr:0,mi:0,hi:0,_z:0} }
    fn new(h:u64, sel:u16, dpl:u8, ist:u8) -> Self {
        Self{ lo:(h&0xFFFF)as u16, mi:((h>>16)&0xFFFF)as u16,
              hi:((h>>32)as u32), sel, ist, attr:0x8E|(dpl<<5), _z:0 }
    }
}
#[repr(C, packed)] struct Idtr { lim:u16, base:u64 }
const IDT_LEN: usize = 256;
static mut IDT:  [IdtE; IDT_LEN] = [IdtE::null(); IDT_LEN];
static mut IDTR: Idtr             = Idtr{lim:0, base:0};
unsafe fn init_idt() {
    IDT[0x08] = IdtE::new(isr_df  as *const () as u64, 0x08, 0, 1); // #DF IST1
    IDT[0x0E] = IdtE::new(isr_pf  as *const () as u64, 0x08, 0, 0); // #PF
    IDT[0x20] = IdtE::new(isr_tmr as *const () as u64, 0x08, 0, 0); // IRQ0 timer
    IDT[0x21] = IdtE::new(isr_kb  as *const () as u64, 0x08, 0, 0); // IRQ1 keyboard
    IDT[0x80] = IdtE::new(isr_sys as *const () as u64, 0x08, 3, 0); // syscall
    IDTR.lim = (core::mem::size_of::<[IdtE; IDT_LEN]>() - 1) as u16;
    IDTR.base = IDT.as_ptr() as u64;
    asm!("lidt [{}]", in(reg) &raw const IDTR, options(preserves_flags));
    asm!("sti", options(nomem, nostack));
}

// ============================================================================
// PIC + PIT
// ============================================================================
unsafe fn init_pic() {
    outb(0x20, 0x11); io_wait(); outb(0xA0, 0x11); io_wait();
    outb(0x21, 0x20); io_wait(); outb(0xA1, 0x28); io_wait();
    outb(0x21, 0x04); io_wait(); outb(0xA1, 0x02); io_wait();
    outb(0x21, 0x01); io_wait(); outb(0xA1, 0x01); io_wait();
    // 0xFC = 1111_1100 → IRQ0 timer + IRQ1 keyboard odmaskowane
    outb(0x21, 0xFC); outb(0xA1, 0xFF);
}
unsafe fn init_pit() {
    let d = (1193180u32 / 100) as u16;
    outb(0x43, 0x36); outb(0x40, (d & 0xFF) as u8); outb(0x40, (d >> 8) as u8);
    asm!("sti", options(nomem, nostack)); // Włącz przerwania - od teraz timer działa
}

// ============================================================================
// ISR MACROS
// ============================================================================
macro_rules! isr_no_err {
    ($n:ident, $h:expr) => {
        #[unsafe(naked)] unsafe extern "C" fn $n() {
            naked_asm!(
                "push rax","push rbp","push rbx","push rcx","push rdx",
                "push rsi","push rdi","push r8","push r9","push r10",
                "push r11","push r12","push r13","push r14","push r15",
                "mov rdi,rsp","call {f}",
                "pop r15","pop r14","pop r13","pop r12","pop r11","pop r10",
                "pop r9","pop r8","pop rdi","pop rsi","pop rdx","pop rcx",
                "pop rbx","pop rbp","pop rax","iretq",
                f = sym $h,
            );
        }
    };
}
macro_rules! isr_with_err {
    ($n:ident, $h:expr) => {
        #[unsafe(naked)] unsafe extern "C" fn $n() {
            naked_asm!(
                "xchg rax,[rsp]",
                "push rbp","push rbx","push rcx","push rdx","push rsi","push rdi",
                "push r8","push r9","push r10","push r11","push r12","push r13","push r14","push r15",
                "mov rdi,rsp","call {f}",
                "pop r15","pop r14","pop r13","pop r12","pop r11","pop r10","pop r9","pop r8",
                "pop rdi","pop rsi","pop rdx","pop rcx","pop rbx","pop rbp",
                "add rsp,8","iretq",
                f = sym $h,
            );
        }
    };
}

// Double Fault — IST1
#[unsafe(naked)]
unsafe extern "C" fn isr_df() {
    naked_asm!("cli","add rsp,8","mov rdi,rsp","call {f}","cli","hlt",f=sym handle_df);
}
#[no_mangle] unsafe extern "C" fn handle_df(f: *mut TF) {
    let rip = (*f).rip;
    print_raw("\n"); printc("[#DF] DOUBLE FAULT @ RIP=", col::LRED);
    let mut b = [0u8; 18]; print_raw(hex_str(rip, &mut b)); print_raw("\n");
    loop { asm!("hlt", options(nomem, nostack)); }
}

isr_with_err!(isr_pf, handle_pf);
#[no_mangle] unsafe extern "C" fn handle_pf(f: *mut TF) {
    let err = (*f).rax; let rip = (*f).rip;
    let addr: u64; asm!("mov {},cr2", out(reg) addr, options(nomem, nostack));
    printc("\n[#PF] PAGE FAULT\n", col::YELLOW);
    print("  addr="); phex!(addr); print("  err="); phex!(err);
    print("  rip="); phex!(rip);
    print(if err & 4 != 0 { " USR" } else { " KRN" });
    print(if err & 2 != 0 { " W\n" } else { " R\n" });
    panic_no_dyn("Unhandled page fault");
}

static mut TICK: u64 = 0;
isr_no_err!(isr_tmr, handle_timer);
#[no_mangle] unsafe extern "C" fn handle_timer(_: *mut TF) { outb(0x20, 0x20); TICK += 1; schedule(); }

// ============================================================================
// KLAWIATURA PS/2 — interrupt-driven IRQ1
// ============================================================================
const SCANMAP_NORM: [char; 59] = [
    '\0','\x1b','1','2','3','4','5','6','7','8','9','0','-','=','\x08',
    '\t','q','w','e','r','t','y','u','i','o','p','[',']','\n',
    '\0','a','s','d','f','g','h','j','k','l',';','\'','`',
    '\0','\\','z','x','c','v','b','n','m',',','.','/','\0',
    '*','\0',' ','\0',
];
const SCANMAP_SHIFT: [char; 59] = [
    '\0','\x1b','!','@','#','$','%','^','&','*','(',')','_','+','\x08',
    '\t','Q','W','E','R','T','Y','U','I','O','P','{','}','\n',
    '\0','A','S','D','F','G','H','J','K','L',':','"','~',
    '\0','|','Z','X','C','V','B','N','M','<','>','?','\0',
    '*','\0',' ','\0',
];
const KB_BUF_SIZE: usize = 64;
static mut KB_BUF:   [char; KB_BUF_SIZE] = ['\0'; KB_BUF_SIZE];
static mut KB_HEAD:  usize = 0;
static mut KB_TAIL:  usize = 0;
static mut KB_SHIFT: bool  = false;

unsafe fn kb_push(c: char) {
    let next = (KB_HEAD + 1) % KB_BUF_SIZE;
    if next != KB_TAIL { KB_BUF[KB_HEAD] = c; KB_HEAD = next; }
}
pub unsafe fn kb_pop() -> Option<char> {
    if KB_HEAD == KB_TAIL { return None; }
    let c = KB_BUF[KB_TAIL]; KB_TAIL = (KB_TAIL + 1) % KB_BUF_SIZE; Some(c)
}

isr_no_err!(isr_kb, handle_kb);
#[no_mangle] unsafe extern "C" fn handle_kb(_: *mut TF) {
    let sc = inb(0x60); // MUSI być odczytany żeby odblokować kontroler
    outb(0x20, 0x20);   // EOI dla IRQ1
    match sc {
        0x2A | 0x36 => { KB_SHIFT = true;  return; }
        0xAA | 0xB6 => { KB_SHIFT = false; return; }
        _ => {}
    }
    if sc & 0x80 != 0 { return; } // key release
    let idx = sc as usize;
    if idx < SCANMAP_NORM.len() {
        let c = if KB_SHIFT { SCANMAP_SHIFT[idx] } else { SCANMAP_NORM[idx] };
        if c != '\0' { kb_push(c); }
    }
}

// ============================================================================
// SYSCALL
// ============================================================================
isr_no_err!(isr_sys, handle_syscall);
#[no_mangle] unsafe extern "C" fn handle_syscall(f: *mut TF) {
    let tf = &mut *f;
    let num = tf.rax; let a1 = tf.rdi; let a2 = tf.rsi; let a3 = tf.rdx;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;
    tf.rax = match num {
        1 => { // write(fd, buf, len)
            if a1 == 1 || a1 == 2 {
                if !valid_buf(p4, a2, a3 as usize) { !0 } else {
                    let ptr = a2 as *const u8;
                    VGA_LOCK.lock();
                    for i in 0..a3 as usize { putc(*ptr.add(i) as char); }
                    VGA_LOCK.unlock();
                    a3
                }
            } else { 0 }
        }
        2  => 0, // read — stub
        0  => {  // exit
            let c = CUR.load(Ordering::Relaxed);
            THREADS[c].state = TS::Dead;
            NTHREADS.fetch_sub(1, Ordering::Relaxed);
            schedule(); 0
        }
        _  => !0,
    };
}

// ============================================================================
// THREADING
// ============================================================================
#[derive(Clone, Copy, PartialEq)]
pub enum TS { Run, Ready, Block, Dead }

#[derive(Copy, Clone)] #[repr(C)]
pub struct Thread {
    pub id:u32, pub state:TS, pub prio:u8,
    pub krsp:VirtAddr, pub ktop:VirtAddr, pub utop:VirtAddr,
    pub cr3:PhysAddr,  pub name:[u8;16],  pub ticks:u64,
}
impl Thread { pub const fn new() -> Self {
    Self{id:0,state:TS::Dead,prio:10,krsp:0,ktop:0,utop:0,cr3:0,name:[0;16],ticks:0}
}}

static mut THREADS:  [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS];
static CUR:          AtomicUsize            = AtomicUsize::new(0);
static NTHREADS:     AtomicUsize            = AtomicUsize::new(0);
static SCHED_LOCK:   Spinlock               = Spinlock::new();

pub unsafe fn sched_init() {
    let tid = spawn_k("idle\0", idle as *const () as u64, 0);
    if tid >= 0 { THREADS[tid as usize].state = TS::Run; CUR.store(tid as usize, Ordering::SeqCst); }
}

// Wątek kernelowy (działa z K_P4)
pub unsafe fn spawn_k(name: &str, entry: u64, arg: u64) -> i32 {
    asm!("cli", options(nomem, nostack)); // Nie pozwól timerowi przerwać tworzenia wątku
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Dead { continue; }
        let t = &mut THREADS[i];
        let ks = 0x0200_0000u64 + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64 + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            vmap(K_P4, ks + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64;
        t.id = i as u32; t.prio = 10;
        t.ktop = kt; t.utop = kt; t.cr3 = K_P4; t.ticks = 0;
        init_thread_stack(t, kt, kt, entry, arg, false);
        set_name(t, name);
        NTHREADS.fetch_add(1, Ordering::Relaxed);
        t.state = TS::Ready; // OSTATNI - bezpieczne dla schedulera
        let mut buf = [0u8; 24];
        print("  [T#"); print(num_str(i, &mut buf)); print("] "); print(name); print("\n");
        return i as i32;
    }
    -1
}

// Wątek userspace w istniejącej przestrzeni adresowej cr3
pub unsafe fn spawn_user_on_cr3(name: &str, entry: u64, arg: u64, cr3: PhysAddr) -> i32 {
    asm!("cli", options(nomem, nostack));
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Dead { continue; }
        let t = &mut THREADS[i];
        // Kernel stack (mapowany w K_P4)
        let ks = 0x0200_0000u64 + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64 + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            vmap(K_P4, ks + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64;
        // User stack (mapowany w cr3)
        let us = 0x0400_0000u64 + i as u64 * (USER_STACK_SIZE + PAGE_SIZE) as u64 + PAGE_SIZE as u64;
        for p in 0..(USER_STACK_SIZE / PAGE_SIZE) {
            vmap(cr3, us + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W | PTE_U);
        }
        let ut = us + USER_STACK_SIZE as u64;
        t.id = i as u32; t.prio = 5;
        t.ktop = kt; t.utop = ut; t.cr3 = cr3; t.ticks = 0;
        init_thread_stack(t, kt, ut, entry, arg, true);
        set_name(t, name);
        t.state = TS::Ready; // OSTATNI
        NTHREADS.fetch_add(1, Ordering::Relaxed);
        let mut buf = [0u8; 24];
        print("  [T#"); print(num_str(i, &mut buf)); print("] "); print(name); print("\n");
        return i as i32;
    }
    -1
}

// Inicjalizacja stosu wątku tak żeby pasowało do thread_switch
// thread_switch pobiera: pop r15,r14,r13,r12,rbp,rbx, ret
// Więc na stosie musi być (od najniższego adresu):
//   rbx=0, rbp=0, r12=0, r13=utop, r14=entry, r15=arg, [ret=trampoline]
unsafe fn init_thread_stack(t: &mut Thread, kt: VirtAddr, ut: VirtAddr, entry: u64, arg: u64, user: bool) {
    // thread_switch: pop r15, pop r14, pop r13, pop r12, pop rbp, pop rbx, ret
    // pop bierze od NIŻSZYCH adresów. push! zmniejsza ksp.
    // Pushujemy od NAJWYŻSZEGO do NAJNIŻSZEGO elementu stosu:
    //   trampoline (ret) ← najwyższy adres (pushowany pierwszy)
    //   arg        → r15 (pierwszy pop)
    //   entry      → r14
    //   ut         → r13
    //   0          → r12
    //   0          → rbp
    //   0          → rbx ← krsp wskazuje tutaj (najniższy adres, ostatni push)
    let mut ksp = kt;
    macro_rules! push { ($v:expr) => { ksp -= 8; *(ksp as *mut u64) = $v as u64; }; }
    push!(if user { tramp_u as *const () as u64 } else { tramp_k as *const () as u64 }); // ret
    push!(arg);   // r15 = argument (pierwszy pop)
    push!(entry); // r14 = entry point
    push!(ut);    // r13 = user stack top
    push!(0u64);  // r12
    push!(0u64);  // rbp
    push!(0u64);  // rbx  ← krsp wskazuje tutaj
    t.krsp = ksp;
}

unsafe fn set_name(t: &mut Thread, name: &str) {
    let b = name.as_bytes();
    for j in 0..core::cmp::min(15, b.len()) { t.name[j] = b[j]; }
}

// tramp_k: r15=arg, r14=entry
#[unsafe(naked)] unsafe extern "C" fn tramp_k() {
    naked_asm!("mov rdi,r15", "call r14", "cli", "hlt");
}
// tramp_u: r15=arg, r14=entry, r13=user_rsp
#[unsafe(naked)] unsafe extern "C" fn tramp_u() {
    naked_asm!(
        "push 0x20|3", "push r13", "push 0x202",
        "push 0x18|3", "push r14", "mov rdi,r15", "iretq",
    );
}

pub unsafe fn schedule() {
    if SCHED_LOCK.locked.swap(true, Ordering::Acquire) { return; }
    let cur = CUR.load(Ordering::Relaxed);
    let mut next = cur;
    for _ in 0..MAX_THREADS {
        next = (next + 1) % MAX_THREADS;
        if THREADS[next].state == TS::Ready { break; }
    }
    if next == cur && THREADS[cur].state == TS::Run {
        SCHED_LOCK.locked.store(false, Ordering::Release); return;
    }
    if THREADS[cur].state == TS::Run { THREADS[cur].state = TS::Ready; }
    THREADS[next].state = TS::Run; THREADS[next].ticks += 1;
    CUR.store(next, Ordering::SeqCst);
    tss_rsp0(THREADS[next].ktop);
    let ncr3 = THREADS[next].cr3;
    let ccr3: u64; asm!("mov {},cr3", out(reg) ccr3, options(nomem, nostack));
    if ncr3 != 0 && ncr3 != ccr3 { asm!("mov cr3,{}", in(reg) ncr3, options(nostack)); }
    SCHED_LOCK.locked.store(false, Ordering::Release);
    thread_switch(&mut THREADS[cur].krsp as *mut u64, THREADS[next].krsp);
}

#[unsafe(naked)]
unsafe extern "C" fn thread_switch(old: *mut VirtAddr, new: VirtAddr) {
    naked_asm!(
        "push rbx","push rbp","push r12","push r13","push r14","push r15",
        "mov [rdi],rsp", "mov rsp,rsi",
        "pop r15","pop r14","pop r13","pop r12","pop rbp","pop rbx",
        "ret",
    );
}

unsafe extern "C" fn idle(_: u64) -> ! {
    loop { asm!("hlt", options(nomem, nostack)); }
}

// Yield — oddaj CPU. Nie używamy int 0x20 (brak EOI).
// schedule() samo sprawdza czy jest co przełączyć.
pub unsafe fn thread_yield() {
    schedule();
}

// ============================================================================
// MULTIBOOT2
// ============================================================================
const MB2_OK: u64 = 0x36d76289;
#[repr(C, packed)] struct Mb2Hdr { total:u32, _res:u32 }
#[repr(C, packed)] struct Mb2Tag { typ:u32, sz:u32 }
#[repr(C, packed)] struct Mb2Mod { typ:u32, sz:u32, start:u32, end:u32 }
pub unsafe fn mb2_module(info: u64) -> Option<(u64, u64)> {
    if info == 0 { return None; }
    let total = (*(info as *const Mb2Hdr)).total as u64;
    let mut off = 8u64;
    while off < total {
        let tag = &*((info + off) as *const Mb2Tag);
        if tag.typ == 0 { break; }
        if tag.typ == 3 {
            let m = &*((info + off) as *const Mb2Mod);
            return Some((m.start as u64, m.end as u64));
        }
        off += (tag.sz as u64 + 7) & !7;
    }
    None
}

// ============================================================================
// USERSPACE LOADER — właściwy ELF64 loader z izolowaną przestrzenią adresową
// ============================================================================

pub unsafe fn load_userspace(mod_start: u64, mod_end: u64) -> bool {
    if mod_end <= mod_start { return false; }

    // IDENTITY MAP: boot.asm mapuje pierwsze 2GB phys==virt
    // mod_start jest < 16MB więc dostępny bezpośrednio jako pointer
    let mod_sz = (mod_end - mod_start) as usize;
    let elf    = mod_start as *const u8;

    let magic = *(elf as *const u32);

    if magic != 0x464C457F {
        // ── FLAT BINARY ─────────────────────────────────────────────────────
        printc("[US] Raw binary
", col::LCYAN);
        let cr3   = new_user_p4();
        const BIN_BASE: u64 = 0x0040_0000;
        let pages = (mod_sz + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..pages {
            let phys = mm_alloc();
            vmap(cr3, BIN_BASE + i as u64 * PAGE_SIZE as u64, phys, PTE_W | PTE_U);
            // Identity map: phys jest dostępne jako virt (phys < 256MB)
            let dst = phys as *mut u8;
            let src = mod_start as *const u8;
            let n   = core::cmp::min(PAGE_SIZE, mod_sz - i * PAGE_SIZE);
            core::ptr::copy_nonoverlapping(src.add(i * PAGE_SIZE), dst, n);
            if n < PAGE_SIZE { core::ptr::write_bytes(dst.add(n), 0, PAGE_SIZE - n); }
        }
        US_ENTRY = BIN_BASE;
        printc("[US] Flat binary @ ", col::LCYAN); phex!(BIN_BASE); print("
");
        let tid = spawn_user_on_cr3("userspace", BIN_BASE, 0, cr3);
        if tid >= 0 { printc("[US] Watek #", col::LGREEN); pnum!(tid); print(" OK
"); return true; }
        else        { printc("[US] Brak slotow!
", col::LRED); return false; }
    }

    // ── ELF64 ───────────────────────────────────────────────────────────────
    let e_type      = *(elf.add(0x10) as *const u16);
    let e_entry_raw = *(elf.add(0x18) as *const u64);
    let e_phoff     = *(elf.add(0x20) as *const u64);
    let e_phentsize = *(elf.add(0x36) as *const u16) as usize;
    // ET_DYN (PIE): vaddr w segmentach są relative do 0, dodajemy LOAD_BASE
    // ET_EXEC: vaddr są absolutne, LOAD_BASE = 0
    let load_base: u64 = if e_type == 3 { 0x0040_0000u64 } else { 0u64 }; // ET_DYN=3 → PIE
    let e_entry = load_base + e_entry_raw;
    let e_phnum     = *(elf.add(0x38) as *const u16) as usize;

    printc("[US] ELF64 ", col::LCYAN);
    if e_type == 2 { print("ET_EXEC"); } else { print("ET_DYN"); }
    print(" entry="); phex!(e_entry);
    print(" phnum="); let mut nb=[0u8;24]; print(num_str(e_phnum,&mut nb)); print("
");

    let cr3 = new_user_p4();

    for i in 0..e_phnum {
        let ph       = elf.add(e_phoff as usize + i * e_phentsize);
        let p_type   = *(ph as *const u32);
        if p_type != 1 { continue; }  // tylko PT_LOAD

        let p_flags  = *(ph.add(0x04) as *const u32);
        let p_offset = *(ph.add(0x08) as *const u64);
        let p_vaddr  = *(ph.add(0x10) as *const u64);
        let p_filesz = *(ph.add(0x20) as *const u64);
        let p_memsz  = *(ph.add(0x28) as *const u64);
        if p_memsz == 0 { continue; }
        // Ogranicz memsz do rozsądnej wartości (max 2MB per segment)
        // ET_DYN Rust binary może mieć ogromny BSS z powodu statycznego stosu
        let p_memsz = core::cmp::min(p_memsz, 2 * 1024 * 1024);

        let mut perm = PTE_U;
        if p_flags & 0x2 != 0 { perm |= PTE_W; }

        let seg_start = (load_base + p_vaddr) & !(PAGE_SIZE as u64 - 1);
        let seg_end   = (load_base + p_vaddr + p_memsz + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

        let mut vaddr = seg_start;
        while vaddr < seg_end {
            let phys = mm_alloc();
            vmap(cr3, vaddr, phys, perm);

            // Identity map: zapisuj bezpośrednio przez adres fizyczny
            let dst = phys as *mut u8;
            core::ptr::write_bytes(dst, 0, PAGE_SIZE); // wyzeruj (dla BSS)

            // Skopiuj dane z pliku ELF jeśli ta strona ma dane
            let vaddr_rel = vaddr - load_base; // vaddr w przestrzeni pliku ELF
            let page_off = if vaddr_rel >= p_vaddr { vaddr_rel - p_vaddr } else { 0 };
            if page_off < p_filesz {
                let file_off = p_offset + page_off;
                let copy_n   = core::cmp::min(
                    PAGE_SIZE as u64,
                    p_filesz - page_off
                ) as usize;
                let src_ptr  = elf.add(file_off as usize);
                let dst_off  = if vaddr < p_vaddr { (p_vaddr - vaddr) as usize } else { 0 };
                core::ptr::copy_nonoverlapping(src_ptr, dst.add(dst_off), copy_n);
            }
            vaddr += PAGE_SIZE as u64;
        }

        let mut buf = [0u8; 24];
        print("  [SEG] vaddr="); phex!(p_vaddr);
        print(" filesz="); print(num_str(p_filesz as usize, &mut buf));
        print(" memsz=");  print(num_str(p_memsz  as usize, &mut buf)); print("
");
    }

    US_ENTRY = e_entry;
    let tid = spawn_user_on_cr3("userspace", e_entry, 0, cr3);
    if tid >= 0 {
        printc("[US] Watek #", col::LGREEN); pnum!(tid); print(" OK
"); true
    } else {
        printc("[US] Brak slotow!
", col::LRED); false
    }
}

// ============================================================================
// TERMINAL KERNELA
// ============================================================================
static mut TERM_LINE: [u8; 256] = [0u8; 256];
static mut TERM_LEN:  usize     = 0;

unsafe fn term_prompt() { printc("\n#$> ", col::LGREEN); }

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
                let ne = t.name.iter().position(|&b| b == 0).unwrap_or(16);
                print(core::str::from_utf8_unchecked(&t.name[..ne]));
                printc(ss, sc);
                print(" ticks="); pnum!(t.ticks as usize); print("\n");
            }
        }
        "userspace" => {
            let mut found = false;
            for i in 0..MAX_THREADS {
                if THREADS[i].state == TS::Dead { continue; }
                let ne = THREADS[i].name.iter().position(|&b| b == 0).unwrap_or(16);
                if &THREADS[i].name[..ne] == b"userspace" {
                    printc("Userspace dziala jako watek #", col::LGREEN);
                    pnum!(i); print("\n"); found = true; break;
                }
            }
            if !found {
                if US_ENTRY != 0 {
                    printc("Uruchamiam userspace @ ", col::LCYAN); phex!(US_ENTRY); print("\n");
                    let cr3 = new_user_p4();
                    let tid = spawn_user_on_cr3("userspace\0", US_ENTRY, 0, cr3);
                    if tid >= 0 { printc("  Watek #", col::LGREEN); pnum!(tid as usize); print(" OK\n"); }
                    else { printc("  Brak slotow!\n", col::LRED); }
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
            let cr3: u64; asm!("mov {},cr3", out(reg) cr3, options(nomem, nostack));
            print("CR3="); phex!(cr3); print("\n");
        }
        "regs" => {
            let mut rsp:u64; let mut rbp:u64; let mut cr3:u64; let mut cr2:u64; let mut rfl:u64;
            asm!("mov {},rsp", out(reg) rsp, options(nomem, nostack));
            asm!("mov {},rbp", out(reg) rbp, options(nomem, nostack));
            asm!("mov {},cr3", out(reg) cr3, options(nomem, nostack));
            asm!("mov {},cr2", out(reg) cr2, options(nomem, nostack));
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
    match c {
        '\n' | '\r' => { term_process_cmd(); term_prompt(); }
        '\x08' => {
            if TERM_LEN > 0 {
                TERM_LEN -= 1;
                VGA_LOCK.lock();
                if CUR_X > 0 { CUR_X -= 1; }
                putc(' ');
                if CUR_X > 0 { CUR_X -= 1; }
                cursor_hw();
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

// Wątek terminala — FIX: używa thread_yield() zamiast spin_loop()
// żeby scheduler mógł normalnie działać między tickami
unsafe extern "C" fn kernel_terminal(_: u64) -> ! {
    printc("\n=== CosinusOS Kernel Terminal ===\n", col::YELLOW);
    print("  Klawiatura PS/2 + COM1 (115200). Wpisz 'help'.\n");
    term_prompt();
    loop {
        let mut got = false;
        while let Some(c) = kb_pop()  { term_handle_char(c); got = true; }
        while let Some(c) = com_read() { com_write(c); term_handle_char(c); got = true; }
        if !got {
            // Brak inputu — oddaj CPU przez schedule()
            // Nie spin_loop (zjada 100% CPU i blokuje inne wątki)
            thread_yield();
        }
    }
}

// ============================================================================
// PANIC
// ============================================================================
fn panic_no_dyn(msg: &str) -> ! {
    unsafe {
        asm!("cli", options(nomem, nostack));
        VCOLOR = col::attr(col::WHITE, col::RED);
        print_raw("\n  *** KERNEL PANIC ***  \n  ");
        print_raw(msg);
        print_raw("  \n");
        VCOLOR = col::WHITE;
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        asm!("cli", options(nomem, nostack));
        VCOLOR = col::attr(col::WHITE, col::RED);
        print_raw("\n  *** KERNEL PANIC ***  \n  ");
        if let Some(s) = info.message().as_str() { print_raw(s); }
        else { print_raw("(no message)"); }
        if let Some(l) = info.location() { print_raw(" @ "); print_raw(l.file()); }
        print_raw("  \n");
        VCOLOR = col::WHITE;
    }
    loop { unsafe { asm!("hlt", options(nomem, nostack)); } }
}

// ============================================================================
// KERNEL MAIN
// ============================================================================
#[no_mangle]
pub extern "C" fn kernel_main(mb_magic: u64, mb_info: u64) -> ! {
    unsafe {
        cls(); serial_init();
        set_col(col::attr(col::LCYAN, col::BLACK));
        print(" ===========================\n");
        print("  CosinusOS Microkernel v3.5\n");
        print(" ===========================\n\n");
        set_col(col::WHITE);
        serial_print("=== CosinusOS v3.5 boot ===\n");

        mm_init(0x0100_0000, 0x0F00_0000); // 16MB–256MB = ~240MB
        vmm_init(0x1000);
        log_ok("PMM + VMM", true);

        init_gdt(); log_ok("GDT", true);
        init_pic(); log_ok("PIC", true);
        init_idt(); log_ok("IDT + IRQ1 keyboard", true);

        // Scheduler PRZED PIT — wątki muszą istnieć zanim przyjdzie timer
        sched_init(); log_ok("Scheduler (idle thread)", true);

        // PIT ostatni — od teraz przychodzą przerwania timera
        init_pit(); log_ok("PIT 100Hz", true);

        print("\n");
        printc("=== Userspace ===\n", col::YELLOW);
        if mb_magic == MB2_OK {
            log_ok("MB2 magic", true);
            match mb2_module(mb_info) {
                Some((s, e)) => {
                    log_ok("Modul userspace", true);
                    print("  Adres: "); phex!(s); print(" - "); phex!(e); print("\n");
                    let ok = load_userspace(s, e);
                    log_ok("Uruchomienie userspace", ok);
                }
                None => {
                    log_ok("Modul userspace", false);
                    printc("  Dodaj do grub.cfg: module2 /boot/userspace.bin\n", col::YELLOW);
                }
            }
        } else {
            log_ok("MB2 magic", false);
            print("  Otrzymano: "); phex!(mb_magic); print("\n");
        }

        print("\n");
        printc("=== Kernel Terminal ===\n", col::YELLOW);
        let t = spawn_k("kterminal\0", kernel_terminal as *const () as u64, 0);
        log_ok("Kernel debug terminal (PS/2 + COM1)", t >= 0);

        print("\n");
        printc("=== Stan systemu ===\n", col::YELLOW);
        print("  Pamiec wolna: "); pnum!(mm_free_kb()); print(" KB\n");
        print("  Watki: "); pnum!(NTHREADS.load(Ordering::Relaxed)); print("\n");

        print("\n");
        set_col(col::attr(col::BLACK, col::LGREEN));
        print(" [ SYSTEM GOTOWY ] ");
        set_col(col::WHITE); print("\n\n");
        serial_print("[OK] boot complete\n");

        schedule();
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}