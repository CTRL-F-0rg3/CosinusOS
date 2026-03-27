// libcosinus — collections.rs


use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut, Index, IndexMut};
use core::slice;
use crate::alloc_impl::HEAP;

// ── Helpers ──────────────────────────────────────────────────────────────────
pub const SYS_PRINT: usize = 1;
unsafe fn heap_alloc<T>(cap: usize) -> Option<NonNull<T>> {
    if cap == 0 { return Some(NonNull::dangling()); }
    let layout = Layout::array::<T>(cap).ok()?;
    let ptr = HEAP.alloc(layout);
    if ptr.is_null() { None } else { Some(NonNull::new_unchecked(ptr as *mut T)) }
}

unsafe fn heap_dealloc<T>(ptr: NonNull<T>, cap: usize) {
    if cap == 0 { return; }
    let layout = Layout::array::<T>(cap).unwrap_unchecked();
    HEAP.dealloc(ptr.as_ptr() as *mut u8, layout);
}

unsafe fn heap_realloc<T>(ptr: NonNull<T>, old_cap: usize, new_cap: usize) -> Option<NonNull<T>> {
    if new_cap == 0 { heap_dealloc(ptr, old_cap); return Some(NonNull::dangling()); }
    if old_cap == 0 { return heap_alloc(new_cap); }
    let old_layout = Layout::array::<T>(old_cap).ok()?;
    let new_layout = Layout::array::<T>(new_cap).ok()?;
    let new_ptr = HEAP.realloc(ptr.as_ptr() as *mut u8, old_layout, new_layout.size());
    if new_ptr.is_null() { None } else { Some(NonNull::new_unchecked(new_ptr as *mut T)) }
}

// ── CosVec<T> ────────────────────────────────────────────────────────────────

pub struct CosVec<T> {
    ptr: NonNull<T>,
    len: usize,
    cap: usize,
    _pd: PhantomData<T>,
}

unsafe impl<T: Send> Send for CosVec<T> {}
unsafe impl<T: Sync> Sync for CosVec<T> {}

impl<T> CosVec<T> {
    pub const fn new() -> Self {
        Self { ptr: NonNull::dangling(), len: 0, cap: 0, _pd: PhantomData }
    }

    pub fn with_capacity(cap: usize) -> Option<Self> {
        let ptr = unsafe { heap_alloc::<T>(cap)? };
        Some(Self { ptr, len: 0, cap, _pd: PhantomData })
    }

    #[inline] pub fn len(&self)      -> usize { self.len }
    #[inline] pub fn capacity(&self) -> usize { self.cap }
    #[inline] pub fn is_empty(&self) -> bool  { self.len == 0 }

    pub fn as_slice(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    pub fn push(&mut self, val: T) -> Result<(), T> {
        if self.len == self.cap {
            if self.grow().is_none() { return Err(val); }
        }
        unsafe { ptr::write(self.ptr.as_ptr().add(self.len), val); }
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 { return None; }
        self.len -= 1;
        Some(unsafe { ptr::read(self.ptr.as_ptr().add(self.len)) })
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        if i >= self.len { return None; }
        Some(unsafe { &*self.ptr.as_ptr().add(i) })
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i >= self.len { return None; }
        Some(unsafe { &mut *self.ptr.as_ptr().add(i) })
    }

    pub fn clear(&mut self) {
        while self.pop().is_some() {}
    }

    pub fn truncate(&mut self, new_len: usize) {
        while self.len > new_len { self.pop(); }
    }

    pub fn iter(&self) -> slice::Iter<'_, T> { self.as_slice().iter() }
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, T> { self.as_mut_slice().iter_mut() }

    pub fn retain<F: FnMut(&T) -> bool>(&mut self, mut f: F) {
        let mut i = 0;
        while i < self.len {
            if !f(unsafe { &*self.ptr.as_ptr().add(i) }) {
                unsafe { ptr::drop_in_place(self.ptr.as_ptr().add(i)); }
                unsafe { ptr::copy(
                    self.ptr.as_ptr().add(i + 1),
                    self.ptr.as_ptr().add(i),
                    self.len - i - 1,
                ); }
                self.len -= 1;
            } else {
                i += 1;
            }
        }
    }

    fn grow(&mut self) -> Option<()> {
        let new_cap = if self.cap == 0 { 4 } else { self.cap * 2 };
        let new_ptr = unsafe { heap_realloc(self.ptr, self.cap, new_cap)? };
        self.ptr = new_ptr;
        self.cap = new_cap;
        Some(())
    }
}

impl<T> Drop for CosVec<T> {
    fn drop(&mut self) {
        self.clear();
        unsafe { heap_dealloc(self.ptr, self.cap); }
    }
}

impl<T> Deref     for CosVec<T> { type Target = [T]; fn deref(&self) -> &[T] { self.as_slice() } }
impl<T> DerefMut  for CosVec<T> { fn deref_mut(&mut self) -> &mut [T] { self.as_mut_slice() } }
impl<T> Index<usize>    for CosVec<T> { type Output = T; fn index(&self, i: usize) -> &T { &self.as_slice()[i] } }
impl<T> IndexMut<usize> for CosVec<T> { fn index_mut(&mut self, i: usize) -> &mut T { &mut self.as_mut_slice()[i] } }

impl<T: core::fmt::Debug> core::fmt::Debug for CosVec<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.as_slice().fmt(f)
    }
}

