// CosinusOS — mm/vmm.rs
// Virtual Memory Manager
//
// Zarządza 4-poziomowymi tablicami stron x86-64 (PML4→PDPT→PD→PT).
//
// Funkcje:
//   vmm_init()         — inicjalizacja, kopiuje boot CR3 do własnego K_P4
//   vmap()             — mapuj 4K stronę virt→phys z flagami
//   vmap_huge()        — mapuj 2MB huge page (PS=1 w PD)
//   vunmap()           — odmapuj 4K stronę, zwolnij puste tablice
//   vunmap_huge()      — odmapuj 2MB huge page
//   virt_to_phys()     — translacja virt→phys przez page walk
//   valid_user()       — sprawdź czy adres jest dostępny z ring-3
//   valid_buf()        — sprawdź zakres bajtów jako user-accessible
//   new_user_p4()      — nowy P4 dziedziczący kernel mappings
//   clone_user_p4()    — CoW clone dla fork()
//   free_user_p4()     — zwolnij wszystkie user mappings i P4
//   tlb_flush_page()   — invlpg dla jednej strony
//   tlb_flush_all()    — reload CR3 (flush całego TLB)

use core::arch::asm;
use super::pmm::{
    PhysAddr, VirtAddr, PAGE_SIZE, HUGE_SIZE,
    MM_LOCK, mm_alloc_nolock, mm_free_nolock, mm_alloc, mm_free_phys,
};
use super::frame::{frame_inc, frame_dec, frame_shared, frame_cow_copy};

// ── PTE flags ────────────────────────────────────────────────────────────────

pub const PTE_P:    u64 = 1 << 0;   // Present
pub const PTE_W:    u64 = 1 << 1;   // Writable
pub const PTE_U:    u64 = 1 << 2;   // User accessible
pub const PTE_PWT:  u64 = 1 << 3;   // Write-through
pub const PTE_PCD:  u64 = 1 << 4;   // Cache-disable
pub const PTE_A:    u64 = 1 << 5;   // Accessed
pub const PTE_D:    u64 = 1 << 6;   // Dirty
pub const PTE_PS:   u64 = 1 << 7;   // Page Size (huge page w PD)
pub const PTE_G:    u64 = 1 << 8;   // Global
pub const PTE_COW:  u64 = 1 << 9;   // Bit 9: CoW-pending (software bit)
pub const PTE_NX:   u64 = 1 << 63;  // No-Execute
pub const PTE_ADDR: u64 = 0x000F_FFFF_FFFF_F000;

// ── Kernel P4 ─────────────────────────────────────────────────────────────────

pub static mut K_P4: PhysAddr = 0;

// ── Page table repr ───────────────────────────────────────────────────────────

#[repr(C, align(4096))]
pub struct PT { pub e: [u64; 512] }

#[inline]
pub unsafe fn pt_ptr(p: PhysAddr) -> *mut PT { p as *mut PT }

// ── PTE helpers ───────────────────────────────────────────────────────────────

#[inline] pub fn pte_make(p: PhysAddr, f: u64) -> u64 { (p & PTE_ADDR) | f | PTE_P }
#[inline] pub fn pte_present(e: u64)  -> bool     { e & PTE_P    != 0 }
#[inline] pub fn pte_user(e: u64)     -> bool     { e & PTE_U    != 0 }
#[inline] pub fn pte_writable(e: u64) -> bool     { e & PTE_W    != 0 }
#[inline] pub fn pte_huge(e: u64)     -> bool     { e & PTE_PS   != 0 }
#[inline] pub fn pte_cow(e: u64)      -> bool     { e & PTE_COW  != 0 }
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

// ── Alokacja zerowej strony ───────────────────────────────────────────────────

