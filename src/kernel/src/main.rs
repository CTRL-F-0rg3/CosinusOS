// src/main.rs  — CosinusOS Microkernel
// Memory management delegated to mm.asm (linked externally).
//

#![no_std]
#![no_main]
#![feature(asm_const)]
#![feature(naked_functions)]
#![feature(abi_x86_interrupt)]

use core::arch::asm;

// ==================== Types ====================
type uint8_t  = u8;
type uint16_t = u16;
type uint32_t = u32;
type uint64_t = u64;
type size_t   = usize;

// ==================== Multiboot2 Header ====================
const MULTIBOOT2_MAGIC:         uint32_t  = 0xe85250d6;
const MULTIBOOT_ARCH_I386:      uint32_t  = 0;
const MULTIBOOT_HEADER_TAG_END: uint16_t  = 0;

#[repr(C, packed)]
struct MultibootHeader {
    magic:         uint32_t,
    architecture:  uint32_t,
    header_length: uint32_t,
    checksum:      uint32_t,
}

#[repr(C, packed)]
struct MultibootHeaderTag {
    type_: uint16_t,
    flags: uint16_t,
    size:  uint32_t,
}

#[repr(C, packed)]
struct MultibootBootstrap {
    header:  MultibootHeader,
    end_tag: MultibootHeaderTag,
}

#[link_section = ".multiboot"]
#[used]
static MULTIBOOT_HEADER: MultibootBootstrap = MultibootBootstrap {
    header: MultibootHeader {
        magic:         MULTIBOOT2_MAGIC,
        architecture:  MULTIBOOT_ARCH_I386,
        header_length: core::mem::size_of::<MultibootBootstrap>() as uint32_t,
        checksum: (-(MULTIBOOT2_MAGIC as i32
            + MULTIBOOT_ARCH_I386 as i32
            + core::mem::size_of::<MultibootBootstrap>() as i32)) as uint32_t,
    },
    end_tag: MultibootHeaderTag {
        type_: MULTIBOOT_HEADER_TAG_END,
        flags: 0,
        size:  8,
    },
};

// ==================== External MM (mm.asm) ====================
extern "C" {

    fn mm_init(base: uint64_t, size: uint64_t);

    
    fn mm_alloc_frame() -> uint64_t;

 
    fn mm_free_frame(frame_phys: uint64_t);

   
    fn mm_memset(dst: *mut u8, val: i32, n: size_t) -> *mut u8;


    fn mm_memcpy(dst: *mut u8, src: *const u8, n: size_t) -> *mut u8;


    fn mm_map_page(virt: uint64_t, phys: uint64_t, flags: uint64_t) -> i32;
}

// ==================== Port I/O ====================
#[inline(always)]
unsafe fn outb(port: uint16_t, val: uint8_t) {
    asm!("outb %al, %dx", in("al") val, in("dx") port, options(nostack));
}

#[inline(always)]
unsafe fn inb(port: uint16_t) -> uint8_t {
    let ret: uint8_t;
    asm!("inb %dx, %al", out("al") ret, in("dx") port, options(nostack));
    ret
}

#[inline(always)]
fn io_wait() {
    unsafe { outb(0x80, 0) };
}

// ==================== VGA Driver ====================
const VGA_MEMORY: *mut uint16_t = 0xB8000 as *mut uint16_t;
const VGA_WIDTH:  usize = 80;
const VGA_HEIGHT: usize = 25;

static mut VGA_BUFFER:   *mut uint16_t = VGA_MEMORY;
static mut CURSOR_X:     usize         = 0;
static mut CURSOR_Y:     usize         = 0;
static mut CURRENT_COLOR: uint8_t      = 0x0F;

unsafe fn vga_update_cursor() {
    let pos = CURSOR_Y * VGA_WIDTH + CURSOR_X;
    outb(0x3D4, 0x0F);
    outb(0x3D5, (pos & 0xFF) as u8);
    outb(0x3D4, 0x0E);
    outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
}

unsafe fn clear_screen() {
    for i in 0..(VGA_WIDTH * VGA_HEIGHT) {
        *VGA_BUFFER.add(i) = ((CURRENT_COLOR as uint16_t) << 8) | b' ' as u16;
    }
    CURSOR_X = 0;
    CURSOR_Y = 0;
    vga_update_cursor();
}

