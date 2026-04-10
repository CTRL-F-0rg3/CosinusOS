// CosinusOS — mm/vma.rs
// Virtual Memory Areas (VMAs) — śledzenie regionów wirtualnej przestrzeni adresowej
//
// Każdy proces ma listę VMA opisujących regiony:
//   • adres bazowy i rozmiar
//   • flagi (R/W/X, user/kernel, demand-paged, guard)
//   • typ (anonymous, file-backed stub, stack, heap)
//
// Demand paging: strony oznaczone VMA_DEMAND nie są fizycznie mapowane
// przy tworzeniu — są alokowane przy pierwszym #PF (not-present fault).
//
// ASLR: nowe regiony heap/stack/anonymous mmap dostają losowy offset
// z hardware RDRAND lub software LFSR.

use super::pmm::{PhysAddr, VirtAddr, PAGE_SIZE, mm_alloc};
use super::vmm::{vmap, vunmap, PTE_W, PTE_U, PTE_NX, PTE_COW};

// ── Stałe ─────────────────────────────────────────────────────────────────────

pub const MAX_VMAS: usize = 128; // max regionów na proces

// Flagi VMA
pub const VMA_R:       u32 = 1 << 0; // readable
pub const VMA_W:       u32 = 1 << 1; // writable
pub const VMA_X:       u32 = 1 << 2; // executable
pub const VMA_USER:    u32 = 1 << 3; // user-accessible
pub const VMA_DEMAND:  u32 = 1 << 4; // demand-paged (lazy)
pub const VMA_GUARD:   u32 = 1 << 5; // guard page (nigdy nie mapuj)
pub const VMA_STACK:   u32 = 1 << 6; // rosnący w dół stack
pub const VMA_HEAP:    u32 = 1 << 7; // heap (sbrk-style)
pub const VMA_SHARED:  u32 = 1 << 8; // shared między procesami
pub const VMA_FIXED:   u32 = 1 << 9; // adres nie może być przesunięty

// Zakresy przestrzeni adresowej userspace (canonical 48-bit)
pub const USER_CODE_BASE:  VirtAddr = 0x0000_0000_0040_0000; // 4 MB
pub const USER_HEAP_BASE:  VirtAddr = 0x0000_0000_1000_0000; // 256 MB
pub const USER_MMAP_BASE:  VirtAddr = 0x0000_0000_4000_0000; // 1 GB
pub const USER_STACK_TOP:  VirtAddr = 0x0000_7FFF_FFFF_0000; // poniżej 128 TB
pub const USER_STACK_SIZE: VirtAddr = 0x0000_0000_0080_0000; // 8 MB domyślny stack

// ASLR entropy — losowy offset stosowany do heap/mmap/stack
const ASLR_HEAP_BITS:  u64 = 8;  // 256 możliwych pozycji × PAGE_SIZE
const ASLR_MMAP_BITS:  u64 = 16; // 64K możliwych pozycji × PAGE_SIZE
const ASLR_STACK_BITS: u64 = 8;  // 256 możliwych pozycji × PAGE_SIZE

// ── Typy ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VmaType {
    Unused,
    Anonymous,   // mmap MAP_ANONYMOUS
    Stack,       // stack procesu
    Heap,        // heap (brk/sbrk)
    Code,        // segment kodu z ELF
    Data,        // segment danych z ELF
    GuardPage,   // strona-pułapka (nigdy nie mapowana)
}

#[derive(Clone, Copy)]
pub struct Vma {
    pub base:  VirtAddr,
    pub size:  usize,        // w bajtach, wielokrotność PAGE_SIZE
    pub flags: u32,
    pub typ:   VmaType,
    // Demand paging: ile stron jest już fizycznie zmapowanych
    pub mapped_pages: usize,
}

impl Vma {
    pub const fn empty() -> Self {
        Self { base: 0, size: 0, flags: 0, typ: VmaType::Unused, mapped_pages: 0 }
    }

    pub fn end(&self) -> VirtAddr { self.base + self.size as u64 }

    pub fn contains(&self, addr: VirtAddr) -> bool {
        addr >= self.base && addr < self.end()
    }

    pub fn overlaps(&self, other_base: VirtAddr, other_size: usize) -> bool {
        self.base < other_base + other_size as u64
            && other_base < self.end()
    }

    pub fn pte_flags(&self) -> u64 {
        let mut f = 0u64;
        if self.flags & VMA_W    != 0 { f |= PTE_W; }
        if self.flags & VMA_USER != 0 { f |= PTE_U; }
        if self.flags & VMA_X    == 0 { f |= PTE_NX; } // W^X domyślnie
        f
    }
}

