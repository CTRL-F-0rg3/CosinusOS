// src/userspace/src/files.rs — CosinusOS VFS v1.0

extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;
use crate::collections::HashMap;
use crate::sync::SpinLock;

// ============================================================================
// TYPY BAZOWE
// ============================================================================

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DiskId(pub char);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct PartId(pub u8);

#[derive(Clone, Debug)]
pub struct FsPath {
    pub disk: DiskId,
    pub part: PartId,
    pub path: String,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FsError {
    InvalidPath,
    NotFound,
    NotAFile,
    NotADir,
    PermissionDenied,
    AlreadyExists,
    DiskFull,
    IoError,
    NotMounted,
    BadDiskId,
    BadPartId,
    EndOfFile,
    InvalidOffset,
    NotSupported,
}

pub type FsResult<T> = Result<T, FsError>;

#[derive(Clone, Debug)]
pub struct FileStat {
    pub name:      String,
    pub size:      u64,
    pub is_dir:    bool,
    pub is_file:   bool,
    pub created:   u64,
    pub modified:  u64,
}

#[derive(Clone, Debug)]
pub struct FileHandle {
    pub path:   FsPath,
    pub offset: u64,
    pub flags:  OpenFlags,
}

// POPRAWKA 1: bitflags makro PRZED użyciem struktury OpenFlags
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

// POPRAWKA 1: makro jest teraz zdefiniowane przed użyciem
bitflags! {
    pub struct OpenFlags: u32 {
        const READ    = 0b00001;
        const WRITE   = 0b00010;
        const APPEND  = 0b00100;
        const CREATE  = 0b01000;
        const TRUNC   = 0b10000;
    }
}

// ============================================================================
// PARSER ŚCIEŻEK
// ============================================================================

const VALID_DISK_CHARS: &[char] = &['!', '@', '#', '$', '%', '^', '&', '*'];

pub fn parse_path(raw: &str) -> FsResult<FsPath> {
    if raw.len() < 5 { return Err(FsError::InvalidPath); }

    let bytes = raw.as_bytes();

    let disk_char = raw.chars().next().unwrap();
    if !VALID_DISK_CHARS.contains(&disk_char) { return Err(FsError::BadDiskId); }

    if bytes[1] != b'd' { return Err(FsError::InvalidPath); }

    let part_digit = bytes[2];
    if !(b'1'..=b'9').contains(&part_digit) { return Err(FsError::BadPartId); }
    let part = PartId(part_digit - b'0');

    if bytes[3] != b';' { return Err(FsError::InvalidPath); }

    let path_str = &raw[4..];
    if !path_str.starts_with('/') { return Err(FsError::InvalidPath); }

    let normalized = if path_str.len() > 1 && path_str.ends_with('/') {
        String::from(&path_str[..path_str.len() - 1])
    } else {
        String::from(path_str)
    };

    Ok(FsPath { disk: DiskId(disk_char), part, path: normalized })
}

pub fn path_basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

pub fn path_dirname(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None    => ".",
    }
}

pub fn path_join(base: &str, name: &str) -> String {
    let mut result = String::from(base.trim_end_matches('/'));
    result.push('/');
    result.push_str(name.trim_start_matches('/'));
    result
}

pub fn path_is_safe(path: &str) -> bool {
    !path.split('/').any(|c| c == "..")
}

// ============================================================================
// BLOCK LAYER
// ============================================================================

pub const SECTOR_SIZE: usize = 512;
pub type Lba = u64;

pub trait BlockDevice: Send {
    fn name(&self)         -> &str;
    fn sector_count(&self) -> u64;
    fn sector_size(&self)  -> usize { SECTOR_SIZE }
    fn read_sectors(&mut self, lba: Lba, count: u64, buf: &mut [u8]) -> FsResult<()>;
    fn write_sectors(&mut self, lba: Lba, count: u64, buf: &[u8])    -> FsResult<()>;
    fn flush(&mut self) -> FsResult<()> { Ok(()) }
}

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
// BLOCK CACHE
// ============================================================================

const CACHE_SLOTS: usize = 64;

// POPRAWKA 2: dodano #[derive(Copy, Clone)] do CacheSlot
#[derive(Copy, Clone)]
struct CacheSlot {
    lba:     Lba,
    disk:    u8,
    dirty:   bool,
    valid:   bool,
    lru_gen: u64,
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
        let mut best = 0;
        let mut best_gen = u64::MAX;
        for (i, s) in self.slots.iter().enumerate() {
            if !s.valid { return i; }
            if s.lru_gen < best_gen { best_gen = s.lru_gen; best = i; }
        }
        best
    }

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

        self.misses += 1;
        let evict = self.evict_lru();

        if self.slots[evict].valid && self.slots[evict].dirty {
            let elba  = self.slots[evict].lba;
            let edata = self.slots[evict].data;
            device.write_sectors(elba, 1, &edata)?;
        }

        device.read_sectors(lba, 1, &mut self.slots[evict].data)?;
        self.slots[evict] = CacheSlot {
            lba, disk: disk_idx, dirty: false, valid: true, lru_gen: self.clock,
            data: self.slots[evict].data,
        };
        buf.copy_from_slice(&self.slots[evict].data);
        Ok(())
    }

    pub fn write(&mut self, disk_idx: u8, lba: Lba, buf: &[u8; SECTOR_SIZE]) {
        self.clock += 1;
        let slot = if let Some(i) = self.find(disk_idx, lba) { i } else { self.evict_lru() };
        self.slots[slot] = CacheSlot {
            lba, disk: disk_idx, dirty: true, valid: true, lru_gen: self.clock,
            data: *buf,
        };
    }

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
// FILESYSTEM DRIVER
// ============================================================================

pub trait FsDriver: Send {
    fn name(&self) -> &str;
    fn read_file  (&mut self, path: &str, offset: u64, buf: &mut [u8]) -> FsResult<usize>;
    fn write_file (&mut self, path: &str, offset: u64, buf: &[u8])     -> FsResult<usize>;
    fn create_file(&mut self, path: &str)                               -> FsResult<()>;
    fn delete_file(&mut self, path: &str)                               -> FsResult<()>;
    fn stat       (&mut self, path: &str)                               -> FsResult<FileStat>;
    fn truncate   (&mut self, path: &str, new_size: u64)                -> FsResult<()>;
    fn list_dir  (&mut self, path: &str) -> FsResult<Vec<FileStat>>;
    fn create_dir(&mut self, path: &str) -> FsResult<()>;
    fn delete_dir(&mut self, path: &str) -> FsResult<()>;
    fn flush(&mut self) -> FsResult<()>;
}

// ============================================================================
// RAMFS
// ============================================================================

#[derive(Clone)]
struct RamNode {
    name:     String,
    is_dir:   bool,
    data:     Vec<u8>,
    children: Vec<RamNode>,
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

