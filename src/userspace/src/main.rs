// CosinusOS userspace — main.rs
// Entry point: reset trybu wyświetlania, init TUI, załaduj pluginy, event loop.

#![no_std]
#![no_main]
#![allow(dead_code)]

#[macro_use] extern crate libcosinus;

mod tui;
mod plugin;
mod plugins {
    pub mod hello;
    pub mod sysinfo;
}

use libcosinus::{read_stdin, sched_yield, exit, debug};
use plugin::{PluginManager, api::PluginFlags};
use plugin::registry;
use tui::Tui;

// ── Globalne stany ────────────────────────────────────────────────────────────

static mut MANAGER: PluginManager = PluginManager::new();
static mut TUI:     Tui           = Tui::new();

// ── Entry ─────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start(_arg: u64) -> ! {
    unsafe { main_loop() }
}

unsafe fn main_loop() -> ! {
    // ── 1. Reset trybu wyświetlania + init TUI ─────────────────────────────
    TUI.init();
    TUI.set_status("CosinusOS Plugin Manager — boot");
    TUI.draw_footer();

    // ── 2. Zarejestruj wbudowane pluginy ───────────────────────────────────
    registry::register(&plugins::hello::PLUGIN_DESC);
    registry::register(&plugins::sysinfo::PLUGIN_DESC);
    // Tutaj dopisujesz kolejne: registry::register(&plugins::myplugin::PLUGIN_DESC);

    // ── 3. Załaduj pluginy z AUTOSTART ─────────────────────────────────────
    for i in 0..registry::count() {
        if let Some(desc) = registry::get(i) {
            if desc.meta.flags.has(PluginFlags::AUTOSTART) {
                if let Some(id) = MANAGER.load(desc) {
                    let mut fb = libcosinus::fmt::FmtBuf::<64>::new();
                    fb.push_str("loaded plugin #").push_u64(id as u64)
                      .push_str(" ").push_str(desc.meta.name_str());
                    debug(fb.as_str());
                }
            }
        }
    }

    TUI.set_status("Ready. Type a command and press Enter.");
    TUI.draw_footer();

    // ── 4. Pierwsze rysowanie paneli ───────────────────────────────────────
    MANAGER.dispatch_draw();

    // ── 5. Event loop ─────────────────────────────────────────────────────
    let mut tick_accum:  u64 = 0;
    let mut draw_accum:  u64 = 0;
    let mut input_buf = [0u8; 1];

    loop {
        // ── Tick (co ~10 iteracji ≈ 100ms przy yield) ──────────────────
        tick_accum += 1;
        if tick_accum >= 10 {
            tick_accum = 0;
            MANAGER.dispatch_tick();
        }

        // ── Redraw (co ~50 iteracji) ────────────────────────────────────
        draw_accum += 1;
        if draw_accum >= 50 {
            draw_accum = 0;
            MANAGER.dispatch_draw();
            TUI.draw_footer();
        }

        // ── IPC ─────────────────────────────────────────────────────────
        MANAGER.dispatch_ipc();

        // ── Input (non-blocking) ────────────────────────────────────────
        match read_stdin(&mut input_buf) {
            Ok(1) => handle_key(input_buf[0]),
            _     => {}
        }

        sched_yield();
    }
}

