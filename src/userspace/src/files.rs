// src/userspace/src/files.rs — CosinusOS VFS v1.0
//
// Schemat adresowania:
//   <dysk><partycja>;<ścieżka>
//
//   Dyski:  ! @ # $ % ^ & *  (8 dysków fizycznych)
//   Partycje: d1 d2 d3 ... d9
//   Separator dysk/ścieżka: ;
//
//   Przykłady:
//     !d1;/home/ctrl/desktop/koty/     — dysk !, partycja 1
//     @d2;/etc/config.cfg              — dysk @, partycja 2
//     #d1;/var/log/kernel.log          — dysk #, partycja 1
//
// Architektura (od góry):
//
//   ┌─────────────────────┐
//   │  Public API (VFS)   │  open / read / write / seek / close / stat / ls / mkdir / rm
//   ├─────────────────────┤
//   │  Path Parser        │  rozkłada "!d1;/home/..." → DiskId + PartId + &str
//   ├─────────────────────┤
//   │  Mount Table        │  HashMap<(DiskId, PartId), FsDriver>
//   ├─────────────────────┤
//   │  Filesystem Driver  │  Trait: FsDriver (FatFs, RamFs, DevFs, ...)
//   ├─────────────────────┤
//   │  Block Layer        │  Trait: BlockDevice (czyta/pisze sektory 512 B)
//   ├─────────────────────┤
//   │  Block Cache        │  LRU cache sektorów (bez alokacji stosu)
//   └─────────────────────┘

extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;
use crate::{HashMap, SpinLock};

// ============================================================================
// TYPY BAZOWE
// ============================================================================

/// Identyfikator dysku fizycznego.
/// Znaki dozwolone: ! @ # $ % ^ & *
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DiskId(pub char);

/// Identyfikator partycji (d1–d9).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct PartId(pub u8); // 1–9

/// Rozszyfrowany adres: dysk + partycja + ścieżka w obrębie systemu plików.
#[derive(Clone, Debug)]
pub struct FsPath {
    pub disk: DiskId,
    pub part: PartId,
    pub path: String,   // zawiera leading '/', np. "/home/ctrl/file.txt"
}

/// Błędy VFS.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FsError {
    InvalidPath,        // niepoprawna składnia ścieżki
    NotFound,           // plik/katalog nie istnieje
    NotAFile,           // oczekiwano pliku, dostano katalog
    NotADir,            // oczekiwano katalogu, dostano plik
    PermissionDenied,
    AlreadyExists,
    DiskFull,
    IoError,
    NotMounted,         // partycja nie jest podmontowana
    BadDiskId,          // nieznany znak dysku
    BadPartId,          // partycja poza zakresem 1–9
    EndOfFile,
    InvalidOffset,
    NotSupported,
}

pub type FsResult<T> = Result<T, FsError>;

/// Metadata pliku lub katalogu.
#[derive(Clone, Debug)]
pub struct FileStat {
    pub name:      String,
    pub size:      u64,
    pub is_dir:    bool,
    pub is_file:   bool,
    pub created:   u64,   // timestamp (ms od epoki — zależnie od RTC)
    pub modified:  u64,
}

/// Deskryptor otwartego pliku.
#[derive(Clone, Debug)]
pub struct FileHandle {
    pub path:   FsPath,
    pub offset: u64,
    pub flags:  OpenFlags,
}

bitflags! {
    pub struct OpenFlags: u32 {
        const READ    = 0b0001;
        const WRITE   = 0b0010;
        const APPEND  = 0b0100;
        const CREATE  = 0b1000;
        const TRUNC   = 0b10000;
    }
}

/// Prosta implementacja bitflags bez zewnętrznej skrzynki.
macro_rules! bitflags {
    (pub struct $name:ident : $base:ty { $( const $flag:ident = $val:expr; )* }) => {
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        pub struct $name(pub $base);
        impl $name {
            $( pub const $flag: Self = Self($val); )*
            pub fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
            pub fn empty() -> Self { Self(0) }
        }
        impl core::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
        }
        impl core::ops::BitAnd for $name {
            type Output = Self;
            fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
        }
    };
}

// Re-definiujemy bez konfliktu (makro jest zdefiniowane powyżej użycia przez struct)
// Rustowe makro musi być przed użyciem — przenosimy OpenFlags tutaj:

// ============================================================================
// PARSER ŚCIEŻEK
// ============================================================================

const VALID_DISK_CHARS: &[char] = &['!', '@', '#', '$', '%', '^', '&', '*'];

/// Parsuje "!d1;/home/ctrl/file.txt" → FsPath.
///
/// Format: <disk_char> 'd' <digit 1–9> ';' <path>
/// Przykład: "!d1;/etc/hosts"  →  disk='!', part=1, path="/etc/hosts"
pub fn parse_path(raw: &str) -> FsResult<FsPath> {
    // Minimum: "!d1;/" = 5 znaków
    if raw.len() < 5 {
        return Err(FsError::InvalidPath);
    }

    let bytes = raw.as_bytes();

    // 1. Znak dysku
    let disk_char = raw.chars().next().unwrap();
    if !VALID_DISK_CHARS.contains(&disk_char) {
        return Err(FsError::BadDiskId);
    }

    // 2. Litera 'd'
    if bytes[1] != b'd' {
        return Err(FsError::InvalidPath);
    }

    // 3. Cyfra partycji 1–9
    let part_digit = bytes[2];
    if !(b'1'..=b'9').contains(&part_digit) {
        return Err(FsError::BadPartId);
    }
    let part = PartId(part_digit - b'0');

    // 4. Separator ';'
    if bytes[3] != b';' {
        return Err(FsError::InvalidPath);
    }

    // 5. Ścieżka (musi zaczynać się od '/')
    let path_str = &raw[4..];
    if !path_str.starts_with('/') {
        return Err(FsError::InvalidPath);
    }

    // Normalizuj: usuń trailing slash (oprócz root "/")
    let normalized = if path_str.len() > 1 && path_str.ends_with('/') {
        String::from(&path_str[..path_str.len() - 1])
    } else {
        String::from(path_str)
    };

    Ok(FsPath {
        disk: DiskId(disk_char),
        part,
        path: normalized,
    })
}

/// Wyodrębnia nazwę pliku/katalogu ze ścieżki (ostatni komponent).
pub fn path_basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// Zwraca katalog nadrzędny ("/home/ctrl" → "/home").
pub fn path_dirname(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None    => ".",
    }
}

/// Łączy dwie ścieżki: path_join("/home", "ctrl") → "/home/ctrl"
pub fn path_join(base: &str, name: &str) -> String {
    let mut result = String::from(base.trim_end_matches('/'));
    result.push('/');
    result.push_str(name.trim_start_matches('/'));
    result
}

/// Sprawdza, czy ścieżka zawiera niedozwolone komponenty ("..").
pub fn path_is_safe(path: &str) -> bool {
    !path.split('/').any(|c| c == "..")
}

// ============================================================================
// BLOCK LAYER — abstrakcja urządzenia blokowego
// ============================================================================

pub const SECTOR_SIZE: usize = 512;

/// Numer sektora (LBA).
pub type Lba = u64;

/// Interfejs urządzenia blokowego (dysk fizyczny, RAM-disk, ...).
pub trait BlockDevice: Send {
    fn name(&self)         -> &str;
    fn sector_count(&self) -> u64;
    fn sector_size(&self)  -> usize { SECTOR_SIZE }

    /// Odczytaj `count` sektorów od `lba` do `buf`.
    /// buf.len() musi być == count * sector_size().
    fn read_sectors(&mut self, lba: Lba, count: u64, buf: &mut [u8]) -> FsResult<()>;

    /// Zapisz `count` sektorów od `lba` z `buf`.
    fn write_sectors(&mut self, lba: Lba, count: u64, buf: &[u8]) -> FsResult<()>;

    /// Flush — wymuś zapis buforowanych danych.
    fn flush(&mut self) -> FsResult<()> { Ok(()) }
}

/// RAM-disk (urządzenie blokowe w pamięci) — do testów i RAMfs.
pub struct RamDisk {
    pub name:    &'static str,
    pub data:    Vec<u8>,
    pub sectors: u64,
}

