// CosinusOS — mm.rs

use core::arch::asm;
use crate::sync::Spinlock;
use crate::debug::{serial_print, putc_raw};

pub type PhysAddr = u64;
pub type VirtAddr = u64;

pub const PAGE_SIZE: usize = 0x1000;
const MAX_FRAMES:    usize = 0x10000;

// ── PTE flags ────────────────────────────────────────────────────────────────
pub const PTE_P:    u64 = 1 << 0;
pub const PTE_W:    u64 = 1 << 1;
pub const PTE_U:    u64 = 1 << 2;
pub const PTE_ADDR: u64 = 0x000F_FFFF_FFFF_F000;

// ── PMM state ────────────────────────────────────────────────────────────────
pub static MM_LOCK: Spinlock = Spinlock::new();
static mut FRAME_BM: [u64; MAX_FRAMES / 64] = [0u64; MAX_FRAMES / 64];
static mut MEM_BASE: PhysAddr = 0;
static mut MEM_SIZE: usize    = 0;
static mut HINT:     usize    = 0;

// ── VMM state ────────────────────────────────────────────────────────────────
pub static mut K_P4: PhysAddr = 0;

// ── PMM internals ────────────────────────────────────────────────────────────
unsafe fn fi(p: PhysAddr) -> usize { ((p - MEM_BASE) / PAGE_SIZE as u64) as usize }
unsafe fn fp(i: usize) -> PhysAddr { MEM_BASE + i as u64 * PAGE_SIZE as u64 }
unsafe fn is_free(i: usize) -> bool { (FRAME_BM[i / 64] & (1u64 << (i % 64))) == 0 }
unsafe fn mark_used(i: usize) { FRAME_BM[i / 64] |=  1u64 << (i % 64); }
unsafe fn mark_free(i: usize) {
    FRAME_BM[i / 64] &= !(1u64 << (i % 64));
    if i / 64 < HINT { HINT = i / 64; }
}

pub unsafe fn mm_init(base: PhysAddr, size: usize) {
    MEM_BASE = base;
    MEM_SIZE = size;
    core::ptr::write_bytes(&raw mut FRAME_BM as *mut u8, 0, core::mem::size_of_val(&raw const FRAME_BM));
    mark_used(0);
    HINT = 0;
    serial_print("[PMM] ");
    pnum_serial(size / 1024 / 1024);
    serial_print(" MiB dostepne\n");
}

pub unsafe fn mm_alloc_nolock() -> PhysAddr {
    for pass in 0..2 {
        let (s, e) = if pass == 0 { (HINT, FRAME_BM.len()) } else { (0, HINT) };
        for w in s..e {
            if FRAME_BM[w] == !0u64 { continue; }
            for bit in 0..64 {
                let idx = w * 64 + bit;
                if idx >= MAX_FRAMES { continue; }
                if is_free(idx) {
                    mark_used(idx);
                    HINT = w;
                    return fp(idx);
                }
            }
        }
    }
    crate::panic_no_dyn("OOM");
}

pub unsafe fn mm_alloc() -> PhysAddr {
    MM_LOCK.lock();
    let p = mm_alloc_nolock();
    MM_LOCK.unlock();
    p
}

pub unsafe fn mm_free_nolock(p: PhysAddr) {
    if p < MEM_BASE { return; }
    let i = fi(p);
    if i >= MAX_FRAMES { return; }
    mark_free(i);
}

pub unsafe fn mm_free_phys(p: PhysAddr) {
    MM_LOCK.lock();
    mm_free_nolock(p);
    MM_LOCK.unlock();
}

pub unsafe fn mm_free_kb()  -> usize { mm_cnt(true)  * PAGE_SIZE / 1024 }
pub unsafe fn mm_used_kb()  -> usize { mm_cnt(false) * PAGE_SIZE / 1024 }
pub unsafe fn mm_total_kb() -> usize { (MEM_SIZE / PAGE_SIZE) * PAGE_SIZE / 1024 }

unsafe fn mm_cnt(free: bool) -> usize {
    let t = MEM_SIZE / PAGE_SIZE;
    let mut n = 0;
    for i in 0..t { if is_free(i) == free { n += 1; } }
    n
}

