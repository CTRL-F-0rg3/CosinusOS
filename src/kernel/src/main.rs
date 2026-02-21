// src/kernel/src/main.rs — CosinusOS Microkernel v3.1
//
// Zmiany względem v3.0:
//  [FIX] User CR3: kopiowanie górnych 256 wpisów P4 z kernelowego P4
//  [FIX] mm_map_page/mm_unmap_page przyjmują p4_phys jako argument
//  [FIX] Pełny TrapFrame we WSZYSTKICH ISR (timer, page fault, syscall) przez makra
//  [FIX] IST=1 dla double fault (dedykowany stos, nie korzysta z bieżącego RSP)
//  [FIX] Guard page dla każdego stosu kernelowego i użytkownika
//  [FIX] Walidacja user-pointerów: present + user bit przez całe drzewo P4→P1
//  [FIX] mm_alloc_frame: hint pointer → O(1) amortyzowane zamiast liniowego skanowania
//  [FIX] mm_unmap_page: lazy freeing pustych P1/P2/P3 (brak memory leak)
//  [FIX] thread_switch: callee-saved + RIP przez ret (spójna konwencja)
#![no_std]
#![no_main]
#![feature(asm_const, naked_functions, abi_x86_interrupt)]
#![allow(unsafe_op_in_unsafe_fn)]

use core::{
    arch::asm,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    panic::PanicInfo,
};

// ============================================================================
// TYPES & CONSTANTS
// ============================================================================
type PhysAddr = u64;
type VirtAddr = u64;

const PAGE_SIZE:            usize = 0x1000;
const MAX_FRAMES:           usize = 0x10000;
const MAX_THREADS:          usize = 64;
const KERNEL_STACK_SIZE:    usize = 0x8000; // 32 KiB
const USER_STACK_SIZE:      usize = 0x4000; // 16 KiB
const DOUBLE_FAULT_STACK_SIZE: usize = 0x4000; // 16 KiB — IST1

/// Higher-half offset: phys + PHYS_OFFSET = virt (identity-mapped region).
const PHYS_OFFSET: VirtAddr = 0xFFFF_8000_0000_0000;

#[inline(always)] pub const fn phys_to_virt(p: PhysAddr) -> VirtAddr { p + PHYS_OFFSET }
#[inline(always)] pub const fn virt_to_phys(v: VirtAddr) -> PhysAddr { v - PHYS_OFFSET }

// ============================================================================
// MULTIBOOT2 HEADER
// ============================================================================
const MB2_MAGIC: u32 = 0xe85250d6;
const MB2_ARCH:  u32 = 0;
const MB2_TAG_END: u16 = 0;

#[repr(C, packed)] struct Mb2Header  { magic:u32, arch:u32, len:u32, checksum:u32 }
#[repr(C, packed)] struct Mb2Tag    { type_:u16, flags:u16, size:u32 }
#[repr(C, packed)] struct Mb2Boot   { hdr: Mb2Header, end: Mb2Tag }

#[link_section = ".multiboot"] #[used]
static MB2: Mb2Boot = Mb2Boot {
    hdr: Mb2Header {
        magic: MB2_MAGIC, arch: MB2_ARCH,
        len: core::mem::size_of::<Mb2Boot>() as u32,
        checksum: (-(MB2_MAGIC as i32 + MB2_ARCH as i32
            + core::mem::size_of::<Mb2Boot>() as i32)) as u32,
    },
    end: Mb2Tag { type_: MB2_TAG_END, flags: 0, size: 8 },
};

// ============================================================================
// PORT I/O
// ============================================================================
#[inline(always)] unsafe fn outb(port: u16, val: u8) {
    asm!("outb %al, %dx", in("al") val, in("dx") port, options(nostack));
}
#[inline(always)] unsafe fn inb(port: u16) -> u8 {
    let r: u8; asm!("inb %dx, %al", out("al") r, in("dx") port, options(nostack)); r
}
fn io_wait() { unsafe { outb(0x80, 0); } }

// ============================================================================
// SPINLOCK
// ============================================================================
pub struct Spinlock { locked: AtomicBool }
impl Spinlock {
    pub const fn new() -> Self { Self { locked: AtomicBool::new(false) } }
    #[inline]
    pub fn lock(&self) {
        while self.locked.swap(true, Ordering::Acquire) {
            while self.locked.load(Ordering::Relaxed) { core::hint::spin_loop(); }
        }
    }
    #[inline]
    pub fn unlock(&self) { self.locked.store(false, Ordering::Release); }
}

// ============================================================================
// VGA DRIVER
// ============================================================================
const VGA_W: usize    = 80;
const VGA_H: usize    = 25;
const VGA_MEM: *mut u16 = 0xB8000 as *mut u16;

static mut VGA_BUF:  *mut u16 = VGA_MEM;
static mut CUR_X:    usize    = 0;
static mut CUR_Y:    usize    = 0;
static mut VGA_COL:  u8       = 0x0F;
static     VGA_LOCK: Spinlock = Spinlock::new();

unsafe fn vga_move_cursor() {
    let pos = CUR_Y * VGA_W + CUR_X;
    outb(0x3D4, 0x0F); outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E); outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
}

pub unsafe fn clear_screen() {
    for i in 0..(VGA_W * VGA_H) {
        *VGA_BUF.add(i) = ((VGA_COL as u16) << 8) | b' ' as u16;
    }
    CUR_X = 0; CUR_Y = 0; vga_move_cursor();
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
        for i in 0..(VGA_H - 1) * VGA_W {
            *VGA_BUF.add(i) = *VGA_BUF.add(i + VGA_W);
        }
        for i in 0..VGA_W {
            *VGA_BUF.add((VGA_H - 1) * VGA_W + i) = ((VGA_COL as u16) << 8) | b' ' as u16;
        }
        CUR_Y = VGA_H - 1;
    }
    vga_move_cursor();
}

