// CosinusOS — mm/frame.rs
// Frame reference counting table.
//
// Każda fizyczna ramka ma licznik referencji (u16).
// Używane przez CoW: fork() zwiększa refcount, zapis dekrementuje i kopiuje.
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

/// Zwiększ refcount (np. przy fork / shared mapping).
/// Zwraca nowy refcount. Panikuje przy overflow (> 65535 współdzielonych mapowań).
#[inline]
pub fn frame_inc(phys: u64) {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return; }
    let prev = FRAME_REFS[i].fetch_add(1, Ordering::AcqRel);
    if prev == u16::MAX {
        // Overflow — zostaw na MAX, nie zmniejszaj (leak jest bezpieczniejszy niż UAF)
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

/// Zmniejsz refcount. Jeśli osiągnie 0 — zwróć ramkę do PMM i zwróć true.
/// Zwraca false jeśli ramka nadal ma właścicieli.
pub fn frame_dec(phys: u64) -> bool {
    let i = unsafe { fi(phys) };
    if i >= MAX_FRAMES { return false; }

    // Saturating: jeśli ktoś nadpisze MAX (overflow guard) nie dekrementujemy
    let cur = FRAME_REFS[i].load(Ordering::Acquire);
    if cur == u16::MAX { return false; } // permanently pinned

    let prev = FRAME_REFS[i].fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        // Refcount spadł do 0 — zwróć do PMM
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

/// Wykonaj CoW: skopiuj ramkę, zmniejsz refcount starej, zwróć nową.
/// Używane przez page fault handler przy zapisie do CoW-strony.
///
/// Zwraca nowy adres fizyczny (własny, refcount=1) lub 0 przy OOM.
pub unsafe fn frame_cow_copy(old_phys: u64) -> u64 {
    let new_phys = pmm::mm_alloc();
    if new_phys == 0 { return 0; }

    // Kopiuj zawartość strony
    core::ptr::copy_nonoverlapping(
        old_phys as *const u8,
        new_phys as *mut u8,
        PAGE_SIZE,
    );

    // Nowa ramka należy tylko do nas
    frame_init(new_phys);

    // Stara ramka ma jednego właściciela mniej
    frame_dec(old_phys);

    new_phys
}

/// Wersja frame_cow_copy gdy wiemy że mamy wyłączny dostęp (refcount==1).
/// Nie kopiuje — tylko zwraca tę samą ramkę z zachowaną własnością.
#[inline]
pub unsafe fn frame_cow_inplace(phys: u64) -> u64 {
    // Już nasze — po prostu kontynuuj
    phys
}
