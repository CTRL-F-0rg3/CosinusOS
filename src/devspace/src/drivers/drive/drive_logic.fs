\ drive_logic.fs - Sekwencje sterujące
include drive_def.fs

: wait-bsy ( -- )
    begin
        ATA_PRIMARY_BASE REG_STATUS inb
        STATUS_BSY and 0=
    until ;

: wait-drq ( -- )
    begin
        ATA_PRIMARY_BASE REG_STATUS inb
        dup STATUS_ERR and if 1 throw then \ Obsługa błędu
        STATUS_DRQ and
    until ;

: select-drive ( drive -- )
    if 0xF0 else 0xE0 then \ 0=Master, 1=Slave
    ATA_PRIMARY_BASE REG_DRIVE_SEL outb
    400ns-delay ;

: read-sector-pio ( lba drive -- )
    select-drive
    1 ATA_PRIMARY_BASE REG_SEC_COUNT outb
    \ ... (tutaj wysyłanie LBA) ...
    CMD_READ_PIO ATA_PRIMARY_BASE REG_COMMAND outb
    wait-drq
    transfer-block-in ; \ Wywołuje krytyczną funkcję z ASM