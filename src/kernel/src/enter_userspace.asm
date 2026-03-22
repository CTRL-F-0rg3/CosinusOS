; CosinusOS — enter_userspace.asm
; ring-0 → ring-3, używa własnego tymczasowego stosu

[bits 64]
global enter_userspace
global eu_stack_top
section .bss
align 16
eu_stack: resb 4096         ; 4KB dedykowany stos dla enter_userspace
eu_stack_top:               ; szczyt stosu

section .text

; extern "C" fn enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64) -> !
; rdi=entry, rsi=stack, rdx=arg, rcx=cr3
enter_userspace:
    cli

    ; Przełącz na własny stos — NIE używamy stosu wywołującego
    ; żeby nie niszczyć stosu kterminal ani idle
    lea rsp, [rel eu_stack_top]

    ; Załaduj CR3 userspace
    mov cr3, rcx

    ; Zapisz parametry
    mov r8,  rdi    ; entry
    mov r9,  rsi    ; stack (RSP userspace)
    mov r10, rdx    ; arg

    ; Wyczyść rejestry
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

    ; Zbuduj iretq ramkę na WŁASNYM stosie (eu_stack)
    push 0x23       ; SS
    push r9         ; RSP userspace
    push 0x202      ; RFLAGS: IF=1
    push 0x1B       ; CS user
    push r8         ; RIP entry

    ; Argument
    mov rdi, r10
    xor r8d,  r8d
    xor r9d,  r9d
    xor r10d, r10d

    ; Debug 'E'
    mov dx, 0xE9
    mov al, 0x45
    out dx, al

    iretq