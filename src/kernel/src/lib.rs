// src/kernel/src/lib.rs — CosinusOS Microkernel v3.3

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
const KERNEL_STACK_SIZE:       usize = 0x8000;
const USER_STACK_SIZE:         usize = 0x4000;
const DOUBLE_FAULT_STACK_SIZE: usize = 0x4000;
const PHYS_OFFSET:             VirtAddr = 0;

#[inline(always)] pub const fn phys_to_virt(p: PhysAddr) -> VirtAddr { p + PHYS_OFFSET }
#[inline(always)] pub const fn virt_to_phys(v: VirtAddr) -> PhysAddr { v - PHYS_OFFSET }

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
// VGA
// ============================================================================
const VGA_W:   usize    = 80;
const VGA_H:   usize    = 25;
const VGA_MEM: *mut u16 = 0xB8000 as *mut u16;

static mut VGA_BUF: *mut u16 = VGA_MEM;
static mut CUR_X:   usize    = 0;
static mut CUR_Y:   usize    = 0;
static mut VGA_COL: u8       = 0x0F;
static     VGA_LOCK: Spinlock = Spinlock::new();

unsafe fn vga_cursor() {
    let pos = CUR_Y * VGA_W + CUR_X;
    outb(0x3D4, 0x0F); outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E); outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
}

pub unsafe fn clear_screen() {
    for i in 0..(VGA_W * VGA_H) { *VGA_BUF.add(i) = ((VGA_COL as u16) << 8) | b' ' as u16; }
    CUR_X = 0; CUR_Y = 0; vga_cursor();
}

unsafe fn putchar_raw(c: char) {
    match c {
        '\n' => { CUR_X = 0; CUR_Y += 1; }
        '\r' => { CUR_X = 0; }
        '\t' => { CUR_X = (CUR_X + 4) & !3; }
        '\x08' => { if CUR_X > 0 { CUR_X -= 1; } }
        _ => {
            *VGA_BUF.add(CUR_Y * VGA_W + CUR_X) = ((VGA_COL as u16) << 8) | c as u16;
            CUR_X += 1;
        }
    }
    if CUR_X >= VGA_W { CUR_X = 0; CUR_Y += 1; }
    if CUR_Y >= VGA_H {
        for i in 0..(VGA_H-1)*VGA_W { *VGA_BUF.add(i) = *VGA_BUF.add(i + VGA_W); }
        for i in 0..VGA_W { *VGA_BUF.add((VGA_H-1)*VGA_W+i) = ((VGA_COL as u16)<<8)|b' ' as u16; }
        CUR_Y = VGA_H - 1;
    }
    vga_cursor();
}

pub unsafe fn print(s: &str) {
    VGA_LOCK.lock();
    for c in s.chars() { putchar_raw(c); }
    VGA_LOCK.unlock();
}
pub unsafe fn print_raw(s: &str) { for c in s.chars() { putchar_raw(c); } }

// ============================================================================
// KOLORY VGA
// ============================================================================
// Atrybuty: bits 7-4 = tło, bits 3-0 = tekst
pub mod color {
    pub const BLACK:         u8 = 0x00;
    pub const BLUE:          u8 = 0x01;
    pub const GREEN:         u8 = 0x02;
    pub const CYAN:          u8 = 0x03;
    pub const RED:           u8 = 0x04;
    pub const MAGENTA:       u8 = 0x05;
    pub const BROWN:         u8 = 0x06;
    pub const LIGHT_GREY:    u8 = 0x07;
    pub const DARK_GREY:     u8 = 0x08;
    pub const LIGHT_BLUE:    u8 = 0x09;
    pub const LIGHT_GREEN:   u8 = 0x0A;
    pub const LIGHT_CYAN:    u8 = 0x0B;
    pub const LIGHT_RED:     u8 = 0x0C;
    pub const LIGHT_MAGENTA: u8 = 0x0D;
    pub const YELLOW:        u8 = 0x0E;
    pub const WHITE:         u8 = 0x0F;
    pub fn attr(fg: u8, bg: u8) -> u8 { (bg << 4) | (fg & 0x0F) }
}

pub unsafe fn set_color(col: u8) { VGA_COL = col; }

pub unsafe fn print_col(s: &str, col: u8) {
    VGA_LOCK.lock();
    let prev = VGA_COL;
    VGA_COL = col;
    for c in s.chars() { putchar_raw(c); }
    VGA_COL = prev;
    VGA_LOCK.unlock();
}

// ============================================================================
// STATUS LOG  — [ OK ] zielony / [ERR] czerwony / [INF] cyjan / [WRN] żółty
// ============================================================================

/// Drukuje wiersz statusu:  "  label ............. [ OK ]\n"
/// ok=true → zielony [ OK ], ok=false → czerwony [ERR]
pub unsafe fn log_ok(label: &str, ok: bool) {
    VGA_LOCK.lock();
    let prev = VGA_COL;

    // Drukuj etykietę białym
    VGA_COL = color::WHITE;
    for c in label.chars() { putchar_raw(c); }

    // Wypełnij kropkami do kolumny 60
    let label_len = label.len();
    let dots_end  = 60usize;
    VGA_COL = color::DARK_GREY;
    for _ in label_len..dots_end { putchar_raw('.'); }

    // Nawias otwierający
    VGA_COL = color::WHITE;
    putchar_raw(' ');
    putchar_raw('[');

    if ok {
        VGA_COL = color::LIGHT_GREEN;
        putchar_raw(' '); putchar_raw('O'); putchar_raw('K'); putchar_raw(' ');
    } else {
        VGA_COL = color::LIGHT_RED;
        putchar_raw('E'); putchar_raw('R'); putchar_raw('R'); putchar_raw('!');
    }

    VGA_COL = color::WHITE;
    putchar_raw(']');
    putchar_raw('\n');

    VGA_COL = prev;
    VGA_LOCK.unlock();
}

