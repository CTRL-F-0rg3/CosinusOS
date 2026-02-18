#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use core::fmt::{self, Write};
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;

// ============================================================================
// SYSCALL INTERFACE
// ============================================================================

#[repr(usize)]
#[derive(Copy, Clone)]
pub enum Syscall {
    Exit  = 0,
    Write = 1,
    Read  = 2,
}

#[inline(always)]
pub unsafe fn syscall0(num: Syscall) -> usize {
    let ret: usize;
    core::arch::asm!(
        "int 0x80",
        in("rax") num as usize,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall1(num: Syscall, arg1: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "int 0x80",
        in("rax") num as usize,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "int 0x80",
        in("rax") num as usize,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

// ============================================================================
// STDIO
// ============================================================================

pub fn print(s: &str) {
    unsafe { syscall3(Syscall::Write, 1, s.as_ptr() as usize, s.len()); }
}

pub fn println(s: &str) {
    print(s);
    print("\n");
}

pub fn exit(code: i32) -> ! {
    unsafe { syscall1(Syscall::Exit, code as usize); }
    loop { unsafe { core::arch::asm!("hlt"); } }
}

// ============================================================================
// ASM HOT-PATH: fast_memcpy / fast_memset / fast_strlen / fast_hash
// Wydzielone do ASM dla czystej wydajności — unikamy pośrednich wywołań rustc.
// ============================================================================

/// Szybkie memcpy oparte na `rep movsq` + tail bytes.
/// Bezpieczne tylko gdy src i dst nie zachodzą na siebie (jak standardowe memcpy).
#[inline(always)]
pub unsafe fn fast_memcpy(dst: *mut u8, src: *const u8, n: usize) {
    core::arch::asm!(
        // Kopiuj qwordami (8 bajtów naraz)
        "mov rcx, {n}",
        "shr rcx, 3",
        "rep movsq",
        // Kopiuj pozostałe bajty (n % 8)
        "mov rcx, {n}",
        "and rcx, 7",
        "rep movsb",
        n   = in(reg) n,
        in("rdi") dst,
        in("rsi") src,
        out("rcx") _,
        options(nostack)
    );
}

/// Szybkie memset oparte na `rep stosq` + tail bytes.
#[inline(always)]
pub unsafe fn fast_memset(dst: *mut u8, val: u8, n: usize) {
    // Rozszerz bajt na 8 bajtów (broadcast)
    let wide: u64 = (val as u64) * 0x0101010101010101u64;
    core::arch::asm!(
        "mov rcx, {n}",
        "shr rcx, 3",
        "rep stosq",
        "mov rcx, {n}",
        "and rcx, 7",
        // stosb używa AL, a rax = wide, więc AL = val — OK
        "rep stosb",
        n   = in(reg) n,
        in("rdi") dst,
        in("rax") wide,
        out("rcx") _,
        options(nostack)
    );
}

/// Szybkie strlen przez `repne scasb`.
#[inline(always)]
pub unsafe fn fast_strlen(s: *const u8) -> usize {
    let mut len: usize;
    core::arch::asm!(
        "xor al, al",          // szukaj bajtu 0
        "mov rcx, 0xFFFFFFFF", // max długość
        "repne scasb",
        "not rcx",
        "dec rcx",             // rcx = długość bez \0
        in("rdi") s,
        out("rcx") len,
        out("al") _,
        options(nostack)
    );
    len
}

/// Oblicza hash FNV-1a 64-bit w ASM — szybszy niż Rust na krótkich kluczach.
/// Zwraca hash 64-bitowy.
#[inline(always)]
pub unsafe fn fnv1a_hash_asm(data: *const u8, len: usize) -> u64 {
    let mut hash: u64;
    core::arch::asm!(
        // FNV offset basis
        "mov {h}, 0xcbf29ce484222325",
        "test {n}, {n}",
        "jz 2f",
        "1:",
        "movzx eax, byte ptr [{ptr}]",
        "xor {h}, rax",                    // hash ^= byte
        "mov rax, 0x100000001b3",
        "imul {h}, rax",                   // hash *= FNV prime
        "inc {ptr}",
        "dec {n}",
        "jnz 1b",
        "2:",
        h   = out(reg) hash,
        ptr = inout(reg) data => _,
        n   = inout(reg) len  => _,
        out("rax") _,
        options(nostack)
    );
    hash
}

// ============================================================================
// MEMORY ALLOCATOR — thread-safe bump allocator ze spinlockiem
// ============================================================================

const HEAP_SIZE: usize = 10 * 1024 * 1024; // 10 MiB

// Wyrównanie do strony (4 KiB) przez #[repr(align)]
#[repr(align(4096))]
struct AlignedHeap([u8; HEAP_SIZE]);
static mut HEAP: AlignedHeap = AlignedHeap([0; HEAP_SIZE]);

// AtomicUsize jako wskaźnik na szczyt stosu — CAS zamiast mutex
static HEAP_POS: AtomicUsize = AtomicUsize::new(0);

pub struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size  = layout.size();
        let align = layout.align();

        // CAS loop — lock-free bump allocation
        loop {
            let pos = HEAP_POS.load(Ordering::Acquire);
            // Wyrównaj `pos` do `align` (potęga 2)
            let aligned = (pos + align - 1) & !(align - 1);
            let new_pos = aligned + size;

            if new_pos > HEAP_SIZE {
                return core::ptr::null_mut(); // OOM
            }

            // Jeśli HEAP_POS wciąż = pos, zarezerwuj [aligned, new_pos)
            match HEAP_POS.compare_exchange(pos, new_pos, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_)  => return HEAP.0.as_mut_ptr().add(aligned),
                Err(_) => continue, // ktoś wziął ten fragment — spróbuj ponownie
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator nie zwalnia — aby dodać zwalnianie, zastąp free-list allocatorem
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

// ============================================================================
// PANIC HANDLER
// ============================================================================

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println("PANIC:");
    if let Some(location) = info.location() {
        print("  File: ");
        print(location.file());
        print("\n");
    }
    exit(1);
}

// ============================================================================
// FORMATTING
// ============================================================================

pub struct Writer;

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        print(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! print_fmt {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let mut writer = $crate::Writer;
        let _ = write!(&mut writer, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println_fmt {
    () => { $crate::print("\n") };
    ($($arg:tt)*) => {{
        $crate::print_fmt!($($arg)*);
        $crate::print("\n");
    }};
}

// ============================================================================
// SYNCHRONIZATION — SpinLock (prawdziwa atomiczna blokada)
// ============================================================================

/// Prawdziwy spinlock przez `lock xchg` (test-and-set).
/// W jądrze bez scheduler-a to jedyna opcja; na SMP działa poprawnie.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data:   core::cell::UnsafeCell<T>,
}

// BEZPIECZEŃSTWO: dostęp tylko przez SpinLockGuard, który wymaga zdobycia blokady
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data:   core::cell::UnsafeCell::new(data),
        }
    }

    /// Blokuje aż do zdobycia locka. Używa `pause` (rep nop) między próbami
    /// aby zmniejszyć zużycie magistrali na HT/SMP.
    pub fn lock(&self) -> SpinLockGuard<T> {
        loop {
            // Szybka ścieżka: spróbuj zdobyć bez pełnego xchg
            if !self.locked.load(Ordering::Relaxed) {
                // Pełny test-and-set z acquire semantics
                if self.locked.compare_exchange(
                    false, true,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ).is_ok() {
                    return SpinLockGuard { lock: self };
                }
            }
            // PAUSE — wskazówka dla CPU że jesteśmy w spin-loop
            // Redukuje zużycie energii i presję na pamięć podręczną L1
            unsafe { core::arch::asm!("pause", options(nostack, nomem)); }
        }
    }

    /// Próba zdobycia locka bez blokowania. Zwraca None jeśli zajęty.
    pub fn try_lock(&self) -> Option<SpinLockGuard<T>> {
        if self.locked.compare_exchange(
            false, true,
            Ordering::Acquire,
            Ordering::Relaxed,
        ).is_ok() {
            Some(SpinLockGuard { lock: self })
        } else {
            None
        }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<'a, T> core::ops::Deref for SpinLockGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T { unsafe { &*self.lock.data.get() } }
}

impl<'a, T> core::ops::DerefMut for SpinLockGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T { unsafe { &mut *self.lock.data.get() } }
}

impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        // Release: wszystkie wcześniejsze zapisy muszą być widoczne zanim zwolnimy lock
        self.lock.locked.store(false, Ordering::Release);
    }
}

// Stary alias Mutex → SpinLock dla zgodności
pub type Mutex<T> = SpinLock<T>;

// ============================================================================
// COLLECTIONS — HashMap z FNV-1a i oddzielnym łańcuchowaniem
// ============================================================================

/// Trait do haszowania kluczy — implementowany przez typy kluczy.
pub trait Hash {
    fn hash(&self) -> u64;
}

impl Hash for &str {
    fn hash(&self) -> u64 {
        unsafe { fnv1a_hash_asm(self.as_ptr(), self.len()) }
    }
}

impl Hash for u64 {
    fn hash(&self) -> u64 {
        // Finalizer Murmur3 — rozróżnia bliskie klucze numeryczne
        let mut x = *self;
        x ^= x >> 33;
        x  = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^= x >> 33;
        x  = x.wrapping_mul(0xc4ceb9fe1a85ec53);
        x ^= x >> 33;
        x
    }
}

impl Hash for u32 {
    fn hash(&self) -> u64 { (*self as u64).hash() }
}

impl Hash for i32 {
    fn hash(&self) -> u64 { (*self as u64).hash() }
}

impl Hash for usize {
    fn hash(&self) -> u64 { (*self as u64).hash() }
}

impl Hash for String {
    fn hash(&self) -> u64 {
        unsafe { fnv1a_hash_asm(self.as_ptr(), self.len()) }
    }
}

