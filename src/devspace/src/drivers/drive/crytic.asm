; critical.asm - Optymalizowane transfery dla Ring 1
section .text

global transfer_sector_in
global transfer_sector_out

; RDI = adres bufora docelowego
; DX = port danych (zazwyczaj 0x1F0)
transfer_sector_in:
    push rcx
    mov rcx, 256    ; 256 wordów = 512 bajtów
    rep insw        ; Szybki transfer z portu DX do [RDI]
    pop rcx
    ret

; RSI = adres bufora źródłowego
; DX = port danych (0x1F0)
transfer_sector_out:
    push rcx
    mov rcx, 256
    rep outsw       ; Szybki transfer z [RSI] do portu DX
    pop rcx
    ret

global 400ns_delay
400ns_delay:
    ; ATA wymaga 400ns przerwy (odczyt statusu 4 razy wystarczy)
    mov dx, 0x3F6
    in al, dx
    in al, dx
    in al, dx
    in al, dx
    ret