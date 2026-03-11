// CosinusOS — display/otg.rs
// OTG (Output Timing Controller) + AUX channel + DP link training + HPD
// Wspólne dla AMD DCN i Intel (Intel nazywa to "transcoder")

use crate::debug::{serial_print, serial_hex, num_str};

// ── MMIO helpers ──────────────────────────────────────────────────────────────
#[inline] pub unsafe fn r32(b:u64,o:u32)->u32 { core::ptr::read_volatile((b+o as u64) as *const u32) }
#[inline] pub unsafe fn w32(b:u64,o:u32,v:u32){ core::ptr::write_volatile((b+o as u64) as *mut u32,v) }
#[inline] pub unsafe fn w32m(b:u64,o:u32,mask:u32,v:u32){
    let old=r32(b,o); w32(b,o,(old&!mask)|(v&mask)); }

pub unsafe fn spinwait(base:u64,off:u32,mask:u32,want:u32,n:usize)->bool {
    for _ in 0..n {
        if r32(base,off)&mask==want { return true; }
        for _ in 0..800 { core::hint::spin_loop(); }
    }
    false
}

// ════════════════════════════════════════════════════════════════════════════
// § Timing struct — 1920×1080 @60Hz (CEA-861)
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct Timing {
    pub h_total:       u32, // 2200
    pub h_active:      u32, // 1920
    pub h_blank_start: u32, // 1920
    pub h_blank_end:   u32, // 2200
    pub h_sync_start:  u32, // 2008
    pub h_sync_end:    u32, // 2052
    pub h_sync_pos:    bool,
    pub v_total:       u32, // 1125
    pub v_active:      u32, // 1080
    pub v_blank_start: u32, // 1080
    pub v_blank_end:   u32, // 1125
    pub v_sync_start:  u32, // 1084
    pub v_sync_end:    u32, // 1089
    pub v_sync_pos:    bool,
    pub pixel_khz:     u32, // 148500
}

pub const T_1080P60: Timing = Timing {
    h_total:2200, h_active:1920, h_blank_start:1920, h_blank_end:2200,
    h_sync_start:2008, h_sync_end:2052, h_sync_pos:true,
    v_total:1125, v_active:1080, v_blank_start:1080, v_blank_end:1125,
    v_sync_start:1084, v_sync_end:1089, v_sync_pos:true,
    pixel_khz: 148_500,
};

// ════════════════════════════════════════════════════════════════════════════
// § AMD OTG (Output Timing Generator) — DCN2/DCN3
// ════════════════════════════════════════════════════════════════════════════

// Offsets relatywne do OTG_BASE (w bajtach)
// Źródło: amdgpu dcn20_optc.c
const OTG_H_TOTAL:           u32 = 0x00 * 4;
const OTG_H_BLANK_START_END: u32 = 0x01 * 4;
const OTG_H_SYNC_A:          u32 = 0x02 * 4;
const OTG_H_SYNC_A_CNTL:     u32 = 0x03 * 4;
const OTG_V_TOTAL:           u32 = 0x06 * 4;
const OTG_V_BLANK_START_END: u32 = 0x07 * 4;
const OTG_V_SYNC_A:          u32 = 0x08 * 4;
const OTG_V_SYNC_A_CNTL:     u32 = 0x09 * 4;
const OTG_INTERLACE_CONTROL: u32 = 0x0C * 4;
const OTG_BLACK_COLOR:       u32 = 0x0D * 4;
const OTG_CONTROL:           u32 = 0x0B * 4;
const OTG_MASTER_EN:         u32 = 1 << 0;

