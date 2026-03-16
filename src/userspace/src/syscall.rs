// CosinusOS Userspace — syscall.rs
// Interfejs syscalli zgodny z kernel/syscall_api.rs nr::*

// Numery — identyczne z kernel/syscall_api.rs::nr
pub mod nr {
    pub const EXIT:          u64 = 0;
    pub const WRITE:         u64 = 1;
    pub const READ:          u64 = 2;
    pub const YIELD:         u64 = 3;
    pub const SPAWN:         u64 = 4;
    pub const SLEEP:         u64 = 5;
    pub const MEM_ALLOC:     u64 = 6;
    pub const MEM_FREE:      u64 = 7;
    pub const IPC_SEND:      u64 = 8;
    pub const IPC_RECV:      u64 = 9;
    pub const IPC_POLL:      u64 = 10;
    pub const THREAD_ID:     u64 = 11;
    pub const TIME:          u64 = 12;
    pub const DEBUG_PRINT:   u64 = 13;
    pub const THREAD_SET_PRIO: u64 = 14;
    pub const WAIT:          u64 = 15;
    pub const OPEN:          u64 = 20;
    pub const CLOSE:         u64 = 21;
    pub const SEEK:          u64 = 22;
    pub const FSTAT:         u64 = 23;
    pub const IOCTL:         u64 = 24;
    pub const MMAP:          u64 = 30;
    pub const MUNMAP:        u64 = 31;
    pub const MPROTECT:      u64 = 32;
    pub const SIGNAL:        u64 = 40;
    pub const KILL:          u64 = 41;
    pub const SIGRET:        u64 = 42;
    pub const GETCWD:        u64 = 50;
    pub const CHDIR:         u64 = 51;
    pub const PIPE:          u64 = 60;
}

// Kody błędów — identyczne z kernel/syscall_api.rs::err
pub mod err {
    pub const OK:       i64 =  0;
    pub const INVAL:    i64 = -1;
    pub const NOMEM:    i64 = -2;
    pub const NOSLOT:   i64 = -3;
    pub const FAULT:    i64 = -4;
    pub const AGAIN:    i64 = -5;
    pub const NOSYS:    i64 = -6;
    pub const PERM:     i64 = -7;
    pub const NOENT:    i64 = -8;
    pub const BADF:     i64 = -9;
    pub const BUSY:     i64 = -10;
    pub const OVERFLOW: i64 = -11;
    pub const NOSUP:    i64 = -12;
    pub const ALIGN:    i64 = -13;
    pub const EXIST:    i64 = -14;
}

// ── Niskopoziomowe wrappery ───────────────────────────────────────────────────

#[inline(always)]
pub unsafe fn syscall0(num: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") num as i64 => ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall1(num: u64, a1: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") num as i64 => ret,
        in("rdi") a1,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall2(num: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") num as i64 => ret,
        in("rdi") a1,
        in("rsi") a2,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") num as i64 => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        options(nostack, preserves_flags),
    );
    ret
}

// ── Wysokopoziomowe API ───────────────────────────────────────────────────────

/// Wypisz bajty na stdout/stderr (fd=1 lub 2)
pub fn write_fd(fd: u64, s: &str) {
    unsafe { syscall3(nr::WRITE, fd, s.as_ptr() as u64, s.len() as u64); }
}

pub fn print(s: &str)   { write_fd(1, s); }
pub fn println(s: &str) { print(s); print("\n"); }
pub fn eprint(s: &str)  { write_fd(2, s); }

/// Wypisz do serial debuggera kernela (COM1) — tylko dev
pub fn debug_print(s: &str) {
    unsafe { syscall3(nr::DEBUG_PRINT, 0, s.as_ptr() as u64, s.len() as u64); }
}

/// Zakończ bieżący wątek
pub fn exit(code: i32) -> ! {
    unsafe { syscall1(nr::EXIT, code as u64); }
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}

/// Oddaj czas procesora
pub fn sched_yield() {
    unsafe { syscall0(nr::YIELD); }
}

/// Uśpij na `ticks` ticków (100 ticków = 1s)
pub fn sleep_ticks(ticks: u64) {
    unsafe { syscall1(nr::SLEEP, ticks); }
}

/// Pobierz TID bieżącego wątku
pub fn thread_id() -> u32 {
    unsafe { syscall1(nr::THREAD_ID, 0) as u32 }
}

/// Pobierz licznik ticków od startu kernela
pub fn ticks() -> u64 {
    unsafe { syscall0(nr::TIME) as u64 }
}

/// Czytaj ze stdin (non-blocking). Zwraca liczbę bajtów lub err::AGAIN
pub fn read_stdin(buf: &mut [u8]) -> i64 {
    unsafe { syscall3(nr::READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Czytaj jedną linię ze stdin — blokuje przez yielding aż do '\n'
pub fn read_line(buf: &mut [u8]) -> usize {
    let mut total = 0usize;
    loop {
        if total >= buf.len() { break; }
        let r = read_stdin(&mut buf[total..total + 1]);
        if r == err::AGAIN as i64 {
            sched_yield();
            continue;
        }
        if r <= 0 { break; }
        let c = buf[total];
        total += 1;
        if c == b'\n' { break; }
    }
    total
}

/// Zmień priorytet bieżącego wątku (0=najwyższy, 255=najniższy)
pub fn set_priority(prio: u8) -> i64 {
    unsafe { syscall1(nr::THREAD_SET_PRIO, prio as u64) }
}

/// Czekaj na zakończenie wątku TID
pub fn wait_thread(tid: u32) -> i64 {
    unsafe { syscall1(nr::WAIT, tid as u64) }
}

/// Pobierz bieżący katalog roboczy
pub fn getcwd(buf: &mut [u8]) -> i64 {
    unsafe { syscall2(nr::GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Ustaw bieżący katalog roboczy
pub fn chdir(path: &str) -> i64 {
    unsafe { syscall2(nr::CHDIR, path.as_ptr() as u64, path.len() as u64) }
}

// ── fmt::Write wrapper — potrzebny przez print_fmt!/println_fmt! ──────────────
pub struct Writer;

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { print(s); Ok(()) }
}