// CosinusOS — input.rs
// PS/2 Keyboard: Scan Code Set 1 (translation włączone w 8042)

use crate::debug::{inb, outb, serial_print, serial_hex, hex_str};

// ════════════════════════════════════════════════════════════════════════════
// § Scan Code Set 1 → ASCII
// ════════════════════════════════════════════════════════════════════════════

static SC1_NORM: [u8; 59] = [
    0, b'\x1b',
    b'1',b'2',b'3',b'4',b'5',b'6',b'7',b'8',b'9',b'0',b'-',b'=',b'\x08',
    b'\t',
    b'q',b'w',b'e',b'r',b't',b'y',b'u',b'i',b'o',b'p',b'[',b']',b'\n',
    0,
    b'a',b's',b'd',b'f',b'g',b'h',b'j',b'k',b'l',b';',b'\'',b'`',
    0, b'\\',
    b'z',b'x',b'c',b'v',b'b',b'n',b'm',b'\x2C',b'.',b'/',
    0, b'*', 0, b' ', 0,
];

static SC1_SHFT: [u8; 59] = [
    0, b'\x1b',
    b'!',b'@',b'#',b'$',b'%',b'^',b'&',b'*',b'(',b')',b'_',b'+',b'\x08',
    b'\t',
    b'Q',b'W',b'E',b'R',b'T',b'Y',b'U',b'I',b'O',b'P',b'{',b'}',b'\n',
    0,
    b'A',b'S',b'D',b'F',b'G',b'H',b'J',b'K',b'L',b':',b'"',b'~',
    0, b'|',
    b'Z',b'X',b'C',b'V',b'B',b'N',b'M',b'<',b'>',b'?',
    0, b'*', 0, b' ', 0,
];

// ════════════════════════════════════════════════════════════════════════════
// § Keyboard state
// ════════════════════════════════════════════════════════════════════════════

static mut KBD_SHIFT: bool = false;
static mut KBD_CTRL:  bool = false;

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
// § IRQ1 handler — Set 1
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn kbd_irq() {
    let sc = inb(0x60);
    serial_print("[KB] sc="); serial_hex(sc as u64); serial_print("\n");

    match sc {
        0x2A | 0x36 => { KBD_SHIFT = true;  return; } // LShift/RShift make
        0xAA | 0xB6 => { KBD_SHIFT = false; return; } // LShift/RShift break
        0x1D        => { KBD_CTRL  = true;  return; } // LCtrl make
        0x9D        => { KBD_CTRL  = false; return; } // LCtrl break
        _ => {}
    }

    // Break codes bit7=1 — ignoruj
    if sc & 0x80 != 0 { return; }

    let idx = sc as usize;
    if idx >= SC1_NORM.len() { return; }

    if KBD_CTRL {
        let base = if KBD_SHIFT { SC1_SHFT[idx] } else { SC1_NORM[idx] };
        if base >= b'a' && base <= b'z' { kb_push((base - b'a' + 1) as char); return; }
        if base >= b'A' && base <= b'Z' { kb_push((base - b'A' + 1) as char); return; }
    }

    let c = if KBD_SHIFT { SC1_SHFT[idx] } else { SC1_NORM[idx] };
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
        if r == 0xFE { break; }
    }
    got_ack
}

// ════════════════════════════════════════════════════════════════════════════
// § Init — Set1 z translation włączonym
// ════════════════════════════════════════════════════════════════════════════

pub unsafe fn init_ps2() {
    serial_print("[PS2] === init ===\n");

    let st = inb(0x64);
    serial_print("[PS2] status="); serial_hex(st as u64); serial_print("\n");
    if st == 0xFF { serial_print("[PS2] brak kontrolera\n"); return; }

    // Maskuj IRQ1 podczas konfiguracji
    let pic_mask = inb(0x21);
    outb(0x21, pic_mask | 0x02);

    // Disable obu portów
    ps2_wait_ibuf(); outb(0x64, 0xAD);
    ps2_wait_ibuf(); outb(0x64, 0xA7);
    ps2_flush();

    // Odczytaj CFG
    ps2_wait_ibuf(); outb(0x64, 0x20);
    let cfg_orig = if ps2_wait_obuf() { inb(0x60) } else { 0 };
    serial_print("[PS2] cfg_orig="); serial_hex(cfg_orig as u64); serial_print("\n");

    // IRQ1=1, translation=1 (Set1 przez hardware translation), IRQ12=0
    let cfg_new = (cfg_orig | 0x41) & !0x02;
    serial_print("[PS2] cfg_new="); serial_hex(cfg_new as u64); serial_print("\n");
    ps2_wait_ibuf(); outb(0x64, 0x60);
    ps2_wait_ibuf(); outb(0x60, cfg_new);

    // Enable port1
    ps2_wait_ibuf(); outb(0x64, 0xAE);

    // Reset klawiatury
    ps2_wait_ibuf(); outb(0x60, 0xFF);
    let mut got_bat = false;
    for i in 0..4usize {
        if !ps2_wait_obuf() { break; }
        let r = inb(0x60);
        serial_print("[PS2] reset["); serial_hex(i as u64);
        serial_print("]="); serial_hex(r as u64); serial_print("\n");
        if r == 0xAA { got_bat = true; }
    }
    serial_print(if got_bat { "[PS2] BAT OK\n" } else { "[PS2] BAT missing\n" });
    ps2_flush();

    // Enable Scanning
    let ack = kbd_cmd(0xF4, 3);
    serial_print(if ack { "[PS2] scanning ON\n" } else { "[PS2] scanning FAIL\n" });
    ps2_flush();

    // Włącz IRQ1
    outb(0x21, inb(0x21) & !0x02);
    serial_print("[PS2] === init done ===\n");
}