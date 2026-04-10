// CosinusOS — mm/vmm.rs
// Virtual Memory Manager
//
// Manages x86-64 4-level page tables (PML4 → PDPT → PD → PT).
//
// Functions:
//   vmm_init()         — init, copies boot CR3 into a new K_P4
//   vmap()             — map a 4K page virt → phys with flags
//   vmap_huge()        — map a 2 MB huge page (PS=1 in PD)
//   vunmap()           — unmap a 4K page, free empty intermediate tables
//   vunmap_huge()      — unmap a 2 MB huge page
//   virt_to_phys()     — walk page tables to translate virt → phys
//   valid_user()       — check whether an address is accessible from ring-3
//   valid_buf()        — check a byte range as user-accessible
//   new_user_p4()      — new P4 that inherits kernel mappings
//   clone_user_p4()    — CoW clone for fork()
//   free_user_p4()     — release all user mappings and the P4 itself
//   tlb_flush_page()   — invlpg for a single page
//   tlb_flush_all()    — CR3 reload (full TLB flush)

use core::arch::asm;
use super::pmm::{
    PhysAddr, VirtAddr, PAGE_SIZE, HUGE_SIZE,
    MM_LOCK, mm_alloc_nolock, mm_free_nolock, mm_alloc, mm_free_phys,
};
use super::frame::{frame_inc, frame_dec, frame_shared, frame_cow_copy};

// ── PTE flags ─────────────────────────────────────────────────────────────────

pub const PTE_P:    u64 = 1 << 0;   // Present
pub const PTE_W:    u64 = 1 << 1;   // Writable
pub const PTE_U:    u64 = 1 << 2;   // User-accessible
pub const PTE_PWT:  u64 = 1 << 3;   // Write-through
pub const PTE_PCD:  u64 = 1 << 4;   // Cache-disable
pub const PTE_A:    u64 = 1 << 5;   // Accessed
pub const PTE_D:    u64 = 1 << 6;   // Dirty
pub const PTE_PS:   u64 = 1 << 7;   // Page Size (huge page in PD)
pub const PTE_G:    u64 = 1 << 8;   // Global
pub const PTE_COW:  u64 = 1 << 9;   // CoW-pending (software bit 9)
pub const PTE_NX:   u64 = 1 << 63;  // No-Execute
pub const PTE_ADDR: u64 = 0x000F_FFFF_FFFF_F000;

// ── Kernel P4 ─────────────────────────────────────────────────────────────────

pub static mut K_P4: PhysAddr = 0;

// ── Page table type ───────────────────────────────────────────────────────────

#[repr(C, align(4096))]
pub struct PT { pub e: [u64; 512] }

#[inline]
pub unsafe fn pt_ptr(p: PhysAddr) -> *mut PT { p as *mut PT }

// ── PTE helpers ───────────────────────────────────────────────────────────────

#[inline] pub fn pte_make(p: PhysAddr, f: u64) -> u64 { (p & PTE_ADDR) | f | PTE_P }
#[inline] pub fn pte_present(e: u64)  -> bool     { e & PTE_P   != 0 }
#[inline] pub fn pte_user(e: u64)     -> bool     { e & PTE_U   != 0 }
#[inline] pub fn pte_writable(e: u64) -> bool     { e & PTE_W   != 0 }
#[inline] pub fn pte_huge(e: u64)     -> bool     { e & PTE_PS  != 0 }
#[inline] pub fn pte_cow(e: u64)      -> bool     { e & PTE_COW != 0 }
#[inline] pub fn pte_addr(e: u64)     -> PhysAddr { e & PTE_ADDR }

// ── TLB ───────────────────────────────────────────────────────────────────────

#[inline]
pub unsafe fn tlb_flush_page(virt: VirtAddr) {
    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
}

#[inline]
pub unsafe fn tlb_flush_all() {
    let cr3: u64;
    asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    asm!("mov cr3, {}", in(reg) cr3, options(nostack));
}

// ── Zero-page allocation ──────────────────────────────────────────────────────

