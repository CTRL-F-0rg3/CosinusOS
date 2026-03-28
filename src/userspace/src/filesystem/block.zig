// block.zig  - no std zig
pub const BLOCK_SIZE: usize = 4096; // 1 blok = 4 KB
pub const TOTAL_BLOCKS: usize = 1024 * 256; // np. 1 GB przy 4 KB blokach

pub fn disk_capacity_bytes() usize {
    return BLOCK_SIZE * TOTAL_BLOCKS;
}

pub fn disk_capacity_kb() usize {
    return disk_capacity_bytes() / 1024;
}

pub fn disk_capacity_mb() usize {
    return disk_capacity_bytes() / (1024 * 1024);
}

pub fn disk_capacity_gb() usize {
    return disk_capacity_bytes() / (1024 * 1024 * 1024);
}
