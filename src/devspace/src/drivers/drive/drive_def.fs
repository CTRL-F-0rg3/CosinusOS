\ drive_def.fs - Rejestry i stałe ATA PIO
constant ATA_PRIMARY_BASE   0x1F0
constant ATA_PRIMARY_CTRL   0x3F6

\ Rejestry (Base + Offset)
: REG_DATA       0 ;
: REG_ERR_FEAT   1 ;
: REG_SEC_COUNT  2 ;
: REG_LBA_LOW    3 ;
: REG_LBA_MID    4 ;
: REG_LBA_HIGH   5 ;
: REG_DRIVE_SEL  6 ;
: REG_COMMAND    7 ;
: REG_STATUS     7 ;

\ Komendy ATA
0x20 constant CMD_READ_PIO
0x30 constant CMD_WRITE_PIO
0xEC constant CMD_IDENTIFY
0xE7 constant CMD_FLUSH

\ Statusy
0x80 constant STATUS_BSY
0x08 constant STATUS_DRQ
0x01 constant STATUS_ERR