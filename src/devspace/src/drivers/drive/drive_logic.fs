\ drive_logic.fs — ATA PIO control sequences
\ Depends on: drive_def.fs (included first by loader)
\ Port I/O words (inb outb inw outw) provided by the Odin Forth VM host.

\ ── 400ns delay ───────────────────────────────────────────────────────────────
\ ATA spec: after writing to drive/head register, wait ≥400ns before reading
\ status. Reading alt-status 4 times ≈ 400ns on a typical bus.

: 400ns-delay ( -- )
    ATA_CTRL inb drop
    ATA_CTRL inb drop
    ATA_CTRL inb drop
    ATA_CTRL inb drop ;

\ ── Status poll ───────────────────────────────────────────────────────────────

\ Wait until BSY clears. Returns: 0=ok, 1=error, 2=timeout
: wait-bsy ( -- result )
    POLL_TIMEOUT
    begin
        dup 0= if drop 2 exit then    \ timeout
        1-
        ATA_BASE REG_STATUS + inb
        dup SR_BSY and 0= if          \ BSY cleared
            SR_ERR and if 1 exit then \ ERR set
            0 exit                    \ ok
        then
        drop
    again ;

\ Wait until DRQ sets (and BSY clear). Returns: 0=ok, 1=error, 2=timeout
: wait-drq ( -- result )
    POLL_TIMEOUT
    begin
        dup 0= if drop 2 exit then
        1-
        ATA_BASE REG_STATUS + inb
        dup SR_BSY and if drop continue then  \ still busy — keep looping
        dup SR_ERR and if drop 1 exit then    \ error
        SR_DRQ and if 0 exit then             \ DRQ set — ready
        drop                                  \ neither — keep looping
    again ;

\ ── Soft reset ────────────────────────────────────────────────────────────────

: ata-reset ( -- )
    CMD_SRST ATA_CTRL outb    \ assert SRST bit
    400ns-delay
    0 ATA_CTRL outb            \ clear SRST
    400ns-delay
    wait-bsy drop ;            \ wait for BSY to clear

\ ── Drive selection ───────────────────────────────────────────────────────────
\ ( drive -- )  drive: 0=master 1=slave

: select-drive ( drive -- )
    0= if DRV_MASTER else DRV_SLAVE then
    ATA_BASE REG_DRIVE_SEL + outb
    400ns-delay ;

\ ── LBA28 setup ───────────────────────────────────────────────────────────────
\ ( lba drive -- )  Writes LBA registers, does NOT send command

: setup-lba28 ( lba drive -- )
    \ Drive/head: DRV_* | lba[27:24]
    over 24 rshift 0x0F and       \ lba[27:24]
    swap 0= if DRV_MASTER else DRV_SLAVE then
    or ATA_BASE REG_DRIVE_SEL + outb
    400ns-delay

    \ Sector count = 1
    1 ATA_BASE REG_SEC_COUNT + outb

    \ LBA bytes
    dup 0xFF and        ATA_BASE REG_LBA_LO  + outb
    dup 8 rshift 0xFF and ATA_BASE REG_LBA_MID + outb
    16 rshift 0xFF and  ATA_BASE REG_LBA_HI  + outb ;

\ ── IDENTIFY ──────────────────────────────────────────────────────────────────
\ ( drive buf-addr -- ok )   buf must be 512 bytes

: ata-identify ( drive buf-addr -- ok )
    >r                            \ save buf-addr to return stack
    select-drive
    0 ATA_BASE REG_SEC_COUNT + outb
    0 ATA_BASE REG_LBA_LO  + outb
    0 ATA_BASE REG_LBA_MID + outb
    0 ATA_BASE REG_LBA_HI  + outb
    CMD_IDENTIFY ATA_BASE REG_CMD + outb

    \ Check if drive exists (status 0 after command = no drive)
    ATA_BASE REG_STATUS + inb
    0= if r> drop 0 exit then

    wait-drq 0<> if r> drop 0 exit then   \ error/timeout

    \ Read 256 words into buffer
    r> ( buf )
    256 0 do
        ATA_BASE REG_DATA + inw
        over w!
        2+
    loop
    drop
    -1 ;                          \ success

\ ── Read one sector ───────────────────────────────────────────────────────────
\ ( lba drive buf-addr -- ok )

: read-sector ( lba drive buf-addr -- ok )
    >r                            \ save buf-addr
    setup-lba28                   \ ( lba drive -- ) writes regs
    CMD_READ_PIO ATA_BASE REG_CMD + outb
    wait-drq 0<> if r> drop 0 exit then

    r> ( buf )
    256 0 do
        ATA_BASE REG_DATA + inw
        over w!
        2+
    loop
    drop
    -1 ;

\ ── Write one sector ──────────────────────────────────────────────────────────
\ ( lba drive buf-addr -- ok )

: write-sector ( lba drive buf-addr -- ok )
    >r
    setup-lba28
    CMD_WRITE_PIO ATA_BASE REG_CMD + outb
    wait-drq 0<> if r> drop 0 exit then

    r> ( buf )
    256 0 do
        over w@                   \ read word from buffer
        ATA_BASE REG_DATA + outw
        2+
    loop
    drop

    \ Flush write cache
    CMD_FLUSH_CACHE ATA_BASE REG_CMD + outb
    wait-bsy drop
    -1 ;

\ ── Multi-sector read ─────────────────────────────────────────────────────────
\ ( lba count drive buf-addr -- ok )

: read-sectors ( lba count drive buf-addr -- ok )
    \ Stack: ( lba count drive buf )
    >r >r >r                      \ save buf, drive, count
    r> r> r>                      \ restore: ( lba count drive buf )
    -rot swap                     \ ( lba buf drive count )
    0 do
        dup >r                    \ save buf
        2 pick                    \ ( lba buf drive lba )  -- duplicate lba
        over                      \ ( lba buf drive lba drive )
        r@                        \ ( lba buf drive lba drive buf )
        read-sector 0= if
            r> 2drop drop drop 0 exit
        then
        r> 512 +                  \ advance buf by one sector
        rot 1+ rot rot            \ advance lba
    loop
    2drop drop -1 ;