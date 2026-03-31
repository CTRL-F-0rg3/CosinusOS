; CosinusOS — enter_userspace.asm
; ring-0 → ring-3 transition via iretq
; Selectors: CS=0x1B (GDT[3]|RPL3), SS=0x23 (GDT[4]|RPL3)
;
; CR3 strategy: caller sets THREADS[2].cr3 before calling us.
; Scheduler loaded it already (SCHED -> userspace means cr3 is active).
; We do NOT switch CR3 here — it was already loaded by the scheduler
; context switch before jumping here. eu_stack is on kernel .bss,
; which is in the current (user) CR3 via the full K_P4 copy.

[bits 64]
global enter_userspace
global eu_stack_top

section .bss
align 16
eu_stack:     resb 65536
eu_stack_top:

section .text

; void enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64)
;                      rdi           rsi          rdx        rcx
enter_userspace:
    cli

    ; Switch to our private ring-0 stack
    lea rsp, [rel eu_stack_top]

    ; Stash params
    mov r8,  rdi    ; entry point
    mov r9,  rsi    ; user RSP
    mov r10, rdx    ; arg

    ; Load CR3 explicitly — scheduler may or may not have done this
    ; eu_stack is in kernel .bss at a low physical address, identity-mapped
    ; and present in every user P4 (K_P4 full copy). So this is safe.
    mov cr3, rcx
    ; TLB flush serialise
    mov rax, cr3
    mov cr3, rax

    ; Build iretq frame — AFTER CR3 load so eu_stack is accessible
    ; in the new address space (K_P4 copy ensures it is).
    push 0x23               ; SS
    push r9                 ; user RSP
    push 0x202              ; RFLAGS: IF=1, bit1=1
    push 0x1B               ; CS (ring-3)
    push r8                 ; RIP

    ; Zero GPRs visible in ring-3
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    mov rdi, r10
    xor r8d,  r8d
    xor r9d,  r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    xor ebp,  ebp

    mov dx, 0xE9
    mov al, 0x45
    out dx, al

    iretq

.hang:
    cli
    hlt
    jmp .hang