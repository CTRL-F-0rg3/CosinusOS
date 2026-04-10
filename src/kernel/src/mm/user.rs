// CosinusOS — mm/user.rs
// Helpery dla przestrzeni adresowej użytkownika
//
// Funkcje wywoływane przez:
//   • userspace_loader (ładowanie ELF/flat binary)
//   • syscall handler (MEM_ALLOC, MEM_FREE, MMAP, MUNMAP)
//   • fork() handler (clone_user_p4 + AddressSpace clone)
//
// Swap stub:
//   Swap jest przygotowany architekturalnie (PTE_SWAP bit + swap slot),
//   ale fizyczny I/O na ATA/NVMe to osobny moduł (swap_io.rs — TODO).

use super::pmm::{PhysAddr, VirtAddr, PAGE_SIZE, mm_alloc, mm_free_phys};
use super::vmm::{
    vmap, vunmap, vmap_huge, new_user_p4, clone_user_p4, free_user_p4,
    handle_cow_fault, valid_buf, valid_user,
    PTE_W, PTE_U, PTE_NX, PTE_P, PTE_COW, PTE_ADDR, K_P4,
    pt_ptr, pte_present, pte_user, pte_addr, pte_cow,
};
use super::vma::{AddressSpace, VMA_R, VMA_W, VMA_X, VMA_USER, VMA_DEMAND};
use super::frame::{frame_inc, frame_dec, frame_shared};

// ── Stałe ─────────────────────────────────────────────────────────────────────

// Hint dla syscall MEM_ALLOC (stary interfejs kompatybilny)
const LEGACY_ALLOC_BASE: VirtAddr = 0x1000_0000;
const LEGACY_ALLOC_TOP:  VirtAddr = 0x3000_0000;

// Bit w PTE dla "strona jest na swap"
pub const PTE_SWAP: u64 = 1 << 10; // bit 10 (software)
// Bity 11..51 = swap slot index gdy Present=0 i Swap=1
pub const PTE_SWAP_SLOT_SHIFT: u64 = 12;
pub const PTE_SWAP_SLOT_MASK:  u64 = 0x000F_FFFF_FFFF_F000; // bity 12..51

// ── Ładowanie ELF segmentu ────────────────────────────────────────────────────

/// Zmapuj jeden segment ELF do przestrzeni adresowej procesu.
/// `src_phys` = fizyczny adres danych segmentu w pamięci kernel (np. z modułu GRUB).
/// `vaddr`    = docelowy adres wirtualny w przestrzeni procesu.
/// `filesz`   = rozmiar danych w pliku.
/// `memsz`    = rozmiar w pamięci (memsz >= filesz; reszta zerowana).
/// `flags`    = PTE_W | PTE_U itd.
pub unsafe fn map_elf_segment(
    p4:       PhysAddr,
    src:      *const u8,
    vaddr:    VirtAddr,
    filesz:   usize,
    memsz:    usize,
    flags:    u64,
) -> bool {
    if memsz == 0 { return true; }
    let memsz = memsz.min(64 * 1024 * 1024); // max 64 MB na segment

    let page_base = vaddr & !(PAGE_SIZE as u64 - 1);
    let page_end  = (vaddr + memsz as u64 + PAGE_SIZE as u64 - 1)
                    & !(PAGE_SIZE as u64 - 1);

    let mut va = page_base;
    while va < page_end {
        let phys = mm_alloc();
        if phys == 0 { return false; }
        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);

        // Oblicz ile bajtów z pliku kopiujemy do tej strony
        let page_virt_off = if va >= vaddr { va - vaddr } else { 0 } as usize;
        if page_virt_off < filesz {
            let dst_off = if va < vaddr { (vaddr - va) as usize } else { 0 };
            let copy_n  = (PAGE_SIZE - dst_off).min(filesz - page_virt_off);
            core::ptr::copy_nonoverlapping(
                src.add(page_virt_off),
                (phys as *mut u8).add(dst_off),
                copy_n,
            );
        }

        if vmap(p4, va, phys, flags) != 0 { return false; }
        va += PAGE_SIZE as u64;
    }
    true
}

