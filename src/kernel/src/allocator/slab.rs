// CosinusOS — allocator/slab_asm_shims.rs
//
// Shim layer between SlabClass (Rust) and slab_hotpath.asm.
// Because SlabClass does not use repr(C), we pass individual field references
// rather than a pointer to the whole struct. This keeps the ABI stable even
// if the Rust struct layout changes.
//
// Usage: call the asm_slab_* wrappers from SlabClass methods.

// ---------------------------------------------------------------------------
// FFI declarations — symbols exported from slab_hotpath.asm
// ---------------------------------------------------------------------------
extern "C" {
    /// Pops the head slot from the free list.
    /// free_head_ptr: *mut (*mut u8) — pointer to cls.free_head
    /// free_count_ptr: *mut usize   — pointer to cls.free_count
    /// Returns: the allocated slot, or null if the list is empty.
    fn slab_pop(
        free_head_ptr:  *mut *mut u8,
        free_count_ptr: *mut usize,
    ) -> *mut u8;

    /// Pushes `ptr` onto the free list head.
    fn slab_push(
        free_head_ptr:  *mut *mut u8,
        free_count_ptr: *mut usize,
        ptr:            *mut u8,
    );

    /// Populates a fresh 4096-byte page into the free list.
    /// obj_size must be a power of two and >= 8.
    fn slab_populate(
        free_head_ptr:   *mut *mut u8,
        free_count_ptr:  *mut usize,
        slab_count_ptr:  *mut usize,
        page:            *mut u8,
        obj_size:        usize,
    );
}

// ---------------------------------------------------------------------------
// Wrapper functions called from SlabClass
// ---------------------------------------------------------------------------

/// Pop from slab free list with prefetch. Returns null if empty.
#[inline(always)]
pub unsafe fn asm_slab_pop(
    free_head:  &mut *mut *mut u8,
    free_count: &mut usize,
) -> *mut u8 {
    slab_pop(
        free_head  as *mut *mut *mut u8 as *mut *mut u8,
        free_count as *mut usize,
    )
}

/// Push onto slab free list with prefetch.
#[inline(always)]
pub unsafe fn asm_slab_push(
    free_head:  &mut *mut *mut u8,
    free_count: &mut usize,
    ptr:        *mut u8,
) {
    slab_push(
        free_head  as *mut *mut *mut u8 as *mut *mut u8,
        free_count as *mut usize,
        ptr,
    );
}

/// Populate an entire fresh slab page into the free list.
#[inline(always)]
pub unsafe fn asm_slab_populate(
    free_head:   &mut *mut *mut u8,
    free_count:  &mut usize,
    slab_count:  &mut usize,
    page:        *mut u8,
    obj_size:    usize,
) {
    slab_populate(
        free_head  as *mut *mut *mut u8 as *mut *mut u8,
        free_count as *mut usize,
        slab_count as *mut usize,
        page,
        obj_size,
    );
}