// CosinusOS — usb/mod.rs
// USB subsystem: publiczne API, PCI scan, hotplug manager, wątek poll
//
// Integracja:
//   pub mod usb;   w lib.rs
//   W kernel_main (po mm_init + idt):
//     let usb_ok = usb::usb_init();
//     debug::log_ok("USB", usb_ok);
//     spawn_k("usb\0", usb::usb_thread as *const () as u64, 0);
//
// W perm.rs musi istnieć:
//   pub unsafe fn kb_push_pub(c: char) { kb_push(c); }

#![allow(dead_code)]

pub mod xhci;
pub mod ehci;
pub mod hid;

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use crate::mm::{mm_alloc, vmap, PAGE_SIZE, PTE_W, K_P4};
use crate::debug::{serial_print, serial_hex, num_str};

// ── Re-eksport typów publicznych ──────────────────────────────────────────────
pub use hid::{Mouse, mouse_get, mouse_reset_delta, UsbDevice, DevClass};

// ════════════════════════════════════════════════════════════════════════════
// § PCI helpers (współdzielone przez xhci + ehci)
// ════════════════════════════════════════════════════════════════════════════

pub const PCI_ADDR: u16 = 0xCF8;
pub const PCI_DATA: u16 = 0xCFC;

#[inline(always)]
pub unsafe fn pci_r32(bus: u8, dev: u8, fun: u8, off: u8) -> u32 {
    let a = 0x8000_0000u32
        | (bus as u32) << 16 | (dev as u32) << 11
        | (fun as u32) << 8  | (off as u32 & 0xFC);
    core::arch::asm!("out dx, eax", in("dx") PCI_ADDR, in("eax") a, options(nostack));
    let v: u32;
    core::arch::asm!("in eax, dx", out("eax") v, in("dx") PCI_DATA, options(nostack));
    v
}

#[inline] pub unsafe fn pci_r8(b:u8,d:u8,f:u8,o:u8)->u8 {
    (pci_r32(b,d,f,o&!3)>>((o&3)*8)) as u8 }
#[inline] pub unsafe fn pci_r16(b:u8,d:u8,f:u8,o:u8)->u16 {
    (pci_r32(b,d,f,o&!3)>>((o&2)*8)) as u16 }
pub unsafe fn pci_w32(bus:u8,dev:u8,fun:u8,off:u8,v:u32) {
    let a = 0x8000_0000u32
        |(bus as u32)<<16|(dev as u32)<<11|(fun as u32)<<8|(off as u32&0xFC);
    core::arch::asm!("out dx, eax",in("dx")PCI_ADDR,in("eax")a,options(nostack));
    core::arch::asm!("out dx, eax",in("dx")PCI_DATA,in("eax")v,options(nostack));
}

#[derive(Copy, Clone)]
pub struct Pci {
    pub bus: u8, pub dev: u8, pub fun: u8,
    pub vendor: u16, pub device: u16,
    pub class: u8, pub sub: u8, pub prog: u8,
}

impl Pci {
    pub unsafe fn probe(bus:u8,dev:u8,fun:u8) -> Option<Self> {
        let id = pci_r32(bus,dev,fun,0);
        if id==0xFFFF_FFFF || id as u16==0xFFFF { return None; }
        let cls = pci_r32(bus,dev,fun,8);
        Some(Self { bus,dev,fun,
            vendor: id as u16, device:(id>>16) as u16,
            class:(cls>>24) as u8, sub:(cls>>16) as u8, prog:(cls>>8) as u8 })
    }
    pub unsafe fn bar(&self, n:usize) -> Option<u64> {
        let off = (0x10+n*4) as u8;
        let lo  = pci_r32(self.bus,self.dev,self.fun,off);
        if lo&1!=0 { return None; }
        let a = if (lo>>1)&3==2 {
            let hi = pci_r32(self.bus,self.dev,self.fun,off+4);
            ((hi as u64)<<32)|(lo as u64&!0xF)
        } else { lo as u64&!0xF };
        if a==0 { None } else { Some(a) }
    }
    pub unsafe fn enable(&self) {
        let cmd = pci_r16(self.bus,self.dev,self.fun,4);
        pci_w32(self.bus,self.dev,self.fun,4,cmd as u32|0x06);
    }
}

pub unsafe fn pci_find(class:u8,sub:u8,prog:u8) -> Option<Pci> {
    for bus in 0u8..=255 {
        for d in 0u8..32 {
            let fmax = if pci_r8(bus,d,0,0x0E)&0x80!=0 {8} else {1};
            for f in 0..fmax {
                if let Some(p) = Pci::probe(bus,d,f) {
                    if p.class==class&&p.sub==sub&&p.prog==prog { return Some(p); }
                }
            }
        }
    }
    None
}

// ════════════════════════════════════════════════════════════════════════════
// § MMIO / memory helpers (współdzielone)
// ════════════════════════════════════════════════════════════════════════════

