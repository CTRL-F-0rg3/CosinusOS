// libcosinus — fmt.rs

pub const NUM_BUF_SIZE: usize = 24;


#[inline]
pub fn num_buf() -> [u8; NUM_BUF_SIZE] { [0u8; NUM_BUF_SIZE] }

// ── Integers ─────────────────────────────────────────────────────────────────

pub fn u64_to_str<'a>(mut v: u64, buf: &'a mut [u8; NUM_BUF_SIZE]) -> &'a str {
    if v == 0 { buf[0] = b'0'; return core::str::from_utf8(&buf[..1]).unwrap(); }
    let mut i = NUM_BUF_SIZE;
    while v > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    core::str::from_utf8(&buf[i..NUM_BUF_SIZE]).unwrap_or("?")
}

pub fn i64_to_str<'a>(v: i64, buf: &'a mut [u8; NUM_BUF_SIZE]) -> &'a str {
    if v >= 0 { return u64_to_str(v as u64, buf); }
    // Ujemna: zapisz abs, potem wstaw '-' przed
    let mut tmp = [0u8; NUM_BUF_SIZE];
    let s = u64_to_str(v.unsigned_abs(), &mut tmp);
    let slen = s.len();
    let start = NUM_BUF_SIZE - slen - 1;
    buf[start] = b'-';
    buf[start + 1..start + 1 + slen].copy_from_slice(s.as_bytes());
    core::str::from_utf8(&buf[start..start + 1 + slen]).unwrap_or("?")
}

pub fn u32_to_str<'a>(v: u32, buf: &'a mut [u8; NUM_BUF_SIZE]) -> &'a str {
    u64_to_str(v as u64, buf)
}

pub fn usize_to_str<'a>(v: usize, buf: &'a mut [u8; NUM_BUF_SIZE]) -> &'a str {
    u64_to_str(v as u64, buf)
}

// ── Hex ──────────────────────────────────────────────────────────────────────

pub fn u64_to_hex<'a>(v: u64, buf: &'a mut [u8; NUM_BUF_SIZE]) -> &'a str {
    const HEX: &[u8] = b"0123456789abcdef";
    buf[0] = b'0'; buf[1] = b'x';
    if v == 0 { buf[2] = b'0'; return core::str::from_utf8(&buf[..3]).unwrap(); }
    let mut i = NUM_BUF_SIZE;
    let mut n = v;
    while n > 0 && i > 2 {
        i -= 1;
        buf[i] = HEX[(n & 0xF) as usize];
        n >>= 4;
    }
    
    let digits = &buf[i..NUM_BUF_SIZE];
    let dlen = digits.len();
    buf.copy_within(i..NUM_BUF_SIZE, 2);
    core::str::from_utf8(&buf[..2 + dlen]).unwrap_or("?")
}

pub fn u64_to_hex_pad<'a>(v: u64, pad: usize, buf: &'a mut [u8; NUM_BUF_SIZE]) -> &'a str {
    const HEX: &[u8] = b"0123456789abcdef";
    let pad = pad.min(NUM_BUF_SIZE - 2);
    buf[0] = b'0'; buf[1] = b'x';
    for j in 0..pad {
        buf[2 + pad - 1 - j] = HEX[((v >> (j * 4)) & 0xF) as usize];
    }
    core::str::from_utf8(&buf[..2 + pad]).unwrap_or("?")
}



pub fn bool_to_str(v: bool) -> &'static str { if v { "true" } else { "false" } }



pub struct FmtBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FmtBuf<N> {
    pub const fn new() -> Self { Self { buf: [0u8; N], len: 0 } }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    pub fn clear(&mut self) { self.len = 0; }

    pub fn push_str(&mut self, s: &str) -> &mut Self {
        let b = s.as_bytes();
        let free = N - self.len;
        let copy = b.len().min(free);
        self.buf[self.len..self.len + copy].copy_from_slice(&b[..copy]);
        self.len += copy;
        self
    }

    pub fn push_u64(&mut self, v: u64) -> &mut Self {
        let mut buf = num_buf();
        self.push_str(u64_to_str(v, &mut buf))
    }

    pub fn push_i64(&mut self, v: i64) -> &mut Self {
        let mut buf = num_buf();
        self.push_str(i64_to_str(v, &mut buf))
    }

    pub fn push_usize(&mut self, v: usize) -> &mut Self { self.push_u64(v as u64) }

    pub fn push_hex(&mut self, v: u64) -> &mut Self {
        let mut buf = num_buf();
        self.push_str(u64_to_hex(v, &mut buf))
    }

    pub fn push_bool(&mut self, v: bool) -> &mut Self {
        self.push_str(bool_to_str(v))
    }

    pub fn push_char(&mut self, c: char) -> &mut Self {
        let mut enc = [0u8; 4];
        self.push_str(c.encode_utf8(&mut enc))
    }
}

impl<const N: usize> core::fmt::Write for FmtBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

// ── Makro fmt! on stack ───────────────────────────────────────────────────────



#[macro_export]
macro_rules! cos_fmt {
    ($n:expr, $($a:tt)*) => {{
        let mut fb = $crate::fmt::FmtBuf::<$n>::new();
        { use core::fmt::Write; let _ = write!(fb, $($a)*); }
        fb
    }};
}