/// HashMap z otwartym adresowaniem (quadratic probing) i obciążeniem < 0.75.
/// Klucze muszą implementować `Hash + PartialEq + Clone`.
pub struct HashMap<K, V> {
    // Każdy slot: None = wolny, Some((key, value, tombstone))
    // tombstone = true → usunięty (dla poprawnego probing)
    slots: Vec<Option<(K, V, bool)>>,
    len:   usize,
    cap:   usize, // zawsze potęga 2
}

impl<K: Hash + PartialEq + Clone, V> HashMap<K, V> {
    pub fn new() -> Self {
        Self::with_capacity(16)
    }

    pub fn with_capacity(cap: usize) -> Self {
        // Zaokrąglij w górę do potęgi 2
        let cap = cap.next_power_of_two();
        let mut slots = Vec::with_capacity(cap);
        for _ in 0..cap { slots.push(None); }
        Self { slots, len: 0, cap }
    }

    #[inline]
    fn probe(&self, key: &K) -> usize {
        let h = key.hash();
        // maska = cap - 1 (cap jest potęgą 2)
        (h as usize) & (self.cap - 1)
    }

    pub fn insert(&mut self, key: K, value: V) {
        // Przy obciążeniu >= 75% rozszerz
        if self.len * 4 >= self.cap * 3 {
            self.resize();
        }

        let start = self.probe(&key);
        let mask  = self.cap - 1;
        let mut i = start;
        let mut j = 0usize; // krok kwadratowy

        loop {
            match &self.slots[i] {
                None => {
                    self.slots[i] = Some((key, value, false));
                    self.len += 1;
                    return;
                }
                Some((k, _, true)) if *k == key => {
                    // Nadpisz tombstone dla tego samego klucza
                    self.slots[i] = Some((key, value, false));
                    self.len += 1;
                    return;
                }
                Some((_, _, true)) => {
                    // Tombstone innego klucza — możemy tu wstawić
                    self.slots[i] = Some((key, value, false));
                    self.len += 1;
                    return;
                }
                Some((k, _, false)) if *k == key => {
                    // Aktualizacja istniejącego klucza
                    self.slots[i] = Some((key, value, false));
                    return;
                }
                _ => {}
            }
            // Quadratic probing: i = (start + j + j²) % cap
            j += 1;
            i  = (start + j + j * j) & mask;
            // Zabezpieczenie — w praktyce nie osiągalne przy load < 0.75
            if j > self.cap { break; }
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let start = self.probe(key);
        let mask  = self.cap - 1;
        let mut i = start;
        let mut j = 0usize;

        loop {
            match &self.slots[i] {
                None => return None, // pusta komórka = koniec łańcucha
                Some((k, v, false)) if k == key => return Some(v),
                Some(_) => {}
            }
            j += 1;
            i  = (start + j + j * j) & mask;
            if j > self.cap { break; }
        }
        None
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let start = self.probe(key);
        let mask  = self.cap - 1;
        let mut i = start;
        let mut j = 0usize;

        loop {
            match &self.slots[i] {
                None => return None,
                Some((k, _, false)) if k == key => {
                    return self.slots[i].as_mut().map(|(_, v, _)| v);
                }
                Some(_) => {}
            }
            j += 1;
            i  = (start + j + j * j) & mask;
            if j > self.cap { break; }
        }
        None
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let start = self.probe(key);
        let mask  = self.cap - 1;
        let mut i = start;
        let mut j = 0usize;

        loop {
            match &self.slots[i] {
                None => return None,
                Some((k, _, false)) if k == key => {
                    if let Some((_, v, ref mut tomb)) = self.slots[i] {
                        *tomb = true; // oznacz jako tombstone
                        self.len -= 1;
                        // BEZPIECZEŃSTWO: bierzemy wartość ze slotu
                        let (_, val, _) = self.slots[i].take().unwrap();
                        return Some(val);
                    }
                }
                Some(_) => {}
            }
            j += 1;
            i  = (start + j + j * j) & mask;
            if j > self.cap { break; }
        }
        None
    }

    pub fn len(&self)      -> usize { self.len }
    pub fn is_empty(&self) -> bool  { self.len == 0 }

    fn resize(&mut self) {
        let new_cap = self.cap * 2;
        let mut new_map = Self::with_capacity(new_cap);
        for slot in self.slots.drain(..) {
            if let Some((k, v, false)) = slot {
                new_map.insert(k, v);
            }
        }
        *self = new_map;
    }
}

// ============================================================================
// RANDOM — xorshift64 (znacznie lepszy niż LCG)
// ============================================================================

// Chroniony SpinLockiem dla bezpieczeństwa wielowątkowego
static RNG_STATE: SpinLock<u64> = SpinLock::new(0x853c49e6748fea9b); // seed niezerowy

pub fn random() -> u64 {
    let mut state = RNG_STATE.lock();
    let mut x = *state;
    // xorshift64 — okres 2⁶⁴ - 1
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

pub fn random_range(min: u64, max: u64) -> u64 {
    debug_assert!(max > min);
    min + (random() % (max - min + 1))
}

pub fn random_u32() -> u32 { random() as u32 }

// ============================================================================
// MATH
// ============================================================================

pub mod math {
    #[inline(always)]
    pub fn min<T: PartialOrd>(a: T, b: T) -> T { if a < b { a } else { b } }

    #[inline(always)]
    pub fn max<T: PartialOrd>(a: T, b: T) -> T { if a > b { a } else { b } }

    #[inline(always)]
    pub fn abs(x: i32) -> i32 { if x < 0 { -x } else { x } }

    /// Clamp — ogranicz wartość do przedziału [lo, hi]
    #[inline(always)]
    pub fn clamp<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
        if v < lo { lo } else if v > hi { hi } else { v }
    }

    /// Liczba bitów ustawionych w u64 (POPCNT przez ASM)
    #[inline(always)]
    pub fn popcount(x: u64) -> u32 {
        let r: u64;
        unsafe {
            core::arch::asm!(
                "popcnt {r}, {x}",
                r = out(reg) r,
                x = in(reg) x,
                options(nostack, nomem)
            );
        }
        r as u32
    }

    /// Wiodące zera (BSR) przez ASM — zwraca 64 dla x=0
    #[inline(always)]
    pub fn leading_zeros(x: u64) -> u32 {
        if x == 0 { return 64; }
        let r: u64;
        unsafe {
            core::arch::asm!(
                "bsr {r}, {x}",
                r = out(reg) r,
                x = in(reg) x,
                options(nostack, nomem)
            );
        }
        63 - r as u32
    }
}

// ============================================================================
// GRAPHICS
// ============================================================================

#[derive(Copy, Clone)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb (r: u8, g: u8, b: u8)         -> Self { Self { r, g, b, a: 255 } }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8)  -> Self { Self { r, g, b, a } }

