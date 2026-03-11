// CosinusOS — usb.rs
// USB Host Controller driver: XHCI (USB 3.x) + EHCI (USB 2.0 fallback)
// HID: klawiatura (Boot Protocol) + mysz
// PCI enumaracja wbudowana — nie wymaga osobnego pci.rs
//
// Integracja z kernelem:
//   1. pub mod usb;  w lib.rs
//   2. W perm.rs dodaj: pub unsafe fn kb_push_pub(c: char) { kb_push(c); }
//   3. W kernel_main po init_pit():
//        let usb_ok = usb::usb_init();
//        debug::log_ok("USB", usb_ok);
//        spawn_k("usb\0", usb::usb_thread as *const () as u64, 0);

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};
use crate::mm::{mm_alloc, vmap, PhysAddr, VirtAddr, PAGE_SIZE, PTE_W, K_P4};
use crate::debug::{serial_print, serial_hex, num_str};

// ════════════════════════════════════════════════════════════════════════════
// § PCI — port I/O config space + BAR + enumeracja
// ════════════════════════════════════════════════════════════════════════════

const PCI_ADDR: u16 = 0xCF8;
const PCI_DATA: u16 = 0xCFC;

#[inline(always)]
unsafe fn pci_r32(bus: u8, dev: u8, fun: u8, off: u8) -> u32 {
    let a = 0x8000_0000u32
        | (bus as u32) << 16 | (dev as u32) << 11
        | (fun as u32) << 8  | (off as u32 & 0xFC);
    core::arch::asm!("out dx, eax", in("dx") PCI_ADDR, in("eax") a, options(nostack));
    let v: u32;
    core::arch::asm!("in eax, dx", out("eax") v, in("dx") PCI_DATA, options(nostack));
    v
}

#[inline] unsafe fn pci_r8(b: u8, d: u8, f: u8, o: u8) -> u8 {
    (pci_r32(b,d,f,o&!3) >> ((o&3)*8)) as u8 }
#[inline] unsafe fn pci_r16(b: u8, d: u8, f: u8, o: u8) -> u16 {
    (pci_r32(b,d,f,o&!3) >> ((o&2)*8)) as u16 }

unsafe fn pci_w32(bus: u8, dev: u8, fun: u8, off: u8, v: u32) {
    let a = 0x8000_0000u32
        | (bus as u32) << 16 | (dev as u32) << 11
        | (fun as u32) << 8  | (off as u32 & 0xFC);
    core::arch::asm!("out dx, eax", in("dx") PCI_ADDR, in("eax") a, options(nostack));
    core::arch::asm!("out dx, eax", in("dx") PCI_DATA, in("eax") v, options(nostack));
}

#[derive(Copy, Clone)]
struct Pci { bus: u8, dev: u8, fun: u8, vendor: u16, device: u16,
             class: u8, sub: u8, prog: u8 }

impl Pci {
    unsafe fn probe(bus: u8, dev: u8, fun: u8) -> Option<Self> {
        let id = pci_r32(bus, dev, fun, 0);
        if id == 0xFFFF_FFFF || id as u16 == 0xFFFF { return None; }
        let cls = pci_r32(bus, dev, fun, 8);
        Some(Self { bus, dev, fun,
            vendor: id as u16, device: (id>>16) as u16,
            class: (cls>>24) as u8, sub: (cls>>16) as u8, prog: (cls>>8) as u8 })
    }

    /// Zwróć adres MMIO BARu n (64 lub 32-bit, nie I/O)
    unsafe fn bar(&self, n: usize) -> Option<u64> {
        let off = (0x10 + n*4) as u8;
        let lo  = pci_r32(self.bus, self.dev, self.fun, off);
        if lo & 1 != 0 { return None; }
        let a = if (lo>>1)&3 == 2 {
            let hi = pci_r32(self.bus, self.dev, self.fun, off+4);
            ((hi as u64)<<32) | (lo as u64 & !0xF)
        } else { lo as u64 & !0xF };
        if a == 0 { None } else { Some(a) }
    }

    /// Memory Space + Bus Master
    unsafe fn enable(&self) {
        let cmd = pci_r16(self.bus, self.dev, self.fun, 4);
        pci_w32(self.bus, self.dev, self.fun, 4, cmd as u32 | 0x06);
    }
}

