// CosinusOS root_file_system/ata.rs
// ATA PIO driver — ring 0, bare metal
// Features: LBA28 + LBA48, read/write/flush, retry logic,
//           security gate integration via glue.c FFI

#![allow(dead_code)]

use core::arch::asm;

// ── Port addresses ────────────────────────────────────────────────────────────
// Primary ATA bus
const ATA0_DATA:    u16 = 0x1F0;
const ATA0_ERR:     u16 = 0x1F1;  // read: error, write: features
const ATA0_SECTORS: u16 = 0x1F2;
const ATA0_LBA_LO:  u16 = 0x1F3;
const ATA0_LBA_MID: u16 = 0x1F4;
const ATA0_LBA_HI:  u16 = 0x1F5;
const ATA0_DRIVE:   u16 = 0x1F6;
const ATA0_STATUS:  u16 = 0x1F7;  // read: status, write: command
const ATA0_CTRL:    u16 = 0x3F6;  // device control / alt status

// Secondary ATA bus
const ATA1_DATA:    u16 = 0x170;
const ATA1_ERR:     u16 = 0x171;
const ATA1_SECTORS: u16 = 0x172;
const ATA1_LBA_LO:  u16 = 0x173;
const ATA1_LBA_MID: u16 = 0x174;
const ATA1_LBA_HI:  u16 = 0x175;
const ATA1_DRIVE:   u16 = 0x176;
const ATA1_STATUS:  u16 = 0x177;
const ATA1_CTRL:    u16 = 0x376;

// Status register bits
const STATUS_ERR:  u8 = 0x01;  // error
const STATUS_IDX:  u8 = 0x02;  // index (always 0)
const STATUS_CORR: u8 = 0x04;  // corrected data (always 0)
const STATUS_DRQ:  u8 = 0x08;  // data request — data ready to transfer
const STATUS_SRV:  u8 = 0x10;  // overlapped mode service request
const STATUS_DF:   u8 = 0x20;  // drive fault (does not set ERR)
const STATUS_RDY:  u8 = 0x40;  // drive ready
const STATUS_BSY:  u8 = 0x80;  // busy — no other bits valid

// Error register bits
const ERR_AMNF:  u8 = 0x01;  // address mark not found
const ERR_TKZNF: u8 = 0x02;  // track 0 not found
const ERR_ABRT:  u8 = 0x04;  // aborted command
const ERR_MCR:   u8 = 0x08;  // media change request
const ERR_IDNF:  u8 = 0x10;  // ID not found
const ERR_MC:    u8 = 0x20;  // media changed
const ERR_UNC:   u8 = 0x40;  // uncorrectable data error
const ERR_BBK:   u8 = 0x80;  // bad block

// Drive register bits
const DRIVE_MASTER: u8 = 0xA0;  // select master drive
const DRIVE_SLAVE:  u8 = 0xB0;  // select slave drive
const DRIVE_LBA:    u8 = 0x40;  // LBA mode

// Commands
const CMD_READ_PIO:    u8 = 0x20;  // LBA28 read
const CMD_WRITE_PIO:   u8 = 0x30;  // LBA28 write
const CMD_READ_EXT:    u8 = 0x24;  // LBA48 read
const CMD_WRITE_EXT:   u8 = 0x34;  // LBA48 write
const CMD_FLUSH:       u8 = 0xE7;  // flush write cache
const CMD_FLUSH_EXT:   u8 = 0xEA;  // flush extended
const CMD_IDENTIFY:    u8 = 0xEC;  // identify device
const CMD_IDENTIFY_PACKET: u8 = 0xA1;

// Control register bits
const CTRL_NIEN:  u8 = 0x02;  // disable interrupts
const CTRL_SRST:  u8 = 0x04;  // software reset
const CTRL_HOB:   u8 = 0x80;  // high order byte (for LBA48 read-back)

// Limits
const SECTOR_SIZE:    usize = 512;
const MAX_RETRY:      usize = 3;
const TIMEOUT_CYCLES: u32   = 0x100_000;
const LBA28_MAX:      u64   = 0x0FFF_FFFF;  // 28-bit LBA max
const LBA48_MAX:      u64   = 0x0000_FFFF_FFFF_FFFF;

// ── FFI to glue.c security gates ─────────────────────────────────────────────

