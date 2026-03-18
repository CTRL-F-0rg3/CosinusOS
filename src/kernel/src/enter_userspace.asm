[bits 64]
global enter_userspace
section .text

; enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64)
;   rdi = entry
;   rsi = stack (user RSP)  
;   rdx = arg
;   rcx = cr3
;
; Problem: timer IRQ może odpalić się i użyć tego samego stosu kernelowego
; co zniszczyłoby iretq ramkę.
;
; Rozwiązanie: przełącz na dedykowany, izolowany stos zanim zbudujesz ramkę.
; Używamy stosu userspace (r9) jako tymczasowego stosu kernelowego —
; jest zmapowany w obu CR3 (K_P4 zawiera mapowania user stacków przez new_user_p4).
; Po iretq CPU przełączy się na właściwy user RSP z ramki.

enter_userspace:
    cli

    ; Zapisz argumenty w callee-saved rejestrach
    mov r12, rdi    ; entry
    mov r13, rsi    ; stack (user RSP)
    mov r14, rdx    ; arg
    mov r15, rcx    ; cr3

    ; Przełącz na user stack jako tymczasowy stos kernelowy
    ; User stack jest identity-mapped w K_P4 (new_user_p4 kopiuje K_P4)
    ; więc jest dostępny przed zmianą CR3
    ; Ustaw RSP na środek user stacka żeby mieć miejsce na ramkę
    mov rsp, r13

    ; Wyrównaj RSP do 16 bajtów
    and rsp, ~0xF

    ; Zbuduj iretq ramkę
    push 0x23       ; SS
    push r13        ; RSP user (oryginalna wartość)
    push 0x202      ; RFLAGS (IF=1)
    push 0x1B       ; CS user
    push r12        ; RIP = entry

    ; Zmień CR3
    mov cr3, r15

    ; Argument
    mov rdi, r14

    ; Wyczyść
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

    iretq