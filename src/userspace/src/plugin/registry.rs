// userspace — plugin/registry.rs
// Statyczny rejestr pluginów zebranych z sekcji .cos_plugins.
//
// Każdy plugin umieszcza swój PluginDescriptor w sekcji .cos_plugins
// przez #[link_section = ".cos_plugins"].
// Linker scala je w ciągły array; __cos_plugins_start / __cos_plugins_end
// to symbole linkerowe dające nam zakres.
//
// Linker script musi zawierać:
//
//   .cos_plugins : {
//       __cos_plugins_start = .;
//       KEEP(*(.cos_plugins))
//       __cos_plugins_end = .;
//   }
//
// Jeśli nie masz własnego linker script, zamiast tego używamy
// REGISTRY: statycznej tablicy referencji rejestrowanych ręcznie
// przez register_plugin() — fallback bez magic linker symbols.

use super::api::PluginDescriptor;

// ── Fallback registry (bez linker symbols) ────────────────────────────────────
// Dla każdego projektu który nie ma custom ld script, pluginy rejestrują się
// ręcznie w registry_init() w main.rs.

const MAX_PLUGINS_STATIC: usize = 256;

struct Registry {
    descs: [Option<&'static PluginDescriptor>; MAX_PLUGINS_STATIC],
    count: usize,
}

static mut REGISTRY: Registry = Registry {
    descs: [None; MAX_PLUGINS_STATIC],
    count: 0,
};

/// Zarejestruj descriptor pluginu.
/// Wywoływane z main.rs przed startem managera.
pub fn register(desc: &'static PluginDescriptor) {
    unsafe {
        if REGISTRY.count < MAX_PLUGINS_STATIC {
            REGISTRY.descs[REGISTRY.count] = Some(desc);
            REGISTRY.count += 1;
        }
    }
}

/// Liczba zarejestrowanych pluginów.
pub fn count() -> usize {
    unsafe { REGISTRY.count }
}

/// Dostęp do descriptora po indeksie.
pub fn get(i: usize) -> Option<&'static PluginDescriptor> {
    unsafe {
        if i < REGISTRY.count { REGISTRY.descs[i] } else { None }
    }
}

/// Iterator po wszystkich zarejestrowanych descriptorach.
pub fn iter() -> impl Iterator<Item = &'static PluginDescriptor> {
    (0..count()).filter_map(get)
}
