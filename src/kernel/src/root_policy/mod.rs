// CosinusOS — root_/mod.rs
// Ring policy engine — governs what each privilege level can do
// Sits as a gate before syscall_dispatch_v2 and any kernel resource access
//
// Architecture:
//   ring_policy/     <- this module
//   ├── mod.rs       <- public API, policy gate, global state
//   ├── policy.rs    <- per-ring capability tables
//   ├── thread_cap.rs <- per-thread capability tokens
//   └── audit.rs     <- policy violation log

pub mod policy;
pub mod thread_cap;
pub mod audit;

use policy::{RingCaps, RING_CAPS};
use thread_cap::{ThreadCap, CAP_TABLE};
use audit::PolicyLog;

use crate::syscall_api::err;
use crate::threading::{THREADS, CUR};
use core::sync::atomic::Ordering;

// ── Global policy state ───────────────────────────────────────────────────────

pub static mut POLICY_LOG:    PolicyLog   = PolicyLog::new();
pub static mut POLICY_ACTIVE: bool        = false;

// ── Syscall permission masks per ring ────────────────────────────────────────
// Each bit = one syscall number (nr::*). Rings 0-3.
// Ring 0 (kernel threads): all syscalls
// Ring 1 (devspace):       subset — no spawn kernel, no raw mem
// Ring 3 (userspace):      further restricted

const RING0_SYSCALL_MASK: u64 = u64::MAX;  // kernel: unrestricted

const RING1_SYSCALL_MASK: u64 =            // devspace: no kernel spawn, no debug
    (1 << crate::syscall_api::nr::EXIT)       |
    (1 << crate::syscall_api::nr::WRITE)      |
    (1 << crate::syscall_api::nr::READ)       |
    (1 << crate::syscall_api::nr::YIELD)      |
    (1 << crate::syscall_api::nr::SPAWN)      |
    (1 << crate::syscall_api::nr::SLEEP)      |
    (1 << crate::syscall_api::nr::MEM_ALLOC)  |
    (1 << crate::syscall_api::nr::MEM_FREE)   |
    (1 << crate::syscall_api::nr::IPC_SEND)   |
    (1 << crate::syscall_api::nr::IPC_RECV)   |
    (1 << crate::syscall_api::nr::IPC_POLL)   |
    (1 << crate::syscall_api::nr::THREAD_ID)  |
    (1 << crate::syscall_api::nr::TIME);

const RING3_SYSCALL_MASK: u64 =            // userspace: no raw mem operations
    (1 << crate::syscall_api::nr::EXIT)       |
    (1 << crate::syscall_api::nr::WRITE)      |
    (1 << crate::syscall_api::nr::READ)       |
    (1 << crate::syscall_api::nr::YIELD)      |
    (1 << crate::syscall_api::nr::SLEEP)      |
    (1 << crate::syscall_api::nr::MEM_ALLOC)  |
    (1 << crate::syscall_api::nr::MEM_FREE)   |
    (1 << crate::syscall_api::nr::IPC_SEND)   |
    (1 << crate::syscall_api::nr::IPC_RECV)   |
    (1 << crate::syscall_api::nr::IPC_POLL)   |
    (1 << crate::syscall_api::nr::THREAD_ID)  |
    (1 << crate::syscall_api::nr::TIME)        |
    (1 << crate::syscall_api::nr::DEBUG_PRINT);

// ── Init ─────────────────────────────────────────────────────────────────────

pub fn init() {
    unsafe {
        POLICY_LOG    = PolicyLog::new();
        POLICY_ACTIVE = true;
        policy::init_ring_caps();
        thread_cap::init_cap_table();
        crate::debug::log_ok("RingPolicy", true);
    }
}

// ── Primary gate — called from syscall_dispatch_v2 ───────────────────────────
//
// Returns Ok(()) if the syscall is permitted for the calling thread.
// Returns Err(i64) with the error code to return to userspace.