    #[inline(always)]
    pub fn to_u32(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) |
        ((self.g as u32) <<  8) | (self.b as u32)
    }

    pub const BLACK:   Color = Color::rgb(0,   0,   0);
    pub const WHITE:   Color = Color::rgb(255, 255, 255);
    pub const RED:     Color = Color::rgb(255, 0,   0);
    pub const GREEN:   Color = Color::rgb(0,   255, 0);
    pub const BLUE:    Color = Color::rgb(0,   0,   255);
    pub const YELLOW:  Color = Color::rgb(255, 255, 0);
    pub const CYAN:    Color = Color::rgb(0,   255, 255);
    pub const MAGENTA: Color = Color::rgb(255, 0,   255);
}

pub struct Framebuffer {
    pub width:  usize,
    pub height: usize,
    pub buffer: Vec<u32>,
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize(width * height, 0);
        Self { width, height, buffer }
    }

    #[inline(always)]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x < self.width && y < self.height {
            self.buffer[y * self.width + x] = color.to_u32();
        }
    }

    /// Czyści bufor używając fast_memset (broadcast przez rep stosq)
    pub fn clear(&mut self, color: Color) {
        let pixel = color.to_u32();
        // fast_memset działa na bajtach — użyj fill bezpośrednio na u32
        // (kompilator i tak wygeneruje SIMD/rep dla fill)
        self.buffer.fill(pixel);
    }

    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        let pixel = color.to_u32();
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);

        for row in y..y_end {
            let base = row * self.width;
            // Wypełnij wiersz przez slice::fill — kompilator wygeneruje memset-like kod
            if let Some(slice) = self.buffer.get_mut(base + x..base + x_end) {
                slice.fill(pixel);
            }
        }
    }

    /// Bresenham line algorithm
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let dx =  (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1i32 } else { -1 };
        let sy = if y0 < y1 { 1i32 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);

        loop {
            if x >= 0 && y >= 0 { self.set_pixel(x as usize, y as usize, color); }
            if x == x1 && y == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy { err += dy; x += sx; }
            if e2 <= dx { err += dx; y += sy; }
        }
    }

    /// Wypełniony okrąg (algorytm Bresenhama)
    pub fn draw_circle(&mut self, cx: i32, cy: i32, r: i32, color: Color) {
        let mut x = 0i32;
        let mut y = r;
        let mut d = 3 - 2 * r;

        while x <= y {
            // Wypełnij 8 symetrycznych ćwiartek
            for (px, py) in [
                (cx - x, cy - y), (cx + x, cy - y),
                (cx - x, cy + y), (cx + x, cy + y),
                (cx - y, cy - x), (cx + y, cy - x),
                (cx - y, cy + x), (cx + y, cy + x),
            ] {
                if px >= 0 && py >= 0 {
                    self.set_pixel(px as usize, py as usize, color);
                }
            }
            if d < 0 { d += 4 * x + 6; }
            else      { d += 4 * (x - y) + 10; y -= 1; }
            x += 1;
        }
    }
}

// ============================================================================
// GUI
// ============================================================================

pub struct Rect {
    pub x:      i32,
    pub y:      i32,
    pub width:  u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    #[inline(always)]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width as i32 &&
        y >= self.y && y < self.y + self.height as i32
    }
}

pub trait Widget {
    fn bounds(&self)                         -> Rect;
    fn draw(&self, fb: &mut Framebuffer);
    fn handle_click(&mut self, x: i32, y: i32) -> bool;
}

pub struct Button {
    pub rect:     Rect,
    pub label:    String,
    pub bg_color: Color,
    pub on_click: Option<Box<dyn FnMut()>>,
}

impl Button {
    pub fn new(rect: Rect, label: &str) -> Self {
        Self {
            rect,
            label:    String::from(label),
            bg_color: Color::rgb(100, 100, 200),
            on_click: None,
        }
    }

    pub fn with_callback<F: FnMut() + 'static>(mut self, callback: F) -> Self {
        self.on_click = Some(Box::new(callback));
        self
    }
}

impl Widget for Button {
    fn bounds(&self) -> Rect {
        Rect::new(self.rect.x, self.rect.y, self.rect.width, self.rect.height)
    }

    fn draw(&self, fb: &mut Framebuffer) {
        fb.draw_rect(
            self.rect.x as usize, self.rect.y as usize,
            self.rect.width as usize, self.rect.height as usize,
            self.bg_color,
        );
    }

    fn handle_click(&mut self, x: i32, y: i32) -> bool {
        if self.rect.contains(x, y) {
            if let Some(ref mut cb) = self.on_click { cb(); }
            true
        } else {
            false
        }
    }
}

pub struct Window {
    pub title:   String,
    pub rect:    Rect,
    pub widgets: Vec<Box<dyn Widget>>,
}

impl Window {
    pub fn new(title: &str, rect: Rect) -> Self {
        Self { title: String::from(title), rect, widgets: Vec::new() }
    }

    pub fn add_widget(&mut self, widget: Box<dyn Widget>) {
        self.widgets.push(widget);
    }

    pub fn draw(&self, fb: &mut Framebuffer) {
        fb.draw_rect(
            self.rect.x as usize, self.rect.y as usize,
            self.rect.width as usize, self.rect.height as usize,
            Color::rgb(200, 200, 200),
        );
        fb.draw_rect(
            self.rect.x as usize, self.rect.y as usize,
            self.rect.width as usize, 30,
            Color::rgb(50, 50, 150),
        );
        for w in &self.widgets { w.draw(fb); }
    }

    pub fn handle_click(&mut self, x: i32, y: i32) {
        for w in &mut self.widgets {
            if w.handle_click(x, y) { break; }
        }
    }
}

// ============================================================================
// TERMINAL
// ============================================================================

pub struct Terminal {
    pub width:    usize,
    pub height:   usize,
    pub buffer:   Vec<Vec<char>>,
    pub cursor_x: usize,
    pub cursor_y: usize,
}

impl Terminal {
    pub fn new(width: usize, height: usize) -> Self {
        let buffer = (0..height)
            .map(|_| vec![' '; width])
            .collect();
        Self { width, height, buffer, cursor_x: 0, cursor_y: 0 }
    }

    pub fn write_char(&mut self, c: char) {
        match c {
            '\n' => { self.cursor_x = 0; self.cursor_y += 1; }
            '\r' => { self.cursor_x = 0; }
            _ => {
                if self.cursor_x < self.width {
                    self.buffer[self.cursor_y][self.cursor_x] = c;
                    self.cursor_x += 1;
                }
            }
        }
        if self.cursor_x >= self.width { self.cursor_x = 0; self.cursor_y += 1; }
        if self.cursor_y >= self.height { self.scroll(); }
    }

    pub fn write_str(&mut self, s: &str) {
        for c in s.chars() { self.write_char(c); }
    }