/// Drukuje "[INF] wiadomość\n" w kolorze cyjan
pub unsafe fn log_info(msg: &str) {
    print_col("[INF] ", color::LIGHT_CYAN);
    print(msg);
    print("\n");
}

/// Drukuje "[WRN] wiadomość\n" w kolorze żółtym
pub unsafe fn log_warn(msg: &str) {
    print_col("[WRN] ", color::YELLOW);
    print(msg);
    print("\n");
}

/// Drukuje "[ERR] wiadomość\n" w kolorze czerwonym
pub unsafe fn log_err(msg: &str) {
    print_col("[ERR] ", color::LIGHT_RED);
    print(msg);
    print("\n");
}

// ============================================================================
// FUNKCJE POMOCNICZE KERNELA
// ============================================================================

/// Liczba wolnych ramek fizycznych
pub unsafe fn mm_free_frames() -> usize {
    let total = MEM_SIZE / PAGE_SIZE;
    let mut free = 0usize;
    for i in 0..total { if is_free(i) { free += 1; } }
    free
}

/// Liczba użytych ramek fizycznych
pub unsafe fn mm_used_frames() -> usize {
    let total = MEM_SIZE / PAGE_SIZE;
    let mut used = 0usize;
    for i in 0..total { if !is_free(i) { used += 1; } }
    used
}

/// Wypisuje stan pamięci
pub unsafe fn mm_dump_stats() {
    let free  = mm_free_frames();
    let used  = mm_used_frames();
    let total = free + used;
    let mut b = [0u8; 20];
    print_col("  Pamiec: ", color::LIGHT_CYAN);
    print(usize_str(used * PAGE_SIZE / 1024, &mut b));   print(" KB uzyte / ");
    print(usize_str(free * PAGE_SIZE / 1024, &mut b));   print(" KB wolne / ");
    print(usize_str(total * PAGE_SIZE / 1024, &mut b));  print(" KB razem\n");
}

/// Wypisuje listę aktywnych wątków
pub unsafe fn thread_dump() {
    let mut b = [0u8; 20];
    print_col("  Watki:\n", color::LIGHT_CYAN);
    for i in 0..MAX_THREADS {
        if THREADS[i].state == TS::Terminated { continue; }
        let t = &THREADS[i];
        print("    ["); print(usize_str(i, &mut b)); print("] ");
        // nazwa wątku
        let mut name_end = 0;
        while name_end < 16 && t.name[name_end] != 0 { name_end += 1; }
        let name_str = core::str::from_utf8_unchecked(&t.name[..name_end]);
        print(name_str);
        print(" — ");
        let state_str = match t.state {
            TS::Running    => "RUNNING",
            TS::Ready      => "READY",
            TS::Blocked    => "BLOCKED",
            TS::Terminated => "DEAD",
        };
        let col = match t.state {
            TS::Running => color::LIGHT_GREEN,
            TS::Ready   => color::LIGHT_CYAN,
            TS::Blocked => color::YELLOW,
            _           => color::DARK_GREY,
        };
        print_col(state_str, col);
        print("\n");
    }
}

/// Zwraca aktualny numer wątku
pub fn current_thread() -> usize { CURRENT.load(Ordering::Relaxed) }

/// Zwraca liczbę aktywnych wątków
pub fn thread_count() -> usize { THREAD_COUNT.load(Ordering::Relaxed) }

// ============================================================================
// SERIAL
// ============================================================================
const COM1: u16 = 0x3F8;
unsafe fn serial_init() {
    outb(COM1+1,0x00); outb(COM1+3,0x80); outb(COM1+0,0x03);
    outb(COM1+1,0x00); outb(COM1+3,0x03); outb(COM1+2,0xC7); outb(COM1+4,0x0B);
}
unsafe fn serial_write(c: char) { while (inb(COM1+5)&0x20)==0 {} outb(COM1, c as u8); }
unsafe fn serial_print(s: &str) { for c in s.chars() { serial_write(c); } }

// ============================================================================
// PMM
// ============================================================================
static MM_LOCK: Spinlock = Spinlock::new();
static mut FRAME_BM:   [u64; MAX_FRAMES/64] = [0u64; MAX_FRAMES/64];
static mut MEM_BASE:   PhysAddr = 0;
static mut MEM_SIZE:   usize    = 0;
static mut ALLOC_HINT: usize    = 0;

unsafe fn fi(p: PhysAddr) -> usize { ((p - MEM_BASE) / PAGE_SIZE as u64) as usize }
unsafe fn fp(i: usize)    -> PhysAddr { MEM_BASE + i as u64 * PAGE_SIZE as u64 }
unsafe fn is_free(i: usize) -> bool  { (FRAME_BM[i/64] & (1u64<<(i%64))) == 0 }
unsafe fn set_used(i: usize) { FRAME_BM[i/64] |=  1u64 << (i%64); }
unsafe fn set_free(i: usize) {
    FRAME_BM[i/64] &= !(1u64 << (i%64));
    if i/64 < ALLOC_HINT { ALLOC_HINT = i/64; }
}

pub unsafe fn mm_init(base: PhysAddr, size: usize) {
    MEM_BASE = base; MEM_SIZE = size;
    let frames = size / PAGE_SIZE;
    core::ptr::write_bytes(FRAME_BM.as_mut_ptr() as *mut u8, 0, core::mem::size_of_val(&FRAME_BM));
    for i in 0..core::cmp::min(256, frames) { set_used(i); }
    ALLOC_HINT = 4;
    let mut b=[0u8;20];
    print("[MM] "); print(usize_str(frames * PAGE_SIZE / 1024 / 1024, &mut b)); print(" MiB\n");
}