extern "C" {
    pub fn cosinus_disk_security_init_all();
    pub fn disk_gate_write(lba: u64, count: u32, ring: u8, buf: *const u8) -> i32;
    pub fn disk_gate_read(lba: u64, count: u32, ring: u8) -> i32;
    pub fn disk_gate_install(lba_start: u64, lba_end: u64) -> i32;
    pub fn disk_gate_post_install(lba_start: u64, data: *const u8,
                                   data_len: u32, lock_after: i32) -> i32;
    pub fn disk_gate_check_tamper() -> i32;
    pub fn disk_gate_tick();
    pub fn disk_gate_violation_count() -> u32;
    pub fn disk_gate_alert_count() -> u32;
    pub fn disk_gate_tag_fail_count() -> u32;
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaError {
    Timeout,
    DiskError(u8),   // error register value
    DriveFault,
    TooLarge,
    SecurityDenied(i32),
    NoDevice,
    LbaOutOfRange,
    InvalidLength,
}

// ── Bus selection ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum AtaBus {
    Primary,
    Secondary,
}

struct BusPorts {
    data:    u16,
    err:     u16,
    sectors: u16,
    lba_lo:  u16,
    lba_mid: u16,
    lba_hi:  u16,
    drive:   u16,
    status:  u16,
    ctrl:    u16,
}

impl BusPorts {
    const fn primary() -> Self {
        Self {
            data:    ATA0_DATA,
            err:     ATA0_ERR,
            sectors: ATA0_SECTORS,
            lba_lo:  ATA0_LBA_LO,
            lba_mid: ATA0_LBA_MID,
            lba_hi:  ATA0_LBA_HI,
            drive:   ATA0_DRIVE,
            status:  ATA0_STATUS,
            ctrl:    ATA0_CTRL,
        }
    }
    const fn secondary() -> Self {
        Self {
            data:    ATA1_DATA,
            err:     ATA1_ERR,
            sectors: ATA1_SECTORS,
            lba_lo:  ATA1_LBA_LO,
            lba_mid: ATA1_LBA_MID,
            lba_hi:  ATA1_LBA_HI,
            drive:   ATA1_DRIVE,
            status:  ATA1_STATUS,
            ctrl:    ATA1_CTRL,
        }
    }
}

// Active bus — default to primary master
static mut ACTIVE_BUS: AtaBus = AtaBus::Primary;

fn ports() -> BusPorts {
    match unsafe { ACTIVE_BUS } {
        AtaBus::Primary   => BusPorts::primary(),
        AtaBus::Secondary => BusPorts::secondary(),
    }
}

// ── Low-level port I/O ────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags));
    v
}

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

#[inline(always)]
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags));
    v
}

#[inline(always)]
unsafe fn outw(port: u16, val: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags));
}

// 400ns delay — read alt status 4 times (each read ~100ns on ISA)
#[inline(always)]
unsafe fn ata_delay(p: &BusPorts) {
    let _ = inb(p.ctrl);
    let _ = inb(p.ctrl);
    let _ = inb(p.ctrl);
    let _ = inb(p.ctrl);
}

// ── Polling wait ──────────────────────────────────────────────────────────────

unsafe fn wait_not_busy(p: &BusPorts) -> Result<(), AtaError> {
    let mut n = 0u32;
    loop {
        let st = inb(p.status);
        if st & STATUS_BSY == 0 {
            return Ok(());
        }
        n += 1;
        if n > TIMEOUT_CYCLES {
            return Err(AtaError::Timeout);
        }
    }
}

unsafe fn wait_drq(p: &BusPorts) -> Result<(), AtaError> {
    let mut n = 0u32;
    loop {
        let st = inb(p.status);
        if st & STATUS_BSY == 0 && st & STATUS_DRQ != 0 {
            return Ok(());
        }
        if st & STATUS_ERR != 0 {
            return Err(AtaError::DiskError(inb(p.err)));
        }
        if st & STATUS_DF != 0 {
            return Err(AtaError::DriveFault);
        }
        n += 1;
        if n > TIMEOUT_CYCLES {
            return Err(AtaError::Timeout);
        }
    }
}

unsafe fn wait_ready(p: &BusPorts) -> Result<(), AtaError> {
    wait_not_busy(p)?;
    let st = inb(p.status);
    if st & STATUS_ERR != 0 {
        return Err(AtaError::DiskError(inb(p.err)));
    }
    if st & STATUS_DF != 0 {
        return Err(AtaError::DriveFault);
    }
    Ok(())
}

// ── Software reset ────────────────────────────────────────────────────────────

