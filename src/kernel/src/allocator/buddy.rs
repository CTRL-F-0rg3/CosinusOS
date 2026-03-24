// CosinusOS — allocator/buddy.rs
// Buddy allocator — zarządzanie blokami 4KB–2MB (power-of-2)
//
// Architektura:
//   Zarządza FIZYCZNĄ przestrzenią heap kernela (oddzielnie od PMM który
//   zarządza ramkami userspace). Buddy operuje na wirtualnych adresach
//   mappowanego heap regionu kernela.
//
//   Order 0 = 4KB  (PAGE_SIZE)
//   Order 1 = 8KB
//   ...
//   Order 9 = 2MB
//
//   MAX_ORDER = 10  →  buddy obsługuje bloki do 2MB
//
// Free listy:
//   free_lists[order] = intrusywna lista wolnych bloków tego orderu.
//   Każdy wolny blok trzyma wskaźniki prev/next w swoich pierwszych 16 bajtach
//   (piszemy bezpośrednio do pamięci bloku — brak dodatkowej alokacji).
//
// Algorytm alloc(order):
//   1. Szukaj wolnego bloku w free_lists[order]
//   2. Jeśli brak, weź z order+1 i split na dwa buddy
//   3. Jeden oddaj do free_lists[order], drugi zwróć
//
// Algorytm free(addr, order):
//   1. Oblicz adres buddy (XOR z rozmiarem bloku)
//   2. Jeśli buddy jest wolny — scal i idź wyżej (coalesce)
//   3. Wstaw do free_lists[order]
//
// Ograniczenia:
//   - HEAP_BASE musi być wyrównany do MAX_BLOCK_SIZE (2MB)
//   - Bitmapa zajętości: 1 bit per blok order-0 → ~512B na 16MB heap
//   - Nie-thread-safe — spinlock nakłada KernelHeap

use core::ptr;

// ─────────────────────────────────────────────────────────────────────────────
// Konfiguracja
// ─────────────────────────────────────────────────────────────────────────────

pub const PAGE_SIZE:   usize = 0x1000;       // 4KB
pub const MAX_ORDER:   usize = 10;           // order 9 = 2MB, +1 bo range
pub const MAX_BLOCK:   usize = PAGE_SIZE << (MAX_ORDER - 1); // 2MB

/// Maksymalny rozmiar heap zarządzanego przez buddy (musi być wielokrotnością MAX_BLOCK)
pub const BUDDY_HEAP_SIZE: usize = 32 * 1024 * 1024; // 32MB

/// Liczba stron order-0 w całym heap
const N_PAGES: usize = BUDDY_HEAP_SIZE / PAGE_SIZE;

/// Rozmiar bitmapy (1 bit per strona order-0)
const BITMAP_WORDS: usize = N_PAGES / 64 + 1;

// ─────────────────────────────────────────────────────────────────────────────
// Intrusywna lista wolnych bloków
// ─────────────────────────────────────────────────────────────────────────────

/// Węzeł intrusywny — zapisywany bezpośrednio w pamięci wolnego bloku
#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
    prev: *mut FreeNode,
}