pub unsafe fn mm_alloc_frame() -> PhysAddr {
    MM_LOCK.lock();
    for pass in 0..2 {
        let (s,e) = if pass==0 { (ALLOC_HINT, FRAME_BM.len()) } else { (0, ALLOC_HINT) };
        for w in s..e {
            if FRAME_BM[w] == !0u64 { continue; }
            for bit in 0..64 {
                let idx = w*64+bit;
                if idx >= MAX_FRAMES { continue; }
                if is_free(idx) {
                    set_used(idx); ALLOC_HINT = w; MM_LOCK.unlock();
                    return fp(idx);
                }
            }
        }
    }
    MM_LOCK.unlock();
    panic_no_dyn("OOM");
}

pub unsafe fn mm_free_frame(p: PhysAddr) {
    if p < MEM_BASE { return; }
    let i = fi(p);
    if i >= MAX_FRAMES { return; }
    MM_LOCK.lock(); set_free(i); MM_LOCK.unlock();
}

// ============================================================================
// VMM
// ============================================================================
const PTE_P:  u64 = 1<<0;
const PTE_W:  u64 = 1<<1;
const PTE_U:  u64 = 1<<2;
const PTE_M:  u64 = 0x000F_FFFF_FFFF_F000;

fn pte_new(p: PhysAddr, f: u64) -> u64 { (p & PTE_M)|f|PTE_P }
fn pte_p(e: u64)   -> bool { e & PTE_P != 0 }
fn pte_u(e: u64)   -> bool { e & PTE_U != 0 }
fn pte_a(e: u64)   -> PhysAddr { e & PTE_M }

#[repr(C, align(4096))]
struct PT { e: [u64; 512] }

unsafe fn pt(p: PhysAddr)  -> *mut PT { p as *mut PT }

unsafe fn zpage() -> PhysAddr {
    let p = mm_alloc_frame();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

unsafe fn goc(t: PhysAddr, i: usize, f: u64) -> PhysAddr {
    let tab = &mut *pt(t);
    if !pte_p(tab.e[i]) { let c = zpage(); tab.e[i] = pte_new(c,f); }
    pte_a(tab.e[i])
}

unsafe fn pt_empty(p: PhysAddr) -> bool { (*pt(p)).e.iter().all(|&e| e==0) }

static mut KERNEL_P4: PhysAddr = 0;

pub unsafe fn mm_init_paging(cr3: PhysAddr) {
    KERNEL_P4 = cr3;
    print("[MMU] P4=0x"); print(u64_hex(cr3, &mut [0u8;18])); print("\n");
}

pub unsafe fn mm_map(p4: PhysAddr, v: VirtAddr, p: PhysAddr, f: u64) -> i32 {
    if v&0xFFF!=0 || p&0xFFF!=0 { return -1; }
    if p4==0 { return -1; }
    MM_LOCK.lock();
    let p3 = goc(p4, ((v>>39)&0x1FF) as usize, PTE_W|PTE_U);
    let p2 = goc(p3, ((v>>30)&0x1FF) as usize, PTE_W|PTE_U);
    let p1 = goc(p2, ((v>>21)&0x1FF) as usize, PTE_W|PTE_U);
    (*pt(p1)).e[((v>>12)&0x1FF) as usize] = pte_new(p, f);
    asm!("invlpg [{}]", in(reg) v, options(nostack, preserves_flags));
    MM_LOCK.unlock();
    0
}

pub unsafe fn mm_unmap(p4: PhysAddr, v: VirtAddr) {
    if p4==0 { return; }
    MM_LOCK.lock();
    let p4i=((v>>39)&0x1FF) as usize; let p3i=((v>>30)&0x1FF) as usize;
    let p2i=((v>>21)&0x1FF) as usize; let p1i=((v>>12)&0x1FF) as usize;
    let t4=&mut *pt(p4); if !pte_p(t4.e[p4i]) { MM_LOCK.unlock(); return; }
    let p3p=pte_a(t4.e[p4i]); let t3=&mut *pt(p3p);
    if !pte_p(t3.e[p3i]) { MM_LOCK.unlock(); return; }
    let p2p=pte_a(t3.e[p3i]); let t2=&mut *pt(p2p);
    if !pte_p(t2.e[p2i]) { MM_LOCK.unlock(); return; }
    let p1p=pte_a(t2.e[p2i]);
    (*pt(p1p)).e[p1i] = 0;
    asm!("invlpg [{}]", in(reg) v, options(nostack, preserves_flags));
    if pt_empty(p1p) { mm_free_frame(p1p); t2.e[p2i]=0;
        if pt_empty(p2p) { mm_free_frame(p2p); t3.e[p3i]=0;
            if pt_empty(p3p) && p4i<256 { mm_free_frame(p3p); t4.e[p4i]=0; }}}
    MM_LOCK.unlock();
}

pub unsafe fn valid_user(p4: PhysAddr, v: VirtAddr) -> bool {
    if p4==0 { return false; }
    macro_rules! chk { ($p:expr, $i:expr) => {{
        let e = (*pt($p)).e[$i];
        if !pte_p(e)||!pte_u(e) { return false; } pte_a(e)
    }}; }
    let p3=chk!(p4, ((v>>39)&0x1FF) as usize);
    let p2=chk!(p3, ((v>>30)&0x1FF) as usize);
    let p1=chk!(p2, ((v>>21)&0x1FF) as usize);
    let e=(*pt(p1)).e[((v>>12)&0x1FF) as usize];
    pte_p(e) && pte_u(e)
}

pub unsafe fn valid_buf(p4: PhysAddr, ptr: VirtAddr, len: usize) -> bool {
    if len==0 { return true; }
    let mut pg = ptr & !(PAGE_SIZE as u64-1);
    while pg < ptr+len as u64 { if !valid_user(p4,pg) { return false; } pg+=PAGE_SIZE as u64; }
    true
}

pub unsafe fn create_user_p4() -> PhysAddr {
    let n = zpage();
    let src=&*pt(KERNEL_P4); let dst=&mut *pt(n);
    for i in 256..512 { dst.e[i] = src.e[i]; }
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
    pub rip:u64, pub cs:u64, pub rflags:u64, pub rsp:u64, pub ss:u64,
}

// ============================================================================
// TSS
// ============================================================================
#[repr(C, packed)]
pub struct Tss {
    _r0:u32, pub rsp0:u64, pub rsp1:u64, pub rsp2:u64,
    _r1:u64, pub ist1:u64, _ist:[u64;6], _r2:u64, _r3:u16, pub iomap:u16,
}
impl Tss {
    pub const fn new() -> Self {
        Self { _r0:0,rsp0:0,rsp1:0,rsp2:0,_r1:0,ist1:0,
               _ist:[0;6],_r2:0,_r3:0, iomap:core::mem::size_of::<Tss>() as u16 }
    }
}
static mut TSS:      Tss                         = Tss::new();
static mut DF_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0u8; DOUBLE_FAULT_STACK_SIZE];
pub unsafe fn tss_rsp0(v: VirtAddr) { TSS.rsp0 = v; }

