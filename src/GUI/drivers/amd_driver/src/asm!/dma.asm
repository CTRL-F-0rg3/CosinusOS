; dma.asm Cosinus os amd driver
BITS 64
SECTION .data
src_buffer: times 4096 db 0
dst_buffer: times 4096 db 0
SECTION .text
global dma_start
; init 
dma_init:
    mov rax, 0x1
    mov [DMA_CONTROL_REG], rax 
.wait_reset :
    mov rax, [DMA_STATUS_REG]
    test rax, [DMA_STATUS_REG]
    jnz .wait_reset
    ret
; transfer 
dma_transfer:
    mov [DMA_SRC_ADDR], rdi
    mov [DMA_DST_ADDR], rsi
    mov [DMA_SIZE], rdx
    ; start dma
    mov rax, 0x1
    mov [DMA_CONTROL_REG], rax
.wait_done:
    mov rax, [DMA_STATUS_REG]
    test rax, 0x2
    jz .wait_done
    ret
dma_start:
    call dma_init
    lea rdi, [rel src_buffer]
    lea rsi, [rel dst_buffer]
    mov rdx, 4096
    call dma_transfer
    ret
SECTION .bss
DMA_SRC_ADDR:    resq 1
DMA_DST_ADDR:    resq 1
DMA_SIZE:        resq 1
DMA_CONTROL_REG: resq 1
DMA_STATUS_REG:  resq 1