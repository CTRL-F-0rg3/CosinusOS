; io.asm Cosinus os amd driver
; io.asm - Primitivy MMIO dla AMD GPU
section .text

global mmio_read32
global mmio_write32

; RDI = adres bazowy rejestru (zmapowany w pamięci)
; ESI = wartość do zapisu
mmio_write32:
    mov [rdi], esi
    mfence          ; Bariera pamięci - upewnij się, że zapis dotarł do GPU
    ret

; RDI = adres bazowy
mmio_read32:
    mov eax, [rdi]
    mfence
    ret