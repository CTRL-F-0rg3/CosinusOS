// mod.rs — DevSpace ATA driver main module
//
// Runs in Ring-1. Owns the ForthVM, handles IPC from Ring-3, calls
// critical ASM transfers for actual PIO sector moves.

pub mod api;

// ── Forth sources embedded at compile time ────────────────────────────────────
// The Forth VM in drive.odin interprets these at runtime.
// No separate Forth compiler needed — VM is the compiler.
const DRIVE_DEF_FS:   &[u8] = include_bytes!("drive_def.fs");
const DRIVE_LOGIC_FS: &[u8] = include_bytes!("drive_logic.fs");

/// Load Forth source into the Odin ForthVM via FFI.
/// Called once during AtaDriver::init().
// Forth source is loaded into the VM at runtime by the Odin ForthVM
// (drive.odin). Until Odin is linked, these are no-ops — the direct
// Rust ATA path (ata_read_sector_direct etc.) handles all I/O.
fn forth_load(_src: &[u8]) -> bool { true }


use self::api::{
    DiskRequest, DiskRequestType, DiskResponse,
    ERR_READ, ERR_WRITE, ERR_IDENTIFY, ERR_FLUSH, ERR_UNSUPPORTED,
    DEVSPACE_IPC_BASE, DEVSPACE_IPC_SIZE, IpcRing,
};

// ── FFI — critical ASM (crytic.asm) ──────────────────────────────────────────

// ── Critical transfer routines (inline Rust, same as crytic.asm) ────────────
// When crytic.o is linked these are replaced by the ASM versions.
// For now pure Rust fallback ensures the binary links without crytic.o.

#[inline(always)]
unsafe fn transfer_sector_in(buf: *mut u8, port: u16) {
    // REP INSW: read 256 words from ATA data port into buf
    core::arch::asm!(
        "rep insw",
        in("dx")  port,
        in("rdi") buf,
        inout("ecx") 256u32 => _,
        options(nostack)
    );
}

#[inline(always)]
unsafe fn transfer_sector_out(buf: *const u8, port: u16) {
    // REP OUTSW: write 256 words from buf to ATA data port
    core::arch::asm!(
        "rep outsw",
        in("dx")  port,
        in("rsi") buf,
        inout("ecx") 256u32 => _,
        options(nostack)
    );
}

#[inline(always)]
unsafe fn delay_400ns() {
    // Read alt-status 4× ≈ 400ns per ATA spec
    let _: u8;
    core::arch::asm!("in al, dx", out("al") _, in("dx") 0x3F6u16, options(nostack, nomem));
    core::arch::asm!("in al, dx", out("al") _, in("dx") 0x3F6u16, options(nostack, nomem));
    core::arch::asm!("in al, dx", out("al") _, in("dx") 0x3F6u16, options(nostack, nomem));
    core::arch::asm!("in al, dx", out("al") _, in("dx") 0x3F6u16, options(nostack, nomem));
}

// ── ATA port addresses ────────────────────────────────────────────────────────

const ATA_DATA:      u16 = 0x1F0;
const ATA_ERROR:     u16 = 0x1F1;
const ATA_SEC_COUNT: u16 = 0x1F2;
const ATA_LBA_LO:    u16 = 0x1F3;
const ATA_LBA_MID:   u16 = 0x1F4;
const ATA_LBA_HI:    u16 = 0x1F5;
const ATA_DRIVE_SEL: u16 = 0x1F6;
const ATA_CMD:       u16 = 0x1F7;
const ATA_STATUS:    u16 = 0x1F7;
const ATA_ALT_CTRL:  u16 = 0x3F6;

const CMD_READ_PIO:   u8 = 0x20;
const CMD_WRITE_PIO:  u8 = 0x30;
const CMD_IDENTIFY:   u8 = 0xEC;
const CMD_FLUSH:      u8 = 0xE7;

const SR_BSY:  u8 = 0x80;
const SR_DRQ:  u8 = 0x08;
const SR_ERR:  u8 = 0x01;

const POLL_TIMEOUT: u32 = 1_000_000;

// ── Port I/O (Ring-1 has IOPL≥1, IN/OUT are permitted) ───────────────────────

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nostack, nomem));
    val
}

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nostack, nomem));
}

// ── Poll helpers ─────────────────────────────────────────────────────────────

unsafe fn poll_bsy() -> bool {
    for _ in 0..POLL_TIMEOUT {
        let s = inb(ATA_STATUS);
        if s & SR_BSY == 0 {
            return s & SR_ERR == 0;
        }
    }
    false // timeout
}

unsafe fn poll_drq() -> bool {
    for _ in 0..POLL_TIMEOUT {
        let s = inb(ATA_STATUS);
        if s & SR_BSY != 0 { continue; }
        if s & SR_ERR != 0 { return false; }
        if s & SR_DRQ != 0 { return true; }
    }
    false
}

// ── LBA28 register setup ──────────────────────────────────────────────────────

