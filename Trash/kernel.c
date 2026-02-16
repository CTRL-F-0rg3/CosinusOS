/*
 * CosinusOS Microkernel - prawdziwy mikrokernel z userspace jako osobnym procesem
 */

#include "types.h"

// ==================== Multiboot2 Header ====================
#define MULTIBOOT2_MAGIC 0xe85250d6
#define MULTIBOOT_ARCH_I386 0
#define MULTIBOOT_HEADER_TAG_END 0

struct multiboot_header {
    uint32_t magic;
    uint32_t architecture;
    uint32_t header_length;
    uint32_t checksum;
} __attribute__((packed));

struct multiboot_header_tag {
    uint16_t type;
    uint16_t flags;
    uint32_t size;
} __attribute__((packed));

__attribute__((section(".multiboot"), used))
struct {
    struct multiboot_header header;
    struct multiboot_header_tag end_tag;
} multiboot_header = {
    .header = {
        .magic = MULTIBOOT2_MAGIC,
        .architecture = MULTIBOOT_ARCH_I386,
        .header_length = sizeof(multiboot_header),
        .checksum = (uint32_t)(-(MULTIBOOT2_MAGIC + MULTIBOOT_ARCH_I386 + (uint32_t)sizeof(multiboot_header)))
    },
    .end_tag = {
        .type = MULTIBOOT_HEADER_TAG_END,
        .flags = 0,
        .size = 8
    }
};

// ==================== Port I/O ====================
static inline void outb(uint16_t port, uint8_t val) {
    __asm__ volatile ("outb %0, %1" : : "a"(val), "Nd"(port));
}

static inline uint8_t inb(uint16_t port) {
    uint8_t ret;
    __asm__ volatile ("inb %1, %0" : "=a"(ret) : "Nd"(port));
    return ret;
}

static inline void io_wait(void) {
    outb(0x80, 0);
}

// ==================== Memory Functions ====================
void* memset(void* dst, int value, size_t n) {
    uint8_t* ptr = (uint8_t*)dst;
    for (size_t i = 0; i < n; i++)
        ptr[i] = (uint8_t)value;
    return dst;
}

void* memcpy(void* dst, const void* src, size_t n) {
    uint8_t* d = (uint8_t*)dst;
    const uint8_t* s = (const uint8_t*)src;
    for (size_t i = 0; i < n; i++)
        d[i] = s[i];
    return dst;
}

// ==================== VGA Driver ====================
#define VGA_MEMORY 0xB8000
#define VGA_WIDTH 80
#define VGA_HEIGHT 25

volatile uint16_t* vga_buffer = (uint16_t*)VGA_MEMORY;
int cursor_x = 0, cursor_y = 0;
uint8_t current_color = 0x0F;

void vga_update_cursor(void) {
    uint16_t pos = cursor_y * VGA_WIDTH + cursor_x;
    outb(0x3D4, 0x0F);
    outb(0x3D5, (uint8_t)(pos & 0xFF));
    outb(0x3D4, 0x0E);
    outb(0x3D5, (uint8_t)((pos >> 8) & 0xFF));
}

void clear_screen(void) {
    for (int i = 0; i < VGA_WIDTH * VGA_HEIGHT; i++)
        vga_buffer[i] = ((uint16_t)current_color << 8) | ' ';
    cursor_x = 0;
    cursor_y = 0;
    vga_update_cursor();
}

void putchar(char c) {
    if (c == '\n') {
        cursor_x = 0;
        cursor_y++;
    } else if (c == '\r') {
        cursor_x = 0;
    } else if (c == '\t') {
        cursor_x = (cursor_x + 4) & ~3;
    } else if (c == '\b') {
        if (cursor_x > 0) cursor_x--;
    } else {
        int pos = cursor_y * VGA_WIDTH + cursor_x;
        vga_buffer[pos] = ((uint16_t)current_color << 8) | c;
        cursor_x++;
    }

    if (cursor_x >= VGA_WIDTH) {
        cursor_x = 0;
        cursor_y++;
    }

    if (cursor_y >= VGA_HEIGHT) {
        for (int i = 0; i < (VGA_HEIGHT - 1) * VGA_WIDTH; i++)
            vga_buffer[i] = vga_buffer[i + VGA_WIDTH];
        for (int i = 0; i < VGA_WIDTH; i++)
            vga_buffer[(VGA_HEIGHT - 1) * VGA_WIDTH + i] = ((uint16_t)current_color << 8) | ' ';
        cursor_y = VGA_HEIGHT - 1;
    }

    vga_update_cursor();
}

