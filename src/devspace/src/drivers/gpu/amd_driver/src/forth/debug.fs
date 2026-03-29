\ debug.fs - Hardware Diagnostic Tools

: dump-gpu-status ( -- )
    ." GRBM_STATUS: " 0x8010 mmio-read h. cr
    ." CP_STAT:     " 0x8640 mmio-read h. cr
    ." VRAM_FREE:   " get-free-vram . cr ;

: trace-ring ( -- )
    ." RPTR: " get-rptr h. 
    ." WPTR: " get-wptr h. cr ;