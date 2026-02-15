
; =====================================================================
; KELNER OS - High-Level Terminal (Super Mode)
; Plik: src/terminal.asm
; Terminal z prefiksem "Super" dla zaawansowanych komend
; =====================================================================

BITS 32

section .data
    super_prompt db 'Super> ', 0
    super_prefix db 'Super ', 0
    super_help_msg db 'Super Terminal - Available commands:', 10
                   db '  Super reboot     - Restart system', 10
                   db '  Super shutdown   - Power off system', 10
                   db '  Super panic      - Trigger kernel panic', 10
                   db '  Super meminfo    - Display memory info', 10
                   db '  Super cpuinfo    - Display CPU info', 10
                   db '  Super diskinfo   - Display disk info', 10
                   db '  Super servlist   - List all services', 10
                   db '  Super servstart  - Start a service', 10
                   db '  Super servstop   - Stop a service', 10
                   db '  Super log        - Show system log', 10
                   db '  Super clear      - Clear screen', 10
                   db '  Super help       - Show this help', 10, 0
    
    cmd_reboot db 'reboot', 0
    cmd_shutdown db 'shutdown', 0
    cmd_panic db 'panic', 0
    cmd_meminfo db 'meminfo', 0
    cmd_cpuinfo db 'cpuinfo', 0
    cmd_diskinfo db 'diskinfo', 0
    cmd_servlist db 'servlist', 0
    cmd_servstart db 'servstart', 0
    cmd_servstop db 'servstop', 0
    cmd_log db 'log', 0
    cmd_clear db 'clear', 0
    cmd_help db 'help', 0
    
    msg_rebooting db 'System rebooting...', 10, 0
    msg_shutdown db 'System shutting down...', 10, 0
    msg_panic db 'Triggering kernel panic...', 10, 0
    msg_invalid db 'Invalid Super command. Type "Super help" for help.', 10, 0
    msg_not_super db 'Not a Super command. Use regular terminal.', 10, 0

section .bss
    super_buffer resb 256
    super_buffer_pos resd 1

section .text
    global super_terminal_init
    global super_terminal_process
    global super_terminal_check_prefix
    global super_terminal_execute

; ===================== INICJALIZACJA =====================
super_terminal_init:
    push ebp
    mov ebp, esp
    
    ; Wyzeruj bufor
    mov edi, super_buffer
    mov ecx, 256
    xor eax, eax
    rep stosb
    
    ; Wyzeruj pozycję
    mov dword [super_buffer_pos], 0
    
    mov esp, ebp
    pop ebp
    ret

; ===================== SPRAWDŹ PREFIX "Super" =====================
; Input: ESI = pointer do stringu
; Output: EAX = 1 jeśli ma prefix, 0 jeśli nie
super_terminal_check_prefix:
    push ebp
    mov ebp, esp
    push esi
    push edi
    
    mov edi, super_prefix
    mov ecx, 6  ; "Super "
    
.compare_loop:
    lodsb
    cmp al, [edi]
    jne .no_match
    inc edi
    loop .compare_loop
    
    mov eax, 1
    jmp .done
    
.no_match:
    xor eax, eax
    
.done:
    pop edi
    pop esi
    mov esp, ebp
    pop ebp
    ret

; ===================== PRZETWARZANIE KOMENDY =====================
; Input: ESI = pointer do pełnej komendy (z "Super ")
super_terminal_process:
    push ebp
    mov ebp, esp
    
    ; Sprawdź czy ma prefix
    call super_terminal_check_prefix
    test eax, eax
    jz .not_super
    
    ; Pomiń "Super "
    add esi, 6
    
    ; Wykonaj komendę
    call super_terminal_execute
    jmp .done
    
.not_super:
    ; Wyświetl komunikat że to nie super komenda
    mov esi, msg_not_super
    call print_string
    
.done:
    mov esp, ebp
    pop ebp
    ret

