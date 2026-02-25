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
    mov esp, stack_top - 0x10

    ; Zapisz multiboot magic (eax) i info pointer (ebx) na stosie
    ; bo edi/esi będą zniszczone przez page table setup
    push ebx        ; [esp+4] = mb_info
    push eax        ; [esp+0] = mb_magic

    ; Sprawdź long mode
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_longmode
    mov eax, 0x80000001
    cpuid
    test edx, (1 << 29)
    jz .no_longmode

    ; Buduj page tables (identity map 0-4GB przez 2MB huge pages)
    mov edi, 0x1000
    xor eax, eax
    mov ecx, 5 * 1024
    rep stosd

    ; P4[0]   → P3lo @ 0x2000
    mov dword [0x1000],        0x2003
    ; P4[256] → P3hi @ 0x3000
    mov dword [0x1000 + 256*8], 0x3003
    ; P3lo[0] → P2lo @ 0x4000
    mov dword [0x2000], 0x4003
    ; P3lo[1] → P2hi @ 0x5000
    mov dword [0x2008], 0x5003
    ; P3hi[0] → P2lo @ 0x4000 (mirror)
    mov dword [0x3000], 0x4003

    ; P2lo: 512 huge pages 0-1GB
    mov edi, 0x4000
    mov eax, 0x83
    mov ecx, 512
.fill_p2lo:
    mov [edi], eax
    add eax, 0x200000
    add edi, 8
    loop .fill_p2lo

    ; P2hi: 512 huge pages 1-2GB
    mov edi, 0x5000
    mov eax, 0x40000083
    mov ecx, 512
.fill_p2hi:
    mov [edi], eax
    add eax, 0x200000
    add edi, 8
    loop .fill_p2hi

    ; Włącz PAE
    mov eax, cr4
    or  eax, (1 << 5)
    mov cr4, eax

    ; CR3 = P4
    mov eax, 0x1000
    mov cr3, eax

    ; Włącz long mode
    mov ecx, 0xC0000080
    rdmsr
    or  eax, (1 << 8)
    wrmsr

    ; Włącz paging
    mov eax, cr0
    or  eax, (1 << 31) | (1 << 0)
    mov cr0, eax

    ; Przywróć magic i mb_info ze stosu DO rejestrów przed skokiem
    ; (w trybie 32-bit, esp wciąż działa)
    pop eax         ; eax = mb_magic
    pop ecx         ; ecx = mb_info
    ; Zachowaj w ebp/ebx które przeżyją jmp do 64-bit
    mov ebp, eax    ; ebp = mb_magic
    mov ebx, ecx    ; ebx = mb_info

    lgdt [gdt64.ptr]
    jmp  0x08:.longmode64

.no_longmode:
    hlt
    jmp .no_longmode

; ─────────────────────────────────────────────
bits 64
.longmode64:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    mov rsp, stack_top

    ; Przenieś magic i mb_info z ebp/ebx → rdi/rsi (ABI argumenty)
    ; movzx zero-extenduje 32-bit → 64-bit bezpiecznie
    movzx rdi, ebp      ; rdi = mb_magic
    movzx rsi, ebx      ; rsi = mb_info

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
    dq (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53)  ; code 0x08
    dq (1 << 41) | (1 << 44) | (1 << 47)                ; data 0x10

gdt64.ptr:
    dw $ - gdt64 - 1
    dq gdt64

section .bss
align 16
stack:
    resb 32768          ; 32KB stack dla boot
stack_top: