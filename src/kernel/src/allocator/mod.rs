// CosinusOS — allocator/mod.rs

pub mod buddy;
pub mod slab;
pub mod kernel_heap;

pub use kernel_heap::{KERNEL_HEAP, KHEAP_BASE, KHEAP_SIZE};
pub use slab::MAX_SLAB_SIZE;
pub use buddy::{PAGE_SIZE as BUDDY_PAGE_SIZE, MAX_ORDER, BUDDY_HEAP_SIZE};

pub unsafe fn init() {
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP) as *mut kernel_heap::KernelHeap);
    heap.init_default();
}

pub unsafe fn init_at(base: usize, size: usize) {
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP) as *mut kernel_heap::KernelHeap);
    heap.init(base, size);
}

pub fn free_kb()  -> usize { KERNEL_HEAP.free_kb() }
pub fn total_kb() -> usize { KERNEL_HEAP.total_kb() }
pub fn used_kb()  -> usize { KERNEL_HEAP.used_kb() }

pub unsafe fn print_stats() {
    use crate::debug::serial_print;
    serial_print("[HEAP] free=");  serial_usize(free_kb());
    serial_print("KB used=");     serial_usize(used_kb());
    serial_print("KB total=");    serial_usize(total_kb());
    serial_print("KB slab=");     serial_usize(KERNEL_HEAP.slab_kb());
    serial_print("KB\n");
    for i in 0..slab::N_CLASSES {
        serial_print("  slab["); serial_usize(slab::SLAB_CLASSES[i]);
        serial_print("B] free="); serial_usize(KERNEL_HEAP.slab_free_slots(i));
        serial_print(" pages=");  serial_usize(KERNEL_HEAP.slab_pages(i));
        serial_print("\n");
    }
}

unsafe fn serial_usize(mut v: usize) {
    use crate::debug::serial_print;
    if v == 0 { serial_print("0"); return; }
    let mut buf = [0u8; 20];
    let mut i = 20usize;
    while v > 0 && i > 0 { i -= 1; buf[i] = b'0' + (v % 10) as u8; v /= 10; }
    if let Ok(s) = core::str::from_utf8(&buf[i..20]) { serial_print(s); }
}