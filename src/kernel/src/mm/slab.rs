// CosinusOS — mm/slab.rs
// Slab allocator — szybki kmalloc/kfree dla kernela
//
// Architektura:
//   • 12 size classes: 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384
//   • Each class: embedded free-list (no external metadata)
//   • Large allocations (> 16384): fall through directly to the PMM
//   • Each slab: one or more pages, no external header
//   • Thread safety: globalny spinlock (TODO: per-CPU caches)
//
// Interfejs publiczny:
//   kmalloc(size)  → *mut u8   (zeroed)
//   kfree(ptr, size)
//   krealloc(ptr, old_size, new_size) → *mut u8

use core::ptr;
use crate::sync::Spinlock;
use super::pmm::{PAGE_SIZE, mm_alloc, mm_free_phys, mm_alloc_nolock, MM_LOCK};

// ── Konfiguracja ──────────────────────────────────────────────────────────────

const SLAB_SIZES: [usize; 12] = [
    8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384,
];
const N_CLASSES: usize = SLAB_SIZES.len();
const MAX_LARGE: usize = SLAB_SIZES[N_CLASSES - 1];

// Ile stron alokujemy na raz dla danej klasy
// (larger classes allocate more pages at once to amortise overhead)
const SLAB_PAGES: [usize; 12] = [
    1, 1, 1, 1, 1, 1, 2, 2, 4, 4, 8, 8,
];

// ── Globalny lock ─────────────────────────────────────────────────────────────

static SLAB_LOCK: Spinlock = Spinlock::new();

// ── SlabClass ─────────────────────────────────────────────────────────────────

struct SlabClass {
    obj_size:  usize,
    slab_pages: usize,
    free_head: *mut FreeNode,
    // Statystyki
    alloc_count: u64,
    free_count:  u64,
    slab_count:  u64,
}

// Every free slot stores a pointer to the next free slot
struct FreeNode {
    next: *mut FreeNode,
}

unsafe impl Send for SlabClass {}
unsafe impl Sync for SlabClass {}

impl SlabClass {
    const fn new(obj_size: usize, slab_pages: usize) -> Self {
        Self {
            obj_size,
            slab_pages,
            free_head: ptr::null_mut(),
            alloc_count: 0,
            free_count:  0,
            slab_count:  0,
        }
    }

    /// Pobierz slot z wolnej listy.
    #[inline]
    unsafe fn pop(&mut self) -> *mut u8 {
        if self.free_head.is_null() { return ptr::null_mut(); }
        let node = self.free_head;
        self.free_head = (*node).next;
        self.alloc_count += 1;
        node as *mut u8
    }

    /// Push one slot back onto the free list.
    #[inline]
    unsafe fn push(&mut self, ptr: *mut u8) {
        let node = ptr as *mut FreeNode;
        (*node).next = self.free_head;
        self.free_head = node;
        self.free_count += 1;
    }

    /// Alokuj nowy slab (1+ stron), podziel na sloty.
    unsafe fn grow(&mut self) -> bool {
        // Alokuj strony
        let pages = self.slab_pages;
        let total = pages * PAGE_SIZE;

        // Allocate the first page as the slab base
        let base = mm_alloc_nolock();
        if base == 0 { return false; }
        core::ptr::write_bytes(base as *mut u8, 0, PAGE_SIZE);

        // Allocate additional pages if the class requires more than one
        for i in 1..pages {
            let p = mm_alloc_nolock();
            if p == 0 {
                // OOM w trakcie — zwolnij to co mamy
                mm_free_phys(base);
                return false;
            }
            core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
            // Only the base address is tracked; a real system would maintain
            // a slab header list.  TODO: buddy allocator.
            let _ = p;
        }

        // Podziel slab na sloty i dodaj do wolnej listy
        let slots = total / self.obj_size;
        // Push in reverse order (last pushed = first used = cache-friendly LIFO)
        let mut i = slots;
        while i > 0 {
            i -= 1;
            let ptr = (base + i as u64 * self.obj_size as u64) as *mut u8;
            // Nie przekraczamy slaba
            if i * self.obj_size + self.obj_size > total { continue; }
            self.push(ptr);
        }
        // Poprawka: pop/push zlicza — skoryguj
        self.free_count -= slots as u64;

        self.slab_count += 1;
        true
    }

    /// Allocate one slot, growing the slab if the free list is empty.
    unsafe fn alloc(&mut self) -> *mut u8 {
        if self.free_head.is_null() {
            if !self.grow() { return ptr::null_mut(); }
        }
        self.pop()
    }
}

// ── Globalne klasy ────────────────────────────────────────────────────────────

static mut CLASSES: [SlabClass; N_CLASSES] = [
    SlabClass::new(SLAB_SIZES[ 0], SLAB_PAGES[ 0]),
    SlabClass::new(SLAB_SIZES[ 1], SLAB_PAGES[ 1]),
    SlabClass::new(SLAB_SIZES[ 2], SLAB_PAGES[ 2]),
    SlabClass::new(SLAB_SIZES[ 3], SLAB_PAGES[ 3]),
    SlabClass::new(SLAB_SIZES[ 4], SLAB_PAGES[ 4]),
    SlabClass::new(SLAB_SIZES[ 5], SLAB_PAGES[ 5]),
    SlabClass::new(SLAB_SIZES[ 6], SLAB_PAGES[ 6]),
    SlabClass::new(SLAB_SIZES[ 7], SLAB_PAGES[ 7]),
    SlabClass::new(SLAB_SIZES[ 8], SLAB_PAGES[ 8]),
    SlabClass::new(SLAB_SIZES[ 9], SLAB_PAGES[ 9]),
    SlabClass::new(SLAB_SIZES[10], SLAB_PAGES[10]),
    SlabClass::new(SLAB_SIZES[11], SLAB_PAGES[11]),
];

