/* CosinusOS — userspace linker script
 * Produkuje ELF64 ET_EXEC załadowany pod 0x400000.
 * Kernel load_elf64 czyta e_entry z nagłówka — _start nie musi być
 * pierwszą funkcją w .text, ale wymuszamy to dla czytelności.
 */

OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS {
    . = 0x0000000000400000;

    /* .text._start MUSI być pierwszy — przy opt-level=z Rust przestawia
     * funkcje i _start może wylądować gdziekolwiek w .text.
     * Dla ELF to nie ma znaczenia (entry z nagłówka), ale dla flat binary
     * byłoby krytyczne. Zostawiamy dla porządku. */
    .text : {
        KEEP(*(.text._start))
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
    }

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    . = ALIGN(4096);

    /DISCARD/ : {
        *(.eh_frame)
        *(.note*)
        *(.comment*)
        *(.gnu.hash)
        *(.dynsym)
        *(.dynstr)
        *(.rela*)
        *(.dynamic)
        *(.got.plt)
        *(.plt)
        *(.interp)
    }
}