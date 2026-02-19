// src/kernel/src/main.rs — CosinusOS Microkernel v2.1 (Fixed ABI)
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

const PAGE_SIZE: usize = 0x1000;
const MAX_FRAMES: usize = 0x10000;
const MAX_THREADS: usize = 64;
const THREAD_STACK_SIZE: usize = 0x4000; // 16KB stos

// ============================================================================
// MULTIBOOT2 HEADER
// ============================================================================
const MULTIBOOT2_MAGIC: u32 = 0xe85250d6;
const MULTIBOOT_ARCH_I386: u32 = 0;
const MULTIBOOT_HEADER_TAG_END: u16 = 0;

#[repr(C, packed)]
struct MultibootHeader {
    magic: u32,
    architecture: u32,
    header_length: u32,
    checksum: u32,
}

#[repr(C, packed)]
struct MultibootHeaderTag {
    type_: u16,
    flags: u16,
    size: u32,
}

#[repr(C, packed)]
struct MultibootBootstrap {
    header: MultibootHeader,
    end_tag: MultibootHeaderTag,
}

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
    end_tag: MultibootHeaderTag {
        type_: MULTIBOOT_HEADER_TAG_END,
        flags: 0,
        size: 8,
    },
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

fn io_wait() {
    unsafe { outb(0x80, 0); }
}

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

unsafe fn vga_update_cursor() {
    let pos = CURSOR_Y * VGA_WIDTH + CURSOR_X;
    outb(0x3D4, 0x0F);
    outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E);
    outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
}

unsafe fn clear_screen() {
    for i in 0..(VGA_WIDTH * VGA_HEIGHT) {
        *VGA_BUFFER.add(i) = ((CURRENT_COLOR as u16) << 8) | b' ' as u16;
    }
    CURSOR_X = 0;
    CURSOR_Y = 0;
    vga_update_cursor();
}

unsafe fn putchar(c: char) {
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

unsafe fn print(s: &str) {
    for c in s.chars() { putchar(c); }
}

// ============================================================================
// SERIAL PORT
// ============================================================================
const COM1: u16 = 0x3F8;

unsafe fn serial_init() {
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x80);
    outb(COM1 + 0, 0x03);
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03);
    outb(COM1 + 2, 0xC7);
    outb(COM1 + 4, 0x0B);
}

unsafe fn serial_write(c: char) {
    while (inb(COM1 + 5) & 0x20) == 0 {}
    outb(COM1, c as u8);
}

unsafe fn serial_print(s: &str) {
    for c in s.chars() { serial_write(c); }
}

// ============================================================================
// SPINLOCK
// ============================================================================
pub struct Spinlock {
    locked: AtomicBool,
}