/// Thread-safe print przez spinlock.
pub unsafe fn print(s: &str) {
    VGA_LOCK.lock();
    for c in s.chars() { putchar_raw(c); }
    VGA_LOCK.unlock();
}

/// Panic-safe print — BEZ locka. Używaj tylko w panic handler / bardzo wczesnym boocie.
pub unsafe fn print_raw(s: &str) {
    for c in s.chars() { putchar_raw(c); }
}

// ============================================================================
// SERIAL
// ============================================================================
const COM1: u16 = 0x3F8;
unsafe fn serial_init() {
    outb(COM1+1,0x00); outb(COM1+3,0x80); outb(COM1+0,0x03);
    outb(COM1+1,0x00); outb(COM1+3,0x03); outb(COM1+2,0xC7); outb(COM1+4,0x0B);
}
unsafe fn serial_write(c: char) { while (inb(COM1+5) & 0x20)==0 {} outb(COM1, c as u8); }
unsafe fn serial_print(s: &str) { for c in s.chars() { serial_write(c); } }

// ============================================================================
// PHYSICAL FRAME ALLOCATOR  (hint pointer → O(1) amortyzowane)
// ============================================================================
static MM_LOCK:       Spinlock = Spinlock::new();
static mut FRAME_BM:  [u64; MAX_FRAMES / 64] = [0u64; MAX_FRAMES / 64];
static mut MEM_BASE:  PhysAddr = 0;
static mut MEM_SIZE:  usize    = 0;
static mut ALLOC_HINT: usize   = 0; // indeks słowa bitmapy od którego zaczynamy

unsafe fn fi(phys: PhysAddr) -> usize { ((phys - MEM_BASE) / PAGE_SIZE as u64) as usize }
unsafe fn fp(idx: usize) -> PhysAddr  { MEM_BASE + idx as u64 * PAGE_SIZE as u64 }
unsafe fn is_free(idx: usize) -> bool { (FRAME_BM[idx/64] & (1u64<<(idx%64))) == 0 }
unsafe fn set_used(idx: usize) { FRAME_BM[idx/64] |=  1u64 << (idx%64); }
unsafe fn set_free(idx: usize) {
    FRAME_BM[idx/64] &= !(1u64 << (idx%64));
    if idx/64 < ALLOC_HINT { ALLOC_HINT = idx/64; } // cofnij hint
}

pub unsafe fn mm_init(base: PhysAddr, size: usize) {
    MEM_BASE = base; MEM_SIZE = size;
    let frames = size / PAGE_SIZE;
    core::ptr::write_bytes(FRAME_BM.as_mut_ptr() as *mut u8, 0,
        core::mem::size_of_val(&FRAME_BM));
    for i in 0..core::cmp::min(256, frames) { set_used(i); }
    ALLOC_HINT = 4; // słowo 4 = ramka 256 (pierwsza wolna)
    let mut b = [0u8;20];
    print("[MM] Initialized: ");
    print(usize_str(frames * PAGE_SIZE / 1024 / 1024, &mut b));
    print(" MiB available\n");
}

pub unsafe fn mm_alloc_frame() -> PhysAddr {
    MM_LOCK.lock();
    let words = FRAME_BM.len();
    for pass in 0..2usize {
        let (s, e) = if pass == 0 { (ALLOC_HINT, words) } else { (0, ALLOC_HINT) };
        for w in s..e {
            if FRAME_BM[w] == !0u64 { continue; }
            for bit in 0..64usize {
                let idx = w * 64 + bit;
                if idx >= MAX_FRAMES { continue; }
                if is_free(idx) {
                    set_used(idx);
                    ALLOC_HINT = w;
                    MM_LOCK.unlock();
                    return fp(idx);
                }
            }
        }
    }
    MM_LOCK.unlock();
    panic_no_dyn("Physical memory exhausted");
}

pub unsafe fn mm_free_frame(phys: PhysAddr) {
    if phys < MEM_BASE { return; }
    let idx = fi(phys);
    if idx >= MAX_FRAMES { return; }
    MM_LOCK.lock();
    set_free(idx);
    MM_LOCK.unlock();
}

// ============================================================================
// PAGE TABLE HELPERS
// ============================================================================
const PTE_PRESENT:   u64 = 1 << 0;
const PTE_WRITABLE:  u64 = 1 << 1;
const PTE_USER:      u64 = 1 << 2;
const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

#[inline] fn pte_new(phys: PhysAddr, flags: u64) -> u64 { (phys & PTE_ADDR_MASK)|flags|PTE_PRESENT }
#[inline] fn pte_present(e: u64) -> bool { e & PTE_PRESENT  != 0 }
#[inline] fn pte_user(e:    u64) -> bool { e & PTE_USER     != 0 }
#[inline] fn pte_addr(e:    u64) -> PhysAddr { e & PTE_ADDR_MASK }

#[repr(C, align(4096))]
struct PageTable { entries: [u64; 512] }

#[inline] unsafe fn pt_at(phys: PhysAddr) -> *mut PageTable {
    phys_to_virt(phys) as *mut PageTable
}

unsafe fn alloc_zeroed_page() -> PhysAddr {
    let phys = mm_alloc_frame();
    core::ptr::write_bytes(pt_at(phys) as *mut u8, 0, PAGE_SIZE);
    phys
}

