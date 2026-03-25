// userspace — plugin/api.rs
// Publiczne API pluginów CosinusOS.
//
// Każdy plugin to bin który:
//   1. Definiuje statyczny PluginDescriptor z metadanymi + fn ptr
//   2. Implementuje PluginApi poprzez te fn ptr (vtable bez trait objects)
//   3. Jest kompilowany razem z userspace (jako moduł Rust)
//
// Komunikacja:
//   plugin ↔ manager  — przez PluginMsg (sync, w tej samej przestrzeni)
//   plugin ↔ plugin   — przez IPC (libcosinus::ipc_send/recv, async)
//   plugin ↔ kernel   — przez libcosinus (syscalle)

use libcosinus::IpcMsg;

// ── Metadane pluginu ──────────────────────────────────────────────────────────

pub const PLUGIN_NAME_LEN: usize = 32;
pub const PLUGIN_CMD_LEN:  usize = 16;
pub const MAX_CMDS:        usize = 8;

#[derive(Copy, Clone)]
pub struct PluginMeta {
    pub name:    [u8; PLUGIN_NAME_LEN],
    pub version: u32,           // packed: major<<16 | minor<<8 | patch
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
    pub const HAS_PANEL:  u32 = 1 << 0;  // plugin chce panel TUI
    pub const HAS_FOCUS:  u32 = 1 << 1;  // plugin obsługuje klawisze
    pub const HAS_IPC:    u32 = 1 << 2;  // plugin używa IPC
    pub const HAS_CMDS:   u32 = 1 << 3;  // plugin rejestruje komendy
    pub const AUTOSTART:  u32 = 1 << 4;  // uruchom przy starcie

    pub fn has(&self, flag: u32) -> bool { self.0 & flag != 0 }
}

#[derive(Copy, Clone)]
pub struct PluginCmd {
    pub name: [u8; PLUGIN_CMD_LEN],
    pub help: [u8; 64],
}

impl PluginCmd {
    pub const fn zeroed() -> Self {
        Self { name: [0; PLUGIN_CMD_LEN], help: [0; 64] }
    }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(PLUGIN_CMD_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

// ── Wiadomości między pluginem a managerem ────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
#[repr(u32)]
pub enum PluginMsgKind {
    Nop        = 0,
    // Manager → plugin
    Init       = 1,   // plugin dostał slot, data[0]=plugin_id
    Shutdown   = 2,   // kernel chce zamknąć plugin
    Tick       = 3,   // co 100ms (PIT tick z kernela)
    KeyEvent   = 4,   // data[0]=keycode (tylko jeśli HAS_FOCUS i ma focus)
    DrawReq    = 5,   // plugin powinien narysować swój panel
    CmdExec    = 6,   // data[0]=cmd_idx, ptr→args_str, len=args_len
    // Plugin → manager
    PanelDirty = 16,  // panel się zmienił, proszę o DrawReq
    RequestFocus = 17,
    ReleaseFocus = 18,
    Log        = 19,  // ptr→str, len
    Quit       = 20,  // plugin chce się wyrejestrować
}

impl PluginMsgKind {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1  => Self::Init, 2  => Self::Shutdown, 3  => Self::Tick,
            4  => Self::KeyEvent, 5  => Self::DrawReq, 6  => Self::CmdExec,
            16 => Self::PanelDirty, 17 => Self::RequestFocus,
            18 => Self::ReleaseFocus, 19 => Self::Log,
            20 => Self::Quit, _  => Self::Nop,
        }
    }
}

// ── DrawCtx — kontekst rysowania dla pluginu ──────────────────────────────────

pub const PANEL_W: usize = 40;
pub const PANEL_H: usize = 12;

#[derive(Copy, Clone)]
pub struct DrawCtx {
    pub x: u16,        // pozycja panelu na ekranie (kolumny VGA)
    pub y: u16,        // pozycja panelu na ekranie (wiersze VGA)
    pub w: u16,        // szerokość panelu
    pub h: u16,        // wysokość panelu
    pub plugin_id: u8,
}

// ── PluginDescriptor — statyczna definicja pluginu ────────────────────────────
//
// Każdy plugin definiuje dokładnie jeden statyczny PluginDescriptor.
// Manager zbiera je przez rejestr (plugin/registry.rs).

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

// SAFETY: pluginy są single-threaded w jednym procesie
unsafe impl Sync for PluginDescriptor {}

// ── Helper makro do definicji pluginu ─────────────────────────────────────────
//
// Użycie w src/plugins/myplugin.rs:
//
//   define_plugin! {
//       name:    "myplugin",
//       version: (0, 1, 0),
//       flags:   PluginFlags::HAS_PANEL | PluginFlags::AUTOSTART,
//       cmds:    [("hello", "print greeting")],
//       init:    my_init,
//       tick:    my_tick,
//       draw:    my_draw,
//   }

#[macro_export]
macro_rules! define_plugin {
    (
        name:    $name:literal,
        version: ($maj:expr, $min:expr, $pat:expr),
        flags:   $flags:expr,
        cmds:    [$( ($cmd:literal, $help:literal) ),* $(,)?],
        init:    $init:expr
        $(, tick:     $tick:expr )?
        $(, draw:     $draw:expr )?
        $(, on_key:   $key:expr  )?
        $(, on_cmd:   $cmd_fn:expr )?
        $(, on_ipc:   $ipc:expr  )?
        $(, shutdown: $shut:expr )?
        $(,)?
    ) => {
        #[used]
        #[unsafe(link_section = ".cos_plugins")]
        pub static PLUGIN_DESC: $crate::plugin::api::PluginDescriptor = {
            // build name bytes
            let mut name_arr = [0u8; $crate::plugin::api::PLUGIN_NAME_LEN];
            let b = $name.as_bytes();
            let mut i = 0;
            while i < b.len() && i < $crate::plugin::api::PLUGIN_NAME_LEN {
                name_arr[i] = b[i]; i += 1;
            }

            // build cmds
            let mut cmds = [$crate::plugin::api::PluginCmd::zeroed();
                            $crate::plugin::api::MAX_CMDS];
            let mut n_cmds = 0usize;
            $({
                let mut cn = [0u8; $crate::plugin::api::PLUGIN_CMD_LEN];
                let cb = $cmd.as_bytes();
                let mut ci = 0;
                while ci < cb.len() && ci < $crate::plugin::api::PLUGIN_CMD_LEN { cn[ci]=cb[ci]; ci+=1; }
                let mut ch = [0u8; 64];
                let hb = $help.as_bytes();
                let mut hi = 0;
                while hi < hb.len() && hi < 64 { ch[hi]=hb[hi]; hi+=1; }
                cmds[n_cmds] = $crate::plugin::api::PluginCmd { name: cn, help: ch };
                n_cmds += 1;
            })*

            $crate::plugin::api::PluginDescriptor {
                meta: $crate::plugin::api::PluginMeta {
                    name: name_arr,
                    version: $crate::plugin::api::PluginMeta::version($maj, $min, $pat),
                    flags: $crate::plugin::api::PluginFlags($flags),
                    cmds,
                    n_cmds,
                },
                init: $init,
                tick:     None $( .or(Some($tick))     )?,
                draw:     None $( .or(Some($draw))     )?,
                on_key:   None $( .or(Some($key))      )?,
                on_cmd:   None $( .or(Some($cmd_fn))   )?,
                on_ipc:   None $( .or(Some($ipc))      )?,
                shutdown: None $( .or(Some($shut))     )?,
            }
        };
    };
}