/// Znajdź pierwsze urządzenie pasujące do class/sub/prog
unsafe fn pci_find(class: u8, sub: u8, prog: u8) -> Option<Pci> {
    for bus in 0u8..=255 {
        for d in 0u8..32 {
            let fmax = if pci_r8(bus,d,0,0x0E) & 0x80 != 0 { 8 } else { 1 };
            for f in 0..fmax {
                if let Some(p) = Pci::probe(bus, d, f) {
                    if p.class==class && p.sub==sub && p.prog==prog { return Some(p); }
                }
            }
        }
    }
    None
}

// ════════════════════════════════════════════════════════════════════════════
// § MMIO / memory helpers
// ════════════════════════════════════════════════════════════════════════════

#[inline] unsafe fn r32(b: u64, o: usize) -> u32 { core::ptr::read_volatile((b+o as u64) as *const u32) }
#[inline] unsafe fn w32(b: u64, o: usize, v: u32) { core::ptr::write_volatile((b+o as u64) as *mut u32, v) }
#[inline] unsafe fn r64(b: u64, o: usize) -> u64  { core::ptr::read_volatile((b+o as u64) as *const u64) }
#[inline] unsafe fn w64(b: u64, o: usize, v: u64) { core::ptr::write_volatile((b+o as u64) as *mut u64, v) }

unsafe fn map_mmio(base: u64, pages: usize) {
    for i in 0..pages {
        let a = base + i as u64 * PAGE_SIZE as u64;
        vmap(K_P4, a, a, PTE_W);
    }
}

unsafe fn zpage() -> u64 {
    let p = mm_alloc();
    core::ptr::write_bytes(p as *mut u8, 0, PAGE_SIZE);
    p
}

unsafe fn spinwait(base: u64, off: usize, mask: u32, want: u32, n: usize) -> bool {
    for _ in 0..n {
        if r32(base,off) & mask == want { return true; }
        for _ in 0..800 { core::hint::spin_loop(); }
    }
    false
}