void print(const char* str) {
    while (*str) putchar(*str++);
}

// ==================== Serial Port ====================
#define COM1 0x3F8

void serial_init(void) {
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x80);
    outb(COM1 + 0, 0x03);
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03);
    outb(COM1 + 2, 0xC7);
    outb(COM1 + 4, 0x0B);
}

void serial_write(char c) {
    while (!(inb(COM1 + 5) & 0x20));
    outb(COM1, c);
}

void serial_print(const char* str) {
    while (*str) serial_write(*str++);
}

// ==================== GDT ====================
struct gdt_entry {
    uint16_t limit_low;
    uint16_t base_low;
    uint8_t base_middle;
    uint8_t access;
    uint8_t granularity;
    uint8_t base_high;
} __attribute__((packed));

struct gdt_ptr {
    uint16_t limit;
    uint64_t base;
} __attribute__((packed));

#define GDT_ENTRIES 5
struct gdt_entry gdt[GDT_ENTRIES];
struct gdt_ptr gdt_ptr;

#define SEG_KERNEL_CODE 0x08
#define SEG_KERNEL_DATA 0x10
#define SEG_USER_CODE   0x18
#define SEG_USER_DATA   0x20

void gdt_set_gate(int num, uint64_t base, uint64_t limit, uint8_t access, uint8_t gran) {
    gdt[num].base_low = (base & 0xFFFF);
    gdt[num].base_middle = (base >> 16) & 0xFF;
    gdt[num].base_high = (base >> 24) & 0xFF;
    gdt[num].limit_low = (limit & 0xFFFF);
    gdt[num].granularity = ((limit >> 16) & 0x0F) | (gran & 0xF0);
    gdt[num].access = access;
}

void init_gdt(void) {
    gdt_ptr.limit = sizeof(gdt) - 1;
    gdt_ptr.base = (uint64_t)&gdt;

    gdt_set_gate(0, 0, 0, 0, 0);                    // NULL
    gdt_set_gate(1, 0, 0xFFFFFFFF, 0x9A, 0xAF);     // Kernel Code
    gdt_set_gate(2, 0, 0xFFFFFFFF, 0x92, 0xAF);     // Kernel Data
    gdt_set_gate(3, 0, 0xFFFFFFFF, 0xFA, 0xAF);     // User Code
    gdt_set_gate(4, 0, 0xFFFFFFFF, 0xF2, 0xAF);     // User Data

    __asm__ volatile ("lgdt %0" : : "m"(gdt_ptr));
    
    // Przeładuj segmenty
    __asm__ volatile (
        "pushq $0x08\n"
        "lea 1f(%%rip), %%rax\n"
        "pushq %%rax\n"
        "lretq\n"
        "1:\n"
        "mov $0x10, %%ax\n"
        "mov %%ax, %%ds\n"
        "mov %%ax, %%es\n"
        "mov %%ax, %%fs\n"
        "mov %%ax, %%gs\n"
        "mov %%ax, %%ss\n"
        ::: "rax", "memory"
    );
}

// ==================== IDT ====================
typedef struct {
    uint16_t offset_low;
    uint16_t selector;
    uint8_t ist;
    uint8_t type_attr;
    uint16_t offset_mid;
    uint32_t offset_high;
    uint32_t zero;
} __attribute__((packed)) idt_entry_t;

typedef struct {
    uint16_t limit;
    uint64_t base;
} __attribute__((packed)) idtr_t;

#define IDT_ENTRIES 256
idt_entry_t idt[IDT_ENTRIES];
idtr_t idtr;

void idt_set_gate(uint8_t num, uint64_t handler, uint8_t dpl) {
    idt[num].offset_low = handler & 0xFFFF;
    idt[num].offset_mid = (handler >> 16) & 0xFFFF;
    idt[num].offset_high = (handler >> 32) & 0xFFFFFFFF;
    idt[num].selector = SEG_KERNEL_CODE;
    idt[num].ist = 0;
    idt[num].type_attr = 0x8E | (dpl << 5);
    idt[num].zero = 0;
}

