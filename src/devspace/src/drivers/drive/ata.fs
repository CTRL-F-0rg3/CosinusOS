\ ata.forth — ATA PIO driver core
\ Runs in CosinusOS userspace via sys_outb/sys_inb syscalls.
\ Forth words are called from Odin via a simple dispatch table.
\
\ ATA primary bus port map (0x1F0 base):
\   +0  data        (16-bit R/W)
\   +1  error/feat
\   +2  sector count
\   +3  LBA lo
\   +4  LBA mid
\   +5  LBA hi
\   +6  drive/head  (bit6=LBA mode, bit4=slave)
\   +7  status/cmd
\   +0x206 (alt status / device ctrl)

\ ── Port constants ────────────────────────────────────────────────────────────

constant ATA_BASE       0x1F0
constant ATA_ALT_CTRL   0x3F6

\ Register offsets from ATA_BASE
constant ATA_DATA       0
constant ATA_ERROR      1
constant ATA_FEAT       1
constant ATA_SECCOUNT   2
constant ATA_LBA_LO     3
constant ATA_LBA_MID    4
constant ATA_LBA_HI     5
constant ATA_DRIVE_HEAD 6
constant ATA_STATUS     7
constant ATA_CMD        7

\ Status register bits
constant ATA_SR_BSY     0x80    \ busy
constant ATA_SR_DRDY    0x40    \ drive ready
constant ATA_SR_DRQ     0x08    \ data request (ready to transfer)
constant ATA_SR_ERR     0x01    \ error

\ Commands
constant ATA_CMD_READ_PIO   0x20
constant ATA_CMD_WRITE_PIO  0x30
constant ATA_CMD_IDENTIFY   0xEC
constant ATA_CMD_FLUSH      0xE7
constant ATA_CMD_SRST       0x04    \ soft reset (to ctrl reg)

\ ── Syscall shims ─────────────────────────────────────────────────────────────
\ CosinusOS exposes port I/O through syscalls (ring-3 cannot use IN/OUT directly).
\ Odin driver sets up these words by writing function pointers into variables.

variable outb-fn    \ fn(port: u16, val: u8)
variable inb-fn     \ fn(port: u16) -> u8
variable inw-fn     \ fn(port: u16) -> u16
variable outw-fn    \ fn(port: u16, val: u16)

\ Wrappers: ( port -- val ) and ( val port -- )
: outb  ( val port -- )  outb-fn @ execute ;
: inb   ( port -- val )  inb-fn  @ execute ;
: inw   ( port -- val )  inw-fn  @ execute ;
: outw  ( val port -- )  outw-fn @ execute ;

\ ── Low-level register access ─────────────────────────────────────────────────

: ata-reg  ( offset -- port )  ATA_BASE + ;

: ata-out  ( val offset -- )   ata-reg outb ;
: ata-in   ( offset -- val )   ata-reg inb  ;
: ata-outw ( val offset -- )   ata-reg outw ;
: ata-inw  ( offset -- val )   ata-reg inw  ;

: ata-status  ( -- status )  ATA_STATUS ata-in ;
: ata-error   ( -- err )     ATA_ERROR  ata-in ;

\ ── Delay: read alt-status 4× (400ns per spec) ────────────────────────────────

: ata-delay400ns ( -- )
    ATA_ALT_CTRL inb drop
    ATA_ALT_CTRL inb drop
    ATA_ALT_CTRL inb drop
    ATA_ALT_CTRL inb drop ;

\ ── Poll BSY clear, then check DRQ or ERR ────────────────────────────────────
\ Returns: 0 = ok, 1 = error, 2 = timeout

constant ATA_POLL_TIMEOUT 100000

: ata-poll-bsy ( -- result )
    ATA_POLL_TIMEOUT
    begin
        dup 0= if drop 2 exit then    \ timeout
        1-
        ata-status
        dup ATA_SR_BSY and 0= if      \ BSY cleared
            ATA_SR_ERR and if         \ ERR set?
                drop 1 exit
            then
            drop 0 exit
        then
        drop
    again ;

: ata-poll-drq ( -- result )
    ATA_POLL_TIMEOUT
    begin
        dup 0= if drop 2 exit then
        1-
        ata-status
        dup ATA_SR_BSY and if drop drop continue then
        dup ATA_SR_ERR and if drop drop 1 exit then
        ATA_SR_DRQ and if drop 0 exit then
        drop
    again ;

\ ── Soft reset ────────────────────────────────────────────────────────────────

: ata-reset ( -- )
    ATA_CMD_SRST ATA_ALT_CTRL outb    \ assert SRST
    ata-delay400ns
    0 ATA_ALT_CTRL outb               \ clear SRST
    ata-delay400ns
    ata-poll-bsy drop ;               \ wait for BSY clear

\ ── Drive select ──────────────────────────────────────────────────────────────
\ drive: 0=master 1=slave

: ata-select-drive ( drive -- )
    0= if 0xE0 else 0xF0 then        \ master=0xE0, slave=0xF0
    ATA_DRIVE_HEAD ata-out
    ata-delay400ns ;

\ ── IDENTIFY ──────────────────────────────────────────────────────────────────
\ Returns total sector count in two cells: ( sectors-lo sectors-hi -- )
\ Writes 512-byte identify buffer to address on stack.

variable identify-buf   \ pointer to 512-byte buffer (set by Odin)

: ata-identify ( drive buf-ptr -- ok )
    identify-buf !                    \ stash buffer pointer
    ata-select-drive
    0 ATA_SECCOUNT  ata-out
    0 ATA_LBA_LO    ata-out
    0 ATA_LBA_MID   ata-out
    0 ATA_LBA_HI    ata-out
    ATA_CMD_IDENTIFY ATA_CMD ata-out

    ata-status 0= if 0 exit then      \ no drive present

    ata-poll-drq 0<> if 0 exit then   \ error or timeout

    \ Read 256 words (512 bytes) into buffer
    identify-buf @ ( buf )
    256 0 do
        ATA_DATA ata-inw              \ read one word
        over !                        \ store at buf ptr
        2+                            \ advance ptr (2 bytes per word)
    loop
    drop
    -1 ;                              \ success

\ ── LBA28 sector read ─────────────────────────────────────────────────────────
\ ( lba drive buf-ptr -- ok )
\ buf-ptr must point to at least 512 bytes of writable memory

: ata-read-sector ( lba drive buf-ptr -- ok )
    -rot                              \ ( buf-ptr lba drive )
    swap                              \ ( buf-ptr drive lba )

    \ Select drive + LBA28 bits 24-27 in drive/head reg
    over 24 rshift 0x0F and           \ lba[27:24]
    swap 0= if 0xE0 else 0xF0 then or \ OR with master/slave bits
    ATA_DRIVE_HEAD ata-out
    ata-delay400ns

    1           ATA_SECCOUNT ata-out  \ sector count = 1
    dup 0xFF and ATA_LBA_LO  ata-out  \ LBA[7:0]
    dup 8 rshift 0xFF and ATA_LBA_MID ata-out  \ LBA[15:8]
    dup 16 rshift 0xFF and ATA_LBA_HI ata-out  \ LBA[23:16]
    drop

    ATA_CMD_READ_PIO ATA_CMD ata-out

    ata-poll-drq 0<> if drop 0 exit then

    \ Read 256 words into buf-ptr
    256 0 do
        ATA_DATA ata-inw
        over !
        2+
    loop
    drop
    -1 ;

\ ── LBA28 sector write ────────────────────────────────────────────────────────
\ ( lba drive buf-ptr -- ok )

: ata-write-sector ( lba drive buf-ptr -- ok )
    -rot
    swap

    over 24 rshift 0x0F and
    swap 0= if 0xE0 else 0xF0 then or
    ATA_DRIVE_HEAD ata-out
    ata-delay400ns

    1           ATA_SECCOUNT ata-out
    dup 0xFF and ATA_LBA_LO  ata-out
    dup 8 rshift 0xFF and ATA_LBA_MID ata-out
    dup 16 rshift 0xFF and ATA_LBA_HI ata-out
    drop

    ATA_CMD_WRITE_PIO ATA_CMD ata-out

    ata-poll-drq 0<> if drop 0 exit then

    \ Write 256 words from buf-ptr
    256 0 do
        over w@                       \ fetch word from buffer
        ATA_DATA ata-outw
        2+                            \ advance buf ptr
    loop
    drop

    \ Flush write cache
    ATA_CMD_FLUSH ATA_CMD ata-out
    ata-poll-bsy drop
    -1 ;

\ ── Multi-sector read (used by cache for prefetch) ────────────────────────────
\ ( lba count drive buf-ptr -- ok )

: ata-read-sectors ( lba count drive buf-ptr -- ok )
    >r >r >r                          \ save buf-ptr, drive, count
    r> r> r>                          \ restore: ( lba count drive buf-ptr )
    rot rot                           \ ( buf-ptr lba count drive )
    swap                              \ ( buf-ptr lba drive count )
    0 do
        \ Read one sector, advance buf-ptr by 512
        2dup                          \ ( buf-ptr lba drive lba drive )
        drop                          \ ( buf-ptr lba drive )
        over over                     \ ( buf-ptr lba drive lba drive )
        3 pick                        \ ( buf-ptr lba drive lba drive buf-ptr )
        ata-read-sector 0= if
            2drop 2drop drop 0 exit   \ propagate error
        then
        512 +                         \ advance buf-ptr
        swap 1+ swap                  \ advance lba
    loop
    2drop drop -1 ;

\ ── Init word called once by Odin at driver startup ──────────────────────────

: ata-init ( -- )
    ata-reset
    \ Probe master drive
    0 identify-buf @ ata-identify
    if
        \ Parse total sectors from IDENTIFY words 60-61 (LBA28 capacity)
        identify-buf @ 60 2* +        \ offset to word 60
        dup w@ swap 2+ w@             \ words 60 and 61
        \ Odin reads these back via exported variables
    then ;