unsafe fn setup_lba28(lba: u64, drive: u8, count: u8) {
    let drv_bits: u8 = if drive == 0 { 0xE0 } else { 0xF0 };
    let head_bits = ((lba >> 24) & 0x0F) as u8;
    outb(ATA_DRIVE_SEL, drv_bits | head_bits);
    delay_400ns();
    outb(ATA_SEC_COUNT, count);
    outb(ATA_LBA_LO,  (lba & 0xFF) as u8);
    outb(ATA_LBA_MID, ((lba >> 8) & 0xFF) as u8);
    outb(ATA_LBA_HI,  ((lba >> 16) & 0xFF) as u8);
}

// ── AtaDriver ────────────────────────────────────────────────────────────────

pub struct AtaDriver {
    pub active_drive: u8,
    identify_buf: [u8; 512],
    lba28_total: u32,
}

impl AtaDriver {
    pub fn new() -> Self {
        Self {
            active_drive: 0,
            identify_buf: [0u8; 512],
            lba28_total:  0,
        }
    }

    pub fn init(&mut self) -> bool {
        unsafe {
            // Soft reset
            outb(ATA_ALT_CTRL, 0x04);
            delay_400ns();
            outb(ATA_ALT_CTRL, 0x00);
            delay_400ns();
            poll_bsy();

            self.identify()
        }
    }

    unsafe fn identify(&mut self) -> bool {
        let drv = if self.active_drive == 0 { 0xE0u8 } else { 0xF0u8 };
        outb(ATA_DRIVE_SEL, drv);
        delay_400ns();

        // Clear registers
        outb(ATA_SEC_COUNT, 0);
        outb(ATA_LBA_LO, 0);
        outb(ATA_LBA_MID, 0);
        outb(ATA_LBA_HI, 0);
        outb(ATA_CMD, CMD_IDENTIFY);

        if inb(ATA_STATUS) == 0 { return false; } // no drive
        if !poll_drq()          { return false; }

        // Read 256 words into identify_buf using fast ASM transfer
        transfer_sector_in(self.identify_buf.as_mut_ptr(), ATA_DATA);

        // Parse LBA28 sector count from words 60-61
        let lo = u16::from_le_bytes([self.identify_buf[120], self.identify_buf[121]]) as u32;
        let hi = u16::from_le_bytes([self.identify_buf[122], self.identify_buf[123]]) as u32;
        self.lba28_total = (hi << 16) | lo;

        self.lba28_total > 0
    }

    // ── Request dispatch ─────────────────────────────────────────────────────

    pub fn handle_request(&mut self, req: DiskRequest) -> DiskResponse {
        match req.req_type {
            DiskRequestType::Read => {
                let ok = self.read_sectors(req.lba, req.sector_count, req.buffer_phys);
                if ok { DiskResponse::ok(req.req_id) }
                else  { DiskResponse::err(req.req_id, ERR_READ) }
            }
            DiskRequestType::Write => {
                let ok = self.write_sectors(req.lba, req.sector_count, req.buffer_phys);
                if ok { DiskResponse::ok(req.req_id) }
                else  { DiskResponse::err(req.req_id, ERR_WRITE) }
            }
            DiskRequestType::Identify => {
                let ok = unsafe { self.identify() };
                if ok { DiskResponse::ok(req.req_id) }
                else  { DiskResponse::err(req.req_id, ERR_IDENTIFY) }
            }
            DiskRequestType::Flush => {
                let ok = self.flush();
                if ok { DiskResponse::ok(req.req_id) }
                else  { DiskResponse::err(req.req_id, ERR_FLUSH) }
            }
            #[allow(unreachable_patterns)]
            _ => DiskResponse::err(req.req_id, ERR_UNSUPPORTED),
        }
    }

    // ── Read ─────────────────────────────────────────────────────────────────

    fn read_sectors(&mut self, lba: u64, count: u32, dest_phys: u64) -> bool {
        unsafe {
            for i in 0..count {
                let buf = (dest_phys + i as u64 * 512) as *mut u8;

                setup_lba28(lba + i as u64, self.active_drive, 1);
                outb(ATA_CMD, CMD_READ_PIO);

                if !poll_drq() { return false; }

                // Fast REP INSW transfer via crytic.asm
                transfer_sector_in(buf, ATA_DATA);
            }
        }
        true
    }

    // ── Write ────────────────────────────────────────────────────────────────

    fn write_sectors(&mut self, lba: u64, count: u32, src_phys: u64) -> bool {
        unsafe {
            for i in 0..count {
                let buf = (src_phys + i as u64 * 512) as *const u8;

                setup_lba28(lba + i as u64, self.active_drive, 1);
                outb(ATA_CMD, CMD_WRITE_PIO);

                if !poll_drq() { return false; }

                // Fast REP OUTSW transfer via crytic.asm
                transfer_sector_out(buf, ATA_DATA);
            }
            // Flush write cache after all sectors written
            self.flush()
        }
    }

    // ── Flush ────────────────────────────────────────────────────────────────

    fn flush(&self) -> bool {
        unsafe {
            outb(ATA_CMD, CMD_FLUSH);
            poll_bsy()
        }
    }
}