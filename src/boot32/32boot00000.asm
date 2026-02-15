; =====================================================================
; BOOTLOADER dla systemu operacyjnego KELNER (optimized)
; Architektura: x86 - mieści się w 512 bajtach
; =====================================================================

BITS 16
ORG 0x7C00

KERNEL_OFFSET equ 0x1000
KERNEL_SECTORS equ 64  ; 64 sektory = 32KB (wystarczy na rozbudowane jądro)

; ===================== START =====================
start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti
    
    mov [boot_drive], dl
    
    ; Załaduj jądro
    mov ah, 0x02
    mov al, KERNEL_SECTORS
    mov ch, 0
    mov cl, 2
    mov dh, 0
    mov dl, [boot_drive]
    mov bx, KERNEL_OFFSET
    int 0x13
    jc error
    
    ; Włącz A20
    mov ax, 0x2401
    int 0x15
    
    ; Załaduj GDT
    cli
    lgdt [gdt_descriptor]
    
    ; Protected mode
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    jmp CODE_SEG:pm_start

error:
    mov si, msg_err
    call print
    jmp $

print:
    mov ah, 0x0E
.loop:
    lodsb
    test al, al
    jz .done
    int 0x10
    jmp .loop
.done:
    ret

boot_drive: db 0
msg_err: db "ERR", 0

; ===================== GDT =====================
gdt_start:
    dq 0x0

gdt_code:
    dw 0xFFFF
    dw 0x0
    db 0x0
    db 10011010b
    db 11001111b
    db 0x0

gdt_data:
    dw 0xFFFF
    dw 0x0
    db 0x0
    db 10010010b
    db 11001111b
    db 0x0

gdt_end:

gdt_descriptor:
    dw gdt_end - gdt_start - 1
    dd gdt_start

CODE_SEG equ gdt_code - gdt_start
DATA_SEG equ gdt_data - gdt_start

; ===================== 32-BIT =====================
BITS 32
pm_start:
    mov ax, DATA_SEG
    mov ds, ax
    mov ss, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    
    mov ebp, 0x90000
    mov esp, ebp
    
    ; Wyczyść ekran
    mov edi, 0xB8000
    mov ecx, 2000
    mov ax, 0x0F20
    rep stosw
    
    ; Komunikat
    mov esi, msg_ok
    mov edi, 0xB8000
    mov ah, 0x0F
.print_loop:
    lodsb
    test al, al
    jz .print_done
    stosw
    jmp .print_loop
.print_done:
    
    ; Uruchom jądro
    call KERNEL_OFFSET
    
    cli
    hlt
    jmp $

msg_ok: db "KELNER BOOT OK", 0

; ===================== PADDING =====================
times 510-($-$$) db 0
dw 0xAA55