// CosinusOS — mm/heap.rs
// Kernel heap — GlobalAlloc backed by slab allocator
//
// Implementuje GlobalAlloc dla kernela (używane przez alloc::vec, Box, etc.)
// Deleguje do slab::kmalloc / slab::kfree.
//
// Uwaga: rozmiar przy dealloc() musi odpowiadać temu z alloc() —
// Rust globalny alokator zawsze przekazuje oryginalny Layout do dealloc.

use core::alloc::{GlobalAlloc, Layout};
use super::slab::{kmalloc, kfree, krealloc};

// ── GlobalAlloc impl ──────────────────────────────────────────────────────────

pub struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Używamy max(size, align) żeby mieć pewność że alignment jest spełniony
        // dla klas slabów (wszystkie są naturalnie wyrównane do swojego rozmiaru)
        let effective = layout.size().max(layout.align());
        kmalloc(effective)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let effective = layout.size().max(layout.align());
        kfree(ptr, effective);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_effective = layout.size().max(layout.align());
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_effective = new_size.max(new_layout.align());
        krealloc(ptr, old_effective, new_effective)
    }
}

#[global_allocator]
pub static KERNEL_HEAP: KernelHeap = KernelHeap;

// ── Heap stats ────────────────────────────────────────────────────────────────

/// Wypisz stan kernel heap (deleguje do slab_dump).
pub unsafe fn heap_dump() {
    super::slab::slab_dump();
}
