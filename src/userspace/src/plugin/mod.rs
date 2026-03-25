// userspace — plugin/mod.rs
// PluginManager: zarządza 256 slotami pluginów.
// Obsługuje hotplug (load/unload w runtime), routing IPC, dispatch klawiszy.

pub mod api;
pub mod registry;

use api::{
    PluginDescriptor, PluginFlags, PluginMsgKind, DrawCtx,
    PANEL_W, PANEL_H,
};
use libcosinus::{IpcMsg, ipc_recv, ipc_poll};

// ── Konfiguracja ─────────────────────────────────────────────────────────────

pub const MAX_PLUGINS: usize = 256;

// Układ paneli: 4 kolumny × N wierszy, każdy panel PANEL_W × PANEL_H
const PANELS_PER_ROW: usize = 4;

// ── PluginSlot ───────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
enum SlotState { Empty, Active, Suspended }

struct PluginSlot {
    state:  SlotState,
    desc:   Option<&'static PluginDescriptor>,
    ctx:    DrawCtx,
    tid:    u32,   // TID wątku pluginu (0 = nie ma własnego wątku)
}

impl PluginSlot {
    const fn empty() -> Self {
        Self {
            state: SlotState::Empty,
            desc:  None,
            ctx:   DrawCtx { x: 0, y: 0, w: PANEL_W as u16, h: PANEL_H as u16, plugin_id: 0 },
            tid:   0,
        }
    }
}

// ── PluginManager ─────────────────────────────────────────────────────────────
pub fn register_plugins(registry: &mut Registry) {
    for desc in plugins::all_plugins().iter() {
        // tu zamieniamy starą linię debug:
        // libcosinus::debug(&plugin_log_name("loaded", desc.meta.name_str()));

        let s: &str = plugin_log_name("loaded", desc.meta.name_str()).as_str();
        libcosinus::debug(s);

        registry.register(desc);
    }
}

pub struct PluginManager {
    slots:      [PluginSlot; MAX_PLUGINS],
    count:      usize,
    focus:      Option<u8>,   // który plugin ma focus klawiatury
    tick_count: u64,
}

impl PluginManager {
    pub const fn new() -> Self {
        Self {
            slots:      [const { PluginSlot::empty() }; MAX_PLUGINS],
            count:      0,
            focus:      None,
            tick_count: 0,
        }
    }

    // ── Hotplug ──────────────────────────────────────────────────────────────

    /// Załaduj plugin z descriptora. Zwraca przydzielone plugin_id lub None.
    pub fn load(&mut self, desc: &'static PluginDescriptor) -> Option<u8> {
        let id = self.find_empty_slot()?;
        let ctx = self.make_ctx(id);

        self.slots[id] = PluginSlot {
            state: SlotState::Active,
            desc:  Some(desc),
            ctx,
            tid:   0,
        };
        self.count += 1;

        // Wywołaj init pluginu
        (desc.init)(id as u8, &ctx);

        libcosinus::debug(&plugin_log_name("loaded", desc.meta.name_str()));
        Some(id as u8)
    }

    /// Odładuj plugin o danym id.
    pub fn unload(&mut self, id: u8) -> bool {
        let i = id as usize;
        if i >= MAX_PLUGINS || self.slots[i].state == SlotState::Empty { return false; }

        let slot = &self.slots[i];
        if let Some(desc) = slot.desc {
            if let Some(shutdown) = desc.shutdown { shutdown(id); }
        }

        if self.focus == Some(id) { self.focus = None; }
        self.slots[i] = PluginSlot::empty();
        self.count -= 1;
        true
    }

    /// Zawieś plugin (nie dostaje ticków ani keyevents, ale zostaje w slocie).
    pub fn suspend(&mut self, id: u8) {
        if (id as usize) < MAX_PLUGINS {
            self.slots[id as usize].state = SlotState::Suspended;
        }
    }

    /// Wznów zawieszone plugin.
    pub fn resume(&mut self, id: u8) {
        if (id as usize) < MAX_PLUGINS
            && self.slots[id as usize].state == SlotState::Suspended
        {
            self.slots[id as usize].state = SlotState::Active;
        }
    }

    // ── Dispatch ─────────────────────────────────────────────────────────────

