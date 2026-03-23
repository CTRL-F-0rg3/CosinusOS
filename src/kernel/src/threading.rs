// CosinusOS — threading.rs
use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::Spinlock;
use crate::mm::{VirtAddr, PhysAddr, PTE_W, PTE_U, K_P4, PAGE_SIZE, KERNEL_STACK_SIZE, USER_STACK_SIZE, vmap, mm_alloc};
use crate::debug::{serial_print, serial_hex, print, num_str};
use crate::perm::tss_rsp0;

pub const MAX_THREADS: usize = 64;

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

pub unsafe extern "C" fn syscall_dispatch(num: u64, arg1: u64, arg2: u64, arg3: u64) {
    match num {
        1 => {
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
            let c = CUR.load(Ordering::Relaxed);
            THREADS[c].state = TS::Dead;
            NTHREADS.fetch_sub(1, Ordering::Relaxed);
            schedule();
        }
        _ => {}
    }
}

unsafe extern "C" {
    pub fn tramp_u();
    pub fn tramp_k();
}

#[derive(Clone, Copy, PartialEq)]
pub enum TS { Run, Ready, Block, Dead }

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Thread {
    pub wake_tick:    u64,
    pub id:           u32,
    pub state:        TS,
    pub prio:         u8,
    pub krsp:         VirtAddr,
    pub ktop:         VirtAddr,
    pub utop:         VirtAddr,
    pub cr3:          PhysAddr,
    pub name:         [u8; 16],
    pub ticks:        u64,
    pub sig_handlers: [u64; 32],
    pub sig_pending:  u64,
    pub cwd:          [u8; 256],
}

impl Thread {
    pub const fn new() -> Self {
        Self {
            id: 0, state: TS::Dead, prio: 10,
            krsp: 0, ktop: 0, utop: 0, cr3: 0,
            name: [0; 16], ticks: 0, wake_tick: 0,
            sig_handlers: [0u64; 32],
            sig_pending:  0,
            cwd:          { let mut a = [0u8; 256]; a[0] = b'/'; a },
        }
    }
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        unsafe { core::str::from_utf8_unchecked(&self.name[..end]) }
    }
}

pub static mut THREADS: [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS];
pub static CUR:         AtomicUsize           = AtomicUsize::new(0);
pub static NTHREADS:    AtomicUsize           = AtomicUsize::new(0);
static SCHED_LOCK:      Spinlock              = Spinlock::new();

pub unsafe fn sched_init() {
    let tid = spawn_k("idle\0", idle as *const () as u64, 0);
    if tid >= 0 {
        THREADS[tid as usize].state = TS::Run;
        CUR.store(tid as usize, Ordering::SeqCst);
        tss_rsp0(THREADS[tid as usize].ktop);
    }
}

pub unsafe fn set_name(t: &mut Thread, name: &str) {
    let b = name.as_bytes();
    for j in 0..core::cmp::min(15, b.len()) { t.name[j] = b[j]; }
}

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