fn log_num(label: &str, v: usize) {
    unsafe { serial_print(label); let mut b=[0u8;24]; serial_print(num_str(v,&mut b)); }
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — Transfer Request Block
// ════════════════════════════════════════════════════════════════════════════

#[repr(C, align(16))] #[derive(Copy,Clone,Default)]
struct Trb { param: u64, status: u32, ctrl: u32 }

impl Trb {
    #[inline] fn typ(self)   -> u32  { (self.ctrl>>10)&0x3F }
    #[inline] fn cycle(self) -> bool { self.ctrl & 1 != 0 }
    #[inline] fn slot(self)  -> usize { (self.ctrl>>24) as usize }
    #[inline] fn cc(self)    -> u32  { (self.status>>24)&0xFF }
}

const TRB_NORMAL:    u32 = 1;
const TRB_SETUP:     u32 = 2;
const TRB_STATUS:    u32 = 4;
const TRB_LINK:      u32 = 6;
const TRB_EN_SLOT:   u32 = 9;
const TRB_ADDR_DEV:  u32 = 11;
const TRB_XFER_EVT:  u32 = 32;
const TRB_CMD_CMPL:  u32 = 33;
const TRB_PORT_CHG:  u32 = 34;
const CC_SUCCESS:    u32 = 1;
const CC_SHORT:      u32 = 13;

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — Producer ring + Event ring
// ════════════════════════════════════════════════════════════════════════════

const RSIZ: usize = 255; // TRBów per ring (+ 1 LINK = 256 = 1 strona)

struct Ring { phys: u64, ptr: *mut Trb, enq: usize, pcs: bool }

impl Ring {
    unsafe fn new() -> Self {
        let p = zpage();
        let lnk = &mut *(p as *mut Trb).add(RSIZ);
        lnk.param = p;
        lnk.ctrl  = (TRB_LINK<<10) | 1 | (1<<1); // TC=1
        Self { phys:p, ptr: p as *mut Trb, enq:0, pcs:true }
    }
    unsafe fn push(&mut self, param: u64, status: u32, ctrl: u32) -> u64 {
        let i = self.enq;
        let t = &mut *self.ptr.add(i);
        t.param = param; t.status = status; t.ctrl = ctrl | self.pcs as u32;
        let pa = self.phys + i as u64 * 16;
        self.enq += 1;
        if self.enq >= RSIZ {
            let lnk = &mut *self.ptr.add(RSIZ);
            if self.pcs { lnk.ctrl |= 1; } else { lnk.ctrl &= !1; }
            self.enq = 0; self.pcs = !self.pcs;
        }
        pa
    }
}

struct EvtRing { phys: u64, erst: u64, ptr: *mut Trb, deq: usize, ccs: bool }

impl EvtRing {
    unsafe fn new() -> Self {
        let p  = zpage();
        let er = zpage();
        *(er as *mut u64)       = p;
        *(er as *mut u64).add(1) = RSIZ as u64;
        Self { phys:p, erst:er, ptr:p as *mut Trb, deq:0, ccs:true }
    }
    unsafe fn pop(&mut self) -> Option<Trb> {
        let t = *self.ptr.add(self.deq);
        if t.cycle() != self.ccs { return None; }
        self.deq += 1;
        if self.deq >= RSIZ { self.deq=0; self.ccs=!self.ccs; }
        Some(t)
    }
    fn erdp(&self) -> u64 { self.phys + self.deq as u64 * 16 }
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — rejestrowe stałe
// ════════════════════════════════════════════════════════════════════════════

const CAP_CAPLENGTH:  usize = 0x00;
const CAP_HCSPARAMS1: usize = 0x04;
const CAP_DBOFF:      usize = 0x14;
const CAP_RTSOFF:     usize = 0x18;
const OP_USBCMD:      usize = 0x00;
const OP_USBSTS:      usize = 0x04;
const OP_DNCTRL:      usize = 0x14;
const OP_CRCR:        usize = 0x18;
const OP_DCBAAP:      usize = 0x30;
const OP_CONFIG:      usize = 0x38;
const CMD_RUN:        u32   = 1;
const CMD_RST:        u32   = 1<<1;
const CMD_INTE:       u32   = 1<<2;
const STS_HCH:        u32   = 1;
const STS_CNR:        u32   = 1<<11;
const PRTSC_CCS:      u32   = 1;
const PRTSC_PR:       u32   = 1<<4;
const PRTSC_CSC:      u32   = 1<<17;
const PRTSC_PRC:      u32   = 1<<21;
const PRTSC_SPD:      u32   = 0xF<<10;

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — główna struktura + init
// ════════════════════════════════════════════════════════════════════════════

const MAX_SLOTS: usize = 32;

struct Xhci {
    cap: u64, op: u64, rt: u64, db: u64,
    max_ports: u8,
    cmd: Ring, evt: EvtRing,
    dcbaap: u64,
    xfer:  [Option<Ring>; MAX_SLOTS],
    xbuf:  [u64; MAX_SLOTS],
    dcls:  [u8;  MAX_SLOTS], // 0=brak 1=kb 2=mouse
}

static mut XHCI: Option<Xhci> = None;

unsafe fn xhci_init(pci: Pci) -> bool {
    let bar = match pci.bar(0) { Some(b)=>b, None => {
        serial_print("[XHCI] brak BAR\n"); return false; }};
    map_mmio(bar, 8);
    pci.enable();

    let clen = (r32(bar, CAP_CAPLENGTH) & 0xFF) as usize;
    let op   = bar + clen as u64;
    let rt   = bar + r32(bar, CAP_RTSOFF) as u64;
    let db   = bar + r32(bar, CAP_DBOFF)  as u64;
    let hcs1 = r32(bar, CAP_HCSPARAMS1);
    let mslots = (hcs1 & 0xFF).min(MAX_SLOTS as u32) as u8;
    let mports = (hcs1 >> 24) as u8;

    serial_print("[XHCI] bar="); serial_hex(bar);
    log_num(" ports=", mports as usize);
    serial_print("\n");

    // Reset
    w32(op, OP_USBCMD, 0);
    if !spinwait(op, OP_USBSTS, STS_CNR, 0, 2000) { serial_print("[XHCI] CNR\n"); return false; }
    w32(op, OP_USBCMD, CMD_RST);
    if !spinwait(op, OP_USBCMD, CMD_RST, 0, 2000) { serial_print("[XHCI] RST\n"); return false; }
    if !spinwait(op, OP_USBSTS, STS_CNR, 0, 2000) { serial_print("[XHCI] CNR2\n"); return false; }

    w32(op, OP_CONFIG, mslots as u32);

    let dcbaap = zpage();
    w64(op, OP_DCBAAP, dcbaap);

    let cmd = Ring::new();
    w64(op, OP_CRCR, cmd.phys | 1); // RCS=1

    let evt = EvtRing::new();
    w32(rt, 0x028, 1);          // ERSTSZ=1
    w64(rt, 0x030, evt.erst);   // ERSTBA
    w64(rt, 0x038, evt.phys);   // ERDP
    w32(rt, 0x020, r32(rt,0x020) | 3); // IMAN: IE+IP

    w32(op, OP_USBCMD, CMD_RUN | CMD_INTE);
    if !spinwait(op, OP_USBSTS, STS_HCH, 0, 1000) { serial_print("[XHCI] start\n"); return false; }

    XHCI = Some(Xhci {
        cap:bar, op, rt, db, max_ports:mports,
        cmd, evt, dcbaap,
        xfer: core::array::from_fn(|_| None),
        xbuf: [0u64; MAX_SLOTS],
        dcls: [0u8;  MAX_SLOTS],
    });
    serial_print("[XHCI] OK\n");
    true
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — command ring helpers
// ════════════════════════════════════════════════════════════════════════════

unsafe fn xhci_ring_db(x: &Xhci, slot: usize, ep: u32) {
    w32(x.db, slot * 4, ep);
}

unsafe fn xhci_erdp(x: &Xhci) {
    w64(x.rt, 0x038, x.evt.erdp() | (1<<3));
}

/// Wyślij komendę i poczekaj na CMD_COMPLETION event (polling)
unsafe fn xhci_cmd_wait(x: &mut Xhci) -> Option<Trb> {
    xhci_ring_db(x, 0, 0);
    for _ in 0..300_000 {
        if let Some(e) = x.evt.pop() {
            xhci_erdp(x);
            if e.typ() == TRB_CMD_CMPL { return Some(e); }
        }
        core::hint::spin_loop();
    }
    None
}

unsafe fn cmd_enable_slot(x: &mut Xhci) -> Option<u8> {
    x.cmd.push(0, 0, TRB_EN_SLOT << 10);
    let e = xhci_cmd_wait(x)?;
    if e.cc() != CC_SUCCESS { return None; }
    Some((e.ctrl >> 24) as u8)
}

unsafe fn cmd_address_device(x: &mut Xhci, slot: u8, ictx: u64) -> bool {
    x.cmd.push(ictx, 0, (TRB_ADDR_DEV<<10) | ((slot as u32)<<24));
    xhci_cmd_wait(x).map_or(false, |e| e.cc() == CC_SUCCESS)
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — Input Context (HID Interrupt IN ep1)
// ════════════════════════════════════════════════════════════════════════════

unsafe fn build_ictx(slot: u8, port: u8, speed: u8, ep_phys: u64, mps: u16) -> u64 {
    let p = zpage() as *mut u32;
    // Input Control Context: Add bits [0]=slot [1]=slot_ctx [3]=ep1_IN
    *p.add(1) = (1<<0)|(1<<1)|(1<<3);
    // Slot Context (@ off 0x20 = idx 8)
    let sc = p.add(8);
    *sc.add(0) = ((speed as u32)<<20) | (1<<27); // Speed | CtxEntries=1
    *sc.add(1) = (port as u32) << 16;
    // EP1 IN Context (@ off 0x60 = idx 24)
    // EPType=3(IntIn) CErr=3 MaxBurst=0
    let ep = p.add(24);
    *ep.add(1) = (3<<1) | (3<<3) | (3<<16);         // Mult CErr EPType=IntIN
    *ep.add(2) = (ep_phys as u32 & !0xF) | 1;        // Dequeue Lo | DCS
    *ep.add(3) = (ep_phys >> 32) as u32;
    *ep.add(4) = mps as u32 | (0<<16);               // MaxPacketSize
    p as u64
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — control transfer (SET_PROTOCOL + SET_IDLE) na ep0
// ════════════════════════════════════════════════════════════════════════════

unsafe fn xhci_set_protocol(x: &mut Xhci, slot: usize) {
    // SET_PROTOCOL(Boot=0): bmRequestType=0x21 bRequest=0x0B wValue=0 wIndex=0 wLength=0
    let setup1: u64 = 0x0000_0000_000B_2100;
    x.cmd.push(setup1, 8, (TRB_SETUP<<10) | (3<<16) | 1);  // TRT=3 IDT=1
    x.cmd.push(0,      0, (TRB_STATUS<<10) | (1<<16) | 1); // Dir=IN
    xhci_ring_db(x, slot, 1); // ep0
    for _ in 0..80_000 { core::hint::spin_loop(); }

    // SET_IDLE(0): bmRequestType=0x21 bRequest=0x0A wValue=0 wIndex=0 wLength=0
    let setup2: u64 = 0x0000_0000_000A_2100;
    x.cmd.push(setup2, 8, (TRB_SETUP<<10) | (3<<16) | 1);
    x.cmd.push(0,      0, (TRB_STATUS<<10) | (1<<16) | 1);
    xhci_ring_db(x, slot, 1);
    for _ in 0..80_000 { core::hint::spin_loop(); }
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — zakolejkuj Interrupt IN transfer
// ════════════════════════════════════════════════════════════════════════════

unsafe fn xhci_queue_in(x: &mut Xhci, slot: usize, len: u32) {
    if let Some(ring) = &mut x.xfer[slot] {
        let buf = x.xbuf[slot];
        ring.push(buf, len, (TRB_NORMAL<<10) | (1<<5) | (1<<2)); // ISP|ENT
        w32(x.db, slot*4, 3); // ep1 IN = doorbell target 3
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — enumeracja portów
// ════════════════════════════════════════════════════════════════════════════

unsafe fn xhci_probe(x: &mut Xhci) {
    for port in 0..x.max_ports as usize {
        let poff = 0x400 + port * 0x10;
        let psc  = r32(x.op, poff);
        if psc & PRTSC_CCS == 0 { continue; }

        log_num("[XHCI] port ", port); serial_print(" connected\n");

        // Port reset
        w32(x.op, poff, (psc & !PRTSC_CSC) | PRTSC_PR);
        spinwait(x.op, poff, PRTSC_PRC, PRTSC_PRC, 60_000);
        let psc2 = r32(x.op, poff);
        w32(x.op, poff, psc2 | PRTSC_PRC); // skasuj PRC

        let speed = ((psc2 & PRTSC_SPD) >> 10) as u8;
        let mps: u16 = match speed { 4=>512, 3=>64, _=>8 };

        let slot = match cmd_enable_slot(x) {
            Some(s) if (s as usize) < MAX_SLOTS && s > 0 => s as usize,
            _ => { serial_print("[XHCI] slot err\n"); continue; }
        };

        // Device Context
        let dctx = zpage();
        *(x.dcbaap as *mut u64).add(slot) = dctx;

        // Transfer ring + data buffer
        let xring = Ring::new();
        let xbuf  = zpage();
        let ictx  = build_ictx(slot as u8, (port+1) as u8, speed, xring.phys, mps);

        x.xbuf[slot] = xbuf;
        x.xfer[slot] = Some(xring);

        if !cmd_address_device(x, slot as u8, ictx) {
            serial_print("[XHCI] addr err\n");
            x.xfer[slot] = None; continue;
        }

        xhci_set_protocol(x, slot);

        // Heurystyka: LowSpeed(1)/FullSpeed(2) → klawiatura, reszta → mysz
        x.dcls[slot] = if speed <= 2 { 1 } else { 2 };
        let rlen = if x.dcls[slot] == 1 { 8u32 } else { 4u32 };
        xhci_queue_in(x, slot, rlen);

        log_num("[XHCI] HID slot=", slot);
        serial_print(if x.dcls[slot]==1 { " KB\n" } else { " mouse\n" });
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § XHCI — poll loop
// ════════════════════════════════════════════════════════════════════════════

unsafe fn xhci_poll(x: &mut Xhci) {
    while let Some(evt) = x.evt.pop() {
        xhci_erdp(x);
        match evt.typ() {
            TRB_XFER_EVT => {
                let slot = evt.slot();
                if slot == 0 || slot >= MAX_SLOTS { continue; }
                let cc = evt.cc();
                if cc != CC_SUCCESS && cc != CC_SHORT { continue; }
                let buf = x.xbuf[slot] as *const u8;
                match x.dcls[slot] {
                    1 => hid_kb(&*(buf as *const [u8;8])),
                    2 => hid_mouse(core::slice::from_raw_parts(buf, 4)),
                    _ => {}
                }
                let rlen = if x.dcls[slot]==2 { 4u32 } else { 8u32 };
                xhci_queue_in(x, slot, rlen);
            }
            TRB_PORT_CHG => { serial_print("[XHCI] hotplug\n"); }
            _ => {}
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § EHCI — USB 2.0 fallback (Periodic Schedule + Interrupt QH per port)
// ════════════════════════════════════════════════════════════════════════════

const EHCI_CAPLENGTH: usize = 0x00;
const EHCI_HCSPAR:    usize = 0x04;
const EHCI_CMD:       usize = 0x00;  // operational
const EHCI_STS:       usize = 0x04;
const EHCI_INTR:      usize = 0x08;
const EHCI_FRBASE:    usize = 0x14;
const EHCI_ASYNCBASE: usize = 0x18;
const EHCI_CFLAG:     usize = 0x40;
const EHCI_PORTSC:    usize = 0x44;
const ECMD_RUN:  u32 = 1;
const ECMD_RST:  u32 = 1<<1;
const ECMD_PSE:  u32 = 1<<4;
const ECMD_ASE:  u32 = 1<<5;
const ESTS_HCH:  u32 = 1<<12;
const EPRT_CCS:  u32 = 1;
const EPRT_PE:   u32 = 1<<2;
const EPRT_RST:  u32 = 1<<8;
const EPRT_OWN:  u32 = 1<<13; // release to companion

const EHCI_QH_TYP: u32 = 2<<1; // H-link type = QH
const ETD_ACTIVE:  u32 = 1<<7;

#[repr(C,align(32))]
struct EhciQH {
    next:    u32, // horizontal link pointer
    epchar:  u32, // endpoint characteristics
    epcap:   u32, // endpoint capabilities
    cur_td:  u32,
    next_td: u32,
    alt_td:  u32,
    token:   u32,
    buf:     [u32;5],
    _pad:    [u32;3],
}

#[repr(C,align(32))]
struct EhciTD {
    next:  u32,
    alt:   u32,
    token: u32,
    buf:   [u32;5],
}

const MAX_EHCI_PORTS: usize = 8;

struct Ehci {
    op: u64,
    n_ports: u8,
    pfl:     u64,  // Periodic Frame List (phys = virt, identity mapped)
    aqh:     u64,  // Async dummy QH
    hid_qh:  [u64; MAX_EHCI_PORTS],
    hid_buf: [u64; MAX_EHCI_PORTS],
    hid_cls: [u8;  MAX_EHCI_PORTS],
}

static mut EHCI: Option<Ehci> = None;

unsafe fn ehci_init(pci: Pci) -> bool {
    let bar = match pci.bar(0) { Some(b)=>b, None => {
        serial_print("[EHCI] brak BAR\n"); return false; }};
    map_mmio(bar, 4);
    pci.enable();

    let clen   = (r32(bar, EHCI_CAPLENGTH) & 0xFF) as usize;
    let op     = bar + clen as u64;
    let hcsp   = r32(bar, EHCI_HCSPAR);
    let nports = (hcsp & 0xF) as u8;

    serial_print("[EHCI] bar="); serial_hex(bar);
    log_num(" ports=", nports as usize); serial_print("\n");

    // Reset
    w32(op, EHCI_CMD, ECMD_RST);
    if !spinwait(op, EHCI_CMD, ECMD_RST, 0, 1000) {
        serial_print("[EHCI] RST timeout\n"); return false; }

    // Periodic Frame List (1024 × 4B, wypełnij terminate=1)
    let pfl = zpage();
    for i in 0..1024usize { *(pfl as *mut u32).add(i) = 1; }

    // Async dummy QH (głowa listy async, wskazuje na siebie)
    let aqh_p = zpage() as *mut EhciQH;
    (*aqh_p).next    = (aqh_p as u32) | EHCI_QH_TYP | (1<<15); // H=1
    (*aqh_p).next_td = 1;
    (*aqh_p).alt_td  = 1;
    (*aqh_p).token   = 0;

    w32(op, EHCI_INTR,    0);
    w32(op, EHCI_STS,     0x3F);          // clear all
    w32(op, EHCI_FRBASE,  pfl as u32);
    w32(op, EHCI_ASYNCBASE, aqh_p as u32);
    w32(op, EHCI_CFLAG,   1);             // CF=1 → EHCI routing

    // FLS=0 → 1024 ramek, Run
    w32(op, EHCI_CMD, ECMD_RUN | ECMD_PSE | ECMD_ASE);

    let mut hid_qh  = [0u64; MAX_EHCI_PORTS];
    let mut hid_buf = [0u64; MAX_EHCI_PORTS];
    let mut hid_cls = [0u8;  MAX_EHCI_PORTS];

    for p in 0..nports as usize {
        let poff = EHCI_PORTSC + p*4;
        let psc  = r32(op, poff);
        if psc & EPRT_CCS == 0 { continue; }

        // Sprawdź prędkość — Low/Full-Speed → oddaj do companion (OHCI/UHCI)
        let chirp = (psc >> 26) & 3;
        if chirp != 2 {
            w32(op, poff, psc | EPRT_OWN);
            log_num("[EHCI] port ", p); serial_print(" -> companion\n");
            continue;
        }

        // HighSpeed port — reset i konfiguruj HID
        w32(op, poff, (psc & !EPRT_PE) | EPRT_RST);
        for _ in 0..25_000 { core::hint::spin_loop(); }
        w32(op, poff, r32(op, poff) & !EPRT_RST);
        for _ in 0..10_000 { core::hint::spin_loop(); }

        if r32(op, poff) & EPRT_PE == 0 {
            log_num("[EHCI] port ", p); serial_print(" enable failed\n"); continue; }

        // Alokuj QH + TD + bufor
        let qh_p = zpage() as *mut EhciQH;
        let td_p = mm_alloc() as *mut EhciTD; // nie zerujemy — piszemy wszystko
        let buf  = zpage();

        (*td_p).next  = 1;
        (*td_p).alt   = 1;
        // active, IN(0x69=PID_IN w tokenie), 8 bajtów, IOC
        (*td_p).token = ETD_ACTIVE | (0x69<<8) | (8<<16) | (1<<15);
        (*td_p).buf[0] = buf as u32;

        // QH: addr=1 ep=1 HS max_pkt=64
        let hlink = if p > 0 { hid_qh[p-1] as u32 } else { qh_p as u32 };
        (*qh_p).next    = hlink | EHCI_QH_TYP; // linkuj w kółko lub na dummy
        (*qh_p).epchar  = 1 | (1<<8) | (2<<12) | (64<<16); // addr ep speed mps
        (*qh_p).epcap   = 1; // s-mask bit 0 = frame 0
        (*qh_p).cur_td  = 0;
        (*qh_p).next_td = td_p as u32;
        (*qh_p).alt_td  = 1;
        (*qh_p).token   = 0;

        // Wstaw QH do każdej ramki PFL
        let link = (qh_p as u32) | EHCI_QH_TYP;
        for i in 0..1024usize { *(pfl as *mut u32).add(i) = link; }

        hid_qh[p]  = qh_p as u64;
        hid_buf[p] = buf;
        hid_cls[p] = 1; // domyślnie klawiatura
        log_num("[EHCI] HID port ", p); serial_print("\n");
    }

    EHCI = Some(Ehci { op, n_ports:nports, pfl, aqh: aqh_p as u64,
                        hid_qh, hid_buf, hid_cls });
    serial_print("[EHCI] OK\n");
    true
}

unsafe fn ehci_poll(e: &mut Ehci) {
    for p in 0..e.n_ports.min(MAX_EHCI_PORTS as u8) as usize {
        let qh_p = e.hid_qh[p];
        if qh_p == 0 { continue; }
        let qh = &mut *(qh_p as *mut EhciQH);
        if qh.token & ETD_ACTIVE != 0 { continue; } // jeszcze w trakcie

        let buf = e.hid_buf[p] as *const u8;
        match e.hid_cls[p] {
            1 => hid_kb(&*(buf as *const [u8;8])),
            2 => hid_mouse(core::slice::from_raw_parts(buf, 4)),
            _ => {}
        }

        // Restart: nowy TD
        let td_p = mm_alloc() as *mut EhciTD;
        (*td_p).next   = 1;
        (*td_p).alt    = 1;
        (*td_p).token  = ETD_ACTIVE | (0x69<<8) | (8<<16) | (1<<15);
        (*td_p).buf[0] = e.hid_buf[p] as u32;
        qh.next_td = td_p as u32;
        qh.token   = 0;
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § HID Boot Protocol — klawiatura (8-bajtowy raport)
// ════════════════════════════════════════════════════════════════════════════

static HID_NORM: [u8;104] = [
    0,0,0,0,
    b'a',b'b',b'c',b'd',b'e',b'f',b'g',b'h',b'i',b'j',b'k',b'l',
    b'm',b'n',b'o',b'p',b'q',b'r',b's',b't',b'u',b'v',b'w',b'x',b'y',b'z',
    b'1',b'2',b'3',b'4',b'5',b'6',b'7',b'8',b'9',b'0',
    b'\n',b'\x1b',b'\x08',b'\t',b' ',
    b'-',b'=',b'[',b']',b'\\',0,b';',b'\'',b'`',b',',b'.',b'/',
    0,    // CapsLock (57)
    0,0,0,0,0,0,0,0,0,0,0,0,  // F1-F12 (58-69)
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  // 70-87
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,      // 88-103
];
static HID_SHFT: [u8;103] = [
    0,0,0,0,
    b'A',b'B',b'C',b'D',b'E',b'F',b'G',b'H',b'I',b'J',b'K',b'L',
    b'M',b'N',b'O',b'P',b'Q',b'R',b'S',b'T',b'U',b'V',b'W',b'X',b'Y',b'Z',
    b'!',b'@',b'#',b'$',b'%',b'^',b'&',b'*',b'(',b')',
    b'\n',b'\x1b',b'\x08',b'\t',b' ',
    b'_',b'+',b'{',b'}',b'|',0,b':',b'"',b'~',b'<',b'>',b'?',
    0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
];

static mut PREV_KB: [u8;8] = [0u8;8];

unsafe fn hid_kb(rep: &[u8;8]) {
    if *rep == PREV_KB { return; }
    let shift = rep[0] & 0x22 != 0; // LShift | RShift
    for &k in &rep[2..8] {
        if k < 4 { continue; }
        if PREV_KB[2..8].contains(&k) { continue; } // tylko nowe klawisze
        let idx = k as usize;
        if idx >= HID_NORM.len() { continue; }
        let c = if shift { HID_SHFT[idx] } else { HID_NORM[idx] };
        if c != 0 { crate::perm::kb_push_pub(c as char); }
    }
    PREV_KB = *rep;
}

// ════════════════════════════════════════════════════════════════════════════
// § HID Boot Protocol — mysz (4-bajtowy raport)
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy,Clone,Default)]
pub struct Mouse {
    pub buttons: u8,
    pub x: i16, pub y: i16,   // absolutne (saturate)
    pub dx: i8,  pub dy: i8,  // ostatnie delta
    pub scroll: i8,
}

static mut MOUSE: Mouse = Mouse { buttons:0, x:0, y:0, dx:0, dy:0, scroll:0 };

unsafe fn hid_mouse(rep: &[u8]) {
    if rep.len() < 3 { return; }
    MOUSE.buttons = rep[0];
    MOUSE.dx      = rep[1] as i8;
    MOUSE.dy      = rep[2] as i8;
    MOUSE.scroll  = if rep.len() > 3 { rep[3] as i8 } else { 0 };
    MOUSE.x = MOUSE.x.saturating_add(MOUSE.dx as i16);
    MOUSE.y = MOUSE.y.saturating_add(MOUSE.dy as i16);
}

pub unsafe fn mouse_get()        -> Mouse { MOUSE }
pub unsafe fn mouse_reset_delta() { MOUSE.dx=0; MOUSE.dy=0; MOUSE.scroll=0; }

// ════════════════════════════════════════════════════════════════════════════
// § Publiczne API
// ════════════════════════════════════════════════════════════════════════════

static USB_UP: AtomicBool = AtomicBool::new(false);

/// Inicjalizuj USB — wywołaj z kernel_main
/// Próbuje XHCI najpierw (USB 3.x), EHCI jako fallback (USB 2.0)
pub unsafe fn usb_init() -> bool {
    // XHCI: PCI class=0x0C sub=0x03 prog_if=0x30
    let ok = match pci_find(0x0C, 0x03, 0x30) {
        Some(p) => { serial_print("[USB] XHCI\n"); xhci_init(p) }
        None    => {
            serial_print("[USB] brak XHCI, próba EHCI\n");
            // EHCI: PCI class=0x0C sub=0x03 prog_if=0x20
            match pci_find(0x0C, 0x03, 0x20) {
                Some(p) => { serial_print("[USB] EHCI\n"); ehci_init(p) }
                None    => { serial_print("[USB] brak kontrolera\n"); false }
            }
        }
    };
    USB_UP.store(ok, Ordering::Release);
    ok
}

/// Poll wszystkich aktywnych kontrolerów — wywołaj z wątku kernelowego
pub unsafe fn usb_poll() {
    if let Some(x) = &mut XHCI { xhci_poll(x); }
    if let Some(e) = &mut EHCI  { ehci_poll(e); }
}

/// Wątek kernelowy: spawn_k("usb\0", usb_thread as *const () as u64, 0)
pub unsafe extern "C" fn usb_thread(_: u64) -> ! {
    loop {
        if USB_UP.load(Ordering::Relaxed) { usb_poll(); }
        crate::threading::thread_yield();
    }
}

pub fn usb_ok() -> bool { USB_UP.load(Ordering::Relaxed) }