; gprr.asm Cosinus os amd driver
; gprr.asm - Indirect Register Access for AMD GPU
; Used when registers are not directly MMIO-mapped.

section .text

global amdgpu_indirect_read
global amdgpu_indirect_write

; RDI = MMIO Base Address
; RSI = Index Offset (e.g., mmINDEX)
; RDX = Data Offset (e.g., mmDATA)
; RCX = Target Register Index
amdgpu_indirect_read:
    mov eax, ecx            ; Move target index to EAX
    add rsi, rdi            ; Calculate absolute address of Index reg
    mov [rsi], eax          ; Write index to mmINDEX
    mfence                  ; Ensure write is posted
    
    add rdx, rdi            ; Calculate absolute address of Data reg
    mov eax, [rdx]          ; Read value from mmDATA
    mfence
    ret

; RDI = MMIO Base Address
; RSI = Index Offset
; RDX = Data Offset
; RCX = Target Register Index
; R8D = Value to write
amdgpu_indirect_write:
    mov eax, ecx
    add rsi, rdi
    mov [rsi], eax          ; Set index
    mfence
    
    add rdx, rdi
    mov [rdx], r8d          ; Write data
    mfence
    ret