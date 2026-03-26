// userspace — tui.rs
// TUI engine: reset trybu VGA, mały tekst (80×25 → rysujemy w trybie tekstowym),
// system paneli, status bar, linia inputu.
//
// "Reset trybu wyświetlania" na x86:
//   Przez BIOS INT 10h nie mamy dostępu z ring-3 protected mode.
//   Zamiast tego kernel init już ustawił VGA text mode 3 (80×25).
//   My robimy soft-reset: czyścimy bufor, ustawiamy kursor, rysujemy layout.
//
// Rysowanie: przez sys_write (fd=1) z sekwencjami ANSI lub bezpośrednio
//   przez debug_print. Skoro kernel ma VGA text buffer (0xB8000), używamy
//   sys_write który kieruje do putc → VGA.

use libcosinus::print;
use libcosinus::fmt::FmtBuf;
use crate::plugin::api::DrawCtx;

// ── Wymiary ekranu ────────────────────────────────────────────────────────────

pub const SCREEN_W: u16 = 80;
pub const SCREEN_H: u16 = 25;
pub const HEADER_H: u16 = 1;
pub const FOOTER_H: u16 = 2;   // status bar + input line
pub const CONTENT_H: u16 = SCREEN_H - HEADER_H - FOOTER_H;

// ── ANSI sekwencje ────────────────────────────────────────────────────────────
// Kernel putc przepuszcza raw bajty do VGA. Jeśli terminal nie interpretuje ANSI,
// możesz zamienić te na bezpośredni zapis do 0xB8000 przez dedykowany syscall.

const ESC: &str = "\x1b[";

pub fn clear_screen() {
    print("\x1b[2J\x1b[H");
}

pub fn cursor_to(x: u16, y: u16) {
    let mut fb = FmtBuf::<24>::new();
    fb.push_str("\x1b[");
    fb.push_u64((y + 1) as u64);
    fb.push_str(";");
    fb.push_u64((x + 1) as u64);
    fb.push_str("H");
    print(fb.as_str());
}

pub fn set_color(fg: u8, bg: u8) {
    let mut fb = FmtBuf::<24>::new();
    fb.push_str("\x1b[");
    fb.push_u64(30 + fg as u64);
    fb.push_str(";");
    fb.push_u64(40 + bg as u64);
    fb.push_str("m");
    print(fb.as_str());
}

pub fn reset_color() { print("\x1b[0m"); }
pub fn bold()        { print("\x1b[1m"); }
pub fn cursor_hide() { print("\x1b[?25l"); }
pub fn cursor_show() { print("\x1b[?25h"); }

// ── Kolory ────────────────────────────────────────────────────────────────────
pub mod color {
    pub const BLACK:   u8 = 0;
    pub const RED:     u8 = 1;
    pub const GREEN:   u8 = 2;
    pub const YELLOW:  u8 = 3;
    pub const BLUE:    u8 = 4;
    pub const MAGENTA: u8 = 5;
    pub const CYAN:    u8 = 6;
    pub const WHITE:   u8 = 7;
}

// ── Prymitywy rysowania ────────────────────────────────────────────────────────

pub fn draw_hline(x: u16, y: u16, len: u16, ch: char) {
    cursor_to(x, y);
    let mut fb = FmtBuf::<128>::new();
    for _ in 0..len { fb.push_char(ch); }
    print(fb.as_str());
}

pub fn draw_vline(x: u16, y: u16, len: u16, ch: char) {
    for i in 0..len {
        cursor_to(x, y + i);
        let mut fb = FmtBuf::<4>::new();
        fb.push_char(ch);
        print(fb.as_str());
    }
}

pub fn draw_box(x: u16, y: u16, w: u16, h: u16, title: &str) {
    // Górna krawędź
    cursor_to(x, y);
    let inner = w.saturating_sub(2) as usize;
    let title_part = &title[..title.len().min(inner.saturating_sub(2))];
    let mut top = FmtBuf::<128>::new();
    top.push_char('┌');
    if !title_part.is_empty() {
        top.push_char('[');
        top.push_str(title_part);
        top.push_char(']');
        let fill = inner.saturating_sub(title_part.len() + 2);
        for _ in 0..fill { top.push_char('─'); }
    } else {
        for _ in 0..inner { top.push_char('─'); }
    }
    top.push_char('┐');
    print(top.as_str());

    // Boki
    for row in 1..h.saturating_sub(1) {
        cursor_to(x, y + row);
        let mut line = FmtBuf::<128>::new();
        line.push_char('│');
        for _ in 0..inner { line.push_char(' '); }
        line.push_char('│');
        print(line.as_str());
    }

    // Dolna krawędź
    cursor_to(x, y + h.saturating_sub(1));
    let mut bot = FmtBuf::<128>::new();
    bot.push_char('└');
    for _ in 0..inner { bot.push_char('─'); }
    bot.push_char('┘');
    print(bot.as_str());
}

pub fn draw_text_at(x: u16, y: u16, text: &str) {
    cursor_to(x, y);
    print(text);
}

pub fn draw_text_clipped(x: u16, y: u16, text: &str, max_w: u16) {
    cursor_to(x, y);
    let clip = &text[..text.len().min(max_w as usize)];
    print(clip);
}

