; CosinusOS — allocator/asm/bitmap_ops.asm
;
; Bitmap operations for BuddyAllocator.
; Replaces the Rust per-bit loops in bitmap_set_free, bitmap_set_used,
; bitmap_is_free with BSF/BSR/BTS/BTR and REP STOSQ bulk paths.
;
; Calling convention: System V AMD64 (rdi, rsi, rdx, rcx, r8, r9)
; All functions preserve rbx, rbp, r12-r15 (callee-saved).
; No red zone used — kernel code may run with interrupts where RSP is sacred.
;
; Rust extern "C" signatures expected in buddy.rs:
;
;   extern "C" {
;       fn bitmap_set_free(bitmap: *mut u64, start_page: usize, n_pages: usize);
;       fn bitmap_set_used(bitmap: *mut u64, start_page: usize, n_pages: usize);
;       fn bitmap_is_free(bitmap: *const u64, start_page: usize, n_pages: usize) -> bool;
;       fn bitmap_find_free(bitmap: *const u64, words: usize) -> usize;
;   }
;
; Parameters:
;   rdi = *mut u64  bitmap base pointer
;   rsi = usize     start_page  (bit index of first page)
;   rdx = usize     n_pages     (number of contiguous pages, i.e. 1 << order)
;
; BITMAP_WORDS = 129 (N_PAGES/64 + 1 = 8192/64 + 1)
; A set bit means FREE, a clear bit means USED (matches buddy.rs convention).

bits 64
default rel

section .text

; ---------------------------------------------------------------------------
; bitmap_set_free(bitmap, start_page, n_pages)
;   Sets n_pages bits starting at start_page.
;   Fast path: if the run is word-aligned and covers whole words, use STOSQ.
;   Slow path: per-bit BTS loop for head/tail partial words.
; ---------------------------------------------------------------------------
global bitmap_set_free
bitmap_set_free:
    ; rdi = bitmap ptr
    ; rsi = start_page (bit index)
    ; rdx = n_pages    (bit count)
    test    rdx, rdx
    jz      .done

    ; --- head: bits before first aligned word boundary ---
    mov     rcx, rsi
    and     rcx, 63             ; bit offset within first word
    jz      .check_bulk

    ; partial first word
    mov     r8,  64
    sub     r8,  rcx            ; bits available in this word
    cmp     rdx, r8
    cmovb   r8,  rdx            ; r8 = min(available, n_pages)

    ; build mask: r8 consecutive bits starting at rcx
    ; cl is already rcx & 63 (bit offset), save it and use for second shift
    push    rcx
    mov     r9,  -1             ; all ones
    mov     rcx, 64
    sub     rcx, r8             ; rcx = 64 - r8
    shr     r9,  cl             ; r9 = (1<<r8)-1  low r8 bits set
    pop     rcx                 ; restore bit offset
    shl     r9,  cl             ; shift mask to bit position

    mov     rax, rsi
    shr     rax, 6              ; word index
    or      [rdi + rax*8], r9

    sub     rdx, r8
    add     rsi, r8
    jz      .done

.check_bulk:
    ; --- bulk: whole words ---
    mov     rax, rsi
    shr     rax, 6              ; first full word index
    mov     rcx, rdx
    shr     rcx, 6              ; number of full words
    jz      .tail

    push    rdi
    lea     rdi, [rdi + rax*8]
    mov     rax, -1             ; 0xFFFFFFFFFFFFFFFF = all pages free
    rep stosq
    pop     rdi

    ; advance counters
    mov     r8,  rcx
    shl     r8,  6              ; bits consumed
    sub     rdx, r8
    add     rsi, r8

.tail:
    ; --- tail: remaining bits in last partial word ---
    test    rdx, rdx
    jz      .done

    ; rdx < 64 bits remain, starting at bit 0 of next word
    mov     r9,  -1
    mov     rcx, 64
    sub     rcx, rdx
    shr     r9,  cl             ; low rdx bits set

    mov     rax, rsi
    shr     rax, 6
    or      [rdi + rax*8], r9

.done:
    ret


; ---------------------------------------------------------------------------
; bitmap_set_used(bitmap, start_page, n_pages)
;   Clears n_pages bits starting at start_page.
;   Same structure as bitmap_set_free but uses AND NOT (BTC / ANDN).
; ---------------------------------------------------------------------------
global bitmap_set_used
bitmap_set_used:
    test    rdx, rdx
    jz      .done

    ; head partial word
    mov     rcx, rsi
    and     rcx, 63
    jz      .check_bulk

    mov     r8,  64
    sub     r8,  rcx
    cmp     rdx, r8
    cmovb   r8,  rdx

    push    rcx
    mov     r9,  -1
    mov     rcx, 64
    sub     rcx, r8
    shr     r9,  cl             ; r9 = low r8 bits set
    pop     rcx
    shl     r9,  cl
    not     r9                  ; invert: mask of bits to KEEP

    mov     rax, rsi
    shr     rax, 6
    and     [rdi + rax*8], r9

    sub     rdx, r8
    add     rsi, r8
    jz      .done