unsafe fn putchar(c: char) {
    match c {
        '\n'   => { CURSOR_X = 0; CURSOR_Y += 1; }
        '\r'   => { CURSOR_X = 0; }
        '\t'   => { CURSOR_X = (CURSOR_X + 4) & !3; }
        '\x08' => { if CURSOR_X > 0 { CURSOR_X -= 1; } }
        _ => {
            let pos = CURSOR_Y * VGA_WIDTH + CURSOR_X;
            *VGA_BUFFER.add(pos) = ((CURRENT_COLOR as uint16_t) << 8) | c as u16;
            CURSOR_X += 1;
        }
    }

    if CURSOR_X >= VGA_WIDTH { CURSOR_X = 0; CURSOR_Y += 1; }

    if CURSOR_Y >= VGA_HEIGHT {
        // scroll up one line
        for i in 0..(VGA_HEIGHT - 1) * VGA_WIDTH {
            *VGA_BUFFER.add(i) = *VGA_BUFFER.add(i + VGA_WIDTH);
        }
        for i in 0..VGA_WIDTH {
            *VGA_BUFFER.add((VGA_HEIGHT - 1) * VGA_WIDTH + i) =
                ((CURRENT_COLOR as uint16_t) << 8) | b' ' as u16;
        }
        CURSOR_Y = VGA_HEIGHT - 1;
    }

    vga_update_cursor();
}

unsafe fn print(s: &str) {
    for c in s.chars() { putchar(c); }
}

// ==================== Serial Port ====================
const COM1: uint16_t = 0x3F8;

unsafe fn serial_init() {
    outb(COM1 + 1, 0x00);           // Disable interrupts
    outb(COM1 + 3, 0x80);           // Enable DLAB (set baud rate divisor)
    // POPRAWKA: divisor musi być zapisany kiedy DLAB=1
    outb(COM1 + 0, 0x03);           // Baud = 38400 (divisor low byte = 3)
    outb(COM1 + 1, 0x00);           // Divisor high byte = 0
    outb(COM1 + 3, 0x03);           // 8 bits, no parity, one stop bit (clears DLAB)
    outb(COM1 + 2, 0xC7);           // Enable FIFO, clear, 14-byte threshold
    outb(COM1 + 4, 0x0B);           // IRQs enabled, RTS/DSR set
}

unsafe fn serial_write(c: char) {
    while (inb(COM1 + 5) & 0x20) == 0 {}
    outb(COM1, c as u8);
}

unsafe fn serial_print(s: &str) {
    for c in s.chars() { serial_write(c); }
}

// ==================== GDT ====================
#[repr(C, packed)]
struct GdtEntry {
    limit_low:   uint16_t,
    base_low:    uint16_t,
    base_middle: uint8_t,
    access:      uint8_t,

    granularity: uint8_t,
    base_high:   uint8_t,
}

#[repr(C, packed)]
struct GdtPtr {
    limit: uint16_t,
    base:  uint64_t,
}

const GDT_ENTRIES: usize = 5;
static mut GDT: [GdtEntry; GDT_ENTRIES] = [GdtEntry {
    limit_low:   0,
    base_low:    0,
    base_middle: 0,
    access:      0,
    granularity: 0,
    base_high:   0,
}; GDT_ENTRIES];

static mut GDT_PTR: GdtPtr = GdtPtr { limit: 0, base: 0 };

const SEG_KERNEL_CODE: uint16_t = 0x08;
const SEG_KERNEL_DATA: uint16_t = 0x10;
const SEG_USER_CODE:   uint16_t = 0x18;
const SEG_USER_DATA:   uint16_t = 0x20;

unsafe fn gdt_set_gate(
    num:    usize,
    base:   uint64_t,
    limit:  uint64_t,
    access: uint8_t,
    gran:   uint8_t,
) {
    GDT[num].base_low    = (base & 0xFFFF) as uint16_t;
    GDT[num].base_middle = ((base >> 16) & 0xFF) as uint8_t;
    GDT[num].base_high   = ((base >> 24) & 0xFF) as uint8_t;
    GDT[num].limit_low   = (limit & 0xFFFF) as uint16_t;
    GDT[num].granularity = (((limit >> 16) & 0x0F) as uint8_t) | (gran & 0xF0);
    GDT[num].access      = access;
}

unsafe fn init_gdt() {
    GDT_PTR.limit = (core::mem::size_of_val(&GDT) - 1) as uint16_t;
    GDT_PTR.base  = &GDT as *const _ as uint64_t;

    
    //                  0x00 dla danych (limit ignorowany w 64-bit)
    gdt_set_gate(0, 0, 0,          0x00, 0x00); // NULL
    gdt_set_gate(1, 0, 0xFFFFFFFF, 0x9A, 0x20); // Kernel Code  (L=1)
    gdt_set_gate(2, 0, 0xFFFFFFFF, 0x92, 0x00); // Kernel Data
    gdt_set_gate(3, 0, 0xFFFFFFFF, 0xFA, 0x20); // User Code    (L=1, DPL=3)
    gdt_set_gate(4, 0, 0xFFFFFFFF, 0xF2, 0x00); // User Data    (DPL=3)

    asm!("lgdt [{}]", in(reg) &GDT_PTR, options(preserves_flags));

    // Reload segment registers
    asm!(
        "pushq $0x08",
        "lea 1f(%rip), %rax",
        "pushq %rax",
        "lretq",
        "1:",
        "mov $0x10, %ax",
        "mov %ax, %ds",
        "mov %ax, %es",
        "mov %ax, %fs",
        "mov %ax, %gs",
        "mov %ax, %ss",
        out("rax") _,
        options(preserves_flags)
    );
}

