; CosinusOS — enter_userspace.asm
; ring-0 → ring-3 transition via iretq
; Selectors: CS=0x1B (GDT[3]|RPL3), SS=0x23 (GDT[4]|RPL3)

[bits 64]
global enter_userspace
global eu_stack_top

section .bss
align 16
eu_stack:     resb 65536   ; 64KB — enough for frame + guard
eu_stack_top:

section .text

; void enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64)
;                      rdi           rsi          rdx        rcx
enter_userspace:
    cli

    ; Switch to our private ring-0 stack (identity-mapped, always accessible)
    lea rsp, [rel eu_stack_top]

    ; Stash all params
    mov r8,  rdi    ; entry point
    mov r9,  rsi    ; user RSP
    mov r10, rdx    ; arg
    mov r11, rcx    ; user CR3

    ; Load user CR3 now — eu_stack is identity-mapped so it stays reachable
    ; as long as new_user_p4 keeps the identity-map entries (P4[0]) in user P4.
    ; We keep P4[0] in user P4 specifically for this window.
    mov cr3, r11

    ; Serialise: flush pipeline after CR3 switch
    mov rax, cr3
    mov cr3, rax

    ; Build iretq frame (CPU reads this at iretq, rsp must be valid in new CR3)
    ; iretq frame layout on stack (top of stack = lowest address):
    ;   +0   RIP
    ;   +8   CS
    ;   +16  RFLAGS
    ;   +24  RSP (user)
    ;   +32  SS
    push 0x23               ; SS
    push r9                 ; user RSP
    push 0x202              ; RFLAGS: IF=1, bit1=1
    push 0x1B               ; CS  (ring-3, GDT[3])
    push r8                 ; RIP

    ; Zero all GPRs that ring-3 will see
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    mov rdi, r10            ; arg → first param
    xor r8d,  r8d
    xor r9d,  r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    xor ebp,  ebp

    ; Confirm we are about to iretq (port 0xE9 = QEMU debug port)
    mov dx,  0xE9
    mov al,  0x45           ; 'E'
    out dx,  al

    iretq

.hang:
    cli
    hlt
    jmp .hang