// ==================== Syscall Handler ====================
// UPROSZCZONA wersja - bez swapgs na razie (wymaga MSRs)
extern void syscall_handler_asm(void);
__asm__(
    ".global syscall_handler_asm\n"
    "syscall_handler_asm:\n"
    "    pushq %rbp\n"
    "    movq %rsp, %rbp\n"
    "    pushq %rbx\n"
    "    pushq %r12\n"
    "    pushq %r13\n"
    "    pushq %r14\n"
    "    pushq %r15\n"
    "    call syscall_handler\n"
    "    popq %r15\n"
    "    popq %r14\n"
    "    popq %r13\n"
    "    popq %r12\n"
    "    popq %rbx\n"
    "    popq %rbp\n"
    "    iretq\n"
);

#define SYS_EXIT 0
#define SYS_WRITE 1
#define SYS_READ 2

void syscall_handler(void) {
    uint64_t syscall_num, arg1, arg2, arg3;

    __asm__ volatile (
        "movq %%rax, %0\n"
        "movq %%rdi, %1\n"
        "movq %%rsi, %2\n"
        "movq %%rdx, %3\n"
        : "=r"(syscall_num), "=r"(arg1), "=r"(arg2), "=r"(arg3)
    );

    switch (syscall_num) {
        case SYS_WRITE:
            if (arg1 == 1 || arg1 == 2) {
                const char* str = (const char*)arg2;
                for (size_t i = 0; i < arg3; i++)
                    putchar(str[i]);
            }
            break;

        case SYS_EXIT:
            print("\n[EXIT: ");
            uint64_t code = arg1;
            if (code == 0) putchar('0');
            else {
                char buf[20];
                int i = 0;
                while (code) {
                    buf[i++] = '0' + (code % 10);
                    code /= 10;
                }
                while (i > 0) putchar(buf[--i]);
            }
            print("]\n");
            __asm__ volatile ("cli; hlt");
            break;
    }
}

// ==================== PIC ====================
void init_pic(void) {
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
}

void init_idt(void) {
    memset(idt, 0, sizeof(idt));
    idt_set_gate(0x80, (uint64_t)syscall_handler_asm, 3); // DPL=3 dla userspace
    idtr.limit = sizeof(idt) - 1;
    idtr.base = (uint64_t)&idt;
    __asm__ volatile ("lidt %0" : : "m"(idtr));
    __asm__ volatile ("sti");
}

// ==================== Userspace Loading ====================
extern uint8_t _binary_build_userspace_raw_bin_start[];
extern uint8_t _binary_build_userspace_raw_bin_end[];

#define USER_STACK_SIZE 0x10000

static uint8_t user_stack[USER_STACK_SIZE] __attribute__((aligned(16)));

void jump_to_userspace(void) {
    uint8_t* userspace_code = _binary_build_userspace_raw_bin_start;
    size_t size = (size_t)(_binary_build_userspace_raw_bin_end - _binary_build_userspace_raw_bin_start);
    
    memcpy((void*)0x400000, userspace_code, size);
    
    serial_print("[Kernel] Userspace loaded, jumping to ring 3...\n");

    uint64_t user_rip = 0x400000;
    uint64_t user_rsp = (uint64_t)user_stack + USER_STACK_SIZE - 8;

    // Iretq do ring 3
    __asm__ volatile (
        "pushq $0x20\n"           // SS (user data)
        "pushq %0\n"              // RSP
        "pushfq\n"                // RFLAGS
        "pushq $0x18\n"           // CS (user code)
        "pushq %1\n"              // RIP
        "iretq\n"
        :
        : "r"(user_rsp), "r"(user_rip)
        : "memory"
    );
}

// ==================== Main Entry ====================
void kernel_main(void) {
    clear_screen();
    serial_init();

    serial_print("=== CosinusOS Microkernel Boot ===\n");
    print("CosinusOS Microkernel\n");
    print("=====================\n\n");

    init_gdt();
    print("[OK] GDT\n");

    init_pic();
    print("[OK] PIC\n");

    init_idt();
    print("[OK] IDT\n");

    print("\nLoading userspace...\n\n");

    jump_to_userspace();

    print("\n[PANIC] Userspace returned!\n");

    while (1) {
        __asm__ volatile ("hlt");
    }
}