// ============================================================================
// GDT
// ============================================================================
#[repr(C,packed)] #[derive(Clone,Copy)]
struct GdtE { ll:u16,lb:u16,mb:u8,acc:u8,gr:u8,hb:u8 }
impl GdtE {
    const fn null() -> Self { Self{ll:0,lb:0,mb:0,acc:0,gr:0,hb:0} }
    fn new(base:u64, lim:u64, acc:u8, gr:u8) -> Self {
        Self { ll:(lim&0xFFFF)as u16, lb:(base&0xFFFF)as u16,
               mb:((base>>16)&0xFF)as u8, acc,
               gr:(((lim>>16)&0xF)as u8)|(gr&0xF0), hb:((base>>24)&0xFF)as u8 }
    }
}
#[repr(C,packed)] struct GdtTable { e:[GdtE;6], tss_hi:u64 }
#[repr(C,packed)] struct GdtPtr   { lim:u16, base:u64 }

static mut GDT:     GdtTable = GdtTable { e:[GdtE::null();6], tss_hi:0 };
static mut GDT_PTR: GdtPtr   = GdtPtr { lim:0, base:0 };

unsafe fn init_gdt() {
    TSS.ist1 = DF_STACK.as_ptr() as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
    let tb = &raw const TSS as u64;
    let tl = (core::mem::size_of::<Tss>()-1) as u64;
    GDT.e[0]=GdtE::null();
    GDT.e[1]=GdtE::new(0,0xFFFFF,0x9A,0x20); // kernel code 0x08
    GDT.e[2]=GdtE::new(0,0xFFFFF,0x92,0x00); // kernel data 0x10
    GDT.e[3]=GdtE::new(0,0xFFFFF,0xFA,0x20); // user code   0x18
    GDT.e[4]=GdtE::new(0,0xFFFFF,0xF2,0x00); // user data   0x20
    GDT.e[5]=GdtE::new(tb,tl,0x89,0x00);      // TSS         0x28
    GDT.tss_hi = tb >> 32;
    GDT_PTR.lim  = (core::mem::size_of::<GdtTable>()-1) as u16;
    GDT_PTR.base = &raw const GDT as u64;
    asm!("lgdt [{}]", in(reg) &raw const GDT_PTR, options(preserves_flags));
    asm!(
        "push 0x08", "lea rax, [rip + 2f]", "push rax", "retfq", "2:",
        "mov ax, 0x10",
        "mov ds, ax", "mov es, ax", "mov fs, ax", "mov gs, ax", "mov ss, ax",
        out("rax") _, options(preserves_flags)
    );
    asm!("ltr ax", in("ax") 0x28u16, options(nostack, preserves_flags));
    print("[GDT] OK\n");
}

// ============================================================================
// IDT
// ============================================================================
#[repr(C,packed)] #[derive(Clone,Copy)]
struct IdtE { lo:u16,sel:u16,ist:u8,attr:u8,mi:u16,hi:u32,_z:u32 }
impl IdtE {
    const fn null() -> Self { Self{lo:0,sel:0,ist:0,attr:0,mi:0,hi:0,_z:0} }
    fn new(h:u64,sel:u16,dpl:u8,ist:u8) -> Self {
        Self { lo:(h&0xFFFF)as u16, mi:((h>>16)&0xFFFF)as u16,
               hi:((h>>32)&0xFFFFFFFF)as u32, sel, ist, attr:0x8E|(dpl<<5), _z:0 }
    }
}
#[repr(C,packed)] struct Idtr { lim:u16, base:u64 }

const IDT_LEN: usize = 256;
static mut IDT:  [IdtE; IDT_LEN] = [IdtE::null(); IDT_LEN];
static mut IDTR: Idtr             = Idtr { lim:0, base:0 };

unsafe fn init_idt() {
    IDT[0x08]=IdtE::new(isr_df  as *const () as u64, 0x08, 0, 1);
    IDT[0x0E]=IdtE::new(isr_pf  as *const () as u64, 0x08, 0, 0);
    IDT[0x20]=IdtE::new(isr_tmr as *const () as u64, 0x08, 0, 0);
    IDT[0x80]=IdtE::new(isr_sys as *const () as u64, 0x08, 3, 0);
    IDTR.lim  = (core::mem::size_of::<[IdtE;IDT_LEN]>()-1) as u16;
    IDTR.base = IDT.as_ptr() as u64;
    asm!("lidt [{}]", in(reg) &raw const IDTR, options(preserves_flags));
    asm!("sti", options(nomem, nostack));
    print("[IDT] OK\n");
}

