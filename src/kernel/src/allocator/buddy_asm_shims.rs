// CosinusOS — allocator/buddy_asm_shims.rs
//
// Drop-in replacement for the three bitmap methods in BuddyAllocator.
// Include this via `mod buddy_asm_shims;` in buddy.rs, then call
// the shim methods instead of the Rust implementations.
//
// Build: bitmap_ops.asm must be compiled and linked by build.zig before
// this crate is linked. See build.zig for the nasm invocation.

// ---------------------------------------------------------------------------
// FFI declarations — symbols exported from bitmap_ops.asm
// ---------------------------------------------------------------------------
extern "C" {
    /// Sets n_pages bits starting at start_page (marks pages as FREE).
    fn bitmap_set_free(bitmap: *mut u64, start_page: usize, n_pages: usize);

    /// Clears n_pages bits starting at start_page (marks pages as USED).
    fn bitmap_set_used(bitmap: *mut u64, start_page: usize, n_pages: usize);

    /// Returns true if all n_pages bits starting at start_page are set (free).
    fn bitmap_is_free(bitmap: *const u64, start_page: usize, n_pages: usize) -> bool;

    /// Scans the bitmap for the first set bit (first free page).
    /// Returns usize::MAX if no free page found.
    #[allow(dead_code)]
    fn bitmap_find_free(bitmap: *const u64, words: usize) -> usize;
}

// ---------------------------------------------------------------------------
// Shim helpers — call from BuddyAllocator impl via `use super::buddy_asm_shims::*`
// ---------------------------------------------------------------------------

/// Inline wrapper: marks `1 << order` pages starting at `addr` as free.
/// `base` is `self.base`, `bitmap` is `self.bitmap.as_mut_ptr()`.
#[inline(always)]
pub unsafe fn asm_bitmap_set_free(
    bitmap: *mut u64,
    base:   usize,
    addr:   usize,
    order:  usize,
) {
    let start_page = (addr - base) >> 12; // / PAGE_SIZE
    let n_pages    = 1usize << order;
    bitmap_set_free(bitmap, start_page, n_pages);
}

/// Inline wrapper: marks `1 << order` pages starting at `addr` as used.
#[inline(always)]
pub unsafe fn asm_bitmap_set_used(
    bitmap: *mut u64,
    base:   usize,
    addr:   usize,
    order:  usize,
) {
    let start_page = (addr - base) >> 12;
    let n_pages    = 1usize << order;
    bitmap_set_used(bitmap, start_page, n_pages);
}

/// Inline wrapper: returns true if all pages in [addr, addr + block_size(order))
/// are marked free.
#[inline(always)]
pub unsafe fn asm_bitmap_is_free(
    bitmap: *const u64,
    base:   usize,
    addr:   usize,
    order:  usize,
) -> bool {
    let start_page = (addr - base) >> 12;
    let n_pages    = 1usize << order;
    bitmap_is_free(bitmap, start_page, n_pages)
}