unsafe fn get_or_create(table_phys: PhysAddr, idx: usize, flags: u64) -> PhysAddr {
    let t = &mut *pt_at(table_phys);
    if !pte_present(t.entries[idx]) {
        let child = alloc_zeroed_page();
        t.entries[idx] = pte_new(child, flags);
    }
    pte_addr(t.entries[idx])
}

unsafe fn page_table_empty(phys: PhysAddr) -> bool {
    (*pt_at(phys)).entries.iter().all(|&e| e == 0)
}

// ============================================================================
// KERNEL P4
// ============================================================================
static mut KERNEL_P4_PHYS: PhysAddr = 0;

pub unsafe fn mm_init_paging(kernel_cr3: PhysAddr) {
    KERNEL_P4_PHYS = kernel_cr3;
    asm!("mov cr3, {}", in(reg) kernel_cr3, options(preserves_flags));
    print("[MMU] Paging enabled\n");
}

// ============================================================================
// MM_MAP_PAGE — p4_phys jako jawny argument
// ============================================================================
pub unsafe fn mm_map_page(p4_phys: PhysAddr, virt: VirtAddr, phys: PhysAddr, flags: u64) -> i32 {
    if virt & 0xFFF != 0 || phys & 0xFFF != 0 {
        print("[MM] map_page: unaligned address!\n"); return -1;
    }
    if p4_phys == 0 { print("[MM] map_page: p4_phys==0!\n"); return -1; }

    MM_LOCK.lock();
    let p4i = ((virt >> 39) & 0x1FF) as usize;
    let p3i = ((virt >> 30) & 0x1FF) as usize;
    let p2i = ((virt >> 21) & 0x1FF) as usize;
    let p1i = ((virt >> 12) & 0x1FF) as usize;

    let p3 = get_or_create(p4_phys, p4i, PTE_WRITABLE | PTE_USER);
    let p2 = get_or_create(p3,      p3i, PTE_WRITABLE | PTE_USER);
    let p1 = get_or_create(p2,      p2i, PTE_WRITABLE | PTE_USER);

    let t1 = &mut *pt_at(p1);
    if pte_present(t1.entries[p1i]) {
        print("[MM] map_page: WARNING overwriting existing PTE\n");
    }
    t1.entries[p1i] = pte_new(phys, flags);
    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    MM_LOCK.unlock();
    0
}

// ============================================================================
// MM_UNMAP_PAGE — z lazy freeing pustych tabel
// ============================================================================
pub unsafe fn mm_unmap_page(p4_phys: PhysAddr, virt: VirtAddr) {
    if p4_phys == 0 { return; }
    MM_LOCK.lock();

    let p4i = ((virt >> 39) & 0x1FF) as usize;
    let p3i = ((virt >> 30) & 0x1FF) as usize;
    let p2i = ((virt >> 21) & 0x1FF) as usize;
    let p1i = ((virt >> 12) & 0x1FF) as usize;

    let p4 = &mut *pt_at(p4_phys);
    if !pte_present(p4.entries[p4i]) { MM_LOCK.unlock(); return; }
    let p3_phys = pte_addr(p4.entries[p4i]);

    let p3 = &mut *pt_at(p3_phys);
    if !pte_present(p3.entries[p3i]) { MM_LOCK.unlock(); return; }
    let p2_phys = pte_addr(p3.entries[p3i]);

    let p2 = &mut *pt_at(p2_phys);
    if !pte_present(p2.entries[p2i]) { MM_LOCK.unlock(); return; }
    let p1_phys = pte_addr(p2.entries[p2i]);

    // 1. Zeruj wpis w P1, potem invlpg
    (*pt_at(p1_phys)).entries[p1i] = 0;
    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));

    // 2. Lazy freeing: zwolnij puste tabele w górę (P1 → P2 → P3)
    if page_table_empty(p1_phys) {
        mm_free_frame(p1_phys);
        p2.entries[p2i] = 0;
        if page_table_empty(p2_phys) {
            mm_free_frame(p2_phys);
            p3.entries[p3i] = 0;
            if page_table_empty(p3_phys) {
                mm_free_frame(p3_phys);
                p4.entries[p4i] = 0;
                // P4 samego siebie nie zwalniamy — zarządzany przez Thread
            }
        }
    }
    MM_LOCK.unlock();
}

// ============================================================================
// WALIDACJA USER-POINTERÓW (present + user bit przez całe P4→P1)
// ============================================================================
pub unsafe fn is_user_page_valid(p4_phys: PhysAddr, virt: VirtAddr) -> bool {
    if p4_phys == 0 { return false; }
    let p4i = ((virt >> 39) & 0x1FF) as usize;
    let p3i = ((virt >> 30) & 0x1FF) as usize;
    let p2i = ((virt >> 21) & 0x1FF) as usize;
    let p1i = ((virt >> 12) & 0x1FF) as usize;

    macro_rules! check {
        ($phys:expr, $idx:expr) => {{
            let t = &*pt_at($phys);
            let e = t.entries[$idx];
            if !pte_present(e) || !pte_user(e) { return false; }
            pte_addr(e)
        }};
    }
    let p3_phys = check!(p4_phys, p4i);
    let p2_phys = check!(p3_phys, p3i);
    let p1_phys = check!(p2_phys, p2i);
    let e1 = (*pt_at(p1_phys)).entries[p1i];
    pte_present(e1) && pte_user(e1)
}

pub unsafe fn validate_user_buf(p4_phys: PhysAddr, ptr: VirtAddr, len: usize) -> bool {
    if len == 0 { return true; }
    let mut page = ptr & !(PAGE_SIZE as u64 - 1);
    let end = ptr + len as u64;
    while page < end {
        if !is_user_page_valid(p4_phys, page) { return false; }
        page += PAGE_SIZE as u64;
    }
    true
}

