// CosinusOS — root_/policy.rs
// Per-ring capability tables
// Defines what each ring level can do at the resource level,
// independent of syscall numbers.

use crate::syscall_api::nr;

// ── Capability flags ──────────────────────────────────────────────────────────

pub mod cap {
    // Resource capabilities
    pub const IO_ACCESS:      u64 = 1 << 0;   // direct IN/OUT port access
    pub const PHYS_MAP:       u64 = 1 << 1;   // map physical memory directly
    pub const IRQ_CONTROL:    u64 = 1 << 2;   // mask/unmask IRQs
    pub const SPAWN_KERNEL:   u64 = 1 << 3;   // spawn ring-0 kernel threads
    pub const RAW_DISK:       u64 = 1 << 4;   // raw disk I/O (bypass rootfs gate)
    pub const MMIO_MAP:       u64 = 1 << 5;   // map MMIO regions
    pub const DEBUG_SERIAL:   u64 = 1 << 6;   // write to serial debug port
    pub const FRAMEBUFFER:    u64 = 1 << 7;   // direct framebuffer access
    pub const IPC_BROADCAST:  u64 = 1 << 8;   // send IPC to all threads
    pub const THREAD_KILL:    u64 = 1 << 9;   // kill arbitrary threads
    pub const POLICY_ADMIN:   u64 = 1 << 10;  // modify ring policy at runtime
    pub const NET_RAW:        u64 = 1 << 11;  // raw network packet injection
    pub const CLOCK_SET:      u64 = 1 << 12;  // set system clock
    pub const MEM_KERNEL:     u64 = 1 << 13;  // allocate from kernel heap
    pub const EXEC_NATIVE:    u64 = 1 << 14;  // execute native ring-0 code paths

    // Composed sets for convenience
    pub const ALL:            u64 = u64::MAX;
    pub const NONE:           u64 = 0;

    // Standard devspace (ring 1) capabilities
    pub const DEVSPACE_DEFAULT: u64 =
        IO_ACCESS | MMIO_MAP | FRAMEBUFFER | DEBUG_SERIAL | RAW_DISK;

    // Standard userspace (ring 3) capabilities
    pub const USERSPACE_DEFAULT: u64 =
        DEBUG_SERIAL;

    // Kernel thread capabilities
    pub const KERNEL_DEFAULT: u64 = ALL;
}

// ── Per-ring capability record ────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct RingCaps {
    pub ring:          u8,
    pub cap_flags:     u64,         // capability bitmask
    pub syscall_mask:  u64,         // allowed syscall numbers as bitmask
    pub can_io:        bool,
    pub can_phys_map:  bool,
    pub can_irq:       bool,
    pub can_raw_disk:  bool,
    pub max_threads:   u8,          // max threads this ring may spawn
    pub max_mem_pages: u32,         // max pages per MEM_ALLOC call
}

impl RingCaps {
    pub const fn new(
        ring:         u8,
        cap_flags:    u64,
        syscall_mask: u64,
        max_threads:  u8,
        max_mem_pages: u32,
    ) -> Self {
        Self {
            ring,
            cap_flags,
            syscall_mask,
            can_io:        cap_flags & cap::IO_ACCESS     != 0,
            can_phys_map:  cap_flags & cap::PHYS_MAP      != 0,
            can_irq:       cap_flags & cap::IRQ_CONTROL   != 0,
            can_raw_disk:  cap_flags & cap::RAW_DISK      != 0,
            max_threads,
            max_mem_pages,
        }
    }

    #[inline]
    pub fn allows_syscall(&self, nr: u64) -> bool {
        if nr >= 64 { return true; }  // extended syscalls — policy TBD
        self.syscall_mask & (1u64 << nr) != 0
    }

    #[inline]
    pub fn has_cap(&self, flag: u64) -> bool {
        self.cap_flags & flag != 0
    }

    #[inline]
    pub fn allows_mem_alloc(&self, pages: u32) -> bool {
        pages <= self.max_mem_pages
    }
}

// ── Global ring capability table ─────────────────────────────────────────────
// Index = ring level (0, 1, 2, 3)

