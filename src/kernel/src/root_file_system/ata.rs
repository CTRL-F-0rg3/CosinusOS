// Bare-metal ATA PIO ring-0 driver for installation only
// Primary bus, master drive. No IRQ — polling only.

use core::arch::asm;

const DATA:    u16 = 0x1F0;
const ERR:     u16 = 0x1F1;
const SECTORS: u16 = 0x1F2;
const LBA_LO:  u16 = 0x1F3;
const LBA_MID: u16 = 0x1F4;
const LBA_HI:  u16 = 0x1F5;
const DRIVE:   u16 = 0x1F6;
const STATUS:  u16 = 0x1F7;
const CMD:     u16 = 0x1F7;

const CMD_READ:  u8 = 0x20;
const CMD_WRITE: u8 = 0x30;
const CMD_FLUSH: u8 = 0xE7;

const STATUS_BSY: u8 = 0x80;
const STATUS_DRQ: u8 = 0x08;
const STATUS_ERR: u8 = 0x01;

#[derive(Debug)]
pub enum AtaError {
    Timeout,
    DiskError,
    TooLarge,
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack));
    val
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

#[inline]
unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    asm!("in ax, dx", in("dx") port, out("ax") val, options(nomem, nostack));
    val
}

#[inline]
unsafe fn outw(port: u16, val: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

unsafe fn wait_ready() -> Result<(), AtaError> {
    let mut timeout = 0u32;
    loop {
        let status = inb(STATUS);
        if status & STATUS_BSY == 0 && status & STATUS_DRQ == 0 {
            return Ok(());
        }
        if status & STATUS_ERR != 0 {
            return Err(AtaError::DiskError);
        }
        timeout += 1;
        if timeout > 0x10_0000 {
            return Err(AtaError::Timeout);
        }
    }
}

unsafe fn wait_drq() -> Result<(), AtaError> {
    let mut timeout = 0u32;
    loop {
        let status = inb(STATUS);
        if status & STATUS_DRQ != 0 { return Ok(()); }
        if status & STATUS_ERR != 0 { return Err(AtaError::DiskError); }
        timeout += 1;
        if timeout > 0x10_0000 { return Err(AtaError::Timeout); }
    }
}

fn setup_lba28(lba: u64, sector_count: u8) {
    unsafe {
        outb(DRIVE,   0xE0 | ((lba >> 24) as u8 & 0x0F)); // LBA mode, master
        outb(ERR,     0x00);
        outb(SECTORS, sector_count);
        outb(LBA_LO,  (lba & 0xFF) as u8);
        outb(LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HI,  ((lba >> 16) & 0xFF) as u8);
    }
}

// Read one 512-byte sector into buf[0..512]
pub unsafe fn read_sector(lba: u64, buf: &mut [u8; 512]) -> Result<(), AtaError> {
    wait_ready()?;
    setup_lba28(lba, 1);
    outb(CMD, CMD_READ);
    wait_drq()?;
    for i in 0..256 {
        let word = inw(DATA);
        buf[i * 2]     = (word & 0xFF) as u8;
        buf[i * 2 + 1] = (word >> 8)   as u8;
    }
    Ok(())
}

// Write one 512-byte sector from buf[0..512]
pub unsafe fn write_sector(lba: u64, buf: &[u8; 512]) -> Result<(), AtaError> {
    wait_ready()?;
    setup_lba28(lba, 1);
    outb(CMD, CMD_WRITE);
    wait_drq()?;
    for i in 0..256 {
        let word = (buf[i * 2] as u16) | ((buf[i * 2 + 1] as u16) << 8);
        outw(DATA, word);
    }
    // Flush write cache
    wait_ready()?;
    outb(CMD, CMD_FLUSH);
    wait_ready()?;
    Ok(())
}

// Write arbitrary bytes starting at lba, pads last sector with zeros
pub unsafe fn write_bytes(lba_start: u64, data: &[u8], max_sectors: u32) -> Result<u32, AtaError> {
    let sectors_needed = (data.len() + 511) / 512;
    if sectors_needed > max_sectors as usize {
        return Err(AtaError::TooLarge);
    }
    let mut sector_buf = [0u8; 512];
    for (i, chunk) in data.chunks(512).enumerate() {
        sector_buf = [0u8; 512];
        sector_buf[..chunk.len()].copy_from_slice(chunk);
        write_sector(lba_start + i as u64, &sector_buf)?;
    }
    Ok(sectors_needed as u32)
}

// Read one sector and return raw bytes — used to check install header
pub unsafe fn read_sector_raw(lba: u64) -> Result<[u8; 512], AtaError> {
    let mut buf = [0u8; 512];
    read_sector(lba, &mut buf)?;
    Ok(buf)
}