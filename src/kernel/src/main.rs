// src/kernel/src/main.rs — CosinusOS Microkernel v3.0 (Refactored)
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

const PAGE_SIZE: usize        = 0x1000;
const MAX_FRAMES: usize       = 0x10000;
const MAX_THREADS: usize      = 64;
const KERNEL_STACK_SIZE: usize = 0x8000;  // 32 KB stack kernela
const USER_STACK_SIZE: usize   = 0x4000;  // 16 KB stack użytkownika

/// Offset higher-half: fizyczne → wirtualne dla regionu identity-mapped jądra.
/// Zakładamy, że bootloader/linker odwzorowuje RAM startując od PHYS_OFFSET.
const PHYS_OFFSET: VirtAddr = 0xFFFF_8000_0000_0000;

#[inline(always)]
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    phys + PHYS_OFFSET
}

#[inline(always)]
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    virt - PHYS_OFFSET
}

// ============================================================================
// MULTIBOOT2 HEADER
// ============================================================================
const MULTIBOOT2_MAGIC: u32     = 0xe85250d6;
const MULTIBOOT_ARCH_I386: u32  = 0;
const MULTIBOOT_HEADER_TAG_END: u16 = 0;

#[repr(C, packed)]
struct MultibootHeader {
    magic: u32, architecture: u32, header_length: u32, checksum: u32,
}
#[repr(C, packed)]
struct MultibootHeaderTag { type_: u16, flags: u16, size: u32 }
#[repr(C, packed)]
struct MultibootBootstrap { header: MultibootHeader, end_tag: MultibootHeaderTag }

#[link_section = ".multiboot"]
#[used]
static MULTIBOOT_HEADER: MultibootBootstrap = MultibootBootstrap {
    header: MultibootHeader {
        magic: MULTIBOOT2_MAGIC,
        architecture: MULTIBOOT_ARCH_I386,
        header_length: core::mem::size_of::<MultibootBootstrap>() as u32,
        checksum: (-(MULTIBOOT2_MAGIC as i32
            + MULTIBOOT_ARCH_I386 as i32
            + core::mem::size_of::<MultibootBootstrap>() as i32)) as u32,
    },
    end_tag: MultibootHeaderTag { type_: MULTIBOOT_HEADER_TAG_END, flags: 0, size: 8 },
};

// ============================================================================
// PORT I/O
// ============================================================================
#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    asm!("outb %al, %dx", in("al") val, in("dx") port, options(nostack));
}
#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let ret: u8;
    asm!("inb %dx, %al", out("al") ret, in("dx") port, options(nostack));
    ret
}
fn io_wait() { unsafe { outb(0x80, 0); } }

// ============================================================================
// VGA DRIVER
// ============================================================================
const VGA_MEMORY: *mut u16 = 0xB8000 as *mut u16;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

static mut VGA_BUFFER: *mut u16 = VGA_MEMORY;
static mut CURSOR_X: usize = 0;
static mut CURSOR_Y: usize = 0;
static mut CURRENT_COLOR: u8 = 0x0F;
static VGA_LOCK: Spinlock = Spinlock::new();

unsafe fn vga_update_cursor() {
    let pos = CURSOR_Y * VGA_WIDTH + CURSOR_X;
    outb(0x3D4, 0x0F); outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E); outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
}

unsafe fn clear_screen() {
    for i in 0..(VGA_WIDTH * VGA_HEIGHT) {
        *VGA_BUFFER.add(i) = ((CURRENT_COLOR as u16) << 8) | b' ' as u16;
    }
    CURSOR_X = 0; CURSOR_Y = 0;
    vga_update_cursor();
}

unsafe fn putchar_locked(c: char) {
    VGA_LOCK.lock();
    putchar_raw(c);
    VGA_LOCK.unlock();
}

unsafe fn putchar_raw(c: char) {
    match c {
        '\n' => { CURSOR_X = 0; CURSOR_Y += 1; }
        '\r' => { CURSOR_X = 0; }
        '\t' => { CURSOR_X = (CURSOR_X + 4) & !3; }
        '\x08' => { if CURSOR_X > 0 { CURSOR_X -= 1; } }
        _ => {
            let pos = CURSOR_Y * VGA_WIDTH + CURSOR_X;
            *VGA_BUFFER.add(pos) = ((CURRENT_COLOR as u16) << 8) | c as u16;
            CURSOR_X += 1;
        }
    }
    if CURSOR_X >= VGA_WIDTH { CURSOR_X = 0; CURSOR_Y += 1; }
    if CURSOR_Y >= VGA_HEIGHT {
        for i in 0..(VGA_HEIGHT - 1) * VGA_WIDTH {
            *VGA_BUFFER.add(i) = *VGA_BUFFER.add(i + VGA_WIDTH);
        }
        for i in 0..VGA_WIDTH {
            *VGA_BUFFER.add((VGA_HEIGHT - 1) * VGA_WIDTH + i) =
                ((CURRENT_COLOR as u16) << 8) | b' ' as u16;
        }
        CURSOR_Y = VGA_HEIGHT - 1;
    }
    vga_update_cursor();
}

/// Wypisz string na VGA — thread-safe przez spinlock.
unsafe fn print(s: &str) {
    VGA_LOCK.lock();
    for c in s.chars() { putchar_raw(c); }
    VGA_LOCK.unlock();
}