impl RamDisk {
    pub fn new(name: &'static str, size_bytes: usize) -> Self {
        let sectors = (size_bytes / SECTOR_SIZE) as u64;
        let mut data = Vec::with_capacity(size_bytes);
        data.resize(size_bytes, 0);
        Self { name, data, sectors }
    }
}

impl BlockDevice for RamDisk {
    fn name(&self) -> &str { self.name }
    fn sector_count(&self) -> u64 { self.sectors }

    fn read_sectors(&mut self, lba: Lba, count: u64, buf: &mut [u8]) -> FsResult<()> {
        let offset = (lba * SECTOR_SIZE as u64) as usize;
        let len    = (count * SECTOR_SIZE as u64) as usize;
        if offset + len > self.data.len() { return Err(FsError::IoError); }
        buf[..len].copy_from_slice(&self.data[offset..offset + len]);
        Ok(())
    }

    fn write_sectors(&mut self, lba: Lba, count: u64, buf: &[u8]) -> FsResult<()> {
        let offset = (lba * SECTOR_SIZE as u64) as usize;
        let len    = (count * SECTOR_SIZE as u64) as usize;
        if offset + len > self.data.len() { return Err(FsError::IoError); }
        self.data[offset..offset + len].copy_from_slice(&buf[..len]);
        Ok(())
    }
}

// ============================================================================
// BLOCK CACHE — LRU cache sektorów (stała liczba slotów)
// ============================================================================

const CACHE_SLOTS: usize = 64; // ile sektorów trzymamy w pamięci

struct CacheSlot {
    lba:     Lba,
    disk:    u8,     // indeks dysku (0–7)
    dirty:   bool,
    valid:   bool,
    lru_gen: u64,    // wartość zegara LRU przy ostatnim dostępie
    data:    [u8; SECTOR_SIZE],
}

impl CacheSlot {
    const fn empty() -> Self {
        Self { lba: 0, disk: 0, dirty: false, valid: false, lru_gen: 0,
               data: [0; SECTOR_SIZE] }
    }
}

pub struct BlockCache {
    slots:   [CacheSlot; CACHE_SLOTS],
    clock:   u64,
    hits:    u64,
    misses:  u64,
}

impl BlockCache {
    pub const fn new() -> Self {
        // const fn — nie możemy użyć [CacheSlot::empty(); N] z non-Copy typem,
        // ale CacheSlot jest Copy (wszystkie pola Copy).
        Self {
            slots:  [CacheSlot::empty(); CACHE_SLOTS],
            clock:  0,
            hits:   0,
            misses: 0,
        }
    }

    fn find(&self, disk: u8, lba: Lba) -> Option<usize> {
        self.slots.iter().position(|s| s.valid && s.disk == disk && s.lba == lba)
    }

    fn evict_lru(&mut self) -> usize {
        // Znajdź slot z najniższym lru_gen (lub nieważny)
        let mut best = 0;
        let mut best_gen = u64::MAX;
        for (i, s) in self.slots.iter().enumerate() {
            if !s.valid { return i; } // wolny slot
            if s.lru_gen < best_gen { best_gen = s.lru_gen; best = i; }
        }
        best
    }

    /// Odczytaj sektor. Jeśli w cache → hit. Inaczej → miss, wczytaj przez device.
    pub fn read(
        &mut self,
        device: &mut dyn BlockDevice,
        disk_idx: u8,
        lba: Lba,
        buf: &mut [u8; SECTOR_SIZE],
    ) -> FsResult<()> {
        self.clock += 1;

        if let Some(i) = self.find(disk_idx, lba) {
            self.hits += 1;
            self.slots[i].lru_gen = self.clock;
            buf.copy_from_slice(&self.slots[i].data);
            return Ok(());
        }

        // Cache miss — wczytaj z urządzenia
        self.misses += 1;
        let evict = self.evict_lru();

        // Jeśli evicted slot jest dirty — wypisz go z powrotem
        if self.slots[evict].valid && self.slots[evict].dirty {
            let elba  = self.slots[evict].lba;
            let edata = self.slots[evict].data;
            device.write_sectors(elba, 1, &edata)?;
        }

        // Wczytaj nowy sektor
        device.read_sectors(lba, 1, &mut self.slots[evict].data)?;
        self.slots[evict] = CacheSlot {
            lba, disk: disk_idx, dirty: false, valid: true, lru_gen: self.clock,
            data: self.slots[evict].data,
        };
        buf.copy_from_slice(&self.slots[evict].data);
        Ok(())
    }

