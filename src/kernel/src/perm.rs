// CosinusOS — perm.rs


use core::arch::{asm, naked_asm};
use crate::debug::{col, outb, io_wait, print_raw, printc, hex_str, print};
use crate::mm::VirtAddr;
use crate::threading::schedule;

const DOUBLE_FAULT_STACK_SIZE: usize = 0x4000;

// ── TrapFrame ────────────────────────────────────────────────────────────────
#[repr(C, align(16))]
pub struct TF {
    pub r15:u64, pub r14:u64, pub r13:u64, pub r12:u64,
    pub r11:u64, pub r10:u64, pub r9:u64,  pub r8:u64,
    pub rdi:u64, pub rsi:u64, pub rdx:u64, pub rcx:u64,
    pub rbx:u64, pub rbp:u64, pub rax:u64,
    pub rip:u64, pub cs:u64,  pub rflags:u64, pub rsp:u64, pub ss:u64,
}

// ── TSS ──────────────────────────────────────────────────────────────────────
#[repr(C, packed)]
pub struct Tss {
    _r0: u32,
    pub rsp0: u64, pub rsp1: u64, pub rsp2: u64,
    _r1: u64,
    pub ist1: u64, _ist: [u64; 6],
    _r2: u64, _r3: u16,
    pub iomap: u16,
}
impl Tss {
    pub const fn new() -> Self {
        Self {
            _r0: 0, rsp0: 0, rsp1: 0, rsp2: 0, _r1: 0,
            ist1: 0, _ist: [0; 6], _r2: 0, _r3: 0,
            iomap: core::mem::size_of::<Tss>() as u16,
        }
    }
}

pub static mut TSS:      Tss                           = Tss::new();
pub static mut DF_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0u8; DOUBLE_FAULT_STACK_SIZE];
const IRQ_STACK_SIZE: usize = 0x4000;
pub static mut IRQ_STACK: [u8; IRQ_STACK_SIZE] = [0u8; IRQ_STACK_SIZE];
pub fn irq_stack_top() -> u64 { unsafe { IRQ_STACK.as_ptr() as u64 + IRQ_STACK_SIZE as u64 } }

pub unsafe fn tss_rsp0(v: VirtAddr) { TSS.rsp0 = v; }
pub unsafe fn tss_use_irq_stack() { TSS.rsp0 = irq_stack_top(); }

// ── GDT ──────────────────────────────────────────────────────────────────────
#[repr(C, packed)] #[derive(Clone, Copy)]
struct GdtE { ll: u16, lb: u16, mb: u8, acc: u8, gr: u8, hb: u8 }
impl GdtE {
    const fn null() -> Self { Self { ll:0, lb:0, mb:0, acc:0, gr:0, hb:0 } }
    fn seg(base: u64, lim: u64, acc: u8, gr: u8) -> Self {
        Self {
            ll: (lim & 0xFFFF) as u16,
            lb: (base & 0xFFFF) as u16,
            mb: ((base >> 16) & 0xFF) as u8,
            acc,
            gr: (((lim >> 16) & 0xF) as u8) | (gr & 0xF0),
            hb: ((base >> 24) & 0xFF) as u8,
        }
    }
}

#[repr(C, packed)] struct GdtTable { e: [GdtE; 6], tss_hi: u64 }
#[repr(C, packed)] struct GdtPtr   { lim: u16, base: u64 }

static mut GDT:     GdtTable = GdtTable { e: [GdtE::null(); 6], tss_hi: 0 };
static mut GDT_PTR: GdtPtr   = GdtPtr { lim: 0, base: 0 };

