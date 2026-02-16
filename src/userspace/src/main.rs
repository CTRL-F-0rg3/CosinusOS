#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use core::fmt::{self, Write};
use core::alloc::{GlobalAlloc, Layout};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;

// ============================================================================
// SYSCALL INTERFACE
// ============================================================================

#[repr(usize)]
#[derive(Copy, Clone)]
pub enum Syscall {
    Exit = 0,
    Write = 1,
    Read = 2,
}

#[inline(always)]
pub unsafe fn syscall0(num: Syscall) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") num as usize,
            lateout("rax") ret,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall1(num: Syscall, arg1: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") num as usize,
            in("rdi") arg1,
            lateout("rax") ret,
            options(nostack)
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") num as usize,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            lateout("rax") ret,
            options(nostack)
        );
    }
    ret
}

// ============================================================================
// STDIO
// ============================================================================

pub fn print(s: &str) {
    unsafe {
        syscall3(Syscall::Write, 1, s.as_ptr() as usize, s.len());
    }
}

pub fn println(s: &str) {
    print(s);
    print("\n");
}

pub fn exit(code: i32) -> ! {
    unsafe {
        syscall1(Syscall::Exit, code as usize);
    }
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

// ============================================================================
// MEMORY ALLOCATOR
// ============================================================================

const HEAP_SIZE: usize = 10 * 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

pub struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();
        
        unsafe {
            let pos = (HEAP_POS + align - 1) & !(align - 1);
            
            if pos + size > HEAP_SIZE {
                return core::ptr::null_mut();
            }
            
            HEAP_POS = pos + size;
            HEAP.as_mut_ptr().add(pos)
        }
    }
    
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

// ============================================================================
// PANIC HANDLER
// ============================================================================

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println("PANIC:");
    if let Some(location) = info.location() {
        print("  File: ");
        print(location.file());
        print("\n");
    }
    exit(1);
}

// ============================================================================
// FORMATTING
// ============================================================================

pub struct Writer;

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        print(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! print_fmt {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let mut writer = $crate::Writer;
        let _ = write!(&mut writer, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println_fmt {
    () => { $crate::print("\n") };
    ($($arg:tt)*) => {{
        $crate::print_fmt!($($arg)*);
        $crate::print("\n");
    }};
}

// ============================================================================
// COLLECTIONS - HashMap
// ============================================================================

use alloc::vec;

pub struct HashMap<K, V> {
    buckets: Vec<Vec<(K, V)>>,
    len: usize,
}

impl<K: PartialEq, V> HashMap<K, V> {
    pub fn new() -> Self {
        let mut buckets = Vec::new();
        for _ in 0..16 {
            buckets.push(Vec::new());
        }
        Self { buckets, len: 0 }
    }
    
    fn hash(&self, key: &K) -> usize {
        let ptr = key as *const K as usize;
        ptr % self.buckets.len()
    }
    
    pub fn insert(&mut self, key: K, value: V) {
        let idx = self.hash(&key);
        let bucket = &mut self.buckets[idx];
        
        for item in bucket.iter_mut() {
            if item.0 == key {
                item.1 = value;
                return;
            }
        }
        
        bucket.push((key, value));
        self.len += 1;
    }
    
    pub fn get(&self, key: &K) -> Option<&V> {
        let idx = self.hash(key);
        let bucket = &self.buckets[idx];
        
        for item in bucket.iter() {
            if &item.0 == key {
                return Some(&item.1);
            }
        }
        None
    }
    
    pub fn len(&self) -> usize {
        self.len
    }
}

// ============================================================================
// SYNCHRONIZATION
// ============================================================================

pub struct Mutex<T> {
    data: core::cell::UnsafeCell<T>,
}

unsafe impl<T> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Self {
            data: core::cell::UnsafeCell::new(data),
        }
    }
    
    pub fn lock(&self) -> MutexGuard<T> {
        MutexGuard { mutex: self }
    }
}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<'a, T> core::ops::Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

// ============================================================================
// MATH
// ============================================================================

pub mod math {
    pub fn min<T: PartialOrd>(a: T, b: T) -> T {
        if a < b { a } else { b }
    }
    
    pub fn max<T: PartialOrd>(a: T, b: T) -> T {
        if a > b { a } else { b }
    }
    
    pub fn abs(x: i32) -> i32 {
        if x < 0 { -x } else { x }
    }
}

// ============================================================================
// RANDOM
// ============================================================================

static mut RNG_STATE: u64 = 12345;

pub fn random() -> u32 {
    unsafe {
        RNG_STATE = RNG_STATE.wrapping_mul(1103515245).wrapping_add(12345);
        (RNG_STATE / 65536) as u32 % 32768
    }
}

pub fn random_range(min: u32, max: u32) -> u32 {
    min + (random() % (max - min + 1))
}

// ============================================================================
// GRAPHICS
// ============================================================================

#[derive(Copy, Clone)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(255, 0, 0);
    pub const GREEN: Color = Color::rgb(0, 255, 0);
    pub const BLUE: Color = Color::rgb(0, 0, 255);
}