// ==================== IDT ====================
#[repr(C, packed)]
struct IdtEntry {
    offset_low:  uint16_t,
    selector:    uint16_t,
    ist:         uint8_t,
    type_attr:   uint8_t,
    offset_mid:  uint16_t,
    offset_high: uint32_t,
    zero:        uint32_t,
}

#[repr(C, packed)]
struct Idtr {
    limit: uint16_t,
    base:  uint64_t,
}

const IDT_ENTRIES: usize = 256;
static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry {
    offset_low:  0,
    selector:    0,
    ist:         0,
    type_attr:   0,
    offset_mid:  0,
    offset_high: 0,
    zero:        0,
}; IDT_ENTRIES];
static mut IDTR: Idtr = Idtr { limit: 0, base: 0 };

unsafe fn idt_set_gate(num: uint8_t, handler: uint64_t, dpl: uint8_t) {
    let e = &mut IDT[num as usize];
    e.offset_low  = (handler & 0xFFFF) as uint16_t;
    e.offset_mid  = ((handler >> 16) & 0xFFFF) as uint16_t;
    e.offset_high = ((handler >> 32) & 0xFFFFFFFF) as uint32_t;
    e.selector    = SEG_KERNEL_CODE;
    e.ist         = 0;
    e.type_attr   = 0x8E | (dpl << 5);  // Interrupt Gate | DPL
    e.zero        = 0;
}

// ==================== Syscall Handler ====================
const SYS_EXIT:  uint64_t = 0;
const SYS_WRITE: uint64_t = 1;
const SYS_READ:  uint64_t = 2;

#[no_mangle]
#[naked]
unsafe extern "C" fn syscall_handler_asm() {

    asm!(
        "pushq %rbp",
        "movq  %rsp, %rbp",
        "pushq %rbx",
        "pushq %r12",
        "pushq %r13",
        "pushq %r14",
        "pushq %r15",
        "call {handler}",
        "popq  %r15",
        "popq  %r14",
        "popq  %r13",
        "popq  %r12",
        "popq  %rbx",
        "popq  %rbp",
        "iretq",
        handler = sym syscall_handler,
        options(noreturn)
    );
}

#[no_mangle]
unsafe extern "C" fn syscall_handler() {
    let syscall_num: uint64_t;
    let arg1:        uint64_t;
    let arg2:        uint64_t;
    let arg3:        uint64_t;

    asm!(
        "movq %rax, {num}",
        "movq %rdi, {a1}",
        "movq %rsi, {a2}",
        "movq %rdx, {a3}",
        num = out(reg) syscall_num,
        a1  = out(reg) arg1,
        a2  = out(reg) arg2,
        a3  = out(reg) arg3,
        options(nostack, preserves_flags)
    );

    match syscall_num {
        SYS_WRITE => {
            if arg1 == 1 || arg1 == 2 {
                let str_ptr = arg2 as *const u8;
                for i in 0..arg3 {
                    putchar(*str_ptr.add(i as usize) as char);
                }
            }
        }
        SYS_EXIT => {
            print("\n[EXIT: ");
            let mut code = arg1;
            if code == 0 {
                putchar('0');
            } else {
                let mut buf = [0u8; 20];
                let mut i   = 0usize;
                while code > 0 {
                    buf[i] = b'0' + (code % 10) as u8;
                    code  /= 10;
                    i     += 1;
                }
                while i > 0 {
                    i -= 1;
                    putchar(buf[i] as char);
                }
            }
            print("]\n");
            asm!("cli; hlt", options(noreturn));
        }
        _ => { /* nieznany syscall — ignoruj */ }
    }
}

