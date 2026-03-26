// userspace — plugins/sysinfo.rs

use crate::plugin::api::{DrawCtx, PluginFlags};
use crate::tui::PanelPainter;
use libcosinus::{ticks, uptime_secs, thread_id, fmt::FmtBuf};

fn init(_id: u8, ctx: &DrawCtx) {
    let mut p = PanelPainter::new(ctx);
    p.frame("sysinfo");
    p.println("CosinusOS sysinfo");
}

fn tick(_id: u8) {}

fn draw(_id: u8, ctx: &DrawCtx) {
    let mut p = PanelPainter::new(ctx);
    p.frame("sysinfo");
    p.clear_inner();
    let mut fb = FmtBuf::<48>::new();
    fb.push_str("uptime: ").push_u64(uptime_secs()).push_str("s");
    p.println(fb.as_str()); fb.clear();
    fb.push_str("ticks:  ").push_u64(ticks());
    p.println(fb.as_str()); fb.clear();
    fb.push_str("tid:    ").push_u64(thread_id() as u64);
    p.println(fb.as_str());
    p.println("─────────────────");
    p.println("CosinusOS v3.5");
}

define_plugin! {
    export_as: SYSINFO_PLUGIN,
    name:      "sysinfo",
    version:   (0, 1, 0),
    flags:     PluginFlags::HAS_PANEL | PluginFlags::AUTOSTART,
    cmds:      [],
    init:      init,
    tick:      Some(tick),
    draw:      Some(draw),
    on_key:    None,
    on_cmd:    None,
    on_ipc:    None,
    shutdown:  None,
}