// block.zig (no_std)
const DISK_INFO_PHYS_ADDR: usize = 0x00100000;

pub const DiskInfo = extern struct {
    magic_number: u32,
    block_size: u32,
    block_count: u64,
    mmio_base: u32,
    is_ready: u32,
};

pub const DiskError = error{
    InitTimeout,
    InvalidMagic,
};

pub const BlockDevice = struct {
    info: *volatile DiskInfo,

    pub fn init() DiskError!BlockDevice {
        const info_ptr: *volatile DiskInfo = @ptrFromInt(DISK_INFO_PHYS_ADDR);

        var timeout: usize = 1000000;
        while (info_ptr.is_ready == 0) {
            if (timeout == 0) return DiskError.InitTimeout;
            timeout -= 1;
            asm volatile ("nop" ::: .{ .memory = true });
        }

        if (info_ptr.magic_number != 0xD15CAFE0) {
            return DiskError.InvalidMagic;
        }

        return BlockDevice{ .info = info_ptr };
    }

    pub fn getBlockSize(self: *const BlockDevice) u32 {
        return self.info.block_size;
    }

    pub fn getTotalSectors(self: *const BlockDevice) u64 {
        return self.info.block_count;
    }
};
