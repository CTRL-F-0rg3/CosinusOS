// CosinusOS — threading.rs
// Kernel thread scheduler and context switcher.
//
// Threads:
//   • MAX_THREADS slots — statically allocated, no heap needed at boot
//   • Two kinds: kernel threads (ring-0) and user threads (ring-3)
//   • Priority-based round-robin; lower prio value = higher priority
//
// Context switch:
//   • Kernel → kernel : thread_switch() saves/restores callee-saved regs on kstack
//   • Kernel → user   : enter_userspace() via iretq (tramp_u sets up the frame)
//
// Stacks:
//   • Kernel stack : mapped at 0x0200_0000 + slot * (KERNEL_STACK_SIZE + PAGE_SIZE)
//   • User stack   : mapped at 0x0400_0000 + slot * (USER_STACK_SIZE  + PAGE_SIZE)
//   • One guard page (unmapped) sits before each stack.

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::Spinlock;
use crate::mm::{
    VirtAddr, PhysAddr, PTE_W, PTE_U, K_P4, PAGE_SIZE,
    KERNEL_STACK_SIZE, USER_STACK_SIZE,
    vmap, mm_alloc,
};
use crate::debug::{serial_print, serial_hex, print, num_str};
use crate::perm::tss_rsp0;

pub const MAX_THREADS: usize = 64;

// ── Syscall entry trampoline ──────────────────────────────────────────────────

/// int 0x80 handler — saves all GPRs, calls syscall_dispatch_v2, restores and iretq.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_handler() {
    core::arch::naked_asm!(
        "push rax","push rbp","push rbx","push rcx","push rdx",
        "push rsi","push rdi","push r8","push r9","push r10",
        "push r11","push r12","push r13","push r14","push r15",
        "mov rdi, rsp",
        "call {f}",
        "pop r15","pop r14","pop r13","pop r12","pop r11","pop r10",
        "pop r9","pop r8","pop rdi","pop rsi","pop rdx","pop rcx",
        "pop rbx","pop rbp","pop rax",
        "iretq",
        f = sym crate::syscall_api::syscall_dispatch_v2,
    );
}

/// Legacy syscall dispatcher (kept for compatibility).
pub unsafe extern "C" fn syscall_dispatch(num: u64, arg1: u64, arg2: u64, arg3: u64) {
    match num {
        1 => {
            // WRITE — print a user string to the VGA console
            let ptr = arg2 as *const u8;
            let len = arg3 as usize;
            if !ptr.is_null() && len < 65536 {
                let s = core::slice::from_raw_parts(ptr, len);
                if let Ok(text) = core::str::from_utf8(s) {
                    crate::debug::print(text);
                }
            }
        }
        0 => {
            // EXIT — mark current thread dead and reschedule
            let c = CUR.load(Ordering::Relaxed);
            THREADS[c].state = TS::Dead;
            NTHREADS.fetch_sub(1, Ordering::Relaxed);
            schedule();
        }
        _ => {}
    }
}

// ── Assembly trampolines (defined in tramp.asm) ───────────────────────────────

unsafe extern "C" {
    pub fn tramp_u(); // ring-3 entry trampoline
    pub fn tramp_k(); // ring-0 entry trampoline
}

// ── Thread state ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum TS { Run, Ready, Block, Dead }

// ── Thread control block ──────────────────────────────────────────────────────

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Thread {
    pub wake_tick:    u64,        // tick at which a sleeping thread wakes up
    pub id:           u32,
    pub state:        TS,
    pub prio:         u8,         // priority: lower = higher precedence
    pub krsp:         VirtAddr,   // saved kernel RSP (context-switch save slot)
    pub ktop:         VirtAddr,   // top of kernel stack (initial RSP / TSS RSP0)
    pub utop:         VirtAddr,   // top of user stack
    pub cr3:          PhysAddr,   // page table root
    pub name:         [u8; 16],
    pub ticks:        u64,        // total scheduler ticks consumed
    pub sig_handlers: [u64; 32],  // signal handler function pointers
    pub sig_pending:  u64,        // bitmask of pending signals
    pub cwd:          [u8; 256],  // current working directory (null-terminated)
}

impl Thread {
    pub const fn new() -> Self {
        Self {
            id: 0, state: TS::Dead, prio: 10,
            krsp: 0, ktop: 0, utop: 0, cr3: 0,
            name: [0; 16], ticks: 0, wake_tick: 0,
            sig_handlers: [0u64; 32],
            sig_pending:  0,
            cwd: { let mut a = [0u8; 256]; a[0] = b'/'; a },
        }
    }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        unsafe { core::str::from_utf8_unchecked(&self.name[..end]) }
    }
}

// ── Global thread table ───────────────────────────────────────────────────────

pub static mut THREADS: [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS];
pub static CUR:         AtomicUsize           = AtomicUsize::new(0);
pub static NTHREADS:    AtomicUsize           = AtomicUsize::new(0);
static SCHED_LOCK:      Spinlock              = Spinlock::new();

// ── Initialisation ────────────────────────────────────────────────────────────