// ==================== PIC ====================
unsafe fn init_pic() {
    outb(0x20, 0x11); io_wait();  // ICW1: init + ICW4 needed
    outb(0xA0, 0x11); io_wait();
    outb(0x21, 0x20); io_wait();  // ICW2: master offset → IRQ0 = INT 0x20
    outb(0xA1, 0x28); io_wait();  // ICW2: slave offset  → IRQ8 = INT 0x28
    outb(0x21, 0x04); io_wait();  // ICW3: slave on IRQ2
    outb(0xA1, 0x02); io_wait();  // ICW3: slave ID = 2
    outb(0x21, 0x01); io_wait();  // ICW4: 8086 mode
    outb(0xA1, 0x01); io_wait();
    outb(0x21, 0xFF);             // Mask all IRQs (master)
    outb(0xA1, 0xFF);             // Mask all IRQs (slave)
}

unsafe fn init_idt() {
    mm_memset(
        IDT.as_mut_ptr() as *mut u8,
        0,
        core::mem::size_of_val(&IDT),
    );

    idt_set_gate(0x80, syscall_handler_asm as uint64_t, 3);

    IDTR.limit = (core::mem::size_of_val(&IDT) - 1) as uint16_t;
    IDTR.base  = &IDT as *const _ as uint64_t;

    asm!("lidt [{}]", in(reg) &IDTR, options(preserves_flags));
    asm!("sti",                       options(nomem, nostack));
}

// ==================== Userspace Loading ====================
extern "C" {
    #[link_name = "_binary_build_userspace_raw_bin_start"]
    static USERSPACE_START: u8;
    #[link_name = "_binary_build_userspace_raw_bin_end"]
    static USERSPACE_END: u8;
}

// Stos użytkownika — alokowany przez MM na etapie init
const USER_STACK_SIZE: usize   = 0x10000; // 64 KiB
const USER_LOAD_ADDR:  uint64_t = 0x400000;
const USER_STACK_VIRT: uint64_t = 0x7FFF_0000;

#[repr(align(16))]
static mut USER_STACK: [u8; USER_STACK_SIZE] = [0; USER_STACK_SIZE];

unsafe fn jump_to_userspace() {
    let userspace_code = &USERSPACE_START as *const u8;
    let size = (&USERSPACE_END as *const u8).offset_from(userspace_code) as size_t;

    // Kopiuj kod użytkownika pod USER_LOAD_ADDR
    mm_memcpy(USER_LOAD_ADDR as *mut u8, userspace_code, size);

    serial_print("[Kernel] Userspace loaded, jumping to ring 3...\n");

    let user_rip: uint64_t = USER_LOAD_ADDR;
    // POPRAWKA: używamy statycznego stosu USER_STACK z wyrównaniem 16-bajtowym
    let user_rsp: uint64_t =
        USER_STACK.as_ptr() as uint64_t + USER_STACK_SIZE as uint64_t - 16;

    // POPRAWKA RFLAGS: pushfq + or IF=1, zamiast czystego pushfq
    // Bez ustawionego IF (bit 9) przerwania byłyby zablokowane w ring 3.
    asm!(
        // Budujemy ramkę iretq: SS, RSP, RFLAGS, CS, RIP
        "pushq  $0x20",          // SS  = User Data (SEG_USER_DATA | RPL3 = 0x20|3 = 0x23)
        "pushq  {rsp}",          // RSP użytkownika
        "pushfq",                // RFLAGS
        "orq    $0x200, (%rsp)", // ustaw IF (bit 9) w RFLAGS na stosie
        "pushq  $0x18",          // CS  = User Code (SEG_USER_CODE | RPL3 = 0x18|3 = 0x1B)
        "pushq  {rip}",          // RIP
        "iretq",
        rip = in(reg) user_rip,
        rsp = in(reg) user_rsp,
        options(noreturn)
    );
}

// ==================== Panic Handler ====================
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("\n[PANIC] ");
        if let Some(msg) = _info.message() {
            print(msg.as_str().unwrap_or("unknown panic"));
        }
        loop {
            asm!("hlt", options(nomem, nostack));
        }
    }
}

// ==================== Main Entry ====================
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    unsafe {
        clear_screen();
        serial_init();

        serial_print("=== CosinusOS Microkernel Boot ===\n");
        print("CosinusOS Microkernel\n");
        print("=====================\n\n");

        // Inicjalizuj MM — region 1 MiB–8 MiB (pomiń pierwsze 1 MiB dla BIOS/VGA)
        mm_init(0x100000, 0x700000);
        print("[OK] MM (bitmap allocator)\n");

        init_gdt();
        print("[OK] GDT\n");

        init_pic();
        print("[OK] PIC\n");

        init_idt();
        print("[OK] IDT\n");

        print("\nLoading userspace...\n\n");
        jump_to_userspace();

        // Nigdy nie powinniśmy tu dotrzeć
        print("\n[PANIC] Userspace returned!\n");
        loop {
            asm!("hlt", options(nomem, nostack));
        }
    }
}