    /// Zapisz sektor (write-back — oznacz dirty).
    pub fn write(
        &mut self,
        disk_idx: u8,
        lba: Lba,
        buf: &[u8; SECTOR_SIZE],
    ) {
        self.clock += 1;
        let slot = if let Some(i) = self.find(disk_idx, lba) {
            i
        } else {
            self.evict_lru()
        };
        self.slots[slot] = CacheSlot {
            lba, disk: disk_idx, dirty: true, valid: true, lru_gen: self.clock,
            data: *buf,
        };
    }

    /// Flush wszystkich dirty slotów z danego dysku.
    pub fn flush_disk(&mut self, device: &mut dyn BlockDevice, disk_idx: u8) -> FsResult<()> {
        for slot in &mut self.slots {
            if slot.valid && slot.dirty && slot.disk == disk_idx {
                device.write_sectors(slot.lba, 1, &slot.data)?;
                slot.dirty = false;
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> (u64, u64) { (self.hits, self.misses) }
}

// ============================================================================
// FILESYSTEM DRIVER — trait dla konkretnych systemów plików
// ============================================================================

/// Każdy sterownik FS implementuje ten trait na bazie danych z BlockDevice.
pub trait FsDriver: Send {
    fn name(&self) -> &str;

    // --- Pliki ---
    fn read_file  (&mut self, path: &str, offset: u64, buf: &mut [u8]) -> FsResult<usize>;
    fn write_file (&mut self, path: &str, offset: u64, buf: &[u8])     -> FsResult<usize>;
    fn create_file(&mut self, path: &str)                               -> FsResult<()>;
    fn delete_file(&mut self, path: &str)                               -> FsResult<()>;
    fn stat       (&mut self, path: &str)                               -> FsResult<FileStat>;
    fn truncate   (&mut self, path: &str, new_size: u64)                -> FsResult<()>;

    // --- Katalogi ---
    fn list_dir  (&mut self, path: &str) -> FsResult<Vec<FileStat>>;
    fn create_dir(&mut self, path: &str) -> FsResult<()>;
    fn delete_dir(&mut self, path: &str) -> FsResult<()>;

    // --- Operacje systemu ---
    fn flush(&mut self) -> FsResult<()>;
}

// ============================================================================
// RAMFS — prosty in-memory filesystem (dla testów i /tmp)
// ============================================================================

/// Węzeł drzewa RamFs.
#[derive(Clone)]
struct RamNode {
    name:     String,
    is_dir:   bool,
    data:     Vec<u8>,        // dla plików
    children: Vec<RamNode>,   // dla katalogów
    modified: u64,
}

impl RamNode {
    fn new_file(name: &str) -> Self {
        Self { name: String::from(name), is_dir: false,
               data: Vec::new(), children: Vec::new(), modified: 0 }
    }
    fn new_dir(name: &str) -> Self {
        Self { name: String::from(name), is_dir: true,
               data: Vec::new(), children: Vec::new(), modified: 0 }
    }

    /// Znajdź węzeł na ścieżce (np. "/home/ctrl/file.txt").
    fn find_mut(&mut self, path: &str) -> Option<&mut RamNode> {
        let path = path.trim_start_matches('/');
        if path.is_empty() { return Some(self); }

        let (first, rest) = match path.find('/') {
            Some(i) => (&path[..i], &path[i+1..]),
            None    => (path, ""),
        };

        for child in &mut self.children {
            if child.name == first {
                return if rest.is_empty() {
                    Some(child)
                } else {
                    child.find_mut(rest)
                };
            }
        }
        None
    }

    fn find_ref(&self, path: &str) -> Option<&RamNode> {
        let path = path.trim_start_matches('/');
        if path.is_empty() { return Some(self); }
        let (first, rest) = match path.find('/') {
            Some(i) => (&path[..i], &path[i+1..]),
            None    => (path, ""),
        };
        for child in &self.children {
            if child.name == first {
                return if rest.is_empty() { Some(child) } else { child.find_ref(rest) };
            }
        }
        None
    }

    /// Dodaj węzeł na ścieżce. Tworzy brakujące katalogi.
    fn ensure_parent_and_add(&mut self, path: &str, node: RamNode) -> FsResult<()> {
        let parent = path_dirname(path);
        let parent_node = if parent == "/" {
            Some(self)
        } else {
            self.find_mut(parent)
        };
        match parent_node {
            Some(p) if p.is_dir => {
                let name = path_basename(path);
                if p.children.iter().any(|c| c.name == name) {
                    return Err(FsError::AlreadyExists);
                }
                p.children.push(node);
                Ok(())
            }
            Some(_) => Err(FsError::NotADir),
            None    => Err(FsError::NotFound),
        }
    }
}

pub struct RamFs {
    root: RamNode,
}

impl RamFs {
    pub fn new() -> Self {
        Self { root: RamNode::new_dir("/") }
    }
}

impl FsDriver for RamFs {
    fn name(&self) -> &str { "ramfs" }

    fn read_file(&mut self, path: &str, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        let node = self.root.find_ref(path).ok_or(FsError::NotFound)?;
        if node.is_dir { return Err(FsError::NotAFile); }
        let off = offset as usize;
        if off >= node.data.len() { return Ok(0); } // EOF
        let available = node.data.len() - off;
        let n = available.min(buf.len());
        buf[..n].copy_from_slice(&node.data[off..off + n]);
        Ok(n)
    }

    fn write_file(&mut self, path: &str, offset: u64, buf: &[u8]) -> FsResult<usize> {
        let node = self.root.find_mut(path).ok_or(FsError::NotFound)?;
        if node.is_dir { return Err(FsError::NotAFile); }
        let off = offset as usize;
        let end = off + buf.len();
        if end > node.data.len() { node.data.resize(end, 0); }
        node.data[off..end].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn create_file(&mut self, path: &str) -> FsResult<()> {
        let name = path_basename(path);
        self.root.ensure_parent_and_add(path, RamNode::new_file(name))
    }

    fn delete_file(&mut self, path: &str) -> FsResult<()> {
        let parent_path = path_dirname(path);
        let name = path_basename(path);
        let parent = if parent_path == "/" {
            &mut self.root
        } else {
            self.root.find_mut(parent_path).ok_or(FsError::NotFound)?
        };
        let pos = parent.children.iter().position(|c| c.name == name && !c.is_dir)
            .ok_or(FsError::NotFound)?;
        parent.children.remove(pos);
        Ok(())
    }

    fn stat(&mut self, path: &str) -> FsResult<FileStat> {
        let node = self.root.find_ref(path).ok_or(FsError::NotFound)?;
        Ok(FileStat {
            name:     node.name.clone(),
            size:     node.data.len() as u64,
            is_dir:   node.is_dir,
            is_file:  !node.is_dir,
            created:  0,
            modified: node.modified,
        })
    }

    fn truncate(&mut self, path: &str, new_size: u64) -> FsResult<()> {
        let node = self.root.find_mut(path).ok_or(FsError::NotFound)?;
        if node.is_dir { return Err(FsError::NotAFile); }
        node.data.resize(new_size as usize, 0);
        Ok(())
    }

    fn list_dir(&mut self, path: &str) -> FsResult<Vec<FileStat>> {
        let node = self.root.find_ref(path).ok_or(FsError::NotFound)?;
        if !node.is_dir { return Err(FsError::NotADir); }
        Ok(node.children.iter().map(|c| FileStat {
            name:     c.name.clone(),
            size:     c.data.len() as u64,
            is_dir:   c.is_dir,
            is_file:  !c.is_dir,
            created:  0,
            modified: c.modified,
        }).collect())
    }

    fn create_dir(&mut self, path: &str) -> FsResult<()> {
        let name = path_basename(path);
        self.root.ensure_parent_and_add(path, RamNode::new_dir(name))
    }

    fn delete_dir(&mut self, path: &str) -> FsResult<()> {
        // Usuń tylko pusty katalog
        let node = self.root.find_ref(path).ok_or(FsError::NotFound)?;
        if !node.is_dir { return Err(FsError::NotADir); }
        if !node.children.is_empty() { return Err(FsError::NotSupported); } // nie pusty

        let parent_path = path_dirname(path);
        let name = path_basename(path);
        let parent = if parent_path == "/" {
            &mut self.root
        } else {
            self.root.find_mut(parent_path).ok_or(FsError::NotFound)?
        };
        let pos = parent.children.iter().position(|c| c.name == name && c.is_dir)
            .ok_or(FsError::NotFound)?;
        parent.children.remove(pos);
        Ok(())
    }

    fn flush(&mut self) -> FsResult<()> { Ok(()) }
}

// ============================================================================
// VFS — główna warstwa montażu i dyspozycji
// ============================================================================

/// Klucz tablicy montażu: (DiskId.0 as u8, PartId.0).
fn mount_key(disk: DiskId, part: PartId) -> u64 {
    ((disk.0 as u64) << 8) | part.0 as u64
}

pub struct Vfs {
    mounts: HashMap<u64, Box<dyn FsDriver>>,
    cache:  BlockCache,
}

impl Vfs {
    pub fn new() -> Self {
        Self {
            mounts: HashMap::new(),
            cache:  BlockCache::new(),
        }
    }

    // ── Montowanie ──────────────────────────────────────────────────────────

    /// Podmontuj sterownik FS pod danym dyskiem i partycją.
    pub fn mount(&mut self, disk: DiskId, part: PartId, driver: Box<dyn FsDriver>) {
        let key = mount_key(disk, part);
        self.mounts.insert(key, driver);
    }

    /// Odmontuj.
    pub fn umount(&mut self, disk: DiskId, part: PartId) -> FsResult<()> {
        let key = mount_key(disk, part);
        match self.mounts.get_mut(&key) {
            Some(drv) => { drv.flush()?; }
            None => return Err(FsError::NotMounted),
        }
        self.mounts.remove(&key);
        Ok(())
    }

    fn driver_for(&mut self, disk: DiskId, part: PartId) -> FsResult<&mut Box<dyn FsDriver>> {
        self.mounts.get_mut(&mount_key(disk, part)).ok_or(FsError::NotMounted)
    }

    // ── Walidacja ───────────────────────────────────────────────────────────

    fn parse_and_validate(&self, raw: &str) -> FsResult<FsPath> {
        let fp = parse_path(raw)?;
        if !path_is_safe(&fp.path) { return Err(FsError::InvalidPath); }
        Ok(fp)
    }

    // ── Public API ──────────────────────────────────────────────────────────

    /// Otwórz plik — zwraca FileHandle.
    pub fn open(&mut self, raw_path: &str, flags: OpenFlags) -> FsResult<FileHandle> {
        let fp = self.parse_and_validate(raw_path)?;

        if flags.contains(OpenFlags::CREATE) {
            // Utwórz jeśli nie istnieje (ignoruj AlreadyExists)
            let drv = self.driver_for(fp.disk, fp.part)?;
            match drv.create_file(&fp.path) {
                Ok(_) | Err(FsError::AlreadyExists) => {}
                Err(e) => return Err(e),
            }
            if flags.contains(OpenFlags::TRUNC) {
                let drv = self.driver_for(fp.disk, fp.part)?;
                drv.truncate(&fp.path, 0)?;
            }
        } else {
            // Sprawdź czy istnieje
            let drv = self.driver_for(fp.disk, fp.part)?;
            let stat = drv.stat(&fp.path)?;
            if stat.is_dir { return Err(FsError::NotAFile); }
        }

        let offset = if flags.contains(OpenFlags::APPEND) {
            let drv = self.driver_for(fp.disk, fp.part)?;
            drv.stat(&fp.path)?.size
        } else {
            0
        };

        Ok(FileHandle { path: fp, offset, flags })
    }

    /// Odczyt z pliku przez handle.
    pub fn read(&mut self, handle: &mut FileHandle, buf: &mut [u8]) -> FsResult<usize> {
        if !handle.flags.contains(OpenFlags::READ) {
            return Err(FsError::PermissionDenied);
        }
        let drv = self.driver_for(handle.path.disk, handle.path.part)?;
        let n = drv.read_file(&handle.path.path, handle.offset, buf)?;
        handle.offset += n as u64;
        Ok(n)
    }

    /// Zapis do pliku przez handle.
    pub fn write(&mut self, handle: &mut FileHandle, buf: &[u8]) -> FsResult<usize> {
        if !handle.flags.contains(OpenFlags::WRITE) && !handle.flags.contains(OpenFlags::APPEND) {
            return Err(FsError::PermissionDenied);
        }
        let drv = self.driver_for(handle.path.disk, handle.path.part)?;
        let n = drv.write_file(&handle.path.path, handle.offset, buf)?;
        handle.offset += n as u64;
        Ok(n)
    }

    /// Seek.
    pub fn seek(&self, handle: &mut FileHandle, offset: u64) {
        handle.offset = offset;
    }

    /// Zamknij plik (flush).
    pub fn close(&mut self, handle: FileHandle) -> FsResult<()> {
        let drv = self.driver_for(handle.path.disk, handle.path.part)?;
        drv.flush()
    }

    /// Stat pliku lub katalogu.
    pub fn stat(&mut self, raw_path: &str) -> FsResult<FileStat> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.stat(&fp.path)
    }

    /// Listuj katalog.
    pub fn ls(&mut self, raw_path: &str) -> FsResult<Vec<FileStat>> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.list_dir(&fp.path)
    }

    /// Utwórz katalog.
    pub fn mkdir(&mut self, raw_path: &str) -> FsResult<()> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.create_dir(&fp.path)
    }

    /// Usuń plik.
    pub fn rm(&mut self, raw_path: &str) -> FsResult<()> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.delete_file(&fp.path)
    }

