[bits 64]
global tramp_k
global tramp_u
section .text

; ── tramp_k: kernel thread entry ─────────────────────────────────────────────
; r14 = entry point (fn(u64) -> !)
; r15 = arg
tramp_k:
    mov     rdi, r15
    call    r14
    cli
    hlt
    jmp     $

; ── tramp_u: userspace thread entry ──────────────────────────────────────────
; r14 = entry point (RIP)
; r13 = user stack top (RSP)
; r15 = arg (→ rdi)
tramp_u:
    cli

    ; Załaduj segmenty danych userspace (DPL=3)
    mov     ax, 0x23        ; user data selektor (GDT[4] | RPL=3)
    mov     ds, ax
    mov     es, ax
    mov     fs, ax
    mov     gs, ax

    ; Zbuduj ramkę iretq:
    ; SS, RSP, RFLAGS, CS, RIP
    push    0x23            ; SS
    push    r13             ; RSP user
    push    0x202           ; RFLAGS (IF=1)
    push    0x1B            ; CS user
    push    r14             ; RIP

    mov     rdi, r15
    xor     rax, rax
    xor     rbx, rbx
    xor     rcx, rcx
    xor     rdx, rdx
    xor     rsi, rsi
    xor     r8,  r8
    xor     r9,  r9
    xor     r10, r10
    xor     r11, r11
    xor     r12, r12
    xor     r13, r13
    xor     r14, r14
    xor     r15, r15
    xor     rbp, rbp

    iretq