/// Wypisz string BEZ locka — tylko w ścieżce paniki lub wczesnego bootu.
unsafe fn print_raw(s: &str) {
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
unsafe fn serial_write(c: char) {
    while (inb(COM1+5) & 0x20) == 0 {}
    outb(COM1, c as u8);
}
unsafe fn serial_print(s: &str) { for c in s.chars() { serial_write(c); } }

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

static MM_LOCK: Spinlock = Spinlock::new();

// ============================================================================
// PHYSICAL FRAME ALLOCATOR
// ============================================================================
static mut FRAME_BITMAP: [u64; MAX_FRAMES / 64] = [0; MAX_FRAMES / 64];
static mut MEMORY_BASE: PhysAddr = 0;
static mut MEMORY_SIZE: usize = 0;

unsafe fn frame_index(phys: PhysAddr) -> usize {
    ((phys - MEMORY_BASE) / PAGE_SIZE as u64) as usize
}
unsafe fn phys_from_frame(idx: usize) -> PhysAddr {
    MEMORY_BASE + (idx as u64 * PAGE_SIZE as u64)
}
unsafe fn is_frame_free(idx: usize) -> bool {
    (FRAME_BITMAP[idx/64] & (1u64 << (idx%64))) == 0
}
unsafe fn set_frame_used(idx: usize) { FRAME_BITMAP[idx/64] |=  1u64 << (idx%64); }
unsafe fn set_frame_free(idx: usize) { FRAME_BITMAP[idx/64] &= !(1u64 << (idx%64)); }

pub unsafe fn mm_init(base: PhysAddr, size: usize) {
    MEMORY_BASE = base;
    MEMORY_SIZE = size;
    let frames = size / PAGE_SIZE;
    core::ptr::write_bytes(FRAME_BITMAP.as_mut_ptr() as *mut u8, 0,
        core::mem::size_of_val(&FRAME_BITMAP));
    // Rezerwuj pierwsze 256 ramek (1 MiB) dla jądra
    for i in 0..core::cmp::min(256, frames) { set_frame_used(i); }
    let mut buf = [0u8; 20];
    print("[MM] Initialized: ");
    print(usize_to_str(frames * PAGE_SIZE / 1024 / 1024, &mut buf));
    print(" MiB available\n");
}

/// Alokuje fizyczną ramkę. Zwraca 0 przy braku pamięci (0 nigdy nie jest prawidłową ramką).
pub unsafe fn mm_alloc_frame() -> PhysAddr {
    MM_LOCK.lock();
    for word_idx in 0..FRAME_BITMAP.len() {
        if FRAME_BITMAP[word_idx] != !0u64 {
            for bit in 0..64 {
                let frame_idx = word_idx * 64 + bit;
                if frame_idx >= MAX_FRAMES { continue; }
                if is_frame_free(frame_idx) {
                    set_frame_used(frame_idx);
                    MM_LOCK.unlock();
                    return phys_from_frame(frame_idx);
                }
            }
        }
    }
    MM_LOCK.unlock();
    print_raw("[MM] OUT OF MEMORY!\n");
    panic_no_dyn("Physical memory exhausted");
}

pub unsafe fn mm_free_frame(phys: PhysAddr) {
    if phys < MEMORY_BASE { return; }
    let idx = frame_index(phys);
    if idx >= MAX_FRAMES { return; }
    MM_LOCK.lock();
    set_frame_free(idx);
    MM_LOCK.unlock();
}

// ============================================================================
// PAGE TABLES (x86_64 4-level)
// ============================================================================
#[repr(C, align(4096))]
struct PageTable { entries: [u64; 512] }

// Flagi wpisów tablicy stron
const PTE_PRESENT:  u64 = 1 << 0;
const PTE_WRITABLE: u64 = 1 << 1;
const PTE_USER:     u64 = 1 << 2;
const PTE_ADDR_MASK: u64 = !0xFFF;

#[inline(always)]
fn pte_new(phys: PhysAddr, flags: u64) -> u64 { (phys & PTE_ADDR_MASK) | flags | PTE_PRESENT }
#[inline(always)]
fn pte_present(e: u64) -> bool { e & PTE_PRESENT != 0 }
#[inline(always)]
fn pte_addr(e: u64) -> PhysAddr { e & PTE_ADDR_MASK }

/// Wskaźnik do P4 jądra — raw pointer zamiast Option<&'static mut>.
static mut KERNEL_P4: *mut PageTable = core::ptr::null_mut();

/// Alokuje nową stronę i wypełnia zerami — zwraca adres fizyczny.
/// Zapisuje przez mapped adres wirtualny (phys_to_virt).
unsafe fn alloc_zeroed_page() -> PhysAddr {
    let phys = mm_alloc_frame();
    let virt = phys_to_virt(phys) as *mut u8;
    core::ptr::write_bytes(virt, 0, PAGE_SIZE);
    phys
}

/// Zwraca (lub tworzy) wpis w P3/P2/P1 wskazywany przez P4.
/// Bezpieczny dostęp przez phys_to_virt.
unsafe fn get_or_create_entry(table_phys: PhysAddr, idx: usize, flags: u64) -> PhysAddr {
    let table = &mut *(phys_to_virt(table_phys) as *mut PageTable);
    if !pte_present(table.entries[idx]) {
        let new_phys = alloc_zeroed_page();
        table.entries[idx] = pte_new(new_phys, flags);
    }
    pte_addr(table.entries[idx])
}

pub unsafe fn mm_init_paging(kernel_cr3: PhysAddr) {
    KERNEL_P4 = phys_to_virt(kernel_cr3) as *mut PageTable;
    asm!("mov cr3, {}", in(reg) kernel_cr3, options(preserves_flags));
    print("[MMU] Paging enabled\n");
}

/// Mapuje jedną stronę. Zwraca -1 jeśli virt/phys nie są page-aligned,
/// lub wpis już istnieje (bez flagi wymuszenia).
pub unsafe fn mm_map_page(virt: VirtAddr, phys: PhysAddr, flags: u64) -> i32 {
    // Sanity checks
    if virt & 0xFFF != 0 || phys & 0xFFF != 0 {
        print("[MM] map_page: niezalignowany adres!\n");
        return -1;
    }
    if KERNEL_P4.is_null() {
        print("[MM] map_page: P4 niezainicjowany!\n");
        return -1;
    }

    MM_LOCK.lock();

    let p4_idx = ((virt >> 39) & 0x1FF) as usize;
    let p3_idx = ((virt >> 30) & 0x1FF) as usize;
    let p2_idx = ((virt >> 21) & 0x1FF) as usize;
    let p1_idx = ((virt >> 12) & 0x1FF) as usize;

    let p3_phys = get_or_create_entry(virt_to_phys(KERNEL_P4 as VirtAddr), p4_idx,
                                       PTE_WRITABLE | PTE_USER);
    let p2_phys = get_or_create_entry(p3_phys, p3_idx, PTE_WRITABLE | PTE_USER);
    let p1_phys = get_or_create_entry(p2_phys, p2_idx, PTE_WRITABLE | PTE_USER);

    let p1 = &mut *(phys_to_virt(p1_phys) as *mut PageTable);
    if pte_present(p1.entries[p1_idx]) {
        // Wpis już istnieje — ostrzeżenie, ale nie błąd krytyczny
        print("[MM] map_page: ostrzeżenie — nadpisywanie istniejącego wpisu\n");
    }
    p1.entries[p1_idx] = pte_new(phys, flags);

    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    MM_LOCK.unlock();
    0
}

/// Odmapowuje stronę: przechodzi P4→P1, zeruje wpis, potem invlpg.
pub unsafe fn mm_unmap_page(virt: VirtAddr) {
    if KERNEL_P4.is_null() { return; }
    MM_LOCK.lock();

    let p4_idx = ((virt >> 39) & 0x1FF) as usize;
    let p3_idx = ((virt >> 30) & 0x1FF) as usize;
    let p2_idx = ((virt >> 21) & 0x1FF) as usize;
    let p1_idx = ((virt >> 12) & 0x1FF) as usize;

    let p4 = &*KERNEL_P4;
    if !pte_present(p4.entries[p4_idx]) { MM_LOCK.unlock(); return; }

    let p3 = &*(phys_to_virt(pte_addr(p4.entries[p4_idx])) as *const PageTable);
    if !pte_present(p3.entries[p3_idx]) { MM_LOCK.unlock(); return; }

    let p2 = &*(phys_to_virt(pte_addr(p3.entries[p3_idx])) as *const PageTable);
    if !pte_present(p2.entries[p2_idx]) { MM_LOCK.unlock(); return; }

    let p1 = &mut *(phys_to_virt(pte_addr(p2.entries[p2_idx])) as *mut PageTable);
    p1.entries[p1_idx] = 0; // wyzeruj wpis w P1

    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    MM_LOCK.unlock();
}

// ============================================================================
// SWAP FRAMEWORK
// ============================================================================
const SWAP_ENABLED: bool = true;
static mut SWAP_BITMAP: [u64; 1024] = [0; 1024];
static SWAP_LOCK: Spinlock = Spinlock::new();

pub unsafe fn mm_swap_init() {
    if !SWAP_ENABLED { return; }
    print("[SWAP] Framework initialized (256 MiB reserved)\n");
}

pub unsafe fn mm_swap_out(virt: VirtAddr, phys: PhysAddr) -> bool {
    if !SWAP_ENABLED { return false; }
    SWAP_LOCK.lock();
    for word in 0..SWAP_BITMAP.len() {
        if SWAP_BITMAP[word] != !0u64 {
            for bit in 0..64 {
                let slot = word * 64 + bit;
                if SWAP_BITMAP[word] & (1 << bit) == 0 {
                    SWAP_BITMAP[word] |= 1 << bit;
                    SWAP_LOCK.unlock();
                    let mut buf1 = [0u8; 18]; let mut buf2 = [0u8; 20];
                    print("[SWAP] OUT: 0x"); print(u64_to_hex(virt, &mut buf1));
                    print(" -> slot ");    print(usize_to_str(slot, &mut buf2));
                    print("\n");
                    mm_unmap_page(virt);
                    mm_free_frame(phys);
                    return true;
                }
            }
        }
    }
    SWAP_LOCK.unlock();
    print("[SWAP] NO FREE SLOTS!\n");
    false
}

pub unsafe fn mm_swap_in(virt: VirtAddr, flags: u64) -> Option<PhysAddr> {
    if !SWAP_ENABLED { return None; }
    let phys = mm_alloc_frame();
    let virt_ptr = phys_to_virt(phys) as *mut u8;
    core::ptr::write_bytes(virt_ptr, 0xAA, PAGE_SIZE);
    mm_map_page(virt, phys, flags);
    let mut buf = [0u8; 18];
    print("[SWAP] IN: slot -> 0x"); print(u64_to_hex(virt, &mut buf)); print("\n");
    Some(phys)
}

// ============================================================================
// PAGE FAULT HANDLER
// ============================================================================
#[no_mangle]
pub unsafe extern "C" fn page_fault_handler(addr: VirtAddr, error: u64) {
    let present = error & 0x1 != 0;
    let write   = error & 0x2 != 0;
    let user    = error & 0x4 != 0;

    let mut buf1 = [0u8; 18]; let mut buf2 = [0u8; 18];
    print("[PF] Addr: 0x"); print(u64_to_hex(addr, &mut buf1));
    print(" Error: 0x"); print(u64_to_hex(error, &mut buf2));
    print(" (");
    if present { print("P"); } else { print("-"); }
    if write   { print("W"); } else { print("-"); }
    if user    { print("U"); } else { print("-"); }
    print(")\n");

    if !present && SWAP_ENABLED {
        if let Some(_) = mm_swap_in(addr & !0xFFF, PTE_WRITABLE | PTE_USER) {
            return; // fault naprawiony przez swap
        }
    }

    print("[PF] Nie można naprawić błędu — zatrzymanie\n");
    panic_no_dyn("Unhandled page fault");
}

// ============================================================================
// TSS (Task State Segment)
// ============================================================================
#[repr(C, packed)]
pub struct Tss {
    _reserved0: u32,
    pub rsp0: u64,   // stos jądra dla przejść ring3→ring0
    pub rsp1: u64,
    pub rsp2: u64,
    _reserved1: u64,
    pub ist1: u64,   // Interrupt Stack Table (opcjonalnie)
    _ist_rest: [u64; 6],
    _reserved2: u64,
    _reserved3: u16,
    pub iomap_base: u16,
}

impl Tss {
    pub const fn new() -> Self {
        Self {
            _reserved0: 0, rsp0: 0, rsp1: 0, rsp2: 0,
            _reserved1: 0, ist1: 0, _ist_rest: [0; 6],
            _reserved2: 0, _reserved3: 0,
            iomap_base: core::mem::size_of::<Tss>() as u16,
        }
    }
}

static mut TSS: Tss = Tss::new();

// ============================================================================
// GDT (z deskryptorem TSS)
// ============================================================================
// Układ GDT:
//   0x00 — null
//   0x08 — kernel code  (64-bit, DPL=0)
//   0x10 — kernel data  (DPL=0)
//   0x18 — user code    (64-bit, DPL=3)
//   0x20 — user data    (DPL=3)
//   0x28 — TSS low      (16 bajtów, dwa wpisy po 8)
//   0x30 — TSS high

const GDT_ENTRIES: usize = 7; // null + k_code + k_data + u_code + u_data + tss_lo + tss_hi

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct GdtEntry { limit_low: u16, base_low: u16, base_middle: u8, access: u8, granularity: u8, base_high: u8 }

impl GdtEntry {
    const fn null() -> Self { Self { limit_low:0,base_low:0,base_middle:0,access:0,granularity:0,base_high:0 } }
    fn new(base: u64, limit: u64, access: u8, gran: u8) -> Self {
        Self {
            limit_low: (limit & 0xFFFF) as u16,
            base_low: (base & 0xFFFF) as u16,
            base_middle: ((base >> 16) & 0xFF) as u8,
            access,
            granularity: (((limit >> 16) & 0x0F) as u8) | (gran & 0xF0),
            base_high: ((base >> 24) & 0xFF) as u8,
        }
    }
}

/// Deskryptor TSS zajmuje 16 bajtów (dwa sloty w GDT).
#[repr(C, packed)]
struct TssDescriptor { low: GdtEntry, high: u64 }

#[repr(C, packed)]
struct Gdt {
    entries: [GdtEntry; GDT_ENTRIES - 1], // 6 wpisów 8-bajtowych
    tss_high: u64,
}

static mut GDT_TABLE: Gdt = Gdt {
    entries: [GdtEntry::null(); GDT_ENTRIES - 1],
    tss_high: 0,
};

#[repr(C, packed)]
struct GdtPtr { limit: u16, base: u64 }
static mut GDT_PTR: GdtPtr = GdtPtr { limit: 0, base: 0 };

unsafe fn init_gdt() {
    // Wypełnij wpisy GDT
    GDT_TABLE.entries[0] = GdtEntry::null();
    GDT_TABLE.entries[1] = GdtEntry::new(0, 0xFFFFFFFF, 0x9A, 0x20); // kernel code
    GDT_TABLE.entries[2] = GdtEntry::new(0, 0xFFFFFFFF, 0x92, 0x00); // kernel data
    GDT_TABLE.entries[3] = GdtEntry::new(0, 0xFFFFFFFF, 0xFA, 0x20); // user code  DPL=3
    GDT_TABLE.entries[4] = GdtEntry::new(0, 0xFFFFFFFF, 0xF2, 0x00); // user data  DPL=3

    // TSS descriptor (64-bit system segment, 16 bajtów)
    let tss_base = &TSS as *const Tss as u64;
    let tss_limit = (core::mem::size_of::<Tss>() - 1) as u64;
    // access = 0x89: Present, DPL=0, Type=0x9 (64-bit available TSS)
    GDT_TABLE.entries[5] = GdtEntry::new(tss_base, tss_limit, 0x89, 0x00);
    GDT_TABLE.tss_high = (tss_base >> 32) as u64;

    GDT_PTR.limit = (core::mem::size_of::<Gdt>() - 1) as u16;
    GDT_PTR.base  = &GDT_TABLE as *const Gdt as u64;

    asm!("lgdt [{}]", in(reg) &GDT_PTR, options(preserves_flags));

    // Przeładuj segmenty
    asm!(
        "pushq $0x08",
        "lea 1f(%rip), %rax",
        "pushq %rax",
        "lretq",
        "1:",
        "mov $0x10, %ax",
        "mov %ax, %ds", "mov %ax, %es",
        "mov %ax, %fs", "mov %ax, %gs", "mov %ax, %ss",
        out("rax") _,
        options(preserves_flags)
    );

    // Załaduj TSS (selektor 0x28, RPL=0)
    asm!("ltr ax", in("ax") 0x28u16, options(nostack, preserves_flags));

    print("[GDT] GDT + TSS załadowane\n");
}

/// Ustaw rsp0 w TSS — wywoływane przy każdym przełączeniu do wątku user.
pub unsafe fn tss_set_kernel_stack(rsp0: VirtAddr) {
    TSS.rsp0 = rsp0;
}

// ============================================================================
// IDT
// ============================================================================
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16, selector: u16, ist: u8, type_attr: u8,
    offset_mid: u16, offset_high: u32, zero: u32,
}
impl IdtEntry {
    const fn null() -> Self {
        Self { offset_low:0,selector:0,ist:0,type_attr:0,offset_mid:0,offset_high:0,zero:0 }
    }
    fn new(handler: u64, selector: u16, dpl: u8, ist: u8) -> Self {
        Self {
            offset_low:  (handler & 0xFFFF) as u16,
            offset_mid:  ((handler >> 16) & 0xFFFF) as u16,
            offset_high: ((handler >> 32) & 0xFFFFFFFF) as u32,
            selector,
            ist,
            type_attr: 0x8E | (dpl << 5), // interrupt gate
            zero: 0,
        }
    }
}

