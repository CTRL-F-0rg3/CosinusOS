// CosinusOS — allocator/mod.rs
// Moduł alokacji pamięci kernela: Slab + Buddy
//
// ┌─────────────────────────────────────────────────────────────────────┐
// │                        KernelHeap                                   │
// │                                                                     │
// │   size <= 512B  →  SlabAllocator  (7 klas: 8..512B, O(1))          │
// │   size >  512B  →  BuddyAllocator (order 0..9: 4KB..2MB, O(log n)) │
// │                                                                     │
// │   #[global_allocator] → Box<T>, Vec<T>, String dostępne w kernelu  │
// └─────────────────────────────────────────────────────────────────────┘
//
// Użycie w kernel_main:
//
//   unsafe {
//       // 1. Zmapuj region KHEAP_BASE..KHEAP_BASE+KHEAP_SIZE
//       for i in 0..(KHEAP_SIZE / PAGE_SIZE) {
//           let phys = mm_alloc();
//           vmap(K_P4, KHEAP_BASE as u64 + i as u64 * PAGE_SIZE as u64,
//                phys, PTE_W);
//       }
//       // 2. Zainicjalizuj heap
//       allocator::init();
//   }
//
// Po init() Box/Vec/String są dostępne w całym kernelu.
//
// Diagnoza / debug:
//
//   allocator::print_stats();  →  drukuje na serial wolne KB, slab pages, itd.

pub mod buddy;
pub mod slab;
pub mod kernel_heap;

pub use kernel_heap::{KERNEL_HEAP, KHEAP_BASE, KHEAP_SIZE};
pub use slab::MAX_SLAB_SIZE;
pub use buddy::{PAGE_SIZE as BUDDY_PAGE_SIZE, MAX_ORDER, BUDDY_HEAP_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// Publiczne API
// ─────────────────────────────────────────────────────────────────────────────

/// Inicjalizacja heap z domyślnym KHEAP_BASE/KHEAP_SIZE.
///
/// Musi być wywołana po zmapowaniu regionu KHEAP_BASE..KHEAP_BASE+KHEAP_SIZE.
/// Wywołaj dokładnie raz z kernel_main.
///
/// # Panic
/// Panics (debug) jeśli wywołana po raz drugi.
pub unsafe fn init() {
    // SAFETY: wywołana raz z kernel_main, przed jakimikolwiek alokacjami
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP)
        as *mut kernel_heap::KernelHeap);
    heap.init_default();
}

/// Inicjalizacja heap z niestandardowym adresem i rozmiarem.
///
/// `base` musi być wyrównany do 2MB (MAX_BLOCK).
/// `size` musi być wielokrotnością 2MB.
pub unsafe fn init_at(base: usize, size: usize) {
    let heap = &mut *(core::ptr::addr_of!(KERNEL_HEAP)
        as *mut kernel_heap::KernelHeap);
    heap.init(base, size);
}

/// Zwróć wolne KB w buddy heap
pub fn free_kb() -> usize {
    KERNEL_HEAP.free_kb()
}

/// Zwróć łączne KB heap
pub fn total_kb() -> usize {
    KERNEL_HEAP.total_kb()
}

/// Zwróć użyte KB w buddy heap
pub fn used_kb() -> usize {
    KERNEL_HEAP.used_kb()
}

/// Wydrukuj statystyki na serial (bez alokacji)
pub fn print_stats() {
    use crate::debug::serial_print;

    serial_print("[HEAP] free=");
    serial_usize(free_kb());
    serial_print("KB  used=");
    serial_usize(used_kb());
    serial_print("KB  total=");
    serial_usize(total_kb());
    serial_print("KB  slab=");
    serial_usize(KERNEL_HEAP.slab_kb());
    serial_print("KB\n");

    // Per-klasa slab
    for i in 0..slab::N_CLASSES {
        serial_print("  slab[");
        serial_usize(slab::SLAB_CLASSES[i]);
        serial_print("B]: free_slots=");
        serial_usize(KERNEL_HEAP.slab_free_slots(i));
        serial_print("  pages=");
        serial_usize(KERNEL_HEAP.slab_pages(i));
        serial_print("\n");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper serial bez alokacji (nie możemy użyć format! przed init heap)
// ─────────────────────────────────────────────────────────────────────────────
fn serial_usize(mut v: usize) {
    use crate::debug::serial_print;
    if v == 0 { serial_print("0"); return; }
    let mut buf = [0u8; 20];
    let mut i = 19usize;
    while v > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    // Bezpieczne — buf[i..] to ascii cyfry
    if let Ok(s) = core::str::from_utf8(&buf[i..20]) {
        serial_print(s);
    }
}
