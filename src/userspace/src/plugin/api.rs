// userspace — plugin/api.rs

use libcosinus::IpcMsg;

pub const PLUGIN_NAME_LEN: usize = 32;
pub const PLUGIN_CMD_LEN:  usize = 16;
pub const MAX_CMDS:        usize = 8;

#[derive(Copy, Clone)]
pub struct PluginMeta {
    pub name:    [u8; PLUGIN_NAME_LEN],
    pub version: u32,
    pub flags:   PluginFlags,
    pub cmds:    [PluginCmd; MAX_CMDS],
    pub n_cmds:  usize,
}

impl PluginMeta {
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(PLUGIN_NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("?")
    }
    pub const fn version(major: u8, minor: u8, patch: u8) -> u32 {
        (major as u32) << 16 | (minor as u32) << 8 | patch as u32
    }
}

#[derive(Copy, Clone, Default)]
pub struct PluginFlags(pub u32);
impl PluginFlags {
    pub const HAS_PANEL: u32 = 1 << 0;
    pub const HAS_FOCUS: u32 = 1 << 1;
    pub const HAS_IPC:   u32 = 1 << 2;
    pub const HAS_CMDS:  u32 = 1 << 3;
    pub const AUTOSTART: u32 = 1 << 4;
    pub fn has(&self, flag: u32) -> bool { self.0 & flag != 0 }
}

#[derive(Copy, Clone)]
pub struct PluginCmd {
    pub name: [u8; PLUGIN_CMD_LEN],
    pub help: [u8; 64],
}
impl PluginCmd {
    pub const fn zeroed() -> Self { Self { name: [0; PLUGIN_CMD_LEN], help: [0; 64] } }
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(PLUGIN_CMD_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

pub const PANEL_W: usize = 40;
pub const PANEL_H: usize = 12;

#[derive(Copy, Clone)]
pub struct DrawCtx {
    pub x: u16, pub y: u16,
    pub w: u16, pub h: u16,
    pub plugin_id: u8,
}

pub type InitFn     = fn(id: u8, ctx: &DrawCtx);
pub type TickFn     = fn(id: u8);
pub type DrawFn     = fn(id: u8, ctx: &DrawCtx);
pub type KeyFn      = fn(id: u8, key: u8);
pub type CmdFn      = fn(id: u8, cmd: u8, args: &str);
pub type IpcFn      = fn(id: u8, msg: &IpcMsg);
pub type ShutdownFn = fn(id: u8);

pub struct PluginDescriptor {
    pub meta:     PluginMeta,
    pub init:     InitFn,
    pub tick:     Option<TickFn>,
    pub draw:     Option<DrawFn>,
    pub on_key:   Option<KeyFn>,
    pub on_cmd:   Option<CmdFn>,
    pub on_ipc:   Option<IpcFn>,
    pub shutdown: Option<ShutdownFn>,
}
unsafe impl Sync for PluginDescriptor {}

// Makro define_plugin!
//
// Pola opcjonalne (tick, draw, on_key, on_cmd, on_ipc, shutdown) są podawane
// bezpośrednio jako Some(fn) lub None — zamiast .or() które nie jest const.
//
// Użycie:
//
//   define_plugin! {
//       export_as: MY_PLUGIN,
//       name: "myplugin", version: (0,1,0), flags: PluginFlags::AUTOSTART,
//       cmds: [("cmd", "opis")],
//       init:     my_init,
//       tick:     Some(my_tick),
//       draw:     Some(my_draw),
//       on_key:   None,
//       on_cmd:   Some(my_cmd),
//       on_ipc:   None,
//       shutdown: None,
//   }

#[macro_export]
macro_rules! define_plugin {
    (
        export_as: $export:ident,
        name:      $name:literal,
        version:   ($maj:expr, $min:expr, $pat:expr),
        flags:     $flags:expr,
        cmds:      [$( ($cmd:literal, $help:literal) ),* $(,)?],
        init:      $init:expr,
        tick:      $tick:expr,
        draw:      $draw:expr,
        on_key:    $key:expr,
        on_cmd:    $cmd_fn:expr,
        on_ipc:    $ipc:expr,
        shutdown:  $shut:expr $(,)?
    ) => {
        #[used]
        pub static $export: $crate::plugin::api::PluginDescriptor = {
            let mut name_arr = [0u8; $crate::plugin::api::PLUGIN_NAME_LEN];
            let nb = $name.as_bytes();
            let mut ni = 0usize;
            while ni < nb.len() && ni < $crate::plugin::api::PLUGIN_NAME_LEN {
                name_arr[ni] = nb[ni]; ni += 1;
            }

            let mut cmds = [$crate::plugin::api::PluginCmd::zeroed();
                            $crate::plugin::api::MAX_CMDS];
            let mut n_cmds = 0usize;
            $({
                let mut cn = [0u8; $crate::plugin::api::PLUGIN_CMD_LEN];
                let cb = $cmd.as_bytes();
                let mut ci = 0usize;
                while ci < cb.len() && ci < $crate::plugin::api::PLUGIN_CMD_LEN {
                    cn[ci] = cb[ci]; ci += 1;
                }
                let mut ch = [0u8; 64];
                let hb = $help.as_bytes();
                let mut hi = 0usize;
                while hi < hb.len() && hi < 64 { ch[hi] = hb[hi]; hi += 1; }
                cmds[n_cmds] = $crate::plugin::api::PluginCmd { name: cn, help: ch };
                n_cmds += 1;
            })*

            $crate::plugin::api::PluginDescriptor {
                meta: $crate::plugin::api::PluginMeta {
                    name:    name_arr,
                    version: $crate::plugin::api::PluginMeta::version($maj, $min, $pat),
                    flags:   $crate::plugin::api::PluginFlags($flags),
                    cmds,
                    n_cmds,
                },
                init:     $init,
                tick:     $tick,
                draw:     $draw,
                on_key:   $key,
                on_cmd:   $cmd_fn,
                on_ipc:   $ipc,
                shutdown: $shut,
            }
        };
    };
}