// ============================================================================
// USER P4 — z dziedziczeniem mapowań kernela (górne 256 wpisów)
// ============================================================================
pub unsafe fn create_user_p4() -> PhysAddr {
    let new_p4 = alloc_zeroed_page();
    let src = &*pt_at(KERNEL_P4_PHYS);
    let dst = &mut *pt_at(new_p4);
    // Wpisy 256..511 = wyższe pół canonical space = kernel higher-half
    for i in 256..512 {
        dst.entries[i] = src.entries[i];
    }
    new_p4
}

// ============================================================================
// SWAP FRAMEWORK
// ============================================================================
const SWAP_ENABLED: bool = true;
static mut SWAP_BM:  [u64; 1024] = [0u64; 1024];
static SWAP_LOCK: Spinlock = Spinlock::new();

pub unsafe fn mm_swap_init() {
    if !SWAP_ENABLED { return; }
    print("[SWAP] Framework initialized\n");
}
pub unsafe fn mm_swap_out(p4_phys: PhysAddr, virt: VirtAddr, phys: PhysAddr) -> bool {
    if !SWAP_ENABLED { return false; }
    SWAP_LOCK.lock();
    for w in 0..SWAP_BM.len() {
        if SWAP_BM[w] == !0u64 { continue; }
        for bit in 0..64usize {
            if SWAP_BM[w] & (1u64 << bit) == 0 {
                SWAP_BM[w] |= 1u64 << bit;
                SWAP_LOCK.unlock();
                let mut b1=[0u8;18]; let mut b2=[0u8;20];
                print("[SWAP] OUT 0x"); print(u64_hex(virt, &mut b1));
                print(" slot "); print(usize_str(w*64+bit, &mut b2)); print("\n");
                mm_unmap_page(p4_phys, virt);
                mm_free_frame(phys);
                return true;
            }
        }
    }
    SWAP_LOCK.unlock();
    print("[SWAP] NO FREE SLOTS!\n");
    false
}
pub unsafe fn mm_swap_in(p4_phys: PhysAddr, virt: VirtAddr, flags: u64) -> Option<PhysAddr> {
    if !SWAP_ENABLED { return None; }
    let phys = mm_alloc_frame();
    core::ptr::write_bytes(phys_to_virt(phys) as *mut u8, 0xAA, PAGE_SIZE);
    mm_map_page(p4_phys, virt, phys, flags);
    let mut b=[0u8;18];
    print("[SWAP] IN -> 0x"); print(u64_hex(virt, &mut b)); print("\n");
    Some(phys)
}

// ============================================================================
// TRAPFRAME — wspólna struktura dla WSZYSTKICH ISR
// ============================================================================
/// Dokładny układ stosu po wejściu do ISR.
/// CPU odkłada (ring3→ring0): [SS, RSP, RFLAGS, CS, RIP].
/// Dla wyjątków z error code CPU wkłada error_code PRZED RIP.
/// Nasz handler w obu przypadkach odkłada rejestry tak, by pasowały do tej struktury.
#[repr(C)]
pub struct TrapFrame {
    // Odkładane przez handler (od szczytu stosu w dół):
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9:  u64, pub r8:  u64,
    pub rdi: u64, pub rsi: u64, pub rdx: u64, pub rcx: u64,
    pub rbx: u64, pub rbp: u64, pub rax: u64, // rax = error_code dla wyjątków z err
    // Odkładane przez CPU (iretq frame, od szczytu stosu):
    pub rip: u64, pub cs: u64, pub rflags: u64, pub rsp: u64, pub ss: u64,
}

// ============================================================================
// TSS
// ============================================================================
#[repr(C, packed)]
pub struct Tss {
    _res0:   u32,
    pub rsp0: u64,  // kernel stack dla ring3→ring0
    pub rsp1: u64,
    pub rsp2: u64,
    _res1:   u64,
    pub ist1: u64,  // IST1: dedykowany stos dla #DF
    _ist:    [u64; 6],
    _res2:   u64,
    _res3:   u16,
    pub iomap: u16, // = sizeof(TSS) → brak IOPM
}
impl Tss {
    pub const fn new() -> Self {
        Self { _res0:0, rsp0:0, rsp1:0, rsp2:0, _res1:0, ist1:0,
               _ist:[0;6], _res2:0, _res3:0,
               iomap: core::mem::size_of::<Tss>() as u16 }
    }
}
static mut TSS: Tss = Tss::new();

// Statyczny stos dla double fault (IST1)
static mut DF_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0u8; DOUBLE_FAULT_STACK_SIZE];

pub unsafe fn tss_set_rsp0(rsp0: VirtAddr) { TSS.rsp0 = rsp0; }

// ============================================================================
// GDT
// ============================================================================
// Selektory:
//   0x00 null | 0x08 kcode | 0x10 kdata | 0x18 ucode | 0x20 udata
//   0x28 TSS_lo (8B) | TSS_hi (8B, za entries[5])

#[repr(C, packed)] #[derive(Clone, Copy)]
struct GdtE { lo_lim: u16, lo_base: u16, mi_base: u8, access: u8, gran: u8, hi_base: u8 }
impl GdtE {
    const fn null() -> Self { Self{lo_lim:0,lo_base:0,mi_base:0,access:0,gran:0,hi_base:0} }
    fn new(base: u64, lim: u64, access: u8, gran: u8) -> Self {
        Self {
            lo_lim:  (lim  & 0xFFFF) as u16,
            lo_base: (base & 0xFFFF) as u16,
            mi_base: ((base >> 16) & 0xFF) as u8,
            access,
            gran: (((lim >> 16) & 0x0F) as u8) | (gran & 0xF0),
            hi_base: ((base >> 24) & 0xFF) as u8,
        }
    }
}
#[repr(C, packed)]
struct GdtTable { entries: [GdtE; 6], tss_hi: u64 }
#[repr(C, packed)]
struct GdtPtr { limit: u16, base: u64 }

