// CosinusOS — threading.rs
// Wątki kernelowe i userspace, scheduler round-robin, thread_switch

use core::arch::{asm, naked_asm};
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::Spinlock;
use crate::mm::{VirtAddr, PhysAddr, PTE_W, PTE_U, K_P4, PAGE_SIZE, KERNEL_STACK_SIZE, USER_STACK_SIZE, vmap, mm_alloc, new_user_p4};
use crate::debug::{serial_print, serial_hex, print, num_str, hex_str};
use crate::perm::tss_rsp0;

// ── Stałe ────────────────────────────────────────────────────────────────────
pub const MAX_THREADS: usize = 64;

// ── Trampoliny (tramp.asm) ────────────────────────────────────────────────────
unsafe extern "C" {
    pub fn tramp_k();
    pub fn tramp_u();
}

// ── Stan wątku ───────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
pub enum TS { Run, Ready, Block, Dead }

// ── Struktura wątku ──────────────────────────────────────────────────────────
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Thread {
    pub id:    u32,
    pub state: TS,
    pub prio:  u8,
    pub krsp:  VirtAddr,
    pub ktop:  VirtAddr,
    pub utop:  VirtAddr,
    pub cr3:   PhysAddr,
    pub name:  [u8; 16],
    pub ticks: u64,
}

impl Thread {
    pub const fn new() -> Self {
        Self {
            id: 0, state: TS::Dead, prio: 10,
            krsp: 0, ktop: 0, utop: 0, cr3: 0,
            name: [0; 16], ticks: 0,
        }
    }
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        unsafe { core::str::from_utf8_unchecked(&self.name[..end]) }
    }
}

// ── Globals ──────────────────────────────────────────────────────────────────
pub static mut THREADS: [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS];
pub static CUR:         AtomicUsize           = AtomicUsize::new(0);
pub static NTHREADS:    AtomicUsize           = AtomicUsize::new(0);
static SCHED_LOCK:      Spinlock              = Spinlock::new();

// ── sched_init ───────────────────────────────────────────────────────────────
pub unsafe fn sched_init() {
    let tid = spawn_k("idle\0", idle as *const () as u64, 0);
    if tid >= 0 {
        THREADS[tid as usize].state = TS::Run;
        CUR.store(tid as usize, Ordering::SeqCst);
    }
}

// ── spawn_k: nowy wątek kernelowy ────────────────────────────────────────────
pub unsafe fn spawn_k(name: &str, entry: u64, arg: u64) -> i32 {
    asm!("cli", options(nomem, nostack));
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Dead { continue; }
        let t  = &mut THREADS[i];
        let ks = 0x0200_0000u64
            + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64
            + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            vmap(K_P4, ks + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64;
        t.id = i as u32; t.prio = 10;
        t.ktop = kt; t.utop = kt; t.cr3 = K_P4; t.ticks = 0;
        init_thread_stack(t, kt, kt, entry, arg, false);
        set_name(t, name);
        NTHREADS.fetch_add(1, Ordering::Relaxed);
        t.state = TS::Ready;
        let mut buf = [0u8; 24];
        print("  [T#"); print(num_str(i, &mut buf)); print("] "); print(name); print("\n");
        return i as i32;
    }
    -1
}

// ── spawn_user_on_cr3: nowy wątek userspace ──────────────────────────────────
pub unsafe fn spawn_user_on_cr3(name: &str, entry: u64, arg: u64, cr3: PhysAddr) -> i32 {
    asm!("cli", options(nomem, nostack));
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Dead { continue; }
        let t = &mut THREADS[i];

        // Kernel stack (mapowany w K_P4)
        let ks = 0x0200_0000u64
            + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64
            + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            vmap(K_P4, ks + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64;

        // User stack (mapowany w cr3 wątku z PTE_U)
        let us = 0x0400_0000u64
            + i as u64 * (USER_STACK_SIZE + PAGE_SIZE) as u64
            + PAGE_SIZE as u64;
        for p in 0..(USER_STACK_SIZE / PAGE_SIZE) {
            vmap(cr3, us + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W | PTE_U);
        }
        let ut = us + USER_STACK_SIZE as u64;

        t.id = i as u32; t.prio = 5;
        t.ktop = kt; t.utop = ut; t.cr3 = cr3; t.ticks = 0;
        init_thread_stack(t, kt, ut, entry, arg, true);

        // Debug: zweryfikuj adres trampoliny
        {
            let ksp           = t.krsp;
            let stack_r14     = *((ksp + 32) as *const u64);
            let stack_r13     = *((ksp + 24) as *const u64);
            let stack_tramp   = *((ksp + 48) as *const u64);
            let expected      = tramp_u as *const () as u64;
            serial_print("[DBG] krsp=");       serial_hex(ksp);
            serial_print(" kt=");              serial_hex(kt);
            serial_print("\n[DBG] r14(entry)="); serial_hex(stack_r14);
            serial_print(" r13(ut)=");         serial_hex(stack_r13);
            serial_print("\n[DBG] tramp=");    serial_hex(stack_tramp);
            serial_print(" expected=");        serial_hex(expected);
            serial_print(if stack_tramp == expected { " OK\n" } else { " MISMATCH!\n" });
        }

        set_name(t, name);
        t.state = TS::Ready;
        NTHREADS.fetch_add(1, Ordering::Relaxed);
        let mut buf = [0u8; 24];
        print("  [T#"); print(num_str(i, &mut buf)); print("] "); print(name); print("\n");
        return i as i32;
    }
    -1
}

