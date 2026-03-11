// CosinusOS — usb/ehci.rs
// EHCI (USB 2.0 HighSpeed) + OHCI (USB 1.x companion) + UHCI (Intel USB 1.x)
// Pełna enumeracja przez control transfers, HID Boot Protocol
// Hotplug: connect + disconnect polling

use super::{Pci, r32, w32, r64, w64, map_mmio, zpage, spinwait, log_num};
use super::hid::{
    UsbDevice, DevClass, parse_config_descriptor, classify,
    hid_dispatch, hid_log_device, dev_alloc, dev_free,
    USB_DEVICES, MAX_USB_DEVICES,
};
use crate::debug::{serial_print, serial_hex, num_str};

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — rejestry
// ════════════════════════════════════════════════════════════════════════════

const EHCI_CAPLENGTH: usize = 0x00;
const EHCI_HCSPAR:    usize = 0x04;
const EHCI_HCCPAR:    usize = 0x08;
// Operational (base = cap + CAPLENGTH)
const EHCI_CMD:    usize = 0x00;
const EHCI_STS:    usize = 0x04;
const EHCI_INTR:   usize = 0x08;
const EHCI_FRLIST: usize = 0x14; // Periodic Frame List Base
const EHCI_ASYNC:  usize = 0x18; // Async List Address
const EHCI_CFLAG:  usize = 0x40; // Configure Flag
const EHCI_PORTSC: usize = 0x44; // Port Status/Control (port 0)

const ECMD_RUN:  u32 = 1<<0;
const ECMD_RST:  u32 = 1<<1;
const ECMD_PSE:  u32 = 1<<4;  // Periodic Schedule Enable
const ECMD_ASE:  u32 = 1<<5;  // Async Schedule Enable
const ECMD_IAAD: u32 = 1<<6;  // Interrupt on Async Advance Doorbell
const ECMD_FLS0: u32 = 0<<2;  // Frame List Size = 1024
const ECMD_ITC:  u32 = 8<<16; // Interrupt threshold = 8 microframes

const ESTS_INT:  u32 = 1<<0;  // USB interrupt
const ESTS_ERR:  u32 = 1<<1;
const ESTS_PCD:  u32 = 1<<2;  // Port Change Detect
const ESTS_FLR:  u32 = 1<<3;
const ESTS_HSE:  u32 = 1<<4;
const ESTS_AAI:  u32 = 1<<5;
const ESTS_HCH:  u32 = 1<<12;
const ESTS_RCL:  u32 = 1<<13;
const ESTS_PS:   u32 = 1<<14;
const ESTS_AS:   u32 = 1<<15;

const EPRT_CCS:  u32 = 1<<0;  // Current Connect Status
const EPRT_CSC:  u32 = 1<<1;  // Connect Status Change
const EPRT_PEN:  u32 = 1<<2;  // Port Enable
const EPRT_PENC: u32 = 1<<3;  // Port Enable Change
const EPRT_OCA:  u32 = 1<<4;
const EPRT_OCC:  u32 = 1<<5;
const EPRT_FPR:  u32 = 1<<6;  // Force Port Resume
const EPRT_SUSP: u32 = 1<<7;
const EPRT_PRST: u32 = 1<<8;  // Port Reset
const EPRT_LS:   u32 = 3<<10; // Line Status
const EPRT_PP:   u32 = 1<<12; // Port Power
const EPRT_OWN:  u32 = 1<<13; // Port Owner (release to companion)
const EPRT_PIC:  u32 = 3<<14;
const EPRT_PTC:  u32 = 0xF<<16;
const EPRT_WKCN: u32 = 1<<20;

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — struktury danych (QH + qTD)
// ════════════════════════════════════════════════════════════════════════════

// H-link type bits
const QH_TYP_ITD: u32 = 0<<1;
const QH_TYP_QH:  u32 = 1<<1;
const QH_TYP_SITD:u32 = 2<<1;
const QH_TYP_FSTN:u32 = 3<<1;
const QH_T_BIT:   u32 = 1;     // Terminate bit

// qTD token bits
const QTD_ACTIVE: u32 = 1<<7;
const QTD_HALT:   u32 = 1<<6;
const QTD_DBE:    u32 = 1<<5;  // Data Buffer Error
const QTD_BABBLE: u32 = 1<<4;
const QTD_XACT:   u32 = 1<<3;  // Transaction Error
const QTD_MMF:    u32 = 1<<2;
const QTD_STS:    u32 = 1<<1;
const QTD_PING:   u32 = 1<<0;
const QTD_IOC:    u32 = 1<<15;
const QTD_PID_OUT:u32 = 0<<8;
const QTD_PID_IN: u32 = 1<<8;
const QTD_PID_SET:u32 = 2<<8;

#[repr(C, align(32))]
pub struct Qtd {
    pub next:     u32,  // Next qTD Pointer
    pub alt_next: u32,  // Alternate Next qTD
    pub token:    u32,  // Status + PID + length
    pub buf:      [u32; 5], // Buffer Page Pointers
    // Padding to 32 bytes
    _pad:         [u32; 0],
}

#[repr(C, align(32))]
pub struct Qh {
    pub next:     u32,  // Horizontal link (to next QH, or terminate)
    pub epchar:   u32,  // Endpoint Characteristics
    pub epcap:    u32,  // Endpoint Capabilities
    pub cur_qtd:  u32,  // Current qTD Pointer
    // Overlay (copy of active qTD)
    pub n_qtd:    u32,
    pub alt_qtd:  u32,
    pub token:    u32,
    pub buf:      [u32; 5],
    _pad:         [u32; 3],
}

// epchar bity
const QH_ADDR:    u32 = 0x7F;      // bits 0-6: device address
const QH_EP:      u32 = 0xF<<8;    // bits 8-11: endpoint number
const QH_EPS_FS:  u32 = 0<<12;     // Full Speed
const QH_EPS_LS:  u32 = 1<<12;     // Low Speed
const QH_EPS_HS:  u32 = 2<<12;     // High Speed
const QH_DTC:     u32 = 1<<14;     // Data Toggle Control
const QH_H:       u32 = 1<<15;     // Head of Reclamation List (async only)
const QH_MPS:     u32 = 0x7FF<<16; // bits 16-26: Max Packet Size
const QH_CTRL:    u32 = 1<<27;     // Control Endpoint Flag (FS/LS on TT)
const QH_RL:      u32 = 0xF<<28;   // NAK Count Reload