static mut GDT:     GdtTable = GdtTable { entries: [GdtE::null(); 6], tss_hi: 0 };
static mut GDT_PTR: GdtPtr   = GdtPtr { limit: 0, base: 0 };

unsafe fn init_gdt() {
    // Ustaw IST1 na dedykowany stos double fault
    TSS.ist1 = DF_STACK.as_ptr() as u64 + DOUBLE_FAULT_STACK_SIZE as u64;

    let tss_base  = &TSS as *const Tss as u64;
    let tss_limit = (core::mem::size_of::<Tss>() - 1) as u64;

    GDT.entries[0] = GdtE::null();
    GDT.entries[1] = GdtE::new(0, 0xFFFFF, 0x9A, 0x20); // kernel code 64-bit
    GDT.entries[2] = GdtE::new(0, 0xFFFFF, 0x92, 0x00); // kernel data
    GDT.entries[3] = GdtE::new(0, 0xFFFFF, 0xFA, 0x20); // user code  DPL=3
    GDT.entries[4] = GdtE::new(0, 0xFFFFF, 0xF2, 0x00); // user data  DPL=3
    // TSS: 0x89 = present, DPL=0, type=9 (64-bit available TSS)
    GDT.entries[5] = GdtE::new(tss_base, tss_limit, 0x89, 0x00);
    GDT.tss_hi = tss_base >> 32;

    GDT_PTR.limit = (core::mem::size_of::<GdtTable>() - 1) as u16;
    GDT_PTR.base  = &GDT as *const GdtTable as u64;

    asm!("lgdt [{}]", in(reg) &GDT_PTR, options(preserves_flags));
    asm!(
        "pushq $0x08",
        "lea 1f(%rip), %rax", "pushq %rax", "lretq", "1:",
        "mov $0x10, %ax",
        "mov %ax, %ds", "mov %ax, %es", "mov %ax, %fs", "mov %ax, %gs", "mov %ax, %ss",
        out("rax") _, options(preserves_flags)
    );
    // Załaduj TSS — selektor 0x28
    asm!("ltr ax", in("ax") 0x28u16, options(nostack, preserves_flags));
    print("[GDT] GDT + TSS loaded\n");
}

// ============================================================================
// IDT
// ============================================================================
#[repr(C, packed)] #[derive(Clone, Copy)]
struct IdtE { off_lo: u16, sel: u16, ist: u8, attr: u8, off_mi: u16, off_hi: u32, _z: u32 }
impl IdtE {
    const fn null() -> Self { Self{off_lo:0,sel:0,ist:0,attr:0,off_mi:0,off_hi:0,_z:0} }
    fn new(handler: u64, sel: u16, dpl: u8, ist: u8) -> Self {
        Self {
            off_lo: (handler & 0xFFFF) as u16,
            off_mi: ((handler >> 16) & 0xFFFF) as u16,
            off_hi: ((handler >> 32) & 0xFFFFFFFF) as u32,
            sel, ist, attr: 0x8E | (dpl << 5), _z: 0,
        }
    }
}
#[repr(C, packed)] struct Idtr { limit: u16, base: u64 }

const IDT_LEN: usize = 256;
static mut IDT:   [IdtE; IDT_LEN] = [IdtE::null(); IDT_LEN];
static mut IDTR:  Idtr             = Idtr { limit: 0, base: 0 };

unsafe fn init_idt() {
    // Wyjątki
    IDT[0x08] = IdtE::new(isr_double_fault as u64, 0x08, 0, 1); // IST=1 → DF_STACK
    IDT[0x0E] = IdtE::new(isr_page_fault   as u64, 0x08, 0, 0);
    // Przerwania sprzętowe
    IDT[0x20] = IdtE::new(isr_timer        as u64, 0x08, 0, 0);
    // Syscall — DPL=3
    IDT[0x80] = IdtE::new(isr_syscall      as u64, 0x08, 3, 0);

    IDTR.limit = (core::mem::size_of_val(&IDT) - 1) as u16;
    IDTR.base  = IDT.as_ptr() as u64;
    asm!("lidt [{}]", in(reg) &IDTR, options(preserves_flags));
    asm!("sti", options(nomem, nostack));
    print("[IDT] IDT loaded\n");
}

// ============================================================================
// PIC + PIT
// ============================================================================
unsafe fn init_pic() {
    outb(0x20,0x11); io_wait(); outb(0xA0,0x11); io_wait();
    outb(0x21,0x20); io_wait(); outb(0xA1,0x28); io_wait();
    outb(0x21,0x04); io_wait(); outb(0xA1,0x02); io_wait();
    outb(0x21,0x01); io_wait(); outb(0xA1,0x01); io_wait();
    outb(0x21,0xFE); // IRQ0 (timer) odblokowany
    outb(0xA1,0xFF);
    print("[PIC] Initialized\n");
}
unsafe fn init_pit() {
    let d: u16 = (1193180u32 / 100) as u16;
    outb(0x43, 0x36); outb(0x40, (d & 0xFF) as u8); outb(0x40, (d >> 8) as u8);
    print("[PIT] 100 Hz\n");
}

// ============================================================================
// ISR MAKRA — budują pełny TrapFrame, wywołują Rust handler
// ============================================================================