// ── AddressSpace — przestrzeń adresowa procesu ───────────────────────────────

pub struct AddressSpace {
    pub p4:    PhysAddr,
    vmas:      [Vma; MAX_VMAS],
    vma_count: usize,
    // Wskaźniki brk dla heap
    pub heap_start: VirtAddr,
    pub heap_end:   VirtAddr,
    // Następny wolny adres dla mmap
    mmap_next: VirtAddr,
    // ASLR seed (LFSR)
    aslr_state: u64,
}

impl AddressSpace {
    pub const fn new(p4: PhysAddr) -> Self {
        const EMPTY_VMA: Vma = Vma::empty();
        Self {
            p4,
            vmas: [EMPTY_VMA; MAX_VMAS],
            vma_count: 0,
            heap_start: 0,
            heap_end:   0,
            mmap_next:  USER_MMAP_BASE,
            aslr_state: 0xDEAD_BEEF_CAFE_0001,
        }
    }

    // ── ASLR LFSR ─────────────────────────────────────────────────────────────

    fn aslr_rand(&mut self) -> u64 {
        // Próbuj hardware RDRAND, fallback do LFSR
        let mut v: u64 = 0;
        unsafe {
            // RDRAND: max 10 prób
            for _ in 0..10 {
                let ok: u8;
                core::arch::asm!(
                    "rdrand {}",
                    "setc {}",
                    out(reg) v,
                    out(reg_byte) ok,
                    options(nostack),
                );
                if ok != 0 { break; }
            }
        }
        if v == 0 {
            // Galois LFSR 64-bit (polynomial: x^64+x^63+x^61+x^60+1)
            let s = self.aslr_state;
            let bit = ((s >> 63) ^ (s >> 62) ^ (s >> 60) ^ (s >> 59)) & 1;
            self.aslr_state = (s << 1) | bit;
            self.aslr_state
        } else {
            self.aslr_state ^= v;
            v
        }
    }

    fn aslr_offset(&mut self, bits: u64) -> u64 {
        let mask = (1u64 << bits) - 1;
        (self.aslr_rand() & mask) * PAGE_SIZE as u64
    }

    // ── VMA management ────────────────────────────────────────────────────────

    pub fn find_vma(&self, addr: VirtAddr) -> Option<&Vma> {
        self.vmas[..self.vma_count].iter().find(|v| v.contains(addr))
    }

    pub fn find_vma_mut(&mut self, addr: VirtAddr) -> Option<&mut Vma> {
        let count = self.vma_count;
        self.vmas[..count].iter_mut().find(|v| v.contains(addr))
    }

    fn has_overlap(&self, base: VirtAddr, size: usize) -> bool {
        self.vmas[..self.vma_count].iter()
            .filter(|v| v.typ != VmaType::Unused)
            .any(|v| v.overlaps(base, size))
    }

    fn add_vma(&mut self, vma: Vma) -> bool {
        if self.vma_count >= MAX_VMAS { return false; }
        self.vmas[self.vma_count] = vma;
        self.vma_count += 1;
        // Posortuj po adresie bazowym (insertion sort — MAX_VMAS małe)
        let n = self.vma_count;
        for i in (1..n).rev() {
            if self.vmas[i].base < self.vmas[i-1].base {
                self.vmas.swap(i, i-1);
            } else { break; }
        }
        true
    }

    fn remove_vma(&mut self, base: VirtAddr) -> bool {
        if let Some(idx) = self.vmas[..self.vma_count]
            .iter().position(|v| v.base == base)
        {
            for i in idx..self.vma_count - 1 {
                self.vmas[i] = self.vmas[i+1];
            }
            self.vma_count -= 1;
            self.vmas[self.vma_count] = Vma::empty();
            return true;
        }
        false
    }

    // Znajdź wolny zakres wirtualny o podanym rozmiarze
    fn find_free_range(&self, size: usize, hint: VirtAddr) -> Option<VirtAddr> {
        let mut addr = hint;
        'outer: loop {
            if addr + size as u64 > USER_STACK_TOP { return None; }
            // Sprawdź czy nie koliduje z istniejącymi VMA
            for vma in &self.vmas[..self.vma_count] {
                if vma.typ == VmaType::Unused { continue; }
                if addr < vma.end() && addr + size as u64 > vma.base {
                    addr = vma.end();
                    // Wyrównaj do strony
                    addr = (addr + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                    continue 'outer;
                }
            }
            return Some(addr);
        }
    }

