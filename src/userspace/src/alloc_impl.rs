// CosinusOS Userspace — alloc_impl.rs
// Bump allocator (GlobalAlloc) — 10MB heap statyczny

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

const HEAP_SIZE: usize = 10 * 1024 * 1024;

#[repr(align(4096))]
struct AlignedHeap([u8; HEAP_SIZE]);

static mut HEAP: AlignedHeap = AlignedHeap([0; HEAP_SIZE]);
static HEAP_POS: AtomicUsize  = AtomicUsize::new(0);

pub struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size  = layout.size();
        let align = layout.align();
        loop {
            let pos     = HEAP_POS.load(Ordering::Acquire);
            let aligned = (pos + align - 1) & !(align - 1);
            let new_pos = aligned + size;
            if new_pos > HEAP_SIZE { return core::ptr::null_mut(); }
            match HEAP_POS.compare_exchange(
                pos, new_pos, Ordering::AcqRel, Ordering::Acquire
            ) {
                Ok(_)  => return HEAP.0.as_mut_ptr().add(aligned),
                Err(_) => continue,
            }
        }
    }
    // Bump — nie zwalnia pamięci
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
pub static ALLOCATOR: BumpAllocator = BumpAllocator;
