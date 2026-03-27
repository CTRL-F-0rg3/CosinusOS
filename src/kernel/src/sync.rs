// CosinusOS — sync.rs


use core::sync::atomic::{AtomicBool, Ordering};

pub struct Spinlock {
    pub locked: AtomicBool,
}

impl Spinlock {
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }
    pub fn lock(&self) {
        while self.locked.swap(true, Ordering::Acquire) {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}