// ── Helpers ───────────────────────────────────────────────────────────────────

fn class_for(size: usize) -> Option<usize> {
    SLAB_SIZES.iter().position(|&c| size <= c)
}

fn round_up_pages(size: usize) -> usize {
    (size + PAGE_SIZE - 1) / PAGE_SIZE
}

// ── Publiczny interfejs ───────────────────────────────────────────────────────

/// Allocate `size` zeroed bytes. Returns null on OOM.
/// Zwraca null przy OOM.
pub unsafe fn kmalloc(size: usize) -> *mut u8 {
    if size == 0 { return ptr::null_mut(); }

    if let Some(ci) = class_for(size) {
        SLAB_LOCK.lock();
        MM_LOCK.lock();
        let ptr = CLASSES[ci].alloc();
        MM_LOCK.unlock();
        SLAB_LOCK.unlock();

        if !ptr.is_null() {
            // Zero the slot — it may contain stale data from a previous alloc
            ptr::write_bytes(ptr, 0, SLAB_SIZES[ci]);
        }
        return ptr;
    }

    // Large allocation — fall through to the PMM
    let pages = round_up_pages(size);
    MM_LOCK.lock();
    let p = mm_alloc_nolock();
    // Allocate remaining pages (only the base address is tracked for now)
    // 
    for _ in 1..pages {
        let _ = mm_alloc_nolock();
    }
    MM_LOCK.unlock();

    if p == 0 { return ptr::null_mut(); }
    ptr::write_bytes(p as *mut u8, 0, pages * PAGE_SIZE);
    p as *mut u8
}

/// Free memory allocated by kmalloc.
/// `size` must match the value passed to the original kmalloc call.
pub unsafe fn kfree(ptr: *mut u8, size: usize) {
    if ptr.is_null() || size == 0 { return; }

    if let Some(ci) = class_for(size) {
        SLAB_LOCK.lock();
        CLASSES[ci].push(ptr);
        SLAB_LOCK.unlock();
        return;
    }

    // Large allocation — return pages to the PMM
    let pages = round_up_pages(size);
    for i in 0..pages {
        mm_free_phys(ptr as u64 + i as u64 * PAGE_SIZE as u64);
    }
}

/// Reallocate a block. Old data is copied into the new block.
pub unsafe fn krealloc(ptr: *mut u8, old_size: usize, new_size: usize) -> *mut u8 {
    if ptr.is_null() { return kmalloc(new_size); }
    if new_size == 0 { kfree(ptr, old_size); return ptr::null_mut(); }

    // If the new size fits in the same size class, reuse in place
    if let (Some(old_ci), Some(new_ci)) = (class_for(old_size), class_for(new_size)) {
        if old_ci == new_ci { return ptr; } // same class — reuse in place
    }

    let new_ptr = kmalloc(new_size);
    if new_ptr.is_null() { return ptr::null_mut(); }

    ptr::copy_nonoverlapping(ptr, new_ptr, old_size.min(new_size));
    kfree(ptr, old_size);
    new_ptr
}

/// Allocate a zeroed array of `count` elements of `elem_size` bytes.
pub unsafe fn kcalloc(count: usize, elem_size: usize) -> *mut u8 {
    kmalloc(count.saturating_mul(elem_size))
}

// ── Statystyki ────────────────────────────────────────────────────────────────

pub struct SlabStats {
    pub class_size:   usize,
    pub alloc_count:  u64,
    pub free_count:   u64,
    pub slab_count:   u64,
    pub live_objects: i64,
}

pub unsafe fn slab_stats(ci: usize) -> Option<SlabStats> {
    if ci >= N_CLASSES { return None; }
    let c = &CLASSES[ci];
    Some(SlabStats {
        class_size:   c.obj_size,
        alloc_count:  c.alloc_count,
        free_count:   c.free_count,
        slab_count:   c.slab_count,
        live_objects: c.alloc_count as i64 - c.free_count as i64,
    })
}

pub unsafe fn slab_dump() {
    use crate::debug::serial_print;
    use super::pmm::pnum_serial;
    serial_print("[SLAB] size  alloc   free    live   slabs\n");
    for ci in 0..N_CLASSES {
        let c = &CLASSES[ci];
        if c.alloc_count == 0 { continue; }
        serial_print("[SLAB] ");
        pnum_serial(c.obj_size);
        serial_print("\t");
        pnum_serial(c.alloc_count as usize);
        serial_print("\t");
        pnum_serial(c.free_count as usize);
        serial_print("\t");
        let live = c.alloc_count.saturating_sub(c.free_count);
        pnum_serial(live as usize);
        serial_print("\t");
        pnum_serial(c.slab_count as usize);
        serial_print("\n");
    }
}