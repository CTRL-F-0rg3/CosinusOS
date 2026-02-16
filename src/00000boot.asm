; ===================================================
; Bootloader x86_64 - uproszczony z 2MB pages
; ===================================================
BITS 16
ORG 0x7C00

KERNEL_OFFSET equ 0x8000       ; kernel na 32KB
KERNEL_SECTORS equ 32          ; zmniejszamy na potrzeby testów

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti
    
    mov [boot_drive], dl
    
    mov si, msg_loading
    call print_string

; ================== Ładowanie kernela ==================
load_kernel:
    mov ah, 0x02
    mov al, KERNEL_SECTORS
    mov ch, 0
    mov cl, 2
    mov dh, 0
    mov dl, [boot_drive]
    mov bx, KERNEL_OFFSET
    int 0x13
    jc disk_error
    
    mov si, msg_ok
    call print_string

; ================== Włącz A20 ==================
enable_a20:
    in al, 0x92
    or al, 2
    out 0x92, al

; ================== Budowa page tables (2MB pages) ==================
setup_identity_paging:
    ; Czyść pamięć dla page tables (0x1000-0x5000)
    mov edi, 0x1000
    mov ecx, 0x1000
    xor eax, eax
    rep stosd
    
    ; PML4[0] -> 0x2000 (PDPT)
    mov edi, 0x1000
    mov DWORD [edi], 0x2003        ; present + writable
    
    ; PDPT[0] -> 0x3000 (PD)
    mov edi, 0x2000
    mov DWORD [edi], 0x3003
    
    ; PD[0..3] -> 2MB huge pages (mapujemy pierwsze 8MB)
    mov edi, 0x3000
    mov eax, 0x83                   ; present + writable + huge page (2MB)
    mov ecx, 4
    
.map_page:
    mov DWORD [edi], eax
    add eax, 0x200000               ; następna strona 2MB
    add edi, 8
    loop .map_page

; ================== Włącz long mode ==================
enter_long_mode:
    ; PAE
    mov eax, cr4
    or eax, 1 << 5                  ; PAE
    mov cr4, eax
    
    ; Załaduj PML4
    mov eax, 0x1000
    mov cr3, eax
    
    ; Long mode enable (EFER.LME)
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr
    
    ; Paging + Protected mode
    mov eax, cr0
    or eax, 0x80000001              ; PG + PE
    mov cr0, eax
    
    ; GDT i skok do 64-bit
    lgdt [gdt_descriptor]
    jmp CODE_SEG:long_mode_init

disk_error:
    mov si, msg_error
    call print_string
    cli
    hlt

; ================== Print funkcja ==================
print_string:
    push ax
    push si
    mov ah, 0x0E
.loop:
    lodsb
    test al, al
    jz .done
    int 0x10
    jmp .loop
.done:
    pop si
    pop ax
    ret

; ================== GDT ==================
align 8
gdt_start:
    dq 0                            ; null descriptor

gdt_code:
    dw 0xFFFF
    dw 0
    db 0
    db 0x9A                         ; present, ring 0, code
    db 0xAF                         ; 64-bit, 4K granularity
    db 0

gdt_data:
    dw 0xFFFF
    dw 0
    db 0
    db 0x92                         ; present, ring 0, data
    db 0xCF                         ; 4K granularity
    db 0

gdt_end:

gdt_descriptor:
    dw gdt_end - gdt_start - 1
    dd gdt_start

CODE_SEG equ gdt_code - gdt_start
DATA_SEG equ gdt_data - gdt_start

; ================== 64-bit entry ==================
BITS 64
long_mode_init:
    ; Wyczyść segmenty
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    
    ; Ustaw stack pointer
    mov rsp, 0x7C00
    
    ; Skok do kernela
    mov rax, KERNEL_OFFSET
    jmp rax

; ================== Dane ==================
BITS 16
boot_drive: db 0
msg_loading: db "Loading kernel...", 13, 10, 0
msg_ok: db "OK! Entering 64-bit...", 13, 10, 0
msg_error: db "DISK ERROR!", 0

; ================== Padding ==================
times 510-($-$$) db 0
dw 0xAA55