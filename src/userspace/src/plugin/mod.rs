// userspace — plugin/mod.rs

#[macro_use]
pub mod api;
pub mod registry;

use api::{PluginDescriptor, PluginFlags, DrawCtx, PANEL_W, PANEL_H};
use libcosinus::{IpcMsg, ipc_recv, ipc_poll};

pub const MAX_PLUGINS: usize = 256;
const PANELS_PER_ROW:  usize = 4;

#[derive(Copy, Clone, PartialEq)]
enum SlotState { Empty, Active, Suspended }

struct PluginSlot {
    state: SlotState,
    desc:  Option<&'static PluginDescriptor>,
    ctx:   DrawCtx,
}

impl PluginSlot {
    const fn empty() -> Self {
        Self {
            state: SlotState::Empty,
            desc:  None,
            ctx:   DrawCtx { x: 0, y: 0, w: PANEL_W as u16, h: PANEL_H as u16, plugin_id: 0 },
        }
    }
}

pub struct PluginManager {
    slots:      [PluginSlot; MAX_PLUGINS],
    count:      usize,
    focus:      Option<u8>,
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

    pub fn load(&mut self, desc: &'static PluginDescriptor) -> Option<u8> {
        let id = self.find_empty()?;
        let ctx = self.make_ctx(id);
        self.slots[id] = PluginSlot { state: SlotState::Active, desc: Some(desc), ctx };
        self.count += 1;
        (desc.init)(id as u8, &ctx);
        let mut fb = libcosinus::fmt::FmtBuf::<64>::new();
        fb.push_str("[PM] loaded: ").push_str(desc.meta.name_str());
        libcosinus::debug(fb.as_str());
        Some(id as u8)
    }

    pub fn unload(&mut self, id: u8) -> bool {
        let i = id as usize;
        if i >= MAX_PLUGINS || self.slots[i].state == SlotState::Empty { return false; }
        if let Some(desc) = self.slots[i].desc {
            if let Some(f) = desc.shutdown { f(id); }
        }
        if self.focus == Some(id) { self.focus = None; }
        self.slots[i] = PluginSlot::empty();
        self.count -= 1;
        true
    }

    pub fn suspend(&mut self, id: u8) {
        if (id as usize) < MAX_PLUGINS { self.slots[id as usize].state = SlotState::Suspended; }
    }

    pub fn resume(&mut self, id: u8) {
        if (id as usize) < MAX_PLUGINS
            && self.slots[id as usize].state == SlotState::Suspended
        {
            self.slots[id as usize].state = SlotState::Active;
        }
    }

    pub fn dispatch_tick(&mut self) {
        self.tick_count += 1;
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state != SlotState::Active { continue; }
            if let Some(desc) = self.slots[i].desc {
                if let Some(f) = desc.tick { f(i as u8); }
            }
        }
    }

    pub fn dispatch_key(&mut self, key: u8) {
        let Some(fid) = self.focus else { return; };
        let i = fid as usize;
        if self.slots[i].state != SlotState::Active { return; }
        if let Some(desc) = self.slots[i].desc {
            if let Some(f) = desc.on_key { f(fid, key); }
        }
    }

    pub fn dispatch_cmd(&mut self, cmd_name: &str, args: &str) -> bool {
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state != SlotState::Active { continue; }
            let Some(desc) = self.slots[i].desc else { continue; };
            if !desc.meta.flags.has(PluginFlags::HAS_CMDS) { continue; }
            for ci in 0..desc.meta.n_cmds {
                if desc.meta.cmds[ci].name_str() == cmd_name {
                    if let Some(f) = desc.on_cmd { f(i as u8, ci as u8, args); return true; }
                }
            }
        }
        false
    }

    pub fn dispatch_draw(&mut self) {
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state != SlotState::Active { continue; }
            let ctx = self.slots[i].ctx;
            if let Some(desc) = self.slots[i].desc {
                if let Some(f) = desc.draw { f(i as u8, &ctx); }
            }
        }
    }

    pub fn dispatch_ipc(&mut self) {
        let pending = ipc_poll();
        for _ in 0..pending {
            let mut msg = IpcMsg::zeroed();
            if ipc_recv(&mut msg).is_err() { break; }
            let pid = ((msg.tag >> 24) & 0xFF) as u8;
            let i = pid as usize;
            if i >= MAX_PLUGINS || self.slots[i].state != SlotState::Active { continue; }
            if let Some(desc) = self.slots[i].desc {
                if let Some(f) = desc.on_ipc { f(pid, &msg); }
            }
        }
    }

    pub fn set_focus(&mut self, id: u8) {
        if (id as usize) < MAX_PLUGINS && self.slots[id as usize].state == SlotState::Active {
            self.focus = Some(id);
        }
    }
    pub fn clear_focus(&mut self)  { self.focus = None; }
    pub fn focused(&self) -> Option<u8> { self.focus }
    pub fn count(&self)   -> usize { self.count }
    pub fn tick_count(&self) -> u64 { self.tick_count }

    pub fn is_active(&self, id: u8) -> bool {
        (id as usize) < MAX_PLUGINS && self.slots[id as usize].state == SlotState::Active
    }

    pub fn active_ids(&self) -> impl Iterator<Item = u8> + '_ {
        (0..MAX_PLUGINS)
            .filter(|&i| self.slots[i].state == SlotState::Active)
            .map(|i| i as u8)
    }

    pub fn find_by_name(&self, name: &str) -> Option<u8> {
        for i in 0..MAX_PLUGINS {
            if self.slots[i].state == SlotState::Empty { continue; }
            if let Some(desc) = self.slots[i].desc {
                if desc.meta.name_str() == name { return Some(i as u8); }
            }
        }
        None
    }

    fn find_empty(&self) -> Option<usize> {
        (0..MAX_PLUGINS).find(|&i| self.slots[i].state == SlotState::Empty)
    }

    fn make_ctx(&self, id: usize) -> DrawCtx {
        let col = id % PANELS_PER_ROW;
        let row = id / PANELS_PER_ROW;
        DrawCtx {
            x: (col * (PANEL_W + 1)) as u16,
            y: (2 + row * (PANEL_H + 1)) as u16,
            w: PANEL_W as u16,
            h: PANEL_H as u16,
            plugin_id: id as u8,
        }
    }
}