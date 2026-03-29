; crytic.asm — Critical ATA PIO transfer routines
; Uses REP INSW / REP OUTSW for fast 512-byte sector transfers.
;
; Called from Rust mod.rs via extern "C".
; Runs in Ring-1 (DevSpace) which has IOPL=1 so IN/OUT are permitted.
;
; Calling convention: System V AMD64
;   transfer_sector_in:  rdi=buf*, rsi=port (u16)
;   transfer_sector_out: rdi=buf*, rsi=port (u16)
;   delay_400ns:         (no args, no return)

bits 64
section .text

global transfer_sector_in
global transfer_sector_out
global delay_400ns

; ── transfer_sector_in ────────────────────────────────────────────────────────
; Read 512 bytes (256 words) from ATA data port into buffer.
; rdi = destination buffer pointer
; rsi = port number (typically 0x1F0)
;
; REP INSW reads CX words from port DX into [RDI], incrementing RDI each time.
; Direction flag must be clear (std x86 calling convention — it is).

transfer_sector_in:
    push    rcx
    push    rdi                   ; save buf ptr (INSW modifies rdi)
    mov     rdx, rsi              ; port → dx (INSW uses dx)
    mov     rcx, 256              ; 256 words = 512 bytes
    cld                           ; ensure DF=0 (increment direction)
    rep insw                      ; transfer: port[dx] → [rdi], rdi+=2, cx--
    pop     rdi
    pop     rcx
    ret

; ── transfer_sector_out ───────────────────────────────────────────────────────
; Write 512 bytes (256 words) from buffer to ATA data port.
; rdi = source buffer pointer
; rsi = port number (typically 0x1F0)
;
; REP OUTSW writes CX words from [RSI] to port DX, incrementing RSI each time.

transfer_sector_out:
    push    rcx
    push    rsi                   ; save buf ptr (OUTSW uses rsi as source)
    mov     rdx, rsi              ; port → dx
    mov     rsi, rdi              ; buf ptr → rsi (OUTSW reads from [rsi])
    mov     rcx, 256
    cld
    rep outsw                     ; transfer: [rsi] → port[dx], rsi+=2, cx--
    pop     rsi
    pop     rcx
    ret

; ── delay_400ns ───────────────────────────────────────────────────────────────
; ATA spec requires ≥400ns between drive select write and status read.
; Reading alt-status (0x3F6) four times provides the required delay.
; No args, no return value, clobbers al and dx only.

delay_400ns:
    mov     dx, 0x3F6
    in      al, dx
    in      al, dx
    in      al, dx
    in      al, dx
    ret