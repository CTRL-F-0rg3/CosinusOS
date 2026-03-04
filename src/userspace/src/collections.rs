// CosinusOS Userspace — collections.rs
// HashMap (quadratic probing) + Hash trait + random (xorshift64)

use alloc::vec::Vec;
use alloc::string::String;
use crate::asm_utils::fnv1a_hash_asm;
use crate::sync::SpinLock;

// ── Hash trait ────────────────────────────────────────────────────────────────

pub trait Hash { fn hash(&self) -> u64; }

impl Hash for &str {
    fn hash(&self) -> u64 { unsafe { fnv1a_hash_asm(self.as_ptr(), self.len()) } }
}
impl Hash for String {
    fn hash(&self) -> u64 { unsafe { fnv1a_hash_asm(self.as_ptr(), self.len()) } }
}
impl Hash for u64 {
    fn hash(&self) -> u64 {
        let mut x = *self;
        x ^= x >> 33; x = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^= x >> 33; x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
        x ^= x >> 33; x
    }
}
impl Hash for u32   { fn hash(&self) -> u64 { (*self as u64).hash() } }
impl Hash for i32   { fn hash(&self) -> u64 { (*self as u64).hash() } }
impl Hash for usize { fn hash(&self) -> u64 { (*self as u64).hash() } }

// ── HashMap ───────────────────────────────────────────────────────────────────

pub struct HashMap<K, V> {
    pub slots: Vec<Option<(K, V, bool)>>,
    len:       usize,
    cap:       usize,
}

impl<K: Hash + PartialEq + Clone, V> HashMap<K, V> {
    pub fn new() -> Self { Self::with_capacity(16) }

    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.next_power_of_two();
        let mut slots = Vec::with_capacity(cap);
        for _ in 0..cap { slots.push(None); }
        Self { slots, len: 0, cap }
    }

    fn probe(&self, key: &K) -> usize { (key.hash() as usize) & (self.cap - 1) }

    pub fn insert(&mut self, key: K, value: V) {
        if self.len * 4 >= self.cap * 3 { self.resize(); }
        let start = self.probe(&key);
        let mask  = self.cap - 1;
        let mut i = start; let mut j = 0usize;
        loop {
            match &self.slots[i] {
                None => { self.slots[i] = Some((key, value, false)); self.len += 1; return; }
                Some((k, _, true))  if *k == key => { self.slots[i] = Some((key, value, false)); self.len += 1; return; }
                Some((_, _, true))               => { self.slots[i] = Some((key, value, false)); self.len += 1; return; }
                Some((k, _, false)) if *k == key => { self.slots[i] = Some((key, value, false)); return; }
                _ => {}
            }
            j += 1; i = (start + j + j * j) & mask;
            if j > self.cap { break; }
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let start = self.probe(key);
        let mask  = self.cap - 1;
        let mut i = start; let mut j = 0usize;
        loop {
            match &self.slots[i] {
                None                            => return None,
                Some((k, v, false)) if k == key => return Some(v),
                Some(_)                         => {}
            }
            j += 1; i = (start + j + j * j) & mask;
            if j > self.cap { break; }
        }
        None
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let start = self.probe(key);
        let mask  = self.cap - 1;
        let mut i = start; let mut j = 0usize;
        loop {
            match &self.slots[i] {
                None                            => return None,
                Some((k, _, false)) if k == key => {
                    return self.slots[i].as_mut().map(|(_, v, _)| v);
                }
                Some(_) => {}
            }
            j += 1; i = (start + j + j * j) & mask;
            if j > self.cap { break; }
        }
        None
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let start = self.probe(key);
        let mask  = self.cap - 1;
        let mut i = start; let mut j = 0usize;
        loop {
            match &self.slots[i] {
                None                            => return None,
                Some((k, _, false)) if k == key => {
                    let (_, val, _) = self.slots[i].take().unwrap();
                    self.len -= 1;
                    return Some(val);
                }
                Some(_) => {}
            }
            j += 1; i = (start + j + j * j) & mask;
            if j > self.cap { break; }
        }
        None
    }

    pub fn len(&self)      -> usize { self.len }
    pub fn is_empty(&self) -> bool  { self.len == 0 }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots.iter().filter_map(|s| {
            if let Some((k, v, false)) = s { Some((k, v)) } else { None }
        })
    }

    fn resize(&mut self) {
        let new_cap = self.cap * 2;
        let mut new_map = Self::with_capacity(new_cap);
        for slot in self.slots.drain(..) {
            if let Some((k, v, false)) = slot { new_map.insert(k, v); }
        }
        *self = new_map;
    }
}

// ── Random xorshift64 ─────────────────────────────────────────────────────────

static RNG_STATE: SpinLock<u64> = SpinLock::new(0x853c49e6748fea9b);

pub fn random() -> u64 {
    let mut state = RNG_STATE.lock();
    let mut x = *state;
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    *state = x; x
}

pub fn random_range(min: u64, max: u64) -> u64 {
    debug_assert!(max > min);
    min + (random() % (max - min + 1))
}

pub fn random_u32() -> u32 { random() as u32 }