pub unsafe fn spawn_user_on_cr3(name: &str, entry: u64, arg: u64, cr3: PhysAddr) -> i32 {
    asm!("cli", options(nomem, nostack));
    for i in 0..MAX_THREADS {
        if THREADS[i].state != TS::Dead { continue; }
        let t = &mut THREADS[i];

        let ks = 0x0200_0000u64
            + i as u64 * (KERNEL_STACK_SIZE + PAGE_SIZE) as u64
            + PAGE_SIZE as u64;
        for p in 0..(KERNEL_STACK_SIZE / PAGE_SIZE) {
            vmap(K_P4, ks + p as u64 * PAGE_SIZE as u64, mm_alloc(), PTE_W);
        }
        let kt = ks + KERNEL_STACK_SIZE as u64;

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

// Układ stosu od krsp w górę (thread_switch popuje w tej kolejności):
//
//   thread_switch wykonuje:
//     pop r15, pop r14, pop r13, pop r12, pop rbp, pop rbx, ret
//
//   Więc na stosie od krsp musi być:
//   [krsp+0 ] r15  = arg
//   [krsp+8 ] r14  = entry
//   [krsp+16] r13  = ut
//   [krsp+24] r12  = 0
//   [krsp+32] rbp  = 0
//   [krsp+40] rbx  = 0
//   [krsp+48] ret addr = tramp_u / tramp_k   ← ret skacze tutaj
//
//   tramp_u następnie wykonuje iretq z ramką:
//   [krsp+56] RIP    = entry
//   [krsp+64] CS     = 0x1B
//   [krsp+72] RFLAGS = 0x202
//   [krsp+80] RSP    = ut
//   [krsp+88] SS     = 0x23
fn init_thread_stack(t: &mut Thread, kt: VirtAddr, ut: VirtAddr, entry: u64, arg: u64, user: bool) {
    let mut ksp = kt;
    unsafe {
        if user {
            // iretq frame (od najwyższego adresu — push kolejno)
            ksp -= 8; *(ksp as *mut u64) = 0x23;       // SS
            ksp -= 8; *(ksp as *mut u64) = ut;          // RSP
            ksp -= 8; *(ksp as *mut u64) = 0x202;       // RFLAGS (IF=1)
            ksp -= 8; *(ksp as *mut u64) = 0x1B;        // CS user code
            ksp -= 8; *(ksp as *mut u64) = entry;       // RIP
            // ret address → tramp_u wykona iretq
            ksp -= 8; *(ksp as *mut u64) = tramp_u as *const () as u64;
        } else {
            ksp -= 8; *(ksp as *mut u64) = tramp_k as *const () as u64;
        }
        // callee-saved regs w kolejności odwrotnej do pop w thread_switch:
        // thread_switch: pop r15, r14, r13, r12, rbp, rbx
        // więc push kolejno: rbx, rbp, r12, r13, r14, r15
        // → na stosie od dołu (niższy adres): r15, r14, r13, r12, rbp, rbx
        ksp -= 8; *(ksp as *mut u64) = 0u64;            // rbx
        ksp -= 8; *(ksp as *mut u64) = 0u64;            // rbp
        ksp -= 8; *(ksp as *mut u64) = 0u64;            // r12
        ksp -= 8; *(ksp as *mut u64) = ut;              // r13 (user stack top dla tramp_u)
        ksp -= 8; *(ksp as *mut u64) = entry;           // r14 (entry dla tramp_u)
        ksp -= 8; *(ksp as *mut u64) = arg;             // r15 (arg)
    }
    t.krsp = ksp;
}

pub unsafe fn schedule() {
    if SCHED_LOCK.locked.swap(true, Ordering::Acquire) { return; }
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
    if THREADS[next].cr3 == K_P4 || THREADS[next].cr3 == 0 {
        tss_rsp0(THREADS[next].ktop);
    } else {
        tss_rsp0(crate::perm::irq_stack_top());
    }

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

    // Jeśli następny wątek to userspace (krsp=0, nie ma kernel stosu)
    // wróć do niego przez enter_userspace zamiast thread_switch
    if THREADS[next].krsp == 0 && THREADS[next].cr3 != K_P4 && THREADS[next].cr3 != 0 {
        let entry = crate::userspace_loader::US_ENTRY;
        let stack = crate::userspace_loader::US_STACK;
        let cr3   = THREADS[next].cr3;
        serial_print("[SCHED] resuming userspace\n");
        enter_userspace(entry, stack, 0, cr3);
    }

    thread_switch(&mut THREADS[cur].krsp as *mut u64, THREADS[next].krsp);
}


unsafe extern "C" {
    pub fn enter_userspace(entry: u64, stack: u64, arg: u64, cr3: u64) -> !;
}

pub unsafe fn jump_to_scheduler() -> ! {
    let mut best = usize::MAX;
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
    { let mut b=[0u8;24]; serial_print(num_str(best,&mut b)); }
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

// thread_switch: zapisuje callee-saved na starym stosie, ładuje nowy
// push: rbx, rbp, r12, r13, r14, r15  → na stosie od dołu: r15,r14,r13,r12,rbp,rbx
// pop:  r15, r14, r13, r12, rbp, rbx
#[unsafe(naked)]
unsafe extern "C" fn thread_switch(old: *mut VirtAddr, new: VirtAddr) {
    core::arch::naked_asm!(
        "push rbx", "push rbp", "push r12", "push r13", "push r14", "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx",
        "ret",
    );
}

unsafe extern "C" fn idle(_: u64) -> ! {
    loop { asm!("hlt", options(nomem, nostack)); }
}