    /// Usuń katalog (musi być pusty).
    pub fn rmdir(&mut self, raw_path: &str) -> FsResult<()> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.delete_dir(&fp.path)
    }

    /// Wczytaj cały plik do Vec<u8>.
    pub fn read_all(&mut self, raw_path: &str) -> FsResult<Vec<u8>> {
        let stat = self.stat(raw_path)?;
        let mut buf = Vec::with_capacity(stat.size as usize);
        buf.resize(stat.size as usize, 0);
        let flags = OpenFlags::READ;
        let mut handle = self.open(raw_path, flags)?;
        self.read(&mut handle, &mut buf)?;
        self.close(handle)?;
        Ok(buf)
    }

    /// Zapisz Vec<u8> jako cały plik (truncate + create).
    pub fn write_all(&mut self, raw_path: &str, data: &[u8]) -> FsResult<()> {
        let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNC;
        let mut handle = self.open(raw_path, flags)?;
        self.write(&mut handle, data)?;
        self.close(handle)
    }

    /// Flush wszystkiego.
    pub fn sync(&mut self) -> FsResult<()> {
        // Iterujemy po wartościach — borrow checker wymaga pętli po kluczach
        let keys: Vec<u64> = self.mounts.slots.iter()
            .filter_map(|s| s.as_ref().and_then(|(k, _, tomb)| if !tomb { Some(*k) } else { None }))
            .collect();
        for k in keys {
            if let Some(drv) = self.mounts.slots.iter_mut()
                .find_map(|s| s.as_mut().and_then(|(key, v, tomb)|
                    if *key == k && !*tomb { Some(v) } else { None }))
            {
                drv.flush()?;
            }
        }
        Ok(())
    }
}

