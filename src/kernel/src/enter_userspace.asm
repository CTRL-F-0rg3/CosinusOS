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

    ; Switch to our own ring-0 stack so we don't corrupt the caller's stack
    lea rsp, [rel eu_stack_top]

    ; Load user CR3
    mov cr3, rcx

    ; Stash params before we clobber registers
    mov r8,  rdi    ; entry point
    mov r9,  rsi    ; user RSP
    mov r10, rdx    ; arg (→ rdi after clear)

    ; Zero all GPRs that will be visible in ring-3
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor edi, edi
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    xor ebp,  ebp

    ; Debug: 'A' = about to build iretq frame
    mov dx, 0xE9
    mov al, 0x41
    out dx, al

    ; Build iretq frame on eu_stack (ring-0 stack, safe before CR3 switch was already done)
    ; Layout (top→bottom on stack, iretq pops bottom→top):
    ;   [RSP+32] SS
    ;   [RSP+24] RSP (user)
    ;   [RSP+16] RFLAGS
    ;   [RSP+ 8] CS
    ;   [RSP+ 0] RIP
    push 0x23       ; SS  = user data selector (GDT[4] | RPL3)
    push r9         ; RSP = user stack top
    push 0x202      ; RFLAGS: IF=1, reserved bit 1 always set
    push 0x1B       ; CS  = user code selector (GDT[3] | RPL3)
    push r8         ; RIP = user entry point

    ; Pass arg in rdi (System V ABI first param)
    mov rdi, r10
    xor r8d,  r8d
    xor r9d,  r9d
    xor r10d, r10d

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