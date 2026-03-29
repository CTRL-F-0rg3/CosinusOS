; pcie.asm Cosinus os amd driver
; pcie.asm - Dostęp do Config Space (metoda 0xCF8/0xCFC)
section .text

global pcie_config_read32

; RDI = bus, RSI = slot, RDX = func, RCX = offset
pcie_config_read32:
    mov eax, 0x80000000
    shl rdi, 16
    or eax, edi         ; Bus
    shl rsi, 11
    or eax, esi         ; Slot
    shl rdx, 8
    or eax, edx         ; Func
    and ecx, 0xFC
    or eax, ecx         ; Offset
    
    mov dx, 0xCF8
    out dx, eax         ; Adresuj
    mov dx, 0xCFC
    in eax, dx          ; Czytaj dane
    ret