// ============================================================================
// PIC + PIT
// ============================================================================
unsafe fn init_pic() {
    outb(0x20,0x11); io_wait(); outb(0xA0,0x11); io_wait();
    outb(0x21,0x20); io_wait(); outb(0xA1,0x28); io_wait();
    outb(0x21,0x04); io_wait(); outb(0xA1,0x02); io_wait();
    outb(0x21,0x01); io_wait(); outb(0xA1,0x01); io_wait();
    outb(0x21,0xFE); outb(0xA1,0xFF);
    print("[PIC] OK\n");
}

unsafe fn init_pit() {
    let d=(1193180u32/100) as u16;
    outb(0x43,0x36); outb(0x40,(d&0xFF)as u8); outb(0x40,(d>>8)as u8);
    print("[PIT] 100Hz\n");
}

// ============================================================================
// ISR
// ============================================================================
macro_rules! isr_no_err {
    ($n:ident, $h:expr) => {
        #[unsafe(naked)] unsafe extern "C" fn $n() {
            naked_asm!(
                "push rax","push rbp","push rbx","push rcx","push rdx",
                "push rsi","push rdi","push r8","push r9","push r10",
                "push r11","push r12","push r13","push r14","push r15",
                "mov rdi, rsp", "call {f}",
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
                // xchg: rax ↔ error_code na stosie → tf.rax = error_code
                "xchg rax, [rsp]",
                "push rbp","push rbx","push rcx","push rdx","push rsi","push rdi",
                "push r8","push r9","push r10","push r11","push r12","push r13","push r14","push r15",
                "mov rdi, rsp", "call {f}",
                "pop r15","pop r14","pop r13","pop r12","pop r11","pop r10","pop r9","pop r8",
                "pop rdi","pop rsi","pop rdx","pop rcx","pop rbx","pop rbp",
                "add rsp, 8", "iretq",
                f = sym $h,
            );
        }
    };
}

// Double Fault — używa IST1, pushuje error_code=0
#[unsafe(naked)]
unsafe extern "C" fn isr_df() {
    naked_asm!(
        "cli",
        "add rsp, 8",    // usuń error_code (zawsze 0)
        "mov rdi, rsp",
        "call {f}",
        "cli", "hlt",
        f = sym handle_df,
    );
}
#[no_mangle]
unsafe extern "C" fn handle_df(_: *mut TF) {
    print_raw("\n[#DF] Double fault!\n");
    loop { asm!("hlt", options(nomem, nostack)); }
}

isr_with_err!(isr_pf, handle_pf);
#[no_mangle]
unsafe extern "C" fn handle_pf(f: *mut TF) {
    let err = (*f).rax;
    let addr: u64;
    asm!("mov {}, cr2", out(reg) addr, options(nomem, nostack));
    print("[PF] 0x"); print(u64_hex(addr, &mut [0u8;18]));
    print(if err&4!=0 {" USR"} else {" KRN"});
    print(if err&2!=0 {" W"} else {" R"});
    print(if err&1!=0 {" PROT\n"} else {" NP\n"});
    panic_no_dyn("Page fault");
}

isr_no_err!(isr_tmr, handle_timer);
#[no_mangle]
unsafe extern "C" fn handle_timer(_: *mut TF) {
    outb(0x20, 0x20);
    schedule();
}

isr_no_err!(isr_sys, handle_syscall);
#[no_mangle]
unsafe extern "C" fn handle_syscall(f: *mut TF) {
    let tf = &mut *f;
    let num = tf.rax; let a1=tf.rdi; let a2=tf.rsi; let a3=tf.rdx;
    let p4 = THREADS[CURRENT.load(Ordering::Relaxed)].cr3;
    tf.rax = match num {
        1 => { // SYS_WRITE
            if a1==1||a1==2 {
                if !valid_buf(p4, a2, a3 as usize) { !0 } else {
                    let ptr=a2 as *const u8;
                    VGA_LOCK.lock();
                    for i in 0..a3 as usize { putchar_raw(*ptr.add(i) as char); }
                    VGA_LOCK.unlock();
                    a3
                }
            } else { 0 }
        }
        2 => 0, // SYS_READ
        0 => {  // SYS_EXIT
            let cur=CURRENT.load(Ordering::Relaxed);
            THREADS[cur].state = TS::Terminated;
            THREAD_COUNT.fetch_sub(1, Ordering::Relaxed);
            schedule(); 0
        }
        _ => !0,
    };
}

// ============================================================================
// THREADING
// ============================================================================
#[derive(Clone,Copy,PartialEq)]
pub enum TS { Running, Ready, Blocked, Terminated }

#[derive(Copy,Clone)] #[repr(C)]
pub struct Thread {
    pub id:u32, pub state:TS, pub prio:u8,
    pub krsp:VirtAddr, pub ktop:VirtAddr, pub utop:VirtAddr,
    pub cr3:PhysAddr,  pub name:[u8;16],
}
impl Thread {
    pub const fn new() -> Self {
        Self { id:0,state:TS::Terminated,prio:10,krsp:0,ktop:0,utop:0,cr3:0,name:[0;16] }
    }
}

static mut THREADS:     [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS];
static CURRENT:         AtomicUsize           = AtomicUsize::new(0);
static THREAD_COUNT:    AtomicUsize           = AtomicUsize::new(0);
static SCHED_LOCK:      Spinlock              = Spinlock::new();