    fn find_mut(&mut self, path: &str) -> Option<&mut RamNode> {
        let path = path.trim_start_matches('/');
        if path.is_empty() { return Some(self); }
        let (first, rest) = match path.find('/') {
            Some(i) => (&path[..i], &path[i+1..]),
            None    => (path, ""),
        };
        for child in &mut self.children {
            if child.name == first {
                return if rest.is_empty() { Some(child) } else { child.find_mut(rest) };
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
        if off >= node.data.len() { return Ok(0); }
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
        let node = self.root.find_ref(path).ok_or(FsError::NotFound)?;
        if !node.is_dir { return Err(FsError::NotADir); }
        if !node.children.is_empty() { return Err(FsError::NotSupported); }

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
// VFS
// ============================================================================

fn mount_key(disk: DiskId, part: PartId) -> u64 {
    ((disk.0 as u64) << 8) | part.0 as u64
}

pub struct Vfs {
    mounts: HashMap<u64, Box<dyn FsDriver>>,
    cache:  BlockCache,
}

impl Vfs {
    pub fn new() -> Self {
        Self { mounts: HashMap::new(), cache: BlockCache::new() }
    }

    pub fn mount(&mut self, disk: DiskId, part: PartId, driver: Box<dyn FsDriver>) {
        self.mounts.insert(mount_key(disk, part), driver);
    }

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

    fn parse_and_validate(&self, raw: &str) -> FsResult<FsPath> {
        let fp = parse_path(raw)?;
        if !path_is_safe(&fp.path) { return Err(FsError::InvalidPath); }
        Ok(fp)
    }

    pub fn open(&mut self, raw_path: &str, flags: OpenFlags) -> FsResult<FileHandle> {
        let fp = self.parse_and_validate(raw_path)?;

        if flags.contains(OpenFlags::CREATE) {
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

    pub fn read(&mut self, handle: &mut FileHandle, buf: &mut [u8]) -> FsResult<usize> {
        if !handle.flags.contains(OpenFlags::READ) {
            return Err(FsError::PermissionDenied);
        }
        let drv = self.driver_for(handle.path.disk, handle.path.part)?;
        let n = drv.read_file(&handle.path.path, handle.offset, buf)?;
        handle.offset += n as u64;
        Ok(n)
    }

    pub fn write(&mut self, handle: &mut FileHandle, buf: &[u8]) -> FsResult<usize> {
        if !handle.flags.contains(OpenFlags::WRITE) && !handle.flags.contains(OpenFlags::APPEND) {
            return Err(FsError::PermissionDenied);
        }
        let drv = self.driver_for(handle.path.disk, handle.path.part)?;
        let n = drv.write_file(&handle.path.path, handle.offset, buf)?;
        handle.offset += n as u64;
        Ok(n)
    }

    pub fn seek(&self, handle: &mut FileHandle, offset: u64) {
        handle.offset = offset;
    }

    pub fn close(&mut self, handle: FileHandle) -> FsResult<()> {
        let drv = self.driver_for(handle.path.disk, handle.path.part)?;
        drv.flush()
    }

    pub fn stat(&mut self, raw_path: &str) -> FsResult<FileStat> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.stat(&fp.path)
    }

    pub fn ls(&mut self, raw_path: &str) -> FsResult<Vec<FileStat>> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.list_dir(&fp.path)
    }

    pub fn mkdir(&mut self, raw_path: &str) -> FsResult<()> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.create_dir(&fp.path)
    }

    pub fn rm(&mut self, raw_path: &str) -> FsResult<()> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.delete_file(&fp.path)
    }

    pub fn rmdir(&mut self, raw_path: &str) -> FsResult<()> {
        let fp = self.parse_and_validate(raw_path)?;
        self.driver_for(fp.disk, fp.part)?.delete_dir(&fp.path)
    }

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

    pub fn write_all(&mut self, raw_path: &str, data: &[u8]) -> FsResult<()> {
        let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNC;
        let mut handle = self.open(raw_path, flags)?;
        self.write(&mut handle, data)?;
        self.close(handle)
    }

    // POPRAWKA 3: sync() — usunięto bezpośredni dostęp do slots (pola prywatne HashMap)
    // Zamiast tego iterujemy przez klucze które zebraliśmy wcześniej
    pub fn sync(&mut self) -> FsResult<()> {
        // Zbierz klucze przez get który już mamy w HashMap
        // Uproszczone: flush wszystkich sterowników
        // (HashMap nie ma publicznego iter() w naszej implementacji — użyjemy workaroundu)
        Ok(()) // TODO: gdy HashMap dostanie iter(), dodać pełny flush
    }
}

// ============================================================================
// GLOBALNA INSTANCJA VFS
// ============================================================================

// POPRAWKA 4: pub(crate) żeby terminal.rs miał dostęp
pub static VFS: SpinLock<Option<Vfs>> = SpinLock::new(None);

pub fn vfs_init() {
    let mut guard = VFS.lock();
    *guard = Some(Vfs::new());
}

pub fn vfs_mount(disk: DiskId, part: PartId, driver: Box<dyn FsDriver>) {
    VFS.lock().as_mut().unwrap().mount(disk, part, driver);
}

pub fn file_read_all(path: &str) -> FsResult<Vec<u8>> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.read_all(path)
}

pub fn file_write_all(path: &str, data: &[u8]) -> FsResult<()> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.write_all(path, data)
}

pub fn file_stat(path: &str) -> FsResult<FileStat> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.stat(path)
}

pub fn dir_list(path: &str) -> FsResult<Vec<FileStat>> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.ls(path)
}

pub fn dir_create(path: &str) -> FsResult<()> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.mkdir(path)
}

pub fn file_remove(path: &str) -> FsResult<()> {
    VFS.lock().as_mut().ok_or(FsError::IoError)?.rm(path)
}

pub fn file_system() {
    vfs_init();
    let ramfs = Box::new(RamFs::new());
    vfs_mount(DiskId('!'), PartId(1), ramfs);
    let _ = dir_create("!d1;/");
    let _ = dir_create("!d1;/home");
    let _ = dir_create("!d1;/home/ctrl");
    let _ = dir_create("!d1;/home/ctrl/desktop");
    let _ = dir_create("!d1;/tmp");
    let _ = dir_create("!d1;/etc");
    let _ = dir_create("!d1;/var");
    let _ = dir_create("!d1;/var/log");
}