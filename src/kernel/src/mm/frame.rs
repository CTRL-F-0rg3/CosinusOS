// CosinusOS — mm/frame.rs
// Frame reference counting table.
//
// Every physical frame has a u16 reference counter.
// Used by CoW: fork() increments refcount, a write decrements and copies.
// Przy refcount == 0 ramka wraca do PMM.
//
// Layout: FRAME_REFS[i] = refcount ramki o indeksie i (fi(phys) z PMM).
// Tablica jest statyczna — MAX_FRAMES * 2 bajty = 128 KB dla 64K ramek.

use core::sync::atomic::{AtomicU16, Ordering};
use super::pmm::{fi, fp, MEM_BASE, MAX_FRAMES, PAGE_SIZE};
use super::pmm;

// ── Tablica refcount ──────────────────────────────────────────────────────────

static FRAME_REFS: [AtomicU16; MAX_FRAMES] = {
    // const init trick — AtomicU16::new(0) jest const
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicU16 = AtomicU16::new(0);
    [Z; MAX_FRAMES]
};

// ── API ───────────────────────────────────────────────────────────────────────

/// Pobierz aktualny refcount ramki.
#[inline]
pub fn frame_ref(phys: u64) -> u16 {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return 0; }
    FRAME_REFS[i].load(Ordering::Relaxed)
}

/// Increment refcount (e.g. on fork / shared mapping).
/// On overflow (> 65535 sharers) the counter is pinned at MAX.
#[inline]
pub fn frame_inc(phys: u64) {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return; }
    let prev = FRAME_REFS[i].fetch_add(1, Ordering::AcqRel);
    if prev == u16::MAX {
        // Pin at MAX — do not wrap around (a leak is safer than use-after-free)
        FRAME_REFS[i].store(u16::MAX, Ordering::Release);
    }
}

/// Ustaw refcount na 1 (przy pierwszej alokacji ramki przez PMM).
#[inline]
pub fn frame_init(phys: u64) {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return; }
    FRAME_REFS[i].store(1, Ordering::Release);
}

/// Decrement refcount. Returns true when the frame was freed.
/// Returns false if other owners remain.
pub fn frame_dec(phys: u64) -> bool {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return false; }

    // Permanently-pinned frames (refcount == MAX) are never decremented
    let cur = FRAME_REFS[i].load(Ordering::Acquire);
    if cur == u16::MAX { return false; } // permanently pinned

    let prev = FRAME_REFS[i].fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        // Refcount dropped to 0 — return frame to PMM
        unsafe { pmm::mm_free_phys(phys); }
        return true;
    }
    false
}

/// Czy ramka jest CoW-shared (refcount > 1)?
#[inline]
pub fn frame_shared(phys: u64) -> bool {
    frame_ref(phys) > 1
}

/// Perform a CoW copy: allocate a new frame, copy contents, return it.
/// Called by the page fault handler on a write to a CoW page.
///
/// Returns the new physical address (refcount=1), or 0 on OOM.
pub unsafe fn frame_cow_copy(old_phys: u64) -> u64 {
    let new_phys = pmm::mm_alloc();
    if new_phys == 0 { return 0; }

    // Copy page contents
    core::ptr::copy_nonoverlapping(
        old_phys as *const u8,
        new_phys as *mut u8,
        PAGE_SIZE,
    );

    // New frame is exclusively ours
    frame_init(new_phys);

    // Old frame has one fewer owner
    frame_dec(old_phys);

    new_phys
}

/// Fast path when we already hold exclusive ownership (refcount == 1).
/// No copy needed — return the same frame unchanged.
#[inline]
pub unsafe fn frame_cow_inplace(phys: u64) -> u64 {
    // Already ours — nothing to do
    phys
}