// ── VMM internals ─────────────────────────────────────────────────────────────
pub fn  pte_make(p: PhysAddr, f: u64) -> u64 { (p & PTE_ADDR) | f | PTE_P }
pub fn  pte_present(e: u64) -> bool  { e & PTE_P != 0 }
pub fn  pte_user(e: u64)    -> bool  { e & PTE_U != 0 }
pub fn  pte_addr(e: u64)    -> PhysAddr { e & PTE_ADDR }

#[repr(C, align(4096))]
pub struct PT { pub e: [u64; 512] }

pub unsafe fn pt_ptr(p: PhysAddr) -> *mut PT { p as *mut PT }

pub unsafe fn zpg() -> PhysAddr {
    let p = mm_alloc_nolock();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

pub unsafe fn zpg_locked() -> PhysAddr {
    let p = mm_alloc();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

// Get-or-create a page table entry at `idx` in table at `tab`.
// `flags` are ORed into the entry — existing entries get missing flags added.
// Huge pages (PS bit) are split into 4K pages on demand.
unsafe fn goc(tab: PhysAddr, idx: usize, flags: u64) -> PhysAddr {
    let t = &mut *pt_ptr(tab);

    if !pte_present(t.e[idx]) {
        // Entry doesn't exist — allocate a fresh page table
        let child = zpg();
        t.e[idx] = pte_make(child, flags);
        return child;
    }

    // Entry exists — ensure required flags are set (e.g. PTE_U for user walk)
    t.e[idx] |= flags & (PTE_W | PTE_U);

    // Split huge page (PS=1) into 4K entries so vmap can address individual pages
    if t.e[idx] & (1 << 7) != 0 {
        let huge_phys = t.e[idx] & 0x000F_FFFF_FFE0_0000;
        let child = zpg();
        let p1 = &mut *pt_ptr(child);
        for j in 0..512usize {
            let phys = huge_phys + j as u64 * PAGE_SIZE as u64;
            // Keep W flag from the huge page; U will be added by caller if needed
            p1.e[j] = pte_make(phys, PTE_W);
        }
        t.e[idx] = pte_make(child, flags);
        // Flush TLB after replacing the huge page entry
        let cr3: u64;
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    }

    pte_addr(t.e[idx])
}

unsafe fn pt_empty(p: PhysAddr) -> bool {
    (*pt_ptr(p)).e.iter().all(|&e| e == 0)
}

pub unsafe fn vmm_init(boot_cr3: PhysAddr) {
    let new_p4 = zpg_locked();
    let boot = &*pt_ptr(boot_cr3);
    let new  = &mut *pt_ptr(new_p4);
    for i in 0..512 { new.e[i] = boot.e[i]; }
    asm!("mov cr3, {}", in(reg) new_p4, options(nostack));
    K_P4 = new_p4;
}

// Map a single 4K page: virt `v` → phys `p` with flags `f` in page table `p4`.
// Intermediate tables (P3/P2/P1) always get PTE_W|PTE_U so user mappings can
// be reached regardless of which flags the leaf entry carries.
pub unsafe fn vmap(p4: PhysAddr, v: VirtAddr, p: PhysAddr, f: u64) -> i32 {
    if v & 0xFFF != 0 || p & 0xFFF != 0 || p4 == 0 { return -1; }

    // Determine intermediate flags: user mappings need PTE_U all the way down.
    // Kernel mappings (no PTE_U in f) only need PTE_W on intermediate levels.
    let inter_flags = if f & PTE_U != 0 { PTE_W | PTE_U } else { PTE_W };

    MM_LOCK.lock();
    let p3 = goc(p4, ((v >> 39) & 0x1FF) as usize, inter_flags);
    let p2 = goc(p3, ((v >> 30) & 0x1FF) as usize, inter_flags);
    let p1 = goc(p2, ((v >> 21) & 0x1FF) as usize, inter_flags);
    (*pt_ptr(p1)).e[((v >> 12) & 0x1FF) as usize] = pte_make(p, f);
    asm!("invlpg [{}]", in(reg) v, options(nostack, preserves_flags));
    MM_LOCK.unlock();
    0
}

pub unsafe fn vunmap(p4: PhysAddr, v: VirtAddr) {
    if p4 == 0 { return; }
    MM_LOCK.lock();
    let p4i = ((v >> 39) & 0x1FF) as usize;
    let p3i = ((v >> 30) & 0x1FF) as usize;
    let p2i = ((v >> 21) & 0x1FF) as usize;
    let p1i = ((v >> 12) & 0x1FF) as usize;
    let t4 = &mut *pt_ptr(p4);
    if !pte_present(t4.e[p4i]) { MM_LOCK.unlock(); return; }
    let p3p = pte_addr(t4.e[p4i]); let t3 = &mut *pt_ptr(p3p);
    if !pte_present(t3.e[p3i]) { MM_LOCK.unlock(); return; }
    let p2p = pte_addr(t3.e[p3i]); let t2 = &mut *pt_ptr(p2p);
    if !pte_present(t2.e[p2i]) { MM_LOCK.unlock(); return; }
    let p1p = pte_addr(t2.e[p2i]);
    (*pt_ptr(p1p)).e[p1i] = 0;
    asm!("invlpg [{}]", in(reg) v, options(nostack, preserves_flags));
    if pt_empty(p1p) { mm_free_nolock(p1p); t2.e[p2i] = 0;
        if pt_empty(p2p) { mm_free_nolock(p2p); t3.e[p3i] = 0;
            if pt_empty(p3p) && p4i < 256 { mm_free_nolock(p3p); t4.e[p4i] = 0; }}}
    MM_LOCK.unlock();
}

pub unsafe fn virt_to_phys(p4: PhysAddr, v: VirtAddr) -> Option<PhysAddr> {
    if p4 == 0 { return None; }
    macro_rules! walk { ($tab:expr, $idx:expr) => {{
        let e = (*pt_ptr($tab)).e[$idx];
        if !pte_present(e) { return None; }
        pte_addr(e)
    }};}
    let p3 = walk!(p4, ((v >> 39) & 0x1FF) as usize);
    let p2 = walk!(p3, ((v >> 30) & 0x1FF) as usize);
    let p1 = walk!(p2, ((v >> 21) & 0x1FF) as usize);
    let e  = (*pt_ptr(p1)).e[((v >> 12) & 0x1FF) as usize];
    if !pte_present(e) { return None; }
    Some(pte_addr(e) | (v & 0xFFF))
}

pub unsafe fn valid_user(p4: PhysAddr, v: VirtAddr) -> bool {
    if p4 == 0 { return false; }
    macro_rules! chk { ($p:expr, $i:expr) => {{
        let e = (*pt_ptr($p)).e[$i];
        if !pte_present(e) || !pte_user(e) { return false; }
        pte_addr(e)
    }};}
    let p3 = chk!(p4, ((v >> 39) & 0x1FF) as usize);
    let p2 = chk!(p3, ((v >> 30) & 0x1FF) as usize);
    let p1 = chk!(p2, ((v >> 21) & 0x1FF) as usize);
    let e  = (*pt_ptr(p1)).e[((v >> 12) & 0x1FF) as usize];
    pte_present(e) && pte_user(e)
}

pub unsafe fn valid_buf(p4: PhysAddr, ptr: VirtAddr, len: usize) -> bool {
    if len == 0 { return true; }
    let mut pg = ptr & !(PAGE_SIZE as u64 - 1);
    while pg < ptr + len as u64 {
        if !valid_user(p4, pg) { return false; }
        pg += PAGE_SIZE as u64;
    }
    true
}

// Allocate a new P4 that shares ALL kernel mappings (full copy of K_P4).
// Kernel lives in identity-mapped lower half — clearing p4[0..256] would
// destroy the kernel's own code/stack/IDT mappings and cause a triple fault
// on the first interrupt after CR3 switch. Userspace pages are added on top
// via vmap and sit at different addresses (0x400000, 0x7C00000, etc.).
pub unsafe fn new_user_p4() -> PhysAddr {
    let n   = zpg_locked();
    let src = &*pt_ptr(K_P4);
    let dst = &mut *pt_ptr(n);
    for i in 0..512 { dst.e[i] = src.e[i]; }
    n
}

unsafe fn pnum_serial(mut v: usize) {
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

pub const KERNEL_STACK_SIZE: usize = 0x8000; // 32 KB
pub const USER_STACK_SIZE:   usize = 0x4000; // 16 KB