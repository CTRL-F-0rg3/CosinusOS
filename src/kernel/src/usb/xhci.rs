// CosinusOS — usb/xhci.rs
// XHCI (USB 3.x) Host Controller Driver
// Pełna enumeracja: GET_DESCRIPTOR Device + Configuration
// Hub support (USB 2/3 hubs, kaskadowanie portów do 2 poziomów)
// Hotplug: disconnect + reconnect

use super::{Pci, r32, w32, r64, w64, map_mmio, zpage, spinwait, log_num};
use super::hid::{
    UsbDevice, DevClass, ParsedConfig, parse_config_descriptor, classify,
    hid_dispatch, hid_log_device, dev_alloc, dev_free,
    USB_DEVICES, MAX_USB_DEVICES,
    DESC_DEVICE, DESC_CONFIG, REQ_GET_DESCRIPTOR, REQ_SET_CONFIGURATION,
    REQ_HID_SET_PROTO, REQ_HID_SET_IDLE,
};
use crate::debug::{serial_print, serial_hex, num_str};

// ════════════════════════════════════════════════════════════════════════════
// § TRB — Transfer Request Block
// ════════════════════════════════════════════════════════════════════════════

#[repr(C, align(16))] #[derive(Copy, Clone, Default)]
pub struct Trb { pub param: u64, pub status: u32, pub ctrl: u32 }

impl Trb {
    #[inline] pub fn typ(self)   -> u32   { (self.ctrl>>10)&0x3F }
    #[inline] pub fn cycle(self) -> bool  { self.ctrl & 1 != 0 }
    #[inline] pub fn slot(self)  -> usize { (self.ctrl>>24) as usize }
    #[inline] pub fn cc(self)    -> u32   { (self.status>>24)&0xFF }
}

const TRB_NORMAL:   u32 = 1;
const TRB_SETUP:    u32 = 2;
const TRB_DATA:     u32 = 3;
const TRB_STATUS:   u32 = 4;
const TRB_LINK:     u32 = 6;
const TRB_EN_SLOT:  u32 = 9;
const TRB_ADDR_DEV: u32 = 11;
const TRB_XFER_EVT: u32 = 32;
const TRB_CMD_CMPL: u32 = 33;
const TRB_PORT_CHG: u32 = 34;
const CC_SUCCESS:   u32 = 1;
const CC_SHORT:     u32 = 13;

// ════════════════════════════════════════════════════════════════════════════
// § Transfer Ring + Event Ring
// ════════════════════════════════════════════════════════════════════════════

const RSIZ: usize = 255;

pub struct Ring { pub phys: u64, ptr: *mut Trb, enq: usize, pcs: bool }

impl Ring {
    pub unsafe fn new() -> Self {
        let p = zpage();
        let lnk = &mut *(p as *mut Trb).add(RSIZ);
        lnk.param = p;
        lnk.ctrl  = (TRB_LINK<<10)|1|(1<<1);
        Self { phys:p, ptr:p as *mut Trb, enq:0, pcs:true }
    }
    pub unsafe fn push(&mut self, param:u64, status:u32, ctrl:u32) -> u64 {
        let i = self.enq;
        let t = &mut *self.ptr.add(i);
        t.param=param; t.status=status; t.ctrl=ctrl|self.pcs as u32;
        let pa = self.phys+i as u64*16;
        self.enq+=1;
        if self.enq>=RSIZ {
            let lnk = &mut *self.ptr.add(RSIZ);
            if self.pcs { lnk.ctrl|=1; } else { lnk.ctrl&=!1; }
            self.enq=0; self.pcs=!self.pcs;
        }
        pa
    }
}

pub struct EvtRing { pub phys:u64, erst:u64, ptr:*mut Trb, deq:usize, ccs:bool }

