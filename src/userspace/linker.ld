OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

SECTIONS {
    . = 0x0000000000400000;

    .text   : { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data   : { *(.data .data.*) }
    .bss    : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }
    . = ALIGN(4096);

    /DISCARD/ : {
        *(.eh_frame) *(.note*) *(.comment*)
        *(.gnu.hash) *(.dynsym) *(.dynstr)
        *(.rela*) *(.dynamic) *(.got.plt)
        *(.plt) *(.interp)
    }
}