// ── Stary interfejs kompatybilny (syscall MEM_ALLOC / MEM_FREE) ──────────────

/// Stary syscall MEM_ALLOC: alokuj `pages` stron pod `hint`.
/// Jeśli hint=0, wybierz adres z zakresu LEGACY_ALLOC_BASE..TOP.
pub unsafe fn legacy_mem_alloc(
    p4:    PhysAddr,
    pages: usize,
    hint:  VirtAddr,
) -> VirtAddr {
    if pages == 0 || pages > 1024 { return 0; } // max 4 MB

    let base = if hint != 0 {
        hint & !(PAGE_SIZE as u64 - 1)
    } else {
        // Szukaj wolnego zakresu
        match find_free_user_range(p4, pages * PAGE_SIZE, LEGACY_ALLOC_BASE) {
            Some(a) => a,
            None    => return 0,
        }
    };

    for i in 0..pages {
        let va = base + i as u64 * PAGE_SIZE as u64;
        if va >= LEGACY_ALLOC_TOP { return 0; }

        let phys = mm_alloc();
        if phys == 0 {
            // OOM — odmapuj co już zmapowaliśmy
            for j in 0..i { vunmap(p4, base + j as u64 * PAGE_SIZE as u64); }
            return 0;
        }
        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
        if vmap(p4, va, phys, PTE_W | PTE_U | PTE_NX) != 0 {
            mm_free_phys(phys);
            for j in 0..i { vunmap(p4, base + j as u64 * PAGE_SIZE as u64); }
            return 0;
        }
    }
    base
}

/// Stary syscall MEM_FREE: zwolnij `pages` stron pod `ptr`.
pub unsafe fn legacy_mem_free(p4: PhysAddr, ptr: VirtAddr, pages: usize) -> i64 {
    if ptr == 0 || pages == 0 { return -1; }
    for i in 0..pages {
        vunmap(p4, ptr + i as u64 * PAGE_SIZE as u64);
    }
    0
}

// ── Szukanie wolnego zakresu wirtualnego ─────────────────────────────────────