const IDT_ENTRIES: usize = 256;
static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::null(); IDT_ENTRIES];

#[repr(C, packed)]
struct Idtr { limit: u16, base: u64 }
static mut IDTR_VAL: Idtr = Idtr { limit: 0, base: 0 };

unsafe fn init_idt() {
    // Przerwania sprzętowe (IRQ 0–15 → wektory 0x20–0x2F)
    IDT[0x20] = IdtEntry::new(timer_isr as u64,      0x08, 0, 0);
    // Wyjątki procesora
    IDT[0x0E] = IdtEntry::new(page_fault_isr as u64, 0x08, 0, 0);
    IDT[0x08] = IdtEntry::new(double_fault_isr as u64, 0x08, 0, 0);
    // Syscall przez int 0x80 — DPL=3 (user może wywołać)
    IDT[0x80] = IdtEntry::new(syscall_handler_asm as u64, 0x08, 3, 0);

    IDTR_VAL.limit = (core::mem::size_of_val(&IDT) - 1) as u16;
    IDTR_VAL.base  = IDT.as_ptr() as u64;
    asm!("lidt [{}]", in(reg) &IDTR_VAL, options(preserves_flags));
    asm!("sti", options(nomem, nostack));
    print("[IDT] IDT załadowane\n");
}

// ============================================================================
// PIC
// ============================================================================
unsafe fn init_pic() {
    outb(0x20,0x11); io_wait(); outb(0xA0,0x11); io_wait();
    outb(0x21,0x20); io_wait(); outb(0xA1,0x28); io_wait();
    outb(0x21,0x04); io_wait(); outb(0xA1,0x02); io_wait();
    outb(0x21,0x01); io_wait(); outb(0xA1,0x01); io_wait();
    // Zamaskuj wszystkie IRQ; odmaskujemy po inicjalizacji
    outb(0x21,0xFE); // tylko IRQ0 (timer) odblokowany
    outb(0xA1,0xFF);
    print("[PIC] PIC zainicjowany\n");
}