// ── init_thread_stack ────────────────────────────────────────────────────────
// Layout stosu (thread_switch: pop rbx rbp r12 r13 r14 r15 ret):
//   [ksp+48] = tramp_addr  ← ret
//   [ksp+40] = arg         → r15
//   [ksp+32] = entry       → r14
//   [ksp+24] = ut          → r13
//   [ksp+16] = 0           → r12
//   [ksp+8 ] = 0           → rbp
//   [ksp+0 ] = 0           → rbx  ← t.krsp
fn init_thread_stack(t: &mut Thread, kt: VirtAddr, ut: VirtAddr, entry: u64, arg: u64, user: bool) {
    let tramp_addr: u64 = if user {
        unsafe { tramp_u as *const () as u64 }
    } else {
        unsafe { tramp_k as *const () as u64 }
    };

    let mut ksp = kt;
    unsafe {
        ksp -= 8; *(ksp as *mut u64) = tramp_addr;
        ksp -= 8; *(ksp as *mut u64) = arg;
        ksp -= 8; *(ksp as *mut u64) = entry;
        ksp -= 8; *(ksp as *mut u64) = ut;
        ksp -= 8; *(ksp as *mut u64) = 0u64;
        ksp -= 8; *(ksp as *mut u64) = 0u64;
        ksp -= 8; *(ksp as *mut u64) = 0u64;
    }
    t.krsp = ksp;

    unsafe {
        let on_stack = *((ksp + 48) as *const u64);
        if on_stack != tramp_addr {
            serial_print("[FATAL] init_thread_stack: tramp mismatch!\n");
        }
    }
}

pub unsafe fn set_name(t: &mut Thread, name: &str) {
    let b = name.as_bytes();
    for j in 0..core::cmp::min(15, b.len()) { t.name[j] = b[j]; }
}

// ── Scheduler ────────────────────────────────────────────────────────────────
pub unsafe fn schedule() {
    if SCHED_LOCK.locked.swap(true, Ordering::Acquire) { return; }
    let cur  = CUR.load(Ordering::Relaxed);
    let mut next = cur;
    for _ in 0..MAX_THREADS {
        next = (next + 1) % MAX_THREADS;
        if THREADS[next].state == TS::Ready { break; }
    }
    if next == cur && THREADS[cur].state == TS::Run {
        SCHED_LOCK.locked.store(false, Ordering::Release);
        return;
    }
    if THREADS[cur].state == TS::Run { THREADS[cur].state = TS::Ready; }
    THREADS[next].state = TS::Run;
    THREADS[next].ticks += 1;
    CUR.store(next, Ordering::SeqCst);
    tss_rsp0(THREADS[next].ktop);

    let ncr3 = THREADS[next].cr3;
    let ccr3: u64;
    asm!("mov {}, cr3", out(reg) ccr3, options(nomem, nostack));
    if ncr3 != 0 && ncr3 != ccr3 {
        asm!("mov cr3, {}", in(reg) ncr3, options(nostack));
    }
    SCHED_LOCK.locked.store(false, Ordering::Release);
    thread_switch(&mut THREADS[cur].krsp as *mut u64, THREADS[next].krsp);
}

pub unsafe fn thread_yield() { schedule(); }

// ── thread_switch (naked) ────────────────────────────────────────────────────
#[unsafe(naked)]
unsafe extern "C" fn thread_switch(old: *mut VirtAddr, new: VirtAddr) {
    naked_asm!(
        "push rbx", "push rbp", "push r12", "push r13", "push r14", "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx",
        "ret",
    );
}

// ── Idle thread ──────────────────────────────────────────────────────────────
unsafe extern "C" fn idle(_: u64) -> ! {
    loop { asm!("hlt", options(nomem, nostack)); }
}