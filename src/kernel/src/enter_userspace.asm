; CosinusOS — enter_userspace.asm
; Czysty przeskok ring-0 → ring-3
;
; extern "C" fn enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64) -> !
; rdi = entry  — RIP userspace (_start)
; rsi = stack  — RSP userspace (szczyt stosu)
; rdx = arg    — argument dla _start (→ rdi w userspace)
; rcx = cr3    — page table userspace

[bits 64]
global enter_userspace
section .text

enter_userspace:
    cli

    ; Załaduj CR3 userspace
    mov cr3, rcx

    ; Przełącz SS na user data zanim zmienimy RSP
    ; (mov ss w ring-0 z DPL=0 jest OK, iretq dopiero zmieni CPL)
    ; Uwaga: NIE robimy mov ds/es/fs/gs — to daje #GP w ring-0 z RPL=3
    mov ax, 0x20        ; GDT[4] user data bez RPL (DPL=3 ale RPL=0)
    mov ss, ax          ; Ustaw SS na user data descriptor

    ; Zbuduj iretq ramkę bezpośrednio na aktualnym RSP
    ; (nie wyrównujemy — iretq nie wymaga wyrównania, każdy push jest 8B)
    push 0x23           ; SS  = user data | RPL=3
    push rsi            ; RSP = user stack top
    push 0x202          ; RFLAGS = IF=1, bit1=1 (reserved, zawsze 1)
    push 0x1B           ; CS  = user code | RPL=3
    push rdi            ; RIP = entry (_start)

    ; Argument dla _start — rdi zostanie ustawiony przez iretq na wartość rdx
    ; ALE: po iretq rdi = wartość sprzed push rdi... nie, rdi jest push'owany jako RIP
    ; Musimy ustawić rdi PRZED iretq ale PO push rdi
    ; Trik: użyj r11 jako tymczasowy
    mov r11, rdx        ; arg

    ; Wyczyść rejestry (poza r11 który trzyma arg)
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor edi, edi        ; wyczyść — zaraz ustawimy z r11
    xor r8d,  r8d
    xor r9d,  r9d
    xor r10d, r10d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    xor ebp,  ebp

    ; Ustaw argument tuż przed iretq
    mov rdi, r11
    xor r11d, r11d

    ; Debug: 'E' na port 0xE9 (QEMU debugcon)
    mov dx,  0xE9
    mov al,  0x45
    out dx,  al

    ; Skocz do userspace
    ; iretq pop: RIP, CS(→CPL=3), RFLAGS(→IF=1), RSP, SS
    iretq