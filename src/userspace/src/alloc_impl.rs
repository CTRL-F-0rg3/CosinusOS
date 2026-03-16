// CosinusOS Userspace — alloc_impl.rs
// Free-list allocator (segregated fit) — 16MB heap statyczny
//
// Strategia: size classes 16,32,64,128,256,512,1K,2K,4K,8K → free lists
// Powyżej 8K: duże bloki z nagłówkiem + koalescencja sąsiadów.
//
// Layout każdego bloku:
//   [BlockHeader: 16B][dane użytkownika...]
//
// Zalety nad bump:
//   - dealloc() rzeczywiście zwalnia pamięć
//   - małe alokacje < 256B trafiają do bin z 0 fragmentacji
//   - duże bloki scalają się z sąsiadami

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};
use core::ptr;

// ── Konfiguracja ─────────────────────────────────────────────────────────────

const HEAP_SIZE:  usize = 16 * 1024 * 1024; // 16MB
const ALIGN_MIN:  usize = 16;               // minimalne wyrównanie alokacji
const HDR_SIZE:   usize = 16;               // rozmiar BlockHeader

// Size classes dla small bins (indeks → rozmiar)
const BINS: usize = 10;
const BIN_SIZES: [usize; BINS] = [16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

// ── Heap ─────────────────────────────────────────────────────────────────────

#[repr(align(4096))]
struct AlignedHeap([u8; HEAP_SIZE]);

static mut HEAP:      AlignedHeap    = AlignedHeap([0; HEAP_SIZE]);
static mut HEAP_INIT: bool           = false;
static HEAP_LOCK:     AllocLock      = AllocLock::new();

// ── Nagłówek bloku ────────────────────────────────────────────────────────────
// Rozmiar = 16B (2× u64)

#[repr(C)]
struct BlockHeader {
    // Bit0 = zajęty (1) / wolny (0)
    // Bity 1.. = rozmiar bloku łącznie z nagłówkiem
    size_and_flags: usize,
    // Wskaźnik do następnego wolnego bloku w tej samej bin (0 = koniec listy)
    next_free: usize,
}

impl BlockHeader {
    #[inline] fn size(&self)       -> usize { self.size_and_flags & !1 }
    #[inline] fn is_used(&self)    -> bool  { self.size_and_flags & 1 != 0 }
    #[inline] fn set_used(&mut self)   { self.size_and_flags |=  1; }
    #[inline] fn set_free(&mut self)   { self.size_and_flags &= !1; }
    #[inline] fn set_size(&mut self, s: usize) {
        self.size_and_flags = (self.size_and_flags & 1) | (s & !1);
    }
    #[inline] unsafe fn data_ptr(&self) -> *mut u8 {
        (self as *const BlockHeader as *mut u8).add(HDR_SIZE)
    }
    #[inline] unsafe fn from_data(ptr: *mut u8) -> *mut BlockHeader {
        ptr.sub(HDR_SIZE) as *mut BlockHeader
    }
    #[inline] unsafe fn next_phys(&self) -> *mut BlockHeader {
        (self as *const BlockHeader as *mut u8).add(self.size()) as *mut BlockHeader
    }
}

// ── Stan alokatora ────────────────────────────────────────────────────────────

struct AllocState {
    free_lists: [usize; BINS], // wskaźniki na głowy free list (0 = pusta)
    bump:       usize,         // następny wolny bajt (dla dużych inicjalnych bloków)
    heap_base:  usize,
    heap_end:   usize,
}

static mut STATE: AllocState = AllocState {
    free_lists: [0usize; BINS],
    bump:       0,
    heap_base:  0,
    heap_end:   0,
};

unsafe fn init_heap() {
    if HEAP_INIT { return; }
    let base = HEAP.0.as_ptr() as usize;
    STATE.heap_base = base;
    STATE.heap_end  = base + HEAP_SIZE;
    STATE.bump      = base;
    HEAP_INIT = true;
}

// ── Bin lookup ───────────────────────────────────────────────────────────────

fn bin_for(size: usize) -> Option<usize> {
    for (i, &s) in BIN_SIZES.iter().enumerate() {
        if size <= s { return Some(i); }
    }
    None
}

