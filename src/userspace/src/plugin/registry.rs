// userspace — plugin/registry.rs

use super::api::PluginDescriptor;


const MAX_PLUGINS_STATIC: usize = 256;

struct Registry {
    descs: [Option<&'static PluginDescriptor>; MAX_PLUGINS_STATIC],
    count: usize,
}

static mut REGISTRY: Registry = Registry {
    descs: [None; MAX_PLUGINS_STATIC],
    count: 0,
};


pub fn register(desc: &'static PluginDescriptor) {
    unsafe {
        if REGISTRY.count < MAX_PLUGINS_STATIC {
            REGISTRY.descs[REGISTRY.count] = Some(desc);
            REGISTRY.count += 1;
        }
    }
}


pub fn count() -> usize {
    unsafe { REGISTRY.count }
}


pub fn get(i: usize) -> Option<&'static PluginDescriptor> {
    unsafe {
        if i < REGISTRY.count { REGISTRY.descs[i] } else { None }
    }
}


pub fn iter() -> impl Iterator<Item = &'static PluginDescriptor> {
    (0..count()).filter_map(get)
}
