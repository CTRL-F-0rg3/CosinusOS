[bits 64]
global tramp_k
global tramp_u
section .text


tramp_u:

    mov rdi, r15


    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor r8d,  r8d
    xor r9d,  r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    xor ebp,  ebp


    mov dx,  0xE9
    mov al,  0x49    ; 'I'
    out dx,  al


    iretq

tramp_k:
    mov rdi, r15     ; arg → rdi
    call r14         
    cli
    hlt
    jmp $