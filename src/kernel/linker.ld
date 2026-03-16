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

    /* ── Embedded userspace blob (opcjonalne) ──────────────────────────────
     * Gdy userspace_blob.o jest dołączony do linkowania (przez objcopy),
     * _userspace_blob_start/_end wskazują na binarke userspace.
     * Gdy plik nie jest dołączony, obie etykiety mają tę samą wartość
     * (start == end) i load_embedded() zwraca false.
     *
     * Jak wygenerować userspace_blob.o:
     *   objcopy -I binary -O elf64-x86-64 \
     *       --rename-section .data=.userspace_blob \
     *       build/userspace.bin build/userspace_blob.o
     * ─────────────────────────────────────────────────────────────────── */
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
