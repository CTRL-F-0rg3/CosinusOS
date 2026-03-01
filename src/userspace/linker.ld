OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);   /* R + X */
    data PT_LOAD FLAGS(6);   /* R + W */
}

SECTIONS
{
    . = 0x0000000000400000;

    /* ===================== TEXT ===================== */

    .text : ALIGN(4K)
    {
        *(.text .text.*)
    } :text

    /* ===================== RODATA ===================== */

    .rodata : ALIGN(4K)
    {
        *(.rodata .rodata.*)
    } :text

    /* ===================== DATA ===================== */

    .data : ALIGN(4K)
    {
        __data_start = .;
        *(.data .data.*)
        __data_end = .;
    } :data

    /* ===================== BSS ===================== */

    .bss : ALIGN(4K)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    } :data

    /* ===================== END ===================== */

    __kernel_end = .;

    /* ===================== DISCARD ===================== */

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.note*)
        *(.comment*)
        *(.gnu*)
        *(.dyn*)
        *(.plt*)
        *(.interp)
        *(.got.plt)
    }
}