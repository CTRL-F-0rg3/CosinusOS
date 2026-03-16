// CosinusOS — syscall_api.rs
// Centralne API syscalli: numery, struktury, typy zwracane
// Używane zarówno przez kernel (dispatch) jak i userspace (jako import)

// ─────────────────────────────────────────────────────────────────────────────
// Numery syscalli
// ─────────────────────────────────────────────────────────────────────────────
pub mod nr {
    // ── Podstawowe I/O ────────────────────────────────────────────────────────
    pub const EXIT:           u64 = 0;
    pub const WRITE:          u64 = 1;
    pub const READ:           u64 = 2;
    pub const YIELD:          u64 = 3;
    pub const SPAWN:          u64 = 4;
    pub const SLEEP:          u64 = 5;

    // ── Pamięć (stare API — page-granular) ───────────────────────────────────
    pub const MEM_ALLOC:      u64 = 6;
    pub const MEM_FREE:       u64 = 7;

    // ── IPC ──────────────────────────────────────────────────────────────────
    pub const IPC_SEND:       u64 = 8;
    pub const IPC_RECV:       u64 = 9;
    pub const IPC_POLL:       u64 = 10;

    // ── Wątki ─────────────────────────────────────────────────────────────────
    pub const THREAD_ID:      u64 = 11;
    pub const TIME:           u64 = 12;
    pub const DEBUG_PRINT:    u64 = 13;
    pub const THREAD_SET_PRIO:u64 = 14;   // zmień priorytet własnego wątku
    pub const WAIT:           u64 = 15;   // czekaj na zakończenie wątku TID

    // ── Pliki / deskryptory ───────────────────────────────────────────────────
    pub const OPEN:           u64 = 20;   // otwórz plik/urządzenie → fd
    pub const CLOSE:          u64 = 21;   // zamknij fd
    pub const SEEK:           u64 = 22;   // ustaw pozycję w pliku
    pub const FSTAT:          u64 = 23;   // pobierz metadane fd → FileStat
    pub const IOCTL:          u64 = 24;   // sterowanie urządzeniem

    // ── Wirtualna pamięć (nowe API — byte-granular) ──────────────────────────
    pub const MMAP:           u64 = 30;   // mapuj pamięć wirtualną
    pub const MUNMAP:         u64 = 31;   // odmapuj zakres
    pub const MPROTECT:       u64 = 32;   // zmień ochronę stron

    // ── Sygnały ───────────────────────────────────────────────────────────────
    pub const SIGNAL:         u64 = 40;   // ustaw handler sygnału
    pub const KILL:           u64 = 41;   // wyślij sygnał do wątku
    pub const SIGRET:         u64 = 42;   // powrót z handlera sygnału (naked)

    // ── Filesystem ────────────────────────────────────────────────────────────
    pub const GETCWD:         u64 = 50;   // pobierz bieżący katalog
    pub const CHDIR:          u64 = 51;   // zmień bieżący katalog

    // ── Pipe ─────────────────────────────────────────────────────────────────
    pub const PIPE:           u64 = 60;   // utwórz parę read/write fd
}

// ─────────────────────────────────────────────────────────────────────────────
// Kody błędów (rax < 0 oznacza błąd)
// ─────────────────────────────────────────────────────────────────────────────
pub mod err {
    pub const OK:          i64 =  0;
    pub const INVAL:       i64 = -1;   // nieprawidłowy argument
    pub const NOMEM:       i64 = -2;   // brak pamięci
    pub const NOSLOT:      i64 = -3;   // brak slotu wątku / fd
    pub const FAULT:       i64 = -4;   // nieprawidłowy wskaźnik userspace
    pub const AGAIN:       i64 = -5;   // kolejka pusta / zasób chwilowo niedostępny
    pub const NOSYS:       i64 = -6;   // nieznany syscall
    pub const PERM:        i64 = -7;   // brak uprawnień
    pub const NOENT:       i64 = -8;   // plik / zasób nie istnieje
    pub const BADF:        i64 = -9;   // nieprawidłowy deskryptor pliku
    pub const BUSY:        i64 = -10;  // zasób zajęty
    pub const OVERFLOW:    i64 = -11;  // przepełnienie bufora / zakresu
    pub const NOSUP:       i64 = -12;  // operacja nieobsługiwana (stub)
    pub const ALIGN:       i64 = -13;  // błąd wyrównania adresu
    pub const EXIST:       i64 = -14;  // zasób już istnieje
}

