; mm.asm Cosinus os amd driver
; mm.asm - Memory Management & TLB Control
; Handles VRAM aperture and GART synchronization.

section .text

global amdgpu_vm_flush_tlb
global amdgpu_set_vram_base

; RDI = MMIO Base Address
; ESI = VM Hub (0 for Graphics, 1 for Multimedia/SDMA)
amdgpu_vm_flush_tlb:
    ; On AMD (GCN/RDNA), flushing involves writing to VM_INVALIDATE_ENG registers
    ; This is a simplified stub. Usually, you write 1 to the bitmask of the engine.
    mov r8, rdi
    add r8, 0x1400          ; Example offset for VM_INVALIDATE_CONTROL
    mov eax, 1
    shl eax, cl             ; CL would hold the engine ID
    mov [r8], eax
    mfence
    
    ; Poll for completion
.wait_flush:
    mov eax, [r8]
    test eax, eax           ; Wait until the hardware clears the bit
    jnz .wait_flush
    ret

; Defines the start of the BAR aperture in the GPU's internal address space
amdgpu_set_vram_base:
    ; Often involves setting MC_VM_FB_LOCATION
    ; RDI = MMIO Base, RSI = Start Address, RDX = End Address
    ret