    /// Wyślij tick do wszystkich aktywnych pluginów.
    pub fn dispatch_tick(&mut self) {
        self.tick_count += 1;
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state != SlotState::Active { continue; }
            if let Some(desc) = self.slots[i].desc {
                if let Some(tick) = desc.tick { tick(i as u8); }
            }
        }
    }

    /// Wyślij zdarzenie klawiszowe do pluginu z focusem.
    pub fn dispatch_key(&mut self, key: u8) {
        let Some(fid) = self.focus else { return; };
        let i = fid as usize;
        if self.slots[i].state != SlotState::Active { return; }
        if let Some(desc) = self.slots[i].desc {
            if let Some(on_key) = desc.on_key { on_key(fid, key); }
        }
    }

    /// Wyślij komendę do pluginu który ją rejestruje.
    pub fn dispatch_cmd(&mut self, cmd_name: &str, args: &str) -> bool {
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state != SlotState::Active { continue; }
            let Some(desc) = self.slots[i].desc else { continue; };
            if !desc.meta.flags.has(PluginFlags::HAS_CMDS) { continue; }
            for ci in 0..desc.meta.n_cmds {
                if desc.meta.cmds[ci].name_str() == cmd_name {
                    if let Some(on_cmd) = desc.on_cmd {
                        on_cmd(i as u8, ci as u8, args);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Zrysuj wszystkie panele aktywnych pluginów (w kolejności slotów).
    pub fn dispatch_draw(&mut self) {
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state != SlotState::Active { continue; }
            let ctx = self.slots[i].ctx;
            if let Some(desc) = self.slots[i].desc {
                if let Some(draw) = desc.draw { draw(i as u8, &ctx); }
            }
        }
    }

    /// Odbierz i zdispatchuj wiadomości IPC skierowane do pluginów.
    /// Każdy plugin może być adresowany bezpośrednio przez TID lub przez plugin_id
    /// zakodowane w polu `tag` IpcMsg (górne 8 bitów = plugin_id).
    pub fn dispatch_ipc(&mut self) {
        let pending = ipc_poll();
        for _ in 0..pending {
            let mut msg = IpcMsg::zeroed();
            if ipc_recv(&mut msg).is_err() { break; }
            let plugin_id = ((msg.tag >> 24) & 0xFF) as u8;
            let i = plugin_id as usize;
            if i >= MAX_PLUGINS || self.slots[i].state != SlotState::Active { continue; }
            if let Some(desc) = self.slots[i].desc {
                if let Some(on_ipc) = desc.on_ipc { on_ipc(plugin_id, &msg); }
            }
        }
    }

    // ── Focus ─────────────────────────────────────────────────────────────────

    pub fn set_focus(&mut self, id: u8) {
        if (id as usize) < MAX_PLUGINS
            && self.slots[id as usize].state == SlotState::Active
        {
            self.focus = Some(id);
        }
    }

    pub fn clear_focus(&mut self) { self.focus = None; }

    pub fn focused(&self) -> Option<u8> { self.focus }

    // ── Pytania o stan ───────────────────────────────────────────────────────

    pub fn count(&self) -> usize { self.count }

    pub fn is_active(&self, id: u8) -> bool {
        (id as usize) < MAX_PLUGINS && self.slots[id as usize].state == SlotState::Active
    }

    pub fn tick_count(&self) -> u64 { self.tick_count }

    /// Lista aktywnych plugin_ids (do iteracji w shell/UI)
    pub fn active_ids(&self) -> impl Iterator<Item = u8> + '_ {
        (0..MAX_PLUGINS)
            .filter(|&i| self.slots[i].state == SlotState::Active)
            .map(|i| i as u8)
    }

    /// Znajdź plugin_id po nazwie
    pub fn find_by_name(&self, name: &str) -> Option<u8> {
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state == SlotState::Empty { continue; }
            if let Some(desc) = self.slots[i].desc {
                if desc.meta.name_str() == name { return Some(i as u8); }
            }
        }
        None
    }

    // ── Wewnętrzne ───────────────────────────────────────────────────────────

    fn find_empty_slot(&self) -> Option<usize> {
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state == SlotState::Empty { return Some(i); }
        }
        None
    }

    fn make_ctx(&self, id: usize) -> DrawCtx {
        // Panele rozłożone w siatce PANELS_PER_ROW kolumn
        let col = id % PANELS_PER_ROW;
        let row = id / PANELS_PER_ROW;
        DrawCtx {
            x: (col * (PANEL_W + 1)) as u16,
            y: (2 + row * (PANEL_H + 1)) as u16,  // y=2: zostawiamy miejsce na header
            w: PANEL_W as u16,
            h: PANEL_H as u16,
            plugin_id: id as u8,
        }
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn plugin_log_name(action: &str, name: &str) -> libcosinus::fmt::FmtBuf<64> {
    let mut fb = libcosinus::fmt::FmtBuf::<64>::new();
    fb.push_str("[PM] ").push_str(action).push_str(": ").push_str(name);
    fb
}