pub unsafe fn amd_otg_program(mmio: u64, otg_base: u32, t: &Timing) {
    let b = mmio + otg_base as u64;

    // Wyłącz OTG przed zmianą timingów
    w32m(mmio, otg_base + OTG_CONTROL, OTG_MASTER_EN, 0);

    w32(mmio, otg_base + OTG_H_TOTAL,           t.h_total - 1);
    w32(mmio, otg_base + OTG_H_BLANK_START_END, (t.h_blank_end << 16) | t.h_blank_start);
    w32(mmio, otg_base + OTG_H_SYNC_A,          (t.h_sync_end << 16) | t.h_sync_start);
    w32(mmio, otg_base + OTG_H_SYNC_A_CNTL,     if t.h_sync_pos { 0 } else { 1 });

    w32(mmio, otg_base + OTG_V_TOTAL,           t.v_total - 1);
    w32(mmio, otg_base + OTG_V_BLANK_START_END, (t.v_blank_end << 16) | t.v_blank_start);
    w32(mmio, otg_base + OTG_V_SYNC_A,          (t.v_sync_end << 16) | t.v_sync_start);
    w32(mmio, otg_base + OTG_V_SYNC_A_CNTL,     if t.v_sync_pos { 0 } else { 1 });

    w32(mmio, otg_base + OTG_INTERLACE_CONTROL, 0); // progressive
    w32(mmio, otg_base + OTG_BLACK_COLOR,       0);

    serial_print("[OTG] timing programmed 1920x1080@60\n");
}

pub unsafe fn amd_otg_enable(mmio: u64, otg_base: u32) {
    w32m(mmio, otg_base + OTG_CONTROL, OTG_MASTER_EN, OTG_MASTER_EN);
    serial_print("[OTG] enabled\n");
}

pub unsafe fn amd_otg_disable(mmio: u64, otg_base: u32) {
    w32m(mmio, otg_base + OTG_CONTROL, OTG_MASTER_EN, 0);
}

// ════════════════════════════════════════════════════════════════════════════
// § HPD — Hot Plug Detect
// ════════════════════════════════════════════════════════════════════════════

// AMD HPD0 base offset
const HPD0_BASE:     u32 = 0x4A80 * 4;
const HPD_INT_CTRL:  u32 = 0x00;
const HPD_INT_STAT:  u32 = 0x04;
const HPD_SENSE_BIT: u32 = 1 << 1;
const HPD_PORT_STEP: u32 = 0x20; // każdy port HPD co 0x20 DWORD-ów

pub unsafe fn amd_hpd_sense(mmio: u64, port: usize) -> bool {
    let off = HPD0_BASE + port as u32 * HPD_PORT_STEP + HPD_INT_STAT;
    r32(mmio, off) & HPD_SENSE_BIT != 0
}

// ════════════════════════════════════════════════════════════════════════════
// § AUX channel — DisplayPort DPCD transactions
// ════════════════════════════════════════════════════════════════════════════

// AMD AUX0 base offset (instancja 0)
const AUX0_BASE:    u32 = 0x5000 * 4;
const AUX_CONTROL:  u32 = 0x00;
const AUX_SW_DATA:  u32 = 0x04; // data FIFO (32-bit entries)
const AUX_SW_STAT:  u32 = 0x08;
const AUX_DONE:     u32 = 1 << 1;
const AUX_NACK:     u32 = 1 << 2;
const AUX_START:    u32 = 1 << 0;

/// Wyślij AUX native transaction (read lub write, max 16B)
pub unsafe fn aux_native(mmio: u64, aux_base: u32, addr: u32,
                          write: bool, data: &mut [u8]) -> usize {
    let len = data.len().min(16) as u32;
    // cmd: 0x9=READ 0x8=WRITE
    let cmd: u32 = if write { 0x8 } else { 0x9 };
    // request word: [31:28]=cmd [27:8]=addr [7:4]=0 [3:0]=len-1
    let req = (cmd << 28) | ((addr & 0xF_FFFF) << 8) | (len - 1);
    w32(mmio, aux_base + AUX_SW_DATA, req);

    if write {
        for (i, &b) in data[..len as usize].iter().enumerate() {
            w32(mmio, aux_base + AUX_SW_DATA + (i as u32 + 1)*4, b as u32);
        }
    }

    // Uruchom
    let ctrl = r32(mmio, aux_base + AUX_CONTROL);
    w32(mmio, aux_base + AUX_CONTROL, ctrl | AUX_START);

    if !spinwait(mmio, aux_base + AUX_SW_STAT, AUX_DONE, AUX_DONE, 10_000) {
        serial_print("[AUX] timeout\n"); return 0;
    }
    if r32(mmio, aux_base + AUX_SW_STAT) & AUX_NACK != 0 { return 0; }

    if !write {
        for i in 0..len as usize {
            let word = r32(mmio, aux_base + AUX_SW_DATA + (i as u32 + 1)*4);
            data[i] = word as u8;
        }
    }
    len as usize
}

