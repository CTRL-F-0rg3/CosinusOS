// CosinusOS userspace — main.rs

#![no_std]
#![no_main]
#![allow(dead_code)]

mod tui;
#[macro_use]
mod plugin;
mod plugins {
    pub mod hello;
    pub mod sysinfo;
}

use libcosinus::{read_stdin, sched_yield, exit, debug};
use plugin::{PluginManager, api::PluginFlags};
use plugin::registry;
use tui::Tui;

static mut MANAGER: PluginManager = PluginManager::new();
static mut TUI:     Tui           = Tui::new();

#[no_mangle]
pub extern "C" fn _start(_arg: u64) -> ! {
    unsafe { main_loop() }
}

unsafe fn main_loop() -> ! {
    TUI.init();
    TUI.set_status("CosinusOS Plugin Manager — boot");
    TUI.draw_footer();

    registry::register(&plugins::hello::HELLO_PLUGIN);
    registry::register(&plugins::sysinfo::SYSINFO_PLUGIN);

    for i in 0..registry::count() {
        if let Some(desc) = registry::get(i) {
            if desc.meta.flags.has(PluginFlags::AUTOSTART) {
                if let Some(id) = MANAGER.load(desc) {
                    let mut fb = libcosinus::fmt::FmtBuf::<64>::new();
                    fb.push_str("loaded #").push_u64(id as u64)
                      .push_str(" ").push_str(desc.meta.name_str());
                    debug(fb.as_str());
                }
            }
        }
    }

    TUI.set_status("Ready. Type a command and press Enter.");
    TUI.draw_footer();
    MANAGER.dispatch_draw();

    let mut tick_acc: u64 = 0;
    let mut draw_acc: u64 = 0;
    let mut key_buf = [0u8; 1];

    loop {
        tick_acc += 1;
        if tick_acc >= 10  { tick_acc = 0; MANAGER.dispatch_tick(); }

        draw_acc += 1;
        if draw_acc >= 50  { draw_acc = 0; MANAGER.dispatch_draw(); TUI.draw_footer(); }

        MANAGER.dispatch_ipc();

        if let Ok(1) = read_stdin(&mut key_buf) { handle_key(key_buf[0]); }

        sched_yield();
    }
}

unsafe fn handle_key(key: u8) {
    match key {
        b'\n' | b'\r' => {
            let len = TUI.input_len();
            if len > 0 {
                // Kopiujemy do lokalnego bufora bo input_clear() czyści źródło
                let mut tmp = [0u8; 128];
                let s = TUI.input_str();
                let b = s.as_bytes();
                let n = b.len().min(128);
                tmp[..n].copy_from_slice(&b[..n]);
                let cmd = core::str::from_utf8(&tmp[..n]).unwrap_or("").trim();
                if !cmd.is_empty() {
                    // Musimy skopiować do osobnego bufora żeby uniknąć aliasingu
                    let mut cbuf = [0u8; 128];
                    cbuf[..cmd.len()].copy_from_slice(cmd.as_bytes());
                    let cmd_owned = core::str::from_utf8(&cbuf[..cmd.len()]).unwrap_or("");
                    execute_command(cmd_owned);
                }
            }
            TUI.input_clear();
            TUI.draw_footer();
        }
        8 | 127 => { TUI.input_backspace(); TUI.draw_footer(); }
        17 => {  // Ctrl+Q
            tui::clear_screen(); tui::cursor_to(0, 0); tui::reset_color();
            libcosinus::print("CosinusOS userspace exit.\n");
            exit(0);
        }
        b'\t' => {
            let next = match MANAGER.focused() {
                None     => MANAGER.active_ids().next(),
                Some(id) => MANAGER.active_ids().skip_while(|&i| i <= id).next()
                    .or_else(|| MANAGER.active_ids().next()),
            };
            if let Some(id) = next {
                MANAGER.set_focus(id);
                let mut fb = libcosinus::fmt::FmtBuf::<48>::new();
                fb.push_str("Focus: plugin #").push_u64(id as u64);
                TUI.set_status(fb.as_str());
                TUI.draw_footer();
            }
        }
        k if k >= 32 && k < 127 => {
            if MANAGER.focused().is_some() {
                MANAGER.dispatch_key(k);
            } else {
                TUI.input_push(k);
                TUI.draw_footer();
            }
        }
        _ => {}
    }
}

unsafe fn execute_command(input: &str) {
    let (cmd, args) = match input.find(' ') {
        Some(i) => (&input[..i], input[i+1..].trim()),
        None    => (input, ""),
    };
    match cmd {
        "help" => TUI.set_status("cmds: help list load unload suspend resume focus unfocus redraw"),
        "list" => {
            let mut fb = libcosinus::fmt::FmtBuf::<128>::new();
            fb.push_str("active:");
            for id in MANAGER.active_ids() { fb.push_str(" #").push_u64(id as u64); }
            TUI.set_status(fb.as_str());
        }
        "load" => {
            let mut found = false;
            for i in 0..registry::count() {
                if let Some(desc) = registry::get(i) {
                    if desc.meta.name_str() == args {
                        match MANAGER.load(desc) {
                            Some(id) => {
                                let mut fb = libcosinus::fmt::FmtBuf::<64>::new();
                                fb.push_str("Loaded ").push_str(args).push_str(" as #").push_u64(id as u64);
                                TUI.set_status(fb.as_str());
                            }
                            None => TUI.set_status("Error: no slots available"),
                        }
                        found = true; break;
                    }
                }
            }
            if !found { TUI.set_status("Error: plugin not in registry"); }
        }
        "unload" => {
            match parse_u8(args) {
                Some(id) if MANAGER.unload(id) => {
                    let mut fb = libcosinus::fmt::FmtBuf::<48>::new();
                    fb.push_str("Unloaded #").push_u64(id as u64);
                    TUI.set_status(fb.as_str());
                    MANAGER.dispatch_draw();
                }
                _ => TUI.set_status("Error: invalid id"),
            }
        }
        "suspend" => { if let Some(id) = parse_u8(args) { MANAGER.suspend(id); TUI.set_status("Suspended."); } }
        "resume"  => { if let Some(id) = parse_u8(args) { MANAGER.resume(id);  TUI.set_status("Resumed.");   } }
        "focus"   => {
            match parse_u8(args) {
                Some(id) => { MANAGER.set_focus(id); TUI.set_status("Focus set."); }
                None => match MANAGER.find_by_name(args) {
                    Some(id) => { MANAGER.set_focus(id); TUI.set_status("Focus set."); }
                    None => TUI.set_status("Error: plugin not found"),
                }
            }
        }
        "unfocus" => { MANAGER.clear_focus(); TUI.set_status("Focus cleared."); }
        "redraw"  => { TUI.init(); MANAGER.dispatch_draw(); }
        other => {
            if !MANAGER.dispatch_cmd(other, args) {
                let mut fb = libcosinus::fmt::FmtBuf::<80>::new();
                fb.push_str("Unknown: ").push_str(other);
                TUI.set_status(fb.as_str());
            }
        }
    }
}

fn parse_u8(s: &str) -> Option<u8> {
    let mut v: u16 = 0;
    if s.is_empty() { return None; }
    for b in s.bytes() {
        if b < b'0' || b > b'9' { return None; }
        v = v * 10 + (b - b'0') as u16;
        if v > 255 { return None; }
    }
    Some(v as u8)
}