pub unsafe fn ata_reset() {
    let p = ports();
    outb(p.ctrl, CTRL_SRST | CTRL_NIEN);
    ata_delay(&p);
    outb(p.ctrl, CTRL_NIEN);
    ata_delay(&p);
    let _ = wait_not_busy(&p);
}

// ── Select drive ─────────────────────────────────────────────────────────────

unsafe fn select_master(p: &BusPorts) -> Result<(), AtaError> {
    outb(p.drive, DRIVE_MASTER);
    ata_delay(p);
    wait_not_busy(p)
}

// ── LBA28 setup ───────────────────────────────────────────────────────────────

unsafe fn setup_lba28(p: &BusPorts, lba: u64, count: u8) {
    // Disable interrupts during setup
    outb(p.ctrl, CTRL_NIEN);
    outb(p.drive, DRIVE_MASTER | DRIVE_LBA | ((lba >> 24) as u8 & 0x0F));
    outb(p.err,     0x00);
    outb(p.sectors, count);
    outb(p.lba_lo,  (lba & 0xFF) as u8);
    outb(p.lba_mid, ((lba >> 8)  & 0xFF) as u8);
    outb(p.lba_hi,  ((lba >> 16) & 0xFF) as u8);
}

// ── LBA48 setup ───────────────────────────────────────────────────────────────

unsafe fn setup_lba48(p: &BusPorts, lba: u64, count: u16) {
    outb(p.ctrl, CTRL_NIEN);
    outb(p.drive, DRIVE_MASTER | DRIVE_LBA);
    outb(p.err,     0x00);
    // High bytes first (count high, lba high bytes)
    outb(p.sectors, (count >> 8) as u8);
    outb(p.lba_lo,  ((lba >> 24) & 0xFF) as u8);
    outb(p.lba_mid, ((lba >> 32) & 0xFF) as u8);
    outb(p.lba_hi,  ((lba >> 40) & 0xFF) as u8);
    // Low bytes second
    outb(p.sectors, (count & 0xFF) as u8);
    outb(p.lba_lo,  (lba & 0xFF)        as u8);
    outb(p.lba_mid, ((lba >> 8)  & 0xFF) as u8);
    outb(p.lba_hi,  ((lba >> 16) & 0xFF) as u8);
}

// ── Read one 512-byte sector (internal, no security check) ────────────────────

unsafe fn read_sector_raw_internal(p: &BusPorts, lba: u64) -> Result<[u8; 512], AtaError> {
    wait_ready(p)?;

    if lba <= LBA28_MAX {
        setup_lba28(p, lba, 1);
        outb(p.status, CMD_READ_PIO);
    } else {
        setup_lba48(p, lba, 1);
        outb(p.status, CMD_READ_EXT);
    }

    ata_delay(p);
    wait_drq(p)?;

    let mut buf = [0u8; 512];
    for i in 0..256 {
        let w = inw(p.data);
        buf[i * 2]     = (w & 0xFF) as u8;
        buf[i * 2 + 1] = (w >> 8)   as u8;
    }

    let st = inb(p.status);
    if st & STATUS_ERR != 0 {
        return Err(AtaError::DiskError(inb(p.err)));
    }
    Ok(buf)
}

// ── Write one 512-byte sector (internal, no security check) ──────────────────

unsafe fn write_sector_raw_internal(p: &BusPorts, lba: u64, buf: &[u8; 512]) -> Result<(), AtaError> {
    wait_ready(p)?;

    if lba <= LBA28_MAX {
        setup_lba28(p, lba, 1);
        outb(p.status, CMD_WRITE_PIO);
    } else {
        setup_lba48(p, lba, 1);
        outb(p.status, CMD_WRITE_EXT);
    }

    ata_delay(p);
    wait_drq(p)?;

    for i in 0..256 {
        let w = (buf[i * 2] as u16) | ((buf[i * 2 + 1] as u16) << 8);
        outw(p.data, w);
    }

    wait_ready(p)?;

    // Flush write cache
    if lba <= LBA28_MAX {
        outb(p.status, CMD_FLUSH);
    } else {
        outb(p.status, CMD_FLUSH_EXT);
    }
    wait_ready(p)?;

    let st = inb(p.status);
    if st & STATUS_ERR != 0 {
        return Err(AtaError::DiskError(inb(p.err)));
    }
    Ok(())
}

// ── Public API — with security gate and retry ─────────────────────────────────

