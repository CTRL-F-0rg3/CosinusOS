bits 32

section .multiboot
align 8
multiboot_header:
    dd 0xE85250D6                           ; magic
    dd 0                                    ; architecture (i386 protected mode)
    dd header_end - multiboot_header        ; header length
    dd -(0xE85250D6 + 0 + (header_end - multiboot_header)) ; checksum
    ; end tag
    dw 0
    dw 0
    dd 8
header_end:

section .text
global _start
extern kernel_main

_start:
    cli
    ; ustaw tymczasowy stack 32-bit
    mov esp, stack_top - 0x10

    ; Sprawdź czy CPU wspiera long mode (CPUID)
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_longmode

    mov eax, 0x80000001
    cpuid
    test edx, (1 << 29)     ; LM bit
    jz .no_longmode

    ; ── Buduj minimalne tablice stron (identity map 0–2GB) ──
    ; Wyzeruj obszar tablic stron (4 strony × 4096 bajtów)
    mov edi, 0x1000
    xor eax, eax
    mov ecx, 4096
    rep stosd

    ; P4[0] → P3 @ 0x2000
    mov dword [0x1000], 0x2003      ; present + writable
    ; P3[0] → P2 @ 0x3000
    mov dword [0x2000], 0x3003
    ; P3[1] → P2 @ 0x4000  (drugi GB)
    mov dword [0x2008], 0x4003

    ; P2: 512 huge pages × 2MB = 1GB (dla 0x3000 i 0x4000)
    mov edi, 0x3000
    mov eax, 0x83               ; present + writable + huge
    mov ecx, 1024               ; 512 + 512 wpisów
.fill_p2:
    mov [edi], eax
    add eax, 0x200000
    add edi, 8
    loop .fill_p2

    ; ── Włącz PAE ──
    mov eax, cr4
    or  eax, (1 << 5)
    mov cr4, eax

    ; ── Załaduj P4 do CR3 ──
    mov eax, 0x1000
    mov cr3, eax

    ; ── Włącz long mode w EFER ──
    mov ecx, 0xC0000080
    rdmsr
    or  eax, (1 << 8)
    wrmsr

    ; ── Włącz paging (CR0.PG) ──
    mov eax, cr0
    or  eax, (1 << 31) | (1 << 0)
    mov cr0, eax

    ; ── Daleki skok do 64-bit code segment ──
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

    ; Ustaw właściwy stack 64-bit
    mov rsp, stack_top

    ; Wywołaj Rust kernel
    call kernel_main

.hang:
    cli
    hlt
    jmp .hang

; ─────────────────────────────────────────────
section .data
align 8

gdt64:
    ; null descriptor
    dq 0
    ; code segment (0x08): execute/read, 64-bit
    dq (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53)
    ; data segment (0x10): read/write
    dq (1 << 41) | (1 << 44) | (1 << 47)

gdt64.ptr:
    dw $ - gdt64 - 1
    dq gdt64

section .bss
align 16
stack:
    resb 16384
stack_top: