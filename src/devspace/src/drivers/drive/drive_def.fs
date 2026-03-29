\ drive_def.fs — ATA PIO registers and constants
\ Included by drive_logic.fs — do not execute standalone

\ ── Primary bus base addresses ────────────────────────────────────────────────
0x1F0 constant ATA_BASE       \ data, lba, cmd registers
0x3F6 constant ATA_CTRL       \ alt-status / device control

\ ── Register offsets (add to ATA_BASE) ───────────────────────────────────────
0 constant REG_DATA           \ 16-bit data port
1 constant REG_ERR            \ error (read) / features (write)
2 constant REG_SEC_COUNT      \ sector count
3 constant REG_LBA_LO         \ LBA bits 7:0
4 constant REG_LBA_MID        \ LBA bits 15:8
5 constant REG_LBA_HI         \ LBA bits 23:16
6 constant REG_DRIVE_SEL      \ drive select + LBA bits 27:24
7 constant REG_CMD            \ command (write)
7 constant REG_STATUS         \ status (read)

\ ── ATA commands ──────────────────────────────────────────────────────────────
0x20 constant CMD_READ_PIO
0x30 constant CMD_WRITE_PIO
0xEC constant CMD_IDENTIFY
0xE7 constant CMD_FLUSH_CACHE
0x04 constant CMD_SRST        \ soft reset (written to ATA_CTRL, not REG_CMD)

\ ── Status register bits ──────────────────────────────────────────────────────
0x80 constant SR_BSY          \ drive is busy — do not issue commands
0x40 constant SR_DRDY         \ drive ready to accept commands
0x08 constant SR_DRQ          \ data request — transfer is ready
0x01 constant SR_ERR          \ error occurred — check REG_ERR

\ ── Drive/head register masks ─────────────────────────────────────────────────
0xE0 constant DRV_MASTER      \ select master, LBA mode
0xF0 constant DRV_SLAVE       \ select slave,  LBA mode

\ ── Poll timeout (iterations, not real time) ──────────────────────────────────
500000 constant POLL_TIMEOUT