// CosinusOS — display/mod.rs
// Publiczne API display subsystemu + autodetect GPU + bridge do debug::putc
//
// Integracja z kernelem:
//   pub mod display; w lib.rs
//
//   W kernel_main po usb_init():
//     let disp_ok = display::display_init();
//     debug::log_ok("Display", disp_ok);
//
// Po inicjalizacji display::putc() zastępuje VGA 0xB8000 —
// wystarczy że debug.rs wywoła go jeśli FB jest aktywny.

pub mod fb;
pub mod otg;
pub mod amd;
pub mod intel;

use core::sync::atomic::{AtomicBool, Ordering};
use crate::debug::{serial_print, serial_hex};
use fb::{fb_alloc, fb_fill, fb_putc, fb_set_color, fb_cls, FB_PHYS, FB_VIRT};
use amd::{AmdState, DcnGen, dcn_gen_from_did, AMD_VENDOR, amd_display_init, amd_hotplug};
use intel::{IntelState, IntelGen, intel_gen_from_did, INTEL_VENDOR,
            intel_display_init, intel_hotplug};

// ════════════════════════════════════════════════════════════════════════════
// § PCI scan (miniaturowe — nie wymaga osobnego pci.rs)
// ════════════════════════════════════════════════════════════════════════════

const PCI_ADDR_P: u16 = 0xCF8;
const PCI_DATA_P: u16 = 0xCFC;

unsafe fn pci_r32(bus:u8,dev:u8,fun:u8,off:u8)->u32{
    let a=0x8000_0000u32|(bus as u32)<<16|(dev as u32)<<11|(fun as u32)<<8|(off as u32&0xFC);
    core::arch::asm!("out dx,eax",in("dx")PCI_ADDR_P,in("eax")a,options(nostack));
    let v:u32;
    core::arch::asm!("in eax,dx",out("eax")v,in("dx")PCI_DATA_P,options(nostack));
    v
}
unsafe fn pci_r8(b:u8,d:u8,f:u8,o:u8)->u8{(pci_r32(b,d,f,o&!3)>>((o&3)*8))as u8}
unsafe fn pci_w32(b:u8,d:u8,f:u8,o:u8,v:u32){
    let a=0x8000_0000u32|(b as u32)<<16|(d as u32)<<11|(f as u32)<<8|(o as u32&0xFC);
    core::arch::asm!("out dx,eax",in("dx")PCI_ADDR_P,in("eax")a,options(nostack));
    core::arch::asm!("out dx,eax",in("dx")PCI_DATA_P,in("eax")v,options(nostack));
}
unsafe fn pci_r16(b:u8,d:u8,f:u8,o:u8)->u16{
    (pci_r32(b,d,f,o&!3)>>((o&2)*8))as u16
}

#[derive(Copy,Clone)]
struct GpuPci { bus:u8, dev:u8, fun:u8, vendor:u16, device:u16 }

impl GpuPci {
    unsafe fn bar64(&self, n: usize) -> Option<u64> {
        let off = (0x10 + n*4) as u8;
        let lo  = pci_r32(self.bus, self.dev, self.fun, off);
        if lo & 1 != 0 { return None; }
        let a = if (lo>>1)&3==2 {
            let hi = pci_r32(self.bus, self.dev, self.fun, off+4);
            ((hi as u64)<<32)|(lo as u64 & !0xF)
        } else { lo as u64 & !0xF };
        if a==0 { None } else { Some(a) }
    }
    unsafe fn enable(&self) {
        let c = pci_r16(self.bus,self.dev,self.fun,4);
        pci_w32(self.bus,self.dev,self.fun,4,c as u32|0x06);
    }
}

enum GpuKind {
    Amd(GpuPci, DcnGen),
    Intel(GpuPci, IntelGen),
}

unsafe fn find_gpu() -> Option<GpuKind> {
    for bus in 0u8..=255 {
        for d in 0u8..32 {
            let fmax = if pci_r8(bus,d,0,0x0E)&0x80!=0 {8} else {1};
            for f in 0..fmax {
                let id = pci_r32(bus,d,f,0);
                if id==0xFFFF_FFFF || id as u16==0xFFFF { continue; }
                let vendor = id as u16;
                let device = (id>>16) as u16;
                let cls    = (pci_r32(bus,d,f,8)>>24) as u8;
                if cls != 0x03 { continue; } // Display Controller only

                if vendor == AMD_VENDOR {
                    if let Some(gen) = dcn_gen_from_did(device) {
                        return Some(GpuKind::Amd(GpuPci{bus,dev:d,fun:f,vendor,device}, gen));
                    }
                }
                if vendor == INTEL_VENDOR {
                    if let Some(gen) = intel_gen_from_did(device) {
                        return Some(GpuKind::Intel(GpuPci{bus,dev:d,fun:f,vendor,device}, gen));
                    }
                }
            }
        }
    }
    None
}