unsafe fn handle_key(key: u8) {
    match key {
        // Enter — wykonaj komendę
        b'\n' | b'\r' => {
            let input = TUI.input_str().trim();
            if !input.is_empty() {
                execute_command(input);
            }
            TUI.input_clear();
            TUI.draw_footer();
        }

        // Backspace
        8 | 127 => {
            TUI.input_backspace();
            TUI.draw_footer();
        }

        // Ctrl+Q — wyjście
        17 => {
            tui::clear_screen();
            tui::cursor_to(0, 0);
            tui::reset_color();
            libcosinus::print("CosinusOS userspace exit.\n");
            exit(0);
        }

        // Tab — obróć focus między pluginami
        b'\t' => {
            let next = match MANAGER.focused() {
                None     => MANAGER.active_ids().next(),
                Some(id) => MANAGER.active_ids()
                    .skip_while(|&i| i <= id)
                    .next()
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

        // Klawisze przekazane do focusowanego pluginu
        k if k >= 32 && k < 127 => {
            // Najpierw sprawdź czy jest jakiś plugin z focusem
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
    // Format: "cmd [args...]"
    let (cmd, args) = match input.find(' ') {
        Some(i) => (&input[..i], input[i+1..].trim()),
        None    => (input, ""),
    };

    match cmd {
        // ── Wbudowane komendy managera ─────────────────────────────────
        "help" => {
            TUI.set_status("cmds: help list load unload suspend resume focus <plugin_cmd>");
        }

        "list" => {
            // Wypisz aktywne pluginy na panelu statusu (uproszczone)
            let mut fb = libcosinus::fmt::FmtBuf::<128>::new();
            fb.push_str("active:");
            for id in MANAGER.active_ids() {
                fb.push_str(" #").push_u64(id as u64);
            }
            TUI.set_status(fb.as_str());
        }

        "load" => {
            // Załaduj plugin z rejestru po nazwie
            let mut found = false;
            for i in 0..registry::count() {
                if let Some(desc) = registry::get(i) {
                    if desc.meta.name_str() == args {
                        match MANAGER.load(desc) {
                            Some(id) => {
                                let mut fb = libcosinus::fmt::FmtBuf::<64>::new();
                                fb.push_str("Loaded ").push_str(args)
                                  .push_str(" as #").push_u64(id as u64);
                                TUI.set_status(fb.as_str());
                            }
                            None => TUI.set_status("Error: no slots available"),
                        }
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                TUI.set_status("Error: plugin not found in registry");
            }
        }

        "unload" => {
            match args.parse::<u8>() {
                Ok(id) if MANAGER.unload(id) => {
                    let mut fb = libcosinus::fmt::FmtBuf::<48>::new();
                    fb.push_str("Unloaded plugin #").push_u64(id as u64);
                    TUI.set_status(fb.as_str());
                    MANAGER.dispatch_draw();
                }
                _ => TUI.set_status("Error: invalid plugin id"),
            }
        }

        "suspend" => {
            if let Ok(id) = args.parse::<u8>() {
                MANAGER.suspend(id);
                TUI.set_status("Plugin suspended.");
            }
        }

        "resume" => {
            if let Ok(id) = args.parse::<u8>() {
                MANAGER.resume(id);
                TUI.set_status("Plugin resumed.");
            }
        }

        "focus" => {
            match args.parse::<u8>() {
                Ok(id) => {
                    MANAGER.set_focus(id);
                    let mut fb = libcosinus::fmt::FmtBuf::<48>::new();
                    fb.push_str("Focus set to plugin #").push_u64(id as u64);
                    TUI.set_status(fb.as_str());
                }
                Err(_) => {
                    // Szukaj po nazwie
                    if let Some(id) = MANAGER.find_by_name(args) {
                        MANAGER.set_focus(id);
                    } else {
                        TUI.set_status("Error: plugin not found");
                    }
                }
            }
        }

        "unfocus" => {
            MANAGER.clear_focus();
            TUI.set_status("Focus cleared.");
        }

        "redraw" => {
            TUI.init();
            MANAGER.dispatch_draw();
        }

        // ── Deleguj do pluginów ────────────────────────────────────────
        other => {
            if !MANAGER.dispatch_cmd(other, args) {
                let mut fb = libcosinus::fmt::FmtBuf::<80>::new();
                fb.push_str("Unknown command: ").push_str(other);
                TUI.set_status(fb.as_str());
            }
        }
    }
}

// str::parse::<u8>() w no_std — prosta implementacja
trait ParseU8 { fn parse<T: FromDecStr>(&self) -> Result<T, ()>; }
trait FromDecStr: Sized { fn from_dec(s: &str) -> Result<Self, ()>; }

impl FromDecStr for u8 {
    fn from_dec(s: &str) -> Result<Self, ()> {
        let mut v: u16 = 0;
        for b in s.bytes() {
            if b < b'0' || b > b'9' { return Err(()); }
            v = v * 10 + (b - b'0') as u16;
            if v > 255 { return Err(()); }
        }
        Ok(v as u8)
    }
}

impl ParseU8 for str {
    fn parse<T: FromDecStr>(&self) -> Result<T, ()> { T::from_dec(self) }
}
