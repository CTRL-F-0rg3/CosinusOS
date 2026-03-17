; CosinusOS — enter_userspace.asm
; Czysty przeskok ring-0 → ring-3 bez historii schedulera.
;
; Wywołanie (C ABI, SysV x86-64):
;   enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64)
;   rdi = entry  — adres _start userspace
;   rsi = stack  — szczyt user stosu (wyrównany do 16B)
;   rdx = arg    — argument dla _start (rdi w userspace)
;   rcx = cr3    — page table userspace
;
; Co robi:
;   1. Ładuje user CR3
;   2. Ustawia user segmenty (przez iretq, NIE przez mov ds)
;   3. Czyści wszystkie rejestry
;   4. iretq → ring-3

[bits 64]
global enter_userspace
section .text

enter_userspace:
    ; Wyłącz przerwania na czas przeskoku
    cli

    ; Załaduj user CR3
    mov cr3, rcx

    ; Zapisz argumenty w bezpiecznych rejestrach
    ; rdi = entry, rsi = stack, rdx = arg
    ; (rdx trzymamy jako arg dla userspace)
    mov r8,  rdi   ; entry
    mov r9,  rsi   ; stack (RSP dla ring-3)
    mov r10, rdx   ; arg → będzie rdi w userspace

    ; Wyczyść rejestry które nie są potrzebne
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor esi, esi
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    xor ebp,  ebp

    ; Zbuduj iretq ramkę na AKTUALNYM stosie kernelowym
    ; iretq pop-uje: RIP, CS, RFLAGS, RSP, SS
    push 0x23        ; SS  = user data (GDT[4] | RPL=3)
    push r9          ; RSP = user stack top
    push 0x202       ; RFLAGS = IF=1, reserved=1
    push 0x1B        ; CS  = user code (GDT[3] | RPL=3)
    push r8          ; RIP = entry

    ; Ustaw argument dla _start
    mov rdi, r10

    ; Debug: wyślij 'E' na port 0xE9 (QEMU debugcon)
    mov dx,  0xE9
    mov al,  0x45    ; 'E' = Enter userspace
    out dx,  al

    ; Skocz do userspace
    ; iretq zmieni CPL 0→3, załaduje SS/RSP/CS/RIP z ramki
    ; Przerwania zostaną włączone przez RFLAGS.IF=1
    iretq
