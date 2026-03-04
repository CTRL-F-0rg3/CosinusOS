; debug_tramp.asm — tymczasowy patch do tramp.asm
; Dodaje breakpoint (int3) na początku tramp_u żeby GDB zatrzymał się zanim iretq
; Po debugowaniu usuń int3

[bits 64]

global tramp_k
global tramp_u

section .text

tramp_k:
    mov     rdi, r15
    call    r14
    cli
    hlt
    jmp     $

tramp_u:
    ; === TYMCZASOWY BREAKPOINT GDB ===
    ; int3                        ; odkomentuj gdy debugujesz przez GDB
    ; =================================
    cli

    ; Wypisz przez port 0xE9 (QEMU debug port) wartości rejestrów
    ; żeby sprawdzić czy r14/r13/r15 mają właściwe wartości
    ; (QEMU musi być uruchomiony z -debugcon stdio lub -debugcon file:debug.log)
    mov     dx, 0xE9

    ; Wypisz marker "T" żeby wiedzieć że tramp_u wystartował
    mov     al, 0x54            ; 'T'
    out     dx, al
    mov     al, 0x52            ; 'R'
    out     dx, al
    mov     al, 0x41            ; 'A'
    out     dx, al
    mov     al, 0x4D            ; 'M'
    out     dx, al
    mov     al, 0x50            ; 'P'
    out     dx, al
    mov     al, 0x5F            ; '_'
    out     dx, al
    mov     al, 0x55            ; 'U'
    out     dx, al
    mov     al, 0x0A            ; '\n'
    out     dx, al

    ; Wypisz r14 (entry) jako 8 bajtów hex przez port 0xE9
    ; helper: wypisz jeden bajt hex
    ; r14 = entry point userspace
    mov     rax, r14
    call    .print_hex64

    mov     al, 0x0A
    out     dx, al

    ; Wypisz r13 (user rsp)
    mov     rax, r13
    call    .print_hex64

    mov     al, 0x0A
    out     dx, al

    ; Teraz właściwy skok do userspace
    push    0x23                ; SS
    push    r13                 ; RSP
    push    0x202               ; RFLAGS
    push    0x1B                ; CS
    push    r14                 ; RIP
    mov     rdi, r15
    xor     rax, rax
    xor     rbx, rbx
    xor     rcx, rcx
    xor     rdx, rdx
    xor     rsi, rsi
    xor     r8,  r8
    xor     r9,  r9
    xor     r10, r10
    xor     r11, r11
    xor     r12, r12
    xor     r13, r13
    xor     r14, r14
    xor     r15, r15
    xor     rbp, rbp
    iretq

; Wypisz rax jako 16 cyfr hex przez port 0xE9 (dx = 0xE9)
.print_hex64:
    push    rcx
    push    rbx
    mov     rcx, 64             ; 64 bity = 16 cyfr hex
.hex_loop:
    sub     rcx, 4
    mov     rbx, rax
    shr     rbx, cl
    and     rbx, 0xF
    cmp     rbx, 10
    jl      .digit
    add     rbx, 'A' - 10
    jmp     .emit
.digit:
    add     rbx, '0'
.emit:
    mov     al, bl
    out     dx, al
    test    rcx, rcx
    jnz     .hex_loop
    pop     rbx
    pop     rcx
    ret