// api.zig - Public Driver API
const structs = @import("structs.zig");

pub const GpuRequestType = enum(u32) {
    SubmitBuffer = 1,
    CreateContext = 2,
    GetStatus = 3,
};

pub const GpuRequest = packed struct {
    req_id: u64,
    req_type: GpuRequestType,
    data_ptr: u64,
    data_size: u32,
};

//pub fn handle_request(req: GpuRequest) i32 {
// Logic to dispatch to Forth VM or direct MMIO
// return 0; // Success
//}
