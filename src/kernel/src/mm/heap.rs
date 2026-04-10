// CosinusOS — mm/heap.rs
// Kernel heap — GlobalAlloc backed by slab allocator
//
// UWAGA: #[global_allocator] NIE jest tutaj — projekt ma już
// własny allocator w src/allocator/kernel_heap.rs.
// Ten plik dostarcza KernelHeap jako alternatywę do ewentualnej zamiany.

use core::alloc::{GlobalAlloc, Layout};
use super::slab::{kmalloc, kfree, krealloc};

pub struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
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

// Żeby przełączyć na slab heap, dodaj w kernel_heap.rs:
//   #[global_allocator]
//   static HEAP: mm::heap::KernelHeap = mm::heap::KernelHeap;

pub unsafe fn heap_dump() {
    super::slab::slab_dump();
}