// ============================================================================
// GLOBALNA INSTANCJA VFS — chroniona SpinLockiem
// ============================================================================

static VFS: SpinLock<Option<Vfs>> = SpinLock::new(None);

/// Inicjalizuj globalny VFS.
pub fn vfs_init() {
    let mut guard = VFS.lock();
    *guard = Some(Vfs::new());
}

/// Podmontuj system plików pod ścieżkę dysku.
pub fn vfs_mount(disk: DiskId, part: PartId, driver: Box<dyn FsDriver>) {
    VFS.lock().as_mut().unwrap().mount(disk, part, driver);
}

/// Wczytaj plik jako Vec<u8>.
pub fn file_read_all(path: &str) -> FsResult<Vec<u8>> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.read_all(path)
}

/// Zapisz dane do pliku.
pub fn file_write_all(path: &str, data: &[u8]) -> FsResult<()> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.write_all(path, data)
}

/// Stat pliku/katalogu.
pub fn file_stat(path: &str) -> FsResult<FileStat> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.stat(path)
}

/// Listuj katalog.
pub fn dir_list(path: &str) -> FsResult<Vec<FileStat>> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.ls(path)
}

/// Utwórz katalog.
pub fn dir_create(path: &str) -> FsResult<()> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.mkdir(path)
}

