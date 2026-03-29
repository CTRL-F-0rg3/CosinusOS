; dma.asm Cosinus os amd driver
; dma.asm - Operacje na wskaźnikach kołowych
section .text

global update_gpu_wptr

; RDI = adres rejestru WPTR (MMIO)
; RSI = nowa wartość wskaźnika
update_gpu_wptr:
    mov [rdi], esi
    sfence          ; Store fence - ważne przed powiadomieniem GPU o nowych danych
    ret