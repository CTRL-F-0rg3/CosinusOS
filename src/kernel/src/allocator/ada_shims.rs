// CosinusOS — allocator/ada_shims.rs
//
// Rust-side FFI bindings dla wszystkich funkcji Ada.
// Sygnatury dopasowane do skompilowanych .ads (C_Bool = c_int, nie bool).

use core::ffi::c_int;

// ---------------------------------------------------------------------------
// integrity_checks
// ---------------------------------------------------------------------------
extern "C" {
    pub fn ada_write_canary(ptr: *mut u8, size: u64);
    pub fn ada_check_canary(ptr: *const u8, size: u64) -> c_int;
    pub fn ada_register_free(ptr: *const u8);
    pub fn ada_is_double_free(ptr: *const u8) -> c_int;
    pub fn ada_check_bounds(
        ptr:             *const u8,
        size:            u64,
        heap_base:       *const u8,
        heap_size:       u64,
        need_page_align: c_int,
    ) -> c_int;
}

// ---------------------------------------------------------------------------
// audit_log
// ---------------------------------------------------------------------------
extern "C" {
    pub fn ada_audit_alloc(ptr: *mut u8,    size: u64, is_slab: c_int);
    pub fn ada_audit_free (ptr: *const u8,  size: u64, is_slab: c_int);
    pub fn ada_audit_stats(
        total_allocs: *mut u64,
        total_frees:  *mut u64,
        live_bytes:   *mut u64,
    );
    pub fn ada_audit_dump(n: u32);
}

// ---------------------------------------------------------------------------
// lifecycle
// ---------------------------------------------------------------------------
extern "C" {
    pub fn ada_alloc_init        (base: *const u8, size: u64);
    pub fn ada_alloc_reinit      (base: *const u8, size: u64);
    pub fn ada_alloc_shutdown    ();
    pub fn ada_alloc_version     () -> u32;
    pub fn ada_alloc_is_initialized() -> c_int;
    pub fn ada_alloc_heap_range  (base: *mut *const u8, size: *mut u64);
}

// ---------------------------------------------------------------------------
// Safe wrappers
// ---------------------------------------------------------------------------

/// Wywołaj po udanej alokacji. Zwraca false = bounds fail => oddaj null.
#[inline]
pub unsafe fn on_alloc(
    ptr:       *mut u8,
    size:      usize,
    heap_base: *const u8,
    heap_size: usize,
    is_slab:   bool,
) -> bool {
    if ptr.is_null() { return false; }
    let ok = ada_check_bounds(
        ptr as *const u8,
        size as u64,
        heap_base,
        heap_size as u64,
        if is_slab { 0 } else { 1 },
    );
    if ok == 0 { return false; }
    ada_write_canary(ptr, size as u64);
    ada_audit_alloc(ptr, size as u64, if is_slab { 1 } else { 0 });
    true
}

/// Wywołaj na początku free(). Zwraca false = double-free lub corruption.
#[inline]
pub unsafe fn on_free(
    ptr:     *mut u8,
    size:    usize,
    is_slab: bool,
) -> bool {
    if ptr.is_null() { return true; }
    if ada_is_double_free(ptr as *const u8) != 0 { return false; }
    if ada_check_canary(ptr as *const u8, size as u64) == 0 { return false; }
    ada_register_free(ptr as *const u8);
    ada_audit_free(ptr as *const u8, size as u64, if is_slab { 1 } else { 0 });
    true
}

/// Odczyt statystyk.
#[inline]
pub unsafe fn audit_stats() -> (u64, u64, u64) {
    let (mut a, mut f, mut l) = (0u64, 0u64, 0u64);
    ada_audit_stats(&mut a, &mut f, &mut l);
    (a, f, l)
}