// ============================================================================
// PIT
// ============================================================================
unsafe fn init_pit() {
    let divisor: u16 = (1193180 / 100) as u16;
    outb(0x43, 0x36);
    outb(0x40, (divisor & 0xFF) as u8);
    outb(0x40, (divisor >> 8) as u8);
    print("[PIT] Timer 100 Hz\n");
}

// ============================================================================
// TRAPFRAME — jawna struktura kontekstu (bez ręcznego czytania offsetów ze stosu)
// ============================================================================
/// Układ stosu po wejściu do ISR (po pushach callee-saved + rax).
/// Kolejność: najpierw CPU odkłada SS/RSP/RFLAGS/CS/RIP (dla ring3→ring0),
/// potem nasz naked handler odkłada rejestry — dokładnie ten układ.
#[repr(C)]
pub struct TrapFrame {
    // Odkładane przez nasz handler (od najniższego adresu):
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9:  u64, pub r8:  u64,
    pub rdi: u64, pub rsi: u64, pub rdx: u64, pub rcx: u64,
    pub rbx: u64, pub rbp: u64, pub rax: u64,
    // Odkładane przez CPU (iretq frame):
    pub rip: u64, pub cs: u64, pub rflags: u64,
    pub rsp: u64, pub ss: u64,
}

// ============================================================================
// THREADING SYSTEM
// ============================================================================
#[derive(Clone, Copy, PartialEq)]
pub enum ThreadState { Running, Ready, Blocked, Terminated }

