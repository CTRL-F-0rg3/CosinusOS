// CosinusOS — allocator/buddy.rs
// Buddy allocator: bloki 4KB–2MB (order 0..9), intrusywna free-lista, bitmap coalesce.
// Nie thread-safe — spinlock zapewnia KernelHeap.

use core::ptr;

pub const PAGE_SIZE:       usize = 0x1000;
pub const MAX_ORDER:       usize = 10;
pub const MAX_BLOCK:       usize = PAGE_SIZE << (MAX_ORDER - 1); // 2MB
pub const BUDDY_HEAP_SIZE: usize = 32 * 1024 * 1024;            // 32MB

const N_PAGES:      usize = BUDDY_HEAP_SIZE / PAGE_SIZE;
const BITMAP_WORDS: usize = N_PAGES / 64 + 1;

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

pub struct BuddyAllocator {
    base:           usize,
    size:           usize,
    free_lists:     [*mut FreeNode; MAX_ORDER],
    bitmap:         [u64; BITMAP_WORDS],
    pub free_bytes: usize,
}

// SAFETY: dostęp wyłącznie przez KernelHeap za spinlockiem
unsafe impl Sync for BuddyAllocator {}
unsafe impl Send for BuddyAllocator {}

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

    pub unsafe fn init(&mut self, base: usize, size: usize) {
        self.base       = base;
        self.size       = size;
        for i in 0..MAX_ORDER    { self.free_lists[i] = ptr::null_mut(); }
        for i in 0..BITMAP_WORDS { self.bitmap[i] = 0; }
        self.free_bytes = 0;

        let mut offset = 0usize;
        while offset + MAX_BLOCK <= size {
            self.free_push(base + offset, MAX_ORDER - 1);
            offset += MAX_BLOCK;
        }
        let mut ord = MAX_ORDER - 2;
        while offset < size {
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

    pub unsafe fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        if size == 0 { return ptr::null_mut(); }
        let order = self.size_to_order(size.max(align));
        if order >= MAX_ORDER { return ptr::null_mut(); }
        self.alloc_order(order)
    }

    pub unsafe fn free(&mut self, ptr: *mut u8, size: usize) {
        if ptr.is_null() || size == 0 { return; }
        let order = self.size_to_order(size);
        if order >= MAX_ORDER { return; }
        self.free_coalesce(ptr as usize, order);
    }

    #[inline]
    fn size_to_order(&self, size: usize) -> usize {
        let mut order = 0usize;
        let mut block = PAGE_SIZE;
        while block < size && order < MAX_ORDER - 1 { block <<= 1; order += 1; }
        order
    }

    #[inline] fn block_size(order: usize) -> usize { PAGE_SIZE << order }
    #[inline] fn buddy_of(&self, addr: usize, order: usize) -> usize {
        self.base + ((addr - self.base) ^ Self::block_size(order))
    }
    #[inline] fn page_idx(&self, addr: usize) -> usize { (addr - self.base) / PAGE_SIZE }

    unsafe fn bitmap_set_free(&mut self, addr: usize, order: usize) {
        let start = self.page_idx(addr);
        for i in start..start + (1 << order) {
            if i / 64 < BITMAP_WORDS { self.bitmap[i / 64] |= 1u64 << (i % 64); }
        }
    }

    unsafe fn bitmap_set_used(&mut self, addr: usize, order: usize) {
        let start = self.page_idx(addr);
        for i in start..start + (1 << order) {
            if i / 64 < BITMAP_WORDS { self.bitmap[i / 64] &= !(1u64 << (i % 64)); }
        }
    }

    fn bitmap_is_free(&self, addr: usize, order: usize) -> bool {
        let start = self.page_idx(addr);
        for i in start..start + (1 << order) {
            if i / 64 >= BITMAP_WORDS { return false; }
            if self.bitmap[i / 64] & (1u64 << (i % 64)) == 0 { return false; }
        }
        true
    }

    unsafe fn free_push(&mut self, addr: usize, order: usize) {
        let node = FreeNode::init(addr as *mut u8);
        (*node).next = self.free_lists[order];
        (*node).prev = ptr::null_mut();
        if !self.free_lists[order].is_null() { (*self.free_lists[order]).prev = node; }
        self.free_lists[order] = node;
        self.bitmap_set_free(addr, order);
        self.free_bytes += Self::block_size(order);
    }

    unsafe fn free_pop(&mut self, order: usize) -> usize {
        if self.free_lists[order].is_null() { return 0; }
        let node = self.free_lists[order];
        self.free_lists[order] = (*node).next;
        if !(*node).next.is_null() { (*(*node).next).prev = ptr::null_mut(); }
        let addr = node as usize;
        self.bitmap_set_used(addr, order);
        self.free_bytes -= Self::block_size(order);
        addr
    }

    unsafe fn free_remove(&mut self, addr: usize, order: usize) {
        let node = addr as *mut FreeNode;
        if !(*node).prev.is_null() {
            (*(*node).prev).next = (*node).next;
        } else {
            self.free_lists[order] = (*node).next;
        }
        if !(*node).next.is_null() { (*(*node).next).prev = (*node).prev; }
        self.bitmap_set_used(addr, order);
        self.free_bytes -= Self::block_size(order);
    }

    unsafe fn alloc_order(&mut self, order: usize) -> *mut u8 {
        let mut found = MAX_ORDER;
        for o in order..MAX_ORDER {
            if !self.free_lists[o].is_null() { found = o; break; }
        }
        if found == MAX_ORDER { return ptr::null_mut(); }

        let addr = self.free_pop(found);
        if addr == 0 { return ptr::null_mut(); }

        let mut cur = found;
        while cur > order {
            cur -= 1;
            self.free_push(addr + Self::block_size(cur), cur);
        }
        addr as *mut u8
    }

    unsafe fn free_coalesce(&mut self, mut addr: usize, mut order: usize) {
        while order < MAX_ORDER - 1 {
            let buddy = self.buddy_of(addr, order);
            if buddy < self.base || buddy >= self.base + self.size { break; }
            if !self.bitmap_is_free(buddy, order) { break; }
            self.free_remove(buddy, order);
            if buddy < addr { addr = buddy; }
            order += 1;
        }
        self.free_push(addr, order);
    }

    pub fn free_kb(&self)  -> usize { self.free_bytes / 1024 }
    pub fn total_kb(&self) -> usize { self.size / 1024 }
    pub fn used_kb(&self)  -> usize { self.total_kb() - self.free_kb() }
}