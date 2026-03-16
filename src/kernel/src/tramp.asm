[bits 64]
global tramp_k
global tramp_u
section .text

; ─────────────────────────────────────────────────────────────────────────────
; tramp_u — trampoline do userspace
;
; Wywoływany przez `ret` z thread_switch gdy nowy wątek startuje po raz pierwszy.
; W tym momencie RSP wskazuje na iretq ramkę która init_thread_stack
; umieściła na stosie kernelowym:
;
;   [RSP+0 ] RIP    = adres entry userspace
;   [RSP+8 ] CS     = 0x1B (user code, RPL=3)
;   [RSP+16] RFLAGS = 0x202 (IF=1)
;   [RSP+24] RSP    = user stack top
;   [RSP+32] SS     = 0x23 (user data, RPL=3)
;
; Rejestry przed iretq (ustawione przez thread_switch pop-y):
;   r15 = arg (przekazany do _start jako rdi)
;   r14 = entry (już w ramce iretq jako RIP — nie używamy go tutaj)
;   r13 = user stack top (już w ramce iretq jako RSP)
;
; Cel: ustawić segmenty DS/ES/FS/GS na user data (0x23),
;      wyczyścić zbędne rejestry, wykonać iretq.
; ─────────────────────────────────────────────────────────────────────────────
tramp_u:
    ; Wyłącz przerwania na czas przeskoku (iretq je włączy przez RFLAGS.IF=1)
    cli

    ; Ustaw user data segments (0x23 = GDT index 4 | RPL=3)
    mov ax, 0x23
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    ; Argument dla _start(arg: u64) — r15 zawiera arg z init_thread_stack
    mov rdi, r15

    ; Debug: wyślij 'U' na port 0xE9 (QEMU debugcon) żeby potwierdzić dotarcie
    mov dx,  0xE9
    mov al,  0x55    ; 'U'
    out dx,  al

    ; Wyczyść wszystkie rejestry oprócz rdi (arg) i rsp (ustawiany przez iretq)
    ; żeby userspace startował z czystym stanem
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

    ; Debug: tuż przed iretq — 'I' na debugcon
    mov dx,  0xE9
    mov al,  0x49    ; 'I'
    out dx,  al

    ; Skocz do userspace — iretq ładuje RIP/CS/RFLAGS/RSP/SS z ramki na stosie
    iretq

; ─────────────────────────────────────────────────────────────────────────────
; tramp_k — trampoline dla wątków kernelowych
;
; Wywoływany przez `ret` z thread_switch.
; Rejestry po pop-ach:
;   r15 = arg
;   r14 = entry (adres funkcji kernelowej)
;
; Wywołuje entry(arg) i po powrocie hlt-uje (wątek nie powinien wracać).
; ─────────────────────────────────────────────────────────────────────────────
tramp_k:
    mov rdi, r15     ; arg → rdi (pierwszy argument SysV ABI)
    call r14         ; wywołaj entry(arg)
    ; Jeśli funkcja wróci (nie powinna) — zatrzymaj wątek
    cli
    hlt
    jmp $