// libcosinus — alloc_impl.rs
// Slab allocator dla userspace: GlobalAlloc oparty na sys_mmap.
//
// Klasy rozmiarów: 8, 16, 32, 64, 128, 256, 512, 1024, 2048 B
// Alokacje > 2048B idą bezpośrednio przez mmap (per-obiekt).
//
// Każda klasa trzyma intrusywną listę wolnych slotów.
// Nowe slaby (strony 4KB) są żądane przez sys_mmap.
// Brak globalnego locka — userspace jest single-threaded per domyśl
// (jeśli potrzebujesz MT, dodaj AtomicBool spinlock w UserHeap).

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;
use crate::mmap_prot;

const PAGE_SIZE: usize = 0x1000;

const SLAB_CLASSES: [usize; 9] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048];
const N_CLASSES:    usize       = SLAB_CLASSES.len();
const MAX_SLAB:     usize       = SLAB_CLASSES[N_CLASSES - 1];

// ── SlabClass ────────────────────────────────────────────────────────────────

struct SlabClass {
    obj_size:  usize,
    free_head: *mut *mut u8,
}

impl SlabClass {
    const fn new(obj_size: usize) -> Self {
        Self { obj_size, free_head: ptr::null_mut() }
    }

    #[inline]
    unsafe fn pop(&mut self) -> *mut u8 {
        if self.free_head.is_null() { return ptr::null_mut(); }
        let slot = self.free_head as *mut u8;
        self.free_head = *(self.free_head as *mut *mut *mut u8).read() as *mut *mut u8;
        slot
    }

    #[inline]
    unsafe fn push(&mut self, ptr: *mut u8) {
        let slot = ptr as *mut *mut u8;
        *slot = self.free_head as *mut u8;
        self.free_head = slot;
    }

    unsafe fn grow(&mut self) -> bool {
        let page = crate::mmap(PAGE_SIZE, mmap_prot::READ | mmap_prot::WRITE);
        if page.is_null() { return false; }
        let slots = PAGE_SIZE / self.obj_size;
        let mut i = slots;
        while i > 0 {
            i -= 1;
            self.push(page.add(i * self.obj_size));
        }
        true
    }

    unsafe fn alloc_slot(&mut self) -> *mut u8 {
        if self.free_head.is_null() && !self.grow() { return ptr::null_mut(); }
        self.pop()
    }
}

// ── UserHeap ─────────────────────────────────────────────────────────────────

struct HeapInner {
    classes: [SlabClass; N_CLASSES],
}

pub struct UserHeap {
    inner: UnsafeCell<HeapInner>,
}

unsafe impl Sync for UserHeap {}
unsafe impl Send for UserHeap {}

impl UserHeap {
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(HeapInner {
                classes: [
                    SlabClass::new(SLAB_CLASSES[0]),
                    SlabClass::new(SLAB_CLASSES[1]),
                    SlabClass::new(SLAB_CLASSES[2]),
                    SlabClass::new(SLAB_CLASSES[3]),
                    SlabClass::new(SLAB_CLASSES[4]),
                    SlabClass::new(SLAB_CLASSES[5]),
                    SlabClass::new(SLAB_CLASSES[6]),
                    SlabClass::new(SLAB_CLASSES[7]),
                    SlabClass::new(SLAB_CLASSES[8]),
                ],
            }),
        }
    }

    fn class_for(size: usize) -> Option<usize> {
        SLAB_CLASSES.iter().position(|&c| size <= c)
    }
}

unsafe impl GlobalAlloc for UserHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size  = layout.size().max(layout.align());
        if size == 0 { return ptr::null_mut(); }

        if let Some(ci) = Self::class_for(size) {
            let inner = &mut *self.inner.get();
            return inner.classes[ci].alloc_slot();
        }

        // Duża alokacja — mmap bezpośrednio
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let ptr = crate::mmap(pages * PAGE_SIZE, mmap_prot::READ | mmap_prot::WRITE);
        if ptr.is_null() { ptr::null_mut() } else { ptr }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() { return; }
        let size = layout.size().max(layout.align());

        if let Some(ci) = Self::class_for(size) {
            let inner = &mut *self.inner.get();
            inner.classes[ci].push(ptr);
            return;
        }

        // Duża alokacja — zwróć przez munmap
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        crate::munmap(ptr, pages * PAGE_SIZE);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
        }
        if new_size == 0 {
            self.dealloc(ptr, layout);
            return ptr::null_mut();
        }
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if new_ptr.is_null() { return ptr::null_mut(); }
        ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
        self.dealloc(ptr, layout);
        new_ptr
    }
}

#[global_allocator]
pub static HEAP: UserHeap = UserHeap::new();