// ─────────────────────────────────────────────────────────────────────────────
// Struktury — wspólne dla kernel i userspace (repr(C))
// ─────────────────────────────────────────────────────────────────────────────

/// Deskryptor spawnu wątku (syscall SPAWN)
#[repr(C)]
pub struct SpawnArgs {
    pub entry:    u64,        // adres funkcji wejściowej
    pub arg:      u64,        // argument (rdi przy wejściu)
    pub stack_sz: u32,        // rozmiar stosu userspace (0 = domyślny 0x4000)
    pub flags:    u32,        // SpawnFlags
    pub name:     [u8; 16],   // nazwa wątku (null-terminated)
}

pub mod spawn_flags {
    pub const KERNEL: u32 = 0;       // wątek kernelowy (tylko kernel może)
    pub const USER:   u32 = 1 << 0;  // wątek userspace
    pub const DETACH: u32 = 1 << 1;  // nie trzymaj slotu po exit
}

/// Wiadomość IPC (syscall IPC_SEND / IPC_RECV)
#[repr(C)]
pub struct IpcMsg {
    pub from:  u32,           // thread ID nadawcy (wypełnia kernel przy recv)
    pub to:    u32,           // thread ID odbiorcy
    pub tag:   u32,           // typ wiadomości (definiuje userspace)
    pub _pad:  u32,
    pub data:  [u64; 4],      // 32 bajty inline danych (bez alokacji)
    pub ptr:   u64,           // opcjonalny wskaźnik na większy bufor
    pub len:   u32,           // rozmiar bufora pod ptr (0 = brak)
    pub _pad2: u32,
}

/// Wynik THREAD_ID / ThreadInfo
#[repr(C)]
pub struct ThreadInfo {
    pub tid:   u32,
    pub prio:  u8,
    pub state: u8,            // 0=Run, 1=Ready, 2=Block, 3=Dead
    pub _pad:  [u8; 2],
}

/// Wynik TIME
#[repr(C)]
pub struct TimeInfo {
    pub ticks:  u64,          // liczba ticków od startu (100Hz)
    pub uptime: u64,          // sekundy od startu
}

/// Argumenty MMAP (syscall MMAP)
///
/// Przepływa przez rdi jako wskaźnik do tej struktury.
/// Zwraca adres wirtualny zmapowanego obszaru lub kod błędu.
#[repr(C)]
pub struct MmapArgs {
    pub hint:   u64,          // sugerowany adres wirtualny (0 = kernel wybiera)
    pub length: u64,          // rozmiar w bajtach (zaokrąglany w górę do strony)
    pub prot:   u32,          // MmapProt flags
    pub flags:  u32,          // MmapFlags
    pub fd:     i32,          // fd pliku (−1 = anonimowe)
    pub _pad:   u32,
    pub offset: u64,          // offset w pliku (tylko jeśli fd >= 0)
}

pub mod mmap_prot {
    pub const NONE:  u32 = 0;
    pub const READ:  u32 = 1 << 0;
    pub const WRITE: u32 = 1 << 1;
    pub const EXEC:  u32 = 1 << 2;
}

pub mod mmap_flags {
    pub const ANON:    u32 = 1 << 0;  // anonimowe mapowanie (bez pliku)
    pub const FIXED:   u32 = 1 << 1;  // wymuś dokładny adres hint
    pub const SHARED:  u32 = 1 << 2;  // mapowanie współdzielone (TODO: CoW)
    pub const PRIVATE: u32 = 1 << 3;  // mapowanie prywatne (domyślne)
}

/// Metadane pliku (syscall FSTAT)
#[repr(C)]
pub struct FileStat {
    pub size:     u64,        // rozmiar w bajtach
    pub kind:     u32,        // FileKind
    pub perm:     u32,        // bitmaska uprawnień (rwxrwxrwx)
    pub inode:    u64,        // numer i-węzła (0 jeśli nieobsługiwane)
    pub ctime:    u64,        // czas tworzenia (ticki kernela)
    pub mtime:    u64,        // czas modyfikacji (ticki kernela)
}

