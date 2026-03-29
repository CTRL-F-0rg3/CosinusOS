\ scheduler.fs - Task scheduling and Fences
\ Uses a "Timestamp" or "Fence" register to track progress.

variable current-fence
constant ADDR_FENCE_REG  0x1234 \ Placeholder for actual fence register

: increment-fence ( -- next )
    1 current-fence +!
    current-fence @ ;

: submit-work ( -- )
    increment-fence
    \ 1. Write fence value to Ring
    \ 2. Trigger Interrupt/Trap
    \ 3. Update WPTR (Write Pointer)
    update-wptr ;

: wait-for-gpu ( fence-val -- )
    begin
        ADDR_FENCE_REG mmio-read
        over >=
    until
    drop ;