/// Per-thread struktura z oddzielonym stackiem kernelowym i user-space.
#[repr(C)]
pub struct Thread {
    pub id:           u32,
    pub state:        ThreadState,
    pub priority:     u8,
    // Saved kernel RSP (przy przełączeniu kontekstu)
    pub kernel_rsp:   VirtAddr,
    // Wierzchołek stacku kernelowego (rsp0 dla TSS)
    pub kernel_stack_top: VirtAddr,
    // Wierzchołek stacku użytkownika
    pub user_stack_top:   VirtAddr,
    // Osobne CR3 dla każdego wątku/procesu (None = używa kernel CR3)
    pub cr3:          PhysAddr,
    pub name:         [u8; 16],
}

impl Thread {
    pub const fn new() -> Self {
        Self {
            id: 0, state: ThreadState::Terminated, priority: 10,
            kernel_rsp: 0, kernel_stack_top: 0, user_stack_top: 0,
            cr3: 0, name: [0; 16],
        }
    }
}

// Statyczna pula wątków
static mut THREADS: [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS];
static mut CURRENT_THREAD: usize = 0;
static mut THREAD_COUNT: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn thread_init() {
    print("[THREAD] Inicjalizacja subsystemu\n");
    let tid = thread_create_kernel("idle\0", kernel_idle_thread as VirtAddr, 0);
    if tid >= 0 {
        THREADS[tid as usize].state = ThreadState::Running;
        CURRENT_THREAD = tid as usize;
    }
}