pub unsafe fn init_gdt() {
    TSS.ist1 = DF_STACK.as_ptr() as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
    let tb = &raw const TSS as u64;
    let tl = (core::mem::size_of::<Tss>() - 1) as u64;

    GDT.e[0] = GdtE::null();
    GDT.e[1] = GdtE::seg(0, 0xFFFFF, 0x9A, 0x20); // 0x08 kern code 64-bit
    GDT.e[2] = GdtE::seg(0, 0xFFFFF, 0x92, 0x00); // 0x10 kern data
    GDT.e[3] = GdtE::seg(0, 0xFFFFF, 0xFA, 0x20); // 0x18 user code  (DPL=3)
    GDT.e[4] = GdtE::seg(0, 0xFFFFF, 0xF2, 0x00); // 0x20 user data  (DPL=3)
    GDT.e[5] = GdtE::seg(tb, tl, 0x89, 0x00);      // 0x28 TSS

    GDT.tss_hi = tb >> 32;
    GDT_PTR.lim  = (core::mem::size_of::<GdtTable>() - 1) as u16;
    GDT_PTR.base = &raw const GDT as u64;

    asm!("lgdt [{}]", in(reg) &raw const GDT_PTR, options(preserves_flags));
    asm!(
        "push 0x08",
        "lea rax, [rip + 2f]",
        "push rax",
        "retfq",
        "2:",
        "mov ax, 0x10",
        "mov ds, ax", "mov es, ax", "mov fs, ax", "mov gs, ax", "mov ss, ax",
        out("rax") _,
        options(preserves_flags)
    );
    asm!("ltr ax", in("ax") 0x28u16, options(nostack, preserves_flags));
}

// ── IDT ──────────────────────────────────────────────────────────────────────
#[repr(C, packed)] #[derive(Clone, Copy)]
struct IdtE { lo: u16, sel: u16, ist: u8, attr: u8, mi: u16, hi: u32, _z: u32 }
impl IdtE {
    const fn null() -> Self { Self { lo:0, sel:0, ist:0, attr:0, mi:0, hi:0, _z:0 } }
    fn new(h: u64, sel: u16, dpl: u8, ist: u8) -> Self {
        Self {
            lo:  (h & 0xFFFF) as u16,
            mi:  ((h >> 16) & 0xFFFF) as u16,
            hi:  (h >> 32) as u32,
            sel, ist,
            attr: 0x8E | (dpl << 5),
            _z:  0,
        }
    }
}

#[repr(C, packed)] struct Idtr { lim: u16, base: u64 }

const IDT_LEN: usize = 256;
static mut IDT:  [IdtE; IDT_LEN] = [IdtE::null(); IDT_LEN];
static mut IDTR: Idtr             = Idtr { lim: 0, base: 0 };

pub unsafe fn init_idt() {
    use crate::threading::syscall_handler;

    IDT[0x08] = IdtE::new(isr_df  as *const () as u64, 0x08, 0, 1); // #DF IST1
    IDT[0x0D] = IdtE::new(isr_gp  as *const () as u64, 0x08, 0, 0); // #GP
    IDT[0x0E] = IdtE::new(isr_pf  as *const () as u64, 0x08, 0, 0); // #PF
    IDT[0x20] = IdtE::new(isr_tmr as *const () as u64, 0x08, 0, 0); // IRQ0 timer
    IDT[0x21] = IdtE::new(isr_kb  as *const () as u64, 0x08, 0, 0); // IRQ1 keyboard
    IDT[0x80] = IdtE::new(syscall_handler as *const () as u64, 0x08, 3, 0); // int 0x80

    IDTR.lim  = (core::mem::size_of::<[IdtE; IDT_LEN]>() - 1) as u16;
    IDTR.base = IDT.as_ptr() as u64;
    asm!("lidt [{}]", in(reg) &raw const IDTR, options(preserves_flags));
    asm!("sti", options(nomem, nostack));
}

// ── PIC ──────────────────────────────────────────────────────────────────────
pub unsafe fn init_pic() {
    outb(0x20, 0x11); io_wait(); outb(0xA0, 0x11); io_wait();
    outb(0x21, 0x20); io_wait(); outb(0xA1, 0x28); io_wait();
    outb(0x21, 0x04); io_wait(); outb(0xA1, 0x02); io_wait();
    outb(0x21, 0x01); io_wait(); outb(0xA1, 0x01); io_wait();

    outb(0x21, 0xFC);
 
    outb(0xA1, 0xFF);
}

// ── PIT ──────────────────────────────────────────────────────────────────────
pub unsafe fn init_pit() {
    let d = (1193180u32 / 100) as u16;
    outb(0x43, 0x36);
    outb(0x40, (d & 0xFF) as u8);
    outb(0x40, (d >> 8) as u8);
    asm!("sti", options(nomem, nostack));
}

// ── ISR helpers ───────────────────────────────────────────────────────────────
macro_rules! isr_no_err {
    ($n:ident, $h:expr) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $n() {
            naked_asm!(
                "push rax","push rbp","push rbx","push rcx","push rdx",
                "push rsi","push rdi","push r8","push r9","push r10",
                "push r11","push r12","push r13","push r14","push r15",
                "mov rdi, rsp", "call {f}",
                "pop r15","pop r14","pop r13","pop r12","pop r11","pop r10",
                "pop r9","pop r8","pop rdi","pop rsi","pop rdx","pop rcx",
                "pop rbx","pop rbp","pop rax","iretq",
                f = sym $h,
            );
        }
    };
}
macro_rules! isr_with_err {
    ($n:ident, $h:expr) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $n() {
            naked_asm!(
                "xchg rax, [rsp]",
                "push rbp","push rbx","push rcx","push rdx","push rsi","push rdi",
                "push r8","push r9","push r10","push r11","push r12","push r13","push r14","push r15",
                "mov rdi, rsp", "call {f}",
                "pop r15","pop r14","pop r13","pop r12","pop r11","pop r10","pop r9","pop r8",
                "pop rdi","pop rsi","pop rdx","pop rcx","pop rbx","pop rbp",
                "add rsp, 8", "iretq",
                f = sym $h,
            );
        }
    };
}

// ── #DF ──────────────────────────────────────────────────────────────────────
#[unsafe(naked)]
pub unsafe extern "C" fn isr_df() {
    naked_asm!(
        "cli",
        "add rsp, 8",
        "push rax","push rbp","push rbx","push rcx","push rdx",
        "push rsi","push rdi","push r8","push r9","push r10",
        "push r11","push r12","push r13","push r14","push r15",
        "mov rdi, rsp",
        "call {f}",
        "cli", "hlt",
        f = sym handle_df
    );
}

#[no_mangle]
pub unsafe extern "C" fn handle_df(f: *mut TF) {
    let cr2: u64;
    asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack));
    print_raw("\n[#DF] DOUBLE FAULT\n");
    print_raw("  RIP="); { let mut b=[0u8;18]; print_raw(hex_str((*f).rip, &mut b)); }
    print_raw("  RSP="); { let mut b=[0u8;18]; print_raw(hex_str((*f).rsp, &mut b)); }
    print_raw("  CR2="); { let mut b=[0u8;18]; print_raw(hex_str(cr2,      &mut b)); }
    print_raw("   CS="); { let mut b=[0u8;18]; print_raw(hex_str((*f).cs,  &mut b)); }
    print_raw("   SS="); { let mut b=[0u8;18]; print_raw(hex_str((*f).ss,  &mut b)); }
    print_raw("\n");
    loop { asm!("hlt", options(nomem, nostack)); }
}

// ── #GP ──────────────────────────────────────────────────────────────────────
isr_with_err!(isr_gp, handle_gp);

#[no_mangle]
pub unsafe extern "C" fn handle_gp(f: *mut TF) {
    let err = (*f).rax;
    let rip = (*f).rip;
    printc("\n[#GP] GENERAL PROTECTION FAULT\n", col::LRED);
    print("  err="); { let mut b=[0u8;18]; print(hex_str(err,      &mut b)); }
    print("  rip="); { let mut b=[0u8;18]; print(hex_str(rip,      &mut b)); }
    print("   cs="); { let mut b=[0u8;18]; print(hex_str((*f).cs,  &mut b)); }
    print("  rsp="); { let mut b=[0u8;18]; print(hex_str((*f).rsp, &mut b)); }
    print("   ss="); { let mut b=[0u8;18]; print(hex_str((*f).ss,  &mut b)); }
    print("\n");
    crate::panic_no_dyn("Unhandled #GP");
}

// ── #PF ──────────────────────────────────────────────────────────────────────
isr_with_err!(isr_pf, handle_pf);

