// CosinusOS — allocator/kernel_heap.rs
//
// KernelHeap: slab (<=512B) + buddy (>512B), GlobalAlloc, chroniony spinlockiem.
// Warstwa Ada: integrity checks + audit log + lifecycle po każdym alloc/free.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use crate::sync::Spinlock;
use super::buddy::BuddyAllocator;
use super::slab::{SlabAllocator, MAX_SLAB_SIZE};
use super::ada_shims;

pub const KHEAP_BASE: usize = 0x1000_0000;
pub const KHEAP_SIZE: usize = 32 * 1024 * 1024;

struct HeapInner {
    buddy:  BuddyAllocator,
    slab:   SlabAllocator,
    inited: bool,
}

pub struct KernelHeap {
    lock:  Spinlock,
    inner: UnsafeCell<HeapInner>,
}

unsafe impl Sync for KernelHeap {}
unsafe impl Send for KernelHeap {}

impl KernelHeap {
    pub const fn new() -> Self {
        Self {
            lock:  Spinlock::new(),
            inner: UnsafeCell::new(HeapInner {
                buddy:  BuddyAllocator::new(),
                slab:   SlabAllocator::new(),
                inited: false,
            }),
        }
    }

    pub unsafe fn init(&self, base: usize, size: usize) {
        let inner = &mut *self.inner.get();
        inner.buddy.init(base, size);
        inner.slab.init(slab_page_alloc, slab_page_free);
        inner.inited = true;
        // Inform Ada layer
        ada_shims::ada_alloc_init(base as *const u8, size as u64);
    }

    pub unsafe fn init_default(&self) {
        self.init(KHEAP_BASE, KHEAP_SIZE);
    }

    pub unsafe fn reinit(&self, base: usize, size: usize) {
        let inner = &mut *self.inner.get();
        inner.buddy.init(base, size);
        inner.inited = true;
        ada_shims::ada_alloc_reinit(base as *const u8, size as u64);
    }

    pub unsafe fn shutdown(&self) {
        ada_shims::ada_alloc_shutdown();
        let inner = &mut *self.inner.get();
        inner.inited = false;
    }

    // --- Stats ---
    pub fn free_kb(&self)  -> usize { unsafe { (*self.inner.get()).buddy.free_kb() } }
    pub fn total_kb(&self) -> usize { unsafe { (*self.inner.get()).buddy.total_kb() } }
    pub fn used_kb(&self)  -> usize { unsafe { (*self.inner.get()).buddy.used_kb() } }
    pub fn slab_kb(&self)  -> usize { unsafe { (*self.inner.get()).slab.total_slab_kb() } }
    pub fn slab_free_slots(&self, c: usize) -> usize {
        unsafe { (*self.inner.get()).slab.free_slots(c) }
    }
    pub fn slab_pages(&self, c: usize) -> usize {
        unsafe { (*self.inner.get()).slab.slab_pages(c) }
    }

    /// Ada audit stats: (total_allocs, total_frees, live_bytes)
    pub fn ada_stats(&self) -> (u64, u64, u64) {
        unsafe { ada_shims::audit_stats() }
    }

    /// Zrzuć ostatnie N wpisów audit logu na serial.
    pub fn ada_dump(&self, n: u32) {
        unsafe { ada_shims::ada_audit_dump(n); }
    }

    /// Numer wersji stanu allokatora (rośnie przy init/reinit).
    pub fn ada_version(&self) -> u32 {
        unsafe { ada_shims::ada_alloc_version() }
    }
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock.lock();
        let inner = &mut *self.inner.get();
        let ptr = do_alloc(inner, layout);
        self.lock.unlock();
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.lock.lock();
        let inner = &mut *self.inner.get();
        do_dealloc(inner, ptr, layout);
        self.lock.unlock();
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

// ---------------------------------------------------------------------------
// Core alloc / dealloc — z warstwą Ady
// ---------------------------------------------------------------------------

unsafe fn do_alloc(inner: &mut HeapInner, layout: Layout) -> *mut u8 {
    if !inner.inited { return core::ptr::null_mut(); }
    let size  = layout.size();
    let align = layout.align();
    if size == 0 { return core::ptr::null_mut(); }

    let is_slab = size <= MAX_SLAB_SIZE && align <= size;

    let ptr = if is_slab {
        let p = inner.slab.alloc(size);
        if p.is_null() { inner.buddy.alloc(size, align) } else { p }
    } else {
        inner.buddy.alloc(size, align)
    };

    if ptr.is_null() { return core::ptr::null_mut(); }

    // Ada: bounds check + write canary + audit log
    let ok = ada_shims::on_alloc(
        ptr,
        size,
        inner.buddy.base as *const u8,
        inner.buddy.size,
        is_slab,
    );
    if !ok {
        // Bounds check failed — put block back and return null
        if is_slab { inner.slab.free(ptr, size); }
        else        { inner.buddy.free(ptr, size); }
        return core::ptr::null_mut();
    }

    ptr
}

unsafe fn do_dealloc(inner: &mut HeapInner, ptr: *mut u8, layout: Layout) {
    if !inner.inited || ptr.is_null() { return; }
    let size  = layout.size();
    let align = layout.align();

    let is_slab = size <= MAX_SLAB_SIZE && align <= size;

    // Ada: double-free check + canary check + audit log
    // If check fails we do NOT free — prevents use-after-free from corrupted state.
    // The serial output from Ada will identify the culprit.
    if !ada_shims::on_free(ptr, size, is_slab) {
        // Corruption detected — kernel should panic here.
        // We call shutdown to flush the audit log, then halt.
        ada_shims::ada_alloc_shutdown();
        core::arch::asm!("cli; hlt", options(noreturn, nostack));
    }

    if is_slab { inner.slab.free(ptr, size); }
    else        { inner.buddy.free(ptr, size); }
}

// ---------------------------------------------------------------------------
// Slab page callbacks — buddy alloc/free for slab backing pages
// ---------------------------------------------------------------------------

unsafe fn slab_page_alloc() -> *mut u8 {
    let inner = &mut *KERNEL_HEAP.inner.get();
    inner.buddy.alloc(super::buddy::PAGE_SIZE, super::buddy::PAGE_SIZE)
}

unsafe fn slab_page_free(ptr: *mut u8) {
    let inner = &mut *KERNEL_HEAP.inner.get();
    inner.buddy.free(ptr, super::buddy::PAGE_SIZE);
}

#[global_allocator]
pub static KERNEL_HEAP: KernelHeap = KernelHeap::new();