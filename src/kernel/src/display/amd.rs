// CosinusOS — display/amd.rs
// AMD DCN 2.x / DCN 3.x — HUBP, DIG encoder, DCCG, SMU power-up
// Źródła: amdgpu open source (MIT license), AMD GPU register reference

use crate::debug::{serial_print, serial_hex};
use super::otg::{r32, w32, w32m, spinwait, Timing, T_1080P60,
                  amd_otg_program, amd_otg_enable, amd_hpd_sense,
                  dp_link_train};
use super::fb::{FB_PHYS, FB_W, FB_H, FB_BPP};
use crate::mm::{vmap, PAGE_SIZE, PTE_W, K_P4};

// ════════════════════════════════════════════════════════════════════════════
// § PCI autodetect — AMD GPU Device IDs → DCN generacja
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum DcnGen { Dcn2, Dcn3 }

pub const AMD_VENDOR: u16 = 0x1002;

pub fn dcn_gen_from_did(did: u16) -> Option<DcnGen> {
    match did {
        // DCN 2.0 — Navi10 (RX 5700/5700XT)
        0x7310..=0x731F => Some(DcnGen::Dcn2),
        // DCN 2.0 — Navi14 (RX 5500/5600)
        0x7340..=0x7347 => Some(DcnGen::Dcn2),
        // DCN 2.1 — Renoir iGPU (Ryzen 4000)
        0x1636          => Some(DcnGen::Dcn2),
        // DCN 2.1 — Cezanne iGPU (Ryzen 5000)
        0x1638          => Some(DcnGen::Dcn2),
        // DCN 2.1 — Lucienne (Ryzen 5000U)
        0x164C          => Some(DcnGen::Dcn2),
        // DCN 2.1 — Van Gogh (Steam Deck)
        0x163F          => Some(DcnGen::Dcn2),
        // DCN 3.0 — Navi21 (RX 6800/6900)
        0x73BF | 0x73A5 => Some(DcnGen::Dcn3),
        // DCN 3.0 — Navi22 (RX 6700 XT)
        0x73DF          => Some(DcnGen::Dcn3),
        // DCN 3.0 — Navi23 (RX 6600 XT)
        0x73FF | 0x73E3 => Some(DcnGen::Dcn3),
        // DCN 3.0 — Navi24 (RX 6400/6500)
        0x7422..=0x7423 => Some(DcnGen::Dcn3),
        // DCN 3.1 — Rembrandt iGPU (Ryzen 6000)
        0x1681 | 0x164D => Some(DcnGen::Dcn3),
        // DCN 3.1 — Rembrandt R (Ryzen 7035)
        0x164E          => Some(DcnGen::Dcn3),
        // DCN 3.1.4 — Phoenix iGPU (Ryzen 7040)
        0x15BF | 0x15C8 => Some(DcnGen::Dcn3),
        // DCN 3.5 — Phoenix2 / Hawk Point
        0x15D8 | 0x1900..=0x190F => Some(DcnGen::Dcn3),
        _               => None,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Register map — AMD DCN2/3 MMIO offsets
// Format: rejestr_index × 4 = byte offset od BAR0
// ════════════════════════════════════════════════════════════════════════════

// SMU mailbox
const mmMP1_C2PMSG_66: u32 = 0x0282 * 4; // message
const mmMP1_C2PMSG_82: u32 = 0x0292 * 4; // param
const mmMP1_C2PMSG_90: u32 = 0x0298 * 4; // response

// DCCG base (Display Clock Generator)
const DCCG_BASE:          u32 = 0x1B00 * 4;
const DCCG_SOFT_RESET:    u32 = DCCG_BASE + 0x00 * 4;
const DCCG_GATE_DISABLE:  u32 = DCCG_BASE + 0x01 * 4;

// HUBP0 base (pixel fetcher instancja 0)
const HUBP0_BASE:          u32 = 0x055A * 4;
const HUBP_ENABLE_OFF:     u32 = 0x00;
const HUBP_ADDR_LO:        u32 = 0x08;
const HUBP_ADDR_HI:        u32 = 0x0C;
const HUBP_PITCH:          u32 = 0x10; // w jednostkach 256B
const HUBP_FMT:            u32 = 0x14; // 0x08 = XRGB8888
const HUBP_VP_WIDTH:       u32 = 0x18;
const HUBP_VP_HEIGHT:      u32 = 0x1C;
const HUBP_CURSOR_EN_OFF:  u32 = 0x40;

// OTG0 base — DCN2 i DCN3 mają różne offsety
const OTG0_BASE_DCN2: u32 = 0x1C00 * 4;
const OTG0_BASE_DCN3: u32 = (0x1C00 + 0x0100) * 4;

// DIO base (Display I/O — encoder)
const DIO_BASE:           u32 = 0x4A00 * 4;
const DIG0_ENC_CTRL:      u32 = DIO_BASE + 0x02 * 4;
const DIG0_HDMI_CTRL:     u32 = DIO_BASE + 0x28 * 4;
const DIG0_HDMI_GC:       u32 = DIO_BASE + 0x29 * 4;
const DIG0_DP_CONFIG:     u32 = DIO_BASE + 0x40 * 4;
const DIG0_DP_VID_CTRL:   u32 = DIO_BASE + 0x41 * 4;
const DIG0_DP_MSA_MISC:   u32 = DIO_BASE + 0x44 * 4;
const DIG0_AFMT_CTRL:     u32 = DIO_BASE + 0x10 * 4;
const DIG_ENABLE_BIT:     u32 = 1 << 0;
const DIG_HDMI_MODE:      u32 = 0 << 1;
const DIG_DP_MODE:        u32 = 1 << 1;

// AUX0 channel base
pub const AUX0_BASE: u32 = 0x5000 * 4;

// ════════════════════════════════════════════════════════════════════════════
// § SMU — power-up display engine
// ════════════════════════════════════════════════════════════════════════════

const SMU_MSG_ENABLE_FEATURES: u32 = 0x02;
const SMU_MSG_DISPLAY_CONFIG:  u32 = 0x09;

unsafe fn smu_send(mmio: u64, msg: u32, param: u32) -> u32 {
    w32(mmio, mmMP1_C2PMSG_90, 0);
    w32(mmio, mmMP1_C2PMSG_82, param);
    w32(mmio, mmMP1_C2PMSG_66, msg);
    for _ in 0..100_000 {
        let r = r32(mmio, mmMP1_C2PMSG_90);
        if r != 0 { return r; }
        core::hint::spin_loop();
    }
    serial_print("[SMU] timeout\n"); 0
}

pub unsafe fn amd_power_up_display(mmio: u64) {
    smu_send(mmio, SMU_MSG_ENABLE_FEATURES, 0xFFFF_FFFF);
    smu_send(mmio, SMU_MSG_DISPLAY_CONFIG,  0x01); // 1 active display
    serial_print("[AMD] SMU display power OK\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § DCCG — Display Clock Generator init
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn amd_dccg_init(mmio: u64) {
    w32(mmio, DCCG_SOFT_RESET,   1);
    for _ in 0..2000 { core::hint::spin_loop(); }
    w32(mmio, DCCG_SOFT_RESET,   0);
    w32(mmio, DCCG_GATE_DISABLE, 0xFFFF_FFFF); // disable clock gating during init
    serial_print("[AMD] DCCG OK\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § HUBP — HW Underlay/Blend Pipe (pixel fetcher)
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn amd_hubp_program(mmio: u64, fb_phys: u64, w: u32, h: u32) {
    let b = HUBP0_BASE;
    let pitch = (w * FB_BPP as u32) / 256; // pitch w jednostkach 256B

    w32(mmio, b + HUBP_ADDR_LO,      fb_phys as u32);
    w32(mmio, b + HUBP_ADDR_HI,      (fb_phys >> 32) as u32);
    w32(mmio, b + HUBP_PITCH,        pitch);
    w32(mmio, b + HUBP_FMT,          0x08); // XRGB8888
    w32(mmio, b + HUBP_VP_WIDTH,     w);
    w32(mmio, b + HUBP_VP_HEIGHT,    h);
    w32(mmio, b + HUBP_CURSOR_EN_OFF, 0);   // software cursor
    w32(mmio, b + HUBP_ENABLE_OFF,   1);

    serial_print("[AMD] HUBP fb="); serial_hex(fb_phys);
    serial_print("\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § DIG encoder — HDMI lub DisplayPort
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone, PartialEq)]
pub enum OutMode { Hdmi, Dp }

pub unsafe fn amd_dig_program(mmio: u64, mode: OutMode) {
    // Wyłącz encoder
    w32(mmio, DIG0_ENC_CTRL, 0);
    w32(mmio, DIG0_AFMT_CTRL, 0); // audio off

    match mode {
        OutMode::Hdmi => {
            // HDMI 24bpp, clock × 1
            w32(mmio, DIG0_HDMI_CTRL, 0x0001_0001);
            w32(mmio, DIG0_HDMI_GC,   0); // AV mute off
            w32(mmio, DIG0_ENC_CTRL,  DIG_ENABLE_BIT | DIG_HDMI_MODE);
            serial_print("[AMD] DIG → HDMI\n");
        }
        OutMode::Dp => {
            // 4 lanes, enhanced framing, 24bpp RGB
            w32(mmio, DIG0_DP_CONFIG,  3 | (1<<4));   // lanes=4, enhanced
            w32(mmio, DIG0_DP_MSA_MISC, 0x20);        // MISC0: 8bpc RGB
            w32(mmio, DIG0_DP_VID_CTRL, 1);           // video stream enable
            w32(mmio, DIG0_ENC_CTRL,   DIG_ENABLE_BIT | DIG_DP_MODE);
            serial_print("[AMD] DIG → DP\n");
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § BAR mapping helper
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn map_bar(base: u64, pages: usize) {
    for i in 0..pages {
        let a = base + i as u64 * PAGE_SIZE as u64;
        vmap(K_P4, a, a, PTE_W);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Główna inicjalizacja AMD
// ════════════════════════════════════════════════════════════════════════════

pub struct AmdState {
    pub mmio:    u64,
    pub gen:     DcnGen,
    pub mode:    OutMode,
    pub active:  bool,
}

pub unsafe fn amd_display_init(mmio: u64, gen: DcnGen) -> Option<AmdState> {
    // Mapuj BAR0 (8MB = 2048 stron)
    map_bar(mmio, 2048);

    serial_print("[AMD] DCN"); serial_print(match gen { DcnGen::Dcn2=>"2.x", DcnGen::Dcn3=>"3.x"});
    serial_print(" mmio="); serial_hex(mmio); serial_print("\n");

    // Power up
    amd_power_up_display(mmio);

    // Clock generator
    amd_dccg_init(mmio);

    // HUBP — pixel fetcher (wymaga FB_PHYS już ustawionego)
    amd_hubp_program(mmio, FB_PHYS, FB_W as u32, FB_H as u32);

    // OTG base zależy od generacji
    let otg_base = if gen == DcnGen::Dcn3 { OTG0_BASE_DCN3 } else { OTG0_BASE_DCN2 };

    // HPD — sprawdź który kabel podłączony (port 0 = DP/HDMI)
    let hpd0 = amd_hpd_sense(mmio, 0);
    let hpd1 = amd_hpd_sense(mmio, 1);
    serial_print("[AMD] HPD0="); serial_hex(hpd0 as u64);
    serial_print(" HPD1=");      serial_hex(hpd1 as u64);
    serial_print("\n");

    // Preferuj DP (port 0), fallback HDMI
    let mode = if hpd0 && dp_link_train(mmio, AUX0_BASE) {
        OutMode::Dp
    } else {
        OutMode::Hdmi
    };

    // DIG encoder
    amd_dig_program(mmio, mode);

    // OTG timing + enable
    amd_otg_program(mmio, otg_base, &T_1080P60);
    amd_otg_enable(mmio, otg_base);

    Some(AmdState { mmio, gen, mode, active: true })
}

/// Hotplug check — wywołaj co kilka sekund
pub unsafe fn amd_hotplug(state: &mut AmdState) {
    let otg_base = if state.gen == DcnGen::Dcn3 { OTG0_BASE_DCN3 } else { OTG0_BASE_DCN2 };
    let hpd = amd_hpd_sense(state.mmio, 0);

    if hpd && !state.active {
        serial_print("[AMD] hotplug: reconnect\n");
        if dp_link_train(state.mmio, AUX0_BASE) {
            amd_dig_program(state.mmio, OutMode::Dp);
            amd_otg_program(state.mmio, otg_base, &T_1080P60);
            amd_otg_enable(state.mmio, otg_base);
            state.mode   = OutMode::Dp;
            state.active = true;
        }
    } else if !hpd && state.active && state.mode == OutMode::Dp {
        serial_print("[AMD] hotplug: disconnect\n");
        state.active = false;
    }
}