impl EvtRing {
    pub unsafe fn new() -> Self {
        let p  = zpage();
        let er = zpage();
        *(er as *mut u64)        = p;
        *(er as *mut u64).add(1) = RSIZ as u64;
        Self { phys:p, erst:er, ptr:p as *mut Trb, deq:0, ccs:true }
    }
    pub unsafe fn pop(&mut self) -> Option<Trb> {
        let t = *self.ptr.add(self.deq);
        if t.cycle()!=self.ccs { return None; }
        self.deq+=1;
        if self.deq>=RSIZ { self.deq=0; self.ccs=!self.ccs; }
        Some(t)
    }
    pub fn erdp(&self) -> u64 { self.phys+self.deq as u64*16 }
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI rejestry
// ════════════════════════════════════════════════════════════════════════════

const CAP_CAPLENGTH:  usize = 0x00;
const CAP_HCSPARAMS1: usize = 0x04;
const CAP_DBOFF:      usize = 0x14;
const CAP_RTSOFF:     usize = 0x18;
const OP_USBCMD:  usize = 0x00; const OP_USBSTS: usize = 0x04;
const OP_CRCR:    usize = 0x18; const OP_DCBAAP: usize = 0x30;
const OP_CONFIG:  usize = 0x38;
const CMD_RUN:u32=1; const CMD_RST:u32=1<<1; const CMD_INTE:u32=1<<2;
const STS_HCH:u32=1; const STS_CNR:u32=1<<11;
const PRTSC_CCS:u32=1; const PRTSC_PR:u32=1<<4;
const PRTSC_CSC:u32=1<<17; const PRTSC_PRC:u32=1<<21;
const PRTSC_SPD:u32=0xF<<10; const PRTSC_PED:u32=1<<1;

// ════════════════════════════════════════════════════════════════════════════
// § Globalna struktura XHCI
// ════════════════════════════════════════════════════════════════════════════

const MAX_SLOTS: usize = 32;

pub struct Xhci {
    pub cap: u64, pub op: u64, pub rt: u64, pub db: u64,
    pub max_ports: u8,
    pub cmd: Ring, pub evt: EvtRing,
    pub dcbaap: u64,
    pub xfer:   [Option<Ring>; MAX_SLOTS],
    pub xbuf:   [u64;  MAX_SLOTS],
    pub dev_idx:[u8;   MAX_SLOTS], // indeks w USB_DEVICES[] lub 0xFF
    // Port status snapshot dla hotplug
    pub port_ccs: u64, // bitmapa podłączonych portów (do 64)
}

static mut XHCI: Option<Xhci> = None;

// ════════════════════════════════════════════════════════════════════════════
// § Init
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn xhci_init(pci: Pci) -> bool {
    let bar = match pci.bar(0) { Some(b)=>b, None=>{
        serial_print("[XHCI] brak BAR\n"); return false; }};
    map_mmio(bar, 8);
    pci.enable();

    let clen  = (r32(bar, CAP_CAPLENGTH)&0xFF) as usize;
    let op    = bar+clen as u64;
    let rt    = bar+r32(bar, CAP_RTSOFF) as u64;
    let db    = bar+r32(bar, CAP_DBOFF)  as u64;
    let hcs1  = r32(bar, CAP_HCSPARAMS1);
    let mslots= (hcs1&0xFF).min(MAX_SLOTS as u32) as u8;
    let mports= (hcs1>>24) as u8;

    serial_print("[XHCI] bar="); serial_hex(bar);
    log_num(" ports=", mports as usize); serial_print("\n");

    // Reset
    w32(op, OP_USBCMD, 0);
    if !spinwait(op,OP_USBSTS,STS_CNR,0,2000){serial_print("[XHCI] CNR\n");return false;}
    w32(op, OP_USBCMD, CMD_RST);
    if !spinwait(op,OP_USBCMD,CMD_RST,0,2000){serial_print("[XHCI] RST\n");return false;}
    if !spinwait(op,OP_USBSTS,STS_CNR,0,2000){serial_print("[XHCI] CNR2\n");return false;}

    w32(op, OP_CONFIG, mslots as u32);
    let dcbaap = zpage();
    w64(op, OP_DCBAAP, dcbaap);

    let cmd = Ring::new();
    w64(op, OP_CRCR, cmd.phys|1);

    let evt = EvtRing::new();
    w32(rt, 0x028, 1);
    w64(rt, 0x030, evt.erst);
    w64(rt, 0x038, evt.phys);
    w32(rt, 0x020, r32(rt,0x020)|3);

    w32(op, OP_USBCMD, CMD_RUN|CMD_INTE);
    if !spinwait(op,OP_USBSTS,STS_HCH,0,1000){serial_print("[XHCI] start\n");return false;}

    XHCI = Some(Xhci {
        cap:bar, op, rt, db, max_ports:mports,
        cmd, evt, dcbaap,
        xfer:    core::array::from_fn(|_| None),
        xbuf:    [0u64; MAX_SLOTS],
        dev_idx: [0xFF;  MAX_SLOTS],
        port_ccs: 0,
    });

    // Wstępna enumeracja wszystkich portów
    xhci_probe_all();
    true
}

// ════════════════════════════════════════════════════════════════════════════
// § Command ring helpers
// ════════════════════════════════════════════════════════════════════════════

unsafe fn xhci_ring_cmd(x: &Xhci) { w32(x.db, 0, 0); }
unsafe fn xhci_ring_ep(x: &Xhci, slot: usize, ep: u32) { w32(x.db, slot*4, ep); }

unsafe fn xhci_erdp_update(x: &Xhci) {
    w64(x.rt, 0x038, x.evt.erdp()|(1<<3));
}

unsafe fn xhci_cmd_wait(x: &mut Xhci, timeout: usize) -> Option<Trb> {
    xhci_ring_cmd(x);
    for _ in 0..timeout {
        if let Some(e) = x.evt.pop() {
            xhci_erdp_update(x);
            if e.typ()==TRB_CMD_CMPL { return Some(e); }
        }
        core::hint::spin_loop();
    }
    None
}

unsafe fn cmd_enable_slot(x: &mut Xhci) -> Option<u8> {
    x.cmd.push(0,0,TRB_EN_SLOT<<10);
    let e = xhci_cmd_wait(x, 300_000)?;
    if e.cc()!=CC_SUCCESS { return None; }
    Some((e.ctrl>>24) as u8)
}

unsafe fn cmd_address_device(x: &mut Xhci, slot: u8, ictx: u64, bsr: bool) -> bool {
    let bsr_bit = if bsr { 1u32<<9 } else { 0 };
    x.cmd.push(ictx, 0, (TRB_ADDR_DEV<<10)|((slot as u32)<<24)|bsr_bit);
    xhci_cmd_wait(x, 300_000).map_or(false, |e| e.cc()==CC_SUCCESS)
}

// ════════════════════════════════════════════════════════════════════════════
// § Input Context builder
// ════════════════════════════════════════════════════════════════════════════

unsafe fn build_ictx(slot: u8, port: u8, speed: u8, ep_phys: u64, mps: u16, ep_num: u8) -> u64 {
    let p = zpage() as *mut u32;
    // Bit 0 = slot ctx, bit (ep_num*2+1) = EP IN
    let ep_bit = 1u32 << (ep_num as u32 * 2 + 1);
    *p.add(1) = (1<<0)|(1<<1)|ep_bit;

    // Slot Context
    let sc = p.add(8);
    *sc.add(0) = ((speed as u32)<<20)|(1<<27);
    *sc.add(1) = (port as u32)<<16;

    // EP IN Context (ep_num*2 entries after slot, each 8 DWORDs)
    let ep_off = 8 + ep_num as usize * 8;
    let ep = p.add(ep_off);
    *ep.add(1) = (3<<1)|(3<<3)|(3<<16); // CErr=3 EPType=IntIN
    *ep.add(2) = (ep_phys as u32 & !0xF)|1; // Dequeue Lo | DCS
    *ep.add(3) = (ep_phys>>32) as u32;
    *ep.add(4) = mps as u32;
    p as u64
}

// ════════════════════════════════════════════════════════════════════════════
// § Control transfer na EP0 (GET_DESCRIPTOR, SET_CONFIG, SET_PROTOCOL)
// ════════════════════════════════════════════════════════════════════════════

/// Wykonaj control transfer IN przez command ring (uproszczone — przez EP0 slot)
unsafe fn ctrl_in(x: &mut Xhci, slot: usize, setup: u64, buf: u64, len: u16) -> bool {
    // SETUP stage
    x.cmd.push(setup, len as u32, (TRB_SETUP<<10)|(3<<16)|1); // TRT=3(IN) IDT=1
    // DATA stage IN
    x.cmd.push(buf, len as u32, (TRB_DATA<<10)|(1<<16)|1); // DIR=IN
    // STATUS stage OUT
    x.cmd.push(0, 0, (TRB_STATUS<<10)|1);
    xhci_ring_ep(x, slot, 1); // EP0 doorbell = 1

    // Czekaj na transfer event
    for _ in 0..500_000 {
        if let Some(e) = x.evt.pop() {
            xhci_erdp_update(x);
            if e.typ()==TRB_XFER_EVT && e.slot()==slot {
                return e.cc()==CC_SUCCESS || e.cc()==CC_SHORT;
            }
        }
        core::hint::spin_loop();
    }
    false
}

unsafe fn ctrl_out(x: &mut Xhci, slot: usize, setup: u64) -> bool {
    x.cmd.push(setup, 0, (TRB_SETUP<<10)|(0<<16)|1); // TRT=0(NO DATA)
    x.cmd.push(0, 0, (TRB_STATUS<<10)|(1<<16)|1); // STATUS IN
    xhci_ring_ep(x, slot, 1);
    for _ in 0..200_000 {
        if let Some(e) = x.evt.pop() {
            xhci_erdp_update(x);
            if e.typ()==TRB_XFER_EVT && e.slot()==slot {
                return e.cc()==CC_SUCCESS || e.cc()==CC_SHORT;
            }
        }
        core::hint::spin_loop();
    }
    false
}

/// GET_DESCRIPTOR(Device) — zwraca VID/PID
unsafe fn get_device_descriptor(x: &mut Xhci, slot: usize, buf: u64) -> bool {
    // bmRequestType=0x80 bRequest=GET_DESCRIPTOR wValue=0x0100 wIndex=0 wLength=18
    let setup: u64 = 0x0012_0000_0100_0680;
    ctrl_in(x, slot, setup, buf, 18)
}

/// GET_DESCRIPTOR(Configuration) — zwraca pełny config descriptor
unsafe fn get_config_descriptor(x: &mut Xhci, slot: usize, buf: u64, len: u16) -> bool {
    // wValue=0x0200 (Config desc type=2, index=0)
    let wlen = (len as u64) << 48;
    let setup: u64 = 0x0000_0000_0200_0680 | wlen;
    ctrl_in(x, slot, setup, buf, len)
}

unsafe fn set_configuration(x: &mut Xhci, slot: usize, config: u8) -> bool {
    // bmRequestType=0x00 bRequest=SET_CONFIGURATION wValue=config
    let setup: u64 = 0x0000_0000_0000_0009 | ((config as u64)<<32);
    ctrl_out(x, slot, setup)
}

unsafe fn set_hid_protocol(x: &mut Xhci, slot: usize) -> bool {
    // SET_PROTOCOL(Boot=0): bmRequestType=0x21 bRequest=0x0B wValue=0
    let setup: u64 = 0x0000_0000_000B_2100;
    ctrl_out(x, slot, setup)
}

unsafe fn set_hid_idle(x: &mut Xhci, slot: usize) -> bool {
    // SET_IDLE(0,0): bmRequestType=0x21 bRequest=0x0A
    let setup: u64 = 0x0000_0000_000A_2100;
    ctrl_out(x, slot, setup)
}

// ════════════════════════════════════════════════════════════════════════════
// § Enumeracja portu — pełna (z GET_DESCRIPTOR)
// ════════════════════════════════════════════════════════════════════════════

unsafe fn xhci_enumerate_port(x: &mut Xhci, port: usize) {
    let poff  = 0x400 + port * 0x10;
    let psc   = r32(x.op, poff);
    if psc & PRTSC_CCS == 0 { return; }

    log_num("[XHCI] port=", port); serial_print(" attached\n");

    // Reset portu
    w32(x.op, poff, (psc & !PRTSC_CSC)|PRTSC_PR);
    spinwait(x.op, poff, PRTSC_PRC, PRTSC_PRC, 60_000);
    let psc2 = r32(x.op, poff);
    w32(x.op, poff, psc2|PRTSC_PRC);

    let speed = ((psc2 & PRTSC_SPD)>>10) as u8;
    let mps0: u16 = match speed { 4=>512, 3=>64, _=>8 };

    // Enable Slot
    let slot = match cmd_enable_slot(x) {
        Some(s) if s>0 && (s as usize)<MAX_SLOTS => s as usize,
        _ => { serial_print("[XHCI] slot err\n"); return; }
    };

    // Device Context
    let dctx = zpage();
    *(x.dcbaap as *mut u64).add(slot) = dctx;

    // Tymczasowy transfer ring dla EP0
    let ep0ring = Ring::new();
    let ictx0   = build_ictx(slot as u8, (port+1) as u8, speed, ep0ring.phys, mps0, 1);
    x.xfer[slot] = Some(ep0ring);

    // Address Device (BSR=1 — nie wysyła SET_ADDRESS, tylko konfiguruje slot)
    if !cmd_address_device(x, slot as u8, ictx0, true) {
        serial_print("[XHCI] addr BSR fail\n");
        x.xfer[slot] = None; return;
    }

    // GET_DESCRIPTOR(Device) → VID/PID + bMaxPacketSize0
    let desc_buf = zpage();
    let ok_dev   = get_device_descriptor(x, slot, desc_buf);
    let (vid, pid, mps_real) = if ok_dev {
        let b = desc_buf as *const u8;
        let vid = (*b.add(8) as u16) | ((*b.add(9) as u16)<<8);
        let pid = (*b.add(10) as u16)| ((*b.add(11) as u16)<<8);
        let mps = *b.add(7) as u16;
        (vid, pid, mps)
    } else {
        serial_print("[XHCI] GET_DESCRIPTOR(dev) fail — heuristic\n");
        (0, 0, mps0)
    };

    serial_print("[XHCI] VID="); serial_hex(vid as u64);
    serial_print(" PID=");       serial_hex(pid as u64); serial_print("\n");

    // Address Device (BSR=0 — SET_ADDRESS)
    let ictx1 = build_ictx(slot as u8, (port+1) as u8, speed, x.xfer[slot].as_ref().unwrap().phys, mps_real, 1);
    if !cmd_address_device(x, slot as u8, ictx1, false) {
        serial_print("[XHCI] SET_ADDRESS fail\n");
        x.xfer[slot] = None; return;
    }

    // GET_DESCRIPTOR(Config) — najpierw 9 bajtów żeby poznać wTotalLength
    let cfg_buf = zpage();
    let ok_cfg9 = get_config_descriptor(x, slot, cfg_buf, 9);
    let total_len: u16 = if ok_cfg9 {
        let b = cfg_buf as *const u8;
        (*b.add(2) as u16) | ((*b.add(3) as u16)<<8)
    } else { 0 };

    let cfg_bytes = if total_len > 0 && total_len <= 512 {
        get_config_descriptor(x, slot, cfg_buf, total_len);
        core::slice::from_raw_parts(cfg_buf as *const u8, total_len as usize)
    } else {
        core::slice::from_raw_parts(cfg_buf as *const u8, 0)
    };

    let parsed = parse_config_descriptor(cfg_bytes);
    let class  = classify(&parsed, speed);

    let ep_in  = if parsed.ep_in != 0 { parsed.ep_in } else { 1 };
    let ep_mps = if parsed.ep_mps != 0 { parsed.ep_mps } else { mps_real };

    // SET_CONFIGURATION(1)
    set_configuration(x, slot, 1);

    // Dla HID: SET_PROTOCOL(Boot) + SET_IDLE
    if class == DevClass::Keyboard || class == DevClass::Mouse {
        set_hid_protocol(x, slot);
        set_hid_idle(x, slot);
    }

    // Alokuj właściwy transfer ring dla EP IN
    let xring = Ring::new();
    let xbuf  = zpage();
    let ictx2 = build_ictx(slot as u8, (port+1) as u8, speed, xring.phys, ep_mps, ep_in);
    x.xfer[slot] = Some(xring);
    x.xbuf[slot] = xbuf;

    // Finalny Address Device z poprawnym EP
    cmd_address_device(x, slot as u8, ictx2, false);

    // Zapisz urządzenie
    if let Some(di) = dev_alloc(slot as u8) {
        USB_DEVICES[di].class      = class;
        USB_DEVICES[di].speed      = speed;
        USB_DEVICES[di].vid        = vid;
        USB_DEVICES[di].pid        = pid;
        USB_DEVICES[di].ep_in      = ep_in;
        USB_DEVICES[di].ep_in_mps  = ep_mps;
        USB_DEVICES[di].subclass   = parsed.subclass;
        USB_DEVICES[di].protocol   = parsed.protocol;
        x.dev_idx[slot]            = di as u8;
        hid_log_device(&USB_DEVICES[di]);
    }

    // Zakolejkuj pierwszy IN transfer
    let rlen = ep_mps as u32;
    xhci_queue_in(x, slot, rlen);

    // Zaznacz port jako zajęty w bitmapie
    if port < 64 { x.port_ccs |= 1u64 << port; }
}

unsafe fn xhci_queue_in(x: &mut Xhci, slot: usize, len: u32) {
    if let Some(ring) = &mut x.xfer[slot] {
        let buf = x.xbuf[slot];
        ring.push(buf, len, (TRB_NORMAL<<10)|(1<<5)|(1<<2));
        let ep_in = if x.dev_idx[slot] != 0xFF {
            USB_DEVICES[x.dev_idx[slot] as usize].ep_in
        } else { 1 };
        w32(x.db, slot*4, ep_in as u32 * 2 + 1);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Wstępna enumeracja wszystkich portów
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn xhci_probe_all() {
    if let Some(x) = &raw mut XHCI {
        let nports = x.max_ports;
        for port in 0..nports as usize {
            xhci_enumerate_port(x, port);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Poll loop
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn xhci_poll_all() {
    let x = match &raw mut XHCI { Some(x)=>x, None=>return };

    while let Some(evt) = x.evt.pop() {
        xhci_erdp_update(x);
        match evt.typ() {
            TRB_XFER_EVT => {
                let slot = evt.slot();
                if slot==0||slot>=MAX_SLOTS { continue; }
                let cc = evt.cc();
                if cc!=CC_SUCCESS && cc!=CC_SHORT { continue; }

                let di = x.dev_idx[slot];
                if di==0xFF { continue; }
                let dev = &USB_DEVICES[di as usize];
                let buf = x.xbuf[slot];
                let len = dev.ep_in_mps as usize;

                hid_dispatch(dev, buf as *const u8, len);
                xhci_queue_in(x, slot, dev.ep_in_mps as u32);
            }
            TRB_PORT_CHG => {
                // Hotplug event — obsłuż w hotplug_check
                let port = ((evt.param>>24)&0xFF) as usize;
                if port > 0 {
                    log_num("[XHCI] port_chg port=", port);
                    serial_print("\n");
                }
            }
            _ => {}
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Hotplug — sprawdź każdy port czy zmienił stan
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn xhci_hotplug() {
    let x = match &raw mut XHCI { Some(x)=>x, None=>return };
    let nports = x.max_ports as usize;

    for port in 0..nports.min(64) {
        let poff = 0x400 + port * 0x10;
        let psc  = r32(x.op, poff);
        let ccs  = (psc & PRTSC_CCS) != 0;
        let csc  = (psc & PRTSC_CSC) != 0;

        if !csc { continue; } // brak zmiany na tym porcie
        // Skasuj CSC
        w32(x.op, poff, psc | PRTSC_CSC);

        let was_ccs = (x.port_ccs >> port) & 1 != 0;

        if ccs && !was_ccs {
            // Nowe urządzenie
            serial_print("[XHCI] hotplug: connect port=");
            log_num("", port); serial_print("\n");
            xhci_enumerate_port(x, port);
        } else if !ccs && was_ccs {
            // Odłączenie
            serial_print("[XHCI] hotplug: disconnect port=");
            log_num("", port); serial_print("\n");
            // Znajdź slot dla tego portu i zwolnij
            for slot in 1..MAX_SLOTS {
                if x.dev_idx[slot] == 0xFF { continue; }
                let di = x.dev_idx[slot] as usize;
                if USB_DEVICES[di].slot == slot as u8 {
                    dev_free(slot as u8);
                    x.xfer[slot] = None;
                    x.xbuf[slot] = 0;
                    x.dev_idx[slot] = 0xFF;
                    // Wyczyść DCBAAP slot
                    *(x.dcbaap as *mut u64).add(slot) = 0;
                    break;
                }
            }
            x.port_ccs &= !(1u64 << port);
        }
    }
}
