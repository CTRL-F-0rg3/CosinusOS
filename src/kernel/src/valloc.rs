// CosinusOS — valloc.rs
// Wirtualny allocator przestrzeni adresowej (per-wątek / per-proces)
//
// Cel: zarządzanie zakresami adresów wirtualnych UŻYTKOWNIKA bez alokacji heap.
// Kernel PMM (mm.rs) zarządza ramkami fizycznymi — valloc zarządza ADRESAMI.
//
// Architektura:
//   ┌────────────────────────────────────────────────────────┐
//   │ VAddrSpace (jeden per wątek)                           │
//   │  bump: u64  — następny wolny adres (rośnie w górę)     │
//   │  free: [FreeRegion; MAX_FREE] — zwrócone zakresy       │
//   │  free_len: usize              — ile wpisów aktywnych   │
//   └────────────────────────────────────────────────────────┘
//
// Algorytm:
//   alloc(n_pages):
//     1. Szukaj w free_list regionu >= n_pages (first-fit)
//     2. Jeśli znaleziony: wytnij n_pages z przodu, resztę zostaw
//     3. Jeśli nie: weź z bump i przesuń bump o n_pages
//
//   free(addr, n_pages):
//     1. Wstaw do free_list (jeśli jest miejsce)
//     2. Scal z sąsiadami (koalescencja)
//
// Zakres adresów userspace:
//   0x0000_8000_0000 – 0x0000_FFFF_F000  (~512GB użytkowe, canonical x86-64)
//   Ale tutaj używamy małego okna: VSPACE_BASE..VSPACE_TOP
//   które nie koliduje ze stosami/tekstem (ładowanymi przez loader poniżej 0x40_0000)
//
// Ograniczenia (bez heap, no_std):
//   MAX_FREE = 64 wpisów wolnych regionów na wątek — wystarczy dla typowego
//   procesu który robi kilkanaście mmap/munmap. Dla przyszłego CosinusOS
//   z pełnym VFS rozszerzamy lub podłączamy slab allocator.

use crate::mm::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Konfiguracja przestrzeni adresowej userspace
// ─────────────────────────────────────────────────────────────────────────────

/// Dolna granica dynamicznej przestrzeni adresowej (128MB — powyżej kodu/stosu)
pub const VSPACE_BASE: u64 = 0x0800_0000;

/// Górna granica (2GB — zostawiamy górę dla przyszłych shared mappings)
pub const VSPACE_TOP:  u64 = 0x8000_0000;

/// Maksymalna liczba wpisów w free liście per wątek (statyczny storage)
pub const MAX_FREE: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// Struktury
// ─────────────────────────────────────────────────────────────────────────────

/// Jeden wolny region w przestrzeni adresowej
#[derive(Copy, Clone, Debug)]
pub struct FreeRegion {
    pub base:   u64,    // adres bazowy (wyrównany do strony)
    pub pages:  usize,  // liczba stron
}

impl FreeRegion {
    pub const fn zero() -> Self { Self { base: 0, pages: 0 } }

    #[inline]
    pub fn end(&self) -> u64 {
        self.base + self.pages as u64 * PAGE_SIZE as u64
    }
}

/// Stan przestrzeni adresowej jednego wątku/procesu.
/// Przechowywany inline w `Thread` — zero alokacji.
#[derive(Copy, Clone)]
pub struct VAddrSpace {
    /// Bump pointer — następny wolny adres (rośnie w górę)
    pub bump:     u64,
    /// Lista zwróconych regionów (posortowana wg adresu dla szybkiej koalescencji)
    pub free:     [FreeRegion; MAX_FREE],
    /// Liczba aktywnych wpisów w free
    pub free_len: usize,
}

impl VAddrSpace {
    pub const fn new() -> Self {
        Self {
            bump:     VSPACE_BASE,
            free:     [FreeRegion::zero(); MAX_FREE],
            free_len: 0,
        }
    }

    /// Czy przestrzeń adresowa jest w stanie początkowym (nic nie zmapowane)
    pub fn is_empty(&self) -> bool {
        self.bump == VSPACE_BASE && self.free_len == 0
    }

    /// Ile stron zostało zaalokowanych przez bump (nie liczy zwróconych)
    pub fn bump_pages(&self) -> usize {
        ((self.bump - VSPACE_BASE) / PAGE_SIZE as u64) as usize
    }
}


pub fn valloc_alloc(vs: &mut VAddrSpace, n_pages: usize) -> Option<u64> {
    if n_pages == 0 { return None; }


    for i in 0..vs.free_len {
        let r = &vs.free[i];
        if r.pages >= n_pages {
            let addr = r.base;
            let leftover = r.pages - n_pages;

            if leftover == 0 {
            
                free_remove(vs, i);
            } else {
           
                vs.free[i].base  += n_pages as u64 * PAGE_SIZE as u64;
                vs.free[i].pages  = leftover;
            }
            return Some(addr);
        }
    }


    let addr = vs.bump;
    let end  = addr + n_pages as u64 * PAGE_SIZE as u64;

    if end > VSPACE_TOP { return None; }

    vs.bump = end;
    Some(addr)
}


