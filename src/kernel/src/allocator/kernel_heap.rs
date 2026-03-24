// CosinusOS — allocator/kernel_heap.rs
// KernelHeap: slab (<=512B) + buddy (>512B), GlobalAlloc, chroniony spinlockiem.

use core::alloc::{GlobalAlloc, Layout};
use crate::sync::Spinlock;
use super::buddy::BuddyAllocator;
use super::slab::{SlabAllocator, MAX_SLAB_SIZE};

// Zaraz po końcu PMM (0x0F00_0000 = 240MB) → heap od 256MB, 32MB długości
pub const KHEAP_BASE: usize = 0x1000_0000;
pub const KHEAP_SIZE: usize = 32 * 1024 * 1024;

pub struct KernelHeap {
    lock:   Spinlock,
    buddy:  BuddyAllocator,
    slab:   SlabAllocator,
    inited: bool,
}

// SAFETY: wszystkie pola chronione spinlockiem
unsafe impl Sync for KernelHeap {}
unsafe impl Send for KernelHeap {}

impl KernelHeap {
    pub const fn new() -> Self {
        Self {
            lock:   Spinlock::new(),
            buddy:  BuddyAllocator::new(),
            slab:   SlabAllocator::new(),
            inited: false,
        }
    }

    pub unsafe fn init(&mut self, base: usize, size: usize) {
        self.buddy.init(base, size);
        self.slab.init(slab_page_alloc, slab_page_free);
        self.inited = true;
    }

    pub unsafe fn init_default(&mut self) {
        self.init(KHEAP_BASE, KHEAP_SIZE);
    }

    pub fn free_kb(&self)  -> usize { self.buddy.free_kb() }
    pub fn total_kb(&self) -> usize { self.buddy.total_kb() }
    pub fn used_kb(&self)  -> usize { self.buddy.used_kb() }
    pub fn slab_kb(&self)  -> usize { self.slab.total_slab_kb() }
    pub fn slab_free_slots(&self, class: usize) -> usize { self.slab.free_slots(class) }
    pub fn slab_pages(&self, class: usize)      -> usize { self.slab.slab_pages(class) }

    unsafe fn do_alloc(&mut self, layout: Layout) -> *mut u8 {
        if !self.inited { return core::ptr::null_mut(); }
        let size  = layout.size();
        let align = layout.align();
        if size == 0 { return core::ptr::null_mut(); }
        if size <= MAX_SLAB_SIZE && align <= size {
            let ptr = self.slab.alloc(size);
            if !ptr.is_null() { return ptr; }
        }
        self.buddy.alloc(size, align)
    }

    unsafe fn do_dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        if !self.inited || ptr.is_null() { return; }
        let size  = layout.size();
        let align = layout.align();
        if size <= MAX_SLAB_SIZE && align <= size {
            self.slab.free(ptr, size);
        } else {
            self.buddy.free(ptr, size);
        }
    }
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let heap = &mut *(self as *const KernelHeap as *mut KernelHeap);
        heap.lock.lock();
        let ptr = heap.do_alloc(layout);
        heap.lock.unlock();
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let heap = &mut *(self as *const KernelHeap as *mut KernelHeap);
        heap.lock.lock();
        heap.do_dealloc(ptr, layout);
        heap.lock.unlock();
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
        }
        if new_size == 0 {
            self.dealloc(ptr, layout);
            return core::ptr::null_mut();
        }
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if new_ptr.is_null() { return core::ptr::null_mut(); }
        core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
        self.dealloc(ptr, layout);
        new_ptr
    }
}

// Callbacki slab→buddy. Wywoływane wewnątrz do_alloc kiedy lock już trzymany —
// używamy buddy bezpośrednio żeby uniknąć re-lock.
unsafe fn slab_page_alloc() -> *mut u8 {
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP) as *mut KernelHeap);
    heap.buddy.alloc(super::buddy::PAGE_SIZE, super::buddy::PAGE_SIZE)
}

unsafe fn slab_page_free(ptr: *mut u8) {
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP) as *mut KernelHeap);
    heap.buddy.free(ptr, super::buddy::PAGE_SIZE);
}

#[global_allocator]
pub static KERNEL_HEAP: KernelHeap = KernelHeap::new();