// CosinusOS — allocator/kernel_heap.rs
// KernelHeap — łączy SlabAllocator (małe) i BuddyAllocator (duże)
//
// Granica podziału: <= MAX_SLAB_SIZE (512B) → slab, > 512B → buddy
//
// Heap region kernela:
//   Zarezerwowany w przestrzeni adresowej kernela: KHEAP_BASE..KHEAP_BASE+KHEAP_SIZE
//   Region musi być zmapowany przed wywołaniem KernelHeap::init().
//   Typowo kernel_main mapuje go bezpośrednio (identity lub offset mapping).
//
//   KHEAP_BASE: 0x0100_0000_0000  (1TB — daleko od kernela i userspace)
//   KHEAP_SIZE: 32MB (32 * 1024 * 1024)
//
// Thread safety:
//   Cały heap jest chroniony jednym Spinlockiem.
//   W przyszłości można przejść na per-CPU slab magazines (Linux-style).
//
// GlobalAlloc impl:
//   alloc   → slab.alloc(size) jeśli <=512B, inaczej buddy.alloc(size, align)
//   dealloc → slab.free(ptr, size) jeśli <=512B, inaczej buddy.free(ptr, size)
//   realloc → alloc nowy + copy + free stary (bez reuse)

use core::alloc::{GlobalAlloc, Layout};
use crate::sync::Spinlock;
use super::buddy::BuddyAllocator;
use super::slab::{SlabAllocator, MAX_SLAB_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// Konfiguracja heap
// ─────────────────────────────────────────────────────────────────────────────

/// Bazowy adres wirtualny kernel heap (musi być zmapowany przed init)
/// Ustaw w linker script lub kernel_main na wolny region kernela.
/// Tymczasowo 0x0100_0000_0000 (1TB w wirtualnej przestrzeni kernela x86-64)
pub const KHEAP_BASE: usize = 0x0100_0000_0000;

/// Rozmiar kernel heap (musi być wielokrotnością 2MB dla buddy)
pub const KHEAP_SIZE: usize = 32 * 1024 * 1024; // 32MB

// ─────────────────────────────────────────────────────────────────────────────
// KernelHeap
// ─────────────────────────────────────────────────────────────────────────────

pub struct KernelHeap {
    lock:   Spinlock,
    buddy:  BuddyAllocator,
    slab:   SlabAllocator,
    inited: bool,
}

impl KernelHeap {
    pub const fn new() -> Self {
        Self {
            lock:   Spinlock::new(),
            buddy:  BuddyAllocator::new(),
            slab:   SlabAllocator::new(),
            inited: false,
        }
    }

    /// Inicjalizacja — wywołaj z kernel_main po zmapowaniu KHEAP regionu.
    ///
    /// # Safety
    /// `base..base+size` musi być zmapowany i zapisywalny (PTE_W).
    /// Wywołaj dokładnie raz przed jakąkolwiek alokacją.
    pub unsafe fn init(&mut self, base: usize, size: usize) {
        self.buddy.init(base, size);
        // Slab dostaje strony z buddy (order-0 = PAGE_SIZE bloki)
        self.slab.init(slab_page_alloc, slab_page_free);
        self.inited = true;
    }

    /// Wersja dla domyślnego KHEAP_BASE/KHEAP_SIZE
    pub unsafe fn init_default(&mut self) {
        self.init(KHEAP_BASE, KHEAP_SIZE);
    }

    // ── Statystyki ───────────────────────────────────────────────────────────

    pub fn free_kb(&self)  -> usize { self.buddy.free_kb() }
    pub fn total_kb(&self) -> usize { self.buddy.total_kb() }
    pub fn used_kb(&self)  -> usize { self.buddy.used_kb() }
    pub fn slab_kb(&self)  -> usize { self.slab.total_slab_kb() }

    pub fn slab_free_slots(&self, class: usize) -> usize { self.slab.free_slots(class) }
    pub fn slab_pages(&self, class: usize)      -> usize { self.slab.slab_pages(class) }

    // ── Wewnętrzne ───────────────────────────────────────────────────────────

    unsafe fn do_alloc(&mut self, layout: Layout) -> *mut u8 {
        if !self.inited { return core::ptr::null_mut(); }

        let size  = layout.size();
        let align = layout.align();

        if size == 0 { return core::ptr::null_mut(); }

        if size <= MAX_SLAB_SIZE && align <= size {
            // Mała alokacja — slab
            let ptr = self.slab.alloc(size);
            if !ptr.is_null() { return ptr; }
            // Fallback do buddy jeśli slab nie może dostać strony
        }

        // Duża alokacja lub fallback — buddy
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

// ─────────────────────────────────────────────────────────────────────────────
// GlobalAlloc impl
// ─────────────────────────────────────────────────────────────────────────────

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Spinlock gwarantuje wyłączność.
        // Używamy *const → *mut cast — KernelHeap jest statycznym globalem.
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

        // Kopiuj min(old_size, new_size) bajtów
        let copy_sz = layout.size().min(new_size);
        core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_sz);

        self.dealloc(ptr, layout);
        new_ptr
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Callbacki slab → buddy (wolne funkcje bo Rust nie lubi closures w no_std static)
// ─────────────────────────────────────────────────────────────────────────────

/// Alokuj jedną stronę (4KB) z buddy dla slab
unsafe fn slab_page_alloc() -> *mut u8 {
    // Dostęp do globalnego KERNEL_HEAP — slab_page_alloc wywoływana tylko
    // wewnątrz KernelHeap::do_alloc kiedy lock jest już trzymany,
    // więc pomijamy lock tutaj (reentrancy!)
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP) as *mut KernelHeap);
    heap.buddy.alloc(super::buddy::PAGE_SIZE, super::buddy::PAGE_SIZE)
}

/// Zwolnij jedną stronę (4KB) do buddy
unsafe fn slab_page_free(ptr: *mut u8) {
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP) as *mut KernelHeap);
    heap.buddy.free(ptr, super::buddy::PAGE_SIZE);
}

// ─────────────────────────────────────────────────────────────────────────────
// Globalny allocator kernela
// ─────────────────────────────────────────────────────────────────────────────

#[global_allocator]
pub static KERNEL_HEAP: KernelHeap = KernelHeap::new();