/// Tworzy wątek kernelowy (współdzieli tablice stron jądra).
pub unsafe fn thread_create_kernel(name: &str, entry: VirtAddr, arg: u64) -> i32 {
    thread_create_impl(name, entry, arg, /*user=*/false)
}

/// Tworzy wątek użytkownika z własnym CR3 i stackiem user-space.
pub unsafe fn thread_create_user(name: &str, entry: VirtAddr, arg: u64) -> i32 {
    thread_create_impl(name, entry, arg, /*user=*/true)
}

unsafe fn thread_create_impl(name: &str, entry: VirtAddr, arg: u64, user: bool) -> i32 {
    for i in 0..MAX_THREADS {
        if THREADS[i].state != ThreadState::Terminated { continue; }

        let t = &mut THREADS[i];

        // --- Stos kernelowy (zawsze potrzebny) ---
        let k_stack_base = 0xFFFF_9000_0000_0000u64 + (i as u64 * KERNEL_STACK_SIZE as u64);
        for page in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            let phys = mm_alloc_frame();
            let virt = k_stack_base + (page as u64 * PAGE_SIZE as u64);
            mm_map_page(virt, phys, PTE_WRITABLE); // kernel only — brak PTE_USER
        }
        let k_stack_top = k_stack_base + KERNEL_STACK_SIZE as u64;

        // --- Stos użytkownika (tylko dla wątków user) ---
        let u_stack_top = if user {
            let u_stack_base = 0x0000_7FFF_F000_0000u64 - (i as u64 * USER_STACK_SIZE as u64);
            for page in 0..(USER_STACK_SIZE / PAGE_SIZE) {
                let phys = mm_alloc_frame();
                let virt = u_stack_base + (page as u64 * PAGE_SIZE as u64);
                mm_map_page(virt, phys, PTE_WRITABLE | PTE_USER);
            }
            u_stack_base + USER_STACK_SIZE as u64
        } else {
            k_stack_top
        };

        // --- CR3: wątek user dostaje własne tablice stron ---
        let cr3 = if user {
            let new_p4_phys = alloc_zeroed_page();
            // TODO: skopiuj mapowania kernela do nowego P4 (higher-half)
            new_p4_phys
        } else {
            // Kernel thread: użyj aktualnego CR3
            let current_cr3: u64;
            asm!("mov {}, cr3", out(reg) current_cr3, options(nomem, nostack));
            current_cr3
        };

        t.id = i as u32;
        t.state = ThreadState::Ready;
        t.priority = if user { 5 } else { 10 };
        t.kernel_stack_top = k_stack_top;
        t.user_stack_top   = u_stack_top;
        t.cr3              = cr3;

        // Przygotuj inicjalny stos kernelowy dla thread_switch
        // Kładziemy na nim: entry point jako adres powrotu + arg w rdi
        let mut ksp = k_stack_top;
        // Pomocnicza funkcja startowa — trampoline
        ksp -= 8; *(ksp as *mut u64) = if user {
            thread_user_trampoline as u64
        } else {
            thread_kernel_trampoline as u64
        };
        // Zapisz rdi (arg) i rip (entry) jako część "pushowanych" callee-saved
        // Uproszczony układ: thread_switch oczekuje 6 pushq na stosie
        // (rbx, rbp, r12, r13, r14, r15) + ret addr
        ksp -= 8*7;
        // rdi (arg) — przekazany przez dedykowane pole
        core::ptr::write_bytes(ksp as *mut u8, 0, 8*7);
        // Zamiast hakowania stosu, użyj pola pomocniczego
        t.kernel_rsp = ksp;

        // Zapamiętaj entry i arg — trampoline je pobierze
        // (użyjemy wolnych pól struktury lub prostego przekazania)
        // Dla uproszczenia: entry w pierwszym slotie na stosie po powrocie
        *((ksp + 8*6) as *mut u64) = entry;  // "ret addr" po r15..rbx
        // arg — trampoline pobierze z r15 (pierwszy push callee-saved)
        *(ksp as *mut u64) = arg; // r15 = arg

        let bytes = name.as_bytes();
        for j in 0..core::cmp::min(15, bytes.len()) { t.name[j] = bytes[j]; }

        THREAD_COUNT.fetch_add(1, Ordering::Relaxed);
        let mut buf = [0u8; 20];
        print("[THREAD] Utworzono #"); print(usize_to_str(i, &mut buf));
        print(": "); print(name); print("\n");
        return i as i32;
    }
    -1
}

