// CosinusOS — input.rs
// PS/2 Input: klawiatura (Scan Code Set 2) + mysz (port 2)
// Integracja:
//   pub mod input;  w lib.rs
//   W perm.rs:
//     handle_kb()  → crate::input::kbd_irq()
//     handle_mouse() → crate::input::mouse_irq()
//   W lib.rs kernel_main po sched_init+pit:
//     input::init_ps2();

use core::arch::asm;
use crate::debug::{inb, outb, serial_print, serial_hex, hex_str};

// ════════════════════════════════════════════════════════════════════════════
// § Scan Code Set 2 → ASCII
// Rozmiar 132 (Usage 0x00 .. 0x83)
// ════════════════════════════════════════════════════════════════════════════

// Set 2 make codes → ASCII (normal)
// Index = scan code bajt (Set 2 single-byte)
static SC2_NORM: [u8; 132] = [
/*00*/ 0,
/*01*/ b'\x1b', // F9  — traktujemy jako ESC dla uproszczenia
/*02*/ 0,
/*03*/ b'\x1b', // F5
/*04*/ b'\x1b', // F3
/*05*/ b'\x1b', // F1
/*06*/ b'\x1b', // F2
/*07*/ b'\x1b', // F12
/*08*/ 0,
/*09*/ b'\x1b', // F10
/*0A*/ b'\x1b', // F8
/*0B*/ b'\x1b', // F6
/*0C*/ b'\x1b', // F4
/*0D*/ b'\t',   // Tab
/*0E*/ b'`',
/*0F*/ 0,
/*10*/ 0,
/*11*/ 0,       // LAlt
/*12*/ 0,       // LShift
/*13*/ 0,
/*14*/ 0,       // LCtrl
/*15*/ b'q',
/*16*/ b'1',
/*17*/ 0,
/*18*/ 0,
/*19*/ 0,
/*1A*/ b'z',
/*1B*/ b's',
/*1C*/ b'a',
/*1D*/ b'w',
/*1E*/ b'2',
/*1F*/ 0,
/*20*/ 0,
/*21*/ b'c',
/*22*/ b'x',
/*23*/ b'd',
/*24*/ b'e',
/*25*/ b'4',
/*26*/ b'3',
/*27*/ 0,
/*28*/ 0,
/*29*/ b' ',
/*2A*/ b'v',
/*2B*/ b'f',
/*2C*/ b't',
/*2D*/ b'r',
/*2E*/ b'5',
/*2F*/ 0,
/*30*/ 0,
/*31*/ b'n',
/*32*/ b'b',
/*33*/ b'h',
/*34*/ b'g',
/*35*/ b'y',
/*36*/ b'6',
/*37*/ 0,
/*38*/ 0,
/*39*/ 0,
/*3A*/ b'm',
/*3B*/ b'j',
/*3C*/ b'u',
/*3D*/ b'7',
/*3E*/ b'8',
/*3F*/ 0,
/*40*/ 0,
/*41*/ b',',
/*42*/ b'k',
/*43*/ b'i',
/*44*/ b'o',
/*45*/ b'0',
/*46*/ b'9',
/*47*/ 0,
/*48*/ 0,
/*49*/ b'.',
/*4A*/ b'/',
/*4B*/ b'l',
/*4C*/ b';',
/*4D*/ b'p',
/*4E*/ b'-',
/*4F*/ 0,
/*50*/ 0,
/*51*/ 0,
/*52*/ b'\'',
/*53*/ 0,
/*54*/ b'[',
/*55*/ b'=',
/*56*/ 0,
/*57*/ 0,
/*58*/ 0,       // CapsLock
/*59*/ 0,       // RShift
/*5A*/ b'\n',   // Enter
/*5B*/ b']',
/*5C*/ 0,
/*5D*/ b'\\',
/*5E*/ 0,
/*5F*/ 0,
/*60*/ 0,
/*61*/ 0,
/*62*/ 0,
/*63*/ 0,
/*64*/ 0,
/*65*/ 0,
/*66*/ b'\x08', // Backspace
/*67*/ 0,
/*68*/ 0,
/*69*/ b'1',    // Numpad 1
/*6A*/ 0,
/*6B*/ b'4',    // Numpad 4
/*6C*/ b'7',    // Numpad 7
/*6D*/ 0,
/*6E*/ 0,
/*6F*/ 0,
/*70*/ b'0',    // Numpad 0
/*71*/ b'.',    // Numpad .
/*72*/ b'2',    // Numpad 2
/*73*/ b'5',    // Numpad 5
/*74*/ b'6',    // Numpad 6
/*75*/ b'8',    // Numpad 8
/*76*/ b'\x1b', // Escape
/*77*/ 0,       // NumLock
/*78*/ b'\x1b', // F11
/*79*/ b'+',    // Numpad +
/*7A*/ b'3',    // Numpad 3
/*7B*/ b'-',    // Numpad -
/*7C*/ b'*',    // Numpad *
/*7D*/ b'9',    // Numpad 9
/*7E*/ 0,       // ScrollLock
/*7F*/ 0,
/*80*/ 0,
/*81*/ 0,
/*82*/ 0,
/*83*/ b'\x1b', // F7
];

// Set 2 make codes → ASCII (Shift)
static SC2_SHFT: [u8; 132] = [
/*00*/ 0,
/*01*/ 0,
/*02*/ 0,
/*03*/ 0,
/*04*/ 0,
/*05*/ 0,
/*06*/ 0,
/*07*/ 0,
/*08*/ 0,
/*09*/ 0,
/*0A*/ 0,
/*0B*/ 0,
/*0C*/ 0,
/*0D*/ b'\t',
/*0E*/ b'~',
/*0F*/ 0,
/*10*/ 0,
/*11*/ 0,       // LAlt
/*12*/ 0,       // LShift
/*13*/ 0,
/*14*/ 0,       // LCtrl
/*15*/ b'Q',
/*16*/ b'!',
/*17*/ 0,
/*18*/ 0,
/*19*/ 0,
/*1A*/ b'Z',
/*1B*/ b'S',
/*1C*/ b'A',
/*1D*/ b'W',
/*1E*/ b'@',
/*1F*/ 0,
/*20*/ 0,
/*21*/ b'C',
/*22*/ b'X',
/*23*/ b'D',
/*24*/ b'E',
/*25*/ b'$',
/*26*/ b'#',
/*27*/ 0,
/*28*/ 0,
/*29*/ b' ',
/*2A*/ b'V',
/*2B*/ b'F',
/*2C*/ b'T',
/*2D*/ b'R',
/*2E*/ b'%',
/*2F*/ 0,
/*30*/ 0,
/*31*/ b'N',
/*32*/ b'B',
/*33*/ b'H',
/*34*/ b'G',
/*35*/ b'Y',
/*36*/ b'^',
/*37*/ 0,
/*38*/ 0,
/*39*/ 0,
/*3A*/ b'M',
/*3B*/ b'J',
/*3C*/ b'U',
/*3D*/ b'&',
/*3E*/ b'*',
/*3F*/ 0,
/*40*/ 0,
/*41*/ b'<',
/*42*/ b'K',
/*43*/ b'I',
/*44*/ b'O',
/*45*/ b')',
/*46*/ b'(',
/*47*/ 0,
/*48*/ 0,
/*49*/ b'>',
/*4A*/ b'?',
/*4B*/ b'L',
/*4C*/ b':',
/*4D*/ b'P',
/*4E*/ b'_',
/*4F*/ 0,
/*50*/ 0,
/*51*/ 0,
/*52*/ b'"',
/*53*/ 0,
/*54*/ b'{',
/*55*/ b'+',
/*56*/ 0,
/*57*/ 0,
/*58*/ 0,
/*59*/ 0,       // RShift
/*5A*/ b'\n',
/*5B*/ b'}',
/*5C*/ 0,
/*5D*/ b'|',
/*5E*/ 0,
/*5F*/ 0,
/*60*/ 0,
/*61*/ 0,
/*62*/ 0,
/*63*/ 0,
/*64*/ 0,
/*65*/ 0,
/*66*/ b'\x08',
/*67*/ 0,
/*68*/ 0,
/*69*/ 0,
/*6A*/ 0,
/*6B*/ 0,
/*6C*/ 0,
/*6D*/ 0,
/*6E*/ 0,
/*6F*/ 0,
/*70*/ 0,
/*71*/ 0,
/*72*/ 0,
/*73*/ 0,
/*74*/ 0,
/*75*/ 0,
/*76*/ b'\x1b',
/*77*/ 0,
/*78*/ 0,
/*79*/ 0,
/*7A*/ 0,
/*7B*/ 0,
/*7C*/ 0,
/*7D*/ 0,
/*7E*/ 0,
/*7F*/ 0,
/*80*/ 0,
/*81*/ 0,
/*82*/ 0,
/*83*/ 0,
];

// ════════════════════════════════════════════════════════════════════════════
// § Keyboard state machine
// Set 2: 0xF0 = break prefix, 0xE0 = extended prefix
// ════════════════════════════════════════════════════════════════════════════

#[derive(PartialEq)]
enum KbdState {
    Idle,
    Extended,   // po 0xE0
    Break,      // po 0xF0
    ExtBreak,   // po 0xE0 0xF0
}

static mut KBD_STATE: KbdState  = KbdState::Idle;
static mut KBD_SHIFT: bool      = false;
static mut KBD_CTRL:  bool      = false;
static mut KBD_ALT:   bool      = false;

// ════════════════════════════════════════════════════════════════════════════
// § Keyboard ring buffer
// ════════════════════════════════════════════════════════════════════════════

const KB_BUF: usize = 64;
static mut KB_BUF_DATA: [char; KB_BUF] = ['\0'; KB_BUF];
static mut KB_HEAD: usize = 0;
static mut KB_TAIL: usize = 0;

unsafe fn kb_push(c: char) {
    let next = (KB_HEAD + 1) % KB_BUF;
    if next != KB_TAIL {
        KB_BUF_DATA[KB_HEAD] = c;
        KB_HEAD = next;
    }
}

pub unsafe fn input_poll() -> Option<char> {
    if KB_HEAD == KB_TAIL { return None; }
    let c = KB_BUF_DATA[KB_TAIL];
    KB_TAIL = (KB_TAIL + 1) % KB_BUF;
    Some(c)
}

/// Wstaw znak bezpośrednio (używane przez USB HID)
pub unsafe fn input_push(c: char) {
    kb_push(c);
}

// ════════════════════════════════════════════════════════════════════════════
// § Mouse ring buffer
// ════════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone, Default)]
pub struct MouseEvent {
    pub buttons: u8,
    pub dx:      i8,
    pub dy:      i8,
    pub scroll:  i8,
}

const MOUSE_BUF: usize = 16;
static mut MOUSE_BUF_DATA: [MouseEvent; MOUSE_BUF] = [MouseEvent { buttons:0, dx:0, dy:0, scroll:0 }; MOUSE_BUF];
static mut MOUSE_HEAD: usize = 0;
static mut MOUSE_TAIL: usize = 0;

unsafe fn mouse_push(e: MouseEvent) {
    let next = (MOUSE_HEAD + 1) % MOUSE_BUF;
    if next != MOUSE_TAIL {
        MOUSE_BUF_DATA[MOUSE_HEAD] = e;
        MOUSE_HEAD = next;
    }
}

pub unsafe fn mouse_poll() -> Option<MouseEvent> {
    if MOUSE_HEAD == MOUSE_TAIL { return None; }
    let e = MOUSE_BUF_DATA[MOUSE_TAIL];
    MOUSE_TAIL = (MOUSE_TAIL + 1) % MOUSE_BUF;
    Some(e)
}

// Absolutna pozycja myszy (akumulowana)
static mut MOUSE_X: i16 = 0;
static mut MOUSE_Y: i16 = 0;
pub unsafe fn mouse_pos() -> (i16, i16) { (MOUSE_X, MOUSE_Y) }

// ════════════════════════════════════════════════════════════════════════════
// § Mouse state machine (3-bajtowy pakiet PS/2)
// ════════════════════════════════════════════════════════════════════════════

static mut MOUSE_PKT:  [u8; 3] = [0u8; 3];
static mut MOUSE_BYTE: u8      = 0; // który bajt pakietu (0,1,2)

unsafe fn mouse_process_packet() {
    let btn = MOUSE_PKT[0];
    // Bit3 musi być 1 — jeśli nie, pakiet jest nieprawidłowy
    if btn & 0x08 == 0 { MOUSE_BYTE = 0; return; }

    let dx = MOUSE_PKT[1] as i8;
    let dy = MOUSE_PKT[2] as i8;

    // Overflow bits (bit6=Y overflow, bit7=X overflow)
    if btn & 0xC0 != 0 { MOUSE_BYTE = 0; return; }

    // Sign bits: bit4=X sign, bit5=Y sign (PS/2 9-bit two's complement)
    // Już zakodowane w i8 przez rzutowanie, ale bit4/5 to bit znaku
    // więc rzutowanie as i8 jest OK gdy overflow=0
    let dx_sign: i16 = if btn & 0x10 != 0 { dx as i16 | -256 } else { dx as i16 };
    let dy_sign: i16 = if btn & 0x20 != 0 { dy as i16 | -256 } else { dy as i16 };

    MOUSE_X = MOUSE_X.saturating_add(dx_sign);
    MOUSE_Y = MOUSE_Y.saturating_sub(dy_sign); // Y odwrócone (PS/2: góra=+)

    let evt = MouseEvent {
        buttons: btn & 0x07,
        dx:      dx_sign as i8,
        dy:      -(dy_sign as i8),
        scroll:  0,
    };
    mouse_push(evt);
}

// ════════════════════════════════════════════════════════════════════════════
// § IRQ handlers (wywoływane z perm.rs)
// ════════════════════════════════════════════════════════════════════════════

/// Wywołaj z handle_kb w perm.rs
pub unsafe fn kbd_irq() {
    let sc = inb(0x60);
    serial_print("[KB] sc="); serial_hex(sc as u64); serial_print("\n");

    match sc {
        // Prefiksy
        0xE0 => {
            KBD_STATE = KbdState::Extended;
            return;
        }
        0xF0 => {
            KBD_STATE = match KBD_STATE {
                KbdState::Extended => KbdState::ExtBreak,
                _                  => KbdState::Break,
            };
            return;
        }
        _ => {}
    }

    let is_break = matches!(KBD_STATE, KbdState::Break | KbdState::ExtBreak);
    let extended  = matches!(KBD_STATE, KbdState::Extended | KbdState::ExtBreak);
    KBD_STATE = KbdState::Idle;

    // Obsługa modyfikatorów
    match (sc, extended) {
        // LShift=0x12, RShift=0x59
        (0x12, false) | (0x59, false) => { KBD_SHIFT = !is_break; return; }
        // LCtrl=0x14, RCtrl=0x14+E0
        (0x14, _)                     => { KBD_CTRL  = !is_break; return; }
        // LAlt=0x11, RAlt=0x11+E0
        (0x11, _)                     => { KBD_ALT   = !is_break; return; }
        // CapsLock=0x58 — toggle przy make
        (0x58, false) if !is_break    => { KBD_SHIFT = !KBD_SHIFT; return; }
        _ => {}
    }

    // Tylko make events generują znaki
    if is_break { return; }

    // Extended keycodes (E0 xx) — strzałki, Insert, Delete, Home, End, PgUp, PgDn
    if extended {
        let c: Option<char> = match sc {
            0x75 => Some('\x1b'), // Up    → ESC sekwencja (uproszczenie)
            0x72 => Some('\x1b'), // Down
            0x6B => Some('\x1b'), // Left
            0x74 => Some('\x1b'), // Right
            0x70 => Some('\x1b'), // Insert
            0x71 => Some('\x7f'), // Delete → DEL
            0x6C => Some('\x1b'), // Home
            0x69 => Some('\x1b'), // End
            0x7D => Some('\x1b'), // PgUp
            0x7A => Some('\x1b'), // PgDn
            0x4A => Some('/'),    // Numpad /
            0x5A => Some('\n'),   // Numpad Enter
            _ => None,
        };
        if let Some(c) = c { kb_push(c); }
        return;
    }

    // Zwykłe klawisze
    let idx = sc as usize;
    if idx >= SC2_NORM.len() { return; }

    // Ctrl+klawisz
    if KBD_CTRL {
        let base = SC2_NORM[idx];
        if base >= b'a' && base <= b'z' {
            kb_push((base - b'a' + 1) as char); // Ctrl+A=0x01 .. Ctrl+Z=0x1A
            return;
        }
        if base >= b'A' && base <= b'Z' {
            kb_push((base - b'A' + 1) as char);
            return;
        }
    }

    let c = if KBD_SHIFT { SC2_SHFT[idx] } else { SC2_NORM[idx] };
    if c != 0 { kb_push(c as char); }
}

