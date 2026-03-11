// CosinusOS — threading.rs
use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::Spinlock;
use crate::mm::{VirtAddr, PhysAddr, PTE_W, PTE_U, K_P4, PAGE_SIZE, KERNEL_STACK_SIZE, USER_STACK_SIZE, vmap, mm_alloc};
use crate::debug::{serial_print, serial_hex, print, num_str};
use crate::perm::tss_rsp0;

pub const MAX_THREADS: usize = 64;

// FIXED: #[naked] -> #[unsafe(naked)], asm! -> naked_asm!, options(noreturn) usunięte,
//        "call syscall_dispatch" -> sym syscall_dispatch
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
        1 => { // Write
            let ptr = arg2 as *const u8;
            let len = arg3 as usize;
            if !ptr.is_null() && len < 65536 {
                let s = core::slice::from_raw_parts(ptr, len);
                if let Ok(text) = core::str::from_utf8(s) {
                    // FIXED: crate::print!(...) -> crate::debug::print(text)
                    crate::debug::print(text);
                }
            }
        }
        0 => { // Exit
            // FIXED: exit_thread() nie istnieje — inline dead + schedule
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
    pub wake_tick: u64,
    pub id:    u32,
    pub state: TS,
    pub prio:  u8,
    pub krsp:  VirtAddr,
    pub ktop:  VirtAddr,
    pub utop:  VirtAddr,
    pub cr3:   PhysAddr,
    pub name:  [u8; 16],
    pub ticks: u64,
    //pub wake_tick: u64,
}

impl Thread {
    pub const fn new() -> Self {
        Self {
            id: 0, state: TS::Dead, prio: 10,
            krsp: 0, ktop: 0, utop: 0, cr3: 0,
            name: [0; 16], ticks: 0,
            wake_tick: 0,
        }
    }
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        unsafe { core::str::from_utf8_unchecked(&self.name[..end]) }
    }
}

pub static mut THREADS: [Thread; MAX_THREADS] = [Thread::new(); MAX_THREADS]; //wake_tick: 0,
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

        {
            let ksp = t.krsp;
            serial_print("[DBG] krsp=");        serial_hex(ksp);
            serial_print(" kt=");               serial_hex(kt);
            serial_print("\n");
            serial_print("[DBG] [+0 ] rbx =");  serial_hex(*((ksp+0)  as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+8 ] rbp =");  serial_hex(*((ksp+8)  as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+16] r12 =");  serial_hex(*((ksp+16) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+24] r13 =");  serial_hex(*((ksp+24) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+32] r14 =");  serial_hex(*((ksp+32) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+40] r15 =");  serial_hex(*((ksp+40) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+48] tramp="); serial_hex(*((ksp+48) as *const u64));
            serial_print(" expected=");         serial_hex(tramp_u as *const () as u64);
            serial_print("\n");
            serial_print("[DBG] [+56] RIP =");  serial_hex(*((ksp+56) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+64] CS  =");  serial_hex(*((ksp+64) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+72] RFLAGS="); serial_hex(*((ksp+72) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+80] RSP =");  serial_hex(*((ksp+80) as *const u64));
            serial_print("\n");
            serial_print("[DBG] [+88] SS  =");  serial_hex(*((ksp+88) as *const u64));
            serial_print("\n");
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

// Stos wygląda tak od krsp w górę:
//   [+0 ] rbx   ← pop rbx
//   [+8 ] rbp   ← pop rbp
//   [+16] r12   ← pop r12
//   [+24] r13   ← pop r13  (ut)
//   [+32] r14   ← pop r14  (entry)
//   [+40] r15   ← pop r15  (arg)
//   [+48] tramp ← ret (skacze do tramp_u)
//   [+56] RIP   ← iretq pobiera (entry userpace)
//   [+64] CS    ← iretq pobiera (0x1B)
//   [+72] RFLAGS← iretq pobiera (0x202)
//   [+80] RSP   ← iretq pobiera (ut)
//   [+88] SS    ← iretq pobiera (0x23)
fn init_thread_stack(t: &mut Thread, kt: VirtAddr, ut: VirtAddr, entry: u64, arg: u64, user: bool) {
    let mut ksp = kt;
    unsafe {
        if user {
            ksp -= 8; *(ksp as *mut u64) = 0x23;
            ksp -= 8; *(ksp as *mut u64) = ut;
            ksp -= 8; *(ksp as *mut u64) = 0x202;
            ksp -= 8; *(ksp as *mut u64) = 0x1B;
            ksp -= 8; *(ksp as *mut u64) = entry;
            ksp -= 8; *(ksp as *mut u64) = tramp_u as *const () as u64;
        } else {
            ksp -= 8; *(ksp as *mut u64) = tramp_k as *const () as u64;
        }
        ksp -= 8; *(ksp as *mut u64) = arg;
        ksp -= 8; *(ksp as *mut u64) = entry;
        ksp -= 8; *(ksp as *mut u64) = ut;
        ksp -= 8; *(ksp as *mut u64) = 0u64;
        ksp -= 8; *(ksp as *mut u64) = 0u64;
        ksp -= 8; *(ksp as *mut u64) = 0u64;
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
    tss_rsp0(THREADS[next].ktop);

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
    thread_switch(&mut THREADS[cur].krsp as *mut u64, THREADS[next].krsp);
}

pub unsafe fn thread_yield() { schedule(); }

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