/// Przeszukaj przestrzeń adresową procesu i znajdź `size` bajtów wolnych.
/// Prosta liniowa heurystyka — dla produkcji użyj AddressSpace::find_free_range.
pub unsafe fn find_free_user_range(
    p4:    PhysAddr,
    size:  usize,
    start: VirtAddr,
) -> Option<VirtAddr> {
    let size = ((size + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let mut addr = (start + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

    'outer: while addr + size as u64 <= 0x0000_7FFF_FFFF_0000 {
        // Sprawdź czy wszystkie strony w tym zakresie są wolne
        for pg in 0..(size / PAGE_SIZE) {
            let va = addr + pg as u64 * PAGE_SIZE as u64;
            if virt_to_phys_user(p4, va).is_some() {
                // Strona zajęta — przeskocz za nią
                addr = va + PAGE_SIZE as u64;
                continue 'outer;
            }
        }
        return Some(addr);
    }
    None
}

/// Translacja virt→phys dla adresów user (zwraca None dla kernel-only stron).
unsafe fn virt_to_phys_user(p4: PhysAddr, v: VirtAddr) -> Option<PhysAddr> {
    super::vmm::virt_to_phys(p4, v).filter(|_| valid_user(p4, v))
}

// ── Page fault dispatcher ─────────────────────────────────────────────────────

/// Główny handler page fault — wywołać z kernel #PF ISR.
///
/// `cr3`        = aktualny CR3 (p4 procesu)
/// `fault_addr` = CR2 (adres który spowodował fault)
/// `err`        = error code z procesora:
///               bit 0: P (0=not-present, 1=protection violation)
///               bit 1: W (0=read, 1=write)
///               bit 2: U (0=kernel, 1=user)
///               bit 3: RSVD
///               bit 4: I (instruction fetch)
///
/// Zwraca true jeśli fault obsłużony (retry insn), false → kernel panic / SIGSEGV.
pub unsafe fn handle_page_fault(
    p4:         PhysAddr,
    fault_addr: VirtAddr,
    err:        u64,
    as_ref:     Option<&mut AddressSpace>,
) -> bool {
    let present   = err & 1 != 0;
    let is_write  = err & 2 != 0;
    let is_user   = err & 4 != 0;

    // Fault jądra w przestrzeni kernela — nie obsługujemy tutaj
    if !is_user && fault_addr >= 0xFFFF_8000_0000_0000 { return false; }

    // 1. CoW write fault (strona present ale read-only z PTE_COW)
    if present && is_write {
        if handle_cow_fault(p4, fault_addr) { return true; }
    }

    // 2. Demand paging (strona not-present, VMA exists)
    if !present {
        if let Some(space) = as_ref {
            if space.handle_demand_fault(fault_addr) { return true; }
        }
    }

    // 3. Swap-in stub (not-present + PTE_SWAP bit)
    if !present {
        let swap_pte = read_pte_for(p4, fault_addr);
        if let Some(pte) = swap_pte {
            if pte & PTE_SWAP != 0 {
                // TODO: swap_io::swap_in(p4, fault_addr, slot)
                // Na razie — nie obsługujemy, zwróć false
                return false;
            }
        }
    }

    false // Prawdziwy błąd — kernel panic lub SIGSEGV dla procesu
}

/// Odczytaj surowe PTE dla adresu wirtualnego (bez sprawdzania P-bitu).
/// Używane do detekcji swap entries.
unsafe fn read_pte_for(p4: PhysAddr, v: VirtAddr) -> Option<u64> {
    use super::vmm::{pt_ptr, pte_present, pte_addr, PTE_ADDR};

    macro_rules! step {
        ($tab:expr, $idx:expr) => {{
            let e = (*pt_ptr($tab)).e[$idx];
            if !pte_present(e) {
                // Sprawdź czy to swap entry (P=0 ale inne bity ustawione)
                if e != 0 { return Some(e); }
                return None;
            }
            pte_addr(e)
        }};
    }

    let p3 = step!(p4, ((v >> 39) & 0x1FF) as usize);
    let p2 = step!(p3, ((v >> 30) & 0x1FF) as usize);
    let p1 = step!(p2, ((v >> 21) & 0x1FF) as usize);
    let e  = (*pt_ptr(p1)).e[((v >> 12) & 0x1FF) as usize];
    Some(e)
}

// ── Guard page install ────────────────────────────────────────────────────────

/// Zainstaluj guard page pod adresem `addr` (usuwa istniejące mapowanie jeśli jest).
/// Guard page powoduje #PF który NIE jest obsługiwany → SIGSEGV dla procesu.
pub unsafe fn install_guard_page(p4: PhysAddr, addr: VirtAddr) {
    vunmap(p4, addr & !(PAGE_SIZE as u64 - 1));
    // Nie mapujemy nic — strona pozostaje not-present.
    // page fault handler sprawdzi VMA i znajdzie VmaType::GuardPage → false → SIGSEGV.
}

// ── Swap stub (szkielet) ──────────────────────────────────────────────────────

/// Zakoduj swap entry w PTE: P=0, SWAP=1, slot w bitach 12..51.
#[inline]
pub fn make_swap_pte(slot: u64) -> u64 {
    (slot << PTE_SWAP_SLOT_SHIFT) | PTE_SWAP
    // Present = 0, więc CPU nie będzie tego traktować jako valid mapping
}

/// Zdekoduj numer slotu z swap PTE.
#[inline]
pub fn swap_pte_slot(pte: u64) -> u64 {
    (pte & PTE_SWAP_SLOT_MASK) >> PTE_SWAP_SLOT_SHIFT
}

// ── Re-eksport dla syscall handlera ──────────────────────────────────────────

pub use super::vmm::{valid_buf, valid_user, new_user_p4, clone_user_p4, free_user_p4};
pub use super::pmm::{mm_alloc, mm_free_phys, mm_alloc_huge, mm_free_huge, mm_free_kb, mm_used_kb};
