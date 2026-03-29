// driver.odin — ATA/IDE PIO disk driver
//
// Layers:
//   1. Forth VM — executes ata.forth words for hardware access
//   2. Port I/O shims — ring-3 calls sys_outb/sys_inb instead of IN/OUT
//   3. Public API — read_sectors / write_sectors / probe used by block.zig FFI
//
// Threading model: single-threaded driver process.
// All calls from FS server go through IPC queue processed here.

package disk_driver

import "core:fmt"
import "core:mem"
import "core:slice"

// ── Syscall numbers (must match kernel syscall_api.rs) ────────────────────────

SYS_OUTB   :: 0x40 // sys_outb(port: u16, val: u8)
SYS_INB    :: 0x41 // sys_inb(port: u16) -> u8
SYS_OUTW   :: 0x42 // sys_outw(port: u16, val: u16)
SYS_INW    :: 0x43 // sys_inw(port: u16) -> u16
SYS_IPC_RECV :: 0x09
SYS_IPC_SEND :: 0x08
SYS_YIELD    :: 0x03
SYS_DEBUG    :: 0x0D

// ── Raw syscall (int 0x80) ────────────────────────────────────────────────────

@(private)
syscall2 :: proc "c" (num, a0, a1: u64) -> i64 {
    ret: i64
    #asm {
        mov rax, num
        mov rdi, a0
        mov rsi, a1
        int 0x80
        mov ret, rax
    }
    return ret
}

@(private)
syscall1 :: proc "c" (num, a0: u64) -> i64 {
    ret: i64
    #asm {
        mov rax, num
        mov rdi, a0
        int 0x80
        mov ret, rax
    }
    return ret
}

// ── Port I/O via syscall ──────────────────────────────────────────────────────

outb :: proc(port: u16, val: u8) {
    syscall2(SYS_OUTB, u64(port), u64(val))
}

inb :: proc(port: u16) -> u8 {
    return u8(syscall2(SYS_INB, u64(port), 0))
}

outw :: proc(port: u16, val: u16) {
    syscall2(SYS_OUTW, u64(port), u64(val))
}

inw :: proc(port: u16) -> u16 {
    return u16(syscall2(SYS_INW, u64(port), 0))
}

debug_print :: proc(s: string) {
    syscall2(SYS_DEBUG, u64(uintptr(raw_data(s))), u64(len(s)))
}

// ── Forth VM ──────────────────────────────────────────────────────────────────
// Minimal indirect-threaded Forth that runs ata.forth.
// Stack: 64-cell data stack, 32-cell return stack.
// Memory: flat 64KB dictionary.

DICT_SIZE    :: 65536
STACK_DEPTH  :: 64
RSTACK_DEPTH :: 32

ForthVM :: struct {
    dict:       [DICT_SIZE]u8,
    dict_here:  u16,
    dstack:     [STACK_DEPTH]i64,
    dsp:        int,              // data stack pointer (grows up)
    rstack:     [RSTACK_DEPTH]u16,
    rsp:        int,
    ip:         u16,              // instruction pointer into dict
    running:    bool,
}

// Cell type tags embedded in bytecode
FORTH_LIT    :: 0x01  // push literal i64 (next 8 bytes)
FORTH_CALL   :: 0x02  // call word at address (next 2 bytes)
FORTH_RET    :: 0x03  // return
FORTH_PRIM   :: 0x04  // primitive opcode (next 1 byte)
FORTH_BRANCH :: 0x05  // unconditional branch (next 2 bytes)
FORTH_0BRANCH:: 0x06  // branch if TOS==0 (next 2 bytes)

// Primitive opcodes
PRIM_ADD   :: 0x01
PRIM_SUB   :: 0x02
PRIM_MUL   :: 0x03
PRIM_DIV   :: 0x04
PRIM_AND   :: 0x05
PRIM_OR    :: 0x06
PRIM_XOR   :: 0x07
PRIM_RSHIFT:: 0x08
PRIM_LSHIFT:: 0x09
PRIM_DUP   :: 0x0A
PRIM_DROP  :: 0x0B
PRIM_SWAP  :: 0x0C
PRIM_OVER  :: 0x0D
PRIM_ROT   :: 0x0E
PRIM_STORE :: 0x0F   // !
PRIM_FETCH :: 0x10   // @
PRIM_STOREW:: 0x11   // w! (16-bit)
PRIM_FETCHW:: 0x12   // w@ (16-bit)
PRIM_OUTB  :: 0x13   // ( val port -- )
PRIM_INB   :: 0x14   // ( port -- val )
PRIM_OUTW  :: 0x15   // ( val port -- )
PRIM_INW   :: 0x16   // ( port -- val )
PRIM_EXIT  :: 0xFF

