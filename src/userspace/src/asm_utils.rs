// CosinusOS Userspace — asm_utils.rs
// Hot-path: memcpy/memset/strlen/fnv1a + moduł math

#[inline(always)]
pub unsafe fn fast_memcpy(dst: *mut u8, src: *const u8, n: usize) {
    core::arch::asm!(
        "mov rcx, {n}", "shr rcx, 3", "rep movsq",
        "mov rcx, {n}", "and rcx, 7", "rep movsb",
        n = in(reg) n, in("rdi") dst, in("rsi") src, out("rcx") _,
        options(nostack)
    );
}

#[inline(always)]
pub unsafe fn fast_memset(dst: *mut u8, val: u8, n: usize) {
    let wide: u64 = (val as u64) * 0x0101010101010101u64;
    core::arch::asm!(
        "mov rcx, {n}", "shr rcx, 3", "rep stosq",
        "mov rcx, {n}", "and rcx, 7", "rep stosb",
        n = in(reg) n, in("rdi") dst, in("rax") wide, out("rcx") _,
        options(nostack)
    );
}

#[inline(always)]
pub unsafe fn fast_strlen(s: *const u8) -> usize {
    let mut len: usize;
    core::arch::asm!(
        "xor al, al", "mov rcx, 0xFFFFFFFF", "repne scasb",
        "not rcx", "dec rcx",
        in("rdi") s, out("rcx") len, out("al") _,
        options(nostack)
    );
    len
}

#[inline(always)]
pub unsafe fn fnv1a_hash_asm(data: *const u8, len: usize) -> u64 {
    let mut hash: u64;
    core::arch::asm!(
        "mov {h}, 0xcbf29ce484222325",
        "test {n}, {n}", "jz 2f",
        "3:",
        "movzx eax, byte ptr [{ptr}]",
        "xor {h}, rax",
        "mov rax, 0x100000001b3",
        "imul {h}, rax",
        "inc {ptr}", "dec {n}", "jnz 3b",
        "2:",
        h   = out(reg) hash,
        ptr = inout(reg) data => _,
        n   = inout(reg) len  => _,
        out("rax") _,
        options(nostack)
    );
    hash
}

pub mod math {
    #[inline(always)] pub fn min<T: PartialOrd>(a: T, b: T) -> T { if a < b { a } else { b } }
    #[inline(always)] pub fn max<T: PartialOrd>(a: T, b: T) -> T { if a > b { a } else { b } }
    #[inline(always)] pub fn abs(x: i32) -> i32 { if x < 0 { -x } else { x } }
    #[inline(always)] pub fn clamp<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
        if v < lo { lo } else if v > hi { hi } else { v }
    }
    #[inline(always)]
    pub fn popcount(x: u64) -> u32 {
        let r: u64;
        unsafe { core::arch::asm!("popcnt {r}, {x}", r = out(reg) r, x = in(reg) x, options(nostack, nomem)); }
        r as u32
    }
    #[inline(always)]
    pub fn leading_zeros(x: u64) -> u32 {
        if x == 0 { return 64; }
        let r: u64;
        unsafe { core::arch::asm!("bsr {r}, {x}", r = out(reg) r, x = in(reg) x, options(nostack, nomem)); }
        63 - r as u32
    }
}
