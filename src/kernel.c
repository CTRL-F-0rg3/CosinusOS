/*
 * CosinusOS Microkernel - Minimalistyczny kernel przełączający do Rust userspace
 */

#include "types.h"

// ============ Port I/O ============
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

// ============ Memory Functions ============
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

// ============ VGA Driver ============
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
    for (int i = 0; i < VGA_WIDTH * VGA_HEIGHT; i++) {
        vga_buffer[i] = ((uint16_t)current_color << 8) | ' ';
    }
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
        // Scroll
        for (int i = 0; i < (VGA_HEIGHT - 1) * VGA_WIDTH; i++) {
            vga_buffer[i] = vga_buffer[i + VGA_WIDTH];
        }
        for (int i = 0; i < VGA_WIDTH; i++) {
            vga_buffer[(VGA_HEIGHT - 1) * VGA_WIDTH + i] = ((uint16_t)current_color << 8) | ' ';
        }
        cursor_y = VGA_HEIGHT - 1;
    }
    
    vga_update_cursor();
}

void print(const char* str) {
    while (*str) {
        putchar(*str++);
    }
}

// ============ Serial Port ============
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
    while (*str) {
        serial_write(*str++);
    }
}

// ============ IDT ============
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

void idt_set_gate(uint8_t num, uint64_t handler) {
    idt[num].offset_low = handler & 0xFFFF;
    idt[num].offset_mid = (handler >> 16) & 0xFFFF;
    idt[num].offset_high = (handler >> 32) & 0xFFFFFFFF;
    idt[num].selector = 0x08;
    idt[num].ist = 0;
    idt[num].type_attr = 0x8E;
    idt[num].zero = 0;
}

// ============ Syscall Handler ============
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
                for (size_t i = 0; i < arg3; i++) {
                    putchar(str[i]);
                }
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

// ============ PIC ============
void init_pic(void) {
    outb(0x20, 0x11);
    io_wait();
    outb(0xA0, 0x11);
    io_wait();
    outb(0x21, 0x20);
    io_wait();
    outb(0xA1, 0x28);
    io_wait();
    outb(0x21, 0x04);
    io_wait();
    outb(0xA1, 0x02);
    io_wait();
    outb(0x21, 0x01);
    io_wait();
    outb(0xA1, 0x01);
    io_wait();
    outb(0x21, 0xFF);
    outb(0xA1, 0xFF);
}

void init_idt(void) {
    memset(idt, 0, sizeof(idt));
    idt_set_gate(0x80, (uint64_t)syscall_handler_asm);
    idtr.limit = sizeof(idt) - 1;
    idtr.base = (uint64_t)&idt;
    __asm__ volatile ("lidt %0" : : "m"(idtr));
    __asm__ volatile ("sti");
}

// ============ Jump to Userspace ============
extern uint8_t _binary_userspace_bin_start;
extern uint8_t _binary_userspace_bin_end;

void jump_to_userspace(void) {
    void (*userspace_main)(void) = (void*)0x400000;
    size_t size = (size_t)(&_binary_userspace_bin_end - &_binary_userspace_bin_start);
    memcpy((void*)0x400000, &_binary_userspace_bin_start, size);
    serial_print("[Kernel] Jumping to userspace\n");
    userspace_main();
}

// ============ Main Entry ============
void _start(void) {
    clear_screen();
    serial_init();
    
    serial_print("=== CosinusOS Boot ===\n");
    print("CosinusOS Kernel\n");
    
    init_pic();
    print("[OK] PIC\n");
    
    init_idt();
    print("[OK] IDT + Syscalls\n");
    
    print("Starting userspace...\n\n");
    
    jump_to_userspace();
    
    print("\n[PANIC] Userspace returned!\n");
    
    // KRYTYCZNE: Nieskończona pętla!
    while (1) {
        __asm__ volatile ("hlt");
    }
}