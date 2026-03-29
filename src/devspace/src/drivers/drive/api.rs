// api.rs - Publiczny interfejs sterownika blokowego w DevSpace
#[repr(u32)]
pub enum DiskRequestType {
    Read = 1,
    Write = 2,
    Identify = 3,
    Flush = 4,
}

#[repr(C, packed)]
pub struct DiskRequest {
    pub req_id: u64,
    pub req_type: DiskRequestType,
    pub lba: u64,
    pub sector_count: u32,
    pub buffer_phys: u64, // Adres fizyczny bufora (Ring 1 może go potrzebować dla DMA/PIO)
}

pub struct DiskResponse {
    pub req_id: u64,
    pub status: i32, // 0 = OK, < 0 = Error Code
}

// Funkcja eksponowana dla innych usług
pub fn send_disk_command(req: DiskRequest) -> DiskResponse {
    // Tutaj implementacja wysyłania do kolejki sterownika ATA
    //todo!("Wysyłka przez IPC do wątku obsługi ATA");
}