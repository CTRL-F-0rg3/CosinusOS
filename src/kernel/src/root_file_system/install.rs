// Writes kernel.elf, devspace.elf, fs_server.bin, userspace.bin
// to their respective disk segments, then writes the install header.
// Called only when installation is not detected.

use super::ata;
use super::layout::{
    InstallHeader, MAGIC, HEADER_LBA,
    SEG_KERNEL, SEG_DEVSPACE, SEG_FSSERVER, SEG_USERSPACE,
};
use crate::debug::{log_ok, serial_print};

// Embedded binaries — included at compile time from build output
// These are the exact files that would be in iso/boot/
// The kernel.elf includes itself (bootloader copies it to MB2 module)

extern "C" {
    // Symbols injected by linker from MB2 modules or embedded sections
    // Defined in linker.ld as:
    //   _binary_kernel_elf_start / _size
    //   _binary_devspace_elf_start / _size
    //   _binary_fs_server_bin_start / _size
    //   _binary_userspace_bin_start / _size
    static _binary_kernel_elf_start:    u8;
    static _binary_kernel_elf_size:     usize;
    static _binary_devspace_elf_start:  u8;
    static _binary_devspace_elf_size:   usize;
    static _binary_fs_server_bin_start: u8;
    static _binary_fs_server_bin_size:  usize;
    static _binary_userspace_bin_start: u8;
    static _binary_userspace_bin_size:  usize;
}

pub struct InstallResult {
    pub kernel_sectors:    u32,
    pub devspace_sectors:  u32,
    pub fsserver_sectors:  u32,
    pub userspace_sectors: u32,
}

pub unsafe fn run_install() -> Result<InstallResult, ata::AtaError> {
    serial_print("[rootfs] Starting installation...\n");

    // --- kernel.elf ---
    let kernel_data = core::slice::from_raw_parts(
        &_binary_kernel_elf_start as *const u8,
        _binary_kernel_elf_size,
    );
    serial_print("[rootfs] Writing kernel.elf...\n");
    let kernel_sectors = ata::write_bytes(
        SEG_KERNEL.lba_start,
        kernel_data,
        SEG_KERNEL.max_sectors,
    )?;

    // --- devspace.elf ---
    let devspace_data = core::slice::from_raw_parts(
        &_binary_devspace_elf_start as *const u8,
        _binary_devspace_elf_size,
    );
    serial_print("[rootfs] Writing devspace.elf...\n");
    let devspace_sectors = ata::write_bytes(
        SEG_DEVSPACE.lba_start,
        devspace_data,
        SEG_DEVSPACE.max_sectors,
    )?;

    // --- fs_server.bin ---
    let fsserver_data = core::slice::from_raw_parts(
        &_binary_fs_server_bin_start as *const u8,
        _binary_fs_server_bin_size,
    );
    serial_print("[rootfs] Writing fs_server.bin...\n");
    let fsserver_sectors = ata::write_bytes(
        SEG_FSSERVER.lba_start,
        fsserver_data,
        SEG_FSSERVER.max_sectors,
    )?;

    // --- userspace.bin ---
    let userspace_data = core::slice::from_raw_parts(
        &_binary_userspace_bin_start as *const u8,
        _binary_userspace_bin_size,
    );
    serial_print("[rootfs] Writing userspace.bin...\n");
    let userspace_sectors = ata::write_bytes(
        SEG_USERSPACE.lba_start,
        userspace_data,
        SEG_USERSPACE.max_sectors,
    )?;

    // --- Write install header at sector 1 ---
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

    Ok(InstallResult {
        kernel_sectors,
        devspace_sectors,
        fsserver_sectors,
        userspace_sectors,
    })
}