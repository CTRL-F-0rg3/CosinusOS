// CosinusOS — allocator/slab.rs
// Slab allocator: klasy 8/16/32/64/128/256/512B, intrusywna free-lista, O(1) alloc/free.
// Nie thread-safe — spinlock zapewnia KernelHeap.

use core::ptr;

const PAGE_SIZE: usize = 0x1000;

pub const SLAB_CLASSES: [usize; 7] = [8, 16, 32, 64, 128, 256, 512];
pub const N_CLASSES:    usize       = SLAB_CLASSES.len();
pub const MAX_SLAB_SIZE: usize      = SLAB_CLASSES[N_CLASSES - 1];

pub type PageAlloc = unsafe fn() -> *mut u8;
pub type PageFree  = unsafe fn(*mut u8);

struct SlabClass {
    obj_size:   usize,
    free_head:  *mut *mut u8,
    free_count: usize,
    slab_count: usize,
}

// SAFETY: dostęp wyłącznie przez KernelHeap za spinlockiem
unsafe impl Sync for SlabClass {}
unsafe impl Send for SlabClass {}

impl SlabClass {
    const fn new(obj_size: usize) -> Self {
        Self { obj_size, free_head: ptr::null_mut(), free_count: 0, slab_count: 0 }
    }

    #[inline]
    unsafe fn pop(&mut self) -> *mut u8 {
        if self.free_head.is_null() { return ptr::null_mut(); }
        let slot = self.free_head as *mut u8;
        self.free_head = *(self.free_head as *mut *mut *mut u8).read() as *mut *mut u8;
        self.free_count -= 1;
        slot
    }

    #[inline]
    unsafe fn push(&mut self, ptr: *mut u8) {
        let slot = ptr as *mut *mut u8;
        *slot = self.free_head as *mut u8;
        self.free_head = slot;
        self.free_count += 1;
    }

    unsafe fn populate_from_page(&mut self, page: *mut u8) {
        let slots = PAGE_SIZE / self.obj_size;
        let mut i = slots;
        while i > 0 {
            i -= 1;
            self.push(page.add(i * self.obj_size));
        }
        self.slab_count += 1;
    }
}

pub struct SlabAllocator {
    classes:    [SlabClass; N_CLASSES],
    page_alloc: Option<PageAlloc>,
    page_free:  Option<PageFree>,
}

// SAFETY: dostęp wyłącznie przez KernelHeap za spinlockiem
unsafe impl Sync for SlabAllocator {}
unsafe impl Send for SlabAllocator {}

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

    #[inline]
    pub fn class_for(size: usize) -> Option<usize> {
        for (i, &cls) in SLAB_CLASSES.iter().enumerate() {
            if size <= cls { return Some(i); }
        }
        None
    }

    pub unsafe fn alloc(&mut self, size: usize) -> *mut u8 {
        let Some(ci) = Self::class_for(size) else { return ptr::null_mut(); };
        let cls = &mut self.classes[ci];
        if !cls.free_head.is_null() { return cls.pop(); }

        let page_fn = match self.page_alloc {
            Some(f) => f,
            None    => return ptr::null_mut(),
        };
        let page = page_fn();
        if page.is_null() { return ptr::null_mut(); }
        cls.populate_from_page(page);
        cls.pop()
    }

    pub unsafe fn free(&mut self, ptr: *mut u8, size: usize) {
        if ptr.is_null() { return; }
        let Some(ci) = Self::class_for(size) else { return; };
        self.classes[ci].push(ptr);
    }

    pub fn free_slots(&self, class: usize) -> usize {
        if class >= N_CLASSES { return 0; }
        self.classes[class].free_count
    }

    pub fn slab_pages(&self, class: usize) -> usize {
        if class >= N_CLASSES { return 0; }
        self.classes[class].slab_count
    }

    pub fn total_slab_kb(&self) -> usize {
        self.classes.iter().map(|c| c.slab_count).sum::<usize>() * PAGE_SIZE / 1024
    }
}