impl FreeNode {
    #[inline]
    unsafe fn init(ptr: *mut u8) -> *mut FreeNode {
        let node = ptr as *mut FreeNode;
        (*node).next = ptr::null_mut();
        (*node).prev = ptr::null_mut();
        node
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BuddyAllocator
// ─────────────────────────────────────────────────────────────────────────────

pub struct BuddyAllocator {
    /// Bazowy adres wirtualny heap
    base:       usize,
    /// Rozmiar heap w bajtach
    size:       usize,
    /// Głowy list wolnych bloków per order
    free_lists: [*mut FreeNode; MAX_ORDER],
    /// Bitmapa: bit=1 → blok order-0 jest wolny (używamy do coalesce check)
    /// Właściwie trzymamy "split bitmap" — bit per para buddy na każdym orderu
    /// Dla uproszczenia: 1 bit per blok order-0 → 0=zajęty, 1=wolny
    bitmap:     [u64; BITMAP_WORDS],
    /// Łączna liczba wolnych bajtów (statystyki)
    pub free_bytes: usize,
}

impl BuddyAllocator {
    pub const fn new() -> Self {
        Self {
            base:       0,
            size:       0,
            free_lists: [ptr::null_mut(); MAX_ORDER],
            bitmap:     [0u64; BITMAP_WORDS],
            free_bytes: 0,
        }
    }

    /// Inicjalizacja — podaj bazowy adres wirtualny i rozmiar (wielokrotność MAX_BLOCK)
    pub unsafe fn init(&mut self, base: usize, size: usize) {
        debug_assert!(base % MAX_BLOCK == 0, "buddy base musi byc wyrownany do 2MB");
        debug_assert!(size % MAX_BLOCK == 0, "buddy size musi byc wielokrotnoscia 2MB");
        debug_assert!(size <= BUDDY_HEAP_SIZE, "buddy size przekracza BUDDY_HEAP_SIZE");

        self.base = base;
        self.size = size;

        // Zeruj free listy i bitmapę
        for i in 0..MAX_ORDER { self.free_lists[i] = ptr::null_mut(); }
        for i in 0..BITMAP_WORDS { self.bitmap[i] = 0; }
        self.free_bytes = 0;

        // Wstaw całą pamięć jako bloki maksymalnego orderu
        let mut offset = 0usize;
        while offset + MAX_BLOCK <= size {
            self.free_push(base + offset, MAX_ORDER - 1);
            offset += MAX_BLOCK;
        }
        // Reszta (jeśli size nie jest wielokrotnością MAX_BLOCK) — wstaw mniejsze
        while offset < size {
            let mut ord = MAX_ORDER - 2;
            let block_sz = PAGE_SIZE << ord;
            if offset + block_sz <= size {
                self.free_push(base + offset, ord);
                offset += block_sz;
            } else if ord > 0 {
                ord -= 1;
            } else {
                break;
            }
        }
    }

    /// Alokuj blok o rozmiarze `size` bajtów.
    /// Zwraca wskaźnik lub null jeśli brak pamięci.
    pub unsafe fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        if size == 0 { return ptr::null_mut(); }
        let order = self.size_to_order(size.max(align));
        if order >= MAX_ORDER { return ptr::null_mut(); }
        self.alloc_order(order)
    }

    /// Zwolnij blok pod `ptr` o rozmiarze `size` bajtów.
    pub unsafe fn free(&mut self, ptr: *mut u8, size: usize) {
        if ptr.is_null() || size == 0 { return; }
        let order = self.size_to_order(size);
        if order >= MAX_ORDER { return; }
        self.free_coalesce(ptr as usize, order);
    }

    // ── Wewnętrzne ──────────────────────────────────────────────────────────

    /// Minimalny order który pomieści `size` bajtów
    #[inline]
    fn size_to_order(&self, size: usize) -> usize {
        let mut order = 0;
        let mut block = PAGE_SIZE;
        while block < size && order < MAX_ORDER - 1 {
            block <<= 1;
            order += 1;
        }
        order
    }

    #[inline]
    fn block_size(order: usize) -> usize {
        PAGE_SIZE << order
    }

    /// Adres buddy bloku (XOR z rozmiarem)
    #[inline]
    fn buddy_of(&self, addr: usize, order: usize) -> usize {
        let size = Self::block_size(order);
        let offset = addr - self.base;
        self.base + (offset ^ size)
    }

    /// Indeks bloku order-0 (dla bitmapy)
    #[inline]
    fn page_idx(&self, addr: usize) -> usize {
        (addr - self.base) / PAGE_SIZE
    }

    // ── Bitmap ──────────────────────────────────────────────────────────────

    unsafe fn bitmap_set_free(&mut self, addr: usize, order: usize) {
        let pages = 1 << order;
        let start = self.page_idx(addr);
        for i in start..start + pages {
            if i / 64 < BITMAP_WORDS {
                self.bitmap[i / 64] |= 1u64 << (i % 64);
            }
        }
    }

    unsafe fn bitmap_set_used(&mut self, addr: usize, order: usize) {
        let pages = 1 << order;
        let start = self.page_idx(addr);
        for i in start..start + pages {
            if i / 64 < BITMAP_WORDS {
                self.bitmap[i / 64] &= !(1u64 << (i % 64));
            }
        }
    }

    /// Sprawdź czy cały blok jest wolny (bitmap)
    fn bitmap_is_free(&self, addr: usize, order: usize) -> bool {
        let pages = 1 << order;
        let start = self.page_idx(addr);
        for i in start..start + pages {
            if i / 64 >= BITMAP_WORDS { return false; }
            if self.bitmap[i / 64] & (1u64 << (i % 64)) == 0 { return false; }
        }
        true
    }

    // ── Free list operacje ───────────────────────────────────────────────────

    /// Wstaw blok do free listy orderu
    unsafe fn free_push(&mut self, addr: usize, order: usize) {
        let node = FreeNode::init(addr as *mut u8);
        (*node).next = self.free_lists[order];
        (*node).prev = ptr::null_mut();
        if !self.free_lists[order].is_null() {
            (*self.free_lists[order]).prev = node;
        }
        self.free_lists[order] = node;
        self.bitmap_set_free(addr, order);
        self.free_bytes += Self::block_size(order);
    }

    /// Wyjmij blok z free listy orderu (zwraca adres lub 0)
    unsafe fn free_pop(&mut self, order: usize) -> usize {
        if self.free_lists[order].is_null() { return 0; }
        let node = self.free_lists[order];
        self.free_lists[order] = (*node).next;
        if !(*node).next.is_null() {
            (*(*node).next).prev = ptr::null_mut();
        }
        let addr = node as usize;
        self.bitmap_set_used(addr, order);
        self.free_bytes -= Self::block_size(order);
        addr
    }

    /// Usuń konkretny blok z free listy (dla coalesce)
    unsafe fn free_remove(&mut self, addr: usize, order: usize) {
        let node = addr as *mut FreeNode;
        if !(*node).prev.is_null() {
            (*(*node).prev).next = (*node).next;
        } else {
            // Node jest głową listy
            self.free_lists[order] = (*node).next;
        }
        if !(*node).next.is_null() {
            (*(*node).next).prev = (*node).prev;
        }
        self.bitmap_set_used(addr, order);
        self.free_bytes -= Self::block_size(order);
    }

    /// Alokuj blok orderu `order` (rekurencyjnie splituje wyższe ordery)
    unsafe fn alloc_order(&mut self, order: usize) -> *mut u8 {
        // Szukaj od order w górę
        let mut found_order = MAX_ORDER; // sentinel
        for o in order..MAX_ORDER {
            if !self.free_lists[o].is_null() {
                found_order = o;
                break;
            }
        }
        if found_order == MAX_ORDER { return ptr::null_mut(); }

        // Wyjmij blok
        let mut addr = self.free_pop(found_order);
        if addr == 0 { return ptr::null_mut(); }

        // Splituj od found_order w dół do order
        let mut current_order = found_order;
        while current_order > order {
            current_order -= 1;
            let buddy_addr = addr + Self::block_size(current_order);
            // Oddaj buddy do free listy
            self.free_push(buddy_addr, current_order);
        }

        addr as *mut u8
    }

    /// Zwolnij blok i scal z buddy (coalesce w górę)
    unsafe fn free_coalesce(&mut self, mut addr: usize, mut order: usize) {
        while order < MAX_ORDER - 1 {
            let buddy = self.buddy_of(addr, order);

            // Buddy musi być w obrębie heap
            if buddy < self.base || buddy >= self.base + self.size { break; }

            // Sprawdź czy buddy jest wolny (bitmap)
            if !self.bitmap_is_free(buddy, order) { break; }

            // Usuń buddy z free listy
            self.free_remove(buddy, order);

            // Scal — nowy blok zaczyna się od niższego adresu
            if buddy < addr { addr = buddy; }
            order += 1;
        }

        self.free_push(addr, order);
    }

    // ── Statystyki ───────────────────────────────────────────────────────────

    pub fn free_kb(&self)  -> usize { self.free_bytes / 1024 }
    pub fn total_kb(&self) -> usize { self.size / 1024 }
    pub fn used_kb(&self)  -> usize { self.total_kb() - self.free_kb() }
}