/// Spawn the idle thread and make it the current running thread.
pub unsafe fn sched_init() {
    let tid = spawn_k("idle\0", idle as *const () as u64, 0);
    if tid >= 0 {
        THREADS[tid as usize].state = TS::Run;
        CUR.store(tid as usize, Ordering::SeqCst);
        tss_rsp0(THREADS[tid as usize].ktop);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub unsafe fn set_name(t: &mut Thread, name: &str) {
    let b = name.as_bytes();
    for j in 0..core::cmp::min(15, b.len()) { t.name[j] = b[j]; }
}

// ── Kernel thread spawn ───────────────────────────────────────────────────────

/// Spawn a kernel-mode (ring-0) thread.
/// Maps a kernel stack, initialises the thread control block, and marks Ready.
pub unsafe fn spawn_k(name: &str, entry: u64, arg: u64) -> i32 {
    asm!("cli", options(nomem, nostack));
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Dead { continue; }
        let t = &mut THREADS[i];

        // Kernel stack layout: one guard page, then KERNEL_STACK_SIZE bytes
        let ks = 0x0200_0000u64
            + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64
            + PAGE_SIZE as u64; // skip guard page
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            vmap(K_P4, ks + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64; // stack top

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

// ── User thread spawn ─────────────────────────────────────────────────────────

/// Spawn a user-mode (ring-3) thread in the given address space `cr3`.
/// Maps both a kernel stack (for syscall/interrupt handling) and a user stack.
pub unsafe fn spawn_user_on_cr3(name: &str, entry: u64, arg: u64, cr3: PhysAddr) -> i32 {
    asm!("cli", options(nomem, nostack));
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Dead { continue; }
        let t = &mut THREADS[i];

        // Kernel stack (ring-0 for this thread)
        let ks = 0x0200_0000u64
            + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64
            + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            vmap(K_P4, ks + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64;

        // User stack (ring-3, mapped into the process address space)
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

        set_name(t, name);
        t.state = TS::Ready;
        NTHREADS.fetch_add(1, Ordering::Relaxed);

        let mut buf = [0u8; 24];
        print("  [T#"); print(num_str(i, &mut buf)); print("] "); print(name); print("\n");
        return i as i32;
    }
    -1
}

// ── Initial kernel stack frame ────────────────────────────────────────────────

/// Build the initial stack frame so that thread_switch() can "return" into
/// the new thread as if it had been context-switched out normally.
///
/// Frame layout (high → low, pushed in order):
///   [user only] SS / user RSP / RFLAGS / CS / RIP  (iretq frame for tramp_u)
///               return address (tramp_u or tramp_k)
///               rbx / rbp / r12 / r13 / r14 / r15
fn init_thread_stack(
    t:     &mut Thread,
    kt:    VirtAddr,  // kernel stack top
    ut:    VirtAddr,  // user stack top (== kt for kernel threads)
    entry: u64,
    arg:   u64,
    user:  bool,
) {
    let mut ksp = kt;
    unsafe {
        if user {
            // iretq frame — tramp_u will execute iretq to enter ring-3
            ksp -= 8; *(ksp as *mut u64) = 0x23;  // SS  (ring-3 data selector)
            ksp -= 8; *(ksp as *mut u64) = ut;     // RSP (user stack top)
            ksp -= 8; *(ksp as *mut u64) = 0x202;  // RFLAGS (IF=1, reserved bit 1)
            ksp -= 8; *(ksp as *mut u64) = 0x1B;   // CS  (ring-3 code selector)
            ksp -= 8; *(ksp as *mut u64) = entry;  // RIP

            ksp -= 8; *(ksp as *mut u64) = tramp_u as *const () as u64;
        } else {
            ksp -= 8; *(ksp as *mut u64) = tramp_k as *const () as u64;
        }
        // Callee-saved registers (thread_switch pops these on first switch-in)
        ksp -= 8; *(ksp as *mut u64) = 0u64;   // rbx
        ksp -= 8; *(ksp as *mut u64) = 0u64;   // rbp
        ksp -= 8; *(ksp as *mut u64) = 0u64;   // r12
        ksp -= 8; *(ksp as *mut u64) = ut;     // r13 — user stack top for tramp_u
        ksp -= 8; *(ksp as *mut u64) = entry;  // r14 — entry point for tramp_u
        ksp -= 8; *(ksp as *mut u64) = arg;    // r15 — argument
    }
    t.krsp = ksp;
}

// ── Scheduler ─────────────────────────────────────────────────────────────────

/// Round-robin scheduler with priority selection.
/// Wakes sleeping threads whose wake_tick has elapsed, then picks the next
/// Ready thread. If the chosen thread is a user-space thread, enters it
/// directly via enter_userspace(); otherwise performs a kernel context switch.
pub unsafe fn schedule() {
    // Non-reentrant — bail if already inside schedule()
    if SCHED_LOCK.locked.swap(true, Ordering::Acquire) { return; }

    // Wake any sleeping threads whose deadline has passed
    let cur_tick = crate::perm::TICK;
    for i in 0..MAX_THREADS {
        let t = &mut THREADS[i];
        if t.state == TS::Block && t.wake_tick != 0 && cur_tick >= t.wake_tick {
            t.wake_tick = 0;
            t.state = TS::Ready;
        }
    }

    let cur  = CUR.load(Ordering::Relaxed);
    let mut next = cur;

    // Find the next Ready thread (simple round-robin)
    for _ in 0..MAX_THREADS {
        next = (next + 1) % MAX_THREADS;
        if THREADS[next].state == TS::Ready { break; }
    }

    // Nothing to switch to
    if next == cur && THREADS[cur].state == TS::Run {
        SCHED_LOCK.locked.store(false, Ordering::Release);
        return;
    }

    if THREADS[cur].state == TS::Run { THREADS[cur].state = TS::Ready; }
    THREADS[next].state = TS::Run;
    THREADS[next].ticks += 1;
    CUR.store(next, Ordering::SeqCst);

    // Update TSS RSP0 — where the CPU will save the stack pointer on ring-3 → ring-0
    if THREADS[next].cr3 == K_P4 || THREADS[next].cr3 == 0 {
        tss_rsp0(THREADS[next].ktop);
    } else {
        tss_rsp0(crate::perm::irq_stack_top());
    }

    // Switch address space if needed
    let ncr3 = THREADS[next].cr3;
    let ccr3: u64;
    asm!("mov {}, cr3", out(reg) ccr3, options(nomem, nostack));
    if ncr3 != 0 && ncr3 != ccr3 {
        asm!("mov cr3, {}", in(reg) ncr3, options(nostack));
    }

    SCHED_LOCK.locked.store(false, Ordering::Release);

    serial_print("[SCHED] cur=");  serial_hex(cur as u64);
    serial_print(" next=");        serial_hex(next as u64);
    serial_print(" new_krsp=");    serial_hex(THREADS[next].krsp);
    serial_print("\n");

    // If the next thread is a user-space thread, enter it via iretq
    if THREADS[next].cr3 != 0 && THREADS[next].cr3 != K_P4 {
        let entry = crate::userspace_loader::US_ENTRY;
        let stack = crate::userspace_loader::US_STACK;
        let cr3   = THREADS[next].cr3;
        serial_print("[SCHED] -> userspace\n");
        SCHED_LOCK.locked.store(false, Ordering::Release);
        if cr3 != 0 { asm!("mov cr3, {}", in(reg) cr3, options(nostack)); }
        enter_userspace(entry, stack, 0, cr3);
    }

    // Kernel thread context switch
    thread_switch(&mut THREADS[cur].krsp as *mut u64, THREADS[next].krsp);
}

// ── Foreign functions ─────────────────────────────────────────────────────────

unsafe extern "C" {
    /// Switch to user space via iretq.
    /// Defined in enter_userspace.asm.
    pub fn enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64) -> !;
}

// ── Boot jump to first thread ─────────────────────────────────────────────────

/// Called once at the end of kernel initialisation.
/// Picks the highest-priority Ready thread and jumps into it without saving
/// any kernel context (there is no thread to return to).
pub unsafe fn jump_to_scheduler() -> ! {
    let mut best      = usize::MAX;
    let mut best_prio = u8::MAX;
    for i in 0..MAX_THREADS {
        if THREADS[i].state == TS::Ready && THREADS[i].prio <= best_prio {
            best_prio = THREADS[i].prio;
            best = i;
        }
    }
    if best == usize::MAX { best = 0; }

    THREADS[best].state = TS::Run;
    THREADS[best].ticks += 1;
    CUR.store(best, Ordering::SeqCst);
    tss_rsp0(THREADS[best].ktop);

    let ncr3 = THREADS[best].cr3;
    if ncr3 != 0 { asm!("mov cr3, {}", in(reg) ncr3, options(nostack)); }

    serial_print("[BOOT] -> #");
    { let mut b = [0u8; 24]; serial_print(num_str(best, &mut b)); }
    serial_print("\n");

    let krsp = THREADS[best].krsp;
    asm!(
        "sti",
        "mov rsp, {k}",
        "pop r15","pop r14","pop r13","pop r12","pop rbp","pop rbx",
        "ret",
        k = in(reg) krsp,
        options(noreturn),
    );
}

pub unsafe fn thread_yield() { schedule(); }

// ── Low-level context switch ──────────────────────────────────────────────────

/// Save callee-saved registers onto the current stack, store RSP into `*old`,
/// load RSP from `new`, and restore callee-saved registers from the new stack.
#[unsafe(naked)]
unsafe extern "C" fn thread_switch(old: *mut VirtAddr, new: VirtAddr) {
    core::arch::naked_asm!(
        "push rbx", "push rbp", "push r12", "push r13", "push r14", "push r15",
        "mov [rdi], rsp",   // save current RSP into *old
        "mov rsp, rsi",     // load new RSP
        "pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx",
        "ret",
    );
}

// ── Idle thread ───────────────────────────────────────────────────────────────

unsafe extern "C" fn idle(_: u64) -> ! {
    loop { asm!("hlt", options(nomem, nostack)); }
}