// block.zig — ATA block device interface
//
// This file is the boundary between the filesystem (cache.zig, fs.zig)
// and the disk driver (driver.odin + ata.forth).
//
// It does NOT touch hardware directly — all I/O goes through FFI calls
// into the Odin driver which handles ATA PIO and port access syscalls.
//
// DiskInfo at 0x100000 is kept for backward compat with the FS server
// boot handshake, but is no longer used for I/O.

// ── FFI imports from driver.odin ─────────────────────────────────────────────

extern fn disk_driver_init() bool;
extern fn disk_driver_sector_count() u64;
extern fn disk_driver_sector_size() u32;
extern fn disk_driver_read(lba: u64, count: u32, buf: [*]u8) bool;
extern fn disk_driver_write(lba: u64, count: u32, buf: [*]u8) bool;
extern fn disk_driver_select(drive: u8) bool;

// ── DiskInfo — boot handshake struct (read by FS server at startup) ───────────
// Written by the Odin driver after probe, read once by main.zig.

pub const DISK_INFO_PHYS_ADDR: usize = 0x0010_0000;
pub const DISK_MAGIC: u32 = 0xD15CAFE0;

pub const DiskInfo = extern struct {
    magic_number: u32,
    block_size: u32,
    block_count: u64,
    mmio_base: u32, // unused for ATA PIO, kept for ABI compat
    is_ready: u32,
};

// ── Error types ───────────────────────────────────────────────────────────────

pub const BlockError = error{
    DriverNotInitialized,
    ReadFailed,
    WriteFailed,
    InvalidSector,
    DriveNotPresent,
    Timeout,
};

// ── BlockDevice ───────────────────────────────────────────────────────────────

pub const BlockDevice = struct {
    sector_count: u64,
    sector_size: u32,
    info: *volatile DiskInfo,

    /// Initialize ATA driver and populate DiskInfo.
    /// Call once at FS server startup.
    pub fn init() BlockError!BlockDevice {
        if (!disk_driver_init()) {
            return BlockError.DriverNotInitialized;
        }

        const count = disk_driver_sector_count();
        const size = disk_driver_sector_size();

        // Write DiskInfo so FS server main.zig can read it
        const info_ptr: *volatile DiskInfo = @ptrFromInt(DISK_INFO_PHYS_ADDR);
        info_ptr.magic_number = DISK_MAGIC;
        info_ptr.block_size = size;
        info_ptr.block_count = count;
        info_ptr.mmio_base = 0;
        info_ptr.is_ready = 1;

        return BlockDevice{
            .sector_count = count,
            .sector_size = size,
            .info = info_ptr,
        };
    }

    pub fn getBlockSize(self: *const BlockDevice) u32 {
        return self.sector_size;
    }

    pub fn getTotalSectors(self: *const BlockDevice) u64 {
        return self.sector_count;
    }

    /// Read a single 512-byte sector into buf.
    /// buf must be at least sector_size bytes.
    pub fn readSector(self: *const BlockDevice, lba: u64, buf: []u8) BlockError!void {
        if (buf.len < 512) return BlockError.ReadFailed;
        if (lba >= self.sector_count) return BlockError.InvalidSector;

        if (!disk_driver_read(lba, 1, buf.ptr)) {
            return BlockError.ReadFailed;
        }
    }

    /// Write a single 512-byte sector from buf.
    pub fn writeSector(self: *const BlockDevice, lba: u64, buf: []const u8) BlockError!void {
        if (buf.len < 512) return BlockError.WriteFailed;
        if (lba >= self.sector_count) return BlockError.InvalidSector;

        // Odin driver takes [*]u8, cast away const — driver does not retain ptr
        if (!disk_driver_write(lba, 1, @constCast(buf.ptr))) {
            return BlockError.WriteFailed;
        }
    }

    /// Read `count` consecutive sectors starting at `lba`.
    /// buf must be count * 512 bytes.
    /// Used by cache.zig for multi-sector prefetch.
    pub fn readSectors(
        self: *const BlockDevice,
        lba: u64,
        count: u32,
        buf: []u8,
    ) BlockError!void {
        if (buf.len < @as(usize, count) * 512) return BlockError.ReadFailed;
        if (lba + count > self.sector_count) return BlockError.InvalidSector;

        if (!disk_driver_read(lba, count, buf.ptr)) {
            return BlockError.ReadFailed;
        }
    }

    /// Write `count` consecutive sectors.
    pub fn writeSectors(
        self: *const BlockDevice,
        lba: u64,
        count: u32,
        buf: []const u8,
    ) BlockError!void {
        if (buf.len < @as(usize, count) * 512) return BlockError.WriteFailed;
        if (lba + count > self.sector_count) return BlockError.InvalidSector;

        if (!disk_driver_write(lba, count, @constCast(buf.ptr))) {
            return BlockError.WriteFailed;
        }
    }

    /// Select master (0) or slave (1) drive.
    pub fn selectDrive(self: *BlockDevice, drive: u8) BlockError!void {
        if (!disk_driver_select(drive)) {
            return BlockError.DriveNotPresent;
        }
        self.sector_count = disk_driver_sector_count();
    }
};