forth_push :: proc(vm: ^ForthVM, val: i64) {
    vm.dstack[vm.dsp] = val
    vm.dsp += 1
}

forth_pop :: proc(vm: ^ForthVM) -> i64 {
    vm.dsp -= 1
    return vm.dstack[vm.dsp]
}

forth_run_prim :: proc(vm: ^ForthVM, prim: u8) -> bool {
    switch prim {
    case PRIM_ADD:    b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a + b)
    case PRIM_SUB:    b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a - b)
    case PRIM_MUL:    b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a * b)
    case PRIM_AND:    b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a & b)
    case PRIM_OR:     b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a | b)
    case PRIM_XOR:    b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a ~ b)
    case PRIM_RSHIFT: b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a >> u64(b))
    case PRIM_LSHIFT: b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, a << u64(b))
    case PRIM_DUP:    a := forth_pop(vm); forth_push(vm, a); forth_push(vm, a)
    case PRIM_DROP:   forth_pop(vm)
    case PRIM_SWAP:   b := forth_pop(vm); a := forth_pop(vm); forth_push(vm, b); forth_push(vm, a)
    case PRIM_OVER:
        b := forth_pop(vm); a := forth_pop(vm)
        forth_push(vm, a); forth_push(vm, b); forth_push(vm, a)
    case PRIM_ROT:
        c := forth_pop(vm); b := forth_pop(vm); a := forth_pop(vm)
        forth_push(vm, b); forth_push(vm, c); forth_push(vm, a)
    case PRIM_STORE:
        addr := forth_pop(vm); val := forth_pop(vm)
        (cast(^i64)uintptr(addr))^ = val
    case PRIM_FETCH:
        addr := forth_pop(vm)
        forth_push(vm, (cast(^i64)uintptr(addr))^)
    case PRIM_STOREW:
        addr := forth_pop(vm); val := forth_pop(vm)
        (cast(^u16)uintptr(addr))^ = u16(val)
    case PRIM_FETCHW:
        addr := forth_pop(vm)
        forth_push(vm, i64((cast(^u16)uintptr(addr))^))
    case PRIM_OUTB:   port := u16(forth_pop(vm)); val := u8(forth_pop(vm)); outb(port, val)
    case PRIM_INB:    port := u16(forth_pop(vm)); forth_push(vm, i64(inb(port)))
    case PRIM_OUTW:   port := u16(forth_pop(vm)); val := u16(forth_pop(vm)); outw(port, val)
    case PRIM_INW:    port := u16(forth_pop(vm)); forth_push(vm, i64(inw(port)))
    case PRIM_EXIT:   return false
    }
    return true
}

forth_step :: proc(vm: ^ForthVM) -> bool {
    if int(vm.ip) >= DICT_SIZE { return false }
    op := vm.dict[vm.ip]
    vm.ip += 1
    switch op {
    case FORTH_LIT:
        val := (cast(^i64)&vm.dict[vm.ip])^
        vm.ip += 8
        forth_push(vm, val)
    case FORTH_CALL:
        addr := (cast(^u16)&vm.dict[vm.ip])^
        vm.ip += 2
        vm.rstack[vm.rsp] = vm.ip
        vm.rsp += 1
        vm.ip = addr
    case FORTH_RET:
        if vm.rsp == 0 { return false }
        vm.rsp -= 1
        vm.ip = vm.rstack[vm.rsp]
    case FORTH_PRIM:
        prim := vm.dict[vm.ip]
        vm.ip += 1
        if !forth_run_prim(vm, prim) { return false }
    case FORTH_BRANCH:
        target := (cast(^u16)&vm.dict[vm.ip])^
        vm.ip = target
    case FORTH_0BRANCH:
        target := (cast(^u16)&vm.dict[vm.ip])^
        vm.ip += 2
        cond := forth_pop(vm)
        if cond == 0 { vm.ip = target }
    case:
        return false
    }
    return true
}

forth_exec :: proc(vm: ^ForthVM, word_addr: u16) {
    vm.ip  = word_addr
    vm.rsp = 0
    for forth_step(vm) {}
}

// ── ATA Forth word addresses (resolved after loading ata.forth bytecode) ──────

ATAWords :: struct {
    reset:        u16,
    identify:     u16,
    read_sector:  u16,
    write_sector: u16,
    read_sectors: u16,
    init:         u16,
}

// ── DriveInfo — populated by IDENTIFY ────────────────────────────────────────