/// Allocate and zero one page; caller must hold MM_LOCK.
unsafe fn zpg() -> PhysAddr {
    let p = mm_alloc_nolock();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

/// Allocate and zero one page; acquires MM_LOCK internally.
pub unsafe fn zpg_locked() -> PhysAddr {
    let p = mm_alloc();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

// ── Internal: get-or-create a page table entry ───────────────────────────────

unsafe fn goc(tab: PhysAddr, idx: usize, flags: u64) -> PhysAddr {
    let t = &mut *pt_ptr(tab);

    if !pte_present(t.e[idx]) {
        // Entry absent — allocate a fresh page table
        let child = zpg();
        t.e[idx] = pte_make(child, flags);
        return child;
    }

    // Entry exists — merge in any missing flags (e.g. PTE_U for user walk)
    t.e[idx] |= flags & (PTE_W | PTE_U);

    // Split a huge page into 4K entries if a finer mapping is needed
    if pte_huge(t.e[idx]) {
        let huge_phys = t.e[idx] & 0x000F_FFFF_FFE0_0000;
        let child = zpg();
        let p1 = &mut *pt_ptr(child);
        for j in 0..512usize {
            let phys = huge_phys + j as u64 * PAGE_SIZE as u64;
            p1.e[j] = pte_make(phys, PTE_W);
        }
        t.e[idx] = pte_make(child, flags);
        tlb_flush_all();
    }

    pte_addr(t.e[idx])
}

// ── VMM initialisation ────────────────────────────────────────────────────────

/// Copy the bootloader CR3 into a new kernel P4 and activate it.
pub unsafe fn vmm_init(boot_cr3: PhysAddr) {
    let new_p4 = zpg_locked();
    let boot = &*pt_ptr(boot_cr3);
    let new  = &mut *pt_ptr(new_p4);
    for i in 0..512 { new.e[i] = boot.e[i]; }
    asm!("mov cr3, {}", in(reg) new_p4, options(nostack));
    K_P4 = new_p4;
}

// ── vmap — map a 4K page ──────────────────────────────────────────────────────

/// Map virtual address `v` to physical address `p` with flags `f` in table `p4`.
/// Returns 0 on success, -1 on invalid arguments.
pub unsafe fn vmap(p4: PhysAddr, v: VirtAddr, p: PhysAddr, f: u64) -> i32 {
    if v & 0xFFF != 0 || p & 0xFFF != 0 || p4 == 0 { return -1; }

    // Intermediate tables need PTE_U all the way down for user mappings
    let inter = if f & PTE_U != 0 { PTE_W | PTE_U } else { PTE_W };

    MM_LOCK.lock();
    let p3 = goc(p4, ((v >> 39) & 0x1FF) as usize, inter);
    let p2 = goc(p3, ((v >> 30) & 0x1FF) as usize, inter);
    let p1 = goc(p2, ((v >> 21) & 0x1FF) as usize, inter);
    (*pt_ptr(p1)).e[((v >> 12) & 0x1FF) as usize] = pte_make(p, f);
    tlb_flush_page(v);
    MM_LOCK.unlock();
    0
}

/// Map a 2 MB huge page. Both `v` and `p` must be 2 MB-aligned.
pub unsafe fn vmap_huge(p4: PhysAddr, v: VirtAddr, p: PhysAddr, f: u64) -> i32 {
    if v % HUGE_SIZE as u64 != 0 || p % HUGE_SIZE as u64 != 0 || p4 == 0 { return -1; }

    let inter = if f & PTE_U != 0 { PTE_W | PTE_U } else { PTE_W };

    MM_LOCK.lock();
    let p3 = goc(p4, ((v >> 39) & 0x1FF) as usize, inter);
    let p2 = goc(p3, ((v >> 30) & 0x1FF) as usize, inter);
    // Write directly into the PD with PS=1
    let pd_idx = ((v >> 21) & 0x1FF) as usize;
    (*pt_ptr(p2)).e[pd_idx] = pte_make(p, f | PTE_PS);
    tlb_flush_page(v);
    MM_LOCK.unlock();
    0
}

// ── vunmap — unmap a page ─────────────────────────────────────────────────────

unsafe fn pt_empty(p: PhysAddr) -> bool {
    (*pt_ptr(p)).e.iter().all(|&e| e == 0)
}

/// Unmap a 4K page. If the containing PT becomes empty, free it
/// (and walk up freeing empty PDPT/PD entries for user-space only).
pub unsafe fn vunmap(p4: PhysAddr, v: VirtAddr) {
    if p4 == 0 { return; }
    MM_LOCK.lock();

    let p4i = ((v >> 39) & 0x1FF) as usize;
    let p3i = ((v >> 30) & 0x1FF) as usize;
    let p2i = ((v >> 21) & 0x1FF) as usize;
    let p1i = ((v >> 12) & 0x1FF) as usize;

    let t4 = &mut *pt_ptr(p4);
    if !pte_present(t4.e[p4i]) { MM_LOCK.unlock(); return; }
    let p3p = pte_addr(t4.e[p4i]);

    let t3 = &mut *pt_ptr(p3p);
    if !pte_present(t3.e[p3i]) { MM_LOCK.unlock(); return; }
    let p2p = pte_addr(t3.e[p3i]);

    let t2 = &mut *pt_ptr(p2p);
    if !pte_present(t2.e[p2i]) { MM_LOCK.unlock(); return; }
    let p1p = pte_addr(t2.e[p2i]);

    let e = (*pt_ptr(p1p)).e[p1i];
    if pte_present(e) {
        // CoW: decrement refcount; PMM frees only when it reaches 0
        frame_dec(pte_addr(e));
        (*pt_ptr(p1p)).e[p1i] = 0;
        tlb_flush_page(v);
    }

    // Reclaim empty intermediate tables (user-space PML4 entries only)
    if pt_empty(p1p) {
        mm_free_nolock(p1p); t2.e[p2i] = 0;
        if pt_empty(p2p) {
            mm_free_nolock(p2p); t3.e[p3i] = 0;
            if pt_empty(p3p) && p4i < 256 {
                mm_free_nolock(p3p); t4.e[p4i] = 0;
            }
        }
    }
    MM_LOCK.unlock();
}

/// Unmap a 2 MB huge page and release all 512 of its frames.
pub unsafe fn vunmap_huge(p4: PhysAddr, v: VirtAddr) {
    if p4 == 0 || v % HUGE_SIZE as u64 != 0 { return; }
    MM_LOCK.lock();

    let p4i = ((v >> 39) & 0x1FF) as usize;
    let p3i = ((v >> 30) & 0x1FF) as usize;
    let p2i = ((v >> 21) & 0x1FF) as usize;

    let t4 = &mut *pt_ptr(p4);
    if !pte_present(t4.e[p4i]) { MM_LOCK.unlock(); return; }
    let p3p = pte_addr(t4.e[p4i]);

    let t3 = &mut *pt_ptr(p3p);
    if !pte_present(t3.e[p3i]) { MM_LOCK.unlock(); return; }
    let p2p = pte_addr(t3.e[p3i]);

    let t2 = &mut *pt_ptr(p2p);
    if pte_present(t2.e[p2i]) && pte_huge(t2.e[p2i]) {
        let phys = pte_addr(t2.e[p2i]);
        for j in 0..512usize {
            frame_dec(phys + j as u64 * PAGE_SIZE as u64);
        }
        t2.e[p2i] = 0;
        tlb_flush_page(v);
    }
    MM_LOCK.unlock();
}

// ── virt_to_phys ──────────────────────────────────────────────────────────────

pub unsafe fn virt_to_phys(p4: PhysAddr, v: VirtAddr) -> Option<PhysAddr> {
    if p4 == 0 { return None; }

    macro_rules! walk {
        ($tab:expr, $idx:expr) => {{
            let e = (*pt_ptr($tab)).e[$idx];
            if !pte_present(e) { return None; }
            // Handle 2 MB huge pages in PD
            if pte_huge(e) {
                let off = v & (HUGE_SIZE as u64 - 1);
                return Some((e & 0x000F_FFFF_FFE0_0000) | off);
            }
            pte_addr(e)
        }};
    }

    let p3 = walk!(p4, ((v >> 39) & 0x1FF) as usize);
    let p2 = walk!(p3, ((v >> 30) & 0x1FF) as usize);
    let p1 = walk!(p2, ((v >> 21) & 0x1FF) as usize);
    let e  = (*pt_ptr(p1)).e[((v >> 12) & 0x1FF) as usize];
    if !pte_present(e) { return None; }
    Some(pte_addr(e) | (v & 0xFFF))
}

// ── User buffer validation ────────────────────────────────────────────────────

pub unsafe fn valid_user(p4: PhysAddr, v: VirtAddr) -> bool {
    if p4 == 0 { return false; }

    macro_rules! chk {
        ($p:expr, $i:expr) => {{
            let e = (*pt_ptr($p)).e[$i];
            if !pte_present(e) || !pte_user(e) { return false; }
            if pte_huge(e) { return true; } // huge page — accessible
            pte_addr(e)
        }};
    }

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

// ── New P4 for a user process ─────────────────────────────────────────────────

/// Allocate a new P4 that inherits all kernel mappings.
/// The kernel occupies PML4[0..255] (identity-mapped lower half).
/// User pages are added on top via vmap() with PTE_U.
pub unsafe fn new_user_p4() -> PhysAddr {
    let n   = zpg_locked();
    let src = &*pt_ptr(K_P4);
    let dst = &mut *pt_ptr(n);
    for i in 0..512 { dst.e[i] = src.e[i]; }
    n
}

/// CoW clone of a P4 for fork().
/// Kernel half (PML4[0..255]) is copied as-is (shared, unmodified).
/// User half (PML4[256..511]) is cloned with CoW: every user leaf PTE
/// is marked read-only + PTE_COW in both the parent and the child.
pub unsafe fn clone_user_p4(src_p4: PhysAddr) -> PhysAddr {
    let dst_p4 = zpg_locked();
    MM_LOCK.lock();

    let src4 = &mut *pt_ptr(src_p4);
    let dst4 = &mut *pt_ptr(dst_p4);

    for i4 in 0..512 {
        if !pte_present(src4.e[i4]) { continue; }

        // Kernel half — copy directly (shared, do not modify)
        if i4 < 256 {
            dst4.e[i4] = src4.e[i4];
            continue;
        }

        // User half — clone with CoW
        let p3p = pte_addr(src4.e[i4]);
        let new_p3 = zpg();
        dst4.e[i4] = pte_make(new_p3, PTE_W | PTE_U);
        src4.e[i4] = pte_make(p3p,    PTE_W | PTE_U);

        let src3 = &mut *pt_ptr(p3p);
        let dst3 = &mut *pt_ptr(new_p3);

        for i3 in 0..512 {
            if !pte_present(src3.e[i3]) { continue; }
            let p2p = pte_addr(src3.e[i3]);
            let new_p2 = zpg();
            dst3.e[i3] = pte_make(new_p2, PTE_W | PTE_U);
            src3.e[i3] = pte_make(p2p,    PTE_W | PTE_U);

            let src2 = &mut *pt_ptr(p2p);
            let dst2 = &mut *pt_ptr(new_p2);

            for i2 in 0..512 {
                if !pte_present(src2.e[i2]) { continue; }

                // Huge page — share and increment refcount for all 512 frames
                if pte_huge(src2.e[i2]) {
                    let phys = pte_addr(src2.e[i2]);
                    for j in 0..512usize {
                        frame_inc(phys + j as u64 * PAGE_SIZE as u64);
                    }
                    // Mark CoW read-only in both parent and child
                    let f = (src2.e[i2] & !PTE_W) | PTE_COW;
                    src2.e[i2] = f;
                    dst2.e[i2] = f;
                    continue;
                }

                let p1p = pte_addr(src2.e[i2]);
                let new_p1 = zpg();
                dst2.e[i2] = pte_make(new_p1, PTE_W | PTE_U);
                src2.e[i2] = pte_make(p1p,    PTE_W | PTE_U);

                let src1 = &mut *pt_ptr(p1p);
                let dst1 = &mut *pt_ptr(new_p1);

                for i1 in 0..512 {
                    let e = src1.e[i1];
                    if !pte_present(e) { continue; }
                    // Kernel pages: copy the PTE unchanged
                    if !pte_user(e) { dst1.e[i1] = e; continue; }

                    let phys = pte_addr(e);
                    frame_inc(phys);

                    // Mark CoW: strip PTE_W, set PTE_COW in both copies
                    let cow_e = (e & !PTE_W) | PTE_COW;
                    src1.e[i1] = cow_e;
                    dst1.e[i1] = cow_e;
                }
            }
        }
    }

    // Flush TLB — src_p4 now has read-only CoW entries
    tlb_flush_all();

    MM_LOCK.unlock();
    dst_p4
}

/// Free all user mappings and the P4 itself.
/// The kernel half (PML4[0..255]) is left untouched.
pub unsafe fn free_user_p4(p4: PhysAddr) {
    if p4 == 0 || p4 == K_P4 { return; }
    MM_LOCK.lock();

    let t4 = &mut *pt_ptr(p4);
    for i4 in 256..512 {
        if !pte_present(t4.e[i4]) { continue; }
        let p3p = pte_addr(t4.e[i4]);
        let t3 = &*pt_ptr(p3p);

        for i3 in 0..512 {
            if !pte_present(t3.e[i3]) { continue; }
            let p2p = pte_addr(t3.e[i3]);
            let t2 = &*pt_ptr(p2p);

            for i2 in 0..512 {
                if !pte_present(t2.e[i2]) { continue; }
                if pte_huge(t2.e[i2]) {
                    let phys = pte_addr(t2.e[i2]);
                    for j in 0..512usize {
                        frame_dec(phys + j as u64 * PAGE_SIZE as u64);
                    }
                    continue;
                }
                let p1p = pte_addr(t2.e[i2]);
                let t1 = &*pt_ptr(p1p);
                for i1 in 0..512 {
                    let e = t1.e[i1];
                    if pte_present(e) && pte_user(e) {
                        frame_dec(pte_addr(e));
                    }
                }
                mm_free_nolock(p1p);
            }
            mm_free_nolock(p2p);
        }
        mm_free_nolock(p3p);
    }

    mm_free_nolock(p4);
    MM_LOCK.unlock();
}

// ── CoW page fault handler ────────────────────────────────────────────────────

/// Handle a CoW page fault for `fault_addr` in address space `p4`.
/// Call from the #PF handler when err & 0x3 == 0x3 (user write to present page).
/// Returns true if the fault was resolved (retry the instruction),
/// false if it is a genuine access violation.
pub unsafe fn handle_cow_fault(p4: PhysAddr, fault_addr: VirtAddr) -> bool {
    let v = fault_addr & !(PAGE_SIZE as u64 - 1);

    MM_LOCK.lock();

    // Walk down to the leaf PTE
    let t4 = &mut *pt_ptr(p4);
    let i4 = ((v >> 39) & 0x1FF) as usize;
    if !pte_present(t4.e[i4]) { MM_LOCK.unlock(); return false; }
    let p3p = pte_addr(t4.e[i4]);

    let t3 = &mut *pt_ptr(p3p);
    let i3 = ((v >> 30) & 0x1FF) as usize;
    if !pte_present(t3.e[i3]) { MM_LOCK.unlock(); return false; }
    let p2p = pte_addr(t3.e[i3]);

    let t2 = &mut *pt_ptr(p2p);
    let i2 = ((v >> 21) & 0x1FF) as usize;
    if !pte_present(t2.e[i2]) { MM_LOCK.unlock(); return false; }

    // Huge page CoW
    if pte_huge(t2.e[i2]) {
        if !pte_cow(t2.e[i2]) { MM_LOCK.unlock(); return false; }
        let old_phys = pte_addr(t2.e[i2]);
        if !frame_shared(old_phys) {
            // Sole owner — restore PTE_W, clear PTE_COW
            t2.e[i2] = (t2.e[i2] & !PTE_COW) | PTE_W;
        } else {
            // Copy the full 2 MB block
            let new_phys = super::pmm::mm_alloc_huge();
            if new_phys == 0 { MM_LOCK.unlock(); return false; }
            core::ptr::copy_nonoverlapping(
                old_phys as *const u8, new_phys as *mut u8, HUGE_SIZE,
            );
            for j in 0..512usize {
                frame_dec(old_phys + j as u64 * PAGE_SIZE as u64);
                super::frame::frame_init(new_phys + j as u64 * PAGE_SIZE as u64);
            }
            t2.e[i2] = pte_make(
                new_phys,
                (t2.e[i2] & !PTE_COW & !PTE_ADDR) | PTE_W | PTE_PS,
            );
        }
        tlb_flush_page(v);
        MM_LOCK.unlock();
        return true;
    }

    let p1p = pte_addr(t2.e[i2]);
    let t1  = &mut *pt_ptr(p1p);
    let i1  = ((v >> 12) & 0x1FF) as usize;
    let e   = t1.e[i1];

    if !pte_present(e) || !pte_cow(e) { MM_LOCK.unlock(); return false; }

    let old_phys = pte_addr(e);

    if !frame_shared(old_phys) {
        // Sole owner — restore write access, clear CoW bit
        t1.e[i1] = (e & !PTE_COW) | PTE_W;
    } else {
        // Physical copy required — drop MM_LOCK around the allocation
        MM_LOCK.unlock();
        let new_phys = frame_cow_copy(old_phys);
        MM_LOCK.lock();
        if new_phys == 0 { MM_LOCK.unlock(); return false; }
        t1.e[i1] = pte_make(new_phys, (e & !PTE_COW & !PTE_ADDR) | PTE_W);
    }

    tlb_flush_page(v);
    MM_LOCK.unlock();
    true
}