// ── Layout TUI ────────────────────────────────────────────────────────────────

pub struct Tui {
    dirty: bool,
    input_buf: [u8; 128],
    input_len: usize,
    status_msg: [u8; 80],
    status_len: usize,
}

impl Tui {
    pub const fn new() -> Self {
        Self {
            dirty: true,
            input_buf: [0; 128],
            input_len: 0,
            status_msg: [0; 80],
            status_len: 0,
        }
    }

    /// Pełny reset + narysuj bazowy layout
    pub fn init(&mut self) {
        clear_screen();
        cursor_hide();
        self.draw_header();
        self.draw_footer();
        self.dirty = false;
    }

    pub fn draw_header(&self) {
        cursor_to(0, 0);
        set_color(color::BLACK, color::CYAN);
        bold();
        let mut hdr = FmtBuf::<128>::new();
        hdr.push_str(" CosinusOS  Plugin Manager ");
        // Wypełnij do końca linii
        let used = hdr.len();
        for _ in used..SCREEN_W as usize { hdr.push_char(' '); }
        print(hdr.as_str());
        reset_color();
    }

    pub fn draw_footer(&self) {
        // Status bar
        cursor_to(0, SCREEN_H - 2);
        set_color(color::BLACK, color::WHITE);
        let mut sb = FmtBuf::<128>::new();
        let status = if self.status_len > 0 {
            core::str::from_utf8(&self.status_msg[..self.status_len]).unwrap_or("")
        } else {
            "Tab=focus  Ctrl+L=reload  Ctrl+Q=quit"
        };
        sb.push_str(status);
        for _ in sb.len()..SCREEN_W as usize { sb.push_char(' '); }
        print(sb.as_str());
        reset_color();

        // Input line
        cursor_to(0, SCREEN_H - 1);
        let mut inp = FmtBuf::<140>::new();
        inp.push_str("> ");
        let cmd = core::str::from_utf8(&self.input_buf[..self.input_len]).unwrap_or("");
        inp.push_str(cmd);
        for _ in inp.len()..SCREEN_W as usize { inp.push_char(' '); }
        print(inp.as_str());

        // Ustaw kursor na koniec inputu
        cursor_to(2 + self.input_len as u16, SCREEN_H - 1);
        cursor_show();
    }

    pub fn set_status(&mut self, msg: &str) {
        let b = msg.as_bytes();
        let len = b.len().min(80);
        self.status_msg[..len].copy_from_slice(&b[..len]);
        self.status_len = len;
    }

    // ── Input line ────────────────────────────────────────────────────────────

    pub fn input_push(&mut self, ch: u8) {
        if self.input_len < 127 {
            self.input_buf[self.input_len] = ch;
            self.input_len += 1;
        }
    }

    pub fn input_backspace(&mut self) {
        if self.input_len > 0 { self.input_len -= 1; }
    }

    pub fn input_clear(&mut self) { self.input_len = 0; }
    pub fn input_len(&self) -> usize { self.input_len }

    pub fn input_str(&self) -> &str {
        core::str::from_utf8(&self.input_buf[..self.input_len]).unwrap_or("")
    }

    pub fn input_take(&mut self) -> &str {
        // Zwraca current input i czyści (zwraca slice do bufora — ważne użyć przed clear)
        let s = core::str::from_utf8(&self.input_buf[..self.input_len]).unwrap_or("");
        s
    }
}

// ── Panel helper dla pluginów ─────────────────────────────────────────────────
// Pluginy rysują wewnątrz swojego DrawCtx przez ten helper zamiast
// bezpośrednio przez cursor_to żeby mieć clipping.

pub struct PanelPainter<'a> {
    pub ctx: &'a DrawCtx,
    pub row: u16,
}

impl<'a> PanelPainter<'a> {
    pub fn new(ctx: &'a DrawCtx) -> Self {
        Self { ctx, row: 0 }
    }

    /// Narysuj ramkę panelu z tytułem
    pub fn frame(&self, title: &str) {
        draw_box(self.ctx.x, self.ctx.y, self.ctx.w, self.ctx.h, title);
    }

    /// Wpisz linię tekstu wewnątrz ramki (auto-wrap do następnej linii)
    pub fn println(&mut self, text: &str) {
        if self.row >= self.ctx.h.saturating_sub(2) { return; }
        let inner_x = self.ctx.x + 1;
        let inner_y = self.ctx.y + 1 + self.row;
        let max_w   = self.ctx.w.saturating_sub(2);
        draw_text_clipped(inner_x, inner_y, text, max_w);
        self.row += 1;
    }

    /// Wyczyść wnętrze panelu
    pub fn clear_inner(&self) {
        let inner_w = self.ctx.w.saturating_sub(2) as usize;
        let mut blank = FmtBuf::<64>::new();
        for _ in 0..inner_w { blank.push_char(' '); }
        for row in 0..self.ctx.h.saturating_sub(2) {
            draw_text_at(self.ctx.x + 1, self.ctx.y + 1 + row, blank.as_str());
        }
    }

    pub fn reset_row(&mut self) { self.row = 0; }
}