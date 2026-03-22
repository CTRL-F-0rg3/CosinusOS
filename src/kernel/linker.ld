OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)
SECTIONS {
    . = 0x0000000000101000;

    .boot : {
        KEEP(*(.multiboot))
        KEEP(*(.multiboot2))
        KEEP(*(.boot))
        *boot.o(.text)
    }

    .text : ALIGN(16) {
        /* Trampoliny muszą być w .text razem z resztą kernela */
        *tramp.o(.text)
        *enter_userspace.o(.text)
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        __data_start = .;
        *(.data .data.*)
        *(.got)
        *(.got.plt)
        __data_end = .;
    }

    .bss : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        __bss_end = .;
    }

    .userspace : ALIGN(4096) {
        _userspace_blob_start = .;
        KEEP(*(.userspace_blob))
        _userspace_blob_end = .;
    }

    /DISCARD/ : {
        *(.plt) *(.eh_frame)
        *(.note*) *(.comment*) *(.debug*)
        *(.gnu.hash) *(.dynsym) *(.dynstr)
        *(.dynamic) *(.interp)
    }
}