/// ISR bez error code. Handler: fn(frame: *mut TrapFrame)
macro_rules! isr_no_err {
    ($name:ident, $handler:expr) => {
        #[naked]
        unsafe extern "C" fn $name() {
            asm!(
                "pushq %rax", "pushq %rbp", "pushq %rbx",
                "pushq %rcx", "pushq %rdx", "pushq %rsi", "pushq %rdi",
                "pushq %r8",  "pushq %r9",  "pushq %r10", "pushq %r11",
                "pushq %r12", "pushq %r13", "pushq %r14", "pushq %r15",
                "mov %rsp, %rdi",  // arg1: *mut TrapFrame
                "call {f}",
                "popq %r15", "popq %r14", "popq %r13", "popq %r12",
                "popq %r11", "popq %r10", "popq %r9",  "popq %r8",
                "popq %rdi", "popq %rsi", "popq %rdx", "popq %rcx",
                "popq %rbx", "popq %rbp", "popq %rax",
                "iretq",
                f = sym $handler,
                options(noreturn)
            );
        }
    };
}

/// ISR z error code. CPU odkłada error_code przed RIP.
/// Zamieniamy error_code ↔ rax przez xchg, dzięki czemu error_code trafia do TrapFrame.rax.
macro_rules! isr_with_err {
    ($name:ident, $handler:expr) => {
        #[naked]
        unsafe extern "C" fn $name() {
            asm!(
                // error_code jest na szczycie stosu (CPU odłożyło przed RIP)
                "xchgq %rax, (%rsp)", // rax ← error_code; (%rsp) ← stary rax
                "pushq %rbp", "pushq %rbx",
                "pushq %rcx", "pushq %rdx", "pushq %rsi", "pushq %rdi",
                "pushq %r8",  "pushq %r9",  "pushq %r10", "pushq %r11",
                "pushq %r12", "pushq %r13", "pushq %r14", "pushq %r15",
                // rax (= error_code) jest już na właściwym miejscu w TrapFrame
                "mov %rsp, %rdi",  // arg1: *mut TrapFrame
                "call {f}",
                "popq %r15", "popq %r14", "popq %r13", "popq %r12",
                "popq %r11", "popq %r10", "popq %r9",  "popq %r8",
                "popq %rdi", "popq %rsi", "popq %rdx", "popq %rcx",
                "popq %rbx", "popq %rbp",
                "addq $8, %rsp",  // usuń error_code (było zamienione z rax)
                "iretq",
                f = sym $handler,
                options(noreturn)
            );
        }
    };
}

// ============================================================================
// KONKRETNE ISR
// ============================================================================

// #DF — IST1, nigdy nie wraca
#[naked]
unsafe extern "C" fn isr_double_fault() {
    asm!(
        "cli",
        "mov %rsp, %rdi",
        "call {f}",
        "cli", "hlt",
        f = sym handle_df,
        options(noreturn)
    );
}
#[no_mangle]
unsafe extern "C" fn handle_df(_frame: *mut TrapFrame) {
    print_raw("\n[#DF] Double fault — system halted\n");
    loop { asm!("hlt", options(nomem, nostack)); }
}

// Page fault — z error code (TrapFrame.rax = error_code)
isr_with_err!(isr_page_fault, handle_pf);
#[no_mangle]
unsafe extern "C" fn handle_pf(frame: *mut TrapFrame) {
    let tf    = &*frame;
    let error = tf.rax;
    let addr: u64;
    asm!("mov {}, cr2", out(reg) addr, options(nomem, nostack));

    let mut b1=[0u8;18]; let mut b2=[0u8;18];
    print("[PF] addr=0x"); print(u64_hex(addr, &mut b1));
    print(" err=0x");      print(u64_hex(error, &mut b2));
    print(if error&1!=0{" P"} else{" -"});
    print(if error&2!=0{"W"} else{"-"});
    print(if error&4!=0{"U\n"} else{"-\n"});

    if error & 1 == 0 && SWAP_ENABLED {
        let p4 = THREADS[CURRENT_THREAD].cr3;
        if mm_swap_in(p4, addr & !(PAGE_SIZE as u64 - 1), PTE_WRITABLE | PTE_USER).is_some() {
            return;
        }
    }
    panic_no_dyn("Unhandled page fault");
}

// Timer — pełny TrapFrame przez makro, wywołuje scheduler
isr_no_err!(isr_timer, handle_timer);
#[no_mangle]
unsafe extern "C" fn handle_timer(_frame: *mut TrapFrame) {
    outb(0x20, 0x20); // EOI do PIC master
    schedule();
}

// Syscall int 0x80
isr_no_err!(isr_syscall, handle_syscall);

// ============================================================================
// SYSCALL DISPATCH
// ============================================================================
const SYS_EXIT:  u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ:  u64 = 2;

#[no_mangle]
unsafe extern "C" fn handle_syscall(frame: *mut TrapFrame) {
    let tf  = &mut *frame;
    let num = tf.rax;
    let a1  = tf.rdi;
    let a2  = tf.rsi;
    let a3  = tf.rdx;
    let p4  = THREADS[CURRENT_THREAD].cr3;

    let ret: u64 = match num {
        SYS_WRITE => {
            if a1 == 1 || a1 == 2 {
                // Walidacja: present + user bit dla każdej strony zakresu
                if !validate_user_buf(p4, a2, a3 as usize) {
                    !0u64 // EFAULT
                } else {
                    let ptr = a2 as *const u8;
                    VGA_LOCK.lock();
                    for i in 0..a3 as usize { putchar_raw(*ptr.add(i) as char); }
                    VGA_LOCK.unlock();
                    a3
                }
            } else { 0 }
        }
        SYS_READ  => 0, // EOF (TODO: klawiatura)
        SYS_EXIT  => {
            let mut b=[0u8;20];
            print("\n[EXIT "); print(usize_str(a1 as usize, &mut b)); print("]\n");
            THREADS[CURRENT_THREAD].state = ThreadState::Terminated;
            THREAD_COUNT.fetch_sub(1, Ordering::Relaxed);
            schedule();
            0
        }
        _ => !0u64, // ENOSYS
    };
    tf.rax = ret; // wartość zwrotna w rax
}