; ===================== WYKONANIE KOMENDY =====================
; Input: ESI = pointer do komendy (bez "Super ")
super_terminal_execute:
    push ebp
    mov ebp, esp
    push esi
    
    ; Sprawdź każdą komendę
    
    ; reboot
    mov edi, cmd_reboot
    call strcmp
    test eax, eax
    jz .do_reboot
    
    ; shutdown
    mov edi, cmd_shutdown
    call strcmp
    test eax, eax
    jz .do_shutdown
    
    ; panic
    mov edi, cmd_panic
    call strcmp
    test eax, eax
    jz .do_panic
    
    ; meminfo
    mov edi, cmd_meminfo
    call strcmp
    test eax, eax
    jz .do_meminfo
    
    ; cpuinfo
    mov edi, cmd_cpuinfo
    call strcmp
    test eax, eax
    jz .do_cpuinfo
    
    ; diskinfo
    mov edi, cmd_diskinfo
    call strcmp
    test eax, eax
    jz .do_diskinfo
    
    ; servlist
    mov edi, cmd_servlist
    call strcmp
    test eax, eax
    jz .do_servlist
    
    ; clear
    mov edi, cmd_clear
    call strcmp
    test eax, eax
    jz .do_clear
    
    ; help
    mov edi, cmd_help
    call strcmp
    test eax, eax
    jz .do_help
    
    ; Nieznana komenda
    mov esi, msg_invalid
    call print_string
    jmp .done

.do_reboot:
    mov esi, msg_rebooting
    call print_string
    call super_reboot
    jmp .done

.do_shutdown:
    mov esi, msg_shutdown
    call print_string
    call super_shutdown
    jmp .done

.do_panic:
    mov esi, msg_panic
    call print_string
    call super_panic
    jmp .done

.do_meminfo:
    call super_meminfo
    jmp .done

.do_cpuinfo:
    call super_cpuinfo
    jmp .done

.do_diskinfo:
    call super_diskinfo
    jmp .done

.do_servlist:
    call super_servlist
    jmp .done

.do_clear:
    call super_clear
    jmp .done

.do_help:
    mov esi, super_help_msg
    call print_string
    jmp .done

.done:
    pop esi
    mov esp, ebp
    pop ebp
    ret

; ===================== SUPER KOMENDY =====================

super_reboot:
    ; Triple fault - najprostszy sposób restartu
    cli
    lidt [0]  ; Załaduj pustą IDT
    int 3     ; Wywołaj interrupt -> triple fault
    ret

super_shutdown:
    ; ACPI shutdown przez port 0x604
    mov ax, 0x2000
    mov dx, 0x604
    out dx, ax
    
    ; QEMU/Bochs shutdown
    mov ax, 0x2000
    mov dx, 0xB004
    out dx, ax
    
    ; Jeśli nie zadziała, zatrzymaj procesor
    cli
    hlt
    ret

super_panic:
    ; Wywołaj funkcję kernel_panic z C
    extern kernel_panic
    push msg_panic
    call kernel_panic
    add esp, 4
    ret

super_meminfo:
    push ebp
    mov ebp, esp
    
    ; Pobierz informacje o pamięci
    extern get_memory_info
    call get_memory_info
    
    mov esp, ebp
    pop ebp
    ret

super_cpuinfo:
    push ebp
    mov ebp, esp
    
    ; Użyj CPUID
    mov eax, 0
    cpuid
    
    ; Wyświetl informacje
    extern display_cpu_info
    push edx
    push ecx
    push ebx
    push eax
    call display_cpu_info
    add esp, 16
    
    mov esp, ebp
    pop ebp
    ret

super_diskinfo:
    push ebp
    mov ebp, esp
    
    ; Wywołaj funkcję C
    extern display_disk_info
    call display_disk_info
    
    mov esp, ebp
    pop ebp
    ret

super_servlist:
    push ebp
    mov ebp, esp
    
    ; Wywołaj funkcję C
    extern display_service_list
    call display_service_list
    
    mov esp, ebp
    pop ebp
    ret

super_clear:
    ; Wyczyść ekran
    mov edi, 0xB8000
    mov ecx, 2000
    mov ax, 0x0F20  ; Spacja z atrybutem
    rep stosw
    
    ; Zresetuj kursor
    extern terminal_reset_cursor
    call terminal_reset_cursor
    ret

;  FUNKCJE POMOCNICZE 

; Input: ESI = string1, EDI = string2
; Output: EAX = 0 jeśli równe, != 0 jeśli różne
strcmp:
    push esi
    push edi
    
.loop:
    lodsb
    mov bl, [edi]
    inc edi
    
    cmp al, bl
    jne .not_equal
    
    test al, al
    jz .equal
    
    jmp .loop
    
.equal:
    xor eax, eax
    jmp .done
    
.not_equal:
    mov eax, 1
    
.done:
    pop edi
    pop esi
    ret


; Input: ESI = pointer do stringa
print_string:
    extern terminal_write
    push esi
    call terminal_write
    add esp, 4
    ret

; \WYŚWIETL PROMPT \
super_terminal_show_prompt:
    push ebp
    mov ebp, esp
    
    mov esi, super_prompt
    call print_string
    
    mov esp, ebp
    pop ebp
    ret