unsafe fn zpg() -> PhysAddr {
    let p = mm_alloc_nolock();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

pub unsafe fn zpg_locked() -> PhysAddr {
    let p = mm_alloc();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

// ── Wewnętrzna: get-or-create wpis tablicy stron ─────────────────────────────

unsafe fn goc(tab: PhysAddr, idx: usize, flags: u64) -> PhysAddr {
    let t = &mut *pt_ptr(tab);

    if !pte_present(t.e[idx]) {
        let child = zpg();
        t.e[idx] = pte_make(child, flags);
        return child;
    }

    // Dodaj brakujące flagi (np. PTE_U jeśli wcześniej było tylko kernel)
    t.e[idx] |= flags & (PTE_W | PTE_U);

    // Rozbij huge page na 4K jeśli potrzeba
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

// ── Inicjalizacja VMM ────────────────────────────────────────────────────────

pub unsafe fn vmm_init(boot_cr3: PhysAddr) {
    let new_p4 = zpg_locked();
    let boot = &*pt_ptr(boot_cr3);
    let new  = &mut *pt_ptr(new_p4);
    for i in 0..512 { new.e[i] = boot.e[i]; }
    asm!("mov cr3, {}", in(reg) new_p4, options(nostack));
    K_P4 = new_p4;
}

// ── vmap — mapuj 4K stronę ───────────────────────────────────────────────────

/// Mapuj virt `v` → phys `p` z flagami `f` w tablicy stron `p4`.
/// Zwraca 0 przy sukcesie, -1 przy błędzie.
pub unsafe fn vmap(p4: PhysAddr, v: VirtAddr, p: PhysAddr, f: u64) -> i32 {
    if v & 0xFFF != 0 || p & 0xFFF != 0 || p4 == 0 { return -1; }

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

/// Mapuj 2MB huge page. `v` i `p` muszą być wyrównane do 2MB.
pub unsafe fn vmap_huge(p4: PhysAddr, v: VirtAddr, p: PhysAddr, f: u64) -> i32 {
    if v % HUGE_SIZE as u64 != 0 || p % HUGE_SIZE as u64 != 0 || p4 == 0 { return -1; }

    let inter = if f & PTE_U != 0 { PTE_W | PTE_U } else { PTE_W };

    MM_LOCK.lock();
    let p3 = goc(p4, ((v >> 39) & 0x1FF) as usize, inter);
    let p2 = goc(p3, ((v >> 30) & 0x1FF) as usize, inter);
    // Wpisz bezpośrednio do PD z PS=1
    let pd_idx = ((v >> 21) & 0x1FF) as usize;
    (*pt_ptr(p2)).e[pd_idx] = pte_make(p, f | PTE_PS);
    tlb_flush_page(v);
    MM_LOCK.unlock();
    0
}

// ── vunmap — odmapuj stronę ───────────────────────────────────────────────────

unsafe fn pt_empty(p: PhysAddr) -> bool {
    (*pt_ptr(p)).e.iter().all(|&e| e == 0)
}

/// Odmapuj 4K stronę. Jeśli to ostatni wpis w tablicy — zwolnij tablicę.
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
        let phys = pte_addr(e);
        // CoW: zmniejsz refcount, zwolnij tylko gdy refcount == 0
        frame_dec(phys);
        (*pt_ptr(p1p)).e[p1i] = 0;
        tlb_flush_page(v);
    }

    // Zwolnij puste pośrednie tablice (tylko user-space, p4i < 256)
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

/// Odmapuj 2MB huge page.
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
        // Zwolnij wszystkie 512 ramek huge page
        for j in 0..512usize {
            frame_dec(phys + j as u64 * PAGE_SIZE as u64);
        }
        t2.e[p2i] = 0;
        tlb_flush_page(v);
    }
    MM_LOCK.unlock();
}

// ── Translacja virt→phys ──────────────────────────────────────────────────────

pub unsafe fn virt_to_phys(p4: PhysAddr, v: VirtAddr) -> Option<PhysAddr> {
    if p4 == 0 { return None; }

    macro_rules! walk {
        ($tab:expr, $idx:expr) => {{
            let e = (*pt_ptr($tab)).e[$idx];
            if !pte_present(e) { return None; }
            // Obsłuż huge page w PDPT (1GB) i PD (2MB)
            if pte_huge(e) {
                // Zakładamy max 2MB huge pages w PD
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

// ── Walidacja user bufferów ───────────────────────────────────────────────────

pub unsafe fn valid_user(p4: PhysAddr, v: VirtAddr) -> bool {
    if p4 == 0 { return false; }

    macro_rules! chk {
        ($p:expr, $i:expr) => {{
            let e = (*pt_ptr($p)).e[$i];
            if !pte_present(e) || !pte_user(e) { return false; }
            if pte_huge(e) { return true; } // huge page — jest dostępna
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

// ── Nowy P4 dla procesu użytkownika ──────────────────────────────────────────

/// Nowy P4 dziedziczący kernel mappings.
/// Kernel zajmuje PML4[0..255] (identity map lower half).
/// User pages dodawane przez vmap() z PTE_U.
pub unsafe fn new_user_p4() -> PhysAddr {
    let n   = zpg_locked();
    let src = &*pt_ptr(K_P4);
    let dst = &mut *pt_ptr(n);
    for i in 0..512 { dst.e[i] = src.e[i]; }
    n
}

/// CoW clone P4 dla fork().
/// Kernel half (PML4[0..255]) kopiowana wprost (shared).
/// User half (PML4[256..511]) klonowana z CoW: każda leaf PTE user
/// jest oznaczana jako CoW (read-only + PTE_COW bit) w obu kopiach.
pub unsafe fn clone_user_p4(src_p4: PhysAddr) -> PhysAddr {
    let dst_p4 = zpg_locked();
    MM_LOCK.lock();

    let src4 = &mut *pt_ptr(src_p4);
    let dst4 = &mut *pt_ptr(dst_p4);

    for i4 in 0..512 {
        if !pte_present(src4.e[i4]) { continue; }

        // Kernel half — kopiuj wprost (shared, nie modyfikujemy)
        if i4 < 256 {
            dst4.e[i4] = src4.e[i4];
            continue;
        }

        // User half — klonuj CoW
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

                // Huge page — kopiuj i zwiększ refcount dla każdej ramki
                if pte_huge(src2.e[i2]) {
                    let phys = pte_addr(src2.e[i2]);
                    for j in 0..512usize {
                        frame_inc(phys + j as u64 * PAGE_SIZE as u64);
                    }
                    // Oznacz jako CoW read-only w obu
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
                    if !pte_user(e)    { dst1.e[i1] = e; continue; } // kernel pages: kopiuj

                    let phys = pte_addr(e);
                    frame_inc(phys);

                    // Oznacz CoW: usuń PTE_W, dodaj PTE_COW w obu
                    let cow_e = (e & !PTE_W) | PTE_COW;
                    src1.e[i1] = cow_e;
                    dst1.e[i1] = cow_e;
                }
            }
        }
    }

    // TLB flush — src_p4 ma teraz read-only CoW entries
    tlb_flush_all();

    MM_LOCK.unlock();
    dst_p4
}

/// Zwolnij wszystkie user mappings i sam P4.
/// Kernel half (PML4[0..255]) nie jest dotykana.
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

/// Obsłuż CoW page fault dla adresu `fault_addr` w przestrzeni adresowej `p4`.
/// Wywołać z #PF handlera gdy err & 0x3 == 0x3 (user write fault).
/// Zwraca true jeśli obsłużono, false jeśli to prawdziwy błąd.
pub unsafe fn handle_cow_fault(p4: PhysAddr, fault_addr: VirtAddr) -> bool {
    let v = fault_addr & !(PAGE_SIZE as u64 - 1);

    MM_LOCK.lock();

    // Walk page table do leaf PTE
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
            // Tylko my — przywróć PTE_W, usuń PTE_COW
            t2.e[i2] = (t2.e[i2] & !PTE_COW) | PTE_W;
        } else {
            // Kopiuj cały 2MB blok
            let new_phys = super::pmm::mm_alloc_huge();
            if new_phys == 0 { MM_LOCK.unlock(); return false; }
            core::ptr::copy_nonoverlapping(old_phys as *const u8, new_phys as *mut u8, HUGE_SIZE);
            for j in 0..512usize {
                frame_dec(old_phys + j as u64 * PAGE_SIZE as u64);
                super::frame::frame_init(new_phys + j as u64 * PAGE_SIZE as u64);
            }
            t2.e[i2] = pte_make(new_phys, (t2.e[i2] & !PTE_COW & !PTE_ADDR) | PTE_W | PTE_PS);
        }
        tlb_flush_page(v);
        MM_LOCK.unlock();
        return true;
    }

    let p1p = pte_addr(t2.e[i2]);
    let t1 = &mut *pt_ptr(p1p);
    let i1 = ((v >> 12) & 0x1FF) as usize;
    let e = t1.e[i1];

    if !pte_present(e) || !pte_cow(e) { MM_LOCK.unlock(); return false; }

    let old_phys = pte_addr(e);

    if !frame_shared(old_phys) {
        // Jedyny właściciel — przywróć write, usuń CoW bit
        t1.e[i1] = (e & !PTE_COW) | PTE_W;
    } else {
        // Wykonaj fizyczną kopię
        MM_LOCK.unlock(); // frame_cow_copy może alokować
        let new_phys = frame_cow_copy(old_phys);
        MM_LOCK.lock();
        if new_phys == 0 { MM_LOCK.unlock(); return false; }
        t1.e[i1] = pte_make(new_phys, (e & !PTE_COW & !PTE_ADDR) | PTE_W);
    }

    tlb_flush_page(v);
    MM_LOCK.unlock();
    true
}