// ============================================================================
// THREADING
// ============================================================================
#[derive(Clone, Copy, PartialEq)]
pub enum ThreadState { Running, Ready, Blocked, Terminated }

#[repr(C)]
pub struct Thread {
    pub id:               u32,
    pub state:            ThreadState,
    pub priority:         u8,
    pub kernel_rsp:       VirtAddr,    // zapisany RSP przy przełączeniu
    pub kernel_stack_top: VirtAddr,    // rsp0 dla TSS
    pub user_stack_top:   VirtAddr,
    pub cr3:              PhysAddr,
    pub name:             [u8; 16],
}
impl Thread {
    pub const fn new() -> Self {
        Self { id:0, state:ThreadState::Terminated, priority:10,
               kernel_rsp:0, kernel_stack_top:0, user_stack_top:0, cr3:0, name:[0;16] }
    }
}

static mut THREADS:        [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS];
static mut CURRENT_THREAD: usize                  = 0;
static     THREAD_COUNT:   AtomicUsize            = AtomicUsize::new(0);

pub unsafe fn thread_init() {
    print("[THREAD] Init\n");
    let tid = thread_create_kernel("idle\0", kernel_idle as VirtAddr, 0);
    if tid >= 0 {
        THREADS[tid as usize].state = ThreadState::Running;
        CURRENT_THREAD = tid as usize;
    }
}

pub unsafe fn thread_create_kernel(name: &str, entry: VirtAddr, arg: u64) -> i32 {
    thread_create_impl(name, entry, arg, false)
}
pub unsafe fn thread_create_user(name: &str, entry: VirtAddr, arg: u64) -> i32 {
    thread_create_impl(name, entry, arg, true)
}

unsafe fn thread_create_impl(name: &str, entry: VirtAddr, arg: u64, user: bool) -> i32 {
    for i in 0..MAX_THREADS {
        if THREADS[i].state != ThreadState::Terminated { continue; }
        let t = &mut THREADS[i];

        // ── Stos kernelowy + guard page ─────────────────────────────────────
        // Układ: [ guard_page(unmapped) | KERNEL_STACK_SIZE ]
        let k_base  = 0xFFFF_9000_0000_0000u64
            + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64;
        // k_base+0 = guard page — celowo nie mapujemy
        let k_start = k_base + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            let phys = mm_alloc_frame();
            let virt = k_start + p as u64 * PAGE_SIZE as u64;
            mm_map_page(KERNEL_P4_PHYS, virt, phys, PTE_WRITABLE); // kernel-only
        }
        let k_top = k_start + KERNEL_STACK_SIZE as u64;

        // ── Stos użytkownika + guard page (tylko user thread) ───────────────
        let (u_top, cr3) = if user {
            let new_cr3 = create_user_p4(); // kopiuje górne 256 wpisów kernelowego P4
            let u_base  = 0x0000_7FFF_0000_0000u64
                - i as u64 * (USER_STACK_SIZE + PAGE_SIZE) as u64;
            // u_base+0 = guard page
            let u_start = u_base + PAGE_SIZE as u64;
            for p in 0..(USER_STACK_SIZE / PAGE_SIZE) {
                let phys = mm_alloc_frame();
                let virt = u_start + p as u64 * PAGE_SIZE as u64;
                mm_map_page(new_cr3, virt, phys, PTE_WRITABLE | PTE_USER);
            }
            (u_start + USER_STACK_SIZE as u64, new_cr3)
        } else {
            (k_top, KERNEL_P4_PHYS)
        };

        t.id               = i as u32;
        t.state            = ThreadState::Ready;
        t.priority         = if user { 5 } else { 10 };
        t.kernel_stack_top = k_top;
        t.user_stack_top   = u_top;
        t.cr3              = cr3;

        // ── Inicjalny stos dla thread_switch ────────────────────────────────
        // thread_switch odkłada/przywraca: rbx, rbp, r12, r13, r14, r15 + ret addr.
        // Trampoline pobierze: r15=arg, r14=entry, r13=user_rsp.
        let mut ksp = k_top;
        macro_rules! push { ($v:expr) => { ksp -= 8; *(ksp as *mut u64) = $v as u64; } }
        push!(if user { trampoline_user as u64 } else { trampoline_kernel as u64 });
        push!(0u64);  // rbx
        push!(0u64);  // rbp
        push!(0u64);  // r12
        push!(u_top); // r13 = user RSP (dla trampoline_user)
        push!(entry); // r14 = entry point
        push!(arg);   // r15 = arg → rdi
        t.kernel_rsp = ksp;

        let b = name.as_bytes();
        for j in 0..core::cmp::min(15, b.len()) { t.name[j] = b[j]; }
        THREAD_COUNT.fetch_add(1, Ordering::Relaxed);

        let mut buf=[0u8;20];
        print("[THREAD] Created #"); print(usize_str(i, &mut buf));
        print(": "); print(name); print("\n");
        return i as i32;
    }
    -1
}

/// Trampoline dla wątków kernelowych.
/// Po thread_switch: r15=arg, r14=entry.
#[naked]
unsafe extern "C" fn trampoline_kernel() -> ! {
    asm!(
        "mov rdi, r15",  // arg → rdi (SysV ABI arg1)
        "call r14",      // call entry(arg)
        "cli", "hlt",    // wątek nie powinien wrócić
        options(noreturn)
    );
}

