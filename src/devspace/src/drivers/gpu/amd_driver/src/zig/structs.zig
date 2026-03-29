// structs.zig - AMD Hardware Structures
// Packed for exact memory layout representation

pub const PM4HeaderType = enum(u2) {
    Type0 = 0,
    Type1 = 1,
    Type2 = 2,
    Type3 = 3,
};

pub const PM4Header = packed struct {
    count: u14, // Number of DWORDs in body
    reserved: u2,
    opcode: u8, // IT_OPCODE
    reserved2: u6,
    header_type: PM4HeaderType,
};

pub const GfxRingState = struct {
    wptr: u32,
    rptr: u32,
    ring_size_dw: u32,
    ring_addr: [*]volatile u32,
};
