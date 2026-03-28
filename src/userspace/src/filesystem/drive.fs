\ disk_init.fs
constant DISK_INFO_ADDR   0x00100000 
constant DISK_MAGIC       0xD15CAFE0 
constant DISK_MMIO_BASE   0xF0000000 

variable d-block-size
variable d-block-count-l
variable d-block-count-h

: probe-disk-hardware ( -- )
    512 d-block-size !
    41943040 d-block-count-l !
    0 d-block-count-h !
;

: store32 ( value addr -- ) ! ;

: store64 ( val_low val_high addr -- )
    over @ over ! 
    swap 4 + ! 
;

: export-disk-info ( -- )
    DISK_MAGIC DISK_INFO_ADDR store32
    d-block-size @ DISK_INFO_ADDR 4 + store32
    d-block-count-l @ d-block-count-h @ DISK_INFO_ADDR 8 + store64
    DISK_MMIO_BASE DISK_INFO_ADDR 12 + store32
    1 DISK_INFO_ADDR 16 + store32
;

: init-disk-driver ( -- )
    probe-disk-hardware
    export-disk-info
;

init-disk-driver