impl Spinlock {
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }

    pub fn lock(&self) {
        while self.locked.swap(true, Ordering::Acquire) {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
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
    let word = idx / 64;
    let bit = idx % 64;
    (FRAME_BITMAP[word] & (1 << bit)) == 0
}

unsafe fn set_frame_used(idx: usize) {
    let word = idx / 64;
    let bit = idx % 64;
    FRAME_BITMAP[word] |= 1 << bit;
}

unsafe fn set_frame_free(idx: usize) {
    let word = idx / 64;
    let bit = idx % 64;
    FRAME_BITMAP[word] &= !(1 << bit);
}

pub unsafe fn mm_init(base: PhysAddr, size: usize) {
    MEMORY_BASE = base;
    MEMORY_SIZE = size;
    let frames = size / PAGE_SIZE;
    
    core::ptr::write_bytes(
        FRAME_BITMAP.as_mut_ptr() as *mut u8,
        0,
        core::mem::size_of_val(&FRAME_BITMAP),
    );
    
    // Zarezerwuj pierwsze 256 ramek (1MB) dla jądra
    for i in 0..core::cmp::min(256, frames) {
        set_frame_used(i);
    }
    
    print("[MM] Initialized: ");
    print(itoa(frames * PAGE_SIZE / 1024 / 1024));
    print(" MiB available\n");
}

pub unsafe fn mm_alloc_frame() -> PhysAddr {
    MM_LOCK.lock();
    for word_idx in 0..FRAME_BITMAP.len() {
        if FRAME_BITMAP[word_idx] != !0 {
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
    print("[MM] OUT OF MEMORY!\n");
    panic!("Physical memory exhausted");
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
struct PageTable {
    entries: [PageTableEntry; 512],
}

#[repr(transparent)]
#[derive(Clone, Copy)]
struct PageTableEntry(u64);

impl PageTableEntry {
    const PRESENT: u64 = 1 << 0;
    const WRITABLE: u64 = 1 << 1;
    const USER: u64 = 1 << 2;
    
    fn new(phys: PhysAddr, flags: u64) -> Self {
        Self((phys & !0xFFF) | flags | Self::PRESENT)
    }
    
    fn is_present(&self) -> bool { self.0 & Self::PRESENT != 0 }
    fn addr(&self) -> PhysAddr { self.0 & !0xFFF }
    fn set(&mut self, entry: PageTableEntry) { self.0 = entry.0; }
}

static mut P4_TABLE: Option<&'static mut PageTable> = None;

pub unsafe fn mm_init_paging(kernel_cr3: PhysAddr) {
    asm!("mov cr3, {}", in(reg) kernel_cr3, options(preserves_flags));
    print("[MMU] Paging enabled\n");
}

pub unsafe fn mm_map_page(virt: VirtAddr, phys: PhysAddr, flags: u64) -> i32 {
    MM_LOCK.lock();
    
    let p4_idx = (virt >> 39) & 0x1FF;
    let p3_idx = (virt >> 30) & 0x1FF;
    let p2_idx = (virt >> 21) & 0x1FF;
    let p1_idx = (virt >> 12) & 0x1FF;
    
    let p4 = P4_TABLE.as_mut().unwrap();
    
    if !p4.entries[p4_idx].is_present() {
        let pt_phys = mm_alloc_frame();
        p4.entries[p4_idx].set(PageTableEntry::new(pt_phys, PageTableEntry::WRITABLE));
        core::ptr::write_bytes(pt_phys as *mut u8, 0, PAGE_SIZE);
    }
    let p3 = &mut *(p4.entries[p4_idx].addr() as *mut PageTable);
    
    if !p3.entries[p3_idx].is_present() {
        let pt_phys = mm_alloc_frame();
        p3.entries[p3_idx].set(PageTableEntry::new(pt_phys, PageTableEntry::WRITABLE));
        core::ptr::write_bytes(pt_phys as *mut u8, 0, PAGE_SIZE);
    }
    let p2 = &mut *(p3.entries[p3_idx].addr() as *mut PageTable);
    
    if !p2.entries[p2_idx].is_present() {
        let pt_phys = mm_alloc_frame();
        p2.entries[p2_idx].set(PageTableEntry::new(pt_phys, PageTableEntry::WRITABLE));
        core::ptr::write_bytes(pt_phys as *mut u8, 0, PAGE_SIZE);
    }
    let p1 = &mut *(p2.entries[p2_idx].addr() as *mut PageTable);
    
    p1.entries[p1_idx].set(PageTableEntry::new(phys, flags));
    
    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    
    MM_LOCK.unlock();
    0
}

pub unsafe fn mm_unmap_page(virt: VirtAddr) {
    MM_LOCK.lock();
    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    MM_LOCK.unlock();
}

// ============================================================================
// SWAP FRAMEWORK
// ============================================================================
const SWAP_ENABLED: bool = true;
static mut SWAP_BITMAP: [u64; 1024] = [0; 1024];

pub unsafe fn mm_swap_init() {
    if !SWAP_ENABLED { return; }
    print("[SWAP] Framework initialized (256 MiB reserved)\n");
}

pub unsafe fn mm_swap_out(virt: VirtAddr, phys: PhysAddr) -> bool {
    if !SWAP_ENABLED { return false; }
    
    for word in 0..SWAP_BITMAP.len() {
        if SWAP_BITMAP[word] != !0 {
            for bit in 0..64 {
                let slot = word * 64 + bit;
                if SWAP_BITMAP[word] & (1 << bit) == 0 {
                    SWAP_BITMAP[word] |= 1 << bit;
                    
                    print("[SWAP] OUT: 0x");
                    print(itoa_hex(virt));
                    print(" -> slot ");
                    print(itoa(slot));
                    print("\n");
                    
                    mm_unmap_page(virt);
                    mm_free_frame(phys);
                    return true;
                }
            }
        }
    }
    print("[SWAP] NO FREE SLOTS!\n");
    false
}

pub unsafe fn mm_swap_in(virt: VirtAddr, flags: u64) -> Option<PhysAddr> {
    if !SWAP_ENABLED { return None; }
    
    let phys = mm_alloc_frame();
    core::ptr::write_bytes(phys as *mut u8, 0xAA, PAGE_SIZE);
    
    mm_map_page(virt, phys, flags);
    print("[SWAP] IN: slot -> 0x");
    print(itoa_hex(virt));
    print("\n");
    Some(phys)
}

// ============================================================================
// PAGE FAULT HANDLER
// ============================================================================
#[no_mangle]
pub unsafe extern "C" fn page_fault_handler(addr: VirtAddr, error: u64) {
    let present = error & 0x1 != 0;
    let write = error & 0x2 != 0;
    let user = error & 0x4 != 0;
    
    print("[PF] Addr: 0x");
    print(itoa_hex(addr));
    print(" Error: 0x");
    print(itoa_hex(error));
    print(" (");
    if present { print("P"); } else { print("-"); }
    if write { print("W"); } else { print("-"); }
    if user { print("U"); } else { print("-"); }
    print(")\n");
    
    if !present {
        if SWAP_ENABLED {
            if let Some(_phys) = mm_swap_in(addr, PageTableEntry::WRITABLE | PageTableEntry::USER) {
                return;
            }
        }
        print("[PF] Cannot resolve fault - killing thread\n");
    }
    
    panic!("Unhandled page fault");
}

// ============================================================================
// THREADING SYSTEM
// ============================================================================
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CpuContext {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9: u64, pub r8: u64,
    pub rdi: u64, pub rsi: u64, pub rbp: u64, pub rbx: u64,
    pub rdx: u64, pub rcx: u64, pub rax: u64,
    pub rip: u64, pub cs: u64, pub rflags: u64,
    pub rsp: u64, pub ss: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ThreadState {
    Running, Ready, Blocked, Terminated,
}

#[repr(C)]
pub struct ThreadControlBlock {
    pub id: u32,
    pub state: ThreadState,
    pub priority: u8,
    pub stack_ptr: VirtAddr,
    pub context: CpuContext,
    pub name: [u8; 16],
}

impl ThreadControlBlock {
    pub const fn new() -> Self {
        Self {
            id: 0, state: ThreadState::Terminated, priority: 10,
            stack_ptr: 0,
            context: CpuContext {
                r15: 0, r14: 0, r13: 0, r12: 0,
                r11: 0, r10: 0, r9: 0, r8: 0,
                rdi: 0, rsi: 0, rbp: 0, rbx: 0,
                rdx: 0, rcx: 0, rax: 0,
                rip: 0, cs: 0, rflags: 0,
                rsp: 0, ss: 0,
            },
            name: [0; 16],
        }
    }
}

static mut THREADS: [ThreadControlBlock; MAX_THREADS] = [ThreadControlBlock::new(); MAX_THREADS];
static mut CURRENT_THREAD: usize = 0;
static mut THREAD_COUNT: usize = 0;
static mut SCHEDULER_TICK: usize = 0;

pub unsafe fn thread_init() {
    print("[THREAD] Subsystem initialized\n");
    let tid = thread_create("idle", kernel_idle_thread as VirtAddr, 0);
    if tid >= 0 {
        THREADS[tid as usize].state = ThreadState::Running;
        CURRENT_THREAD = tid as usize;
    }
}

pub unsafe fn thread_create(name: &str, entry: VirtAddr, arg: u64) -> i32 {
    for i in 0..MAX_THREADS {
        if THREADS[i].state == ThreadState::Terminated {
            let tcb = &mut THREADS[i];
            
            // POPRAWKA: Alokuj pełny stos (16KB = 4 strony)
            let stack_virt_base = 0xFFFF_8000_0000_0000 + (i as u64 * THREAD_STACK_SIZE as u64);
            
            for page in 0..(THREAD_STACK_SIZE / PAGE_SIZE) {
                let stack_phys = mm_alloc_frame();
                let page_virt = stack_virt_base + (page as u64 * PAGE_SIZE as u64);
                mm_map_page(page_virt, stack_phys, PageTableEntry::WRITABLE | PageTableEntry::USER);
            }
            
            tcb.id = i as u32;
            tcb.state = ThreadState::Ready;
            tcb.priority = 10;
            tcb.stack_ptr = stack_virt_base + THREAD_STACK_SIZE as u64;
            tcb.context.rip = entry;
            tcb.context.rsp = tcb.stack_ptr;
            
            // POPRAWKA: Poprawne selektory GDT z RPL=3 (user mode)
            // 0x18 = user code (3 * 8), 0x20 = user data (4 * 8)
            tcb.context.cs = 0x18 | 3;  // | 3 ustawia RPL na ring 3
            tcb.context.ss = 0x20 | 3;
            tcb.context.rflags = 0x202; // IF = 1
            tcb.context.rdi = arg;
            
            let bytes = name.as_bytes();
            for j in 0..core::cmp::min(15, bytes.len()) {
                tcb.name[j] = bytes[j];
            }
            
            THREAD_COUNT += 1;
            print("[THREAD] Created #");
            print(itoa(i));
            print(": ");
            print(name);
            print("\n");
            return i as i32;
        }
    }
    -1
}

pub unsafe fn schedule() {
    SCHEDULER_TICK += 1;
    
    let start = CURRENT_THREAD;
    for _ in 0..MAX_THREADS {
        CURRENT_THREAD = (CURRENT_THREAD + 1) % MAX_THREADS;
        if THREADS[CURRENT_THREAD].state == ThreadState::Ready {
            THREADS[start].state = ThreadState::Ready;
            THREADS[CURRENT_THREAD].state = ThreadState::Running;
            thread_switch(&THREADS[start].context, &THREADS[CURRENT_THREAD].context);
            return;
        }
    }
    THREADS[start].state = ThreadState::Running;
}

#[naked]
unsafe extern "C" fn thread_switch(_old: *const CpuContext, _new: *const CpuContext) {
    asm!(
        "pushq %rbx", "pushq %rbp", "pushq %r12", "pushq %r13", "pushq %r14", "pushq %r15",
        "movq %rsp, (%rdi)",
        "movq (%rsi), %rsp",
        "popq %r15", "popq %r14", "popq %r13", "popq %r12", "popq %rbp", "popq %rbx",
        "ret",
        options(noreturn)
    );
}

unsafe extern "C" fn kernel_idle_thread(_arg: u64) -> ! {
    loop {
        asm!("hlt", options(nomem, nostack));
    }
}

// ============================================================================
// PIT TIMER
// ============================================================================
unsafe fn init_pit() {
    let divisor = 1193180 / 100;
    outb(0x43, 0x36);
    outb(0x40, (divisor & 0xFF) as u8);
    outb(0x40, (divisor >> 8) as u8);
    print("[PIT] Timer initialized at 100 Hz\n");
}

#[no_mangle]
pub unsafe extern "C" fn timer_isr() {
    outb(0x20, 0x20);
    schedule();
}

// ============================================================================
// GDT
// ============================================================================
#[repr(C, packed)]
struct GdtEntry {
    limit_low: u16, base_low: u16, base_middle: u8,
    access: u8, granularity: u8, base_high: u8,
}

#[repr(C, packed)]
struct GdtPtr { limit: u16, base: u64 }

const GDT_ENTRIES: usize = 5;
static mut GDT: [GdtEntry; GDT_ENTRIES] = [GdtEntry {
    limit_low: 0, base_low: 0, base_middle: 0,
    access: 0, granularity: 0, base_high: 0,
}; GDT_ENTRIES];

static mut GDT_PTR: GdtPtr = GdtPtr { limit: 0, base: 0 };

unsafe fn gdt_set_gate(num: usize, base: u64, limit: u64, access: u8, gran: u8) {
    GDT[num].base_low = (base & 0xFFFF) as u16;
    GDT[num].base_middle = ((base >> 16) & 0xFF) as u8;
    GDT[num].base_high = ((base >> 24) & 0xFF) as u8;
    GDT[num].limit_low = (limit & 0xFFFF) as u16;
    GDT[num].granularity = (((limit >> 16) & 0x0F) as u8) | (gran & 0xF0);
    GDT[num].access = access;
}

unsafe fn init_gdt() {
    GDT_PTR.limit = (core::mem::size_of_val(&GDT) - 1) as u16;
    GDT_PTR.base = &GDT as *const _ as u64;

    // 0: Null
    gdt_set_gate(0, 0, 0, 0x00, 0x00);
    // 1: Kernel code (0x08)
    gdt_set_gate(1, 0, 0xFFFFFFFF, 0x9A, 0x20);
    // 2: Kernel data (0x10)
    gdt_set_gate(2, 0, 0xFFFFFFFF, 0x92, 0x00);
    // 3: User code (0x18) - DPL=3 (0xFA)
    gdt_set_gate(3, 0, 0xFFFFFFFF, 0xFA, 0x20);
    // 4: User data (0x20) - DPL=3 (0xF2)
    gdt_set_gate(4, 0, 0xFFFFFFFF, 0xF2, 0x00);

    asm!("lgdt [{}]", in(reg) &GDT_PTR, options(preserves_flags));

    asm!(
        "pushq $0x08", "lea 1f(%rip), %rax", "pushq %rax", "lretq", "1:",
        "mov $0x10, %ax", "mov %ax, %ds", "mov %ax, %es",
        "mov %ax, %fs", "mov %ax, %gs", "mov %ax, %ss",
        out("rax") _,
        options(preserves_flags)
    );
}

// ============================================================================
// IDT
// ============================================================================
#[repr(C, packed)]
struct IdtEntry {
    offset_low: u16, selector: u16, ist: u8, type_attr: u8,
    offset_mid: u16, offset_high: u32, zero: u32,
}

#[repr(C, packed)]
struct Idtr { limit: u16, base: u64 }

const IDT_ENTRIES: usize = 256;
static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry {
    offset_low: 0, selector: 0, ist: 0, type_attr: 0,
    offset_mid: 0, offset_high: 0, zero: 0,
}; IDT_ENTRIES];
static mut IDTR: Idtr = Idtr { limit: 0, base: 0 };

unsafe fn idt_set_gate(num: u8, handler: u64, dpl: u8) {
    let e = &mut IDT[num as usize];
    e.offset_low = (handler & 0xFFFF) as u16;
    e.offset_mid = ((handler >> 16) & 0xFFFF) as u16;
    e.offset_high = ((handler >> 32) & 0xFFFFFFFF) as u32;
    e.selector = 0x08; // Kernel code segment
    e.ist = 0;
    e.type_attr = 0x8E | (dpl << 5); // 0x8E = interrupt gate, DPL w bitach 5-6
    e.zero = 0;
}

unsafe fn init_idt() {
    core::ptr::write_bytes(IDT.as_mut_ptr() as *mut u8, 0, core::mem::size_of_val(&IDT));

    // 0x80: Syscall - DPL=3 (user can call)
    idt_set_gate(0x80, syscall_handler_asm as u64, 3);
    // 0x0E: Page fault
    idt_set_gate(0x0E, page_fault_handler as u64, 0);
    // 0x20: Timer
    idt_set_gate(0x20, timer_isr as u64, 0);

    IDTR.limit = (core::mem::size_of_val(&IDT) - 1) as u16;
    IDTR.base = &IDT as *const _ as u64;

    asm!("lidt [{}]", in(reg) &IDTR, options(preserves_flags));
    asm!("sti", options(nomem, nostack));
}

// ============================================================================
// PIC
// ============================================================================
unsafe fn init_pic() {
    outb(0x20, 0x11); io_wait();
    outb(0xA0, 0x11); io_wait();
    outb(0x21, 0x20); io_wait();
    outb(0xA1, 0x28); io_wait();
    outb(0x21, 0x04); io_wait();
    outb(0xA1, 0x02); io_wait();
    outb(0x21, 0x01); io_wait();
    outb(0xA1, 0x01); io_wait();
    outb(0x21, 0xFF);
    outb(0xA1, 0xFF);
    print("[PIC] Initialized\n");
}

// ============================================================================
// SYSCALL HANDLER - POPRAWIONY ABI
// ============================================================================
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2; // NOWE: obsługa read

/// POPRAWKA: Naked handler zachowuje wszystkie rejestry ABI
#[naked]
unsafe extern "C" fn syscall_handler_asm() {
    asm!(
        // Zachowaj wszystkie rejestry (kolejność odwrotna do przywracania)
        "pushq %r15", "pushq %r14", "pushq %r13", "pushq %r12",
        "pushq %r11", "pushq %r10", "pushq %r9", "pushq %r8",
        "pushq %rdi", "pushq %rsi", "pushq %rdx", "pushq %rcx",
        "pushq %rbx", "pushq %rbp",
        
        // Wywołaj handler w Rust
        "call {handler}",
        
        // Przywróć rejestry
        "popq %rbp", "popq %rbx",
        "popq %rcx", "popq %rdx", "popq %rsi", "popq %rdi",
        "popq %r8", "popq %r9", "popq %r10", "popq %r11",
        "popq %r12", "popq %r13", "popq %r14", "popq %r15",
        
        // Powrót przez iretq (int 0x80 odkłada ss, rsp, rflags, cs, rip)
        "iretq",
        handler = sym syscall_handler,
        options(noreturn)
    );
}

/// POPRAWKA: Handler zwraca wartość w rax (zgodnie z ABI)
#[no_mangle]
unsafe extern "C" fn syscall_handler() -> u64 {
    let syscall_num: u64;
    let arg1: u64;
    let arg2: u64;
    let arg3: u64;

    // Odczytaj argumenty z rejestrów (zachowanych przez asm!)
    asm!(
        "movq 8(%rsp), {num}",   // rax był pushnięty na stos
        "movq 24(%rsp), {a1}",   // rdi
        "movq 32(%rsp), {a2}",   // rsi  
        "movq 40(%rsp), {a3}",   // rdx
        num = out(reg) syscall_num,
        a1 = out(reg) arg1,
        a2 = out(reg) arg2, 
        a3 = out(reg) arg3,
        options(nostack, preserves_flags)
    );

    let ret: u64 = match syscall_num {
        SYS_WRITE => {
            if arg1 == 1 || arg1 == 2 { // stdout lub stderr
                let str_ptr = arg2 as *const u8;
                let len = arg3 as usize;
                for i in 0..len {
                    putchar(*str_ptr.add(i) as char);
                }
                arg3 // Zwróć liczbę zapisanych bajtów
            } else {
                0
            }
        }
        SYS_READ => {
            // NOWE: Tymczasowa implementacja - zwraca 0 (EOF)
            // W pełnej wersji: buforowane wejście z klawiatury
            0
        }
        SYS_EXIT => {
            print("\n[EXIT: ");
            let mut code = arg1;
            if code == 0 { putchar('0'); }
            else {
                let mut buf = [0u8; 20];
                let mut i = 0usize;
                while code > 0 {
                    buf[i] = b'0' + (code % 10) as u8;
                    code /= 10; i += 1;
                }
                while i > 0 { i -= 1; putchar(buf[i] as char); }
            }
            print("]\n");
            asm!("cli; hlt", options(noreturn));
        }
        _ => 0 // Nieznany syscall
    };

    // Zapisz wartość zwrotną do rax (zostanie przywrócone przez popq)
    asm!("movq {ret}, %rax", ret = in(reg) ret, options(nostack));
    
    ret
}

// ============================================================================
// HELPERS
// ============================================================================
fn itoa(mut val: usize) -> &'static str {
    static mut BUF: [u8; 20] = [0; 20];
    unsafe {
        if val == 0 { return "0"; }
        let mut i = 19;
        while val > 0 && i > 0 {
            BUF[i] = b'0' + (val % 10) as u8;
            val /= 10; i -= 1;
        }
        core::str::from_utf8_unchecked(&BUF[i+1..])
    }
}

fn itoa_hex(mut val: u64) -> &'static str {
    static mut BUF: [u8; 18] = *b"0x0000000000000000";
    let hex = b"0123456789ABCDEF";
    unsafe {
        for i in (2..18).rev() {
            BUF[i] = hex[(val & 0xF) as usize];
            val >>= 4;
        }
        core::str::from_utf8_unchecked(&BUF)
    }
}

// ============================================================================
// MAIN ENTRY
// ============================================================================
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    unsafe {
        clear_screen();
        serial_init();

        serial_print("=== CosinusOS Microkernel Boot ===\n");
        print("CosinusOS Microkernel v2.1 (Fixed ABI)\n");
        print("============================\n\n");

        mm_init(0x100000, 0x700000);
        mm_init_paging(0x1000);
        mm_swap_init();

        init_gdt();
        print("[OK] GDT\n");

        init_pic();
        init_idt();
        print("[OK] IDT\n");

        init_pit();
        thread_init();

        print("\n[OK] System ready. Creating test threads...\n");
        
        for i in 0..3 {
            thread_create("worker", test_thread_entry as VirtAddr, i as u64);
        }
        
        schedule();
        
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}

unsafe extern "C" fn test_thread_entry(arg: u64) -> ! {
    print("[Thread ");
    print(itoa(arg as usize));
    print("] Started\n");
    loop {
        for _ in 0..1000000 { core::hint::spin_loop(); }
        print("[T");
        print(itoa(arg as usize));
        print("] Tick\n");
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        print("\n[PANIC] ");
        if let Some(msg) = info.message() {
            print(msg.as_str().unwrap_or("unknown"));
        }
        loop { asm!("hlt", options(nomem, nostack)); }
    }
}