// CosinusOS Userspace — sync.rs
// SpinLock<T> z guardem RAII + Mutex alias

use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data:   core::cell::UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        Self { locked: AtomicBool::new(false), data: core::cell::UnsafeCell::new(data) }
    }
    pub fn lock(&self) -> SpinLockGuard<T> {
        loop {
            if !self.locked.load(Ordering::Relaxed) {
                if self.locked
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return SpinLockGuard { lock: self };
                }
            }
            unsafe { core::arch::asm!("pause", options(nostack, nomem)); }
        }
    }
    pub fn try_lock(&self) -> Option<SpinLockGuard<T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
    }
}

pub struct SpinLockGuard<'a, T> { lock: &'a SpinLock<T> }

impl<'a, T> core::ops::Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T { unsafe { &*self.lock.data.get() } }
}
impl<'a, T> core::ops::DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T { unsafe { &mut *self.lock.data.get() } }
}
impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) { self.lock.locked.store(false, Ordering::Release); }
}

pub type Mutex<T> = SpinLock<T>;
