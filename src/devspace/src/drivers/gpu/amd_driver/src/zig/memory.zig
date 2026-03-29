// memory.zig - VRAM and GART Aperture Logic
const std = @import("std");

pub const VRAM_START = 0x0000_0000; // GPU Internal Address
pub const GART_START = 0x8000_0000;

pub const MemoryManager = struct {
    vram_size: u64,
    gart_table_ptr: [*]u64,

    // Map a system page into the GPU GART
    pub fn map_page(self: *MemoryManager, gpu_addr: u64, sys_addr: u64) void {
        const index = (gpu_addr - GART_START) / 4096;
        // AMD GART entries usually need valid/writable bits set
        self.gart_table_ptr[index] = sys_addr | 0x1;
    }
};
