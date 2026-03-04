// CosinusOS Userspace — graphics.rs
// Color, Framebuffer (software renderer) + GUI: Rect, Widget, Button, Window

use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;

// ── Color ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct Color { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }

impl Color {
    pub const fn rgb (r: u8, g: u8, b: u8)        -> Self { Self { r, g, b, a: 255 } }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self { Self { r, g, b, a } }

    #[inline(always)]
    pub fn to_u32(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) |
        ((self.g as u32) << 8)  |  (self.b as u32)
    }

    pub const BLACK:   Color = Color::rgb(0,   0,   0);
    pub const WHITE:   Color = Color::rgb(255, 255, 255);
    pub const RED:     Color = Color::rgb(255, 0,   0);
    pub const GREEN:   Color = Color::rgb(0,   255, 0);
    pub const BLUE:    Color = Color::rgb(0,   0,   255);
    pub const YELLOW:  Color = Color::rgb(255, 255, 0);
    pub const CYAN:    Color = Color::rgb(0,   255, 255);
    pub const MAGENTA: Color = Color::rgb(255, 0,   255);
}

// ── Framebuffer ───────────────────────────────────────────────────────────────

pub struct Framebuffer {
    pub width:  usize,
    pub height: usize,
    pub buffer: Vec<u32>,
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize(width * height, 0);
        Self { width, height, buffer }
    }

    #[inline(always)]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x < self.width && y < self.height {
            self.buffer[y * self.width + x] = color.to_u32();
        }
    }

    pub fn clear(&mut self, color: Color) {
        self.buffer.fill(color.to_u32());
    }

    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        let pixel = color.to_u32();
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for row in y..y_end {
            let base = row * self.width;
            if let Some(slice) = self.buffer.get_mut(base + x..base + x_end) {
                slice.fill(pixel);
            }
        }
    }

    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let dx  =  (x1 - x0).abs();
        let dy  = -(y1 - y0).abs();
        let sx  = if x0 < x1 { 1i32 } else { -1 };
        let sy  = if y0 < y1 { 1i32 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            if x >= 0 && y >= 0 { self.set_pixel(x as usize, y as usize, color); }
            if x == x1 && y == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy { err += dy; x += sx; }
            if e2 <= dx { err += dx; y += sy; }
        }
    }

    pub fn draw_circle(&mut self, cx: i32, cy: i32, r: i32, color: Color) {
        let (mut x, mut y, mut d) = (0i32, r, 3 - 2 * r);
        while x <= y {
            for (px, py) in [
                (cx-x, cy-y),(cx+x, cy-y),(cx-x, cy+y),(cx+x, cy+y),
                (cx-y, cy-x),(cx+y, cy-x),(cx-y, cy+x),(cx+y, cy+x),
            ] {
                if px >= 0 && py >= 0 { self.set_pixel(px as usize, py as usize, color); }
            }
            if d < 0 { d += 4*x+6; } else { d += 4*(x-y)+10; y -= 1; }
            x += 1;
        }
    }
}

// ── GUI ───────────────────────────────────────────────────────────────────────

pub struct Rect { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self { Self { x, y, width, height } }
    #[inline(always)]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width  as i32 &&
        y >= self.y && y < self.y + self.height as i32
    }
}

pub trait Widget {
    fn bounds(&self)                            -> Rect;
    fn draw(&self, fb: &mut Framebuffer);
    fn handle_click(&mut self, x: i32, y: i32) -> bool;
}

pub struct Button {
    pub rect:     Rect,
    pub label:    String,
    pub bg_color: Color,
    pub on_click: Option<Box<dyn FnMut()>>,
}

impl Button {
    pub fn new(rect: Rect, label: &str) -> Self {
        Self {
            rect,
            label:    String::from(label),
            bg_color: Color::rgb(100, 100, 200),
            on_click: None,
        }
    }
    pub fn with_callback<F: FnMut() + 'static>(mut self, callback: F) -> Self {
        self.on_click = Some(Box::new(callback)); self
    }
}

impl Widget for Button {
    fn bounds(&self) -> Rect {
        Rect::new(self.rect.x, self.rect.y, self.rect.width, self.rect.height)
    }
    fn draw(&self, fb: &mut Framebuffer) {
        fb.draw_rect(
            self.rect.x as usize, self.rect.y as usize,
            self.rect.width as usize, self.rect.height as usize,
            self.bg_color,
        );
    }
    fn handle_click(&mut self, x: i32, y: i32) -> bool {
        if self.rect.contains(x, y) {
            if let Some(ref mut cb) = self.on_click { cb(); }
            true
        } else { false }
    }
}

pub struct Window {
    pub title:   String,
    pub rect:    Rect,
    pub widgets: Vec<Box<dyn Widget>>,
}

impl Window {
    pub fn new(title: &str, rect: Rect) -> Self {
        Self { title: String::from(title), rect, widgets: Vec::new() }
    }
    pub fn add_widget(&mut self, widget: Box<dyn Widget>) { self.widgets.push(widget); }
    pub fn draw(&self, fb: &mut Framebuffer) {
        fb.draw_rect(
            self.rect.x as usize, self.rect.y as usize,
            self.rect.width as usize, self.rect.height as usize,
            Color::rgb(200, 200, 200),
        );
        // Pasek tytułu
        fb.draw_rect(
            self.rect.x as usize, self.rect.y as usize,
            self.rect.width as usize, 30,
            Color::rgb(50, 50, 150),
        );
        for w in &self.widgets { w.draw(fb); }
    }
    pub fn handle_click(&mut self, x: i32, y: i32) {
        for w in &mut self.widgets { if w.handle_click(x, y) { break; } }
    }
}