const QH_SMASK:   u32 = 0xFF;      // epcap bits 0-7: Split Start Mask
const QH_CMASK:   u32 = 0xFF<<8;   // epcap bits 8-15: Split Completion Mask
const QH_HUBADDR: u32 = 0x7F<<16;  // epcap bits 16-22: Hub Address
const QH_PORT:    u32 = 0x7F<<23;  // epcap bits 23-29: Port Number
const QH_MULT:    u32 = 3<<30;     // High-Bandwidth Pipe Multiplier

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — struktura kontrolera
// ════════════════════════════════════════════════════════════════════════════

const MAX_EHCI_PORTS: usize = 16;

// Per-port HID state
struct EhciPort {
    active:   bool,
    dev_idx:  u8,       // indeks w USB_DEVICES[]
    qh:       u64,      // wskaźnik na QH (phys=virt identity mapped)
    qtd:      u64,      // aktualny qTD
    buf:      u64,      // bufor danych (1 strona)
    addr:     u8,       // USB device address
    speed:    u8,
    ep_in:    u8,
    ep_mps:   u16,
    toggle:   bool,
}

impl EhciPort {
    const fn empty() -> Self {
        Self { active:false, dev_idx:0xFF, qh:0, qtd:0, buf:0,
               addr:0, speed:0, ep_in:1, ep_mps:64, toggle:false }
    }
}

pub struct Ehci {
    op:      u64,
    n_ports: u8,
    pfl:     u64,   // Periodic Frame List (1024 × u32)
    aqh:     u64,   // Async Dummy QH (head)
    ports:   [EhciPort; MAX_EHCI_PORTS],
    // Next USB address to assign (1–127)
    next_addr: u8,
    // Snapshot CCS per port dla hotplug
    port_ccs: u32,
}

static mut EHCI: Option<Ehci> = None;

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — init
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn ehci_init(pci: Pci) -> bool {
    let bar = match pci.bar(0) { Some(b)=>b, None=>{
        serial_print("[EHCI] brak BAR\n"); return false; }};
    map_mmio(bar, 4);
    pci.enable();

    let clen   = (r32(bar, EHCI_CAPLENGTH) & 0xFF) as usize;
    let op     = bar + clen as u64;
    let hcsp   = r32(bar, EHCI_HCSPAR);
    let nports = (hcsp & 0xF) as u8;

    serial_print("[EHCI] bar="); serial_hex(bar);
    log_num(" ports=", nports as usize); serial_print("\n");

    // Reset kontrolera
    w32(op, EHCI_CMD, ECMD_RST);
    if !spinwait(op, EHCI_CMD, ECMD_RST, 0, 2000) {
        serial_print("[EHCI] RST timeout\n"); return false; }

    // Periodic Frame List (1024 × terminate)
    let pfl = zpage();
    for i in 0..1024usize { *(pfl as *mut u32).add(i) = QH_T_BIT; }

    // Async dummy QH (wskazuje na siebie, H=1)
    let aqh_p = zpage() as *mut Qh;
    (*aqh_p).next   = (aqh_p as u32) | QH_TYP_QH;
    (*aqh_p).epchar = QH_H | (64<<16); // H=1 mps=64
    (*aqh_p).n_qtd  = QH_T_BIT;
    (*aqh_p).alt_qtd= QH_T_BIT;
    (*aqh_p).token  = 0;

    // Wyczyść statusy, ustaw listę i uruchom
    w32(op, EHCI_INTR, 0);
    w32(op, EHCI_STS,  0x3F);
    w32(op, EHCI_FRLIST, pfl as u32);
    w32(op, EHCI_ASYNC, aqh_p as u32);
    w32(op, EHCI_CFLAG, 1);   // CF=1 — EHCI przejmuje porty od OHCI/UHCI
    w32(op, EHCI_CMD, ECMD_RUN | ECMD_PSE | ECMD_ASE | ECMD_ITC);

    if !spinwait(op, EHCI_STS, ESTS_HCH, 0, 500) {
        serial_print("[EHCI] run timeout\n"); }

    let mut e = Ehci {
        op, n_ports: nports, pfl, aqh: aqh_p as u64,
        ports: core::array::from_fn(|_| EhciPort::empty()),
        next_addr: 1, port_ccs: 0,
    };

    // Wstępna enumeracja portów
    for p in 0..nports as usize {
        ehci_probe_port(&mut e, p);
    }

    EHCI = Some(e);
    serial_print("[EHCI] OK\n");
    true
}

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — control transfer przez async QH
// ════════════════════════════════════════════════════════════════════════════

/// Zbuduj łańcuch qTD: SETUP → [DATA] → STATUS
/// Zwraca adres pierwszego qTD (phys = virt)
unsafe fn build_ctrl_qtds(
    setup_data: u64, // 8-bajtowe SETUP pakiet
    buf: u64, buf_len: u16,
    dir_in: bool,
) -> (u64, u64) { // (first_qtd, last_qtd)
    // SETUP qTD
    let setup_buf = zpage();
    *(setup_buf as *mut u64) = setup_data;

    let q0 = zpage() as *mut Qtd;
    (*q0).token  = QTD_ACTIVE | QTD_PID_SET | (8<<16) | (3<<10);
    (*q0).buf[0] = setup_buf as u32;

    let data_qtd = if buf_len > 0 {
        let qd = zpage() as *mut Qtd;
        (*qd).token  = QTD_ACTIVE
            | (if dir_in { QTD_PID_IN } else { QTD_PID_OUT })
            | ((buf_len as u32)<<16)
            | (1<<31) // DATA1
            | (3<<10);
        (*qd).buf[0] = buf as u32;
        (*qd).buf[1] = (buf + 0x1000) as u32;
        (*q0).next = qd as u32;
        (*q0).alt_next = QH_T_BIT;
        Some(qd)
    } else {
        (*q0).next = QH_T_BIT;
        (*q0).alt_next = QH_T_BIT;
        None
    };

    // STATUS qTD (odwrócony kierunek)
    let qs = zpage() as *mut Qtd;
    (*qs).token  = QTD_ACTIVE | QTD_IOC
        | (if dir_in { QTD_PID_OUT } else { QTD_PID_IN })
        | (1<<31) // DATA1
        | (3<<10);
    (*qs).next     = QH_T_BIT;
    (*qs).alt_next = QH_T_BIT;

    if let Some(qd) = data_qtd {
        (*qd).next     = qs as u32;
        (*qd).alt_next = QH_T_BIT;
    } else {
        (*q0).next = qs as u32;
    }

    (q0 as u64, qs as u64)
}