pub struct Framebuffer {
    pub width: usize,
    pub height: usize,
    pub buffer: Vec<u32>,
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::new();
        buffer.resize(width * height, 0);
        Self { width, height, buffer }
    }
    
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x < self.width && y < self.height {
            let idx = y * self.width + x;
            self.buffer[idx] = ((color.a as u32) << 24) |
                              ((color.r as u32) << 16) |
                              ((color.g as u32) << 8) |
                              (color.b as u32);
        }
    }
    
    pub fn clear(&mut self, color: Color) {
        let pixel = ((color.a as u32) << 24) |
                   ((color.r as u32) << 16) |
                   ((color.g as u32) << 8) |
                   (color.b as u32);
        for p in self.buffer.iter_mut() {
            *p = pixel;
        }
    }
    
    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        for dy in 0..h {
            for dx in 0..w {
                self.set_pixel(x + dx, y + dy, color);
            }
        }
    }
    
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        
        let mut x = x0;
        let mut y = y0;
        
        loop {
            if x >= 0 && y >= 0 {
                self.set_pixel(x as usize, y as usize, color);
            }
            
            if x == x1 && y == y1 { break; }
            
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }
}

// ============================================================================
// GUI
// ============================================================================

pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }
    
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width as i32 &&
        y >= self.y && y < self.y + self.height as i32
    }
}

pub trait Widget {
    fn bounds(&self) -> Rect;
    fn draw(&self, fb: &mut Framebuffer);
    fn handle_click(&mut self, x: i32, y: i32) -> bool;
}

pub struct Button {
    pub rect: Rect,
    pub label: String,
    pub bg_color: Color,
    pub on_click: Option<Box<dyn FnMut()>>,
}

impl Button {
    pub fn new(rect: Rect, label: &str) -> Self {
        Self {
            rect,
            label: String::from(label),
            bg_color: Color::rgb(100, 100, 200),
            on_click: None,
        }
    }
    
    pub fn with_callback<F: FnMut() + 'static>(mut self, callback: F) -> Self {
        self.on_click = Some(Box::new(callback));
        self
    }
}

impl Widget for Button {
    fn bounds(&self) -> Rect {
        Rect::new(self.rect.x, self.rect.y, self.rect.width, self.rect.height)
    }
    
    fn draw(&self, fb: &mut Framebuffer) {
        fb.draw_rect(
            self.rect.x as usize,
            self.rect.y as usize,
            self.rect.width as usize,
            self.rect.height as usize,
            self.bg_color
        );
    }
    
    fn handle_click(&mut self, x: i32, y: i32) -> bool {
        if self.rect.contains(x, y) {
            if let Some(ref mut callback) = self.on_click {
                callback();
            }
            true
        } else {
            false
        }
    }
}

pub struct Window {
    pub title: String,
    pub rect: Rect,
    pub widgets: Vec<Box<dyn Widget>>,
}

impl Window {
    pub fn new(title: &str, rect: Rect) -> Self {
        Self {
            title: String::from(title),
            rect,
            widgets: Vec::new(),
        }
    }
    
    pub fn add_widget(&mut self, widget: Box<dyn Widget>) {
        self.widgets.push(widget);
    }
    
    pub fn draw(&self, fb: &mut Framebuffer) {
        fb.draw_rect(
            self.rect.x as usize,
            self.rect.y as usize,
            self.rect.width as usize,
            self.rect.height as usize,
            Color::rgb(200, 200, 200)
        );
        
        fb.draw_rect(
            self.rect.x as usize,
            self.rect.y as usize,
            self.rect.width as usize,
            30,
            Color::rgb(50, 50, 150)
        );
        
        for widget in &self.widgets {
            widget.draw(fb);
        }
    }
    