.check_bulk:
    mov     rax, rsi
    shr     rax, 6
    mov     rcx, rdx
    shr     rcx, 6
    jz      .tail

    push    rdi
    lea     rdi, [rdi + rax*8]
    xor     eax, eax            ; 0x0000...0000 = all pages used
    rep stosq
    pop     rdi

    mov     r8,  rcx
    shl     r8,  6
    sub     rdx, r8
    add     rsi, r8

.tail:
    test    rdx, rdx
    jz      .done

    mov     r9,  -1
    mov     rcx, 64
    sub     rcx, rdx
    shr     r9,  cl
    not     r9

    mov     rax, rsi
    shr     rax, 6
    and     [rdi + rax*8], r9

.done:
    ret


; ---------------------------------------------------------------------------
; bitmap_is_free(bitmap, start_page, n_pages) -> bool (u8: 0 or 1)
;   Returns 1 if all n_pages bits are set (free), 0 otherwise.
;   Fast path: whole words checked with CMP -1.
;   Partial words masked and compared.
; ---------------------------------------------------------------------------
global bitmap_is_free
bitmap_is_free:
    test    rdx, rdx
    jz      .yes                ; zero pages = vacuously free

    ; head partial word
    mov     rcx, rsi
    and     rcx, 63
    jz      .check_bulk

    mov     r8,  64
    sub     r8,  rcx
    cmp     rdx, r8
    cmovb   r8,  rdx

    push    rcx
    mov     r9,  -1
    mov     rcx, 64
    sub     rcx, r8
    shr     r9,  cl             ; r9 = low r8 bits set
    pop     rcx
    shl     r9,  cl             ; mask for bits we care about

    mov     rax, rsi
    shr     rax, 6
    mov     r10, [rdi + rax*8]
    and     r10, r9
    cmp     r10, r9
    jne     .no                 ; some bits not set = not free

    sub     rdx, r8
    add     rsi, r8
    jz      .yes

.check_bulk:
    mov     rax, rsi
    shr     rax, 6
    mov     rcx, rdx
    shr     rcx, 6
    jz      .tail

.bulk_loop:
    cmp     qword [rdi + rax*8], -1
    jne     .no
    inc     rax
    loop    .bulk_loop

    mov     r8,  rcx
    shl     r8,  6
    sub     rdx, r8
    add     rsi, r8

.tail:
    test    rdx, rdx
    jz      .yes

    mov     r9,  -1
    mov     rcx, 64
    sub     rcx, rdx
    shr     r9,  cl

    mov     rax, rsi
    shr     rax, 6
    mov     r10, [rdi + rax*8]
    and     r10, r9
    cmp     r10, r9
    jne     .no

.yes:
    mov     eax, 1
    ret
.no:
    xor     eax, eax
    ret


; ---------------------------------------------------------------------------
; bitmap_find_free(bitmap, words) -> usize
;
; Scans the bitmap word by word looking for the first non-zero word,
; then uses BSF to find the lowest free bit within it.
; Returns bit index (page index) of the first free page.
; Returns usize::MAX (0xFFFFFFFFFFFFFFFF) if no free page found.
;
; Used internally by BuddyAllocator::alloc_order as an optional fast-scan
; hint — the caller still validates via free_lists, this just accelerates
; the "is any page free at all?" check.
;
; rdi = *const u64  bitmap
; rsi = usize       words  (BITMAP_WORDS = 129)
; ---------------------------------------------------------------------------
global bitmap_find_free
bitmap_find_free:
    test    rsi, rsi
    jz      .not_found

    xor     rcx, rcx            ; word index

.scan_loop:
    mov     rax, [rdi + rcx*8]
    test    rax, rax
    jnz     .found_word
    inc     rcx
    cmp     rcx, rsi
    jb      .scan_loop

.not_found:
    mov     rax, -1             ; usize::MAX
    ret

.found_word:
    ; rax = word with at least one free bit
    ; rcx = word index
    bsf     rdx, rax            ; rdx = bit index within word
    shl     rcx, 6              ; rcx = bit base of this word
    lea     rax, [rcx + rdx]   ; absolute bit (page) index
    ret
