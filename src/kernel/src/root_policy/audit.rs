// CosinusOS — root_/audit.rs
// Policy violation audit log
// Ring-buffer of recent policy violations — used for diagnostics
// and potential future intrusion detection.

use crate::debug::{serial_print, hex_str, num_str};

pub const MAX_LOG_ENTRIES: usize = 256;

// ── Violation kinds ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ViolationKind {
    None            = 0,
    SyscallDenied   = 1,   // ring-level syscall mask denied
    ThreadRevoked   = 2,   // thread is fully revoked
    ThreadDenied    = 3,   // thread-specific deny list hit
    RingCapDenied   = 4,   // ring capability table denied
    KernelSpawnDenied = 5, // tried to spawn kernel thread from non-ring-0
    IoDenied        = 6,   // I/O port access denied
    PhysMapDenied   = 7,   // physical memory map denied
    IrqDenied       = 8,   // IRQ control denied
    MemLimitExceeded = 9,  // mem alloc over limit
    PolicyAdminDenied = 10, // tried to modify policy without POLICY_ADMIN cap
}

impl ViolationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None              => "none",
            Self::SyscallDenied     => "syscall_denied",
            Self::ThreadRevoked     => "thread_revoked",
            Self::ThreadDenied      => "thread_denied",
            Self::RingCapDenied     => "ring_cap_denied",
            Self::KernelSpawnDenied => "kernel_spawn_denied",
            Self::IoDenied          => "io_denied",
            Self::PhysMapDenied     => "phys_map_denied",
            Self::IrqDenied         => "irq_denied",
            Self::MemLimitExceeded  => "mem_limit_exceeded",
            Self::PolicyAdminDenied => "policy_admin_denied",
        }
    }
}

// ── Log entry ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct LogEntry {
    pub tick:     u64,
    pub kind:     ViolationKind,
    pub ring:     u8,
    pub tid:      u32,
    pub detail:   u64,   // syscall nr, port, or address depending on kind
}

impl LogEntry {
    pub const fn empty() -> Self {
        Self {
            tick:   0,
            kind:   ViolationKind::None,
            ring:   0,
            tid:    0,
            detail: 0,
        }
    }
}

// ── Ring-buffer log ───────────────────────────────────────────────────────────

pub struct PolicyLog {
    entries:    [LogEntry; MAX_LOG_ENTRIES],
    head:       usize,
    total:      u32,
}

impl PolicyLog {
    pub const fn new() -> Self {
        Self {
            entries: [LogEntry::empty(); MAX_LOG_ENTRIES],
            head:    0,
            total:   0,
        }
    }

    pub fn record(
        &mut self,
        kind:   ViolationKind,
        ring:   u8,
        tid:    u32,
        detail: u64,
    ) {
        self.entries[self.head] = LogEntry {
            tick:   unsafe { crate::perm::TICK },
            kind,
            ring,
            tid,
            detail,
        };
        self.head = (self.head + 1) % MAX_LOG_ENTRIES;
        if self.total < u32::MAX {
            self.total += 1;
        }

        // Serial log for all violations
        serial_print("[POLICY] ");
        serial_print(kind.as_str());
        serial_print(" ring=");
        { let mut b = [0u8; 24]; serial_print(num_str(ring as usize, &mut b)); }
        serial_print(" tid=");
        { let mut b = [0u8; 24]; serial_print(num_str(tid as usize, &mut b)); }
        serial_print(" detail=");
        { let mut b = [0u8; 18]; serial_print(hex_str(detail, &mut b)); }
        serial_print("\n");
    }

    pub fn total_violations(&self) -> u32 {
        self.total
    }

    /// Dump the most recent `n` entries to serial output.
    pub fn dump_recent(&self, n: usize) {
        let count = n.min(self.total as usize).min(MAX_LOG_ENTRIES);
        serial_print("[POLICY] === recent violations ===\n");
        for i in 0..count {
            let idx = (self.head + MAX_LOG_ENTRIES - 1 - i) % MAX_LOG_ENTRIES;
            let e = &self.entries[idx];
            if e.kind == ViolationKind::None { continue; }
            serial_print("  [");
            { let mut b = [0u8; 24]; serial_print(num_str(e.tick as usize, &mut b)); }
            serial_print("] ");
            serial_print(e.kind.as_str());
            serial_print(" ring=");
            { let mut b = [0u8; 24]; serial_print(num_str(e.ring as usize, &mut b)); }
            serial_print(" tid=");
            { let mut b = [0u8; 24]; serial_print(num_str(e.tid as usize, &mut b)); }
            serial_print(" detail=");
            { let mut b = [0u8; 18]; serial_print(hex_str(e.detail, &mut b)); }
            serial_print("\n");
        }
        serial_print("[POLICY] total=");
        { let mut b = [0u8; 24]; serial_print(num_str(self.total as usize, &mut b)); }
        serial_print("\n");
    }

    /// Clear the log.
    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() {
            *e = LogEntry::empty();
        }
        self.head  = 0;
        self.total = 0;
    }
}