#[inline]
pub unsafe fn dpcd_read(mmio: u64, aux_base: u32, addr: u32) -> u8 {
    let mut b = [0u8; 1];
    aux_native(mmio, aux_base, addr, false, &mut b);
    b[0]
}

#[inline]
pub unsafe fn dpcd_write(mmio: u64, aux_base: u32, addr: u32, val: u8) {
    let mut b = [val];
    aux_native(mmio, aux_base, addr, true, &mut b);
}

// ════════════════════════════════════════════════════════════════════════════
// § DisplayPort Link Training — TPS1 (Clock Recovery) + TPS2 (EQ)
// ════════════════════════════════════════════════════════════════════════════

// DPCD adresy
const DPCD_REV:            u32 = 0x0000;
const DPCD_MAX_LINK_RATE:  u32 = 0x0001;
const DPCD_MAX_LANE_COUNT: u32 = 0x0002;
const DPCD_LINK_BW_SET:    u32 = 0x0100;
const DPCD_LANE_COUNT_SET: u32 = 0x0101;
const DPCD_TRAINING_PAT:   u32 = 0x0102;
const DPCD_LANE0_1_STATUS: u32 = 0x0202;
const DPCD_LANE_ALIGN:     u32 = 0x0204;

pub unsafe fn dp_link_train(mmio: u64, aux_base: u32) -> bool {
    let rev  = dpcd_read(mmio, aux_base, DPCD_REV);
    if rev == 0 { serial_print("[DP] no sink\n"); return false; }

    let max_rate  = dpcd_read(mmio, aux_base, DPCD_MAX_LINK_RATE);
    let max_lanes = dpcd_read(mmio, aux_base, DPCD_MAX_LANE_COUNT) & 0x1F;
    let rate      = max_rate.min(0x14);  // max HBR2 (5.4 Gbps)
    let lanes     = max_lanes.min(4);

    serial_print("[DP] DPCD rev="); serial_hex(rev as u64);
    serial_print(" rate="); serial_hex(rate as u64);
    unsafe { serial_print(" lanes="); let mut b=[0u8;24]; serial_print(num_str(lanes as usize, &mut b)); }
    serial_print("\n");

    dpcd_write(mmio, aux_base, DPCD_LINK_BW_SET,   rate);
    dpcd_write(mmio, aux_base, DPCD_LANE_COUNT_SET, lanes | (1<<7)); // Enhanced Framing

    // TPS1 — Clock Recovery
    dpcd_write(mmio, aux_base, DPCD_TRAINING_PAT, 0x21); // TPS1 + scramble off
    let mut vswing = [0x00u8; 4];
    aux_native(mmio, aux_base, 0x0103, true, &mut vswing[..lanes as usize]);

    let mut cr_ok = false;
    for _ in 0..5 {
        for _ in 0..10_000 { core::hint::spin_loop(); }
        let st   = dpcd_read(mmio, aux_base, DPCD_LANE0_1_STATUS);
        let mask = if lanes >= 2 { 0x11u8 } else { 0x01u8 };
        if st & mask == mask { cr_ok = true; break; }
    }
    if !cr_ok { serial_print("[DP] CR failed\n"); return false; }

    // TPS2 — Channel Equalization
    dpcd_write(mmio, aux_base, DPCD_TRAINING_PAT, 0x22);
    let mut eq_ok = false;
    for _ in 0..5 {
        for _ in 0..10_000 { core::hint::spin_loop(); }
        let st    = dpcd_read(mmio, aux_base, DPCD_LANE0_1_STATUS);
        let align = dpcd_read(mmio, aux_base, DPCD_LANE_ALIGN);
        let mask  = if lanes >= 2 { 0x77u8 } else { 0x07u8 };
        if st & mask == mask && align & 1 != 0 { eq_ok = true; break; }
    }

    dpcd_write(mmio, aux_base, DPCD_TRAINING_PAT, 0x00); // wyłącz TPS

    if eq_ok { serial_print("[DP] link training OK\n"); }
    else      { serial_print("[DP] EQ failed\n"); }
    eq_ok
}
