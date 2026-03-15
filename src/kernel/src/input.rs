// CosinusOS — input.rs
// PS/2 Keyboard: Scan Code Set 2, IRQ1
// Bez myszy — kernel text mode nie potrzebuje myszy

use crate::debug::{inb, outb, serial_print, serial_hex, hex_str};

// ════════════════════════════════════════════════════════════════════════════
// § Scan Code Set 2 → ASCII
// ════════════════════════════════════════════════════════════════════════════

static SC2_NORM: [u8; 132] = [
/*00*/ 0,
/*01*/ 0,       // F9
/*02*/ 0,
/*03*/ 0,       // F5
/*04*/ 0,       // F3
/*05*/ 0,       // F1
/*06*/ 0,       // F2
/*07*/ 0,       // F12
/*08*/ 0,
/*09*/ 0,       // F10
/*0A*/ 0,       // F8
/*0B*/ 0,       // F6
/*0C*/ 0,       // F4
/*0D*/ b'\t',
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
/*5A*/ b'\n',
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
/*69*/ b'1',
/*6A*/ 0,
/*6B*/ b'4',
/*6C*/ b'7',
/*6D*/ 0,
/*6E*/ 0,
/*6F*/ 0,
/*70*/ b'0',
/*71*/ b'.',
/*72*/ b'2',
/*73*/ b'5',
/*74*/ b'6',
/*75*/ b'8',
/*76*/ b'\x1b', // Escape
/*77*/ 0,
/*78*/ 0,       // F11
/*79*/ b'+',
/*7A*/ b'3',
/*7B*/ b'-',
/*7C*/ b'*',
/*7D*/ b'9',
/*7E*/ 0,
/*7F*/ 0,
/*80*/ 0,
/*81*/ 0,
/*82*/ 0,
];

static SC2_SHFT: [u8; 132] = [
/*00*/ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
/*0D*/ b'\t',
/*0E*/ b'~',
/*0F*/ 0, 0, 0, 0, 0, 0,
/*15*/ b'Q',
/*16*/ b'!',
/*17*/ 0, 0, 0,
/*1A*/ b'Z',
/*1B*/ b'S',
/*1C*/ b'A',
/*1D*/ b'W',
/*1E*/ b'@',
/*1F*/ 0, 0,
/*21*/ b'C',
/*22*/ b'X',
/*23*/ b'D',
/*24*/ b'E',
/*25*/ b'$',
/*26*/ b'#',
/*27*/ 0, 0,
/*29*/ b' ',
/*2A*/ b'V',
/*2B*/ b'F',
/*2C*/ b'T',
/*2D*/ b'R',
/*2E*/ b'%',
/*2F*/ 0, 0,
/*31*/ b'N',
/*32*/ b'B',
/*33*/ b'H',
/*34*/ b'G',
/*35*/ b'Y',
/*36*/ b'^',
/*37*/ 0, 0, 0,
/*3A*/ b'M',
/*3B*/ b'J',
/*3C*/ b'U',
/*3D*/ b'&',
/*3E*/ b'*',
/*3F*/ 0, 0,
/*41*/ b'<',
/*42*/ b'K',
/*43*/ b'I',
/*44*/ b'O',
/*45*/ b')',
/*46*/ b'(',
/*47*/ 0, 0,
/*49*/ b'>',
/*4A*/ b'?',
/*4B*/ b'L',
/*4C*/ b':',
/*4D*/ b'P',
/*4E*/ b'_',
/*4F*/ 0, 0, 0,
/*52*/ b'"',
/*53*/ 0,
/*54*/ b'{',
/*55*/ b'+',
/*56*/ 0, 0, 0, 0,
/*5A*/ b'\n',
/*5B*/ b'}',
/*5C*/ 0,
/*5D*/ b'|',
/*5E*/ 0, 0, 0, 0, 0, 0, 0, 0,
/*66*/ b'\x08',
/*67*/ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
/*76*/ b'\x1b',
/*77*/ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

// ════════════════════════════════════════════════════════════════════════════
// § Keyboard state machine
// ════════════════════════════════════════════════════════════════════════════

#[derive(PartialEq)]
enum KbdState { Idle, Extended, Break, ExtBreak }

static mut KBD_STATE: KbdState = KbdState::Idle;
static mut KBD_SHIFT: bool     = false;
static mut KBD_CTRL:  bool     = false;

// ════════════════════════════════════════════════════════════════════════════
// § Ring buffer
// ════════════════════════════════════════════════════════════════════════════

const KB_BUF: usize = 64;
static mut KB_BUF_DATA: [char; KB_BUF] = ['\0'; KB_BUF];
static mut KB_HEAD: usize = 0;
static mut KB_TAIL: usize = 0;

unsafe fn kb_push(c: char) {
    let next = (KB_HEAD + 1) % KB_BUF;
    if next != KB_TAIL { KB_BUF_DATA[KB_HEAD] = c; KB_HEAD = next; }
}

pub unsafe fn input_poll() -> Option<char> {
    if KB_HEAD == KB_TAIL { return None; }
    let c = KB_BUF_DATA[KB_TAIL];
    KB_TAIL = (KB_TAIL + 1) % KB_BUF;
    Some(c)
}

pub unsafe fn input_push(c: char) { kb_push(c); }

// ════════════════════════════════════════════════════════════════════════════
// § IRQ1 handler
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn kbd_irq() {
    let sc = inb(0x60);
    serial_print("[KB] sc="); serial_hex(sc as u64); serial_print("\n");

    match sc {
        0xE0 => { KBD_STATE = KbdState::Extended; return; }
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

    match (sc, extended) {
        (0x12, false) | (0x59, false) => { KBD_SHIFT = !is_break; return; }
        (0x14, _)                     => { KBD_CTRL  = !is_break; return; }
        (0x11, _)                     => { return; }
        (0x58, false) if !is_break    => { KBD_SHIFT = !KBD_SHIFT; return; }
        _ => {}
    }

    if is_break { return; }

    if extended {
        match sc {
            0x71 => { kb_push('\x7f'); }
            0x5A => { kb_push('\n');   }
            0x4A => { kb_push('/');    }
            _ => {}
        }
        return;
    }

    let idx = sc as usize;
    if idx >= SC2_NORM.len() { return; }

    if KBD_CTRL {
        let base = if KBD_SHIFT { SC2_SHFT[idx] } else { SC2_NORM[idx] };
        if base >= b'a' && base <= b'z' { kb_push((base - b'a' + 1) as char); return; }
        if base >= b'A' && base <= b'Z' { kb_push((base - b'A' + 1) as char); return; }
    }

    let c = if KBD_SHIFT { SC2_SHFT[idx] } else { SC2_NORM[idx] };
    if c != 0 { kb_push(c as char); }
}

// ════════════════════════════════════════════════════════════════════════════
// § 8042 helpers
// ════════════════════════════════════════════════════════════════════════════

unsafe fn ps2_flush() {
    for _ in 0..16 {
        if inb(0x64) & 0x01 != 0 {
            let b = inb(0x60);
            serial_print("[PS2] flush="); serial_hex(b as u64); serial_print("\n");
        } else { break; }
    }
}

unsafe fn ps2_wait_ibuf() {
    let mut t = 0usize;
    while inb(0x64) & 0x02 != 0 {
        t += 1;
        if t > 100_000 { serial_print("[PS2] ibuf timeout\n"); return; }
        core::hint::spin_loop();
    }
}

unsafe fn ps2_wait_obuf() -> bool {
    let mut t = 0usize;
    while inb(0x64) & 0x01 == 0 {
        t += 1;
        if t > 100_000 { return false; }
        core::hint::spin_loop();
    }
    true
}

/// Wyślij komendę do klawiatury, zbierz max n odpowiedzi, zwróć true jeśli ACK
unsafe fn kbd_cmd(cmd: u8, n: usize) -> bool {
    serial_print("[PS2] kbd_cmd="); serial_hex(cmd as u64); serial_print("\n");
    ps2_wait_ibuf();
    outb(0x60, cmd);
    let mut got_ack = false;
    for i in 0..n {
        if !ps2_wait_obuf() {
            serial_print("[PS2] cmd_timeout i="); serial_hex(i as u64); serial_print("\n");
            break;
        }
        let r = inb(0x60);
        serial_print("[PS2] cmd_resp["); serial_hex(i as u64);
        serial_print("]="); serial_hex(r as u64); serial_print("\n");
        if r == 0xFA { got_ack = true; break; }
        if r == 0xFE { // resend
            serial_print("[PS2] resend!\n");
            break;
        }
    }
    got_ack
}

// ════════════════════════════════════════════════════════════════════════════
// § Główna inicjalizacja
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn init_ps2() {
    serial_print("[PS2] === init ===\n");

    let st = inb(0x64);
    serial_print("[PS2] status="); serial_hex(st as u64); serial_print("\n");
    if st == 0xFF { serial_print("[PS2] brak kontrolera\n"); return; }

    // Disable obu portów
    ps2_wait_ibuf(); outb(0x64, 0xAD);
    ps2_wait_ibuf(); outb(0x64, 0xA7);
    ps2_flush();

    // Odczytaj CFG
    ps2_wait_ibuf(); outb(0x64, 0x20);
    let cfg_orig = if ps2_wait_obuf() { inb(0x60) } else { 0 };
    serial_print("[PS2] cfg_orig="); serial_hex(cfg_orig as u64); serial_print("\n");

    // CFG: IRQ1=1, IRQ12=0, translation=0
    let cfg_new = (cfg_orig | 0x01) & !0x42;
    serial_print("[PS2] cfg_new="); serial_hex(cfg_new as u64); serial_print("\n");
    ps2_wait_ibuf(); outb(0x64, 0x60);
    ps2_wait_ibuf(); outb(0x60, cfg_new);

    // Weryfikacja CFG
    ps2_wait_ibuf(); outb(0x64, 0x20);
    let cfg_check = if ps2_wait_obuf() { inb(0x60) } else { 0xFF };
    serial_print("[PS2] cfg_check="); serial_hex(cfg_check as u64); serial_print("\n");
    if cfg_check & 0x40 != 0 {
        serial_print("[PS2] WARN: translation wciaz ON!\n");
    } else {
        serial_print("[PS2] translation OFF OK\n");
    }

    // Enable port1
    ps2_wait_ibuf(); outb(0x64, 0xAE);
    serial_print("[PS2] port1 enabled\n");

    // Reset klawiatury
    serial_print("[PS2] === kbd reset ===\n");
    ps2_wait_ibuf(); outb(0x60, 0xFF);
    let mut got_bat = false;
    for i in 0..4usize {
        if !ps2_wait_obuf() {
            serial_print("[PS2] reset timeout i="); serial_hex(i as u64); serial_print("\n");
            break;
        }
        let r = inb(0x60);
        serial_print("[PS2] reset["); serial_hex(i as u64);
        serial_print("]="); serial_hex(r as u64); serial_print("\n");
        if r == 0xAA { got_bat = true; }
    }
    serial_print(if got_bat { "[PS2] BAT OK\n" } else { "[PS2] BAT missing\n" });
    ps2_flush();

    // Set Scan Code Set 2: wyślij 0xF0, potem 0x02
    serial_print("[PS2] === set SC2 ===\n");
    let ack_f0 = kbd_cmd(0xF0, 3);
    serial_print(if ack_f0 { "[PS2] 0xF0 ACK\n" } else { "[PS2] 0xF0 no ACK\n" });

    // Wyślij parametr 0x02
    serial_print("[PS2] sending param 0x02\n");
    ps2_wait_ibuf(); outb(0x60, 0x02);
    if ps2_wait_obuf() {
        let r = inb(0x60);
        serial_print("[PS2] param_resp="); serial_hex(r as u64); serial_print("\n");
    } else {
        serial_print("[PS2] param timeout\n");
    }
    ps2_flush();

    // Weryfikacja aktualnego scan set: 0xF0 0x00
    serial_print("[PS2] === verify SC ===\n");
    kbd_cmd(0xF0, 3);
    ps2_wait_ibuf(); outb(0x60, 0x00);
    for i in 0..3usize {
        if !ps2_wait_obuf() { break; }
        let r = inb(0x60);
        serial_print("[PS2] verify["); serial_hex(i as u64);
        serial_print("]="); serial_hex(r as u64); serial_print("\n");
    }
    ps2_flush();

    // Enable Scanning
    serial_print("[PS2] === enable scanning ===\n");
    let ack_f4 = kbd_cmd(0xF4, 3);
    serial_print(if ack_f4 { "[PS2] scanning ON\n" } else { "[PS2] scanning FAIL\n" });
    ps2_flush();

    serial_print("[PS2] === init done ===\n");
}