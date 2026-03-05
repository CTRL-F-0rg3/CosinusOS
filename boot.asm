bits 32

section .multiboot
align 8
multiboot_header:
    dd 0xE85250D6
    dd 0
    dd header_end - multiboot_header
    dd -(0xE85250D6 + 0 + (header_end - multiboot_header))
    dw 0
    dw 0
    dd 8
header_end:

section .boot
global _start
extern kernel_main

_start:
    cli

    ; Zachowaj magic i info od GRUB
    mov [mb_magic_save], eax
    mov [mb_info_save],  ebx

    ; Tymczasowy stos (z dala od page tables pod 0x1000)
    mov esp, 0x90000

    ; ── Sprawdź long mode ───────────────────────────────────────
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb  .no_longmode
    mov eax, 0x80000001
    cpuid
    test edx, (1 << 29)
    jz  .no_longmode

    ; ── Zeruj page tables 0x1000–0x6000 ────────────────────────
    mov edi, 0x1000
    xor eax, eax
    mov ecx, 5 * 1024           ; 5 × 4KB = 20KB
    rep stosd

    ; P4[0]   → P3lo @ 0x2000  (identity 0–1GB)
    mov dword [0x1000 +   0*8], 0x2003
    ; P4[1]   → P3hi @ 0x3000  (identity 1–2GB)
    mov dword [0x1000 +   1*8], 0x3003
    ; P4[256] → P3lo mirror (kernel higher-half alias)
    mov dword [0x1000 + 256*8], 0x2003

    ; P3lo[0] → P2a @ 0x4000
    mov dword [0x2000], 0x4003
    ; P3hi[0] → P2b @ 0x5000
    mov dword [0x3000], 0x5003

    ; P2a: huge pages 0–1GB (512 × 2MB)
    mov edi, 0x4000
    mov eax, 0x83               ; PS|RW|P
    mov ecx, 512
.fill_p2a:
    mov [edi], eax
    mov dword [edi+4], 0        ; wyczyść górne 32 bity wpisu
    add eax, 0x200000
    add edi, 8
    dec ecx
    jnz .fill_p2a

    ; P2b: huge pages 1–2GB
    mov edi, 0x5000
    mov eax, 0x40000083
    mov ecx, 512
.fill_p2b:
    mov [edi], eax
    mov dword [edi+4], 0
    add eax, 0x200000
    add edi, 8
    dec ecx
    jnz .fill_p2b

    ; ── Włącz PAE ───────────────────────────────────────────────
    mov eax, cr4
    or  eax, (1 << 5)
    mov cr4, eax

    ; ── Załaduj CR3 ─────────────────────────────────────────────
    mov eax, 0x1000
    mov cr3, eax

    ; ── EFER.LME ────────────────────────────────────────────────
    mov ecx, 0xC0000080
    rdmsr
    or  eax, (1 << 8)
    wrmsr

    ; ── Włącz paging ────────────────────────────────────────────
    mov eax, cr0
    or  eax, (1 << 31) | 1
    mov cr0, eax

    ; ── Załaduj GDT i skocz do 64-bit ───────────────────────────
    ; GDT musi być adresowany fizycznie w 32-bit PM!
    ; Używamy LEAQ-style przez wartość absolutną 32-bit
    mov eax, gdt64_ptr_low
    lgdt [eax]
    jmp  0x08:.longmode64

.no_longmode:
    hlt
    jmp .no_longmode

; ─────────────────────────────────────────────────────────────────────
bits 64
.longmode64:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; Właściwy 64-bitowy stos kernelowy
    mov rsp, stack_top

    ; Przeładuj GDT przez pełny 64-bitowy wskaźnik
    ; (teraz jesteśmy w long mode więc 64-bit base w GDT ptr jest używane)
    lgdt [gdt64_ptr]

    ; Zero-extend magic i info → rdi, rsi
    mov eax, [mb_magic_save]
    mov ecx, [mb_info_save]
    mov edi, eax
    mov esi, ecx

    ; ── Włącz SSE/SSE2 (CR4.OSFXSR + CR4.OSXMMEXCPT) ──────────────────
    mov rax, cr4
    or  rax, (1 << 9) | (1 << 10)
    mov cr4, rax
    ; ────────────────────────────────────────────────────────────────
    call kernel_main

.hang:
    cli
    hlt
    jmp .hang

; ─────────────────────────────────────────────────────────────────────
; GDT — umieszczona w .boot sekcji (mapowana od 0x101000)
; KRYTYCZNE: oba wskaźniki (32-bit i 64-bit) muszą wskazywać
; na te same deskryptory
; ─────────────────────────────────────────────────────────────────────
section .boot
align 8

gdt64:
    dq 0                                            ; null descriptor
    dq (1<<43)|(1<<44)|(1<<47)|(1<<53)              ; 0x08 kernel code 64-bit
    dq (1<<41)|(1<<44)|(1<<47)                      ; 0x10 kernel data

; Wskaźnik GDT dla lgdt w trybie 32-bit PM
; base musi być 32-bitowym adresem fizycznym — działa bo kernel < 4GB
gdt64_ptr_low:
    dw gdt64_ptr_low - gdt64 - 1   ; limit = rozmiar GDT - 1
    dd gdt64                        ; 32-bitowy adres fizyczny (wystarczy < 4GB)

; Wskaźnik GDT dla przeładowania w trybie 64-bit
gdt64_ptr:
    dw gdt64_ptr_low - gdt64 - 1   ; ten sam limit
    dq gdt64                        ; pełny 64-bitowy adres

; Zachowane wartości Multiboot
align 4
mb_magic_save: dd 0
mb_info_save:  dd 0

; ─────────────────────────────────────────────────────────────────────
section .bss
align 16
stack:
    resb 65536          ; 64KB boot stack
stack_top: