\ cmdbuf.fs - AMD PM4 Packet Construction
\ Packet Header Type 3: [7:0] count, [15:8] reserved, [23:16] opcode, [31:30] type

: pm4-header ( count opcode -- header )
    16 lshift swap          ( opcode<<16 count )
    0x30000000 or or ;      ( type3_bits | opcode | count )

\ Writes a packet to the command buffer
\ ( opcode count -- )
: emit-packet ( opcode count -- )
    over over pm4-header    \ generate header
    ring-write              \ push to ring buffer
    0 do
        ring-write          \ push arguments (words)
    loop ;

\ Example: Draw Index (Opcode 0x27)
: draw-index ( index-count -- )
    0x27 1 emit-packet ;