    fn scroll(&mut self) {
        self.buffer.rotate_left(1);
        let w = self.width;
        self.buffer[self.height - 1].fill(' ');
        self.cursor_y = self.height - 1;
    }

    pub fn clear(&mut self) {
        for row in &mut self.buffer { row.fill(' '); }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }
}

// ============================================================================
// DRIVER INTERFACE
// ============================================================================

pub trait Driver {
    fn name(&self)                            -> &str;
    fn init(&mut self)                        -> Result<(), ()>;
    fn read(&mut self, buf: &mut [u8])        -> Result<usize, ()>;
    fn write(&mut self, buf: &[u8])           -> Result<usize, ()>;
}

pub struct DriverManager {
    drivers: Vec<Box<dyn Driver>>,
}

impl DriverManager {
    pub fn new() -> Self { Self { drivers: Vec::new() } }

    pub fn register(&mut self, driver: Box<dyn Driver>) {
        self.drivers.push(driver);
    }

    pub fn init_all(&mut self) {
        for driver in &mut self.drivers {
            match driver.init() {
                Ok(_)  => { print("Driver "); print(driver.name()); println(" initialized"); }
                Err(_) => { print("Driver "); print(driver.name()); println(" failed!");      }
            }
        }
    }
}

// ============================================================================
// ENTRY POINT
// ============================================================================

#[no_mangle]
pub extern "C" fn _start() -> ! {
    main();
    exit(0);
}

// ============================================================================
// MAIN — demo
// ============================================================================

fn main() {
    println("==================================");
    println("  CosinusOS Userspace v2");
    println("==================================");
    println("");

    // --- Vec ---
    let mut vec: Vec<i32> = Vec::new();
    for i in 1..=5 { vec.push(i); }
    print("Vec: ");
    for v in &vec { print_fmt!("{} ", v); }
    println("");

    // --- String ---
    let mut s = String::from("Hello ");
    s.push_str("from CosinusOS!");
    println(&s);

    // --- HashMap (FNV-1a + quadratic probing) ---
    let mut map: HashMap<&str, i32> = HashMap::new();
    map.insert("alpha",   1);
    map.insert("beta",    2);
    map.insert("gamma",   3);
    map.insert("beta",    99); // aktualizacja
    if let Some(v) = map.get(&"beta")  { println_fmt!("beta  = {}", v); }
    if let Some(v) = map.get(&"gamma") { println_fmt!("gamma = {}", v); }
    println_fmt!("map.len = {}", map.len());

    // --- SpinLock ---
    let counter: SpinLock<u64> = SpinLock::new(0);
    for _ in 0..100 { *counter.lock() += 1; }
    println_fmt!("counter = {}", *counter.lock());

    // --- xorshift64 RNG ---
    print("Random: ");
    for _ in 0..5 { print_fmt!("{} ", random_u32()); }
    println("");

    // --- ASM math ---
    println_fmt!("popcount(0xFF) = {}", math::popcount(0xFF));
    println_fmt!("leading_zeros(1) = {}", math::leading_zeros(1));

    // --- Framebuffer ---
    println("\nCreating framebuffer (800x600)...");
    let mut fb = Framebuffer::new(800, 600);
    fb.clear(Color::BLACK);
    fb.draw_rect(10,  10,  200, 100, Color::RED);
    fb.draw_rect(220, 10,  200, 100, Color::GREEN);
    fb.draw_rect(430, 10,  200, 100, Color::BLUE);
    fb.draw_line(0, 0, 799, 599, Color::WHITE);
    fb.draw_circle(400, 300, 80, Color::YELLOW);
    println("Framebuffer ready!");

    // --- Terminal ---
    println("\nCreating terminal (80x25)...");
    let mut term = Terminal::new(80, 25);
    term.write_str("Welcome to CosinusOS v2!\n");
    term.write_str("All systems nominal.\n");
    println("Terminal ready!");

    println("");
    println("==================================");
    println("  All systems operational!");
    println("==================================");

    loop { unsafe { core::arch::asm!("hlt"); } }
}