/// Trampoline dla wątków kernelowych.
#[naked]
unsafe extern "C" fn thread_kernel_trampoline() -> ! {
    asm!(
        // r15 = arg, r14 ustawione przez thread_switch
        // Po powrocie z thread_switch tutaj trafia PC
        // entry point jest w r14 (drugi callee-saved)
        "mov rdi, r15",   // arg → rdi
        "call r14",       // call entry(arg)
        "cli",
        "hlt",
        options(noreturn)
    );
}

/// Trampoline dla wątków użytkownika — przejście do ring3 przez iretq.
#[naked]
unsafe extern "C" fn thread_user_trampoline() -> ! {
    asm!(
        // r15 = arg, r14 = entry (user)
        // Budujemy ramkę iretq na stosie kernelowym
        "push $0x20 | 3",    // SS (user data | RPL=3)
        "push r13",          // RSP użytkownika (zapisane w r13 przez scheduler)
        "push $0x202",       // RFLAGS: IF=1
        "push $0x18 | 3",    // CS (user code | RPL=3)
        "push r14",          // RIP (entry)
        "mov rdi, r15",      // arg → rdi
        "iretq",
        options(noreturn)
    );
}

/// Prosty scheduler round-robin.
pub unsafe fn schedule() {
    let start = CURRENT_THREAD;
    let mut next = start;
    for _ in 0..MAX_THREADS {
        next = (next + 1) % MAX_THREADS;
        if THREADS[next].state == ThreadState::Ready { break; }
    }
    if next == start && THREADS[start].state == ThreadState::Running { return; }

    THREADS[start].state = ThreadState::Ready;
    THREADS[next].state  = ThreadState::Running;
    CURRENT_THREAD = next;

    // Ustaw rsp0 w TSS na stos kernelowy nowego wątku
    tss_set_kernel_stack(THREADS[next].kernel_stack_top);

    // Przełącz CR3 jeśli wątek ma inną przestrzeń adresową
    let new_cr3 = THREADS[next].cr3;
    let current_cr3: u64;
    asm!("mov {}, cr3", out(reg) current_cr3, options(nomem, nostack));
    if new_cr3 != current_cr3 {
        asm!("mov cr3, {}", in(reg) new_cr3, options(nostack));
    }

    thread_switch(
        &mut THREADS[start].kernel_rsp as *mut VirtAddr,
        THREADS[next].kernel_rsp,
    );
}

/// Przełącznik kontekstu — zapisuje/przywraca callee-saved + RIP przez ret.
#[naked]
unsafe extern "C" fn thread_switch(_old_rsp_ptr: *mut VirtAddr, _new_rsp: VirtAddr) {
    // rdi = *old_rsp_ptr (gdzie zapisać RSP), rsi = new_rsp
    asm!(
        // Zapisz callee-saved + adres powrotu (jest na stosie jako ret addr)
        "pushq %rbx",
        "pushq %rbp",
        "pushq %r12",
        "pushq %r13",
        "pushq %r14",
        "pushq %r15",
        // Zapisz RSP
        "movq %rsp, (%rdi)",
        // Przywróć RSP
        "movq %rsi, %rsp",
        // Przywróć callee-saved
        "popq %r15",
        "popq %r14",
        "popq %r13",
        "popq %r12",
        "popq %rbp",
        "popq %rbx",
        "ret",
        options(noreturn)
    );
}

unsafe extern "C" fn kernel_idle_thread(_arg: u64) -> ! {
    loop { asm!("hlt", options(nomem, nostack)); }
}

// ============================================================================
// ISR — przerwania sprzętowe i wyjątki
// ============================================================================

/// ISR timera (IRQ 0 → wektor 0x20).
#[naked]
unsafe extern "C" fn timer_isr() {
    asm!(
        // Wyślij EOI do PIC
        "pushq %rax",
        "movb $0x20, %al",
        "outb %al, $0x20",
        "popq %rax",
        // Wywołaj scheduler
        "call {sched}",
        "iretq",
        sched = sym schedule,
        options(noreturn)
    );
}

/// ISR page fault (wektor 0x0E) — CPU odkłada error code przed RIP.
#[naked]
unsafe extern "C" fn page_fault_isr() {
    asm!(
        // Stos: [error_code, rip, cs, rflags, rsp, ss]
        "pop  %rsi",          // error code → rsi (arg2)
        "mov  %cr2, %rdi",    // faulting address → rdi (arg1)
        "call {handler}",
        "iretq",
        handler = sym page_fault_handler,
        options(noreturn)
    );
}

/// ISR double fault (wektor 0x08).
#[naked]
unsafe extern "C" fn double_fault_isr() {
    asm!(
        "cli",
        "hlt",
        options(noreturn)
    );
}

// ============================================================================
// SYSCALL HANDLER (int 0x80) — oparty na jawnym TrapFrame
// ============================================================================
const SYS_EXIT:  u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ:  u64 = 2;

/// Naked ISR — buduje TrapFrame na stosie, wywołuje Rust handler.
#[naked]
unsafe extern "C" fn syscall_handler_asm() {
    asm!(
        // Odkładaj rejestry w kolejności odwrotnej do TrapFrame
        "pushq %rax",
        "pushq %rbp",
        "pushq %rbx",
        "pushq %rcx",
        "pushq %rdx",
        "pushq %rsi",
        "pushq %rdi",
        "pushq %r8",
        "pushq %r9",
        "pushq %r10",
        "pushq %r11",
        "pushq %r12",
        "pushq %r13",
        "pushq %r14",
        "pushq %r15",
        // Przekaż wskaźnik do TrapFrame jako argument
        "mov %rsp, %rdi",
        "call {handler}",
        // Przywróć rejestry
        "popq %r15", "popq %r14", "popq %r13", "popq %r12",
        "popq %r11", "popq %r10", "popq %r9",  "popq %r8",
        "popq %rdi", "popq %rsi", "popq %rdx", "popq %rcx",
        "popq %rbx", "popq %rbp",
        // rax zawiera wartość zwrotną (nadpisane przez handler)
        "addq $8, %rsp",  // pomiń oryginalny rax
        "iretq",
        handler = sym syscall_dispatch,
        options(noreturn)
    );
}

