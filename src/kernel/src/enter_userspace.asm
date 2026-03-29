; CosinusOS — enter_userspace.asm
; ring-0 → ring-3 transition via iretq
; Selectors: CS=0x1B (GDT[3]|RPL3), SS=0x23 (GDT[4]|RPL3)

[bits 64]
global enter_userspace
global eu_stack_top

section .bss
align 16
eu_stack:     resb 4096
eu_stack_top:

section .text

; void enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64)
;                      rdi           rsi          rdx        rcx
enter_userspace:
    cli

    ; Switch to our own ring-0 stack (in kernel .bss, identity-mapped)
    lea rsp, [rel eu_stack_top]

    ; Stash params before we clobber registers
    mov r8,  rdi    ; entry point
    mov r9,  rsi    ; user RSP
    mov r10, rdx    ; arg (→ rdi after iretq)
    mov r11, rcx    ; user CR3

    ; Build iretq frame NOW, while still on kernel CR3 so eu_stack is reachable.
    ; After mov cr3, r11 the identity-mapped kernel .bss (eu_stack) may not be
    ; accessible if the user P4 has P4[0] cleared (which it should for safety).
    ; Layout (iretq pops in order: RIP CS RFLAGS RSP SS):
    push 0x23       ; SS  = user data selector (GDT[4] | RPL3)
    push r9         ; RSP = user stack top
    push 0x202      ; RFLAGS: IF=1, bit1 always 1
    push 0x1B       ; CS  = user code selector (GDT[3] | RPL3)
    push r8         ; RIP = user entry point

    ; Switch to user CR3 — from this point eu_stack may be unreachable,
    ; but we no longer need it: the iretq frame is already on rsp.
    mov cr3, r11

    ; Zero all GPRs visible in ring-3 (except rdi = arg, rsp implicit)
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    mov rdi, r10    ; pass arg as first param (System V ABI)
    xor r8d,  r8d
    xor r9d,  r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    xor ebp,  ebp

    ; Debug: 'E' = executing iretq
    mov dx, 0xE9
    mov al, 0x45
    out dx, al

    iretq
    ; If we somehow return (we shouldn't), hang
.hang:
    cli
    hlt
    jmp .hang