DriveInfo :: struct {
    present:      bool,
    lba28_sectors: u32,   // from IDENTIFY words 60-61
    lba48_sectors: u64,   // from IDENTIFY words 100-103 (if supported)
    supports_lba48: bool,
    model:        [41]u8, // from IDENTIFY words 27-46, swapped bytes
    sector_size:  u32,    // always 512 for now
}

// ── Global driver state ───────────────────────────────────────────────────────

DriverState :: struct {
    vm:           ForthVM,
    words:        ATAWords,
    drive:        [2]DriveInfo,   // [0]=master [1]=slave
    identify_buf: [512]u8,
    active_drive: u8,             // 0=master 1=slave
    initialized:  bool,
}

@(private)
g_state: DriverState

// ── IDENTIFY parser ───────────────────────────────────────────────────────────

@(private)
parse_identify :: proc(buf: []u8, info: ^DriveInfo) {
    if len(buf) < 512 { return }

    // Words are little-endian u16; byte-swap model string (ATA quirk)
    for i in 0..<40 {
        info.model[i] = buf[54 + (i ~ 1)] // words 27-46, byte-swapped
    }
    info.model[40] = 0

    // LBA28 total sectors: words 60-61
    lo := u32((cast(^u16)&buf[120])^)  // word 60
    hi := u32((cast(^u16)&buf[122])^)  // word 61
    info.lba28_sectors = (hi << 16) | lo

    // Check LBA48 support: word 83 bit 10
    w83 := (cast(^u16)&buf[166])^
    info.supports_lba48 = (w83 & 0x0400) != 0

    if info.supports_lba48 {
        // LBA48 total sectors: words 100-103
        w100 := u64((cast(^u16)&buf[200])^)
        w101 := u64((cast(^u16)&buf[202])^)
        w102 := u64((cast(^u16)&buf[204])^)
        w103 := u64((cast(^u16)&buf[206])^)
        info.lba48_sectors = w100 | (w101 << 16) | (w102 << 32) | (w103 << 48)
    }

    info.sector_size = 512
    info.present     = info.lba28_sectors > 0 || info.lba48_sectors > 0
}

// ── ATA soft reset (direct — bypasses Forth for reliability at init) ──────────

@(private)
ata_reset_direct :: proc() {
    outb(0x3F6, 0x04) // assert SRST
    // ~5µs delay via repeated inb
    for _ in 0..<1000 { inb(0x3F6) }
    outb(0x3F6, 0x00) // clear SRST
    for _ in 0..<1000 { inb(0x3F6) }
    // Poll BSY clear (max 30s per ATA spec, we do 1M iterations)
    for _ in 0..<1_000_000 {
        s := inb(0x1F7)
        if (s & 0x80) == 0 { break }
    }
}

// ── Poll helpers (direct) ─────────────────────────────────────────────────────

ATA_TIMEOUT :: 1_000_000

@(private)
ata_poll_bsy :: proc() -> bool {
    for _ in 0..<ATA_TIMEOUT {
        s := inb(0x1F7)
        if (s & 0x80) == 0 {
            return (s & 0x01) == 0  // false = error
        }
    }
    return false // timeout
}

@(private)
ata_poll_drq :: proc() -> bool {
    for _ in 0..<ATA_TIMEOUT {
        s := inb(0x1F7)
        if (s & 0x80) != 0 { continue }   // still busy
        if (s & 0x01) != 0 { return false } // error
        if (s & 0x08) != 0 { return true }  // DRQ set
    }
    return false
}

// ── IDENTIFY (direct) ─────────────────────────────────────────────────────────

@(private)
ata_identify_direct :: proc(drive: u8, buf: []u8) -> bool {
    sel: u8 = 0xE0 if drive == 0 else 0xF0
    outb(0x1F6, sel)
    // 400ns delay
    for _ in 0..<4 { inb(0x3F6) }

    outb(0x1F2, 0)
    outb(0x1F3, 0)
    outb(0x1F4, 0)
    outb(0x1F5, 0)
    outb(0x1F7, 0xEC) // IDENTIFY

    s := inb(0x1F7)
    if s == 0 { return false }

    if !ata_poll_drq() { return false }

    for i in 0..<256 {
        w := inw(0x1F0)
        buf[i*2]     = u8(w & 0xFF)
        buf[i*2 + 1] = u8(w >> 8)
    }
    return true
}

// ── Sector read (direct, LBA28) ───────────────────────────────────────────────