pub unsafe fn thread_init() {
    let tid = tcreate_k("idle\0", kernel_idle as *const () as u64, 0);
    if tid>=0 { THREADS[tid as usize].state=TS::Running; CURRENT.store(tid as usize, Ordering::SeqCst); }
    print("[THREAD] init OK\n");
}

pub unsafe fn tcreate_k(name:&str, entry:u64, arg:u64) -> i32 { tcreate(name,entry,arg,false) }
pub unsafe fn tcreate_u(name:&str, entry:u64, arg:u64) -> i32 { tcreate(name,entry,arg,true)  }

unsafe fn tcreate(name:&str, entry:u64, arg:u64, user:bool) -> i32 {
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Terminated { continue; }
        let t = &mut THREADS[i];

        // Stos kernela: region zaczyna się od 32MB + offset per-thread
        let kb = 0x0200_0000u64 + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64;
        let ks = kb + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE/PAGE_SIZE) {
            mm_map(KERNEL_P4, ks + p as u64*PAGE_SIZE as u64, mm_alloc_frame(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64;

        let (ut, cr3) = if user {
            let ncr3 = create_user_p4();
            // Stos użytkownika: 64MB + offset
            let ub = 0x0400_0000u64 + i as u64 * (USER_STACK_SIZE + PAGE_SIZE) as u64;
            let us = ub + PAGE_SIZE as u64;
            for p in 0..(USER_STACK_SIZE/PAGE_SIZE) {
                mm_map(ncr3, us + p as u64*PAGE_SIZE as u64, mm_alloc_frame(), PTE_W|PTE_U);
            }
            (us + USER_STACK_SIZE as u64, ncr3)
        } else { (kt, KERNEL_P4) };

        t.id=i as u32; t.state=TS::Ready; t.prio=if user{5}else{10};
        t.ktop=kt; t.utop=ut; t.cr3=cr3;

        // Stos początkowy wątku (trampoline wywoła entry z arg)
        let mut ksp = kt;
        macro_rules! push { ($v:expr) => { ksp-=8; *(ksp as *mut u64)=$v as u64; } }
        push!(if user { trampoline_user as *const () as u64 }
              else    { trampoline_kernel as *const () as u64 });
        push!(0u64);  // r15 = arg
        push!(0u64);  // r14 = entry
        push!(0u64);  // r13 = utop
        push!(ut);    // → r13 przy pop
        push!(entry); // → r14 przy pop
        push!(arg);   // → r15 przy pop
        t.krsp = ksp;

        let b=name.as_bytes();
        for j in 0..core::cmp::min(15,b.len()) { t.name[j]=b[j]; }
        THREAD_COUNT.fetch_add(1, Ordering::Relaxed);

        let mut buf=[0u8;20];
        print("[T] #"); print(usize_str(i,&mut buf)); print(" "); print(name); print("\n");
        return i as i32;
    }
    -1
}

#[unsafe(naked)]
unsafe extern "C" fn trampoline_kernel() {
    naked_asm!("mov rdi, r15", "call r14", "cli", "hlt");
}

#[unsafe(naked)]
unsafe extern "C" fn trampoline_user() {
    naked_asm!(
        "push 0x20|3", "push r13", "push 0x202",
        "push 0x18|3", "push r14",
        "mov rdi, r15", "iretq",
    );
}

pub unsafe fn schedule() {
    if SCHED_LOCK.locked.swap(true, Ordering::Acquire) { return; }
    let cur = CURRENT.load(Ordering::Relaxed);
    let mut next = cur;
    for _ in 0..MAX_THREADS {
        next = (next+1) % MAX_THREADS;
        if THREADS[next].state == TS::Ready { break; }
    }
    if next==cur && THREADS[cur].state==TS::Running {
        SCHED_LOCK.locked.store(false, Ordering::Release); return;
    }
    if THREADS[cur].state==TS::Running { THREADS[cur].state=TS::Ready; }
    THREADS[next].state = TS::Running;
    CURRENT.store(next, Ordering::SeqCst);
    tss_rsp0(THREADS[next].ktop);
    let ncr3=THREADS[next].cr3;
    let ccr3:u64; asm!("mov {}, cr3", out(reg) ccr3, options(nomem,nostack));
    if ncr3!=0 && ncr3!=ccr3 { asm!("mov cr3, {}", in(reg) ncr3, options(nostack)); }
    SCHED_LOCK.locked.store(false, Ordering::Release);
    thread_switch(&mut THREADS[cur].krsp as *mut u64, THREADS[next].krsp);
}

#[unsafe(naked)]
unsafe extern "C" fn thread_switch(old: *mut VirtAddr, new: VirtAddr) {
    naked_asm!(
        "push rbx","push rbp","push r12","push r13","push r14","push r15",
        "mov [rdi], rsp", "mov rsp, rsi",
        "pop r15","pop r14","pop r13","pop r12","pop rbp","pop rbx",
        "ret",
    );
}

unsafe extern "C" fn kernel_idle(_:u64) -> ! {
    loop { asm!("hlt", options(nomem,nostack)); }
}

// ============================================================================
// MULTIBOOT2 — parsowanie modułów
// ============================================================================
const MB2_MAGIC_EXPECTED: u64 = 0x36d76289;

#[repr(C, packed)]
struct Mb2InfoHeader { total_size: u32, reserved: u32 }

#[repr(C, packed)]
struct Mb2Tag { typ: u32, size: u32 }

#[repr(C, packed)]
struct Mb2TagModule {
    typ: u32, size: u32,
    mod_start: u32, mod_end: u32,
    // string follows
}

// Zwraca (start, end) pierwszego modułu (userspace.bin)
pub unsafe fn mb2_find_module(info_ptr: u64) -> Option<(u64, u64)> {
    if info_ptr == 0 { return None; }
    let hdr = &*(info_ptr as *const Mb2InfoHeader);
    let total = hdr.total_size as u64;
    let mut off = 8u64; // skip header
    while off < total {
        let tag = &*((info_ptr + off) as *const Mb2Tag);
        if tag.typ == 0 { break; } // end tag
        if tag.typ == 3 { // module tag
            let mt = &*((info_ptr + off) as *const Mb2TagModule);
            let start = mt.mod_start as u64;
            let end   = mt.mod_end   as u64;
            return Some((start, end));
        }
        // następny tag wyrównany do 8
        let sz = tag.size as u64;
        off += (sz + 7) & !7;
    }
    None
}

// ============================================================================
// USERSPACE LOADER
// Ładuje flat binary (ELF lub raw) z multiboot modułu jako wątek kernela
// (user=false, bo jeszcze nie mamy pełnego ELF parsera)
// Mapuje go pod stałym adresem wirtualnym i uruchamia
// ============================================================================
const US_VIRT_BASE: u64 = 0x0080_0000; // 8MB — adres wirtualny userspace

pub unsafe fn load_userspace(mod_start: u64, mod_end: u64) {
    const PAGE_SIZE_U64: u64 = PAGE_SIZE as u64;
    
    if mod_end <= mod_start {
        print("[US] Niepoprawny zakres modułu!\n");
        return;
    }

    let mut b = [0u8; 20];
    
    // === DIAGNOZA: Co mamy w userspace.bin? ===
    print("[US] Modul: 0x"); print(u64_hex(mod_start, &mut b));
    print(" - 0x"); print(u64_hex(mod_end, &mut b));
    print(" ("); print(usize_str((mod_end - mod_start) as usize, &mut b)); print(" B)\n");

    // Sprawdź pierwsze bajty
    let first_word = *(mod_start as *const u32);
    print("[US] Pierwsze 4 bajty: 0x"); print(u64_hex(first_word as u64, &mut b)); print("\n");

    // === Zaokrąglij adresy do stron ===
    let phys_aligned = mod_start & !(PAGE_SIZE_U64 - 1);
    let offset_in_page = mod_start - phys_aligned;
    let size_raw = (mod_end - mod_start) as usize;
    let size_aligned = ((offset_in_page + size_raw as u64 + PAGE_SIZE_U64 - 1) / PAGE_SIZE_U64) * PAGE_SIZE_U64;

    // === Mapuj pod US_VIRT_BASE ===
    let pages = (size_aligned / PAGE_SIZE_U64) as usize;
    print("[US] Mapowanie "); print(usize_str(pages, &mut b)); print(" stron...\n");
    
    for p in 0..pages {
        let phys = phys_aligned + (p as u64) * PAGE_SIZE_U64;
        let virt = US_VIRT_BASE + (p as u64) * PAGE_SIZE_U64;
        if mm_map(KERNEL_P4, virt, phys, PTE_W) != 0 {
            print("[US] Błąd mapowania strony "); print(usize_str(p, &mut b)); print("\n");
            return;
        }
    }
    print("[US] Zmapowano "); print(usize_str(pages, &mut b)); print(" stron\n");

    // === Sprawdź czy to ELF ===
    let magic = *(US_VIRT_BASE as *const u32);
    
    let entry_point = if magic == 0x464C457F {
        // ELF
        print("[US] Wykryto ELF\n");
        
        // Sprawdź e_type (offset 16)
        let e_type = *((US_VIRT_BASE + 16) as *const u16);
        let e_entry = *((US_VIRT_BASE + 24) as *const u64);
        
        print("[US] e_type=0x"); print(u64_hex(e_type as u64, &mut b)); print("\n");
        print("[US] e_entry=0x"); print(u64_hex(e_entry, &mut b)); print("\n");
        
        // e_type: 1=REL, 2=EXEC, 3=DYN
        if e_type == 2 {
            // ET_EXEC - ma hardcoded adresy, użyj entry bez zmian
            print("[US] ELF jest ET_EXEC - uzywam entry z naglowka\n");
            e_entry
        } else {
            // ET_REL lub ET_DYN - przelicz względem US_VIRT_BASE
            print("[US] ELF jest pozycjonowalny - przeliczam entry\n");
            US_VIRT_BASE + offset_in_page
        }
    } else {
        // Raw binary
        print("[US] Raw binary - entry=0x"); print(u64_hex(US_VIRT_BASE + offset_in_page, &mut b)); print("\n");
        US_VIRT_BASE + offset_in_page
    };

    // === Stwórz wątek userspace ===
    // Używamy trybu kernela (bezpieczniej na początek)
    for i in 1..MAX_THREADS {
        if THREADS[i].state == TS::Terminated {
            let t = &mut THREADS[i];
            
            // Przygotuj stos jądrowy
            let kb = 0x0200_0000u64 + (i as u64) * (KERNEL_STACK_SIZE as u64 + PAGE_SIZE as u64);
            let ks = kb + PAGE_SIZE as u64;
            for p in 0..(KERNEL_STACK_SIZE/PAGE_SIZE) {
                mm_map(KERNEL_P4, ks + (p as u64)*PAGE_SIZE as u64, mm_alloc_frame(), PTE_W);
            }
            let kt = ks + KERNEL_STACK_SIZE as u64;

            t.id = i as u32;
            t.state = TS::Ready;
            t.prio = 10;
            t.ktop = kt;
            t.utop = 0;
            t.cr3 = KERNEL_P4;
            t.name = [0; 16];
            
            // Przygotuj stos z trampoline
            let mut ksp = kt;
            macro_rules! push { ($v:expr) => { ksp -= 8; *(ksp as *mut u64) = $v as u64; } }
            
            push!(trampoline_kernel as *const () as u64);
            push!(0u64);  // r15 = arg
            push!(entry_point); // r14 = entry point
            push!(0u64);  // r13
            push!(0u64);  // utop
            push!(entry_point);
            push!(0u64);  // arg
            
            t.krsp = ksp;
            
            let name = b"userspace";
            for j in 0..name.len().min(15) { t.name[j] = name[j]; }
            
            THREAD_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            
            print("[T] #"); print(usize_str(i, &mut b)); print(" userspace (entry=0x"); 
            print(u64_hex(entry_point, &mut b)); print(")\n");
            return;
        }
    }
    
    print("[US] Brak wolnych slotów na wątki!\n");
}

// ============================================================================
// HELPERS
// ============================================================================
pub fn usize_str<'a>(mut v: usize, buf: &'a mut [u8;20]) -> &'a str {
    if v==0 { buf[19]=b'0'; return unsafe{core::str::from_utf8_unchecked(&buf[19..])}; }
    let mut i=19usize;
    while v>0 { buf[i]=b'0'+(v%10)as u8; v/=10; if i==0{break}else{i-=1;} }
    unsafe { core::str::from_utf8_unchecked(&buf[i+1..]) }
}

