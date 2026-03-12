// CosinusOS — usb/hid.rs
// HID: pełna enumeracja GET_DESCRIPTOR, klawiatura, mysz, audio controls
// Tabela urządzeń USB (do 16 jednocześnie)

use crate::debug::{serial_print, serial_hex, num_str};

// ════════════════════════════════════════════════════════════════════════════
// § Stałe USB — klasy, podklasy, protokoły
// ════════════════════════════════════════════════════════════════════════════

pub const USB_CLASS_HID:    u8 = 0x03;
pub const USB_CLASS_AUDIO:  u8 = 0x01;
pub const USB_SUB_BOOT:     u8 = 0x01;
pub const USB_PROTO_KB:     u8 = 0x01;
pub const USB_PROTO_MOUSE:  u8 = 0x02;

// Typy deskryptorów
pub const DESC_DEVICE:      u8 = 0x01;
pub const DESC_CONFIG:      u8 = 0x02;
pub const DESC_STRING:      u8 = 0x03;
pub const DESC_INTERFACE:   u8 = 0x04;
pub const DESC_ENDPOINT:    u8 = 0x05;
pub const DESC_HID:         u8 = 0x21;
pub const DESC_REPORT:      u8 = 0x22;

// bRequest
pub const REQ_GET_DESCRIPTOR:   u8 = 0x06;
pub const REQ_SET_CONFIGURATION:u8 = 0x09;
pub const REQ_SET_INTERFACE:    u8 = 0x0B;
pub const REQ_HID_SET_PROTO:    u8 = 0x0B; // class request
pub const REQ_HID_SET_IDLE:     u8 = 0x0A;
pub const REQ_HID_GET_REPORT:   u8 = 0x01;

// ════════════════════════════════════════════════════════════════════════════
// § Typy urządzeń
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum DevClass {
    Unknown,
    Keyboard,
    Mouse,
    AudioControl,   // HID audio volume/mute
    Hub,
}

#[derive(Copy, Clone)]
pub struct UsbDevice {
    pub slot:     u8,       // XHCI slot lub EHCI port index
    pub class:    DevClass,
    pub speed:    u8,       // 1=LS 2=FS 3=HS 4=SS
    pub addr:     u8,       // USB address (po SET_ADDRESS)
    pub ep_in:    u8,       // endpoint IN numer
    pub ep_in_mps:u16,      // max packet size endpoint IN
    pub active:   bool,
    // Dane identyfikacyjne z GET_DESCRIPTOR
    pub vid:      u16,
    pub pid:      u16,
    pub subclass: u8,
    pub protocol: u8,
}

impl UsbDevice {
    pub const fn empty() -> Self {
        Self {
            slot:0, class:DevClass::Unknown, speed:0, addr:0,
            ep_in:0, ep_in_mps:8, active:false,
            vid:0, pid:0, subclass:0, protocol:0,
        }
    }
}

pub const MAX_USB_DEVICES: usize = 16;
pub static mut USB_DEVICES: [UsbDevice; MAX_USB_DEVICES] =
    [UsbDevice::empty(); MAX_USB_DEVICES];
static mut USB_NDEV: usize = 0;

pub fn hid_device_count() -> usize { unsafe { USB_NDEV } }

pub unsafe fn dev_alloc(slot: u8) -> Option<usize> {
    for i in 0..MAX_USB_DEVICES {
        if !USB_DEVICES[i].active {
            USB_DEVICES[i] = UsbDevice::empty();
            USB_DEVICES[i].slot = slot;
            USB_DEVICES[i].active = true;
            USB_NDEV += 1;
            return Some(i);
        }
    }
    None
}