@(private)
ata_read_sector_direct :: proc(lba: u32, drive: u8, buf: []u8) -> bool {
    if len(buf) < 512 { return false }

    sel := u8((0xE0 if drive == 0 else 0xF0) | u8((lba >> 24) & 0x0F))
    outb(0x1F6, sel)
    for _ in 0..<4 { inb(0x3F6) }

    outb(0x1F2, 1)
    outb(0x1F3, u8(lba & 0xFF))
    outb(0x1F4, u8((lba >> 8) & 0xFF))
    outb(0x1F5, u8((lba >> 16) & 0xFF))
    outb(0x1F7, 0x20) // READ SECTORS

    if !ata_poll_drq() { return false }

    for i in 0..<256 {
        w := inw(0x1F0)
        buf[i*2]     = u8(w & 0xFF)
        buf[i*2 + 1] = u8(w >> 8)
    }
    return true
}

// ── Sector write (direct, LBA28) ─────────────────────────────────────────────

@(private)
ata_write_sector_direct :: proc(lba: u32, drive: u8, buf: []u8) -> bool {
    if len(buf) < 512 { return false }

    sel := u8((0xE0 if drive == 0 else 0xF0) | u8((lba >> 24) & 0x0F))
    outb(0x1F6, sel)
    for _ in 0..<4 { inb(0x3F6) }

    outb(0x1F2, 1)
    outb(0x1F3, u8(lba & 0xFF))
    outb(0x1F4, u8((lba >> 8) & 0xFF))
    outb(0x1F5, u8((lba >> 16) & 0xFF))
    outb(0x1F7, 0x30) // WRITE SECTORS

    if !ata_poll_drq() { return false }

    for i in 0..<256 {
        lo := buf[i*2]
        hi := buf[i*2 + 1]
        outw(0x1F0, u16(lo) | (u16(hi) << 8))
    }

    // Flush write cache
    outb(0x1F7, 0xE7)
    ata_poll_bsy()
    return true
}

// ── Public API (called from block.zig via FFI) ────────────────────────────────

// Initialize driver: reset, probe both drives
@(export)
disk_driver_init :: proc "c" () -> bool {
    ata_reset_direct()

    // Probe master (drive 0)
    if ata_identify_direct(0, g_state.identify_buf[:]) {
        parse_identify(g_state.identify_buf[:], &g_state.drive[0])
        if g_state.drive[0].present {
            debug_print("[ata] master present\n")
        }
    }

    // Probe slave (drive 1)
    if ata_identify_direct(1, g_state.identify_buf[:]) {
        parse_identify(g_state.identify_buf[:], &g_state.drive[1])
        if g_state.drive[1].present {
            debug_print("[ata] slave present\n")
        }
    }

    g_state.active_drive = 0
    g_state.initialized  = g_state.drive[0].present || g_state.drive[1].present
    return g_state.initialized
}

// Total sectors on active drive
@(export)
disk_driver_sector_count :: proc "c" () -> u64 {
    d := &g_state.drive[g_state.active_drive]
    if d.supports_lba48 { return d.lba48_sectors }
    return u64(d.lba28_sectors)
}

@(export)
disk_driver_sector_size :: proc "c" () -> u32 {
    return 512
}

// Read `count` sectors starting at `lba` into `buf`.
// buf must be count * 512 bytes.
@(export)
disk_driver_read :: proc "c" (lba: u64, count: u32, buf: [^]u8) -> bool {
    if !g_state.initialized { return false }
    drive := g_state.active_drive
    for i in u32(0)..<count {
        off := uintptr(i) * 512
        sector_buf := buf[off:off+512]
        if !ata_read_sector_direct(u32(lba) + i, drive, sector_buf) {
            debug_print("[ata] read error\n")
            return false
        }
    }
    return true
}

// Write `count` sectors starting at `lba` from `buf`.
@(export)
disk_driver_write :: proc "c" (lba: u64, count: u32, buf: [^]u8) -> bool {
    if !g_state.initialized { return false }
    drive := g_state.active_drive
    for i in u32(0)..<count {
        off := uintptr(i) * 512
        sector_buf := buf[off:off+512]
        if !ata_write_sector_direct(u32(lba) + i, drive, sector_buf) {
            debug_print("[ata] write error\n")
            return false
        }
    }
    return true
}

// Select which drive to use (0=master, 1=slave)
@(export)
disk_driver_select :: proc "c" (drive: u8) -> bool {
    if drive > 1 || !g_state.drive[drive].present { return false }
    g_state.active_drive = drive
    return true
}

// Run a Forth word by name — used for debug/REPL from kterminal
@(export)
disk_driver_forth_exec :: proc "c" (word_addr: u16) {
    forth_exec(&g_state.vm, word_addr)
}