/// Wykonaj control transfer na danym porcie/adresie
/// Zwraca true jeśli sukces (ostatni qTD nie ma ACTIVE|HALT)
unsafe fn ehci_ctrl_transfer(
    e: &mut Ehci, port_idx: usize,
    setup: u64, buf: u64, len: u16, dir_in: bool,
) -> bool {
    let p = &e.ports[port_idx];
    let addr  = p.addr;
    let speed = p.speed;

    // Tymczasowy QH dla control EP0
    let qh_p = zpage() as *mut Qh;
    let hs_bit = match speed { 3 => QH_EPS_HS, 1 => QH_EPS_LS, _ => QH_EPS_FS };
    (*qh_p).epchar = (addr as u32)       // device addr
        | (0<<8)                          // EP 0
        | hs_bit
        | QH_DTC                          // DTC=1 (toggle from qTD)
        | (8<<16)                         // max pkt size EP0
        | QH_CTRL;                        // control endpoint
    (*qh_p).epcap  = (1<<30) | 0x01;     // Mult=1, s-mask=0x01
    (*qh_p).n_qtd  = QH_T_BIT;
    (*qh_p).alt_qtd= QH_T_BIT;
    (*qh_p).token  = 0;

    let (first_qtd, last_qtd) = build_ctrl_qtds(setup, buf, len, dir_in);
    (*qh_p).n_qtd = first_qtd as u32;

    // Wstaw QH do async listy za dummy QH
    let aqh_p = e.aqh as *mut Qh;
    (*qh_p).next   = (*aqh_p).next;
    (*aqh_p).next  = (qh_p as u32) | QH_TYP_QH;

    // Poczekaj na zakończenie (ostatni qTD: ACTIVE=0 HALT=0)
    let ok = {
        let mut done = false;
        for _ in 0..500_000 {
            let tok = (*(last_qtd as *const Qtd)).token;
            if tok & QTD_ACTIVE == 0 {
                done = tok & QTD_HALT == 0;
                break;
            }
            core::hint::spin_loop();
        }
        done
    };

    // Usuń tymczasowy QH z listy
    (*aqh_p).next = (*qh_p).next;

    ok
}

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — enumeracja portu
// ════════════════════════════════════════════════════════════════════════════