pub mod file_kind {
    pub const REG:  u32 = 0;  // plik zwykły
    pub const DIR:  u32 = 1;  // katalog
    pub const DEV:  u32 = 2;  // urządzenie
    pub const PIPE: u32 = 3;  // pipe
    pub const FIFO: u32 = 4;  // FIFO
}

/// Flagi otwarcia pliku (syscall OPEN, pole flags)
pub mod open_flags {
    pub const RDONLY:  u32 = 0;
    pub const WRONLY:  u32 = 1 << 0;
    pub const RDWR:    u32 = 1 << 1;
    pub const CREATE:  u32 = 1 << 2;
    pub const TRUNC:   u32 = 1 << 3;
    pub const APPEND:  u32 = 1 << 4;
    pub const NONBLOCK:u32 = 1 << 5;
}

/// Skąd liczyć offset (syscall SEEK, pole whence)
pub mod seek_whence {
    pub const SET: u32 = 0;   // od początku pliku
    pub const CUR: u32 = 1;   // od bieżącej pozycji
    pub const END: u32 = 2;   // od końca pliku
}

/// Numery sygnałów
pub mod sig {
    pub const KILL:   u32 = 1;   // zakończ wątek (nie do złapania)
    pub const TERM:   u32 = 2;   // grzeczne zakończenie
    pub const SEGV:   u32 = 3;   // naruszenie ochrony pamięci
    pub const BUS:    u32 = 4;   // błąd szyny / wyrównania
    pub const ILL:    u32 = 5;   // niedozwolona instrukcja
    pub const FPE:    u32 = 6;   // błąd arytmetyczny
    pub const IO:     u32 = 7;   // dane dostępne na fd
    pub const PIPE:   u32 = 8;   // zapis do zamkniętego pipe
    pub const USR1:   u32 = 16;  // sygnały użytkownika
    pub const USR2:   u32 = 17;
    pub const MAX:    u32 = 32;
}

/// Para fd dla pipe (syscall PIPE, przekazywana przez wskaźnik)
#[repr(C)]
pub struct PipeFds {
    pub read_fd:  i32,
    pub write_fd: i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher
// ─────────────────────────────────────────────────────────────────────────────
pub type SyscallFn = unsafe fn(*mut crate::perm::TF) -> i64;

#[no_mangle]
pub unsafe fn syscall_dispatch_v2(tf: *mut crate::perm::TF) {
    let num = (*tf).rax;
    let ret: i64 = match num {
        // ── istniejące ────────────────────────────────────────────────────────
        nr::EXIT          => sys_exit(tf),
        nr::WRITE         => sys_write(tf),
        nr::READ          => sys_read(tf),
        nr::YIELD         => sys_yield(tf),
        nr::SPAWN         => sys_spawn(tf),
        nr::SLEEP         => sys_sleep(tf),
        nr::MEM_ALLOC     => sys_mem_alloc(tf),
        nr::MEM_FREE      => sys_mem_free(tf),
        nr::IPC_SEND      => crate::ipc::sys_ipc_send(tf),
        nr::IPC_RECV      => crate::ipc::sys_ipc_recv(tf),
        nr::IPC_POLL      => crate::ipc::sys_ipc_poll(tf),
        nr::THREAD_ID     => sys_thread_id(tf),
        nr::TIME          => sys_time(tf),
        nr::DEBUG_PRINT   => sys_debug_print(tf),
        // ── nowe ─────────────────────────────────────────────────────────────
        nr::THREAD_SET_PRIO => sys_thread_set_prio(tf),
        nr::WAIT            => sys_wait(tf),
        nr::OPEN            => sys_open(tf),
        nr::CLOSE           => sys_close(tf),
        nr::SEEK            => sys_seek(tf),
        nr::FSTAT           => sys_fstat(tf),
        nr::IOCTL           => sys_ioctl(tf),
        nr::MMAP            => sys_mmap(tf),
        nr::MUNMAP          => sys_munmap(tf),
        nr::MPROTECT        => sys_mprotect(tf),
        nr::SIGNAL          => sys_signal(tf),
        nr::KILL            => sys_kill(tf),
        nr::SIGRET          => sys_sigret(tf),
        nr::GETCWD          => sys_getcwd(tf),
        nr::CHDIR           => sys_chdir(tf),
        nr::PIPE            => sys_pipe(tf),
        _                   => err::NOSYS,
    };
    (*tf).rax = ret as u64;
}

// ─────────────────────────────────────────────────────────────────────────────
// Implementacje — istniejące (bez zmian)
// ─────────────────────────────────────────────────────────────────────────────

unsafe fn sys_exit(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR, TS, NTHREADS, schedule};
    use core::sync::atomic::Ordering;
    let c = CUR.load(Ordering::Relaxed);
    THREADS[c].state = TS::Dead;
    NTHREADS.fetch_sub(1, Ordering::Relaxed);
    schedule();
    err::OK
}

