/* devspace/linker.ld — DevSpace Ring-1 ELF linker script
 *
 * Load address: 0x500000 (after kernel at ~0x100000, userspace at 0x400000)
 * The kernel loads this ELF via multiboot2 module, maps it into a Ring-1
 * address space, sets IOPL=1 in EFLAGS, and jumps to _start.
 *
 * We keep full ELF structure (not stripped to raw binary) so the kernel
 * can parse PT_LOAD segments and set up page tables correctly.
 */

ENTRY(_start)

SECTIONS
{
    . = 0x0000000000500000;

    /* ── Code ──────────────────────────────────────────────────────────────── */
    .text ALIGN(4096) : {
        *(.text._start)     /* entry point first */
        *(.text*)
    }

    /* ── Read-only data ────────────────────────────────────────────────────── */
    .rodata ALIGN(4096) : {
        *(.rodata*)
    }

    /* ── Read-write data ───────────────────────────────────────────────────── */
    .data ALIGN(4096) : {
        *(.data*)
    }

    /* ── BSS ───────────────────────────────────────────────────────────────── */
    .bss ALIGN(4096) : {
        __bss_start = .;
        *(.bss*)
        *(COMMON)
        __bss_end = .;
    }

    /* ── Stack (8KB, defined here so linker knows the size) ────────────────── */
    .stack ALIGN(4096) : {
        __stack_bottom = .;
        . += 0x2000;    /* 8 KB */
        __stack_top = .;
    }

    /* Discard debug and exception handling sections */
    /DISCARD/ : {
        *(.eh_frame*)
        *(.debug*)
        *(.comment)
        *(.note*)
    }
}