unsafe fn ehci_probe_port(e: &mut Ehci, pi: usize) {
    if pi >= MAX_EHCI_PORTS { return; }
    let poff = EHCI_PORTSC + pi * 4;
    let psc  = r32(e.op, poff);
    if psc & EPRT_CCS == 0 { return; }

    log_num("[EHCI] port=", pi); serial_print(" attached\n");

    // Sprawdź Line Status — LS/FS → oddaj do companion jeśli mamy OHCI
    let ls_bits = (psc >> 10) & 3;
    let speed: u8;
    if ls_bits == 1 {
        // K-state = Low Speed (LS)
        speed = 1;
        // Oddaj do companion (OHCI) jeśli jest
        w32(e.op, poff, psc | EPRT_OWN);
        log_num("[EHCI] port=", pi);
        serial_print(" LS -> companion\n");
        // Companion wykryje sam w ohci_probe_port
        return;
    } else {
        speed = if ls_bits == 0 { 3 } else { 2 }; // SE0=HS, J=FS
    }

    // Reset portu
    w32(e.op, poff, (psc & !EPRT_PEN) | EPRT_PRST);
    for _ in 0..25_000 { core::hint::spin_loop(); }
    w32(e.op, poff, r32(e.op, poff) & !EPRT_PRST);
    for _ in 0..10_000 { core::hint::spin_loop(); }

    let psc2 = r32(e.op, poff);
    if psc2 & EPRT_PEN == 0 {
        // FS device — EHCI powinien oddać do companion
        w32(e.op, poff, psc2 | EPRT_OWN);
        log_num("[EHCI] port=", pi);
        serial_print(" FS -> companion\n");
        return;
    }

    // Włącz power
    w32(e.op, poff, psc2 | EPRT_PP);

    // Przydziel adres USB
    let addr = e.next_addr;
    e.next_addr = (e.next_addr % 126) + 1;

    // Zainicjuj port struct (addr=0 na start, ep0 mps=64)
    e.ports[pi].addr  = 0;
    e.ports[pi].speed = speed;
    e.ports[pi].ep_in = 1;
    e.ports[pi].ep_mps = 64;

    // GET_DESCRIPTOR(Device) — 18 bajtów
    let dbuf = zpage();
    // bmRequestType=0x80 bRequest=0x06 wValue=0x0100 wIndex=0 wLength=18
    let setup_dev: u64 = 0x0012_0000_0100_0680;
    let ok_dev = ehci_ctrl_transfer(e, pi, setup_dev, dbuf, 18, true);
    let (vid, pid) = if ok_dev {
        let b = dbuf as *const u8;
        let v = (*b.add(8) as u16) | ((*b.add(9) as u16)<<8);
        let p2= (*b.add(10) as u16)| ((*b.add(11) as u16)<<8);
        (v, p2)
    } else {
        serial_print("[EHCI] GET_DESCRIPTOR dev fail\n");
        (0u16, 0u16)
    };
    serial_print("[EHCI] VID="); serial_hex(vid as u64);
    serial_print(" PID=");       serial_hex(pid as u64); serial_print("\n");

    // SET_ADDRESS
    let setup_addr: u64 = ((addr as u64)<<32) | 0x0000_0000_0000_0005;
    ehci_ctrl_transfer(e, pi, setup_addr, 0, 0, false);
    e.ports[pi].addr = addr;
    for _ in 0..5_000 { core::hint::spin_loop(); } // tEDADDR ≥ 2ms

    // GET_DESCRIPTOR(Config) — najpierw 9 bajtów
    let cfgbuf = zpage();
    let setup_cfg9: u64 = 0x0009_0000_0200_0680;
    let ok9 = ehci_ctrl_transfer(e, pi, setup_cfg9, cfgbuf, 9, true);
    let total = if ok9 {
        let b = cfgbuf as *const u8;
        (*b.add(2) as u16) | ((*b.add(3) as u16)<<8)
    } else { 0 };

    let cfg_bytes = if total > 0 && total <= 512 {
        let wlen_shift = (total as u64) << 48;
        let setup_cfgn: u64 = 0x0000_0000_0200_0680 | wlen_shift;
        ehci_ctrl_transfer(e, pi, setup_cfgn, cfgbuf, total, true);
        core::slice::from_raw_parts(cfgbuf as *const u8, total as usize)
    } else {
        core::slice::from_raw_parts(cfgbuf as *const u8, 0)
    };

    let parsed = parse_config_descriptor(cfg_bytes);
    let class  = classify(&parsed, speed);
    let ep_in  = if parsed.ep_in != 0 { parsed.ep_in } else { 1 };
    let ep_mps = if parsed.ep_mps != 0 { parsed.ep_mps } else { 64 };

    e.ports[pi].ep_in  = ep_in;
    e.ports[pi].ep_mps = ep_mps;

    // SET_CONFIGURATION(1)
    let setup_scfg: u64 = 0x0000_0000_0001_0009;
    ehci_ctrl_transfer(e, pi, setup_scfg, 0, 0, false);

    // Dla HID: SET_PROTOCOL(Boot) + SET_IDLE
    if class == DevClass::Keyboard || class == DevClass::Mouse {
        let setup_proto: u64 = 0x0000_0000_000B_2100;
        ehci_ctrl_transfer(e, pi, setup_proto, 0, 0, false);
        let setup_idle: u64 = 0x0000_0000_000A_2100;
        ehci_ctrl_transfer(e, pi, setup_idle, 0, 0, false);
    }

    // Zbuduj stały QH + qTD dla Interrupt IN polling
    let buf  = zpage();
    let qtd  = build_intr_qtd(buf, ep_mps as u32, true);
    let qh_p = build_intr_qh(addr, ep_in, ep_mps, speed, qtd);

    // Wstaw QH do Periodic Frame List (każda ramka)
    let qh_link = (qh_p as u32) | QH_TYP_QH;
    for i in 0..1024usize { *(e.pfl as *mut u32).add(i) = qh_link; }

    e.ports[pi].qh     = qh_p;
    e.ports[pi].qtd    = qtd;
    e.ports[pi].buf    = buf;
    e.ports[pi].toggle = false;

    // Zapisz urządzenie
    if let Some(di) = dev_alloc(pi as u8) {
        USB_DEVICES[di].class      = class;
        USB_DEVICES[di].speed      = speed;
        USB_DEVICES[di].vid        = vid;
        USB_DEVICES[di].pid        = pid;
        USB_DEVICES[di].ep_in      = ep_in;
        USB_DEVICES[di].ep_in_mps  = ep_mps;
        USB_DEVICES[di].subclass   = parsed.subclass;
        USB_DEVICES[di].protocol   = parsed.protocol;
        e.ports[pi].dev_idx        = di as u8;
        e.ports[pi].active         = true;
        hid_log_device(&USB_DEVICES[di]);
    }

    if pi < 32 { e.port_ccs |= 1u32 << pi; }
}

/// Zbuduj Interrupt IN qTD
unsafe fn build_intr_qtd(buf: u64, len: u32, toggle: bool) -> u64 {
    let q = zpage() as *mut Qtd;
    let dt = if toggle { 1u32<<31 } else { 0 };
    (*q).token    = QTD_ACTIVE | QTD_PID_IN | QTD_IOC | dt | (len<<16) | (3<<10);
    (*q).next     = QH_T_BIT;
    (*q).alt_next = QH_T_BIT;
    (*q).buf[0]   = buf as u32;
    (*q).buf[1]   = (buf + 0x1000) as u32;
    q as u64
}

/// Zbuduj QH dla Interrupt endpoint
unsafe fn build_intr_qh(addr: u8, ep: u8, mps: u16, speed: u8, qtd: u64) -> u64 {
    let qh_p = zpage() as *mut Qh;
    let hs_bit = match speed { 3 => QH_EPS_HS, 1 => QH_EPS_LS, _ => QH_EPS_FS };
    (*qh_p).next   = QH_T_BIT; // koniec listy — wstawiamy do PFL bezpośrednio
    (*qh_p).epchar = (addr as u32)
        | ((ep as u32)<<8)
        | hs_bit
        | ((mps as u32)<<16);
    // s-mask: bit 0 = microframe 0; c-mask dla LS/FS split completion
    (*qh_p).epcap  = if speed < 3 { (1<<30) | 0x1C_01 } else { (1<<30) | 0x01 };
    (*qh_p).cur_qtd = 0;
    (*qh_p).n_qtd  = qtd as u32;
    (*qh_p).alt_qtd= QH_T_BIT;
    (*qh_p).token  = 0;
    qh_p as u64
}

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — poll loop
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn ehci_poll_all() {
    let e = match &mut EHCI { Some(x)=>x, None=>return };

    for pi in 0..e.n_ports.min(MAX_EHCI_PORTS as u8) as usize {
        if !e.ports[pi].active { continue; }
        let qh_p = e.ports[pi].qh as *mut Qh;
        let tok  = (*qh_p).token;

        // Transfer wciąż aktywny
        if tok & QTD_ACTIVE != 0 { continue; }
        // Błąd — restart
        if tok & QTD_HALT != 0 {
            ehci_restart_intr(e, pi);
            continue;
        }

        // Sukces — przetwórz dane
        let di = e.ports[pi].dev_idx;
        if di != 0xFF {
            let dev = &USB_DEVICES[di as usize];
            let buf = e.ports[pi].buf;
            hid_dispatch(dev, buf as *const u8, dev.ep_in_mps as usize);
        }

        // Toggle i restart
        e.ports[pi].toggle = !e.ports[pi].toggle;
        ehci_restart_intr(e, pi);
    }
}