unsafe fn sys_write(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use crate::debug::{putc, VGA_LOCK};
    use core::sync::atomic::Ordering;

    let fd  = (*tf).rdi;
    let ptr = (*tf).rsi;
    let len = (*tf).rdx as usize;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if fd != 1 && fd != 2 { return err::INVAL; }
    if len == 0 { return err::OK; }
    if len > 65536 { return err::INVAL; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    VGA_LOCK.lock();
    for i in 0..len { putc(*(ptr as *const u8).add(i) as char); }
    VGA_LOCK.unlock();

    len as i64
}

unsafe fn sys_read(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let fd  = (*tf).rdi;
    let ptr = (*tf).rsi;
    let len = (*tf).rdx as usize;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if fd != 0 { return err::INVAL; }
    if len == 0 { return err::OK; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    let mut n = 0usize;
    let buf = core::slice::from_raw_parts_mut(ptr as *mut u8, len);
    while n < len {
        match crate::perm::kb_pop() {
            Some(c) => { buf[n] = c as u8; n += 1; if c == '\n' { break; } }
            None    => break,
        }
    }
    if n == 0 { err::AGAIN } else { n as i64 }
}

unsafe fn sys_yield(_tf: *mut crate::perm::TF) -> i64 {
    crate::threading::thread_yield();
    err::OK
}

unsafe fn sys_spawn(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;
    let sz  = core::mem::size_of::<SpawnArgs>();
    if !valid_buf(p4, ptr, sz) { return err::FAULT; }

    let args  = &*(ptr as *const SpawnArgs);
    let name  = core::str::from_utf8(&args.name)
        .unwrap_or("thread")
        .trim_end_matches('\0');

    if args.flags & spawn_flags::USER != 0 {
        let new_cr3 = crate::mm::new_user_p4();
        let tid = crate::threading::spawn_user_on_cr3(name, args.entry, args.arg, new_cr3);
        if tid >= 0 { tid as i64 } else { err::NOSLOT }
    } else {
        err::PERM
    }
}

unsafe fn sys_sleep(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR, TS};
    use core::sync::atomic::Ordering;

    let ticks = (*tf).rdi;
    if ticks == 0 { return err::OK; }

    let c = CUR.load(Ordering::Relaxed);
    THREADS[c].wake_tick = crate::perm::TICK + ticks;
    THREADS[c].state = TS::Block;
    crate::threading::thread_yield();
    err::OK
}

unsafe fn sys_mem_alloc(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{mm_alloc, vmap, PTE_W, PTE_U, PAGE_SIZE};
    use core::sync::atomic::Ordering;

    let pages = (*tf).rdi as usize;
    let hint  = (*tf).rsi;

    if pages == 0 || pages > 512 { return err::INVAL; }

    let p4    = THREADS[CUR.load(Ordering::Relaxed)].cr3;
    let base  = if hint != 0 && hint >= 0x1000 { hint } else { 0x1000_0000u64 };
    let vbase = (base + 0xFFF) & !0xFFF;

    for i in 0..pages {
        let vaddr = vbase + i as u64 * PAGE_SIZE as u64;
        let phys  = mm_alloc();
        if vmap(p4, vaddr, phys, PTE_W | PTE_U) != 0 {
            return if i > 0 { vbase as i64 } else { err::NOMEM };
        }
        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
    }
    vbase as i64
}

unsafe fn sys_mem_free(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{virt_to_phys, vunmap, mm_free_phys, PAGE_SIZE};
    use core::sync::atomic::Ordering;

    let ptr   = (*tf).rdi;
    let pages = (*tf).rsi as usize;

    if ptr == 0 || ptr & 0xFFF != 0 { return err::INVAL; }
    if pages == 0 || pages > 512    { return err::INVAL; }

    let p4 = THREADS[CUR.load(Ordering::Relaxed)].cr3;
    for i in 0..pages {
        let vaddr = ptr + i as u64 * PAGE_SIZE as u64;
        if let Some(phys) = virt_to_phys(p4, vaddr) {
            mm_free_phys(phys);
            vunmap(p4, vaddr);
        }
    }
    err::OK
}

unsafe fn sys_thread_id(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let c   = CUR.load(Ordering::Relaxed);
    let p4  = THREADS[c].cr3;

    if ptr != 0 {
        if !valid_buf(p4, ptr, core::mem::size_of::<ThreadInfo>()) { return err::FAULT; }
        let info = &mut *(ptr as *mut ThreadInfo);
        info.tid   = THREADS[c].id;
        info.prio  = THREADS[c].prio;
        info.state = THREADS[c].state as u8;
        info._pad  = [0; 2];
    }
    THREADS[c].id as i64
}

unsafe fn sys_time(tf: *mut crate::perm::TF) -> i64 {
    use crate::mm::valid_buf;
    use crate::threading::{THREADS, CUR};
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if ptr != 0 {
        if !valid_buf(p4, ptr, core::mem::size_of::<TimeInfo>()) { return err::FAULT; }
        let info = &mut *(ptr as *mut TimeInfo);
        info.ticks  = crate::perm::TICK;
        info.uptime = crate::perm::TICK / 100;
    }
    crate::perm::TICK as i64
}

unsafe fn sys_debug_print(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use crate::debug::serial_print;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rsi;
    let len = (*tf).rdx as usize;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if len == 0 { return err::OK; }
    if len > 4096 { return err::INVAL; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    let bytes = core::slice::from_raw_parts(ptr as *const u8, len);
    if let Ok(s) = core::str::from_utf8(bytes) {
        serial_print("[US] ");
        serial_print(s);
    }
    len as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Implementacje — nowe syscalle
// ─────────────────────────────────────────────────────────────────────────────

/// THREAD_SET_PRIO — rdi=nowy_priorytet (0=najwyższy, 255=najniższy)
/// Zmienia priorytet bieżącego wątku. Wyższe wartości = niższy priorytet.
unsafe fn sys_thread_set_prio(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use core::sync::atomic::Ordering;

    let prio = (*tf).rdi;
    if prio > 255 { return err::INVAL; }

    let c = CUR.load(Ordering::Relaxed);
    THREADS[c].prio = prio as u8;
    err::OK
}

/// WAIT — rdi=tid (wątek do oczekiwania, u32)
/// Blokuje bieżący wątek dopóki wskazany wątek nie skończy się (TS::Dead).
/// Zwraca kod wyjścia wątku (na razie zawsze 0) lub err::INVAL jeśli TID nie istnieje.
unsafe fn sys_wait(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR, TS, MAX_THREADS};
    use core::sync::atomic::Ordering;

    let tid = (*tf).rdi as usize;
    if tid >= MAX_THREADS { return err::INVAL; }

    // Natychmiastowy powrót jeśli cel już martwy lub nigdy nie żył
    if THREADS[tid].state == TS::Dead { return err::OK; }

    // Aktywne czekanie (spin + yield) — bez dedykowanej listy wait
    // W przyszłości: lista waiterów w Thread
    loop {
        crate::threading::thread_yield();
        if THREADS[tid].state == TS::Dead { return err::OK; }
    }
}

// ── Pliki / deskryptory ───────────────────────────────────────────────────────
//
// Pełna implementacja wymaga VFS (patrz diagram: Filesystem → LibVFS).
// Poniżej są kompletne stuby z właściwą walidacją argumentów i kodami błędów,
// gotowe do podłączenia pod rzeczywiste sterowniki filesystemu.

/// OPEN — rdi=ptr_path, rsi=len_path, rdx=flags (open_flags)
/// Zwraca fd (>= 3, bo 0/1/2 = stdin/stdout/stderr) lub kod błędu.
unsafe fn sys_open(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let path_ptr = (*tf).rdi;
    let path_len = (*tf).rsi as usize;
    let _flags   = (*tf).rdx as u32;
    let p4       = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if path_len == 0 || path_len > 4096 { return err::INVAL; }
    if !valid_buf(p4, path_ptr, path_len) { return err::FAULT; }

    // TODO: przekaż ścieżkę do VFS gdy będzie gotowy
    // let path = core::slice::from_raw_parts(path_ptr as *const u8, path_len);
    // crate::vfs::open(path, flags)
    err::NOSUP
}

/// CLOSE — rdi=fd
unsafe fn sys_close(tf: *mut crate::perm::TF) -> i64 {
    let fd = (*tf).rdi;
    if fd < 3 { return err::INVAL; }   // stdin/stdout/stderr niezmykalne przez userspace
    // TODO: crate::vfs::close(fd)
    err::NOSUP
}

/// SEEK — rdi=fd, rsi=offset (i64), rdx=whence (seek_whence)
/// Zwraca nową pozycję w pliku lub kod błędu.
unsafe fn sys_seek(tf: *mut crate::perm::TF) -> i64 {
    let fd     = (*tf).rdi;
    let _off   = (*tf).rsi as i64;
    let whence = (*tf).rdx as u32;

    if fd < 3 { return err::INVAL; }
    if whence > seek_whence::END { return err::INVAL; }
    // TODO: crate::vfs::seek(fd, off, whence)
    err::NOSUP
}

/// FSTAT — rdi=fd, rsi=ptr_FileStat
unsafe fn sys_fstat(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let fd  = (*tf).rdi;
    let ptr = (*tf).rsi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if fd < 3 { return err::INVAL; }
    if !valid_buf(p4, ptr, core::mem::size_of::<FileStat>()) { return err::FAULT; }
    // TODO: crate::vfs::fstat(fd, &mut *(ptr as *mut FileStat))
    err::NOSUP
}

/// IOCTL — rdi=fd, rsi=request, rdx=arg
/// Kanał do sterowników urządzeń. Numery request definiują poszczególne sterowniki.
unsafe fn sys_ioctl(tf: *mut crate::perm::TF) -> i64 {
    let fd      = (*tf).rdi;
    let request = (*tf).rsi;
    let _arg    = (*tf).rdx;

    if fd < 3 { return err::INVAL; }
    // TODO: przekaż do device manager / devspace
    // crate::devspace::ioctl(fd, request, arg)
    let _ = request;
    err::NOSUP
}

// ── Wirtualna pamięć ──────────────────────────────────────────────────────────

/// MMAP — rdi=ptr_MmapArgs
///
/// Mapuje obszar pamięci wirtualnej dla bieżącego procesu.
/// Używa valloc.rs jako allocatora adresów wirtualnych.
/// Zwraca adres wirtualny lub kod błędu.
unsafe fn sys_mmap(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{valid_buf, mm_alloc, vmap, PAGE_SIZE, PTE_W, PTE_U};
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let c   = CUR.load(Ordering::Relaxed);
    let p4  = THREADS[c].cr3;

    if !valid_buf(p4, ptr, core::mem::size_of::<MmapArgs>()) { return err::FAULT; }
    let args = &*(ptr as *const MmapArgs);

    // Odrzuć file-backed mapowania dopóki VFS nie gotowy
    if args.fd >= 0 { return err::NOSUP; }

    if args.length == 0 { return err::INVAL; }
    // Maksymalnie 64MB naraz
    if args.length > 64 * 1024 * 1024 { return err::INVAL; }

    // Przelicz prot → PTE flags
    let mut pte_flags: u64 = PTE_U;
    if args.prot & mmap_prot::WRITE != 0 { pte_flags |= PTE_W; }
    // EXEC: na razie brak NX bitu w PTE, więc wszystko jest wykonywalny
    // TODO: ustawić NX gdy włączymy EFER.NXE

    // Wybór adresu
    let pages  = ((args.length as usize) + PAGE_SIZE - 1) / PAGE_SIZE;
    let vbase  = if args.flags & mmap_flags::FIXED != 0 && args.hint >= 0x1000 {
        // Wymuś dokładny adres — wyrównaj do strony
        if args.hint & (PAGE_SIZE as u64 - 1) != 0 { return err::ALIGN; }
        args.hint
    } else {
        // Zapytaj valloc o wolny zakres
        match crate::valloc::valloc_alloc(&mut THREADS[c].valloc, pages) {
            Some(a) => a,
            None    => return err::NOMEM,
        }
    };

    // Mapuj strony
    for i in 0..pages {
        let vaddr = vbase + i as u64 * PAGE_SIZE as u64;
        let phys  = mm_alloc();
        core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
        if vmap(p4, vaddr, phys, pte_flags) != 0 {
            // Cofnij już zmapowane
            for j in 0..i {
                let va = vbase + j as u64 * PAGE_SIZE as u64;
                if let Some(ph) = crate::mm::virt_to_phys(p4, va) {
                    crate::mm::mm_free_phys(ph);
                    crate::mm::vunmap(p4, va);
                }
            }
            crate::valloc::valloc_free(&mut THREADS[c].valloc, vbase, pages);
            return err::NOMEM;
        }
    }

    vbase as i64
}

/// MUNMAP — rdi=addr, rsi=length
unsafe fn sys_munmap(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{virt_to_phys, vunmap, mm_free_phys, PAGE_SIZE};
    use core::sync::atomic::Ordering;

    let addr   = (*tf).rdi;
    let length = (*tf).rsi as usize;

    if addr == 0 || addr & (PAGE_SIZE as u64 - 1) != 0 { return err::ALIGN; }
    if length == 0 { return err::INVAL; }

    let c      = CUR.load(Ordering::Relaxed);
    let p4     = THREADS[c].cr3;
    let pages  = (length + PAGE_SIZE - 1) / PAGE_SIZE;

    for i in 0..pages {
        let vaddr = addr + i as u64 * PAGE_SIZE as u64;
        if let Some(phys) = virt_to_phys(p4, vaddr) {
            mm_free_phys(phys);
            vunmap(p4, vaddr);
        }
    }

    crate::valloc::valloc_free(&mut THREADS[c].valloc, addr, pages);
    err::OK
}

/// MPROTECT — rdi=addr, rsi=length, rdx=prot (mmap_prot)
/// Zmienia ochronę stron bez realokacji.
unsafe fn sys_mprotect(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::{virt_to_phys, vmap, PAGE_SIZE, PTE_U, PTE_W};
    use core::arch::asm;
    use core::sync::atomic::Ordering;

    let addr   = (*tf).rdi;
    let length = (*tf).rsi as usize;
    let prot   = (*tf).rdx as u32;

    if addr == 0 || addr & (PAGE_SIZE as u64 - 1) != 0 { return err::ALIGN; }
    if length == 0 { return err::INVAL; }

    let c     = CUR.load(Ordering::Relaxed);
    let p4    = THREADS[c].cr3;
    let pages = (length + PAGE_SIZE - 1) / PAGE_SIZE;

    let mut pte_flags: u64 = PTE_U;
    if prot & mmap_prot::WRITE != 0 { pte_flags |= PTE_W; }

    for i in 0..pages {
        let vaddr = addr + i as u64 * PAGE_SIZE as u64;
        if let Some(phys) = virt_to_phys(p4, vaddr) {
            // Przepisz PTE z nowymi flagami (vmap nadpisze istniejący wpis)
            vmap(p4, vaddr, phys, pte_flags);
            // TLB flush dla tej strony już wykonuje vmap przez invlpg
        }
    }

    err::OK
}

// ── Sygnały ───────────────────────────────────────────────────────────────────

/// SIGNAL — rdi=signum, rsi=handler_ptr (0 = domyślny = zakończ wątek)
/// Rejestruje handler sygnału dla bieżącego wątku.
unsafe fn sys_signal(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use core::sync::atomic::Ordering;

    let signum  = (*tf).rdi as u32;
    let handler = (*tf).rsi;   // adres funkcji userspace lub 0

    if signum == 0 || signum >= sig::MAX { return err::INVAL; }
    if signum == sig::KILL { return err::PERM; }  // SIGKILL niezłapywalny

    let c = CUR.load(Ordering::Relaxed);
    // Zapisz handler w tablicy sygnałów wątku
    THREADS[c].sig_handlers[signum as usize] = handler;
    err::OK
}

/// KILL — rdi=tid, rsi=signum
/// Wysyła sygnał do wskazanego wątku.
unsafe fn sys_kill(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR, TS, MAX_THREADS};
    use core::sync::atomic::Ordering;

    let tid    = (*tf).rdi as usize;
    let signum = (*tf).rsi as u32;

    if tid >= MAX_THREADS  { return err::INVAL; }
    if signum == 0 || signum >= sig::MAX { return err::INVAL; }

    let target = &mut THREADS[tid];
    if target.state == TS::Dead { return err::INVAL; }

    // SIGKILL: natychmiastowe zakończenie
    if signum == sig::KILL {
        target.state = TS::Dead;
        crate::threading::NTHREADS.fetch_sub(1, Ordering::Relaxed);
        return err::OK;
    }

    // Pozostałe sygnały: ustaw pending bit w wątku
    if signum < 64 {
        target.sig_pending |= 1u64 << signum;
    }

    // Obudź jeśli blokował
    if target.state == TS::Block {
        target.state = TS::Ready;
    }

    err::OK
}

/// SIGRET — powrót z handlera sygnału
/// Wywoływany przez trampoline sygnału w userspace.
/// Przywraca zapisany kontekst (SavedContext) ze stosu.
unsafe fn sys_sigret(_tf: *mut crate::perm::TF) -> i64 {
    // TODO: przywróć SavedContext ze stosu userspace wątku
    // Na razie stub — pełna implementacja przy dodaniu signal delivery
    err::NOSUP
}

// ── Filesystem ────────────────────────────────────────────────────────────────

/// GETCWD — rdi=ptr_buf, rsi=buf_len
/// Wypełnia bufor bieżącą ścieżką roboczą wątku.
unsafe fn sys_getcwd(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let len = (*tf).rsi as usize;
    let c   = CUR.load(Ordering::Relaxed);
    let p4  = THREADS[c].cr3;

    if len == 0 { return err::INVAL; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    // Skopiuj cwd wątku do bufora userspace
    let cwd = &THREADS[c].cwd;
    let cwd_len = cwd.iter().position(|&b| b == 0).unwrap_or(cwd.len());
    if cwd_len + 1 > len { return err::OVERFLOW; }

    let dst = core::slice::from_raw_parts_mut(ptr as *mut u8, len);
    dst[..cwd_len].copy_from_slice(&cwd[..cwd_len]);
    dst[cwd_len] = 0;

    cwd_len as i64
}

/// CHDIR — rdi=ptr_path, rsi=len_path
unsafe fn sys_chdir(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let len = (*tf).rsi as usize;
    let c   = CUR.load(Ordering::Relaxed);
    let p4  = THREADS[c].cr3;

    if len == 0 || len > 255 { return err::INVAL; }
    if !valid_buf(p4, ptr, len) { return err::FAULT; }

    let src = core::slice::from_raw_parts(ptr as *const u8, len);
    let cwd = &mut THREADS[c].cwd;
    cwd[..len].copy_from_slice(src);
    if len < cwd.len() { cwd[len] = 0; }

    // TODO: walidacja ścieżki przez VFS gdy gotowy
    err::OK
}

// ── Pipe ─────────────────────────────────────────────────────────────────────

/// PIPE — rdi=ptr_PipeFds
/// Tworzy anonimowy pipe: para fd [read_fd, write_fd].
/// Dane przepływają przez ring buffer w kernelu (bez alokacji heap).
unsafe fn sys_pipe(tf: *mut crate::perm::TF) -> i64 {
    use crate::threading::{THREADS, CUR};
    use crate::mm::valid_buf;
    use core::sync::atomic::Ordering;

    let ptr = (*tf).rdi;
    let c   = CUR.load(Ordering::Relaxed);
    let p4  = THREADS[c].cr3;

    if !valid_buf(p4, ptr, core::mem::size_of::<PipeFds>()) { return err::FAULT; }

    // TODO: crate::pipe::pipe_create() → (read_fd, write_fd)
    // Stub — pipe będzie potrzebował tabeli fd per-thread w Thread
    let _ = ptr;
    err::NOSUP
}