/// Trampoline dla wątków użytkownika.
/// Po thread_switch: r15=arg, r14=entry (user RIP), r13=user RSP.
/// Buduje iretq frame i skacze do ring3.
#[naked]
unsafe extern "C" fn trampoline_user() -> ! {
    asm!(
        "push 0x20 | 3",  // SS  = user data | RPL=3
        "push r13",        // RSP = user stack top
        "push 0x202",      // RFLAGS: IF=1 + reserved
        "push 0x18 | 3",  // CS  = user code | RPL=3
        "push r14",        // RIP = entry point
        "mov rdi, r15",    // arg → rdi
        "iretq",
        options(noreturn)
    );
}

/// Round-robin scheduler.
pub unsafe fn schedule() {
    let start = CURRENT_THREAD;
    let mut next = start;
    for _ in 0..MAX_THREADS {
        next = (next + 1) % MAX_THREADS;
        if THREADS[next].state == ThreadState::Ready { break; }
    }
    if next == start && THREADS[start].state == ThreadState::Running { return; }

    if THREADS[start].state == ThreadState::Running {
        THREADS[start].state = ThreadState::Ready;
    }
    THREADS[next].state = ThreadState::Running;
    CURRENT_THREAD = next;

    // Aktualizuj rsp0 w TSS (kernel stack nowego wątku)
    tss_set_rsp0(THREADS[next].kernel_stack_top);

    // Przełącz CR3 jeśli inna przestrzeń adresowa
    let new_cr3 = THREADS[next].cr3;
    let cur_cr3: u64;
    asm!("mov {}, cr3", out(reg) cur_cr3, options(nomem, nostack));
    if new_cr3 != cur_cr3 {
        asm!("mov cr3, {}", in(reg) new_cr3, options(nostack));
    }

    thread_switch(
        &mut THREADS[start].kernel_rsp as *mut VirtAddr,
        THREADS[next].kernel_rsp,
    );
}

/// Przełącznik kontekstu — callee-saved przez stos, RIP przez ret.
#[naked]
unsafe extern "C" fn thread_switch(old_rsp_out: *mut VirtAddr, new_rsp: VirtAddr) {
    asm!(
        "pushq %rbx", "pushq %rbp",
        "pushq %r12", "pushq %r13", "pushq %r14", "pushq %r15",
        "movq %rsp, (%rdi)",  // *old_rsp_out = rsp
        "movq %rsi, %rsp",    // rsp = new_rsp
        "popq %r15", "popq %r14", "popq %r13", "popq %r12",
        "popq %rbp", "popq %rbx",
        "ret",
        options(noreturn)
    );
}

unsafe extern "C" fn kernel_idle(_: u64) -> ! {
    loop { asm!("hlt", options(nomem, nostack)); }
}

// ============================================================================
// HELPERS — bez static mut BUF
// ============================================================================
pub fn usize_str<'a>(mut v: usize, buf: &'a mut [u8; 20]) -> &'a str {
    if v == 0 { buf[19] = b'0'; return unsafe { core::str::from_utf8_unchecked(&buf[19..]) }; }
    let mut i = 19usize;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if i == 0 { break; } else { i -= 1; }
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i+1..]) }
}

pub fn u64_hex<'a>(mut v: u64, buf: &'a mut [u8; 18]) -> &'a str {
    const H: &[u8] = b"0123456789ABCDEF";
    buf[0] = b'0'; buf[1] = b'x';
    for i in (2..18).rev() { buf[i] = H[(v & 0xF) as usize]; v >>= 4; }
    unsafe { core::str::from_utf8_unchecked(buf) }
}

// ============================================================================
// PANIC PATH — bez schedulera, locków, dynamicznych alokacji
// ============================================================================
fn panic_no_dyn(msg: &str) -> ! {
    unsafe {
        asm!("cli", options(nomem, nostack));
        print_raw("\n[KERNEL PANIC] ");
        print_raw(msg);
        print_raw("\n");
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe { asm!("cli", options(nomem, nostack)); }
    unsafe { print_raw("\n[PANIC] "); }
    if let Some(m) = info.message() {
        match m.as_str() {
            Some(s) => unsafe { print_raw(s); },
            None    => unsafe { print_raw("(fmt args)"); },
        }
    }
    if let Some(l) = info.location() {
        unsafe { print_raw(" @ "); print_raw(l.file()); }
    }
    unsafe { print_raw("\n"); }
    loop { unsafe { asm!("hlt", options(nomem, nostack)); } }
}

// ============================================================================
// KERNEL MAIN
// ============================================================================
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    unsafe {
        clear_screen();
        serial_init();
        serial_print("=== CosinusOS Microkernel v3.1 ===\n");
        print("CosinusOS Microkernel v3.1\n");
        print("==========================\n\n");

        mm_init(0x100000, 0x700000);
        mm_init_paging(0x1000);
        mm_swap_init();

        init_gdt(); // ładuje też TSS z IST1 ustawionym na DF_STACK
        init_pic();
        init_idt();
        init_pit();
        thread_init();

        print("\n[OK] System ready. Creating test threads...\n");
        for i in 0..3u64 {
            thread_create_kernel("worker\0", test_thread as VirtAddr, i);
        }

        schedule();
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}

unsafe extern "C" fn test_thread(arg: u64) -> ! {
    let mut b = [0u8; 20];
    print("[T"); print(usize_str(arg as usize, &mut b)); print("] started\n");
    loop {
        for _ in 0..1_000_000u64 { core::hint::spin_loop(); }
        print("[T"); print(usize_str(arg as usize, &mut b)); print("] tick\n");
    }
}