// ════════════════════════════════════════════════════════════════════════════
// § Stan globalny
// ════════════════════════════════════════════════════════════════════════════

enum ActiveGpu {
    Amd(AmdState),
    Intel(IntelState),
}

static mut GPU: Option<ActiveGpu> = None;
static DISP_READY: AtomicBool = AtomicBool::new(false);

// ════════════════════════════════════════════════════════════════════════════
// § Test pattern — 8 kolorowych pasków (sprawdza czy FB działa)
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn display_test_pattern() {
    use fb::{FB_W, FB_H, fb_pixel};
    for y in 0..FB_H {
        for x in 0..FB_W {
            let c = match (x * 8) / FB_W {
                0 => 0x00_000000u32,
                1 => 0x00_FF0000,
                2 => 0x00_FF7F00,
                3 => 0x00_FFFF00,
                4 => 0x00_00FF00,
                5 => 0x00_00FFFF,
                6 => 0x00_0000FF,
                _ => 0x00_FFFFFF,
            };
            fb_pixel(x, y, c);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Publiczne API
// ════════════════════════════════════════════════════════════════════════════

/// Inicjalizuj display — wywołaj z kernel_main
pub unsafe fn display_init() -> bool {
    // 1. Alokuj framebuffer (wymagane przed inicjalizacją GPU)
    if !fb_alloc() {
        serial_print("[DISP] FB alloc failed\n");
        return false;
    }

    // 2. Wykryj GPU
    let kind = match find_gpu() {
        Some(k) => k,
        None => {
            serial_print("[DISP] no supported GPU found\n");
            return false;
        }
    };

    // 3. Zainicjalizuj właściwy driver
    let state = match kind {
        GpuKind::Amd(pci, gen) => {
            serial_print("[DISP] AMD GPU device=");
            serial_hex(pci.device as u64); serial_print("\n");
            pci.enable();
            let mmio = match pci.bar64(0) {
                Some(b) => b,
                None => { serial_print("[DISP] AMD: no BAR0\n"); return false; }
            };
            match amd_display_init(mmio, gen) {
                Some(s) => ActiveGpu::Amd(s),
                None    => { serial_print("[DISP] AMD init failed\n"); return false; }
            }
        }
        GpuKind::Intel(pci, gen) => {
            serial_print("[DISP] Intel GPU device=");
            serial_hex(pci.device as u64); serial_print("\n");
            pci.enable();
            let mmio = match pci.bar64(0) {
                Some(b) => b,
                None => { serial_print("[DISP] Intel: no BAR0\n"); return false; }
            };
            match intel_display_init(mmio, gen) {
                Some(s) => ActiveGpu::Intel(s),
                None    => { serial_print("[DISP] Intel init failed\n"); return false; }
            }
        }
    };

    GPU = Some(state);
    DISP_READY.store(true, Ordering::Release);

    serial_print("[DISP] OK — 1920x1080@60 XRGB8888\n");

    // Test pattern żeby od razu potwierdzić że FB podłączony
    display_test_pattern();

    true
}

/// Czy display jest aktywny?
#[inline]
pub fn display_ok() -> bool { DISP_READY.load(Ordering::Relaxed) }

/// Hotplug check — wywołaj co kilka sekund z wątku kernelowego
pub unsafe fn display_hotplug() {
    match &mut GPU {
        Some(ActiveGpu::Amd(s))   => amd_hotplug(s),
        Some(ActiveGpu::Intel(s)) => intel_hotplug(s),
        None => {}
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Bridge do debug.rs — zastąp VGA 0xB8000 gdy FB aktywny
// ════════════════════════════════════════════════════════════════════════════

/// Wywołaj zamiast VGA putc gdy display jest aktywny.
/// W debug.rs zmodyfikuj putc() tak żeby sprawdzał display_ok():
///
///   pub unsafe fn putc(c: char) {
///       if crate::display::display_ok() {
///           crate::display::putc(c); return;
///       }
///       // ... stary kod VGA 0xB8000 ...
///   }
#[inline]
pub unsafe fn putc(c: char) { fb_putc(c); }

/// Zmień kolor tekstu (fg/bg jako XRGB u32)
pub unsafe fn set_color(fg: u32, bg: u32) { fb_set_color(fg, bg); }

/// Wyczyść ekran
pub unsafe fn cls() { fb_cls(); }

/// Bezpośredni dostęp do FB (dla GUI / userspace blitter)
pub unsafe fn framebuffer() -> Option<*mut u32> { fb::fb_ptr() }
pub fn fb_width()  -> usize { fb::FB_W }
pub fn fb_height() -> usize { fb::FB_H }
pub fn fb_pitch()  -> usize { fb::FB_PITCH }
