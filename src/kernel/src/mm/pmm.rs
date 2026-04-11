// CosinusOS — mm/pmm.rs
// Physical Memory Manager
//
// Implementacja:
//   • Bitmap pierwszego poziomu (jeden bit = jedna ramka 4 KB)
//   • Hint-based fast alloc: HINT points at the last-used bitmap word
//   • Huge page alloc: mm_alloc_huge() — 512 kolejnych ramek (2 MB aligned)
//   • Stats: free / used / total in KB
//   • NUMA stub: single region (extendable to multiple banks)
//
// Publiczne typy:
//   PhysAddr = u64   — adres fizyczny
//   VirtAddr = u64   — virtual address (used by VMM)

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::Spinlock;
use crate::debug::{serial_print, putc_raw};

// ── Typy ─────────────────────────────────────────────────────────────────────

pub type PhysAddr = u64;
pub type VirtAddr = u64;

// ── Constants ────────────────────────────────────────────────────────────────

pub const PAGE_SIZE:  usize = 0x1000;        // 4 KB
pub const HUGE_SIZE:  usize = 0x20_0000;     // 2 MB = 512 stron
pub const MAX_FRAMES: usize = 0x10000;       // 64K frames = 256 MB addressable

// ── Stan PMM ──────────────────────────────────────────────────────────────────

pub static MM_LOCK: Spinlock = Spinlock::new();

// Exported pub(super) so frame.rs can access fi / fp / MEM_BASE
pub(super) static mut MEM_BASE: PhysAddr = 0;
static mut MEM_SIZE:  usize    = 0;
static mut FRAME_BM:  [u64; MAX_FRAMES / 64] = [0u64; MAX_FRAMES / 64];
static mut HINT:      usize    = 0;

// Statystyki atomiczne (bez locka dla szybkiego odczytu)
static FREE_FRAMES: AtomicUsize = AtomicUsize::new(0);

// ── Helpers ───────────────────────────────────────────────────────────────────

#[inline]
pub(super) unsafe fn fi(p: PhysAddr) -> usize {
    ((p - MEM_BASE) / PAGE_SIZE as u64) as usize
}

#[inline]
pub(super) unsafe fn fp(i: usize) -> PhysAddr {
    MEM_BASE + i as u64 * PAGE_SIZE as u64
}

#[inline]
unsafe fn is_free(i: usize) -> bool {
    (FRAME_BM[i / 64] & (1u64 << (i % 64))) == 0
}

#[inline]
unsafe fn mark_used(i: usize) {
    if is_free(i) {
        FRAME_BM[i / 64] |= 1u64 << (i % 64);
        FREE_FRAMES.fetch_sub(1, Ordering::Relaxed);
    }
}

#[inline]
unsafe fn mark_free(i: usize) {
    if !is_free(i) {
        FRAME_BM[i / 64] &= !(1u64 << (i % 64));
        FREE_FRAMES.fetch_add(1, Ordering::Relaxed);
        if i / 64 < HINT { HINT = i / 64; }
    }
}

// ── Inicjalizacja ─────────────────────────────────────────────────────────────

/// Initialise the PMM. Must be called exactly once during kernel startup.
/// `base` must be PAGE_SIZE-aligned; `size` is the number of available bytes.
pub unsafe fn mm_init(base: PhysAddr, size: usize) {
    MEM_BASE = base;
    MEM_SIZE = size;

    core::ptr::write_bytes(
        core::ptr::addr_of_mut!(FRAME_BM) as *mut u8,
        0,
        core::mem::size_of_val(&FRAME_BM),
    );

    // Ramka 0 zawsze zarezerwowana (NULL protection)
    mark_used(0);

    let total = size / PAGE_SIZE;
    FREE_FRAMES.store(total.saturating_sub(1), Ordering::Relaxed);
    HINT = 0;

    // Inicjalizuj refcount dla wszystkich ramek
    for i in 1..total.min(MAX_FRAMES) {
        super::frame::frame_init(fp(i));
    }

    serial_print("[PMM] ");
    pnum_serial(size / 1024 / 1024);
    serial_print(" MiB dostepne (");
    pnum_serial(total);
    serial_print(" ramek)\n");
}

// ── Alokacja / zwolnienie ─────────────────────────────────────────────────────

