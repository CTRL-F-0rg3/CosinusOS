// CosinusOS — mm/frame.rs
// Frame reference counting table.
//
// Every physical frame has a u16 reference counter.
// Used by CoW: fork() increments refcount, a write decrements and copies.
// When refcount reaches 0 the frame is returned to the PMM.
//
// Layout: FRAME_REFS[i] = refcount for frame index i (fi(phys) from PMM).
// Static table — MAX_FRAMES * 2 bytes = 128 KB for 64K frames.

use core::sync::atomic::{AtomicU16, Ordering};
use super::pmm::{fi, fp, MEM_BASE, MAX_FRAMES, PAGE_SIZE};
use super::pmm;

// ── Reference count table ─────────────────────────────────────────────────────

static FRAME_REFS: [AtomicU16; MAX_FRAMES] = {
    // const-init trick — AtomicU16::new(0) is const
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicU16 = AtomicU16::new(0);
    [Z; MAX_FRAMES]
};

// ── API ───────────────────────────────────────────────────────────────────────

/// Return the current refcount for a physical frame.
#[inline]
pub fn frame_ref(phys: u64) -> u16 {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return 0; }
    FRAME_REFS[i].load(Ordering::Relaxed)
}

/// Increment refcount (e.g. on fork / shared mapping).
/// On overflow (> 65535 sharers) the counter is pinned at MAX —
/// a leak is safer than a use-after-free.
#[inline]
pub fn frame_inc(phys: u64) {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return; }
    let prev = FRAME_REFS[i].fetch_add(1, Ordering::AcqRel);
    if prev == u16::MAX {
        // Pin at MAX — do not wrap around
        FRAME_REFS[i].store(u16::MAX, Ordering::Release);
    }
}

/// Set refcount to 1 (called by PMM on first allocation of a frame).
#[inline]
pub fn frame_init(phys: u64) {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return; }
    FRAME_REFS[i].store(1, Ordering::Release);
}

/// Decrement refcount. If it reaches 0, return the frame to the PMM.
/// Returns true when the frame was freed, false if other owners remain.
pub fn frame_dec(phys: u64) -> bool {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return false; }

    // Permanently-pinned frames (refcount == MAX) are never freed.
    let cur = FRAME_REFS[i].load(Ordering::Acquire);
    if cur == u16::MAX { return false; }

    let prev = FRAME_REFS[i].fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        // Refcount dropped to 0 — return frame to PMM
        unsafe { pmm::mm_free_phys(phys); }
        return true;
    }
    false
}

/// Returns true when the frame is shared between two or more mappings.
#[inline]
pub fn frame_shared(phys: u64) -> bool {
    frame_ref(phys) > 1
}

/// Perform a CoW copy: allocate a new frame, copy page contents, decrement
/// the old frame's refcount, and return the new exclusive frame.
/// Returns the new physical address, or 0 on OOM.
pub unsafe fn frame_cow_copy(old_phys: u64) -> u64 {
    let new_phys = pmm::mm_alloc();
    if new_phys == 0 { return 0; }

    core::ptr::copy_nonoverlapping(
        old_phys as *const u8,
        new_phys as *mut u8,
        PAGE_SIZE,
    );

    frame_init(new_phys); // new frame is exclusively ours
    frame_dec(old_phys);  // old frame has one fewer owner
    new_phys
}

/// Fast path when we already hold exclusive ownership (refcount == 1).
/// No copy needed — return the same frame unchanged.
#[inline]
pub unsafe fn frame_cow_inplace(phys: u64) -> u64 {
    phys
}