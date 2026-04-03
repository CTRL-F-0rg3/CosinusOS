// Disk layout for CosinusOS installation
// Sector 0: MBR (untouched)
// Sector 1: CosinusOS install header (magic + segment table)
// Sectors 2048+: kernel.elf
// Sectors 16384+: devspace.elf
// Sectors 32768+: fs_server.bin
// Sectors 49152+: userspace.bin

pub const MAGIC: [u8; 8] = *b"COSINST\0";
pub const HEADER_LBA: u64 = 1;
pub const SECTOR_SIZE: usize = 512;

pub const SEG_KERNEL:    SegmentSlot = SegmentSlot { lba_start: 2048,  max_sectors: 8192  };
pub const SEG_DEVSPACE:  SegmentSlot = SegmentSlot { lba_start: 16384, max_sectors: 8192  };
pub const SEG_FSSERVER:  SegmentSlot = SegmentSlot { lba_start: 32768, max_sectors: 4096  };
pub const SEG_USERSPACE: SegmentSlot = SegmentSlot { lba_start: 49152, max_sectors: 8192  };

#[derive(Clone, Copy)]
pub struct SegmentSlot {
    pub lba_start:   u64,
    pub max_sectors: u32,
}

// Written to disk at HEADER_LBA so next boot can find segments
#[repr(C, packed)]
pub struct InstallHeader {
    pub magic:           [u8; 8],
    pub kernel_lba:      u64,
    pub kernel_sectors:  u32,
    pub devspace_lba:    u64,
    pub devspace_sectors: u32,
    pub fsserver_lba:    u64,
    pub fsserver_sectors: u32,
    pub userspace_lba:   u64,
    pub userspace_sectors: u32,
    pub _pad:            [u8; 456], // pad to 512 bytes (one sector)
}

impl InstallHeader {
    pub const fn zeroed() -> Self {
        Self {
            magic:             [0u8; 8],
            kernel_lba:        0,
            kernel_sectors:    0,
            devspace_lba:      0,
            devspace_sectors:  0,
            fsserver_lba:      0,
            fsserver_sectors:  0,
            userspace_lba:     0,
            userspace_sectors: 0,
            _pad:              [0u8; 456],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC
    }
}