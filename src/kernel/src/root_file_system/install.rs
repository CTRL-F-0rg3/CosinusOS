// Reads all MB2 modules in order:
//   module 0 — kernel.elf
//   module 1 — devspace.elf
//   module 2 — fs_server.bin
//   module 3 — userspace.bin
// Writes each to its disk segment, then writes install header.

use super::ata;
use super::layout::{
    InstallHeader, MAGIC, HEADER_LBA,
    SEG_KERNEL, SEG_DEVSPACE, SEG_FSSERVER, SEG_USERSPACE,
};
use crate::debug::serial_print;

#[repr(C, packed)] struct Mb2Hdr { total: u32, _res: u32 }
#[repr(C, packed)] struct Mb2Tag { typ: u32, sz: u32 }
#[repr(C, packed)] struct Mb2Mod { typ: u32, sz: u32, start: u32, end: u32 }

// Collect up to 4 MB2 modules — returns (start, end) pairs
unsafe fn collect_modules(info: u64) -> [Option<(u64, u64)>; 4] {
    let mut out = [None; 4];
    let mut idx = 0usize;

    if info == 0 { return out; }
    let total = (*(info as *const Mb2Hdr)).total as u64;
    let mut off = 8u64;

    while off < total && idx < 4 {
        let tag = &*((info + off) as *const Mb2Tag);
        if tag.typ == 0 { break; }
        if tag.typ == 3 {
            let m = &*((info + off) as *const Mb2Mod);
            if m.end > m.start {
                out[idx] = Some((m.start as u64, m.end as u64));
                idx += 1;
            }
        }
        off += (tag.sz as u64 + 7) & !7;
    }
    out
}

pub struct InstallResult {
    pub kernel_sectors:    u32,
    pub devspace_sectors:  u32,
    pub fsserver_sectors:  u32,
    pub userspace_sectors: u32,
}

pub unsafe fn run_install(mb_info: u64) -> Result<InstallResult, ata::AtaError> {
    serial_print("[rootfs] Collecting MB2 modules...\n");
    let mods = collect_modules(mb_info);

    // Write each module to its segment — None or zero-size = skip (0 sectors)
    let kernel_sectors = match mods[0] {
        Some((s, e)) => {
            serial_print("[rootfs] Writing kernel.elf...\n");
            let data = core::slice::from_raw_parts(s as *const u8, (e - s) as usize);
            ata::write_bytes(SEG_KERNEL.lba_start, data, SEG_KERNEL.max_sectors)?
        }
        None => { serial_print("[rootfs] kernel.elf not found in MB2, skipping\n"); 0 }
    };

    let devspace_sectors = match mods[1] {
        Some((s, e)) => {
            serial_print("[rootfs] Writing devspace.elf...\n");
            let data = core::slice::from_raw_parts(s as *const u8, (e - s) as usize);
            ata::write_bytes(SEG_DEVSPACE.lba_start, data, SEG_DEVSPACE.max_sectors)?
        }
        None => { serial_print("[rootfs] devspace.elf not found in MB2, skipping\n"); 0 }
    };

    let fsserver_sectors = match mods[2] {
        Some((s, e)) => {
            serial_print("[rootfs] Writing fs_server.bin...\n");
            let data = core::slice::from_raw_parts(s as *const u8, (e - s) as usize);
            ata::write_bytes(SEG_FSSERVER.lba_start, data, SEG_FSSERVER.max_sectors)?
        }
        None => { serial_print("[rootfs] fs_server.bin not found in MB2, skipping\n"); 0 }
    };

    let userspace_sectors = match mods[3] {
        Some((s, e)) => {
            serial_print("[rootfs] Writing userspace.bin...\n");
            let data = core::slice::from_raw_parts(s as *const u8, (e - s) as usize);
            ata::write_bytes(SEG_USERSPACE.lba_start, data, SEG_USERSPACE.max_sectors)?
        }
        None => { serial_print("[rootfs] userspace.bin not found in MB2, skipping\n"); 0 }
    };

    // Write install header
    let header = InstallHeader {
        magic:             MAGIC,
        kernel_lba:        SEG_KERNEL.lba_start,
        kernel_sectors,
        devspace_lba:      SEG_DEVSPACE.lba_start,
        devspace_sectors,
        fsserver_lba:      SEG_FSSERVER.lba_start,
        fsserver_sectors,
        userspace_lba:     SEG_USERSPACE.lba_start,
        userspace_sectors,
        _pad:              [0u8; 456],
    };

    let header_bytes = core::slice::from_raw_parts(
        &header as *const InstallHeader as *const u8,
        512,
    );
    let mut sector_buf = [0u8; 512];
    sector_buf.copy_from_slice(header_bytes);
    ata::write_sector(HEADER_LBA, &sector_buf)?;
    serial_print("[rootfs] Install header written.\n");

    Ok(InstallResult { kernel_sectors, devspace_sectors, fsserver_sectors, userspace_sectors })
}