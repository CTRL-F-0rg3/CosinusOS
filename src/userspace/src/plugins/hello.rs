// userspace — plugins/hello.rs
// Demo plugin: rysuje panel z licznikiem ticków, rejestruje komendę "hello".

use crate::plugin::api::{DrawCtx, PluginFlags};
use crate::tui::PanelPainter;
use libcosinus::fmt::FmtBuf;

// Stan pluginu — statyczny, bo plugin jest singletonem w procesie
static mut TICK_COUNT: u64  = 0;
static mut LAST_MSG:   [u8; 32] = [0; 32];
static mut LAST_MSG_LEN: usize  = 0;

fn init(_id: u8, ctx: &DrawCtx) {
    let mut p = PanelPainter::new(ctx);
    p.frame("hello");
    p.println("Initialized.");
}

fn tick(_id: u8) {
    unsafe { TICK_COUNT += 1; }
}

fn draw(_id: u8, ctx: &DrawCtx) {
    let mut p = PanelPainter::new(ctx);
    p.frame("hello");
    p.clear_inner();

    let mut fb = FmtBuf::<48>::new();
    fb.push_str("ticks: ").push_u64(unsafe { TICK_COUNT });
    p.println(fb.as_str());

    if unsafe { LAST_MSG_LEN } > 0 {
        let msg = unsafe {
            core::str::from_utf8(&LAST_MSG[..LAST_MSG_LEN]).unwrap_or("")
        };
        let mut line = FmtBuf::<48>::new();
        line.push_str("msg: ").push_str(msg);
        p.println(line.as_str());
    }

    p.println("cmd: 'hello <text>'");
}

fn on_cmd(_id: u8, _cmd_idx: u8, args: &str) {
    let b = args.as_bytes();
    let len = b.len().min(32);
    unsafe {
        LAST_MSG[..len].copy_from_slice(&b[..len]);
        LAST_MSG_LEN = len;
    }
}

define_plugin! {
    name:    "hello",
    version: (0, 1, 0),
    flags:   PluginFlags::HAS_PANEL | PluginFlags::HAS_CMDS | PluginFlags::AUTOSTART,
    cmds:    [("hello", "print a message in the hello panel")],
    init:    init,
    tick:    tick,
    draw:    draw,
    on_cmd:  on_cmd,
}