/// Wywołaj z handle_mouse w perm.rs (IRQ12)
pub unsafe fn mouse_irq() {
    let byte = inb(0x60);
    match MOUSE_BYTE {
        0 => {
            // Pierwszym bajtem musi być status z bitem 3 ustawionym
            if byte & 0x08 != 0 {
                MOUSE_PKT[0] = byte;
                MOUSE_BYTE = 1;
            }
            // Jeśli bit3=0 — ignoruj i czekaj na resync
        }
        1 => { MOUSE_PKT[1] = byte; MOUSE_BYTE = 2; }
        2 => { MOUSE_PKT[2] = byte; MOUSE_BYTE = 0; mouse_process_packet(); }
        _ => { MOUSE_BYTE = 0; }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// § 8042 PS/2 Controller init
// ════════════════════════════════════════════════════════════════════════════

unsafe fn ps2_wait_write() {
    let mut tries = 0usize;
    while inb(0x64) & 0x02 != 0 {
        tries += 1;
        if tries > 100_000 { return; }
        core::hint::spin_loop();
    }
}

unsafe fn ps2_wait_read() -> bool {
    let mut tries = 0usize;
    while inb(0x64) & 0x01 == 0 {
        tries += 1;
        if tries > 100_000 { return false; }
        core::hint::spin_loop();
    }
    true
}

unsafe fn ps2_send_kbd(cmd: u8) -> bool {
    ps2_wait_write();
    outb(0x60, cmd);
    // Czekaj na ACK (0xFA)
    if !ps2_wait_read() { return false; }
    let r = inb(0x60);
    r == 0xFA
}

unsafe fn ps2_send_mouse(cmd: u8) -> bool {
    // Wyślij do port 2: najpierw 0xD4 do 8042, potem komenda
    ps2_wait_write();
    outb(0x64, 0xD4);
    ps2_wait_write();
    outb(0x60, cmd);
    if !ps2_wait_read() { return false; }
    let r = inb(0x60);
    r == 0xFA
}

pub unsafe fn init_ps2() {
    serial_print("[PS2] init\n");

    let status = inb(0x64);
    serial_print("[PS2] status=");
    { let mut b = [0u8;18]; serial_print(hex_str(status as u64, &mut b)); }
    serial_print("\n");

    if status == 0xFF {
        serial_print("[PS2] brak kontrolera\n");
        return;
    }

    // Opróżnij output buffer
    if status & 0x01 != 0 { let _ = inb(0x60); }

    // Disable obu portów na czas konfiguracji
    ps2_wait_write(); outb(0x64, 0xAD); // disable port 1
    ps2_wait_write(); outb(0x64, 0xA7); // disable port 2

    // Opróżnij buffer po disable
    while inb(0x64) & 0x01 != 0 { let _ = inb(0x60); }

    // Odczytaj Configuration Byte
    ps2_wait_write(); outb(0x64, 0x20);
    let mut cfg = if ps2_wait_read() { inb(0x60) } else { 0x47 };
    serial_print("[PS2] cfg=");
    { let mut b = [0u8;18]; serial_print(hex_str(cfg as u64, &mut b)); }
    serial_print("\n");

    // Włącz IRQ1 (bit0) i IRQ12 (bit1)
    // Zostaw translation (bit6) WŁĄCZONE — Set2→Set1 translation wyłączamy
    // przez komendę do klawiatury, nie przez bit CFG
    // UWAGA: zostawiamy bit6=1 (translation) ale wyślemy do klawiatury
    //        komendę Set Scancode Set 2 żeby dostać Set2 bezpośrednio
    cfg |= 0x03;   // IRQ1 + IRQ12 enable
    cfg &= !0x30;  // wyczyść clock disable bits (bit4=port1 clock, bit5=port2 clock)
    // bit6 (translation) — wyłączamy żeby dostać czysty Set2
    cfg &= !0x40;

    // Zapisz Configuration Byte
    ps2_wait_write(); outb(0x64, 0x60);
    ps2_wait_write(); outb(0x60, cfg);

    // Sprawdź czy jest port 2 (mysz)
    ps2_wait_write(); outb(0x64, 0xA8); // enable port 2 tymczasowo
    ps2_wait_write(); outb(0x64, 0x20); // odczytaj cfg ponownie
    let cfg2 = if ps2_wait_read() { inb(0x60) } else { 0 };
    let has_mouse = cfg2 & 0x20 == 0; // bit5=0 oznacza że port2 jest aktywny
    serial_print(if has_mouse { "[PS2] port2 (mouse) OK\n" } else { "[PS2] brak port2\n" });

    // Disable port 2 z powrotem (włączymy po konfiguracji)
    ps2_wait_write(); outb(0x64, 0xA7);

    // ── Klawiatura: reset + Set Scan Code Set 2 ─────────────────────────────

    // Enable port 1
    ps2_wait_write(); outb(0x64, 0xAE);

    // Reset klawiatury (0xFF)
    serial_print("[PS2] kbd reset\n");
    ps2_wait_write(); outb(0x60, 0xFF);
    // Czekaj na ACK (0xFA) potem BAT (0xAA)
    let mut got_aa = false;
    for _ in 0..3 {
        if !ps2_wait_read() { break; }
        let r = inb(0x60);
        serial_print("[PS2] kbd resp=");
        { let mut b = [0u8;18]; serial_print(hex_str(r as u64, &mut b)); }
        serial_print("\n");
        if r == 0xAA { got_aa = true; break; }
    }

    if got_aa {
        // Ustaw Scan Code Set 2 (komenda 0xF0, parametr 0x02)
        if ps2_send_kbd(0xF0) {
            ps2_wait_write(); outb(0x60, 0x02);
            if ps2_wait_read() {
                let r = inb(0x60);
                serial_print("[PS2] set sc2 resp=");
                { let mut b = [0u8;18]; serial_print(hex_str(r as u64, &mut b)); }
                serial_print("\n");
            }
        }

        // Enable Scanning (0xF4)
        let ok = ps2_send_kbd(0xF4);
        serial_print(if ok { "[PS2] kbd scanning OK\n" } else { "[PS2] kbd F4 fail\n" });
    }

    // ── Mysz: reset + enable ─────────────────────────────────────────────────

    if has_mouse {
        // Enable port 2
        ps2_wait_write(); outb(0x64, 0xA8);

        // Reset myszy
        serial_print("[PS2] mouse reset\n");
        let ok_rst = ps2_send_mouse(0xFF);
        if ok_rst {
            // Czekaj na BAT myszy (0xAA + 0x00)
            for _ in 0..3 {
                if !ps2_wait_read() { break; }
                let r = inb(0x60);
                serial_print("[PS2] mouse resp=");
                { let mut b = [0u8;18]; serial_print(hex_str(r as u64, &mut b)); }
                serial_print("\n");
            }
        }

        // Enable myszy (0xF4)
        let ok_en = ps2_send_mouse(0xF4);
        serial_print(if ok_en { "[PS2] mouse OK\n" } else { "[PS2] mouse F4 fail\n" });

        // Włącz IRQ12 w CFG
        ps2_wait_write(); outb(0x64, 0x20);
        let mut cfg3 = if ps2_wait_read() { inb(0x60) } else { cfg };
        cfg3 |= 0x02; // IRQ12 enable
        ps2_wait_write(); outb(0x64, 0x60);
        ps2_wait_write(); outb(0x60, cfg3);
    }

    serial_print("[PS2] init done\n");
}