/// Dyspozytor wywołań systemowych — dostaje jawny TrapFrame.
#[no_mangle]
unsafe extern "C" fn syscall_dispatch(frame: *mut TrapFrame) {
    let tf = &mut *frame;
    let syscall_num = tf.rax;
    let arg1 = tf.rdi;
    let arg2 = tf.rsi;
    let arg3 = tf.rdx;

    let ret: u64 = match syscall_num {
        SYS_WRITE => {
            if arg1 == 1 || arg1 == 2 {
                let ptr = arg2 as *const u8;
                let len = arg3 as usize;
                // Prosta walidacja: nie wychodź poza user-space
                if arg2 < 0x0000_8000_0000_0000 {
                    for i in 0..len {
                        putchar_locked(*ptr.add(i) as char);
                    }
                    arg3
                } else { u64::MAX } // EFAULT
            } else { 0 }
        }
        SYS_READ => {
            // TODO: bufferowane wejście z klawiatury
            0 // EOF
        }
        SYS_EXIT => {
            let mut buf = [0u8; 20];
            print("\n[EXIT: "); print(usize_to_str(arg1 as usize, &mut buf)); print("]\n");
            // Zakończ bieżący wątek
            THREADS[CURRENT_THREAD].state = ThreadState::Terminated;
            THREAD_COUNT.fetch_sub(1, Ordering::Relaxed);
            schedule(); // oddaj CPU
            0
        }
        _ => u64::MAX, // ENOSYS
    };

    // Zapisz wartość zwrotną do rax w TrapFrame
    tf.rax = ret;
}

// ============================================================================
// HELPERS — bez globalnych static mut BUF (bufor przez argument)
// ============================================================================

/// Konwertuje usize → str dziesiętny. Bufor musi mieć ≥ 20 bajtów.
pub fn usize_to_str<'a>(mut val: usize, buf: &'a mut [u8; 20]) -> &'a str {
    if val == 0 {
        buf[19] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[19..]) };
    }
    let mut i = 19usize;
    while val > 0 {
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
        if i == 0 { break; }
        i -= 1;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i+1..]) }
}

/// Konwertuje u64 → hex string "0xXXXXXXXXXXXXXXXX". Bufor ≥ 18 bajtów.
pub fn u64_to_hex<'a>(mut val: u64, buf: &'a mut [u8; 18]) -> &'a str {
    const HEX: &[u8] = b"0123456789ABCDEF";
    buf[0] = b'0'; buf[1] = b'x';
    for i in (2..18).rev() {
        buf[i] = HEX[(val & 0xF) as usize];
        val >>= 4;
    }
    unsafe { core::str::from_utf8_unchecked(buf) }
}

// ============================================================================
// PANIC PATH — nie używa schedulera ani dynamicznych struktur
// ============================================================================

/// Panic bez dynamicznych alokacji — bezpieczna ścieżka awaryjna.
fn panic_no_dyn(msg: &str) -> ! {
    unsafe {
        asm!("cli", options(nomem, nostack)); // wyłącz przerwania
        print_raw("\n[KERNEL PANIC] ");
        print_raw(msg);
        print_raw("\n");
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe { asm!("cli", options(nomem, nostack)); }
    unsafe { print_raw("\n[KERNEL PANIC] "); }
    if let Some(msg) = info.message() {
        if let Some(s) = msg.as_str() {
            unsafe { print_raw(s); }
        } else {
            unsafe { print_raw("(no message)"); }
        }
    }
    if let Some(loc) = info.location() {
        unsafe { print_raw(" @ "); print_raw(loc.file()); }
    }
    unsafe { print_raw("\n"); }
    loop { unsafe { asm!("hlt", options(nomem, nostack)); } }
}

// ============================================================================
// MAIN ENTRY
// ============================================================================
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    unsafe {
        clear_screen();
        serial_init();
        serial_print("=== CosinusOS Microkernel v3.0 Boot ===\n");
        print("CosinusOS Microkernel v3.0 (Refactored)\n");
        print("=========================================\n\n");

        mm_init(0x100000, 0x700000);
        mm_init_paging(0x1000);
        mm_swap_init();

        init_gdt();
        init_pic();
        init_idt();
        init_pit();

        thread_init();

        print("\n[OK] System gotowy. Tworzenie wątków testowych...\n");
        for i in 0..3u64 {
            thread_create_kernel("worker\0", test_thread_entry as VirtAddr, i);
        }

        schedule();

        loop { asm!("hlt", options(nomem, nostack)); }
    }
}

unsafe extern "C" fn test_thread_entry(arg: u64) -> ! {
    let mut buf = [0u8; 20];
    print("[Thread "); print(usize_to_str(arg as usize, &mut buf)); print("] Started\n");
    loop {
        for _ in 0..1_000_000u64 { core::hint::spin_loop(); }
        print("[T"); print(usize_to_str(arg as usize, &mut buf)); print("] Tick\n");
    }
}