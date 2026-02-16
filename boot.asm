section .multiboot
align 8
multiboot_header:
    dd 0xE85250D6                ; magic
    dd 0                         ; architecture (i386)
    dd header_end - multiboot_header
    dd -(0xE85250D6 + 0 + (header_end - multiboot_header))

    ; end tag
    dw 0
    dw 0
    dd 8
header_end:

section .text
global _start
extern kernel_main

_start:
    cli

    ; ustaw stack
    lea rsp, [rel stack_top]

    ; wywołaj Rust kernel
    call kernel_main

.hang:
    hlt
    jmp .hang

section .bss
align 16
stack:
    resb 16384
stack_top: