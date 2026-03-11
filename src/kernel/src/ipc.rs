// CosinusOS — ipc.rs
// Message-passing IPC między wątkami userspace
// Każdy wątek ma kolejkę FIFO wiadomości (bez alokacji heap — ring buffer)

use crate::sync::Spinlock;
use crate::syscall_api::{IpcMsg, err};
use crate::perm::TF;
use crate::threading::{THREADS, CUR, MAX_THREADS, TS};
use crate::mm::valid_buf;
use core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Kolejka wiadomości dla jednego wątku
// ─────────────────────────────────────────────────────────────────────────────
const IPC_QUEUE_DEPTH: usize = 16;

#[repr(C)]
struct MsgQueue {
    msgs: [IpcMsg; IPC_QUEUE_DEPTH],
    head: usize,
    tail: usize,
    lock: Spinlock,
}

// IpcMsg nie implementuje Copy przez tablice u64 — ręcznie zerujemy
unsafe fn zero_msg(m: *mut IpcMsg) {
    core::ptr::write_bytes(m as *mut u8, 0, core::mem::size_of::<IpcMsg>());
}

impl MsgQueue {
    const fn new() -> Self {
        // SAFETY: zero-init jest poprawny dla IpcMsg (same liczby)
        Self {
            msgs: unsafe { core::mem::zeroed() },
            head: 0,
            tail: 0,
            lock: Spinlock::new(),
        }
    }

    fn len(&self) -> usize {
        (self.head + IPC_QUEUE_DEPTH - self.tail) % IPC_QUEUE_DEPTH
    }

    fn is_empty(&self) -> bool { self.head == self.tail }

    fn is_full(&self) -> bool {
        (self.head + 1) % IPC_QUEUE_DEPTH == self.tail
    }

    /// Wstaw wiadomość (kopiuj przez pole po polu — IpcMsg nie jest Copy)
    unsafe fn push(&mut self, msg: &IpcMsg) -> bool {
        if self.is_full() { return false; }
        let dst = &mut self.msgs[self.head];
        dst.from  = msg.from;
        dst.to    = msg.to;
        dst.tag   = msg.tag;
        dst._pad  = 0;
        dst.data  = msg.data;
        dst.ptr   = msg.ptr;
        dst.len   = msg.len;
        dst._pad2 = 0;
        self.head = (self.head + 1) % IPC_QUEUE_DEPTH;
        true
    }

    /// Wyjmij wiadomość (kopiuj do dst)
    unsafe fn pop(&mut self, dst: *mut IpcMsg) -> bool {
        if self.is_empty() { return false; }
        let src = &self.msgs[self.tail];
        (*dst).from  = src.from;
        (*dst).to    = src.to;
        (*dst).tag   = src.tag;
        (*dst)._pad  = 0;
        (*dst).data  = src.data;
        (*dst).ptr   = src.ptr;
        (*dst).len   = src.len;
        (*dst)._pad2 = 0;
        self.tail = (self.tail + 1) % IPC_QUEUE_DEPTH;
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Globalne kolejki — jedna na wątek
// ─────────────────────────────────────────────────────────────────────────────
static mut IPC_QUEUES: [MsgQueue; MAX_THREADS] = {
    // const-init ręcznie (MsgQueue::new() jest const)
    [const { MsgQueue::new() }; MAX_THREADS]
};

// ─────────────────────────────────────────────────────────────────────────────
// Syscalle IPC
// ─────────────────────────────────────────────────────────────────────────────

/// IPC_SEND: rdi=ptr do IpcMsg w userspace
pub unsafe fn sys_ipc_send(tf: *mut TF) -> i64 {
    let ptr = (*tf).rdi;
    let p4  = THREADS[CUR.load(Ordering::Relaxed)].cr3;

    if !valid_buf(p4, ptr, core::mem::size_of::<IpcMsg>()) {
        return err::FAULT;
    }

    let msg = &*(ptr as *const IpcMsg);
    let to  = msg.to as usize;

    if to >= MAX_THREADS { return err::INVAL; }
    if THREADS[to].state == TS::Dead { return err::INVAL; }

    // Wpisz nadawcę
    let from = THREADS[CUR.load(Ordering::Relaxed)].id;
    // Musimy tymczasowo skopiować msg z from ustawionym
    let mut local_msg: IpcMsg = core::mem::zeroed();
    local_msg.from  = from;
    local_msg.to    = msg.to;
    local_msg.tag   = msg.tag;
    local_msg.data  = msg.data;
    local_msg.ptr   = msg.ptr;
    local_msg.len   = msg.len;

    let q = &mut IPC_QUEUES[to];
    q.lock.lock();
    let ok = q.push(&local_msg);
    q.lock.unlock();

    if !ok { return err::AGAIN; }  // kolejka pełna

    // Obudź odbiorcę jeśli blokował na IPC_RECV
    if THREADS[to].state == TS::Block {
        THREADS[to].state = TS::Ready;
    }

    err::OK
}

/// IPC_RECV: rdi=ptr do IpcMsg (wypełnia kernel), rsi=flagi (0=non-blocking, 1=block)
pub unsafe fn sys_ipc_recv(tf: *mut TF) -> i64 {
    let ptr   = (*tf).rdi;
    let block = (*tf).rsi != 0;
    let c     = CUR.load(Ordering::Relaxed);
    let p4    = THREADS[c].cr3;

    if !valid_buf(p4, ptr, core::mem::size_of::<IpcMsg>()) {
        return err::FAULT;
    }

    loop {
        let q = &mut IPC_QUEUES[c];
        q.lock.lock();
        let got = q.pop(ptr as *mut IpcMsg);
        q.lock.unlock();

        if got { return err::OK; }

        if !block { return err::AGAIN; }

        // Blokuj wątek i czekaj na wiadomość
        THREADS[c].state = TS::Block;
        crate::threading::thread_yield();
        // Po powrocie z yield sprawdź ponownie
    }
}

/// IPC_POLL: rdi=ptr do u32 (wypełni liczbą wiadomości w kolejce)
pub unsafe fn sys_ipc_poll(tf: *mut TF) -> i64 {
    let ptr = (*tf).rdi;
    let c   = CUR.load(Ordering::Relaxed);
    let p4  = THREADS[c].cr3;

    if ptr != 0 {
        if !valid_buf(p4, ptr, core::mem::size_of::<u32>()) {
            return err::FAULT;
        }
        let q = &IPC_QUEUES[c];
        *(ptr as *mut u32) = q.len() as u32;
    }

    IPC_QUEUES[c].len() as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Publiczne API kernela (dla wątków kernelowych)
// ─────────────────────────────────────────────────────────────────────────────

/// Wyślij wiadomość do wątku `to` bezpośrednio z kernela
pub unsafe fn k_send(to: usize, tag: u32, data: [u64; 4]) -> bool {
    if to >= MAX_THREADS || THREADS[to].state == TS::Dead { return false; }
    let msg = IpcMsg {
        from:  0,  // 0 = kernel
        to:    to as u32,
        tag,
        _pad:  0,
        data,
        ptr:   0,
        len:   0,
        _pad2: 0,
    };
    let q = &mut IPC_QUEUES[to];
    q.lock.lock();
    let ok = q.push(&msg);
    q.lock.unlock();
    if ok && THREADS[to].state == TS::Block {
        THREADS[to].state = TS::Ready;
    }
    ok
}

/// Sprawdź czy wątek ma nieprzeczytane wiadomości
pub unsafe fn k_has_msgs(tid: usize) -> bool {
    if tid >= MAX_THREADS { return false; }
    !IPC_QUEUES[tid].is_empty()
}