// CosinusOS — allocator/slab.rs
// Slab allocator — fixed-size klasy dla małych alokacji kernela
//
// Architektura:
//   Klasy rozmiarów: 8, 16, 32, 64, 128, 256, 512 bajtów (7 klas)
//   Każda klasa ma własną free-listę wolnych slotów (intrusywna, O(1) alloc/free)
//
//   Slab = strona (4KB) podzielona na sloty danej klasy.
//   Każdy wolny slot przechowuje wskaźnik do następnego wolnego (intrusive linked list).
//
//   Pamięć na slabes pochodzi z BuddyAllocator (żądamy order-0 bloków).
//
// Dlaczego slab jest lepszy niż bump dla kernela:
//   - O(1) alloc i free (versus bump który nie ma free)
//   - Zero fragmentacji dla obiektów stałego rozmiaru (Thread, IpcMsg, itd.)
//   - Cache-friendly — obiekty tej samej klasy leżą blisko siebie
//
// Klasy i typowe użytkowania:
//   8B   → małe flagi, liczniki
//   16B  → małe struktury
//   32B  → krótkie stringi, deskryptory
//   64B  → cache-line aligned obiekty
//   128B → małe bufory, wiadomości IPC
//   256B → średnie struktury
//   512B → bufor przed buddy (>512B idzie do buddy)
//
// Thread-safety: SlabAllocator nie jest thread-safe — spinlock nakłada KernelHeap

use core::ptr;

// ─────────────────────────────────────────────────────────────────────────────
// Konfiguracja
// ─────────────────────────────────────────────────────────────────────────────

const PAGE_SIZE:   usize = 0x1000;

/// Rozmiary klas slab (muszą być >= size_of::<*mut u8>() = 8)
pub const SLAB_CLASSES: [usize; 7] = [8, 16, 32, 64, 128, 256, 512];
pub const N_CLASSES:    usize       = SLAB_CLASSES.len();

/// Maksymalny rozmiar obsługiwany przez slab (>MAX_SLAB_SIZE idzie do buddy)
pub const MAX_SLAB_SIZE: usize = SLAB_CLASSES[N_CLASSES - 1]; // 512B

// ─────────────────────────────────────────────────────────────────────────────
// SlabClass — jedna klasa rozmiarów
// ─────────────────────────────────────────────────────────────────────────────

struct SlabClass {
    /// Rozmiar obiektu w tej klasie
    obj_size: usize,
    /// Głowa intrusywnej listy wolnych slotów
    free_head: *mut *mut u8,
    /// Liczba wolnych slotów (statystyki)
    free_count: usize,
    /// Liczba zaalokowanych stron (slabów)
    slab_count: usize,
}

impl SlabClass {
    const fn new(obj_size: usize) -> Self {
        Self {
            obj_size,
            free_head: ptr::null_mut(),
            free_count: 0,
            slab_count: 0,
        }
    }

    /// Ile slotów mieści się w jednej stronie
    #[inline]
    fn slots_per_slab(&self) -> usize {
        PAGE_SIZE / self.obj_size
    }

    /// Pobierz slot z free listy (O(1))
    #[inline]
    unsafe fn pop(&mut self) -> *mut u8 {
        if self.free_head.is_null() { return ptr::null_mut(); }
        let slot = self.free_head as *mut u8;
        // Każdy wolny slot trzyma wskaźnik do następnego w swoich pierwszych bajtach
        self.free_head = *(self.free_head as *mut *mut *mut u8).read() as *mut *mut u8;
        self.free_count -= 1;
        slot
    }

    /// Wstaw slot do free listy (O(1))
    #[inline]
    unsafe fn push(&mut self, ptr: *mut u8) {
        // Wpisz adres poprzedniego head do pierwszych bajtów slotu
        let slot = ptr as *mut *mut u8;
        *slot = self.free_head as *mut u8;
        self.free_head = slot;
        self.free_count += 1;
    }

    /// Podziel nowo uzyskaną stronę na sloty i dodaj do free listy
    unsafe fn populate_from_page(&mut self, page: *mut u8) {
        let slots = self.slots_per_slab();
        // Linkuj sloty od końca do początku (żeby pierwszy slot był na górze listy)
        let mut i = slots;
        while i > 0 {
            i -= 1;
            let slot = page.add(i * self.obj_size);
            self.push(slot);
        }
        self.slab_count += 1;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SlabAllocator
// ─────────────────────────────────────────────────────────────────────────────

/// Callback do alokacji stron z buddy (żeby slab nie zależał bezpośrednio od buddy)
/// Zwraca wskaźnik do strony (PAGE_SIZE bajtów) lub null
pub type PageAlloc = unsafe fn() -> *mut u8;
pub type PageFree  = unsafe fn(*mut u8);

pub struct SlabAllocator {
    classes:    [SlabClass; N_CLASSES],
    page_alloc: Option<PageAlloc>,
    page_free:  Option<PageFree>,
}

impl SlabAllocator {
    pub const fn new() -> Self {
        Self {
            classes: [
                SlabClass::new(SLAB_CLASSES[0]),
                SlabClass::new(SLAB_CLASSES[1]),
                SlabClass::new(SLAB_CLASSES[2]),
                SlabClass::new(SLAB_CLASSES[3]),
                SlabClass::new(SLAB_CLASSES[4]),
                SlabClass::new(SLAB_CLASSES[5]),
                SlabClass::new(SLAB_CLASSES[6]),
            ],
            page_alloc: None,
            page_free:  None,
        }
    }

    pub fn init(&mut self, alloc: PageAlloc, free: PageFree) {
        self.page_alloc = Some(alloc);
        self.page_free  = Some(free);
    }

    /// Znajdź indeks klasy dla rozmiaru `size` (lub None jeśli >MAX_SLAB_SIZE)
    #[inline]
    pub fn class_for(size: usize) -> Option<usize> {
        for (i, &cls) in SLAB_CLASSES.iter().enumerate() {
            if size <= cls { return Some(i); }
        }
        None
    }

    /// Alokuj obiekt rozmiaru `size` (wyrównaj do klasy)
    pub unsafe fn alloc(&mut self, size: usize) -> *mut u8 {
        let Some(ci) = Self::class_for(size) else { return ptr::null_mut(); };
        let cls = &mut self.classes[ci];

        // Spróbuj z free listy
        if !cls.free_head.is_null() {
            return cls.pop();
        }

        // Brak wolnych slotów — żądaj nowej strony od buddy
        let page_fn = self.page_alloc?;
        let page = page_fn();
        if page.is_null() { return ptr::null_mut(); }

        // Podziel stronę na sloty
        cls.populate_from_page(page);

        // Teraz jest co zwrócić
        cls.pop()
    }

    /// Zwolnij obiekt rozmiaru `size` pod adresem `ptr`
    pub unsafe fn free(&mut self, ptr: *mut u8, size: usize) {
        if ptr.is_null() { return; }
        let Some(ci) = Self::class_for(size) else { return; };
        self.classes[ci].push(ptr);
    }

    // ── Statystyki ───────────────────────────────────────────────────────────

    pub fn free_slots(&self, class: usize) -> usize {
        if class >= N_CLASSES { return 0; }
        self.classes[class].free_count
    }

    pub fn slab_pages(&self, class: usize) -> usize {
        if class >= N_CLASSES { return 0; }
        self.classes[class].slab_count
    }

    /// Łączna pamięć użyta przez wszystkie slabes (strony)
    pub fn total_slab_kb(&self) -> usize {
        let pages: usize = self.classes.iter().map(|c| c.slab_count).sum();
        pages * PAGE_SIZE / 1024
    }
}