/// Read a single sector. No security gate — reads are mostly unrestricted.
pub unsafe fn read_sector(lba: u64, buf: &mut [u8; 512]) -> Result<(), AtaError> {
    if lba > LBA48_MAX {
        return Err(AtaError::LbaOutOfRange);
    }

    // Security gate check (ring 0)
    let gate = disk_gate_read(lba, 1, 0);
    if gate != 0 {
        return Err(AtaError::SecurityDenied(gate));
    }

    let p = ports();
    select_master(&p)?;

    for attempt in 0..MAX_RETRY {
        match read_sector_raw_internal(&p, lba) {
            Ok(data) => {
                buf.copy_from_slice(&data);
                return Ok(());
            }
            Err(AtaError::DiskError(_)) if attempt < MAX_RETRY - 1 => {
                ata_reset();
                select_master(&p)?;
            }
            Err(e) => return Err(e),
        }
    }
    Err(AtaError::Timeout)
}

/// Read a single sector raw — no security gate. Used for install header check.
pub unsafe fn read_sector_raw(lba: u64) -> Result<[u8; 512], AtaError> {
    if lba > LBA48_MAX {
        return Err(AtaError::LbaOutOfRange);
    }
    let p = ports();
    select_master(&p)?;
    read_sector_raw_internal(&p, lba)
}

/// Write a single sector with security gate.
pub unsafe fn write_sector(lba: u64, buf: &[u8; 512]) -> Result<(), AtaError> {
    if lba > LBA48_MAX {
        return Err(AtaError::LbaOutOfRange);
    }

    let gate = disk_gate_write(lba, 1, 0, buf.as_ptr());
    if gate != 0 {
        return Err(AtaError::SecurityDenied(gate));
    }

    let p = ports();
    select_master(&p)?;

    for attempt in 0..MAX_RETRY {
        match write_sector_raw_internal(&p, lba, buf) {
            Ok(()) => return Ok(()),
            Err(AtaError::DiskError(_)) if attempt < MAX_RETRY - 1 => {
                ata_reset();
                select_master(&p)?;
            }
            Err(e) => return Err(e),
        }
    }
    Err(AtaError::Timeout)
}

/// Write a single sector — no security gate. Used internally by install.
pub unsafe fn write_sector_unchecked(lba: u64, buf: &[u8; 512]) -> Result<(), AtaError> {
    if lba > LBA48_MAX {
        return Err(AtaError::LbaOutOfRange);
    }
    let p = ports();
    select_master(&p)?;

    for attempt in 0..MAX_RETRY {
        match write_sector_raw_internal(&p, lba, buf) {
            Ok(()) => return Ok(()),
            Err(AtaError::DiskError(_)) if attempt < MAX_RETRY - 1 => {
                ata_reset();
                select_master(&p)?;
            }
            Err(e) => return Err(e),
        }
    }
    Err(AtaError::Timeout)
}

/// Write arbitrary bytes to disk starting at lba_start.
/// Pads last sector with zeros. Returns sectors written.
/// Uses security gate per-sector-batch.
pub unsafe fn write_bytes(lba_start: u64, data: &[u8], max_sectors: u32) -> Result<u32, AtaError> {
    if data.is_empty() {
        return Ok(0);
    }

    let sectors_needed = (data.len() + SECTOR_SIZE - 1) / SECTOR_SIZE;

    if sectors_needed > max_sectors as usize {
        return Err(AtaError::TooLarge);
    }

    if lba_start + sectors_needed as u64 > LBA48_MAX {
        return Err(AtaError::LbaOutOfRange);
    }

    // Run gate check for the whole range upfront
    let gate = disk_gate_write(lba_start, sectors_needed as u32, 0, data.as_ptr());
    if gate != 0 {
        return Err(AtaError::SecurityDenied(gate));
    }

    let p = ports();
    select_master(&p)?;

    let mut sector_buf = [0u8; SECTOR_SIZE];

    for (i, chunk) in data.chunks(SECTOR_SIZE).enumerate() {
        sector_buf = [0u8; SECTOR_SIZE];
        sector_buf[..chunk.len()].copy_from_slice(chunk);

        let lba = lba_start + i as u64;
        let mut written = false;

        for attempt in 0..MAX_RETRY {
            match write_sector_raw_internal(&p, lba, &sector_buf) {
                Ok(()) => { written = true; break; }
                Err(AtaError::DiskError(_)) if attempt < MAX_RETRY - 1 => {
                    ata_reset();
                    select_master(&p)?;
                }
                Err(e) => return Err(e),
            }
        }

        if !written {
            return Err(AtaError::Timeout);
        }
    }

    Ok(sectors_needed as u32)
}

