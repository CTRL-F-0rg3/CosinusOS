package amd_driver

// Import symbols from Zig (compiled as static lib or object)
foreign import zig "zig_bin.a"
foreign zig {
    @(link_name="get_pm4_header")
    get_pm4_header :: proc(opcode: u8, count: u16) -> u32 ---
}

// Global Driver State in Odin
Driver_State :: struct {
    mmio_base:   uintptr,
    vram_size:   u64,
    ring_buffer: ^u32,
    wptr:        u32,
    forth_vm:    rawptr, // Pointer to Forth VM instance
}

g_gpu: Driver_State

init_gpu :: proc() {
    // 1. Setup MMIO via Rust/ASM calls
    // 2. Calibrate Forth VM: map Odin functions to Forth primitives
    setup_forth_primitives()
    
    // 3. Kick off the Ring Buffer using Zig structs
    header := get_pm4_header(0x27, 1) // Draw Index opcode
    push_to_ring(header)
}