// wrappers.zig - Zig friendly wrappers for ASM primitives
const asm_io = @extern(*const fn (u64, u32) void, .{ .name = "mmio_write32" });
const asm_read = @extern(*const fn (u64) u32, .{ .name = "mmio_read32" });

pub fn writeReg(base: u64, offset: u32, value: u32) void {
    asm_io(base + offset, value);
}

pub fn readReg(base: u64, offset: u32) u32 {
    return asm_read(base + offset);
}

// Efficiently push multiple DWORDs to the Ring
pub fn writeRing(ring: [*]volatile u32, wptr: *u32, data: []const u32) void {
    for (data) |val| {
        ring[wptr.*] = val;
        wptr.* += 1;
    }
}