    // ── Inicjalizacja przestrzeni adresowej ──────────────────────────────────

    /// Inicjalizuj heap i stack z ASLR.
    pub fn init_aslr(&mut self) {
        let heap_off  = self.aslr_offset(ASLR_HEAP_BITS);
        let stack_off = self.aslr_offset(ASLR_STACK_BITS);

        self.heap_start = USER_HEAP_BASE + heap_off;
        self.heap_end   = self.heap_start;
        self.mmap_next  = USER_MMAP_BASE + self.aslr_offset(ASLR_MMAP_BITS);

        // Stack: rośnie w dół od USER_STACK_TOP - random_offset
        let stack_base = USER_STACK_TOP - USER_STACK_SIZE - stack_off;

        // Guard page przed (poniżej) stackiem
        let guard = Vma {
            base:  stack_base,
            size:  PAGE_SIZE,
            flags: VMA_GUARD | VMA_USER,
            typ:   VmaType::GuardPage,
            mapped_pages: 0,
        };
        self.add_vma(guard);

        // Stack VMA (demand-paged, rośnie w dół)
        let stack = Vma {
            base:  stack_base + PAGE_SIZE as u64,
            size:  USER_STACK_SIZE as usize - PAGE_SIZE,
            flags: VMA_R | VMA_W | VMA_USER | VMA_DEMAND | VMA_STACK,
            typ:   VmaType::Stack,
            mapped_pages: 0,
        };
        self.add_vma(stack);
    }

    // ── mmap — mapuj anonimowy region ────────────────────────────────────────

    /// Mapuj `size` bajtów pod `hint` (lub auto-wybór z ASLR).
    /// Jeśli `demand=true` — strony są alokowane leniwie przy page fault.
    pub fn mmap_anon(&mut self, hint: VirtAddr, size: usize, flags: u32, demand: bool) -> VirtAddr {
        let size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        if size == 0 { return 0; }

        // Wybór adresu
        let base = if hint != 0 && flags & VMA_FIXED != 0 {
            hint
        } else {
            let start = if hint != 0 { hint } else { self.mmap_next };
            let off = self.aslr_offset(ASLR_MMAP_BITS);
            match self.find_free_range(size, (start + off) & !(PAGE_SIZE as u64 - 1)) {
                Some(a) => a,
                None    => return 0,
            }
        };

        if self.has_overlap(base, size) { return 0; }

        let actual_flags = flags | VMA_R | VMA_USER | if demand { VMA_DEMAND } else { 0 };

        let vma = Vma {
            base,
            size,
            flags: actual_flags,
            typ:   VmaType::Anonymous,
            mapped_pages: 0,
        };

        if !self.add_vma(vma) { return 0; }
        self.mmap_next = base + size as u64 + PAGE_SIZE as u64; // bump pointer

        if !demand {
            // Eager alloc: mapuj wszystkie strony od razu
            let p4 = self.p4;
            let pte_f = vma.pte_flags();
            for pg in 0..(size / PAGE_SIZE) {
                unsafe {
                    let phys = mm_alloc();
                    if phys == 0 { return 0; }
                    core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
                    vmap(p4, base + pg as u64 * PAGE_SIZE as u64, phys, pte_f);
                }
            }
            if let Some(v) = self.find_vma_mut(base) {
                v.mapped_pages = size / PAGE_SIZE;
            }
        }

        base
    }

    /// Odmapuj region pod `base` (musi pasować do istniejącego VMA).
    pub unsafe fn munmap(&mut self, base: VirtAddr, size: usize) -> bool {
        let size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let p4 = self.p4;

        // Znajdź VMA i zwolnij fizyczne strony
        let vma = match self.vmas[..self.vma_count].iter()
            .find(|v| v.base == base && v.size == size)
        {
            Some(v) => *v,
            None    => return false,
        };

        // Odmapuj strony (tylko te które były zmapowane)
        let pages = vma.size / PAGE_SIZE;
        for pg in 0..pages {
            vunmap(p4, base + pg as u64 * PAGE_SIZE as u64);
        }

        self.remove_vma(base);
        true
    }

    // ── sbrk — rozszerz heap ─────────────────────────────────────────────────