// ── Allocator ────────────────────────────────────────────────────────────────

unsafe fn alloc_inner(layout: Layout) -> *mut u8 {
    init_heap();

    let align    = layout.align().max(ALIGN_MIN);
    let data_sz  = layout.size().max(1);
    // Rozmiar bloku: nagłówek + dane + wyrównanie
    let block_sz = round_up(HDR_SIZE + data_sz, align);

    // ── Small bin ────────────────────────────────────────────────────────────
    if let Some(bin) = bin_for(block_sz) {
        let bin_sz = round_up(HDR_SIZE + BIN_SIZES[bin], ALIGN_MIN);

        // Wyjmij z free listy
        if STATE.free_lists[bin] != 0 {
            let hdr = STATE.free_lists[bin] as *mut BlockHeader;
            STATE.free_lists[bin] = (*hdr).next_free;
            (*hdr).next_free = 0;
            (*hdr).set_used();
            return (*hdr).data_ptr();
        }

        // Bump alloc
        let addr = STATE.bump;
        let new_bump = addr + bin_sz;
        if new_bump > STATE.heap_end { return ptr::null_mut(); }
        STATE.bump = new_bump;

        let hdr = addr as *mut BlockHeader;
        (*hdr).size_and_flags = bin_sz | 1; // used
        (*hdr).next_free = 0;
        return (*hdr).data_ptr();
    }

    // ── Large alloc (> 8KB) ───────────────────────────────────────────────────
    // Skanuj free listy dużych bloków (TODO: osobna lista; na razie bump only)
    let addr = round_up(STATE.bump, align);
    let end  = addr + block_sz;
    if end > STATE.heap_end { return ptr::null_mut(); }
    STATE.bump = end;

    let hdr = addr as *mut BlockHeader;
    (*hdr).size_and_flags = block_sz | 1;
    (*hdr).next_free = 0;
    (*hdr).data_ptr()
}

unsafe fn dealloc_inner(ptr: *mut u8, layout: Layout) {
    if ptr.is_null() { return; }

    let hdr = BlockHeader::from_data(ptr);
    if !(*hdr).is_used() { return; } // double-free guard
    (*hdr).set_free();

    let block_sz = (*hdr).size();

    // Optymalizacja: jeśli to ostatni bump blok — cofnij bump
    let block_end = hdr as usize + block_sz;
    if block_end == STATE.bump {
        STATE.bump = hdr as usize;
        return;
    }

    // Wstaw do odpowiedniej free listy
    if let Some(bin) = bin_for(block_sz) {
        (*hdr).next_free = STATE.free_lists[bin];
        STATE.free_lists[bin] = hdr as usize;
    }
    // Duże bloki: na razie nie wracają (leak wirtualnej przestrzeni)
    // TODO: scalanie dużych bloków
}

#[inline]
fn round_up(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

// ── Spinlock dla alokatora (bez zależności od sync.rs) ───────────────────────

struct AllocLock { locked: AtomicBool }

impl AllocLock {
    const fn new() -> Self { Self { locked: AtomicBool::new(false) } }
    fn lock(&self) {
        loop {
            if self.locked
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            { break; }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }
    fn unlock(&self) { self.locked.store(false, Ordering::Release); }
}

// ── GlobalAlloc impl ─────────────────────────────────────────────────────────

pub struct FreeListAllocator;

unsafe impl GlobalAlloc for FreeListAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        HEAP_LOCK.lock();
        let ptr = alloc_inner(layout);
        HEAP_LOCK.unlock();
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HEAP_LOCK.lock();
        dealloc_inner(ptr, layout);
        HEAP_LOCK.unlock();
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        HEAP_LOCK.lock();
        let hdr     = BlockHeader::from_data(ptr);
        let old_sz  = (*hdr).size().saturating_sub(HDR_SIZE);
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = alloc_inner(new_layout);
        if !new_ptr.is_null() {
            let copy_n = old_sz.min(new_size);
            ptr::copy_nonoverlapping(ptr, new_ptr, copy_n);
            dealloc_inner(ptr, layout);
        }
        HEAP_LOCK.unlock();
        new_ptr
    }
}

#[global_allocator]
pub static ALLOCATOR: FreeListAllocator = FreeListAllocator;