/// Allocate one 4K frame without acquiring MM_LOCK (caller must hold it).
pub unsafe fn mm_alloc_nolock() -> PhysAddr {
    for pass in 0..2 {
        let (s, e) = if pass == 0 {
            (HINT, FRAME_BM.len())
        } else {
            (0, HINT)
        };

        for w in s..e {
            if FRAME_BM[w] == !0u64 { continue; }
            for bit in 0..64 {
                let idx = w * 64 + bit;
                if idx >= MAX_FRAMES { continue; }
                if idx >= MEM_SIZE / PAGE_SIZE { continue; }
                if is_free(idx) {
                    mark_used(idx);
                    HINT = w;
                    let phys = fp(idx);
                    super::frame::frame_init(phys);
                    return phys;
                }
            }
        }
    }
    crate::panic_no_dyn("[PMM] OOM — brak wolnych ramek");
}

/// Allocate one 4K frame (acquires MM_LOCK).
pub unsafe fn mm_alloc() -> PhysAddr {
    MM_LOCK.lock();
    let p = mm_alloc_nolock();
    MM_LOCK.unlock();
    p
}

/// Allocate and zero one 4K frame (acquires MM_LOCK).
pub unsafe fn mm_alloc_zeroed() -> PhysAddr {
    let p = mm_alloc();
    if p != 0 {
        core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    }
    p
}

/// Allocate a 2 MB huge page (512 contiguous, 2 MB-aligned frames).
/// Returns the physical base address, or 0 on OOM.
pub unsafe fn mm_alloc_huge() -> PhysAddr {
    MM_LOCK.lock();

    let total = MEM_SIZE / PAGE_SIZE;
    // Search for 512 contiguous free frames aligned to 512
    let mut i = 512usize; // zacznij od 512 (pomijamy pierwsze 2MB dla kernela)
    while i + 512 <= total.min(MAX_FRAMES) {
        if i % 512 != 0 { i = (i + 511) & !(511); continue; }

        // Check whether all 512 frames are free
        let mut ok = true;
        'outer: for w in 0..8usize { // 512/64 = 8 words
            let word_idx = (i / 64) + w;
            if FRAME_BM[word_idx] != 0 { ok = false; break 'outer; }
        }

        if ok {
            for j in 0..512 {
                mark_used(i + j);
                super::frame::frame_init(fp(i + j));
            }
            MM_LOCK.unlock();
            return fp(i);
        }
        i += 512;
    }
    MM_LOCK.unlock();
    0 // OOM — no contiguous block available
}

/// Free one frame without acquiring MM_LOCK (caller must hold it).
pub unsafe fn mm_free_nolock(p: PhysAddr) {
    if p < MEM_BASE { return; }
    let i = fi(p);
    if i >= MAX_FRAMES || i >= MEM_SIZE / PAGE_SIZE { return; }
    mark_free(i);
}

/// Free one frame (acquires MM_LOCK).
/// Prefer calling through frame::frame_dec() when using CoW refcounting.
/// 
pub unsafe fn mm_free_phys(p: PhysAddr) {
    MM_LOCK.lock();
    mm_free_nolock(p);
    MM_LOCK.unlock();
}

/// Zwolnij huge page (512 ramek).
pub unsafe fn mm_free_huge(p: PhysAddr) {
    if p % HUGE_SIZE as u64 != 0 { return; }
    MM_LOCK.lock();
    for j in 0..512usize {
        mm_free_nolock(p + j as u64 * PAGE_SIZE as u64);
    }
    MM_LOCK.unlock();
}

// ── Statystyki ────────────────────────────────────────────────────────────────

pub unsafe fn mm_free_kb()  -> usize { FREE_FRAMES.load(Ordering::Relaxed) * PAGE_SIZE / 1024 }
pub unsafe fn mm_used_kb()  -> usize {
    let total = MEM_SIZE / PAGE_SIZE;
    let free  = FREE_FRAMES.load(Ordering::Relaxed);
    total.saturating_sub(free) * PAGE_SIZE / 1024
}
pub unsafe fn mm_total_kb() -> usize { (MEM_SIZE / PAGE_SIZE) * PAGE_SIZE / 1024 }
pub unsafe fn mm_free_pages() -> usize { FREE_FRAMES.load(Ordering::Relaxed) }

/// Wypisz raport stanu PMM na serial.
pub unsafe fn mm_dump_stats() {
    serial_print("[PMM] free=");
    pnum_serial(mm_free_kb());
    serial_print(" KB used=");
    pnum_serial(mm_used_kb());
    serial_print(" KB total=");
    pnum_serial(mm_total_kb());
    serial_print(" KB\n");
}

// ── Helper do wypisywania liczb na serial (no-alloc) ─────────────────────────

pub(super) unsafe fn pnum_serial(mut v: usize) {
    if v == 0 { serial_print("0"); return; }
    let mut buf = [0u8; 24];
    let mut i = 23usize;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if i > 0 { i -= 1; } else { break; }
    }
    for b in &buf[i + 1..] {
        putc_raw(*b as char);
    }
}