/// Write bytes without security gate — used by install.rs which does
/// its own gate check via disk_gate_install() before calling this.
pub unsafe fn write_bytes_unchecked(lba_start: u64, data: &[u8], max_sectors: u32)
    -> Result<u32, AtaError>
{
    if data.is_empty() {
        return Ok(0);
    }

    let sectors_needed = (data.len() + SECTOR_SIZE - 1) / SECTOR_SIZE;
    if sectors_needed > max_sectors as usize {
        return Err(AtaError::TooLarge);
    }

    let p = ports();
    select_master(&p)?;

    let mut sector_buf = [0u8; SECTOR_SIZE];

    for (i, chunk) in data.chunks(SECTOR_SIZE).enumerate() {
        sector_buf = [0u8; SECTOR_SIZE];
        sector_buf[..chunk.len()].copy_from_slice(chunk);

        let lba = lba_start + i as u64;
        let mut written = false;

        for attempt in 0..MAX_RETRY {
            match write_sector_raw_internal(&p, lba, &sector_buf) {
                Ok(()) => { written = true; break; }
                Err(AtaError::DiskError(_)) if attempt < MAX_RETRY - 1 => {
                    ata_reset();
                    select_master(&p)?;
                }
                Err(e) => return Err(e),
            }
        }

        if !written {
            return Err(AtaError::Timeout);
        }
    }

    Ok(sectors_needed as u32)
}

/// Identify device — returns 512 bytes of IDENTIFY data
pub unsafe fn identify() -> Result<[u8; 512], AtaError> {
    let p = ports();
    select_master(&p)?;
    wait_ready(&p)?;

    outb(p.drive,   DRIVE_MASTER);
    outb(p.sectors, 0);
    outb(p.lba_lo,  0);
    outb(p.lba_mid, 0);
    outb(p.lba_hi,  0);
    outb(p.status,  CMD_IDENTIFY);

    ata_delay(&p);

    let st = inb(p.status);
    if st == 0 {
        return Err(AtaError::NoDevice);
    }

    // If LBA_MID or LBA_HI are non-zero it's not ATA (ATAPI/SATA emulation)
    let mid = inb(p.lba_mid);
    let hi  = inb(p.lba_hi);
    if mid != 0 || hi != 0 {
        return Err(AtaError::NoDevice);
    }

    wait_drq(&p)?;

    let mut buf = [0u8; 512];
    for i in 0..256 {
        let w = inw(p.data);
        buf[i * 2]     = (w & 0xFF) as u8;
        buf[i * 2 + 1] = (w >> 8)   as u8;
    }

    Ok(buf)
}

/// Extract LBA48 max sector count from IDENTIFY data
pub fn identify_lba48_sectors(id: &[u8; 512]) -> u64 {
    // Words 100-103 contain max LBA48 address
    let lo = (id[200] as u64) | ((id[201] as u64) << 8)
           | ((id[202] as u64) << 16) | ((id[203] as u64) << 24);
    let hi = (id[204] as u64) | ((id[205] as u64) << 8)
           | ((id[206] as u64) << 16) | ((id[207] as u64) << 24);
    lo | (hi << 32)
}

/// Check if drive supports LBA48
pub fn identify_supports_lba48(id: &[u8; 512]) -> bool {
    // Word 83, bit 10 = LBA48 support
    let w83 = (id[166] as u16) | ((id[167] as u16) << 8);
    (w83 & (1 << 10)) != 0
}

/// Initialize security layer — call once after kernel heap is ready
pub fn security_init() {
    unsafe { cosinus_disk_security_init_all(); }
}

/// Tick security state — call from PIT handler
pub fn security_tick() {
    unsafe { disk_gate_tick(); }
}

/// Check for tamper — returns true if tamper detected (triggers emergency lock)
pub fn check_tamper() -> bool {
    unsafe { disk_gate_check_tamper() != 0 }
}

/// Get security stats for diagnostics
pub struct SecurityStats {
    pub violations:  u32,
    pub alerts:      u32,
    pub tag_failures: u32,
}

pub fn security_stats() -> SecurityStats {
    unsafe {
        SecurityStats {
            violations:   disk_gate_violation_count(),
            alerts:       disk_gate_alert_count(),
            tag_failures: disk_gate_tag_fail_count(),
        }
    }
}