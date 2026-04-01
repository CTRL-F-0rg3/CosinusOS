; CosinusOS — allocator/asm/slab_hotpath.asm
;
; Hot-path slab allocator operations with PREFETCHNTA prefetching.
; Replaces SlabClass::pop, SlabClass::push, SlabClass::populate_from_page.
;
; SlabClass memory layout (Rust repr(C) / natural alignment, 8-byte fields):
;   offset  0: obj_size:   usize  (8 bytes)
;   offset  8: free_head:  *mut *mut u8  (8 bytes)
;   offset 16: free_count: usize  (8 bytes)
;   offset 24: slab_count: usize  (8 bytes)
;   sizeof(SlabClass) = 32 bytes
;
; IMPORTANT: Rust's SlabClass does NOT use repr(C) explicitly in the source.
; The FFI wrappers below receive individual field pointers so they are
; layout-independent. Rust shim wrappers extract the fields before calling.
; See buddy.rs / slab.rs for the corresponding extern "C" declarations.
;
; Rust extern "C" signatures:
;
;   extern "C" {
;       // Returns the allocated slot, or null if free_head is null.
;       fn slab_pop(
;           free_head_ptr: *mut *mut u8,   // &mut cls.free_head cast
;           free_count_ptr: *mut usize,    // &mut cls.free_count
;       ) -> *mut u8;
;
;       // Pushes ptr onto the free list.
;       fn slab_push(
;           free_head_ptr: *mut *mut u8,
;           free_count_ptr: *mut usize,
;           ptr: *mut u8,
;       );
;
;       // Populates a fresh slab page into the free list.
;       // obj_size must be a power of two and >= 8.
;       fn slab_populate(
;           free_head_ptr: *mut *mut u8,
;           free_count_ptr: *mut usize,
;           slab_count_ptr: *mut usize,
;           page: *mut u8,
;           obj_size: usize,
;       );
;   }
;
; Calling convention: System V AMD64
;   rdi, rsi, rdx, rcx, r8, r9  — arguments
;   rax                          — return value
;   rbx, rbp, r12-r15           — callee-saved (preserved)
;
; Prefetch strategy:
;   PREFETCHNTA — "non-temporal hint": fetch into L1 bypassing L2/L3.
;   Ideal for allocator free-list nodes which are write-once, read-once
;   and should not pollute the cache hierarchy with stale slab metadata.
;   We prefetch the *next* node while processing the current one,
;   hiding the DRAM latency (typically 60-200 ns) behind pointer work.

bits 64
default rel

section .text

; ---------------------------------------------------------------------------
; slab_pop(free_head_ptr, free_count_ptr) -> *mut u8
;
;   rdi = *mut (*mut u8)   pointer to cls.free_head
;   rsi = *mut usize       pointer to cls.free_count
;
;   Returns: rax = allocated slot pointer, or 0 if list empty
;
;   Equivalent Rust:
;       let slot = self.free_head as *mut u8;
;       self.free_head = *(self.free_head as *mut *mut *mut u8).read() as *mut *mut u8;
;       self.free_count -= 1;
;       slot
; ---------------------------------------------------------------------------
global slab_pop
slab_pop:
    mov     rax, [rdi]          ; rax = *free_head_ptr  (current head)
    test    rax, rax
    jz      .empty

    ; Prefetch the node after current head before we dereference current.
    ; The next pointer lives at offset 0 of the current slot (it IS the slot).
    ; We read it speculatively into rax first, then prefetch what's after it.
    mov     rdx, [rax]          ; rdx = next node pointer (slot data = next ptr)
    test    rdx, rdx
    jz      .no_prefetch
    prefetchnta [rdx]           ; warm next slot into L1 non-temporally
.no_prefetch:

    mov     [rdi], rdx          ; *free_head_ptr = next
    dec     qword [rsi]         ; free_count -= 1
    ; rax already holds the slot pointer — return it
    ret

.empty:
    xor     eax, eax            ; return null
    ret


