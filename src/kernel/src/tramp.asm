[bits 64]
global tramp_k
global tramp_u
section .text

; ─────────────────────────────────────────────────────────────────────────────
; tramp_u — trampoline do userspace
;
; Wywoływany przez `ret` z thread_switch gdy wątek startuje po raz pierwszy.
; RSP wskazuje na iretq ramkę na stosie kernelowym:
;
;   [RSP+0 ] RIP    = entry userspace
;   [RSP+8 ] CS     = 0x1B (user code, RPL=3)
;   [RSP+16] RFLAGS = 0x202 (IF=1)
;   [RSP+24] RSP    = user stack top
;   [RSP+32] SS     = 0x23 (user data, RPL=3)
;
; Rejestry po pop-ach z thread_switch:
;   r15 = arg (przekazany do _start jako rdi)
;
; WAŻNE: NIE ładuj DS/ES/FS/GS przed iretq — jesteśmy w ring-0 (CPL=0)
; i ładowanie segmentu z RPL=3 daje #GP.
; W x86-64 long mode DS/ES są ignorowane przez CPU dla operacji pamięci
; więc nie trzeba ich ustawiać. iretq automatycznie zmienia CPL na 3
; i ładuje SS z ramki.
; ─────────────────────────────────────────────────────────────────────────────
tramp_u:
    ; Przenieś arg (r15) do rdi — pierwszy argument _start(arg: u64)
    mov rdi, r15

    ; Wyczyść pozostałe rejestry — userspace startuje z czystym stanem
    ; NIE czyścimy rdi (arg) ani rsp (ustawiany przez iretq)
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

    ; Debug: wyślij 'I' na port 0xE9 (QEMU -debugcon) tuż przed iretq
    ; Dodaj do QEMU: -debugcon file:/tmp/qemu-debugcon.log
    ; Jeśli widzisz 'I' to trampoline dotarł do iretq
    mov dx,  0xE9
    mov al,  0x49    ; 'I'
    out dx,  al

    ; Skocz do userspace — iretq ładuje RIP/CS/RFLAGS/RSP/SS z ramki na stosie
    ; Po iretq: CPL=3, RSP=user stack, RIP=entry, CS=0x1B, SS=0x23
    iretq

; ─────────────────────────────────────────────────────────────────────────────
; tramp_k — trampoline dla wątków kernelowych
;
; Rejestry po pop-ach:
;   r15 = arg
;   r14 = entry (adres funkcji kernelowej)
; ─────────────────────────────────────────────────────────────────────────────
tramp_k:
    mov rdi, r15     ; arg → rdi
    call r14         ; wywołaj entry(arg)
    cli
    hlt
    jmp $