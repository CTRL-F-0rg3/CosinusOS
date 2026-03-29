; interrupts.asm Cosinus os amd driver
; interrupts.asm - Low-level IRQ handling
section .text

global gpu_irq_handler_stub
extern gpu_rust_handler ; Funkcja w Rust (mod.rs)

gpu_irq_handler_stub:
    push rax
    push rcx
    push rdx
    push rsi
    push rdi
    ; ... zachowaj resztę rejestrów ...
    
    call gpu_rust_handler
    
    ; EOI (End of Interrupt) dla APIC (zakładając x2APIC lub MMIO APIC)
    ; mov rax, [APIC_BASE + 0xB0]
    
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rax
    iretq