#[no_mangle]
pub unsafe extern "C" fn handle_pf(f: *mut TF) {
    let err  = (*f).rax;
    let rip  = (*f).rip;
    let addr: u64;
    let cr3:  u64;
    asm!("mov {}, cr2", out(reg) addr, options(nomem, nostack));
    asm!("mov {}, cr3", out(reg) cr3,  options(nomem, nostack));
    printc("\n[#PF] PAGE FAULT\n", col::YELLOW);
    print("  addr="); { let mut b=[0u8;18]; print(hex_str(addr, &mut b)); }
    print("  err=");  { let mut b=[0u8;18]; print(hex_str(err,  &mut b)); }
    print("  rip=");  { let mut b=[0u8;18]; print(hex_str(rip,  &mut b)); }
    print("  cr3=");  { let mut b=[0u8;18]; print(hex_str(cr3,  &mut b)); }
    print(if err & 4 != 0 { " USR" } else { " KRN" });
    print(if err & 2 != 0 { " W\n" } else { " R\n" });

    use crate::mm::pt_ptr;
    let pml4i = ((addr >> 39) & 0x1FF) as usize;
    let pdpti = ((addr >> 30) & 0x1FF) as usize;
    let pdi   = ((addr >> 21) & 0x1FF) as usize;
    let pti   = ((addr >> 12) & 0x1FF) as usize;
    crate::debug::serial_print("[#PF] walk cr3=");
    { let mut b=[0u8;18]; crate::debug::serial_print(crate::debug::hex_str(cr3,&mut b)); }
    crate::debug::serial_print(" addr=");
    { let mut b=[0u8;18]; crate::debug::serial_print(crate::debug::hex_str(addr,&mut b)); }
    crate::debug::serial_print("\n");
    let pml4e = (*pt_ptr(cr3)).e[pml4i];
    crate::debug::serial_print("  PML4["); 
    { let mut b=[0u8;24]; crate::debug::serial_print(crate::debug::num_str(pml4i,&mut b)); }
    crate::debug::serial_print("]=");
    { let mut b=[0u8;18]; crate::debug::serial_print(crate::debug::hex_str(pml4e,&mut b)); }
    crate::debug::serial_print("\n");
    if pml4e & 1 != 0 {
        let pdpt = pml4e & 0x000F_FFFF_FFFF_F000;
        let pdpte = (*pt_ptr(pdpt)).e[pdpti];
        crate::debug::serial_print("  PDPT[");
        { let mut b=[0u8;24]; crate::debug::serial_print(crate::debug::num_str(pdpti,&mut b)); }
        crate::debug::serial_print("]=");
        { let mut b=[0u8;18]; crate::debug::serial_print(crate::debug::hex_str(pdpte,&mut b)); }
        crate::debug::serial_print("\n");
        if pdpte & 1 != 0 {
            let pd = pdpte & 0x000F_FFFF_FFFF_F000;
            let pde = (*pt_ptr(pd)).e[pdi];
            crate::debug::serial_print("  PD[");
            { let mut b=[0u8;24]; crate::debug::serial_print(crate::debug::num_str(pdi,&mut b)); }
            crate::debug::serial_print("]=");
            { let mut b=[0u8;18]; crate::debug::serial_print(crate::debug::hex_str(pde,&mut b)); }
            crate::debug::serial_print("\n");
            if pde & 1 != 0 {
                let pt = pde & 0x000F_FFFF_FFFF_F000;
                let pte = (*pt_ptr(pt)).e[pti];
                crate::debug::serial_print("  PT[");
                { let mut b=[0u8;24]; crate::debug::serial_print(crate::debug::num_str(pti,&mut b)); }
                crate::debug::serial_print("]=");
                { let mut b=[0u8;18]; crate::debug::serial_print(crate::debug::hex_str(pte,&mut b)); }
                crate::debug::serial_print("\n");
            }
        }
    }
    crate::panic_no_dyn("Unhandled page fault");
}

// ── Timer IRQ0 ────────────────────────────────────────────────────────────────
pub static mut TICK: u64 = 0;

isr_no_err!(isr_tmr, handle_timer);

#[no_mangle]
pub unsafe extern "C" fn handle_timer(_: *mut TF) {
    outb(0x20, 0x20);
    TICK += 1;
    schedule();
}

// ── Keyboard IRQ1 ─────────────────────────────────────────────────────────────
isr_no_err!(isr_kb, handle_kb);

#[no_mangle]
pub unsafe extern "C" fn handle_kb(_: *mut TF) {
    outb(0x20, 0x20); // EOI
    crate::input::kbd_irq();
}

// ── Syscall int 0x80 ──────────────────────────────────────────────────────────
isr_no_err!(isr_sys, handle_syscall);

#[no_mangle]
pub unsafe extern "C" fn handle_syscall(f: *mut TF) {
    crate::syscall_api::syscall_dispatch_v2(f);
}

// ── PublicAPI ─────────────────────────────────────────────────────────────
pub unsafe fn kb_pop() -> Option<char> {
    crate::input::input_poll()
}

pub unsafe fn kb_push_pub(c: char) {
    crate::input::input_push(c);
}