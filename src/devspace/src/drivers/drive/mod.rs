// mod.rs - Główny moduł sterownika dysku ATA
use crate::api::{DiskRequest, DiskResponse, DiskRequestType};

// Importy funkcji z critical.asm
extern "C" {
    fn transfer_sector_in(buf: *mut u8, port: u16);
    fn transfer_sector_out(buf: *const u8, port: u16);
    fn delay_400ns();
}

pub struct AtaDriver {
    pub active_drive: u8, // 0 = Master, 1 = Slave
    pub forth_vm: ForthVM, // Twoja instancja maszyny Forth
}

impl AtaDriver {
    pub fn new() -> Self {
        let mut driver = Self {
            active_drive: 0,
            forth_vm: ForthVM::init(), 
        };
        // Ładowanie drive_def.fs i drive_logic.fs do pamięci VM
        driver.load_forth_logic();
        driver
    }

    /// Główny punkt wejścia dla żądań IPC z Ring 3 (VFS/User)
    pub fn handle_request(&mut self, req: DiskRequest) -> DiskResponse {
        match req.req_type {
            DiskRequestType::Read => {
                let success = self.read_sectors(req.lba, req.sector_count, req.buffer_phys);
                DiskResponse { req_id: req.req_id, status: if success { 0 } else { -1 } }
            }
            DiskRequestType::Write => {
                let success = self.write_sectors(req.lba, req.sector_count, req.buffer_phys);
                DiskResponse { req_id: req.req_id, status: if success { 0 } else { -2 } }
            }
            DiskRequestType::Identify => {
                self.identify_drive()
            }
            _ => DiskResponse { req_id: req.req_id, status: -255 },
        }
    }

    fn read_sectors(&mut self, lba: u64, count: u32, dest_phys: u64) -> bool {
        // 1. Wywołaj Forth, aby przygotował rejestry ATA (LBA, CMD_READ)
        // forth_exec(self.forth_vm, "ata-prepare-read", lba, count);

        // 2. Wykonaj krytyczny transfer przez ASM (Ring 1 ma dostęp do portów)
        unsafe {
            for i in 0..count {
                let offset = (i * 512) as usize;
                // Czekaj na DRQ (możesz to zrobić w Forth lub tutaj)
                transfer_sector_in((dest_phys as *mut u8).add(offset), 0x1F0);
            }
        }
        true
    }

    fn load_forth_logic(&mut self) {
        // Tutaj kompilujesz/ładujesz drive_def.fs i drive_logic.fs
        // tak aby VM widziała słowa 'ata-read', 'ata-reset' itp.
    }
}

// Globalna instancja sterownika (wymaga synchronizacji w DevSpace)
static mut ATA_INSTANCE: Option<AtaDriver> = None;

#[no_mangle]
pub extern "C" fn dev_space_init() {
    unsafe {
        ATA_INSTANCE = Some(AtaDriver::new());
    }
    // Tutaj pętla odbierająca wiadomości IPC
}