unsafe fn ehci_restart_intr(e: &mut Ehci, pi: usize) {
    let buf    = e.ports[pi].buf;
    let mps    = e.ports[pi].ep_mps as u32;
    let toggle = e.ports[pi].toggle;
    let qtd    = build_intr_qtd(buf, mps, toggle);

    let qh_p = e.ports[pi].qh as *mut Qh;
    (*qh_p).n_qtd  = qtd as u32;
    (*qh_p).alt_qtd= QH_T_BIT;
    (*qh_p).token  = 0;
    e.ports[pi].qtd = qtd;
}

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — hotplug
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn ehci_hotplug() {
    let e = match &mut EHCI { Some(x)=>x, None=>return };

    for pi in 0..e.n_ports.min(MAX_EHCI_PORTS as u8) as usize {
        let poff = EHCI_PORTSC + pi * 4;
        let psc  = r32(e.op, poff);
        let csc  = psc & EPRT_CSC != 0;
        if !csc { continue; }
        w32(e.op, poff, psc | EPRT_CSC); // skasuj CSC

        let ccs      = psc & EPRT_CCS != 0;
        let was_ccs  = (e.port_ccs >> pi) & 1 != 0;

        if ccs && !was_ccs {
            log_num("[EHCI] connect port=", pi); serial_print("\n");
            ehci_probe_port(e, pi);
        } else if !ccs && was_ccs {
            log_num("[EHCI] disconnect port=", pi); serial_print("\n");
            let di = e.ports[pi].dev_idx;
            if di != 0xFF { dev_free(pi as u8); }
            e.ports[pi] = EhciPort::empty();
            // Usuń QH z PFL — wróć do terminate
            for i in 0..1024usize { *(e.pfl as *mut u32).add(i) = QH_T_BIT; }
            e.port_ccs &= !(1u32 << pi);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § OHCI — USB 1.x (companion do EHCI lub samodzielny)
// ════════════════════════════════════════════════════════════════════════════

// OHCI rejestry (offset od MMIO base)
const OHCI_REVISION:    usize = 0x00;
const OHCI_CTRL:        usize = 0x04;
const OHCI_CMD_STATUS:  usize = 0x08;
const OHCI_INT_STATUS:  usize = 0x0C;
const OHCI_INT_ENABLE:  usize = 0x10;
const OHCI_INT_DISABLE: usize = 0x14;
const OHCI_HCCA:        usize = 0x18; // HCCA base (256-byte aligned)
const OHCI_CTRL_HEAD_ED:usize = 0x20;
const OHCI_CTRL_CUR_ED: usize = 0x24;
const OHCI_BULK_HEAD_ED:usize = 0x28;
const OHCI_BULK_CUR_ED: usize = 0x2C;
const OHCI_DONE_HEAD:   usize = 0x30;
const OHCI_FM_INTERVAL: usize = 0x34;
const OHCI_FM_REMAINING:usize = 0x38;
const OHCI_FM_NUMBER:   usize = 0x3C;
const OHCI_PERIODIC_ST: usize = 0x40;
const OHCI_LS_THRESH:   usize = 0x44;
const OHCI_RH_DESCR_A:  usize = 0x48;
const OHCI_RH_DESCR_B:  usize = 0x4C;
const OHCI_RH_STATUS:   usize = 0x50;
const OHCI_RH_PORT:     usize = 0x54; // RH_PORT_STATUS[0] (n=0)

// OHCI_CTRL bits
const OHCI_CTRL_CBSR: u32 = 3<<0;
const OHCI_CTRL_PLE:  u32 = 1<<2;  // Periodic List Enable
const OHCI_CTRL_IE:   u32 = 1<<3;  // Isochronous Enable
const OHCI_CTRL_CLE:  u32 = 1<<4;  // Control List Enable
const OHCI_CTRL_BLE:  u32 = 1<<5;  // Bulk List Enable
const OHCI_CTRL_HCFS: u32 = 3<<6;  // Host Controller Functional State
const OHCI_CTRL_IR:   u32 = 1<<8;  // Interrupt Routing
const OHCI_CTRL_RWC:  u32 = 1<<9;
const OHCI_CTRL_RWE:  u32 = 1<<10;
const OHCI_HCFS_RESET:u32 = 0<<6;
const OHCI_HCFS_RESUME:u32=1<<6;
const OHCI_HCFS_OPER: u32 = 2<<6;
const OHCI_HCFS_SUSP: u32 = 3<<6;
// OHCI_CMD_STATUS bits
const OHCI_CS_HCR:    u32 = 1<<0;  // Host Controller Reset
const OHCI_CS_CLF:    u32 = 1<<1;  // Control List Filled
const OHCI_CS_BLF:    u32 = 1<<2;
const OHCI_CS_OCR:    u32 = 1<<3;
const OHCI_CS_SOC:    u32 = 3<<16; // Scheduling Overrun Count
// RH_PORT_STATUS bits
const OHCI_PORT_CCS:  u32 = 1<<0;
const OHCI_PORT_PES:  u32 = 1<<1;  // Port Enable Status
const OHCI_PORT_PSS:  u32 = 1<<2;  // Port Suspend Status
const OHCI_PORT_POCI: u32 = 1<<3;
const OHCI_PORT_PRS:  u32 = 1<<4;  // Port Reset Status
const OHCI_PORT_PPS:  u32 = 1<<8;  // Port Power Status
const OHCI_PORT_LSDA: u32 = 1<<9;  // Low Speed Device Attached
const OHCI_PORT_CSC:  u32 = 1<<16;
const OHCI_PORT_PESC: u32 = 1<<17;
const OHCI_PORT_PSSC: u32 = 1<<18;
const OHCI_PORT_OCIC: u32 = 1<<19;
const OHCI_PORT_PRSC: u32 = 1<<20; // Port Reset Status Change
// Zapis do RH_PORT_STATUS przez set-bity
const OHCI_PORT_SET_PRS:  u32 = 1<<4;  // inicjuj reset
const OHCI_PORT_SET_PPS:  u32 = 1<<8;  // włącz power
const OHCI_PORT_CLR_PESC: u32 = 1<<17;
const OHCI_PORT_CLR_PSSC: u32 = 1<<18;
const OHCI_PORT_CLR_PRSC: u32 = 1<<20;

// OHCI Endpoint Descriptor
#[repr(C, align(16))]
struct OhciEd {
    ctrl:    u32,  // FA EP D S K F MPS
    tail_td: u32,
    head_td: u32,  // bity 0-1: Halted, toggleCarry
    next_ed: u32,
}

// OHCI Transfer Descriptor
#[repr(C, align(16))]
struct OhciTd {
    ctrl:     u32, // R DP DI T EC CC
    cur_buf:  u32,
    next_td:  u32,
    buf_end:  u32,
}

const OHCI_TD_DP_SETUP: u32 = 0<<19;
const OHCI_TD_DP_OUT:   u32 = 1<<19;
const OHCI_TD_DP_IN:    u32 = 2<<19;
const OHCI_TD_T_DATA0:  u32 = 2<<24; // Toggle = DATA0
const OHCI_TD_T_DATA1:  u32 = 3<<24; // Toggle = DATA1
const OHCI_TD_T_AUTO:   u32 = 0<<24; // Toggle from ED
const OHCI_TD_CC_NOTAC: u32 = 0xF<<28; // Not Accessed (initial)
const OHCI_TD_DI_0:     u32 = 0<<21; // Delay Interrupt = 0 frames
const OHCI_TD_DI_NO:    u32 = 7<<21; // No interrupt
const OHCI_TD_R:        u32 = 1<<18; // Buffer Rounding

const MAX_OHCI_PORTS: usize = 16;

struct OhciPort {
    active:  bool,
    dev_idx: u8,
    ed_intr: u64, // Interrupt ED
    td_intr: u64, // Interrupt TD
    buf:     u64,
    addr:    u8,
    ep_in:   u8,
    ep_mps:  u16,
    toggle:  bool,
}

impl OhciPort { const fn empty() -> Self {
    Self { active:false, dev_idx:0xFF, ed_intr:0, td_intr:0, buf:0,
           addr:0, ep_in:1, ep_mps:8, toggle:false } } }

pub struct Ohci {
    base:     u64,
    n_ports:  u8,
    hcca:     u64,   // HCCA (256B aligned)
    ports:    [OhciPort; MAX_OHCI_PORTS],
    next_addr:u8,
    port_ccs: u32,
}

static mut OHCI: Option<Ohci> = None;

pub unsafe fn ohci_init(pci: Pci) -> bool {
    let bar = match pci.bar(0) { Some(b)=>b, None=>{
        serial_print("[OHCI] brak BAR\n"); return false; }};
    map_mmio(bar, 4);
    pci.enable();

    let rev    = r32(bar, OHCI_REVISION) & 0xFF;
    let rh_a   = r32(bar, OHCI_RH_DESCR_A);
    let nports = ((rh_a>>1)&0x7F) as u8;

    serial_print("[OHCI] bar="); serial_hex(bar);
    log_num(" rev=", rev as usize);
    log_num(" ports=", nports as usize); serial_print("\n");

    // Wejdź w tryb RESET
    w32(bar, OHCI_CTRL, OHCI_HCFS_RESET);
    for _ in 0..10_000 { core::hint::spin_loop(); }

    // Software Reset (SWR)
    w32(bar, OHCI_CMD_STATUS, OHCI_CS_HCR);
    for _ in 0..10_000 {
        if r32(bar, OHCI_CMD_STATUS) & OHCI_CS_HCR == 0 { break; }
        core::hint::spin_loop();
    }

    // HCCA — 256 bajtów wyrównanych
    let hcca = zpage(); // strona = 4096B, więc OK
    core::ptr::write_bytes(hcca as *mut u8, 0, 256);
    w32(bar, OHCI_HCCA, hcca as u32);

    // Ustaw FM_INTERVAL na standardową wartość (11999 + BSR)
    w32(bar, OHCI_FM_INTERVAL, 0xA7782EDF);
    // LS Threshold
    w32(bar, OHCI_LS_THRESH, 0x0628);

    // Uruchom
    w32(bar, OHCI_CTRL, OHCI_HCFS_OPER | OHCI_CTRL_PLE | OHCI_CTRL_CLE | OHCI_CTRL_CBSR);

    // Power on all ports
    for i in 0..nports as usize {
        w32(bar, OHCI_RH_PORT + i*4, OHCI_PORT_SET_PPS);
    }
    for _ in 0..50_000 { core::hint::spin_loop(); } // min 10ms

    let mut o = Ohci {
        base: bar, n_ports: nports, hcca,
        ports: core::array::from_fn(|_| OhciPort::empty()),
        next_addr: 1, port_ccs: 0,
    };

    // Wstępna enumeracja
    for pi in 0..nports as usize {
        ohci_probe_port(&mut o, pi);
    }

    OHCI = Some(o);
    serial_print("[OHCI] OK\n");
    true
}

/// Control transfer przez OHCI (setup + [data] + status)
unsafe fn ohci_ctrl_transfer(
    o: &mut Ohci, base: u64,
    setup_data: u64, buf: u64, len: u16, dir_in: bool,
    addr: u8,
) -> bool {
    // ED dla EP0
    let ed_p  = zpage() as *mut OhciEd;
    let tail  = zpage() as *mut OhciTd; // dummy tail TD

    (*ed_p).ctrl    = (addr as u32) | (0<<7) | (0xE<<10) | (8<<16); // addr ep0 D=auto mps=8
    (*ed_p).next_ed = 0;

    // SETUP TD
    let setup_buf = zpage();
    *(setup_buf as *mut u64) = setup_data;
    let td_setup = zpage() as *mut OhciTd;
    (*td_setup).ctrl    = OHCI_TD_DP_SETUP | OHCI_TD_T_DATA0 | OHCI_TD_DI_NO | OHCI_TD_CC_NOTAC;
    (*td_setup).cur_buf = setup_buf as u32;
    (*td_setup).buf_end = setup_buf as u32 + 7;

    // DATA TD
    let td_data = if len > 0 {
        let td = zpage() as *mut OhciTd;
        let pid = if dir_in { OHCI_TD_DP_IN } else { OHCI_TD_DP_OUT };
        (*td).ctrl    = pid | OHCI_TD_T_DATA1 | OHCI_TD_DI_NO | OHCI_TD_CC_NOTAC | OHCI_TD_R;
        (*td).cur_buf = buf as u32;
        (*td).buf_end = buf as u32 + len as u32 - 1;
        (*td_setup).next_td = td as u32;
        Some(td)
    } else {
        (*td_setup).next_td = tail as u32;
        None
    };

    // STATUS TD
    let td_status = zpage() as *mut OhciTd;
    let spid = if dir_in { OHCI_TD_DP_OUT } else { OHCI_TD_DP_IN };
    (*td_status).ctrl    = spid | OHCI_TD_T_DATA1 | OHCI_TD_DI_0 | OHCI_TD_CC_NOTAC;
    (*td_status).cur_buf = 0;
    (*td_status).buf_end = 0;
    (*td_status).next_td = tail as u32;

    if let Some(d) = td_data {
        (*d).next_td = td_status as u32;
    } else {
        (*td_setup).next_td = td_status as u32;
    }

    (*ed_p).head_td = td_setup as u32;
    (*ed_p).tail_td = tail as u32;

    // Wstaw ED do Control list
    w32(base, OHCI_CTRL_HEAD_ED, ed_p as u32);
    w32(base, OHCI_CMD_STATUS, OHCI_CS_CLF); // CLF
    // Włącz Control list processing
    let ctrl = r32(base, OHCI_CTRL) | OHCI_CTRL_CLE;
    w32(base, OHCI_CTRL, ctrl);

    // Poczekaj na zakończenie
    let ok = {
        let mut done = false;
        for _ in 0..400_000 {
            let cc = (*(td_status as *const OhciTd)).ctrl >> 28;
            if cc != 0xF { done = cc == 0; break; }
            core::hint::spin_loop();
        }
        done
    };

    // Usuń z listy
    w32(base, OHCI_CTRL_HEAD_ED, 0);
    let ctrl = r32(base, OHCI_CTRL) & !OHCI_CTRL_CLE;
    w32(base, OHCI_CTRL, ctrl);
    ok
}

unsafe fn ohci_probe_port(o: &mut Ohci, pi: usize) {
    if pi >= MAX_OHCI_PORTS { return; }
    let poff = OHCI_RH_PORT + pi * 4;
    let psc  = r32(o.base, poff);
    if psc & OHCI_PORT_CCS == 0 { return; }

    log_num("[OHCI] port=", pi); serial_print(" attached\n");

    // Reset
    w32(o.base, poff, OHCI_PORT_SET_PRS);
    for _ in 0..25_000 { core::hint::spin_loop(); }
    // Poczekaj na PRSC
    for _ in 0..50_000 {
        if r32(o.base, poff) & OHCI_PORT_PRSC != 0 { break; }
        core::hint::spin_loop();
    }
    w32(o.base, poff, OHCI_PORT_CLR_PRSC);

    let psc2  = r32(o.base, poff);
    let speed = if psc2 & OHCI_PORT_LSDA != 0 { 1u8 } else { 2u8 };

    let addr = o.next_addr;
    o.next_addr = (o.next_addr % 126) + 1;
    o.ports[pi].addr  = 0;
    o.ports[pi].speed = speed;

    // GET_DESCRIPTOR(Device)
    let dbuf = zpage();
    let setup_dev: u64 = 0x0012_0000_0100_0680;
    let ok_dev = ohci_ctrl_transfer(o, o.base, setup_dev, dbuf, 18, true, 0);
    let (vid, pid) = if ok_dev {
        let b = dbuf as *const u8;
        ((*b.add(8) as u16)|((*b.add(9) as u16)<<8),
         (*b.add(10) as u16)|((*b.add(11) as u16)<<8))
    } else { (0, 0) };

    // SET_ADDRESS
    let setup_addr: u64 = ((addr as u64)<<32) | 0x0000_0000_0000_0005;
    ohci_ctrl_transfer(o, o.base, setup_addr, 0, 0, false, 0);
    o.ports[pi].addr = addr;
    for _ in 0..5_000 { core::hint::spin_loop(); }

    // GET_DESCRIPTOR(Config) 9B
    let cfgbuf = zpage();
    let setup_cfg9: u64 = 0x0009_0000_0200_0680;
    let ok9 = ohci_ctrl_transfer(o, o.base, setup_cfg9, cfgbuf, 9, true, addr);
    let total = if ok9 {
        let b = cfgbuf as *const u8;
        (*b.add(2) as u16)|((*b.add(3) as u16)<<8)
    } else { 0 };
    let cfg_bytes = if total > 0 && total <= 512 {
        let wlen_shift = (total as u64)<<48;
        let setup_cfgn: u64 = 0x0000_0000_0200_0680 | wlen_shift;
        ohci_ctrl_transfer(o, o.base, setup_cfgn, cfgbuf, total, true, addr);
        core::slice::from_raw_parts(cfgbuf as *const u8, total as usize)
    } else { core::slice::from_raw_parts(cfgbuf as *const u8, 0) };

    let parsed = parse_config_descriptor(cfg_bytes);
    let class  = classify(&parsed, speed);
    let ep_in  = if parsed.ep_in != 0 { parsed.ep_in } else { 1 };
    let ep_mps = if parsed.ep_mps != 0 { parsed.ep_mps } else { 8 };

    o.ports[pi].ep_in  = ep_in;
    o.ports[pi].ep_mps = ep_mps;

    // SET_CONFIGURATION(1)
    let setup_scfg: u64 = 0x0000_0000_0001_0009;
    ohci_ctrl_transfer(o, o.base, setup_scfg, 0, 0, false, addr);

    // HID SET_PROTOCOL + SET_IDLE
    if class == DevClass::Keyboard || class == DevClass::Mouse {
        ohci_ctrl_transfer(o, o.base, 0x0000_0000_000B_2100, 0, 0, false, addr);
        ohci_ctrl_transfer(o, o.base, 0x0000_0000_000A_2100, 0, 0, false, addr);
    }

    // Zbuduj ED + TD dla Interrupt IN polling
    let buf   = zpage();
    let td_p  = zpage() as *mut OhciTd;
    let tail  = zpage() as *mut OhciTd;
    let ed_p  = zpage() as *mut OhciEd;

    (*td_p).ctrl    = OHCI_TD_DP_IN | OHCI_TD_T_DATA0 | OHCI_TD_DI_0
                    | OHCI_TD_CC_NOTAC | OHCI_TD_R;
    (*td_p).cur_buf = buf as u32;
    (*td_p).buf_end = buf as u32 + ep_mps as u32 - 1;
    (*td_p).next_td = tail as u32;

    (*ed_p).ctrl    = (addr as u32) | ((ep_in as u32)<<7)
                    | (if speed==1 {1<<13} else {0})
                    | ((ep_mps as u32)<<16);
    (*ed_p).head_td = td_p as u32;
    (*ed_p).tail_td = tail as u32;
    (*ed_p).next_ed = 0;

    // Wstaw ED do Interrupt list (slot 0 w HCCA = co-ramkę)
    *(o.hcca as *mut u32) = ed_p as u32;
    // Włącz Periodic list
    w32(o.base, OHCI_CTRL, r32(o.base, OHCI_CTRL) | OHCI_CTRL_PLE);

    o.ports[pi].ed_intr  = ed_p as u64;
    o.ports[pi].td_intr  = td_p as u64;
    o.ports[pi].buf      = buf;
    o.ports[pi].active   = true;

    if let Some(di) = dev_alloc(pi as u8 + 64) {
        USB_DEVICES[di].class     = class;
        USB_DEVICES[di].speed     = speed;
        USB_DEVICES[di].vid       = vid;
        USB_DEVICES[di].pid       = pid;
        USB_DEVICES[di].ep_in     = ep_in;
        USB_DEVICES[di].ep_in_mps = ep_mps;
        o.ports[pi].dev_idx       = di as u8;
        hid_log_device(&USB_DEVICES[di]);
    }

    if pi < 32 { o.port_ccs |= 1u32 << pi; }
}

pub unsafe fn ohci_poll_all() {
    let o = match &mut OHCI { Some(x)=>x, None=>return };

    for pi in 0..o.n_ports.min(MAX_OHCI_PORTS as u8) as usize {
        if !o.ports[pi].active { continue; }
        let td_p = o.ports[pi].td_intr as *mut OhciTd;
        let cc   = (*td_p).ctrl >> 28;
        if cc == 0xF { continue; } // Not Accessed — wciąż aktywny

        if cc == 0 {
            let di = o.ports[pi].dev_idx;
            if di != 0xFF {
                let dev = &USB_DEVICES[di as usize];
                let buf = o.ports[pi].buf;
                hid_dispatch(dev, buf as *const u8, dev.ep_in_mps as usize);
            }
        }

        // Restart TD
        let buf   = o.ports[pi].buf;
        let mps   = o.ports[pi].ep_mps;
        let ed_p  = o.ports[pi].ed_intr as *mut OhciEd;
        let tail  = (*ed_p).tail_td as *mut OhciTd;
        let toggle = o.ports[pi].toggle;
        o.ports[pi].toggle = !toggle;

        let new_td = zpage() as *mut OhciTd;
        (*new_td).ctrl    = OHCI_TD_DP_IN
            | (if toggle { OHCI_TD_T_DATA1 } else { OHCI_TD_T_DATA0 })
            | OHCI_TD_DI_0 | OHCI_TD_CC_NOTAC | OHCI_TD_R;
        (*new_td).cur_buf = buf as u32;
        (*new_td).buf_end = buf as u32 + mps as u32 - 1;
        (*new_td).next_td = tail as u32;

        (*ed_p).head_td = new_td as u32;
        o.ports[pi].td_intr = new_td as u64;
    }
}

pub unsafe fn ohci_hotplug() {
    let o = match &mut OHCI { Some(x)=>x, None=>return };

    for pi in 0..o.n_ports.min(MAX_OHCI_PORTS as u8) as usize {
        let poff = OHCI_RH_PORT + pi * 4;
        let psc  = r32(o.base, poff);
        if psc & OHCI_PORT_CSC == 0 { continue; }
        w32(o.base, poff, OHCI_PORT_CSC); // skasuj CSC

        let ccs     = psc & OHCI_PORT_CCS != 0;
        let was_ccs = (o.port_ccs >> pi) & 1 != 0;

        if ccs && !was_ccs {
            log_num("[OHCI] connect port=", pi); serial_print("\n");
            ohci_probe_port(o, pi);
        } else if !ccs && was_ccs {
            log_num("[OHCI] disconnect port=", pi); serial_print("\n");
            let di = o.ports[pi].dev_idx;
            if di != 0xFF { dev_free(pi as u8 + 64); }
            o.ports[pi] = OhciPort::empty();
            o.port_ccs &= !(1u32 << pi);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Publiczne poll/hotplug (wywoływane z mod.rs)
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn ehci_poll_all_controllers() {
    ehci_poll_all();
    ohci_poll_all();
}

pub unsafe fn ehci_hotplug_all() {
    ehci_hotplug();
    ohci_hotplug();
}