pub static mut RING_CAPS: [RingCaps; 4] = [
    // Ring 0 — kernel: everything allowed
    RingCaps::new(
        0,
        cap::ALL,
        u64::MAX,
        255,
        4096,
    ),
    // Ring 1 — devspace: I/O, MMIO, disk, framebuffer; no kernel spawn, no IRQ
    RingCaps::new(
        1,
        cap::DEVSPACE_DEFAULT,
        RING1_MASK,
        16,
        256,
    ),
    // Ring 2 — reserved (future use, e.g. trusted services)
    RingCaps::new(
        2,
        cap::NONE,
        0,
        0,
        0,
    ),
    // Ring 3 — userspace: very restricted, no direct hardware
    RingCaps::new(
        3,
        cap::USERSPACE_DEFAULT,
        RING3_MASK,
        8,
        128,
    ),
];

// Syscall masks per ring — which syscall numbers are permitted
const RING1_MASK: u64 =
    (1 << nr::EXIT)       |
    (1 << nr::WRITE)      |
    (1 << nr::READ)       |
    (1 << nr::YIELD)      |
    (1 << nr::SPAWN)      |
    (1 << nr::SLEEP)      |
    (1 << nr::MEM_ALLOC)  |
    (1 << nr::MEM_FREE)   |
    (1 << nr::IPC_SEND)   |
    (1 << nr::IPC_RECV)   |
    (1 << nr::IPC_POLL)   |
    (1 << nr::THREAD_ID)  |
    (1 << nr::TIME);

const RING3_MASK: u64 =
    (1 << nr::EXIT)       |
    (1 << nr::WRITE)      |
    (1 << nr::READ)       |
    (1 << nr::YIELD)      |
    (1 << nr::SLEEP)      |
    (1 << nr::MEM_ALLOC)  |
    (1 << nr::MEM_FREE)   |
    (1 << nr::IPC_SEND)   |
    (1 << nr::IPC_RECV)   |
    (1 << nr::IPC_POLL)   |
    (1 << nr::THREAD_ID)  |
    (1 << nr::TIME)        |
    (1 << nr::DEBUG_PRINT);

// ── Runtime policy modification ───────────────────────────────────────────────
// Only ring 0 threads with POLICY_ADMIN cap may call these.

pub fn init_ring_caps() {
    // Caps are already initialized via const — nothing to do at runtime.
    // This function exists for future dynamic policy loading.
}

/// Grant an additional capability to a ring at runtime.
/// Caller must have POLICY_ADMIN capability.
pub unsafe fn ring_grant_cap(ring: u8, flag: u64) {
    if ring < 4 {
        RING_CAPS[ring as usize].cap_flags |= flag;
        RING_CAPS[ring as usize].can_io       = RING_CAPS[ring as usize].cap_flags & cap::IO_ACCESS    != 0;
        RING_CAPS[ring as usize].can_phys_map = RING_CAPS[ring as usize].cap_flags & cap::PHYS_MAP     != 0;
        RING_CAPS[ring as usize].can_irq      = RING_CAPS[ring as usize].cap_flags & cap::IRQ_CONTROL  != 0;
        RING_CAPS[ring as usize].can_raw_disk = RING_CAPS[ring as usize].cap_flags & cap::RAW_DISK     != 0;
    }
}

/// Revoke a capability from a ring at runtime.
pub unsafe fn ring_revoke_cap(ring: u8, flag: u64) {
    if ring < 4 {
        RING_CAPS[ring as usize].cap_flags &= !flag;
        RING_CAPS[ring as usize].can_io       = RING_CAPS[ring as usize].cap_flags & cap::IO_ACCESS    != 0;
        RING_CAPS[ring as usize].can_phys_map = RING_CAPS[ring as usize].cap_flags & cap::PHYS_MAP     != 0;
        RING_CAPS[ring as usize].can_irq      = RING_CAPS[ring as usize].cap_flags & cap::IRQ_CONTROL  != 0;
        RING_CAPS[ring as usize].can_raw_disk = RING_CAPS[ring as usize].cap_flags & cap::RAW_DISK     != 0;
    }
}

/// Restrict max memory allocation for a ring.
pub unsafe fn ring_set_max_mem(ring: u8, pages: u32) {
    if ring < 4 {
        RING_CAPS[ring as usize].max_mem_pages = pages;
    }
}

/// Get a copy of a ring's capability record.
pub fn ring_caps_of(ring: u8) -> Option<RingCaps> {
    if ring < 4 {
        Some(unsafe { RING_CAPS[ring as usize] })
    } else {
        None
    }
}
