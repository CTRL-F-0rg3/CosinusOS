// api.rs — Public interface for the DevSpace ATA block driver
//
// Used by Ring-3 (VFS / filesystem server) to issue disk I/O requests.
// Requests travel through an IPC queue to the Ring-1 driver thread.

// ── Request / Response types ──────────────────────────────────────────────────

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DiskRequestType {
    Read     = 1,
    Write    = 2,
    Identify = 3,
    Flush    = 4,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DiskRequest {
    pub req_id:       u64,
    pub req_type:     DiskRequestType,
    pub lba:          u64,
    pub sector_count: u32,
    /// Physical address of the data buffer.
    /// Ring-1 maps this directly; Ring-3 callers must ensure it stays valid.
    pub buffer_phys:  u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DiskResponse {
    pub req_id: u64,
    /// 0 = success, negative = error code (see ERR_* constants below)
    pub status: i32,
}

impl DiskResponse {
    pub fn ok(req_id: u64) -> Self {
        Self { req_id, status: 0 }
    }
    pub fn err(req_id: u64, code: i32) -> Self {
        Self { req_id, status: code }
    }
    pub fn is_ok(&self) -> bool { self.status == 0 }
}

// ── Error codes ───────────────────────────────────────────────────────────────

pub const ERR_READ:      i32 = -1;
pub const ERR_WRITE:     i32 = -2;
pub const ERR_IDENTIFY:  i32 = -3;
pub const ERR_FLUSH:     i32 = -4;
pub const ERR_UNSUPPORTED: i32 = -255;

// ── IPC queue address (shared between Ring-1 driver and Ring-3 callers) ───────
// Kernel maps this region into both address spaces at startup.

pub const DEVSPACE_IPC_BASE: usize = 0x0000_6000_0000_0000;
pub const DEVSPACE_IPC_SIZE: usize = 4096;

/// Ring index in the shared IPC ring buffer
#[repr(C)]
pub struct IpcRing {
    pub write_idx: u32,
    pub read_idx:  u32,
    pub _pad:      [u8; 8],
    pub slots:     [DiskRequest; 60], // 60 × ~48B fits in 4KB
}

// ── send_disk_command ─────────────────────────────────────────────────────────
// Ring-3 callers use this. Writes to the shared IPC ring and waits for a
// response in the response ring (second page after IPC_BASE).

pub fn send_disk_command(req: DiskRequest) -> DiskResponse {
    unsafe {
        let ring = &mut *(DEVSPACE_IPC_BASE as *mut IpcRing);
        let resp_ring = (DEVSPACE_IPC_BASE + DEVSPACE_IPC_SIZE) as *const DiskResponse;

        // Write request into next slot
        let wi = (ring.write_idx as usize) % 60;
        ring.slots[wi] = req;
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        ring.write_idx = ring.write_idx.wrapping_add(1);

        // Spin waiting for response with matching req_id
        // In a real scheduler this would block on an IPC semaphore
        let deadline = 10_000_000u64;
        let mut spins = 0u64;
        loop {
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            let resp = *resp_ring;
            if resp.req_id == req.req_id {
                return resp;
            }
            spins += 1;
            if spins > deadline {
                return DiskResponse::err(req.req_id, ERR_UNSUPPORTED);
            }
            core::hint::spin_loop();
        }
    }
}