/// Usuń plik.
pub fn file_remove(path: &str) -> FsResult<()> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.rm(path)
}

// ============================================================================
// CONVENIENCE — publiczny entry point
// ============================================================================

/// Inicjalizacja podsystemu plików.
/// Montuje domyślny RamFs pod !d1 (dysk główny).
pub fn file_system() {
    vfs_init();

    // Domyślnie: !d1 → RamFs (do celów testowych i /tmp)
    let ramfs = Box::new(RamFs::new());
    vfs_mount(DiskId('!'), PartId(1), ramfs);

    // Utwórz standardowe katalogi
    let _ = dir_create("!d1;/");
    let _ = dir_create("!d1;/home");
    let _ = dir_create("!d1;/home/ctrl");
    let _ = dir_create("!d1;/home/ctrl/desktop");
    let _ = dir_create("!d1;/tmp");
    let _ = dir_create("!d1;/etc");
    let _ = dir_create("!d1;/var");
    let _ = dir_create("!d1;/var/log");
}

// ============================================================================
// TESTY
// ============================================================================

#[cfg(test)]
pub fn run_vfs_tests() {
    use crate::println_fmt;

    println_fmt!("=== VFS Tests ===");

    // Test parsowania ścieżek
    let p = parse_path("!d1;/home/ctrl/file.txt").unwrap();
    assert_eq!(p.disk, DiskId('!'));
    assert_eq!(p.part, PartId(1));
    assert_eq!(p.path, "/home/ctrl/file.txt");

    let p2 = parse_path("@d3;/etc/config").unwrap();
    assert_eq!(p2.disk, DiskId('@'));
    assert_eq!(p2.part, PartId(3));

    assert!(parse_path("Xd1;/bad").is_err()); // niepoprawny znak dysku
    assert!(parse_path("!d0;/bad").is_err()); // partycja 0 niedozwolona
    assert!(parse_path("!d1/bad").is_err());  // brak ;

    println_fmt!("[OK] parse_path");

    // Test RamFs
    let mut fs = RamFs::new();
    fs.create_dir("/home").unwrap();
    fs.create_dir("/home/ctrl").unwrap();
    fs.create_file("/home/ctrl/test.txt").unwrap();
    fs.write_file("/home/ctrl/test.txt", 0, b"hello world").unwrap();

    let mut buf = [0u8; 11];
    let n = fs.read_file("/home/ctrl/test.txt", 0, &mut buf).unwrap();
    assert_eq!(n, 11);
    assert_eq!(&buf, b"hello world");
    println_fmt!("[OK] RamFs read/write");

    let entries = fs.list_dir("/home/ctrl").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "test.txt");
    println_fmt!("[OK] RamFs list_dir");

    // Test przez VFS
    vfs_init();
    vfs_mount(DiskId('!'), PartId(1), Box::new(RamFs::new()));

    dir_create("!d1;/docs").unwrap();
    file_write_all("!d1;/docs/readme.txt", b"CosinusOS rocks").unwrap();
    let data = file_read_all("!d1;/docs/readme.txt").unwrap();
    assert_eq!(&data, b"CosinusOS rocks");
    println_fmt!("[OK] VFS write_all / read_all");

    println_fmt!("=== All VFS tests passed ===");
}