// CosinusOS — root_/thread_cap.rs
// Per-thread capability tokens
// Each thread slot has a ThreadCap that can grant or deny specific
// syscalls/resources beyond what the ring-level policy allows.
// This enables fine-grained sandboxing within the same ring.

pub const MAX_THREADS:   usize = 64;
const MAX_DENY_LIST:     usize = 16;   // max syscalls in per-thread deny list

// ── Capability flags (thread-level, same namespace as policy::cap) ─────────────

pub mod tcap {
    // Thread can elevate certain operations if granted by kernel
    pub const ELEVATED_IPC:   u64 = 1 << 0;   // IPC to kernel threads
    pub const ELEVATED_MEM:   u64 = 1 << 1;   // larger mem alloc limit
    pub const TRUST_SPAWN:    u64 = 1 << 2;   // allowed to spawn subthreads
    pub const RESTRICT_WRITE: u64 = 1 << 3;   // write syscall limited to serial
    pub const SANDBOX_STRICT: u64 = 1 << 4;   // enforce strict deny list
    pub const REVOKED:        u64 = 1 << 63;  // thread fully revoked

    pub const DEFAULT_KERNEL: u64 = ELEVATED_IPC | ELEVATED_MEM | TRUST_SPAWN;
    pub const DEFAULT_USER:   u64 = TRUST_SPAWN;
    pub const DEFAULT_DEV:    u64 = ELEVATED_IPC | TRUST_SPAWN;
}

// ── Per-thread capability token ───────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct ThreadCap {
    pub flags:     u64,                    // capability flags
    pub deny_list: [u64; MAX_DENY_LIST],   // syscall numbers to deny
    pub deny_count: u8,
    pub owner_ring: u8,                    // ring this thread was spawned at
    pub max_mem_pages: u32,               // thread-specific mem limit (0 = use ring default)
}

impl ThreadCap {
    pub const fn new_empty() -> Self {
        Self {
            flags:         0,
            deny_list:     [u64::MAX; MAX_DENY_LIST],
            deny_count:    0,
            owner_ring:    3,
            max_mem_pages: 0,
        }
    }

    pub const fn new_kernel() -> Self {
        Self {
            flags:         tcap::DEFAULT_KERNEL,
            deny_list:     [u64::MAX; MAX_DENY_LIST],
            deny_count:    0,
            owner_ring:    0,
            max_mem_pages: 0,
        }
    }

    pub const fn new_user() -> Self {
        Self {
            flags:         tcap::DEFAULT_USER,
            deny_list:     [u64::MAX; MAX_DENY_LIST],
            deny_count:    0,
            owner_ring:    3,
            max_mem_pages: 128,
        }
    }

    pub const fn new_devspace() -> Self {
        Self {
            flags:         tcap::DEFAULT_DEV,
            deny_list:     [u64::MAX; MAX_DENY_LIST],
            deny_count:    0,
            owner_ring:    1,
            max_mem_pages: 256,
        }
    }

    #[inline]
    pub fn is_revoked(&self) -> bool {
        self.flags & tcap::REVOKED != 0
    }

    #[inline]
    pub fn has(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }

    #[inline]
    pub fn denies(&self, syscall_nr: u64) -> bool {
        if self.deny_count == 0 {
            return false;
        }
        for i in 0..self.deny_count as usize {
            if self.deny_list[i] == syscall_nr {
                return true;
            }
        }
        false
    }

    pub fn grant(&mut self, flag: u64) {
        // Cannot grant to a revoked thread
        if self.is_revoked() { return; }
        self.flags |= flag;
    }

    pub fn revoke(&mut self, flag: u64) {
        self.flags &= !flag;
    }

    pub fn revoke_all(&mut self) {
        self.flags = tcap::REVOKED;
        self.deny_count = MAX_DENY_LIST as u8;
        for i in 0..MAX_DENY_LIST {
            self.deny_list[i] = i as u64;  // deny all known syscalls
        }
    }

    pub fn add_deny(&mut self, syscall_nr: u64) {
        if self.deny_count as usize >= MAX_DENY_LIST {
            return;
        }
        // Don't add duplicates
        for i in 0..self.deny_count as usize {
            if self.deny_list[i] == syscall_nr {
                return;
            }
        }
        self.deny_list[self.deny_count as usize] = syscall_nr;
        self.deny_count += 1;
    }

    pub fn remove_deny(&mut self, syscall_nr: u64) {
        let mut found = MAX_DENY_LIST;
        for i in 0..self.deny_count as usize {
            if self.deny_list[i] == syscall_nr {
                found = i;
                break;
            }
        }
        if found < MAX_DENY_LIST && self.deny_count > 0 {
            // Swap with last and decrement
            let last = self.deny_count as usize - 1;
            self.deny_list[found] = self.deny_list[last];
            self.deny_list[last] = u64::MAX;
            self.deny_count -= 1;
        }
    }

    /// Check if this thread is allowed to allocate `pages` pages.
    /// Returns true if within thread-specific limit (or limit is 0 = defer to ring).
    pub fn allows_mem_alloc(&self, pages: u32) -> bool {
        if self.max_mem_pages == 0 {
            return true;  // defer to ring policy
        }
        pages <= self.max_mem_pages
    }

    /// Set a custom memory limit for this thread (pages).
    pub fn set_mem_limit(&mut self, pages: u32) {
        self.max_mem_pages = pages;
    }
}

// ── Global capability table ───────────────────────────────────────────────────

pub static mut CAP_TABLE: [ThreadCap; MAX_THREADS] = [ThreadCap::new_empty(); MAX_THREADS];

pub fn init_cap_table() {
    unsafe {
        // Slot 0 = idle thread (kernel, full caps)
        CAP_TABLE[0] = ThreadCap::new_kernel();
        // Slot 1 = kterminal (kernel thread)
        CAP_TABLE[1] = ThreadCap::new_kernel();
        // Slot 2 = first userspace thread
        CAP_TABLE[2] = ThreadCap::new_user();
        // Remaining slots start as empty until a thread is spawned
        for i in 3..MAX_THREADS {
            CAP_TABLE[i] = ThreadCap::new_empty();
        }
    }
}

/// Called by spawn_k / spawn_user_on_cr3 to initialize a new thread's caps.
pub unsafe fn init_thread_cap(tid: usize, ring: u8) {
    if tid >= MAX_THREADS { return; }
    CAP_TABLE[tid] = match ring {
        0 => ThreadCap::new_kernel(),
        1 => ThreadCap::new_devspace(),
        3 => ThreadCap::new_user(),
        _ => ThreadCap::new_empty(),
    };
}

/// Called when a thread exits — clear its capability slot.
pub unsafe fn clear_thread_cap(tid: usize) {
    if tid >= MAX_THREADS { return; }
    CAP_TABLE[tid] = ThreadCap::new_empty();
}

/// Query how much memory a thread is allowed to allocate.
/// Returns (thread_limit, ring_limit) — caller takes the minimum.
pub unsafe fn mem_limit(tid: usize, ring: u8) -> u32 {
    if tid >= MAX_THREADS { return 0; }
    let thread_limit = CAP_TABLE[tid].max_mem_pages;
    let ring_limit   = super::policy::RING_CAPS[ring as usize].max_mem_pages;
    if thread_limit == 0 {
        ring_limit
    } else {
        thread_limit.min(ring_limit)
    }
}
