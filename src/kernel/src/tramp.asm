[bits 64]
global tramp_k
global tramp_u
section .text

tramp_u:
    cli
    mov ax, 0x23
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov rdi, r15

    ; debug: potwierdź dotarcie
    mov dx, 0xE9
    mov al, 0x55    ; 'U'
    out dx, al

    xor rax, rax
    xor rbx, rbx
    xor rcx, rcx
    xor rdx, rdx
    xor rsi, rsi
    xor r8,  r8
    xor r9,  r9
    xor r10, r10
    xor r11, r11
    xor r12, r12
    xor r13, r13
    xor r14, r14
    xor r15, r15
    xor rbp, rbp

    ; debug: tuż przed iretq
    mov dx, 0xE9
    mov al, 0x49    ; 'I'
    out dx, al

    iretq

tramp_k:
    mov rdi, r15
    call r14
    cli
    hlt
    jmp $