pub unsafe fn dev_free(slot: u8) {
    for i in 0..MAX_USB_DEVICES {
        if USB_DEVICES[i].active && USB_DEVICES[i].slot == slot {
            USB_DEVICES[i].active = false;
            if USB_NDEV > 0 { USB_NDEV -= 1; }
            serial_print("[HID] device disconnected slot=");
            serial_hex(slot as u64); serial_print("\n");
            return;
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Parsowanie deskryptora konfiguracji (raw bytes z GET_DESCRIPTOR)
// ════════════════════════════════════════════════════════════════════════════

/// Wynik parsowania: klasa, ep IN, mps, subclass, protocol
#[derive(Default)]
pub struct ParsedConfig {
    pub class:    u8,
    pub subclass: u8,
    pub protocol: u8,
    pub ep_in:    u8,
    pub ep_mps:   u16,
    pub ep_interval: u8,
    pub has_audio: bool,
}

pub fn parse_config_descriptor(buf: &[u8]) -> ParsedConfig {
    let mut r = ParsedConfig::default();
    let mut i = 0usize;
    while i + 2 <= buf.len() {
        let len  = buf[i] as usize;
        let typ  = buf[i+1];
        if len == 0 || i + len > buf.len() { break; }
        let d = &buf[i..i+len];

        match typ {
            t if t == DESC_INTERFACE && len >= 9 => {
                // bInterfaceClass=d[5] bSubClass=d[6] bProtocol=d[7]
                let ic  = d[5];
                let isc = d[6];
                let ipr = d[7];
                if ic == USB_CLASS_HID {
                    r.class    = ic;
                    r.subclass = isc;
                    r.protocol = ipr;
                }
                if ic == USB_CLASS_AUDIO { r.has_audio = true; }
            }
            t if t == DESC_ENDPOINT && len >= 7 => {
                // bEndpointAddress=d[2] bmAttributes=d[3] wMaxPacketSize=d[4..5]
                let addr  = d[2];
                let attr  = d[3];
                let mps   = (d[4] as u16) | ((d[5] as u16) << 8);
                let interval = d[6];
                // Szukamy Interrupt IN endpoint (attr & 0x03 == 0x03, addr bit7=1)
                if (attr & 0x03) == 0x03 && (addr & 0x80) != 0 && r.ep_in == 0 {
                    r.ep_in       = addr & 0x0F;
                    r.ep_mps      = mps;
                    r.ep_interval = interval;
                }
            }
            _ => {}
        }
        i += len;
    }
    r
}

/// Określ klasę urządzenia z ParsedConfig
pub fn classify(cfg: &ParsedConfig, speed: u8) -> DevClass {
    if cfg.has_audio { return DevClass::AudioControl; }
    match (cfg.class, cfg.subclass, cfg.protocol) {
        (USB_CLASS_HID, USB_SUB_BOOT, USB_PROTO_KB)    => DevClass::Keyboard,
        (USB_CLASS_HID, USB_SUB_BOOT, USB_PROTO_MOUSE) => DevClass::Mouse,
        (USB_CLASS_HID, _, USB_PROTO_KB)               => DevClass::Keyboard,
        (USB_CLASS_HID, _, USB_PROTO_MOUSE)            => DevClass::Mouse,
        // Heurystyka fallback gdy GET_DESCRIPTOR nie zadziałał
        (0, 0, 0) => match speed { 1|2 => DevClass::Keyboard, _ => DevClass::Mouse },
        _ => DevClass::Unknown,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § Klawiatura — Boot Protocol (8-bajtowy raport)
// ════════════════════════════════════════════════════════════════════════════

// HID Usage ID → ASCII (Normal + Shift)
// Rozmiar 104 (Usage 0x00 .. 0x67)
static HID_NORM: [u8; 104] = [
/*00*/ 0,0,0,0,
/*04*/ b'a',b'b',b'c',b'd',b'e',b'f',b'g',b'h',b'i',b'j',b'k',b'l',b'm',b'n',b'o',b'p',
/*14*/ b'q',b'r',b's',b't',b'u',b'v',b'w',b'x',b'y',b'z',
/*1E*/ b'1',b'2',b'3',b'4',b'5',b'6',b'7',b'8',b'9',b'0',
/*28*/ b'\n',b'\x1b',b'\x08',b'\t',b' ',
/*2D*/ b'-',b'=',b'[',b']',b'\\',0,b';',b'\'',b'`',b'\x2C',b'.',b'/',
/*39*/ 0,
/*3A*/ 0,0,0,0,0,0,0,0,0,0,0,0,
/*46*/ 0,0,0,0,
/*4A*/ 0,0,0,0,
/*4E*/ 0,0,
/*50*/ 0,0,0,0,
/*54*/ 0,0,0,0,0,0,0,0,0,
/*5D*/ 0,0,0,0,0,0,0,
/*64*/ 0,0,0,0,
];

static HID_SHFT: [u8; 104] = [
    0,0,0,0,                                                    // 0x00-0x03
    b'A',b'B',b'C',b'D',b'E',b'F',b'G',b'H',b'I',b'J',b'K',b'L', // 0x04-0x0F
    b'M',b'N',b'O',b'P',b'Q',b'R',b'S',b'T',b'U',b'V',b'W',b'X',b'Y',b'Z', // 0x10-0x1D
    b'!',b'@',b'#',b'$',b'%',b'^',b'&',b'*',b'(',b')',        // 0x1E-0x27
    b'\n',b'\x1b',b'\x08',b'\t',b' ',                          // 0x28-0x2C
    b'_',b'+',b'{',b'}',b'|',0,b':',b'"',b'~',b'<',b'>',b'?', // 0x2D-0x38
    0,                                                           // 0x39 CapsLock
    0,0,0,0,0,0,0,0,0,0,0,0,                                    // 0x3A-0x45 F1-F12
    0,0,0,0,                                                     // 0x46-0x49
    0,0,0,0,                                                     // 0x4A-0x4D
    0,0,                                                         // 0x4E-0x4F
    0,0,0,0,                                                     // 0x50-0x53
    0,0,0,0,0,0,0,0,0,                                          // 0x54-0x5C
    0,0,0,0,0,0,0,                                              // 0x5D-0x63
    0,0,0,                                                      // 0x64-0x66
    0,                                                          // 0x67 pad
];

static mut PREV_KB: [u8; 8] = [0u8; 8];

pub unsafe fn hid_kb(rep: &[u8; 8]) {
    if *rep == PREV_KB { return; }
    let mods  = rep[0];
    let shift = mods & 0x22 != 0; // LShift(bit1) | RShift(bit5)
    let ctrl  = mods & 0x11 != 0; // LCtrl | RCtrl
    let alt   = mods & 0x44 != 0; // LAlt | RAlt

    for &k in &rep[2..8] {
        if k < 4 { continue; }
        if PREV_KB[2..8].contains(&k) { continue; }
        let idx = k as usize;
        if idx >= HID_NORM.len() { continue; }

        // Ctrl+C / Ctrl+D obsługa
        if ctrl {
            match k {
                0x06 => { crate::perm::kb_push_pub('\x03'); continue; } // Ctrl+C = ETX
                0x07 => { crate::perm::kb_push_pub('\x04'); continue; } // Ctrl+D = EOT
                0x08 => { crate::perm::kb_push_pub('\x08'); continue; } // Ctrl+H = BS
                _ => {}
            }
        }

        let c = if shift { HID_SHFT[idx] } else { HID_NORM[idx] };
        if c != 0 { crate::perm::kb_push_pub(c as char); }
    }
    PREV_KB = *rep;
}

// ════════════════════════════════════════════════════════════════════════════
// § Mysz — Boot Protocol (3–4 bajtowy raport)
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone, Default)]
pub struct Mouse {
    pub buttons: u8,
    pub x:       i16,  // akumulowana pozycja
    pub y:       i16,
    pub dx:      i8,   // ostatnia delta
    pub dy:      i8,
    pub scroll:  i8,
    pub changed: bool,
}

static mut MOUSE: Mouse = Mouse {
    buttons:0, x:0, y:0, dx:0, dy:0, scroll:0, changed:false };

pub unsafe fn hid_mouse(rep: &[u8]) {
    if rep.len() < 3 { return; }
    MOUSE.buttons = rep[0];
    MOUSE.dx      = rep[1] as i8;
    MOUSE.dy      = rep[2] as i8;
    MOUSE.scroll  = if rep.len() > 3 { rep[3] as i8 } else { 0 };
    MOUSE.x       = MOUSE.x.saturating_add(MOUSE.dx as i16);
    MOUSE.y       = MOUSE.y.saturating_add(MOUSE.dy as i16);
    MOUSE.changed = true;
}

pub unsafe fn mouse_get()         -> Mouse { MOUSE }
pub unsafe fn mouse_reset_delta() { MOUSE.dx=0; MOUSE.dy=0; MOUSE.scroll=0; MOUSE.changed=false; }

// ════════════════════════════════════════════════════════════════════════════
// § Audio HID — volume / mute controls
// ════════════════════════════════════════════════════════════════════════════

// Usage IDs dla Consumer Control (Usage Page 0x0C)
const AUDIO_MUTE:        u16 = 0x00E2;
const AUDIO_VOL_UP:      u16 = 0x00E9;
const AUDIO_VOL_DOWN:    u16 = 0x00EA;
const AUDIO_PLAY_PAUSE:  u16 = 0x00CD;
const AUDIO_NEXT_TRACK:  u16 = 0x00B5;
const AUDIO_PREV_TRACK:  u16 = 0x00B6;
const AUDIO_STOP:        u16 = 0x00B7;

#[derive(Copy, Clone, Default)]
pub struct AudioState {
    pub volume:    i8,   // -100..100 (delta)
    pub muted:     bool,
    pub play_pause:bool,
    pub next:      bool,
    pub prev:      bool,
    pub stop:      bool,
    pub changed:   bool,
}

static mut AUDIO: AudioState = AudioState {
    volume:0, muted:false, play_pause:false,
    next:false, prev:false, stop:false, changed:false,
};

/// Parsuj raport Consumer Control (2-bajtowy Usage ID)
pub unsafe fn hid_audio(rep: &[u8]) {
    if rep.len() < 2 { return; }
    let usage = (rep[0] as u16) | ((rep[1] as u16) << 8);
    AUDIO.changed = true;
    match usage {
        u if u == AUDIO_MUTE       => { AUDIO.muted     = !AUDIO.muted; }
        u if u == AUDIO_VOL_UP     => { AUDIO.volume     = AUDIO.volume.saturating_add(5); }
        u if u == AUDIO_VOL_DOWN   => { AUDIO.volume     = AUDIO.volume.saturating_sub(5); }
        u if u == AUDIO_PLAY_PAUSE => { AUDIO.play_pause = true; }
        u if u == AUDIO_NEXT_TRACK => { AUDIO.next       = true; }
        u if u == AUDIO_PREV_TRACK => { AUDIO.prev       = true; }
        u if u == AUDIO_STOP       => { AUDIO.stop       = true; }
        _ => { AUDIO.changed = false; }
    }
}

pub unsafe fn audio_get()         -> AudioState { AUDIO }
pub unsafe fn audio_reset()       { AUDIO.play_pause=false; AUDIO.next=false; AUDIO.prev=false; AUDIO.stop=false; AUDIO.changed=false; }

// ════════════════════════════════════════════════════════════════════════════
// § Dispatcher — przetworz raport na podstawie klasy urządzenia
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn hid_dispatch(dev: &UsbDevice, buf: *const u8, len: usize) {
    let data = core::slice::from_raw_parts(buf, len);
    match dev.class {
        DevClass::Keyboard => {
            if len >= 8 { hid_kb(&*(buf as *const [u8;8])); }
        }
        DevClass::Mouse => {
            hid_mouse(data);
        }
        DevClass::AudioControl => {
            hid_audio(data);
        }
        _ => {}
    }
}

/// Loguj informacje o urządzeniu
pub unsafe fn hid_log_device(dev: &UsbDevice) {
    serial_print("[HID] slot="); serial_hex(dev.slot as u64);
    serial_print(" class=");
    serial_print(match dev.class {
        DevClass::Keyboard     => "KB",
        DevClass::Mouse        => "MOUSE",
        DevClass::AudioControl => "AUDIO",
        DevClass::Hub          => "HUB",
        DevClass::Unknown      => "?",
    });
    serial_print(" vid="); serial_hex(dev.vid as u64);
    serial_print(" pid="); serial_hex(dev.pid as u64);
    serial_print(" ep_in="); serial_hex(dev.ep_in as u64);
    serial_print(" mps="); { let mut b=[0u8;24]; serial_print(num_str(dev.ep_in_mps as usize, &mut b)); }
    serial_print("\n");
}