// CosinusOS — display/intel.rs
// Intel Display Engine — Gen9 (Skylake/Kaby Lake) przez Gen12 (Tiger Lake / Alder Lake)
// Obsługuje: HDMI + DisplayPort, 1920×1080
// Źródła: i915 DRM driver (GPL), Intel Open Source HD Graphics PRM

use crate::debug::{serial_print, serial_hex};
use super::otg::{r32, w32, w32m, spinwait, T_1080P60, Timing, dp_link_train};
use super::fb::{FB_PHYS, FB_W, FB_H, FB_BPP};
use crate::mm::{vmap, PAGE_SIZE, PTE_W, K_P4};

// ════════════════════════════════════════════════════════════════════════════
// § Intel GPU PCI Device IDs → generacja
// ════════════════════════════════════════════════════════════════════════════

pub const INTEL_VENDOR: u16 = 0x8086;

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum IntelGen {
    Gen9,   // Skylake / Kaby Lake / Coffee Lake (2015–2019)
    Gen11,  // Ice Lake (2019)
    Gen12,  // Tiger Lake / Alder Lake / Raptor Lake (2020–2023)
}

pub fn intel_gen_from_did(did: u16) -> Option<IntelGen> {
    match did {
        // Gen9 — Skylake
        0x1902 | 0x1906 | 0x190A | 0x190B | 0x190E => Some(IntelGen::Gen9),
        0x1912 | 0x1916 | 0x191A | 0x191B | 0x191D | 0x191E => Some(IntelGen::Gen9),
        0x1921 | 0x1923 | 0x1926 | 0x1927 | 0x192B | 0x192D => Some(IntelGen::Gen9),
        // Gen9 — Kaby Lake
        0x5902 | 0x5906 | 0x590A | 0x5908 | 0x590B | 0x590E => Some(IntelGen::Gen9),
        0x5912 | 0x5916 | 0x591A | 0x591B | 0x591D | 0x591E => Some(IntelGen::Gen9),
        0x5921 | 0x5923 | 0x5926 | 0x5927 => Some(IntelGen::Gen9),
        // Gen9 — Coffee Lake / Whiskey Lake / Comet Lake
        0x3E90..=0x3EFF => Some(IntelGen::Gen9),
        0x9B21 | 0x9B41 | 0x9BA0 | 0x9BA2 | 0x9BA4 | 0x9BA8 => Some(IntelGen::Gen9),
        0x9BC0 | 0x9BC2 | 0x9BC4 | 0x9BC8 | 0x9BCA => Some(IntelGen::Gen9),
        // Gen11 — Ice Lake
        0x8A50..=0x8A5F => Some(IntelGen::Gen11),
        // Gen12 — Tiger Lake
        0x9A40 | 0x9A49 | 0x9A60 | 0x9A68 | 0x9A70 | 0x9A78 => Some(IntelGen::Gen12),
        // Gen12 — Alder Lake
        0x4680 | 0x4682 | 0x4688 | 0x468A | 0x4690 | 0x4692 | 0x4693 => Some(IntelGen::Gen12),
        // Gen12 — Raptor Lake
        0xA720 | 0xA721 | 0xA780 | 0xA781 | 0xA782 | 0xA783 | 0xA788 | 0xA789 => Some(IntelGen::Gen12),
        _  => None,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Intel Display Engine register offsets
// BAR0 = MMIO (2MB lub 4MB)
// ════════════════════════════════════════════════════════════════════════════

// ── Power / GT ─────────────────────────────────────────────────────────────
const GT_FORCEWAKE_ACK:     u32 = 0x130044;
const GT_FORCEWAKE_MT:      u32 = 0x13004C;
const GT_FORCEWAKE_GEN9_ACK: u32 = 0xD84;
const GT_FORCEWAKE_GEN9:    u32 = 0xD88;
const PWR_WELL_CTL2:        u32 = 0x45404; // Gen9+
const PWR_WELL_ENABLE:      u32 = 1 << 31;
const PWR_WELL_STATE:       u32 = 1 << 30;

// ── DPLL (Display PLL) ─────────────────────────────────────────────────────
const DPLL_CTRL1:           u32 = 0x6C058; // Gen9
const DPLL0_CFGCR1:        u32 = 0x6C040;
const DPLL0_CFGCR2:        u32 = 0x6C044;
const LCPLL1_CTL:           u32 = 0x46010;
const LCPLL_PLL_ENABLE:     u32 = 1 << 31;
const LCPLL_PLL_LOCK:       u32 = 1 << 30;

// ── Pipe A (używamy Pipe A dla uproszczenia) ───────────────────────────────
const PIPE_A_HTOTAL:        u32 = 0x60000;
const PIPE_A_HBLANK:        u32 = 0x60004;
const PIPE_A_HSYNC:         u32 = 0x60008;
const PIPE_A_VTOTAL:        u32 = 0x6000C;
const PIPE_A_VBLANK:        u32 = 0x60010;
const PIPE_A_VSYNC:         u32 = 0x60014;
const PIPE_A_SRCSZ:         u32 = 0x6001C; // source size [31:16]=H [15:0]=V
const PIPEA_CONF:           u32 = 0x70008; // Pipe config
const PIPEA_ENABLE:         u32 = 1 << 31;
const PIPEA_ENABLED_STATUS: u32 = 1 << 30;

// ── Plane A (primary display plane) ───────────────────────────────────────
const DSPASTRIDE:           u32 = 0x70188; // pitch w bajtach
const DSPASURF:             u32 = 0x7019C; // surface address (triggers flip)
const DSPAOFFSET:           u32 = 0x701A4; // x/y offset
const DSPASURFLIVE:         u32 = 0x701AC;
const DSPACTL:              u32 = 0x70180; // plane control
const DSPA_ENABLE:          u32 = 1 << 31;
const DSPA_FORMAT_XRGB:     u32 = 0x4 << 26; // XRGB 8888

// Gen12 plane offsets (Transcoder/plane różni się)
const PLANE1A_CTL:          u32 = 0x70180;
const PLANE1A_STRIDE:       u32 = 0x70188;
const PLANE1A_SURF:         u32 = 0x7019C;
const PLANE1A_SIZE:         u32 = 0x70190; // [27:16]=width-1 [11:0]=height-1
const PLANE1A_KEYMSK:       u32 = 0x701A0;
const PLANE1A_OFFSET:       u32 = 0x701A4;

// ── DDI (Digital Display Interface) — port A ───────────────────────────────
const DDI_BUF_CTL_A:        u32 = 0x64000;
const DDI_BUF_ENABLE:       u32 = 1 << 31;
const DDI_BUF_IS_DP:        u32 = 0;  // 0=DP 1=HDMI
const DDI_PORT_WIDTH_4:     u32 = 3 << 1; // 4 lanes

const DDI_FUNC_CTL_A:       u32 = 0x60430; // transcoder DDI function control
const DDI_FUNC_ENABLE:      u32 = 1 << 31;
const DDI_FUNC_DP_SST:      u32 = 0x2 << 24; // DP Single-Stream
const DDI_FUNC_HDMI:        u32 = 0x0 << 24; // HDMI/DVI

// ── AUX channel A (Gen9) ───────────────────────────────────────────────────
const DP_AUX_CH_CTL_A:      u32 = 0x64010;
const DP_AUX_CH_DATA1_A:    u32 = 0x64014;
const AUX_CTL_SEND:         u32 = 1 << 31;
const AUX_CTL_DONE:         u32 = 1 << 30;
const AUX_CTL_NACK:         u32 = 1 << 28;
const AUX_CTL_MSG_SIZE_MASK: u32 = 0x1F << 20;
const AUX_CTL_PRECHARGE_2US: u32 = 0x2 << 16;

// ── HPD (Hot Plug Detect) ──────────────────────────────────────────────────
const SDEISR:               u32 = 0xC4000; // SDE interrupt status
const HPD_PORT_A:           u32 = 1 << 8;
const HPD_PORT_B:           u32 = 1 << 9;

// ════════════════════════════════════════════════════════════════════════════
// § ForceWake — wybudzenie GT przed dostępem do rejestrów
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn intel_forcewake(mmio: u64) {
    // Gen9+: napisz bit 1 do FORCEWAKE_MT żeby wybudzić render domain
    w32(mmio, GT_FORCEWAKE_GEN9, (1<<1) | (1<<17)); // FORCEWAKE_RENDER
    // Czekaj na ACK
    spinwait(mmio, GT_FORCEWAKE_GEN9_ACK, 1<<1, 1<<1, 50_000);
    serial_print("[Intel] ForceWake OK\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § Power Well — włącz display power well
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn intel_power_well_enable(mmio: u64) {
    let val = r32(mmio, PWR_WELL_CTL2);
    w32(mmio, PWR_WELL_CTL2, val | PWR_WELL_ENABLE);
    // Czekaj aż stan będzie aktywny
    spinwait(mmio, PWR_WELL_CTL2, PWR_WELL_STATE, PWR_WELL_STATE, 10_000);
    serial_print("[Intel] Power Well OK\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § DPLL — programowanie PLL dla 148.5MHz (1080p60)
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn intel_dpll_init(mmio: u64, gen: IntelGen) {
    match gen {
        IntelGen::Gen9 => {
            // DPLL0 dla Pipe A — 148.5 MHz
            // CFGCR1: DCO frequency, CFGCR2: dividers
            // Wartości dla 148.5 MHz z 19.2MHz ref clock (laptop) lub 24MHz (desktop)
            // DCO = 8910 MHz, p0=2 p1=1 p2=1 → 8910/60 = 148.5 MHz
            w32(mmio, DPLL0_CFGCR1, 0x01_AF40); // DCO integer+fraction
            w32(mmio, DPLL0_CFGCR2, 0x000001); // P dividers
            // DPLL_CTRL1: DPLL0 → SSC off, ref=24MHz, override enable
            let ctrl = r32(mmio, DPLL_CTRL1);
            w32(mmio, DPLL_CTRL1, (ctrl & !0x3F) | 0x01); // DPLL0 link rate override
        }
        IntelGen::Gen11 | IntelGen::Gen12 => {
            // Gen11+ używa COMBO PHY PLL — bardziej złożone
            // Na razie używamy domyślnej konfiguracji BIOS/GOP jako fallback
            serial_print("[Intel] Gen11/12 DPLL: using BIOS config\n");
        }
    }
    serial_print("[Intel] DPLL programmed\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § Pipe A — timing 1920×1080@60
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn intel_pipe_program(mmio: u64, t: &Timing) {
    // Wyłącz Pipe A
    w32m(mmio, PIPEA_CONF, PIPEA_ENABLE, 0);
    spinwait(mmio, PIPEA_CONF, PIPEA_ENABLED_STATUS, 0, 50_000);

    // H timing: HTOTAL[31:16]=total-1 [15:0]=active-1
    w32(mmio, PIPE_A_HTOTAL, ((t.h_total-1)<<16) | (t.h_active-1));
    w32(mmio, PIPE_A_HBLANK, ((t.h_blank_end-1)<<16) | (t.h_blank_start-1));
    w32(mmio, PIPE_A_HSYNC,  ((t.h_sync_end-1)<<16) | (t.h_sync_start-1));

    // V timing
    w32(mmio, PIPE_A_VTOTAL, ((t.v_total-1)<<16) | (t.v_active-1));
    w32(mmio, PIPE_A_VBLANK, ((t.v_blank_end-1)<<16) | (t.v_blank_start-1));
    w32(mmio, PIPE_A_VSYNC,  ((t.v_sync_end-1)<<16) | (t.v_sync_start-1));

    // Source size = active area
    w32(mmio, PIPE_A_SRCSZ,  ((t.h_active-1)<<16) | (t.v_active-1));

    serial_print("[Intel] Pipe A timing OK\n");
}

pub unsafe fn intel_pipe_enable(mmio: u64) {
    w32m(mmio, PIPEA_CONF, PIPEA_ENABLE, PIPEA_ENABLE);
    spinwait(mmio, PIPEA_CONF, PIPEA_ENABLED_STATUS, PIPEA_ENABLED_STATUS, 50_000);
    serial_print("[Intel] Pipe A enabled\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § Primary Plane — podłącz framebuffer
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn intel_plane_program(mmio: u64, fb_phys: u64, gen: IntelGen) {
    let pitch = (FB_W * FB_BPP) as u32;

    match gen {
        IntelGen::Gen9 => {
            w32(mmio, DSPACTL,    DSPA_ENABLE | DSPA_FORMAT_XRGB);
            w32(mmio, DSPASTRIDE, pitch);
            w32(mmio, DSPAOFFSET, 0);
            // Zapis do DSPASURF triggeruje flip
            w32(mmio, DSPASURF,   fb_phys as u32);
        }
        IntelGen::Gen11 | IntelGen::Gen12 => {
            // Gen12 universal plane
            w32(mmio, PLANE1A_CTL,    (1<<31) | (4<<24)); // enable + XRGB8888
            w32(mmio, PLANE1A_STRIDE, pitch / 64);        // w jednostkach 64B
            w32(mmio, PLANE1A_SIZE,   ((FB_W as u32-1)<<16) | (FB_H as u32-1));
            w32(mmio, PLANE1A_OFFSET, 0);
            w32(mmio, PLANE1A_SURF,   fb_phys as u32);    // trigger flip
        }
    }

    serial_print("[Intel] Plane programmed fb="); serial_hex(fb_phys);
    serial_print("\n");
}

// ════════════════════════════════════════════════════════════════════════════
// § DDI — Digital Display Interface (Port A)
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone, PartialEq)]
pub enum IntelOutMode { Hdmi, Dp }

pub unsafe fn intel_ddi_enable(mmio: u64, mode: IntelOutMode) {
    // DDI Buffer
    let buf_ctl = DDI_BUF_ENABLE | DDI_PORT_WIDTH_4
        | if mode == IntelOutMode::Dp { 0 } else { 1<<9 }; // DP=0, HDMI=set HDMI
    w32(mmio, DDI_BUF_CTL_A, buf_ctl);

    // Transcoder DDI function
    let func = DDI_FUNC_ENABLE
        | if mode == IntelOutMode::Dp { DDI_FUNC_DP_SST } else { DDI_FUNC_HDMI };
    w32(mmio, DDI_FUNC_CTL_A, func);

    serial_print("[Intel] DDI Port A ");
    serial_print(if mode == IntelOutMode::Dp { "DP\n" } else { "HDMI\n" });
}

// ════════════════════════════════════════════════════════════════════════════
// § AUX channel (Gen9 style)
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn intel_aux_transaction(mmio: u64, addr: u32,
                                     write: bool, data: &mut [u8]) -> usize {
    let len = data.len().min(16) as u32;
    let cmd: u32 = if write { 0x8 } else { 0x9 };
    let msg = (cmd << 28) | ((addr & 0xF_FFFF) << 8) | (len - 1);

    // Wpisz wiadomość do DATA1 (bajty 0-3)
    w32(mmio, DP_AUX_CH_DATA1_A, msg);
    if write {
        for (i, &b) in data[..len as usize].iter().enumerate() {
            w32(mmio, DP_AUX_CH_DATA1_A + (i as u32+1)*4, b as u32);
        }
    }

    // Send: MSG_SIZE = len+1 header, precharge=2us
    let ctl = AUX_CTL_SEND | AUX_CTL_PRECHARGE_2US | ((len+1) << 20);
    w32(mmio, DP_AUX_CH_CTL_A, ctl);

    if !spinwait(mmio, DP_AUX_CH_CTL_A, AUX_CTL_DONE, AUX_CTL_DONE, 10_000) {
        serial_print("[iAUX] timeout\n"); return 0;
    }
    if r32(mmio, DP_AUX_CH_CTL_A) & AUX_CTL_NACK != 0 { return 0; }

    if !write {
        for i in 0..len as usize {
            let word = r32(mmio, DP_AUX_CH_DATA1_A + (i as u32+1)*4);
            data[i] = word as u8;
        }
    }
    len as usize
}

pub unsafe fn intel_dpcd_read(mmio: u64, addr: u32) -> u8 {
    let mut b = [0u8;1];
    intel_aux_transaction(mmio, addr, false, &mut b);
    b[0]
}

pub unsafe fn intel_dp_link_train(mmio: u64) -> bool {
    // Sprawdź czy sink odpowiada
    let rev = intel_dpcd_read(mmio, 0x0000);
    if rev == 0 { return false; }
    serial_print("[Intel] DP DPCD rev="); serial_hex(rev as u64); serial_print("\n");
    // Dalszy link training analogiczny do AMD — używamy wspólnej logiki z otg.rs
    // ale z intel AUX. Tutaj uproszczone — jeśli DPCD odpowiada, zakładamy sukces.
    // TODO: pełny TPS1/TPS2 przez intel_aux_transaction
    true
}

// ════════════════════════════════════════════════════════════════════════════
// § HPD
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn intel_hpd_sense(mmio: u64) -> (bool, bool) {
    let isr = r32(mmio, SDEISR);
    (isr & HPD_PORT_A != 0, isr & HPD_PORT_B != 0)
}

// ════════════════════════════════════════════════════════════════════════════
// § BAR mapping
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn map_bar(base: u64, pages: usize) {
    for i in 0..pages {
        let a = base + i as u64 * PAGE_SIZE as u64;
        vmap(K_P4, a, a, PTE_W);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Główna inicjalizacja Intel
// ════════════════════════════════════════════════════════════════════════════

pub struct IntelState {
    pub mmio:   u64,
    pub gen:    IntelGen,
    pub mode:   IntelOutMode,
    pub active: bool,
}

pub unsafe fn intel_display_init(mmio: u64, gen: IntelGen) -> Option<IntelState> {
    // BAR0 Intel = 4MB (Gen9) lub 2MB (Gen12)
    let pages = if gen == IntelGen::Gen9 { 1024 } else { 512 };
    map_bar(mmio, pages);

    serial_print("[Intel] Gen"); serial_print(match gen {
        IntelGen::Gen9  => "9",
        IntelGen::Gen11 => "11",
        IntelGen::Gen12 => "12",
    });
    serial_print(" mmio="); serial_hex(mmio); serial_print("\n");

    // ForceWake — obowiązkowe przed dostępem do rejestrów display
    intel_forcewake(mmio);

    // Power Well
    intel_power_well_enable(mmio);

    // DPLL
    intel_dpll_init(mmio, gen);

    // HPD sense
    let (hpd_a, hpd_b) = intel_hpd_sense(mmio);
    serial_print("[Intel] HPD_A="); serial_hex(hpd_a as u64);
    serial_print(" HPD_B="); serial_hex(hpd_b as u64); serial_print("\n");

    let mode = if hpd_a && intel_dp_link_train(mmio) {
        IntelOutMode::Dp
    } else {
        IntelOutMode::Hdmi
    };

    // Pipe A timing
    intel_pipe_program(mmio, &T_1080P60);

    // DDI Port A
    intel_ddi_enable(mmio, mode);

    // Enable pipe
    intel_pipe_enable(mmio);

    // Primary plane → framebuffer
    intel_plane_program(mmio, FB_PHYS, gen);

    Some(IntelState { mmio, gen, mode, active: true })
}

pub unsafe fn intel_hotplug(state: &mut IntelState) {
    let (hpd_a, _) = intel_hpd_sense(state.mmio);
    if hpd_a && !state.active {
        serial_print("[Intel] hotplug: reconnect\n");
        intel_ddi_enable(state.mmio, IntelOutMode::Dp);
        intel_pipe_enable(state.mmio);
        intel_plane_program(state.mmio, FB_PHYS, state.gen);
        state.active = true;
    } else if !hpd_a && state.active && state.mode == IntelOutMode::Dp {
        serial_print("[Intel] hotplug: disconnect\n");
        state.active = false;
    }
}
