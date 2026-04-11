// CosinusOS — mm/mod.rs
// Memory Manager — root module
//
// Struktura:
//   mm/
//   ├── mod.rs      — ten plik, re-eksporty, inicjalizacja
//   ├── pmm.rs      — Physical Memory Manager (bitmap)
//   ├── vmm.rs      — Virtual Memory Manager (page tables, CoW)
//   ├── frame.rs    — Refcounting ramek (dla CoW i shared mappings)
//   ├── vma.rs      — Virtual Memory Areas (demand paging, ASLR, heap)
//   ├── slab.rs     — Slab allocator (kmalloc/kfree)
//   ├── heap.rs     — GlobalAlloc kernel heap (delegates to slab)
//   └── user.rs     — Userspace helpers (ELF load, syscall backend, #PF dispatch)
//
// Initialisation order (call from kernel main):
//   1. mm::init(phys_base, phys_size)  — PMM + VMM
//   2. (opcjonalnie) mm::heap_dump()   — diagnostyka
//
// Publiczne API (re-eksportowane na poziomie mm::):
//   PMM:   mm_alloc, mm_free_phys, mm_alloc_huge, mm_free_huge
//          mm_free_kb, mm_used_kb, mm_total_kb
//   VMM:   vmap, vunmap, vmap_huge, vunmap_huge
//          new_user_p4, clone_user_p4, free_user_p4
//          virt_to_phys, valid_user, valid_buf
//          handle_cow_fault, tlb_flush_page, tlb_flush_all
//   Frame: frame_inc, frame_dec, frame_shared, frame_cow_copy
//   VMA:   AddressSpace, VMA_* flags
//   Slab:  kmalloc, kfree, krealloc, kcalloc
//   User:  map_elf_segment, legacy_mem_alloc, legacy_mem_free
//          handle_page_fault, install_guard_page
//          make_swap_pte, swap_pte_slot
//
// Backwards-compatible with the original single-file mm.rs:
//   • PhysAddr, VirtAddr, PAGE_SIZE
//   • PTE_W, PTE_U, PTE_P, PTE_ADDR
//   • MM_LOCK, K_P4
//   • mm_alloc(), mm_alloc_nolock(), mm_free_phys(), mm_free_nolock()
//   • vmap(), vunmap(), virt_to_phys(), valid_user(), valid_buf()
//   • new_user_p4(), vmm_init(), mm_init()
//   • mm_free_kb(), mm_used_kb(), mm_total_kb()

pub mod frame;
pub mod pmm;
pub mod vmm;
pub mod vma;
pub mod slab;
pub mod heap;
pub mod user;

// ── Re-exports ───────────────────────────────────────────────────────────────────

pub use pmm::{
    PhysAddr, VirtAddr, PAGE_SIZE, HUGE_SIZE, MAX_FRAMES,
    MM_LOCK,
    mm_init, mm_alloc, mm_alloc_nolock, mm_alloc_zeroed,
    mm_alloc_huge, mm_free_phys, mm_free_nolock, mm_free_huge,
    mm_free_kb, mm_used_kb, mm_total_kb, mm_free_pages, mm_dump_stats,
};

// Backwards-compat constants that the original mm.rs exported at crate level
pub const KERNEL_STACK_SIZE: usize = 0x8000; // 32 KB (dla threading.rs)

pub use vmm::{
    // PTE flags
    PTE_P, PTE_W, PTE_U, PTE_PWT, PTE_PCD, PTE_A, PTE_D,
    PTE_PS, PTE_G, PTE_COW, PTE_NX, PTE_ADDR,
    // VMM state
    K_P4,
    // Typy
    PT, pt_ptr,
    // Helpers
    pte_make, pte_present, pte_user, pte_writable, pte_huge, pte_cow, pte_addr,
    // TLB
    tlb_flush_page, tlb_flush_all,
    // Scratch alloc
    zpg_locked,
    // Init
    vmm_init,
    // Mapping
    vmap, vunmap, vmap_huge, vunmap_huge,
    // Translacja
    virt_to_phys, valid_user, valid_buf,
    // P4 management
    new_user_p4, clone_user_p4, free_user_p4,
    // CoW
    handle_cow_fault,
};

pub use frame::{
    frame_ref, frame_inc, frame_dec, frame_init,
    frame_shared, frame_cow_copy, frame_cow_inplace,
};

pub use vma::{
    AddressSpace,
    VMA_R, VMA_W, VMA_X, VMA_USER, VMA_DEMAND, VMA_GUARD,
    VMA_STACK, VMA_HEAP, VMA_SHARED, VMA_FIXED,
    USER_CODE_BASE, USER_HEAP_BASE, USER_MMAP_BASE,
    USER_STACK_TOP, USER_STACK_SIZE,
    VmaType, Vma,
};

pub use slab::{kmalloc, kfree, krealloc, kcalloc, slab_dump};

pub use user::{
    map_elf_segment,
    legacy_mem_alloc, legacy_mem_free,
    find_free_user_range,
    handle_page_fault,
    install_guard_page,
    make_swap_pte, swap_pte_slot,
    PTE_SWAP, PTE_SWAP_SLOT_SHIFT, PTE_SWAP_SLOT_MASK,
};

// ── Inicjalizacja ─────────────────────────────────────────────────────────────

/// Initialise the entire memory subsystem.
/// Call once from kernel main after detecting available RAM.
///
/// `phys_base` — base address of available RAM (PAGE_SIZE-aligned)
/// `phys_size` — size of available RAM in bytes
/// `boot_cr3`  — CR3 value provided by the bootloader
pub unsafe fn init(phys_base: PhysAddr, phys_size: usize, boot_cr3: PhysAddr) {
    mm_init(phys_base, phys_size);
    vmm_init(boot_cr3);
}

// ── Diagnostyka ───────────────────────────────────────────────────────────────

pub unsafe fn dump_all() {
    mm_dump_stats();
    slab_dump();
}