pub fn valloc_free(vs: &mut VAddrSpace, addr: u64, n_pages: usize) {
    if n_pages == 0 || addr < VSPACE_BASE || addr >= VSPACE_TOP { return; }
    if addr & (PAGE_SIZE as u64 - 1) != 0 { return; } // źle wyrównany

    let expected_bump = addr + n_pages as u64 * PAGE_SIZE as u64;
    if expected_bump == vs.bump {
        vs.bump = addr;

        loop {
            let mut merged = false;
            for i in 0..vs.free_len {
                if vs.free[i].end() == vs.bump {
                    vs.bump = vs.free[i].base;
                    free_remove(vs, i);
                    merged = true;
                    break;
                }
            }
            if !merged { break; }
        }
        return;
    }


    if vs.free_len >= MAX_FREE {
 
        return;
    }


    let mut ins = vs.free_len;
    for i in 0..vs.free_len {
        if vs.free[i].base > addr {
            ins = i;
            break;
        }
    }

    let mut j = vs.free_len;
    while j > ins {
        vs.free[j] = vs.free[j - 1];
        j -= 1;
    }
    vs.free[ins] = FreeRegion { base: addr, pages: n_pages };
    vs.free_len += 1;

    coalesce(vs);
}

pub fn valloc_contains(vs: &VAddrSpace, addr: u64, pages: usize) -> bool {
    if addr < VSPACE_BASE || addr >= VSPACE_TOP { return false; }
    let end = addr + pages as u64 * PAGE_SIZE as u64;
    if end > vs.bump { return false; }

    for i in 0..vs.free_len {
        let r = &vs.free[i];
        if r.base < end && r.end() > addr {
            return false; 
        }
    }
    true
}

pub fn valloc_reset(vs: &mut VAddrSpace) {
    vs.bump     = VSPACE_BASE;
    vs.free_len = 0;

    for i in 0..MAX_FREE { vs.free[i] = FreeRegion::zero(); }
}


pub fn valloc_used_pages(vs: &VAddrSpace) -> usize {
    let total_bump = ((vs.bump - VSPACE_BASE) / PAGE_SIZE as u64) as usize;
    let free_pages: usize = vs.free[..vs.free_len].iter().map(|r| r.pages).sum();
    total_bump.saturating_sub(free_pages)
}



#[inline]
fn free_remove(vs: &mut VAddrSpace, i: usize) {
    for j in i..(vs.free_len - 1) {
        vs.free[j] = vs.free[j + 1];
    }
    vs.free_len -= 1;
    vs.free[vs.free_len] = FreeRegion::zero();
}

fn coalesce(vs: &mut VAddrSpace) {
    let mut i = 0;
    while i + 1 < vs.free_len {
        let end_i = vs.free[i].end();
        let base_j = vs.free[i + 1].base;
        if end_i == base_j {
            
            vs.free[i].pages += vs.free[i + 1].pages;
            free_remove(vs, i + 1);
           
        } else {
            i += 1;
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn pg(n: u64) -> u64 { VSPACE_BASE + n * PAGE_SIZE as u64 }

    #[test]
    fn test_bump_alloc() {
        let mut vs = VAddrSpace::new();
        let a = valloc_alloc(&mut vs, 4).unwrap();
        assert_eq!(a, VSPACE_BASE);
        let b = valloc_alloc(&mut vs, 2).unwrap();
        assert_eq!(b, pg(4));
    }

    #[test]
    fn test_free_and_reuse() {
        let mut vs = VAddrSpace::new();
        let a = valloc_alloc(&mut vs, 4).unwrap();
        let b = valloc_alloc(&mut vs, 4).unwrap();
        let _ = b;
        valloc_free(&mut vs, a, 4);
        let c = valloc_alloc(&mut vs, 4).unwrap();
        assert_eq!(c, a); // reuse
    }

    #[test]
    fn test_coalesce() {
        let mut vs = VAddrSpace::new();
        let a = valloc_alloc(&mut vs, 2).unwrap();
        let b = valloc_alloc(&mut vs, 2).unwrap();
        let _ = valloc_alloc(&mut vs, 2); 
        valloc_free(&mut vs, a, 2);
        valloc_free(&mut vs, b, 2);

        assert_eq!(vs.free_len, 1);
        assert_eq!(vs.free[0].pages, 4);
    }

    #[test]
    fn test_bump_retract_on_free() {
        let mut vs = VAddrSpace::new();
        let a = valloc_alloc(&mut vs, 8).unwrap();
        valloc_free(&mut vs, a, 8);
      
        assert_eq!(vs.bump, VSPACE_BASE);
        assert_eq!(vs.free_len, 0);
    }

    #[test]
    fn test_contains() {
        let mut vs = VAddrSpace::new();
        let a = valloc_alloc(&mut vs, 4).unwrap();
        assert!(valloc_contains(&vs, a, 4));
        assert!(!valloc_contains(&vs, a + 4 * PAGE_SIZE as u64, 1));
        valloc_free(&mut vs, a, 4);
        assert!(!valloc_contains(&vs, a, 4));
    }

    #[test]
    fn test_partial_reuse() {
        let mut vs = VAddrSpace::new();
        let a = valloc_alloc(&mut vs, 8).unwrap();
        let _ = valloc_alloc(&mut vs, 1);
        valloc_free(&mut vs, a, 8);
        let b = valloc_alloc(&mut vs, 3).unwrap();
        assert_eq!(b, a);
        assert_eq!(vs.free[0].pages, 5);
        assert_eq!(vs.free[0].base, a + 3 * PAGE_SIZE as u64);
    }
}