    /// Zwiększ heap o `increment` bajtów. Zwraca stary brk lub 0 przy OOM.
    pub unsafe fn sbrk(&mut self, increment: isize) -> VirtAddr {
        let old_end = self.heap_end;
        let new_end = if increment >= 0 {
            old_end + increment as u64
        } else {
            old_end.saturating_sub((-increment) as u64)
        };

        // Wyrównaj do strony
        let new_end = (new_end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

        if new_end <= old_end && increment >= 0 { return old_end; }

        let p4 = self.p4;

        if new_end > old_end {
            // Rozszerzanie: alokuj nowe strony
            let mut pg = old_end;
            while pg < new_end {
                // Sprawdź czy nie wchodzi w inne VMA
                if self.has_overlap(pg, PAGE_SIZE) { break; }
                let phys = mm_alloc();
                if phys == 0 { break; }
                core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
                vmap(p4, pg, phys, PTE_W | PTE_U | super::vmm::PTE_NX);
                pg += PAGE_SIZE as u64;
            }
            self.heap_end = pg;

            // Aktualizuj lub utwórz heap VMA
            if self.heap_start == old_end {
                // Pierwszy sbrk — utwórz VMA
                let vma = Vma {
                    base:  self.heap_start,
                    size:  (self.heap_end - self.heap_start) as usize,
                    flags: VMA_R | VMA_W | VMA_USER | VMA_HEAP,
                    typ:   VmaType::Heap,
                    mapped_pages: (self.heap_end - self.heap_start) as usize / PAGE_SIZE,
                };
                self.add_vma(vma);
            } else if let Some(v) = self.find_vma_mut(self.heap_start) {
                v.size = (self.heap_end - self.heap_start) as usize;
                v.mapped_pages = v.size / PAGE_SIZE;
            }
        } else {
            // Kurczenie: zwolnij strony
            let mut pg = new_end;
            while pg < old_end {
                vunmap(p4, pg);
                pg += PAGE_SIZE as u64;
            }
            self.heap_end = new_end;
            if let Some(v) = self.find_vma_mut(self.heap_start) {
                v.size = (self.heap_end - self.heap_start) as usize;
                v.mapped_pages = v.size / PAGE_SIZE;
            }
        }

        old_end
    }

    // ── Demand paging ─────────────────────────────────────────────────────────

    /// Obsłuż demand-page fault dla adresu `fault_addr`.
    /// Wywołać z #PF handlera gdy strona jest not-present ale VMA istnieje.
    /// Zwraca true jeśli zmapowano stronę, false jeśli to prawdziwy błąd (SIGSEGV).
    pub unsafe fn handle_demand_fault(&mut self, fault_addr: VirtAddr) -> bool {
        let page = fault_addr & !(PAGE_SIZE as u64 - 1);

        // Znajdź VMA
        let (flags, is_demand, is_guard) = match self.find_vma(fault_addr) {
            None    => return false,
            Some(v) => {
                if v.typ == VmaType::GuardPage { return false; } // → segfault
                (v.pte_flags(), v.flags & VMA_DEMAND != 0, v.flags & VMA_GUARD != 0)
            }
        };

        if is_guard || !is_demand { return false; }

        // Alokuj i wyzeruj ramkę
        let phys = mm_alloc();
        if phys == 0 { return false; } // OOM → segfault

        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
        vmap(self.p4, page, phys, flags);

        // Aktualizuj mapped_pages
        if let Some(v) = self.find_vma_mut(fault_addr) {
            v.mapped_pages += 1;
        }

        true
    }

    // ── Dump ─────────────────────────────────────────────────────────────────

    pub unsafe fn dump(&self) {
        use crate::debug::serial_print;
        serial_print("[VMA] p4=");
        { let mut b = [0u8;18]; serial_print(crate::debug::hex_str(self.p4, &mut b)); }
        serial_print(" vmas=");
        super::pmm::pnum_serial(self.vma_count);
        serial_print("\n");
        for v in &self.vmas[..self.vma_count] {
            serial_print("  ");
            { let mut b=[0u8;18]; serial_print(crate::debug::hex_str(v.base, &mut b)); }
            serial_print("..");
            { let mut b=[0u8;18]; serial_print(crate::debug::hex_str(v.end(), &mut b)); }
            serial_print(match v.typ {
                VmaType::Anonymous => " ANON",
                VmaType::Stack     => " STACK",
                VmaType::Heap      => " HEAP",
                VmaType::Code      => " CODE",
                VmaType::Data      => " DATA",
                VmaType::GuardPage => " GUARD",
                VmaType::Unused    => " (unused)",
            });
            if v.flags & VMA_DEMAND != 0 { serial_print(" demand"); }
            if v.flags & VMA_W != 0 { serial_print(" W"); }
            if v.flags & VMA_X != 0 { serial_print(" X"); }
            serial_print("\n");
        }
    }
}
