// CosinusOS Userspace — main.rs
// Entry point + demo main
// Istniejące pliki: files.rs, Terminal.rs (nieruszone)
// Nowe moduły: syscall, alloc_impl, sync, asm_utils, collections, graphics, drivers

#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;

// ── Moduły ────────────────────────────────────────────────────────────────────
mod alloc_impl;   // GlobalAllocator — musi być pierwszy
mod syscall;
mod sync;
mod asm_utils;
mod collections;
mod graphics;
mod drivers;
mod files;
mod terminal;     // Terminal userspace

// ── Re-eksporty dla wygody w main ─────────────────────────────────────────────
use syscall::{print, println, exit};
use sync::SpinLock;
use collections::{HashMap, random_u32};
use graphics::{Color, Framebuffer, Rect, Button, Window};
use asm_utils::math;
use files::*;

// ── Makra formatowania (potrzebują syscall::Writer) ───────────────────────────
macro_rules! print_fmt {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let mut w = syscall::Writer;
        let _ = write!(&mut w, $($arg)*);
    }};
}
macro_rules! println_fmt {
    ()              => { syscall::print("\n") };
    ($($arg:tt)*) => {{ print_fmt!($($arg)*); syscall::print("\n"); }};
}

// ── Panic handler ────────────────────────────────────────────────────────────
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println("PANIC:");
    if let Some(loc) = info.location() {
        print("  File: "); print(loc.file()); print("\n");
    }
    exit(1);
}

// ── Entry point ───────────────────────────────────────────────────────────────
// Kernel wywołuje: extern "C" fn entry(arg: u64) -> !
// arg = 0 na razie (przyszłość: wskaźnik do bloku startowego)
#[no_mangle]
pub unsafe extern "C" fn _start(arg: u64) -> ! { main(arg); exit(0); }

// ── Main ─────────────────────────────────────────────────────────────────────
fn main(_arg: u64) {
    println("==================================");
    println("  CosinusOS Userspace v2");
    println("==================================");
    println("");

    // ── VFS ──────────────────────────────────────────────────────────────────
    file_system();

    file_write_all("!d1;/home/ctrl/desktop/koty/notatka.txt", b"miau").unwrap();
    let _data = file_read_all("!d1;/home/ctrl/desktop/koty/notatka.txt").unwrap();

    // ── Vec + String ─────────────────────────────────────────────────────────
    let mut v: Vec<i32> = Vec::new();
    for i in 1..=5 { v.push(i); }
    print("Vec: ");
    for val in &v { print_fmt!("{} ", val); }
    println("");

    let mut s = String::from("Hello ");
    s.push_str("from CosinusOS!");
    println(&s);

    // ── HashMap ──────────────────────────────────────────────────────────────
    let mut map: HashMap<&str, i32> = HashMap::new();
    map.insert("alpha", 1);
    map.insert("beta",  2);
    map.insert("gamma", 3);
    map.insert("beta", 99);
    if let Some(v) = map.get(&"beta")  { println_fmt!("beta  = {}", v); }
    if let Some(v) = map.get(&"gamma") { println_fmt!("gamma = {}", v); }
    println_fmt!("map.len = {}", map.len());

    // ── SpinLock ─────────────────────────────────────────────────────────────
    let counter: SpinLock<u64> = SpinLock::new(0);
    for _ in 0..100 { *counter.lock() += 1; }
    println_fmt!("counter = {}", *counter.lock());

    // ── Random ───────────────────────────────────────────────────────────────
    print("Random: ");
    for _ in 0..5 { print_fmt!("{} ", random_u32()); }
    println("");

    // ── Math ─────────────────────────────────────────────────────────────────
    println_fmt!("popcount(0xFF)    = {}", math::popcount(0xFF));
    println_fmt!("leading_zeros(1)  = {}", math::leading_zeros(1));

    // ── Framebuffer ──────────────────────────────────────────────────────────
    println("\nCreating framebuffer (800x600)...");
    let mut fb = Framebuffer::new(800, 600);
    fb.clear(Color::BLACK);
    fb.draw_rect(10,  10, 200, 100, Color::RED);
    fb.draw_rect(220, 10, 200, 100, Color::GREEN);
    fb.draw_rect(430, 10, 200, 100, Color::BLUE);
    fb.draw_line(0, 0, 799, 599, Color::WHITE);
    fb.draw_circle(400, 300, 80, Color::YELLOW);
    println("Framebuffer ready!");

    // ── GUI demo ─────────────────────────────────────────────────────────────
    let mut win = Window::new("Test Window", Rect::new(100, 100, 400, 300));
    win.add_widget(Box::new(
        Button::new(Rect::new(120, 200, 80, 30), "Click me")
            .with_callback(|| println("Button clicked!"))
    ));
    win.draw(&mut fb);
    win.handle_click(125, 210);

    println("");
    println("==================================");
    println("  Launching terminal...");
    println("==================================");
    println("");

    // ── Terminal — główna pętla userspace ─────────────────────────────────────
    terminal::terminal_main();
} // coś