#[inline] pub unsafe fn r32(b:u64,o:usize)->u32 { core::ptr::read_volatile((b+o as u64) as *const u32) }
#[inline] pub unsafe fn w32(b:u64,o:usize,v:u32){ core::ptr::write_volatile((b+o as u64) as *mut u32,v) }
#[inline] pub unsafe fn r64(b:u64,o:usize)->u64  { core::ptr::read_volatile((b+o as u64) as *const u64) }
#[inline] pub unsafe fn w64(b:u64,o:usize,v:u64){ core::ptr::write_volatile((b+o as u64) as *mut u64,v) }

pub unsafe fn map_mmio(base:u64,pages:usize) {
    for i in 0..pages {
        let a = base+i as u64*PAGE_SIZE as u64;
        vmap(K_P4,a,a,PTE_W);
    }
}

pub unsafe fn zpage() -> u64 {
    let p = mm_alloc();
    core::ptr::write_bytes(p as *mut u8,0,PAGE_SIZE);
    p
}

pub unsafe fn spinwait(base:u64,off:usize,mask:u32,want:u32,n:usize)->bool {
    for _ in 0..n {
        if r32(base,off)&mask==want { return true; }
        for _ in 0..800 { core::hint::spin_loop(); }
    }
    false
}

pub fn log_num(label:&str,v:usize) {
    unsafe { serial_print(label); let mut b=[0u8;24]; serial_print(num_str(v,&mut b)); }
}

// ════════════════════════════════════════════════════════════════════════════
// § Stan globalny USB subsystemu
// ════════════════════════════════════════════════════════════════════════════

static USB_UP:   AtomicBool = AtomicBool::new(false);
// Typ aktywnego kontrolera: 0=brak 1=XHCI 2=EHCI
static USB_TYPE: AtomicU8   = AtomicU8::new(0);

// ════════════════════════════════════════════════════════════════════════════
// § Hotplug manager
// ════════════════════════════════════════════════════════════════════════════

// Co ile ticków sprawdzamy hotplug (100 Hz PIT → 50 = co 0.5s)
const HOTPLUG_INTERVAL: u64 = 50;
static mut LAST_HOTPLUG_TICK: u64 = 0;

unsafe fn hotplug_check() {
    let tick = crate::perm::TICK;
    if tick.wrapping_sub(LAST_HOTPLUG_TICK) < HOTPLUG_INTERVAL { return; }
    LAST_HOTPLUG_TICK = tick;

    match USB_TYPE.load(Ordering::Relaxed) {
        1 => xhci::xhci_hotplug(),
        2 => ehci::ehci_hotplug_all(),
        _ => {}
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Publiczne API
// ════════════════════════════════════════════════════════════════════════════

/// Inicjalizuj USB — wywołaj z kernel_main po mm_init + init_idt
pub unsafe fn usb_init() -> bool {
    // Próba XHCI (USB 3.x) — class=0x0C sub=0x03 prog=0x30
    if let Some(p) = pci_find(0x0C, 0x03, 0x30) {
        serial_print("[USB] XHCI znaleziony\n");
        if xhci::xhci_init(p) {
            USB_TYPE.store(1, Ordering::Release);
            USB_UP.store(true, Ordering::Release);
            serial_print("[USB] XHCI OK\n");
            return true;
        }
        serial_print("[USB] XHCI init fail, próba EHCI\n");
    }

    // Fallback EHCI (USB 2.0) — class=0x0C sub=0x03 prog=0x20
    if let Some(p) = pci_find(0x0C, 0x03, 0x20) {
        serial_print("[USB] EHCI znaleziony\n");
        if ehci::ehci_init(p) {
            USB_TYPE.store(2, Ordering::Release);
            USB_UP.store(true, Ordering::Release);
            serial_print("[USB] EHCI OK\n");
            return true;
        }
    }

    // OHCI (USB 1.x companion) — class=0x0C sub=0x03 prog=0x10
    if let Some(p) = pci_find(0x0C, 0x03, 0x10) {
        serial_print("[USB] OHCI znaleziony\n");
        if ehci::ohci_init(p) {
            USB_TYPE.store(2, Ordering::Release);
            USB_UP.store(true, Ordering::Release);
            serial_print("[USB] OHCI OK\n");
            return true;
        }
    }

    serial_print("[USB] brak kontrolera\n");
    false
}

/// Poll — wywołuj z usb_thread w pętli
pub unsafe fn usb_poll() {
    if !USB_UP.load(Ordering::Relaxed) { return; }
    match USB_TYPE.load(Ordering::Relaxed) {
        1 => xhci::xhci_poll_all(),
        2 => ehci::ehci_poll_all_controllers(),
        _ => {}
    }
    hotplug_check();
}

/// Wątek kernelowy USB
pub unsafe extern "C" fn usb_thread(_: u64) -> ! {
    loop {
        usb_poll();
        crate::threading::thread_yield();
    }
}

pub fn usb_ok() -> bool { USB_UP.load(Ordering::Relaxed) }

/// Zwróć liczbę podłączonych urządzeń HID
pub fn usb_hid_count() -> usize { hid::hid_device_count() }
