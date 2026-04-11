// CosinusOS — mm/heap.rs
// Kernel heap — GlobalAlloc implementation backed by the slab allocator.
//
// NOTE: #[global_allocator] is NOT declared here — the project already has a
// global allocator in src/allocator/kernel_heap.rs. Declaring a second one
// would cause a compile error ("cannot define multiple global allocators").
//
// This struct is provided as a drop-in replacement. To switch over, replace
// the allocator declaration in src/allocator/kernel_heap.rs with:
//
//   #[global_allocator]
//   static HEAP: mm::heap::KernelHeap = mm::heap::KernelHeap;
//
// and remove the existing one.

use core::alloc::{GlobalAlloc, Layout};
use super::slab::{kmalloc, kfree, krealloc};

pub struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Use max(size, align) so all slab classes satisfy alignment naturally.
        let effective = layout.size().max(layout.align());
        kmalloc(effective)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let effective = layout.size().max(layout.align());
        kfree(ptr, effective);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_effective = layout.size().max(layout.align());
        let new_layout    = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_effective = new_size.max(new_layout.align());
        krealloc(ptr, old_effective, new_effective)
    }
}

/// Print kernel heap / slab statistics to the serial port.
pub unsafe fn heap_dump() {
    super::slab::slab_dump();
}