    pub fn handle_click(&mut self, x: i32, y: i32) {
        for widget in &mut self.widgets {
            if widget.handle_click(x, y) {
                break;
            }
        }
    }
}

// ============================================================================
// TERMINAL
// ============================================================================

pub struct Terminal {
    pub width: usize,
    pub height: usize,
    pub buffer: Vec<Vec<char>>,
    pub cursor_x: usize,
    pub cursor_y: usize,
}

impl Terminal {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::new();
        for _ in 0..height {
            let mut row = Vec::new();
            row.resize(width, ' ');
            buffer.push(row);
        }
        
        Self {
            width,
            height,
            buffer,
            cursor_x: 0,
            cursor_y: 0,
        }
    }
    
    pub fn write_char(&mut self, c: char) {
        if c == '\n' {
            self.cursor_x = 0;
            self.cursor_y += 1;
        } else {
            if self.cursor_x < self.width {
                self.buffer[self.cursor_y][self.cursor_x] = c;
                self.cursor_x += 1;
            }
        }
        
        if self.cursor_x >= self.width {
            self.cursor_x = 0;
            self.cursor_y += 1;
        }
        
        if self.cursor_y >= self.height {
            self.scroll();
        }
    }
    
    pub fn write_str(&mut self, s: &str) {
        for c in s.chars() {
            self.write_char(c);
        }
    }
    
    fn scroll(&mut self) {
        for y in 1..self.height {
            self.buffer[y - 1] = self.buffer[y].clone();
        }
        self.buffer[self.height - 1] = vec![' '; self.width];
        self.cursor_y = self.height - 1;
    }
    
    pub fn clear(&mut self) {
        for row in &mut self.buffer {
            for c in row {
                *c = ' ';
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }
}

// ============================================================================
// DRIVER INTERFACE
// ============================================================================

pub trait Driver {
    fn name(&self) -> &str;
    fn init(&mut self) -> Result<(), ()>;
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()>;
    fn write(&mut self, buf: &[u8]) -> Result<usize, ()>;
}

pub struct DriverManager {
    drivers: Vec<Box<dyn Driver>>,
}

impl DriverManager {
    pub fn new() -> Self {
        Self {
            drivers: Vec::new(),
        }
    }
    
    pub fn register(&mut self, driver: Box<dyn Driver>) {
        self.drivers.push(driver);
    }
    
    pub fn init_all(&mut self) {
        for driver in &mut self.drivers {
            match driver.init() {
                Ok(_) => {
                    print("Driver ");
                    print(driver.name());
                    println(" initialized");
                }
                Err(_) => {
                    print("Driver ");
                    print(driver.name());
                    println(" failed!");
                }
            }
        }
    }
}

// ============================================================================
// ENTRY POINT
// ============================================================================

#[no_mangle]  // <-- POPRAWIONO: usunięto "unsafe"
pub extern "C" fn _start() -> ! {
    main();
    exit(0);
}

// ============================================================================
// MAIN
// ============================================================================

fn main() {
    println("==================================");
    println("  CosinusOS Userspace Ready!");
    println("==================================");
    println("");
    
    let mut vec = Vec::new();
    vec.push(1);
    vec.push(2);
    vec.push(3);
    print("Vec: ");
    for v in &vec {
        print_fmt!("{} ", v);
    }
    println("");
    
    let mut s = String::from("Hello ");
    s.push_str("from Rust!");
    println(&s);
    
    let mut map = HashMap::new();
    map.insert("key1", 42);
    map.insert("key2", 100);
    if let Some(val) = map.get(&"key1") {
        println_fmt!("key1 = {}", val);
    }
    
    println("");
    println("Creating framebuffer...");
    let mut fb = Framebuffer::new(800, 600);
    fb.clear(Color::BLACK);
    fb.draw_rect(100, 100, 200, 150, Color::RED);
    fb.draw_line(0, 0, 799, 599, Color::WHITE);
    println("Framebuffer ready!");
    
    println("");
    println("Creating terminal...");
    let mut term = Terminal::new(80, 25);
    term.write_str("Welcome to CosinusOS!\n");
    term.write_str("Terminal ready.\n");
    println("Terminal ready!");
    
    println("");
    println("==================================");
    println("  All systems operational!");
    println("==================================");
    println("");
    println("System is idle. Press Ctrl+Alt+Del to reboot.");
    
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}