pub unsafe fn check_syscall(
    syscall_nr: u64,
    cs:         u64,   // CS register — encodes ring level
) -> Result<(), i64> {
    if !POLICY_ACTIVE {
        return Ok(());
    }

    let ring = (cs & 3) as u8;  // CPL from CS selector
    let tid  = CUR.load(Ordering::Relaxed);

    // 1. Ring-level syscall mask check
    let mask = ring_syscall_mask(ring);
    if syscall_nr < 64 && (mask & (1u64 << syscall_nr)) == 0 {
        POLICY_LOG.record(
            audit::ViolationKind::SyscallDenied,
            ring, tid as u32, syscall_nr,
        );
        return Err(err::PERM);
    }

    // 2. Per-thread capability check
    let cap = CAP_TABLE[tid];
    if cap.is_revoked() {
        POLICY_LOG.record(
            audit::ViolationKind::ThreadRevoked,
            ring, tid as u32, syscall_nr,
        );
        return Err(err::PERM);
    }

    // 3. Thread-specific syscall deny list
    if cap.denies(syscall_nr) {
        POLICY_LOG.record(
            audit::ViolationKind::ThreadDenied,
            ring, tid as u32, syscall_nr,
        );
        return Err(err::PERM);
    }

    // 4. Ring capability resource check
    let ring_cap = &RING_CAPS[ring as usize];
    if !ring_cap.allows_syscall(syscall_nr) {
        POLICY_LOG.record(
            audit::ViolationKind::RingCapDenied,
            ring, tid as u32, syscall_nr,
        );
        return Err(err::PERM);
    }

    Ok(())
}

// ── Resource access gates ────────────────────────────────────────────────────
// These are called from individual syscall handlers for sensitive ops.

/// Check if the calling thread may spawn a new kernel thread.
pub unsafe fn check_spawn_kernel(cs: u64) -> Result<(), i64> {
    let ring = (cs & 3) as u8;
    if ring != 0 {
        POLICY_LOG.record(
            audit::ViolationKind::KernelSpawnDenied,
            ring, CUR.load(Ordering::Relaxed) as u32, 0,
        );
        return Err(err::PERM);
    }
    Ok(())
}

/// Check if the calling thread may access I/O ports directly.
pub unsafe fn check_io_access(cs: u64, port: u16) -> Result<(), i64> {
    let ring = (cs & 3) as u8;
    let ring_cap = &RING_CAPS[ring as usize];
    if !ring_cap.can_io {
        POLICY_LOG.record(
            audit::ViolationKind::IoDenied,
            ring, CUR.load(Ordering::Relaxed) as u32, port as u64,
        );
        return Err(err::PERM);
    }
    Ok(())
}

/// Check if the calling thread may map physical memory directly.
pub unsafe fn check_phys_map(cs: u64, phys: u64) -> Result<(), i64> {
    let ring = (cs & 3) as u8;
    let ring_cap = &RING_CAPS[ring as usize];
    if !ring_cap.can_phys_map {
        POLICY_LOG.record(
            audit::ViolationKind::PhysMapDenied,
            ring, CUR.load(Ordering::Relaxed) as u32, phys,
        );
        return Err(err::PERM);
    }
    Ok(())
}

/// Check if the calling thread may modify IRQ masks.
pub unsafe fn check_irq_control(cs: u64) -> Result<(), i64> {
    let ring = (cs & 3) as u8;
    if ring != 0 {
        POLICY_LOG.record(
            audit::ViolationKind::IrqDenied,
            ring, CUR.load(Ordering::Relaxed) as u32, 0,
        );
        return Err(err::PERM);
    }
    Ok(())
}

// ── Thread capability management ─────────────────────────────────────────────

/// Grant a thread additional capability. Called by kernel when spawning.
pub unsafe fn grant_cap(tid: usize, cap_flag: u64) {
    if tid < thread_cap::MAX_THREADS {
        CAP_TABLE[tid].grant(cap_flag);
    }
}

/// Revoke a specific capability from a thread.
pub unsafe fn revoke_cap(tid: usize, cap_flag: u64) {
    if tid < thread_cap::MAX_THREADS {
        CAP_TABLE[tid].revoke(cap_flag);
    }
}

/// Fully revoke a thread — it can no longer make syscalls.
pub unsafe fn revoke_thread(tid: usize) {
    if tid < thread_cap::MAX_THREADS {
        CAP_TABLE[tid].revoke_all();
    }
}

/// Add a syscall to a thread's deny list.
pub unsafe fn deny_syscall(tid: usize, nr: u64) {
    if tid < thread_cap::MAX_THREADS {
        CAP_TABLE[tid].add_deny(nr);
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

fn ring_syscall_mask(ring: u8) -> u64 {
    match ring {
        0 => RING0_SYSCALL_MASK,
        1 => RING1_SYSCALL_MASK,
        3 => RING3_SYSCALL_MASK,
        _ => 0,  // unknown ring: deny everything
    }
}

// ── Diagnostics ──────────────────────────────────────────────────────────────

pub fn violation_count() -> u32 {
    unsafe { POLICY_LOG.total_violations() }
}

pub fn dump_recent(n: usize) {
    unsafe { POLICY_LOG.dump_recent(n); }
}