pub fn u64_hex<'a>(mut v: u64, buf: &'a mut [u8;18]) -> &'a str {
    const H: &[u8] = b"0123456789ABCDEF";
    buf[0]=b'0'; buf[1]=b'x';
    for i in (2..18).rev() { buf[i]=H[(v&0xF)as usize]; v>>=4; }
    unsafe { core::str::from_utf8_unchecked(buf) }
}

fn panic_no_dyn(msg: &str) -> ! {
    unsafe {
        asm!("cli", options(nomem,nostack));
        print_raw("\n[PANIC] "); print_raw(msg); print_raw("\n");
        loop { asm!("hlt", options(nomem,nostack)); }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe { asm!("cli", options(nomem,nostack)); }
    unsafe { print_raw("\n[PANIC] "); }
    match info.message().as_str() {
        Some(s) => unsafe { print_raw(s); },
        None    => unsafe { print_raw("?"); },
    }
    unsafe { print_raw("\n"); }
    loop { unsafe { asm!("hlt", options(nomem,nostack)); } }
}

// ============================================================================
// KERNEL MAIN
// ============================================================================
#[no_mangle]
pub extern "C" fn kernel_main(mb_magic: u64, mb_info: u64) -> ! {
    unsafe {
        clear_screen();
        serial_init();
        serial_print("CosinusOS v3.3\n");

        // ── Nagłówek ──────────────────────────────────────────────────────────
        set_color(color::attr(color::LIGHT_CYAN, color::BLACK));
        print("  ____           _                   ___  ____  \n");
        print(" / ___|___  ___ (_)_ __  _   _ ___  / _ \\/ ___| \n");
        print("| |   / _ \\/ __|| | '_ \\| | | / __|| | | \\___ \\ \n");
        print("| |__| (_) \\__ \\| | | | | |_| \\__ \\| |_| |___) |\n");
        print(" \\____\\___/|___/|_|_| |_|\\__,_|___/ \\___/|____/ \n");
        set_color(color::WHITE);
        print("                  Microkernel v3.3\n");
        print("\n");

        // ── Inicjalizacja z logiem statusu ────────────────────────────────────
        print_col("=== Boot sequence ===\n", color::YELLOW);
        print("\n");

        // PMM
        mm_init(0x0080_0000, 0x0780_0000);
        mm_init_paging(0x1000);
        log_ok("Physical Memory Manager", true);
        mm_dump_stats();

        // GDT
        init_gdt();
        log_ok("Global Descriptor Table", true);

        // PIC
        init_pic();
        log_ok("Programmable Interrupt Controller", true);

        // IDT
        init_idt();
        log_ok("Interrupt Descriptor Table", true);

        // Scheduler / wątki — PRZED PIT
        thread_init();
        log_ok("Thread scheduler", true);

        // PIT — jako ostatni żeby nie odpalił przed wątkami
        init_pit();
        log_ok("Programmable Interval Timer (100Hz)", true);

        print("\n");

        // ── Multiboot2 / userspace ────────────────────────────────────────────
        let mut b = [0u8; 18];
        print_col("=== Userspace loader ===\n", color::YELLOW);
        print("\n");

        if mb_magic == MB2_MAGIC_EXPECTED {
            log_ok("Multiboot2 magic", true);
            match mb2_find_module(mb_info) {
                Some((start, end)) => {
                    log_ok("Userspace module found", true);
                    load_userspace(start, end);
                    // sprawdź czy wątek userspace powstał
                    let ok = thread_count() > 1;
                    log_ok("Userspace thread created", ok);
                    if !ok {
                        log_err("Nie udalo sie uruchomic userspace!");
                    }
                }
                None => {
                    log_ok("Userspace module found", false);
                    log_warn("Brak modulu userspace w obrazie ISO.");
                    log_warn("Dodaj 'module2 /boot/userspace.bin' do grub.cfg");
                }
            }
        } else {
            log_ok("Multiboot2 magic", false);
            print_col("  Otrzymano: 0x", color::LIGHT_RED);
            print(u64_hex(mb_magic, &mut b));
            print("\n");
            log_warn("Pomijam ladowanie userspace.");
        }

        print("\n");
        print_col("=== Stan systemu ===\n", color::YELLOW);
        print("\n");
        mm_dump_stats();
        thread_dump();

        print("\n");
        print_col("[ SYSTEM GOTOWY ]", color::attr(color::BLACK, color::LIGHT_GREEN));
        set_color(color::WHITE);
        print(" Scheduler uruchomiony.\n\n");

        serial_print("[OK] Kernel boot complete\n");

        schedule();
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}