// ── CosString ────────────────────────────────────────────────────────────────

pub struct CosString {
    bytes: CosVec<u8>,
}

impl CosString {
    pub const fn new() -> Self { Self { bytes: CosVec::new() } }

    pub fn from_str(s: &str) -> Option<Self> {
        let mut v = CosVec::with_capacity(s.len())?;
        for &b in s.as_bytes() { v.push(b).ok()?; }
        Some(Self { bytes: v })
    }

    pub fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(self.bytes.as_slice()) }
    }

    pub fn len(&self)      -> usize { self.bytes.len() }
    pub fn is_empty(&self) -> bool  { self.bytes.is_empty() }

    pub fn push_str(&mut self, s: &str) -> Result<(), ()> {
        for &b in s.as_bytes() { self.bytes.push(b).map_err(|_| ())?; }
        Ok(())
    }

    pub fn push_char(&mut self, c: char) -> Result<(), ()> {
        let mut enc = [0u8; 4];
        self.push_str(c.encode_utf8(&mut enc))
    }

    pub fn clear(&mut self) { self.bytes.clear(); }

    pub fn as_bytes(&self) -> &[u8] { self.bytes.as_slice() }

    pub fn contains(&self, pat: &str) -> bool {
        let h = self.as_bytes();
        let n = pat.as_bytes();
        if n.is_empty() { return true; }
        if n.len() > h.len() { return false; }
        h.windows(n.len()).any(|w| w == n)
    }

    pub fn starts_with(&self, pat: &str) -> bool {
        self.as_bytes().starts_with(pat.as_bytes())
    }

    pub fn ends_with(&self, pat: &str) -> bool {
        self.as_bytes().ends_with(pat.as_bytes())
    }

    pub fn trim(&self) -> &str { self.as_str().trim() }
}

impl Deref for CosString {
    type Target = str;
    fn deref(&self) -> &str { self.as_str() }
}

impl core::fmt::Write for CosString {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push_str(s).map_err(|_| core::fmt::Error)
    }
}

impl core::fmt::Display for CosString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::fmt::Debug for CosString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "\"{}\"", self.as_str())
    }
}

// ── CosBox<T> ────────────────────────────────────────────────────────────────

pub struct CosBox<T> {
    ptr: NonNull<T>,
    _pd: PhantomData<T>,
}

unsafe impl<T: Send> Send for CosBox<T> {}
unsafe impl<T: Sync> Sync for CosBox<T> {}

impl<T> CosBox<T> {
    pub fn new(val: T) -> Option<Self> {
        let layout = Layout::new::<T>();
        let ptr = unsafe { HEAP.alloc(layout) };
        if ptr.is_null() { return None; }
        let typed = ptr as *mut T;
        unsafe { ptr::write(typed, val); }
        Some(Self { ptr: unsafe { NonNull::new_unchecked(typed) }, _pd: PhantomData })
    }

    pub fn into_inner(self) -> T {
        let val = unsafe { ptr::read(self.ptr.as_ptr()) };
        let layout = Layout::new::<T>();
        unsafe { HEAP.dealloc(self.ptr.as_ptr() as *mut u8, layout); }
        core::mem::forget(self);
        val
    }
}

impl<T> Deref    for CosBox<T> { type Target = T; fn deref(&self) -> &T { unsafe { self.ptr.as_ref() } } }
impl<T> DerefMut for CosBox<T> { fn deref_mut(&mut self) -> &mut T { unsafe { self.ptr.as_mut() } } }

impl<T> Drop for CosBox<T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(self.ptr.as_ptr());
            HEAP.dealloc(self.ptr.as_ptr() as *mut u8, Layout::new::<T>());
        }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for CosBox<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (**self).fmt(f)
    }
}

// ── CosOption<T> — Result-friendly Option wrapper ────────────────────────────
pub fn syscall_print(s: &str) {
    unsafe {
        syscall_1(SYS_PRINT, s.as_ptr() as usize);
    }
}

pub unsafe fn syscall_1(num: usize, arg: usize) {
    core::arch::asm!(
        "syscall",
        in("rax") num,
        in("rdi") arg,
    );
}

pub struct Writer;

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        syscall_print(s);
        Ok(())
    }
}
pub trait IntoResult<T> {
    fn ok_or_nomem(self) -> Result<T, i64>;
}

impl<T> IntoResult<T> for Option<T> {
    #[inline]
    fn ok_or_nomem(self) -> Result<T, i64> {
        self.ok_or(crate::err::NOMEM)
    }
}