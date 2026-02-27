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

section .text
global _start
extern kernel_main

_start:
    cli

    ; Zachowaj magic i info ZANIM cokolwiek zniszczymy
    ; ebx = mb_info (od GRUB), eax = magic
    mov [mb_magic_save], eax
    mov [mb_info_save],  ebx

    ; Ustaw tymczasowy stos (daleko od page tables które będą pod 0x1000)
    mov esp, 0x90000

    ; Sprawdź czy CPU wspiera long mode
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_longmode
    mov eax, 0x80000001
    cpuid
    test edx, (1 << 29)
    jz .no_longmode

    ; ── Zeruj obszar page tables (0x1000 - 0x6000) ──
    mov edi, 0x1000
    xor eax, eax
    mov ecx, 5 * 1024       ; 5 stron × 1024 dwordów = 5 × 4KB
    rep stosd

    ; P4[0]    → P3lo @ 0x2000  (0 - 1GB)
    mov dword [0x1000 +   0*8], 0x2003
    ; P4[1]    → P3hi @ 0x3000  (1 - 2GB)
    mov dword [0x1000 +   1*8], 0x3003
    ; P4[256]  → P3lo (mirror dla kernel space wyżej)
    mov dword [0x1000 + 256*8], 0x2003

    ; P3lo[0] → P2a @ 0x4000  (0 - 1GB: wpisy 0-511)
    mov dword [0x2000 + 0*8], 0x4003
    ; P3hi[0] → P2b @ 0x5000  (1 - 2GB)
    mov dword [0x3000 + 0*8], 0x5003

    ; P2a: identity map 0–1GB (512 × 2MB huge pages)
    mov edi, 0x4000
    mov eax, 0x83           ; G=0 | PS=1 | R/W=1 | P=1
    mov ecx, 512
.fill_p2a:
    mov [edi], eax
    add eax, 0x200000
    add edi, 8
    loop .fill_p2a

    ; P2b: identity map 1–2GB
    mov edi, 0x5000
    mov eax, 0x40000083     ; base 1GB
    mov ecx, 512
.fill_p2b:
    mov [edi], eax
    add eax, 0x200000
    add edi, 8
    loop .fill_p2b

    ; Włącz PAE
    mov eax, cr4
    or  eax, (1 << 5)
    mov cr4, eax

    ; Załaduj P4
    mov eax, 0x1000
    mov cr3, eax

    ; Włącz long mode (EFER.LME)
    mov ecx, 0xC0000080
    rdmsr
    or  eax, (1 << 8)
    wrmsr

    ; Włącz paging + protected mode
    mov eax, cr0
    or  eax, (1 << 31) | 1
    mov cr0, eax

    lgdt [gdt64.ptr]
    jmp  0x08:.longmode64

.no_longmode:
    hlt
    jmp .no_longmode

; ─────────────────────────────────────────────
bits 64
.longmode64:
    ; Załaduj data segmenty
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; Ustaw właściwy 64-bit stos
    mov rsp, stack_top

    ; Wczytaj zachowane wartości (zero-extend 32→64)
    mov eax, [mb_magic_save]
    mov ecx, [mb_info_save]
    movzx rdi, eax   ; arg1 = magic
    movzx rsi, ecx   ; arg2 = mb_info ptr

    call kernel_main

.hang:
    cli
    hlt
    jmp .hang

; ─────────────────────────────────────────────
section .data
align 8

gdt64:
    dq 0
    dq (1<<43)|(1<<44)|(1<<47)|(1<<53)  ; 0x08 kernel code 64-bit
    dq (1<<41)|(1<<44)|(1<<47)           ; 0x10 kernel data

gdt64.ptr:
    dw $ - gdt64 - 1
    dq gdt64

mb_magic_save: dd 0
mb_info_save:  dd 0

section .bss
align 16
stack:
    resb 65536   ; 64KB boot stack (daleko od page tables)
stack_top: