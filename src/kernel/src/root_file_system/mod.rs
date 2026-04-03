pub mod ata;
pub mod layout;
pub mod install;

use layout::{HEADER_LBA, MAGIC};
use crate::debug::{log_ok, serial_print, print, printc, col};

pub fn check_and_install(mb_info: u64) {
    unsafe {
        serial_print("[rootfs] Checking installation...\n");

        let sector = match ata::read_sector_raw(HEADER_LBA) {
            Ok(s)  => s,
            Err(_) => {
                serial_print("[rootfs] ATA read failed, skipping install.\n");
                log_ok("RootFS install", false);
                return;
            }
        };

        if &sector[0..8] == &MAGIC {
            serial_print("[rootfs] Already installed.\n");
            log_ok("RootFS already installed", true);
            return;
        }

        printc("=== Installing CosinusOS to disk ===\n", col::YELLOW);
        serial_print("[rootfs] Not installed, starting...\n");

        match install::run_install(mb_info) {
            Ok(res) => {
                print("[rootfs] kernel:    ");
                { let mut b = [0u8; 24]; print(crate::debug::num_str(res.kernel_sectors    as usize, &mut b)); }
                print(" sectors\n");
                print("[rootfs] devspace:  ");
                { let mut b = [0u8; 24]; print(crate::debug::num_str(res.devspace_sectors  as usize, &mut b)); }
                print(" sectors\n");
                print("[rootfs] fsserver:  ");
                { let mut b = [0u8; 24]; print(crate::debug::num_str(res.fsserver_sectors  as usize, &mut b)); }
                print(" sectors\n");
                print("[rootfs] userspace: ");
                { let mut b = [0u8; 24]; print(crate::debug::num_str(res.userspace_sectors as usize, &mut b)); }
                print(" sectors\n");
                log_ok("RootFS install", true);
                printc("=== Installation complete ===\n", col::LGREEN);
            }
            Err(_) => {
                serial_print("[rootfs] Install FAILED\n");
                log_ok("RootFS install", false);
            }
        }
    }
}