; ---------------------------------------------------------------------------
; slab_push(free_head_ptr, free_count_ptr, ptr)
;
;   rdi = *mut (*mut u8)   pointer to cls.free_head
;   rsi = *mut usize       pointer to cls.free_count
;   rdx = *mut u8          slot to push
;
;   Equivalent Rust:
;       let slot = ptr as *mut *mut u8;
;       *slot = self.free_head as *mut u8;
;       self.free_head = slot;
;       self.free_count += 1;
; ---------------------------------------------------------------------------
global slab_push
slab_push:
    ; Prefetch the slot we are about to write, so the store hits a warm line.
    prefetchnta [rdx]

    mov     rax, [rdi]          ; rax = current free_head
    mov     [rdx], rax          ; *ptr = old head  (store next pointer into slot)
    mov     [rdi], rdx          ; free_head = ptr
    inc     qword [rsi]         ; free_count += 1
    ret


; ---------------------------------------------------------------------------
; slab_populate(free_head_ptr, free_count_ptr, slab_count_ptr,
;               page, obj_size)
;
;   rdi = *mut (*mut u8)   pointer to cls.free_head
;   rsi = *mut usize       pointer to cls.free_count
;   rdx = *mut usize       pointer to cls.slab_count
;   rcx = *mut u8          page base pointer  (4096-byte aligned)
;   r8  = usize            obj_size           (power of two, >= 8)
;
;   Walks PAGE_SIZE / obj_size slots in REVERSE order (matches Rust behavior:
;   highest address pushed first so lowest address is popped first),
;   linking them into the free list via slab_push logic inlined here.
;
;   We prefetch the slot two iterations ahead to overlap store latency
;   with the loop control overhead.
;
;   Equivalent Rust:
;       let slots = PAGE_SIZE / self.obj_size;
;       let mut i = slots;
;       while i > 0 { i -= 1; self.push(page.add(i * self.obj_size)); }
;       self.slab_count += 1;
; ---------------------------------------------------------------------------

%define PAGE_SIZE 0x1000

global slab_populate
slab_populate:
    push    r12
    push    r13
    push    r14

    ; r12 = free_head_ptr
    ; r13 = free_count_ptr
    ; r14 = page base
    mov     r12, rdi
    mov     r13, rsi
    ; rdx = slab_count_ptr (used at the end)
    mov     r14, rcx            ; page base

    ; compute slot count: slots = PAGE_SIZE / obj_size
    ; obj_size is a power of two — BSF gives log2, then shift PAGE_SIZE
    bsf     rcx, r8             ; rcx = log2(obj_size)  (count goes into cl)
    mov     rax, PAGE_SIZE
    shr     rax, cl             ; rax = PAGE_SIZE >> log2(obj_size) = slot count
    mov     rcx, rax            ; rcx = slot count (loop counter)

    ; r8 = obj_size (still valid)
    ; rsi = free_count_ptr (still valid after saving to r13)

    ; i = rcx (slot count), walk down to 0
    ; slot address = r14 + (i-1) * r8

    ; prefetch first two slots we'll process (highest two addresses)
    ; slot[rcx-1] = r14 + (rcx-1)*r8
    lea     rax, [rcx - 1]
    imul    rax, r8
    add     rax, r14
    prefetchnta [rax]

    cmp     rcx, 2
    jb      .push_loop_start
    lea     rax, [rcx - 2]
    imul    rax, r8
    add     rax, r14
    prefetchnta [rax]

.push_loop_start:
    ; rcx counts down: process slot[rcx-1] each iteration
    test    rcx, rcx
    jz      .done_push

.push_loop:
    dec     rcx

    ; compute slot address: rax = r14 + rcx * r8
    mov     rax, rcx
    imul    rax, r8
    add     rax, r14            ; rax = slot ptr

    ; prefetch two slots ahead (slot[rcx-2])
    cmp     rcx, 2
    jb      .skip_prefetch
    lea     r9, [rcx - 2]
    imul    r9, r8
    add     r9, r14
    prefetchnta [r9]
.skip_prefetch:

    ; inline slab_push: link slot into free list
    mov     r9, [r12]           ; r9 = current free_head
    mov     [rax], r9           ; *slot = old head
    mov     [r12], rax          ; free_head = slot
    inc     qword [r13]         ; free_count += 1

    test    rcx, rcx
    jnz     .push_loop

.done_push:
    inc     qword [rdx]         ; slab_count += 1

    pop     r14
    pop     r13
    pop     r12
    ret
