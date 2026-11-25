//! The C formatter and streams (feature `cxx`, STD-TARGET.md "C++ through ferros_rt"): `vsnprintf` and its kin, and `stdout`/`stderr` on the console.
//! openh264's log lines go through it (its default trace prints warnings and errors to `stderr`), so it formats for real, but only what C libraries here use: the flags `-`, `0`, `+`, space and `#`, a width and a precision (numbers or `*`), the lengths `hh`, `h`, `l`, `ll`, `z`, `j`, `t` and `L`, and the conversions `d` `i` `u` `x` `X` `o` `c` `s` `p` `f` `F` and `%`.
//! Anything else (`e`, `g`, `a`, `n`, wide strings) is written out as it stands in the format, so it shows on the console rather than reading an argument wrongly; `%n` never writes.
//! There are no files on ferros: `fopen` answers null, and a stream call on anything but `stdout` and `stderr` fails.
//!
//! Each `saturating_sub` below is C's padding rule, never a guard on a bug: a field longer than its width gets no padding, a number with more digits than its precision no zeros, and an `snprintf` past its buffer's size writes nothing more (each subtraction would otherwise wrap and ask for 2^64 bytes of padding).

use core::ffi::{VaList, c_char, c_int, c_void};

/// A stream, opaque to C: only the two below exist.
#[repr(C)]
pub struct File {
    _console: u8,
}

static STDOUT_FILE: File = File { _console: 1 };
static STDERR_FILE: File = File { _console: 2 };

/// C's `FILE*` as a static the image's relocations fill.
#[repr(transparent)]
pub struct Stream(*const File);
unsafe impl Sync for Stream {}

#[unsafe(no_mangle)]
pub static stdout: Stream = Stream(&STDOUT_FILE);
#[unsafe(no_mangle)]
pub static stderr: Stream = Stream(&STDERR_FILE);

fn is_console(f: *const File) -> bool {
    core::ptr::eq(f, &STDOUT_FILE) || core::ptr::eq(f, &STDERR_FILE)
}

/// Where formatted bytes go: a C buffer of a given size (everything counted, what fits kept), or the console (in 1 KiB pieces from the stack, as console.rs writes them).
trait Sink {
    fn put(&mut self, b: &[u8]);
}

struct Buf {
    at: *mut u8,
    room: usize,
    len: usize,
}

impl Sink for Buf {
    fn put(&mut self, b: &[u8]) {
        // WHY the check: snprintf's contract is the caller's size, which the formatted length may pass; past it bytes are counted, not written.
        let n = if self.len + b.len() <= self.room { b.len() } else { self.room.saturating_sub(self.len) };
        unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), self.at.add(self.len), n) };
        self.len += b.len();
    }
}

struct Console {
    buf: [u8; 1 << 10],
    used: usize,
    len: usize,
}

impl Console {
    fn new() -> Self {
        Console { buf: [0; 1 << 10], used: 0, len: 0 }
    }
    fn flush(&mut self) {
        crate::console::write(&self.buf[..self.used]);
        self.used = 0;
    }
}

impl Sink for Console {
    fn put(&mut self, mut b: &[u8]) {
        self.len += b.len();
        while !b.is_empty() {
            if self.used == self.buf.len() {
                self.flush();
            }
            let n = core::cmp::min(b.len(), self.buf.len() - self.used);
            self.buf[self.used..self.used + n].copy_from_slice(&b[..n]);
            self.used += n;
            b = &b[n..];
        }
    }
}

fn pad(out: &mut dyn Sink, c: u8, n: usize) {
    for _ in 0..n {
        out.put(&[c]);
    }
}

#[derive(Default)]
struct Spec {
    left: bool,
    zero: bool,
    plus: bool,
    space: bool,
    alt: bool,
    width: usize,
    prec: Option<usize>,
}

/// A field: `sign` and `prefix` before the zeros a `0` flag adds, then `body`, padded to the width.
fn field(out: &mut dyn Sink, s: &Spec, sign: &[u8], prefix: &[u8], zeros: usize, body: &[u8]) {
    let len = sign.len() + prefix.len() + zeros + body.len();
    let fill = s.width.saturating_sub(len);
    if s.left {
        out.put(sign);
        out.put(prefix);
        pad(out, b'0', zeros);
        out.put(body);
        pad(out, b' ', fill);
    } else if s.zero {
        out.put(sign);
        out.put(prefix);
        pad(out, b'0', zeros + fill);
        out.put(body);
    } else {
        pad(out, b' ', fill);
        out.put(sign);
        out.put(prefix);
        pad(out, b'0', zeros);
        out.put(body);
    }
}

/// `v`'s digits in `base` into the end of `d`; the digits' start.
fn digits(mut v: u64, base: u64, upper: bool, d: &mut [u8; 64]) -> usize {
    let set: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut i = d.len();
    loop {
        i -= 1;
        d[i] = set[(v % base) as usize];
        v /= base;
        if v == 0 {
            return i;
        }
    }
}

fn int(out: &mut dyn Sink, s: &Spec, neg: bool, v: u64, base: u64, upper: bool, prefix: &[u8]) {
    let mut d = [0u8; 64];
    let mut i = digits(v, base, upper, &mut d);
    // A precision of 0 prints nothing for 0, as C says.
    if s.prec == Some(0) && v == 0 {
        i = d.len();
    }
    let body = &d[i..];
    let sign: &[u8] = if neg { b"-" } else if s.plus { b"+" } else if s.space { b" " } else { b"" };
    let zeros = s.prec.map_or(0, |p| p.saturating_sub(body.len()));
    let spec = Spec { zero: s.zero && s.prec.is_none(), ..*s };
    field(out, &spec, sign, prefix, zeros, body);
}

/// `%f`: the integer part exactly while it fits 64 bits (beyond, its leading 19 digits and zeros), the fraction rounded to the precision.
fn float(out: &mut dyn Sink, s: &Spec, v: f64, upper: bool) {
    let neg = v.is_sign_negative();
    let a = if neg { -v } else { v };
    let sign: &[u8] = if neg { b"-" } else if s.plus { b"+" } else if s.space { b" " } else { b"" };
    if a.is_nan() || a.is_infinite() {
        let w: &[u8] = match (a.is_nan(), upper) {
            (true, false) => b"nan",
            (true, true) => b"NAN",
            (false, false) => b"inf",
            (false, true) => b"INF",
        };
        field(out, &Spec { zero: false, ..*s }, sign, b"", 0, w);
        return;
    }
    let prec = s.prec.unwrap_or(6);
    // Digits of the fraction carried exactly: 10^19 is the largest power of ten in 64 bits; past that, zeros.
    let exact = core::cmp::min(prec, 19);
    let scale = 10u64.pow(exact as u32);
    // The integer part and the scaled fraction, rounded half up; a carry moves into the integer part.
    let (mut ip, mut tens) = (a, 0u32);
    while ip >= 1.8e19 {
        ip /= 10.;
        tens += 1;
    }
    let mut whole = ip as u64;
    let mut frac = if tens == 0 { ((a - whole as f64) * scale as f64 + 0.5) as u64 } else { 0 };
    if frac >= scale {
        frac -= scale;
        whole += 1;
    }
    let mut body = [0u8; 128];
    let mut n = 0;
    let mut d = [0u8; 64];
    let i = digits(whole, 10, false, &mut d);
    for &c in &d[i..] {
        body[n] = c;
        n += 1;
    }
    let mut fd = [0u8; 64];
    let fi = digits(frac, 10, false, &mut fd);
    out_parts(out, s, sign, &body[..n], tens as usize, prec, exact, &fd[fi..]);
}

/// The float's field from its parts: the integer digits, `tens` zeros after them, the point, the fraction's digits zero-padded on the left to `exact` and on the right to `prec`.
#[allow(clippy::too_many_arguments)]
fn out_parts(out: &mut dyn Sink, s: &Spec, sign: &[u8], whole: &[u8], tens: usize, prec: usize, exact: usize, frac: &[u8]) {
    let point = prec > 0 || s.alt;
    let len = sign.len() + whole.len() + tens + point as usize + prec;
    let fill = s.width.saturating_sub(len);
    if !s.left && !s.zero {
        pad(out, b' ', fill);
    }
    out.put(sign);
    if !s.left && s.zero {
        pad(out, b'0', fill);
    }
    out.put(whole);
    pad(out, b'0', tens);
    if point {
        out.put(b".");
    }
    if prec > 0 {
        // The fraction is under 10^exact, so it has at most `exact` digits.
        pad(out, b'0', exact - frac.len());
        out.put(frac);
        pad(out, b'0', prec - exact);
    }
    if s.left {
        pad(out, b' ', fill);
    }
}

/// The formatter: `fmt` with its arguments into `out`.
unsafe fn format(out: &mut dyn Sink, fmt: *const c_char, ap: &mut VaList<'_>) {
    let mut p = fmt as *const u8;
    loop {
        let c = unsafe { *p };
        if c == 0 {
            return;
        }
        if c != b'%' {
            // A run of plain bytes at once.
            let start = p;
            while unsafe { *p } != 0 && unsafe { *p } != b'%' {
                p = unsafe { p.add(1) };
            }
            out.put(unsafe { core::slice::from_raw_parts(start, p as usize - start as usize) });
            continue;
        }
        let at = p;
        p = unsafe { p.add(1) };
        let mut s = Spec::default();
        loop {
            match unsafe { *p } {
                b'-' => s.left = true,
                b'0' => s.zero = true,
                b'+' => s.plus = true,
                b' ' => s.space = true,
                b'#' => s.alt = true,
                _ => break,
            }
            p = unsafe { p.add(1) };
        }
        if unsafe { *p } == b'*' {
            let w = unsafe { ap.next_arg::<c_int>() };
            if w < 0 {
                s.left = true;
            }
            s.width = w.unsigned_abs() as usize;
            p = unsafe { p.add(1) };
        } else {
            while unsafe { *p }.is_ascii_digit() {
                s.width = s.width * 10 + (unsafe { *p } - b'0') as usize;
                p = unsafe { p.add(1) };
            }
        }
        if unsafe { *p } == b'.' {
            p = unsafe { p.add(1) };
            if unsafe { *p } == b'*' {
                let v = unsafe { ap.next_arg::<c_int>() };
                // A negative precision is taken as none, as C says.
                s.prec = if v < 0 { None } else { Some(v as usize) };
                p = unsafe { p.add(1) };
            } else {
                let mut v = 0;
                while unsafe { *p }.is_ascii_digit() {
                    v = v * 10 + (unsafe { *p } - b'0') as usize;
                    p = unsafe { p.add(1) };
                }
                s.prec = Some(v);
            }
        }
        // The length: how wide the integer argument is (every one is read as the 64-bit register it arrives in, then cut to this width).
        let mut bits = 32;
        loop {
            match unsafe { *p } {
                b'h' => bits = if bits == 16 { 8 } else { 16 },
                b'l' | b'z' | b'j' | b't' => bits = 64,
                b'L' => {}
                _ => break,
            }
            p = unsafe { p.add(1) };
        }
        let conv = unsafe { *p };
        if conv == 0 {
            return;
        }
        p = unsafe { p.add(1) };
        let unsigned = |ap: &mut VaList<'_>| -> u64 {
            let v = unsafe { ap.next_arg::<u64>() };
            if bits == 64 { v } else { v & ((1u64 << bits) - 1) }
        };
        match conv {
            b'd' | b'i' => {
                let raw = unsafe { ap.next_arg::<i64>() };
                // Sign-extend from the argument's width.
                let v = if bits == 64 { raw } else { (raw << (64 - bits)) >> (64 - bits) };
                int(out, &s, v < 0, v.unsigned_abs(), 10, false, b"");
            }
            b'u' => int(out, &s, false, unsigned(ap), 10, false, b""),
            b'x' | b'X' => {
                let v = unsigned(ap);
                let prefix: &[u8] = match (s.alt && v != 0, conv == b'X') {
                    (true, false) => b"0x",
                    (true, true) => b"0X",
                    _ => b"",
                };
                int(out, &s, false, v, 16, conv == b'X', prefix);
            }
            b'o' => int(out, &s, false, unsigned(ap), 8, false, if s.alt { b"0" } else { b"" }),
            b'p' => {
                let v = unsafe { ap.next_arg::<*const c_void>() } as u64;
                int(out, &Spec { plus: false, space: false, ..s }, false, v, 16, false, b"0x");
            }
            b'c' => {
                let v = unsafe { ap.next_arg::<c_int>() } as u8;
                field(out, &Spec { zero: false, ..s }, b"", b"", 0, &[v]);
            }
            b's' => {
                let q = unsafe { ap.next_arg::<*const c_char>() } as *const u8;
                let body: &[u8] = if q.is_null() {
                    b"(null)"
                } else {
                    // At most the precision's bytes are read, so a string with no end inside it is never read past.
                    let mut n = 0;
                    while s.prec.is_none_or(|m| n < m) && unsafe { *q.add(n) } != 0 {
                        n += 1;
                    }
                    unsafe { core::slice::from_raw_parts(q, n) }
                };
                field(out, &Spec { zero: false, ..s }, b"", b"", 0, body);
            }
            b'f' | b'F' => float(out, &s, unsafe { ap.next_arg::<f64>() }, conv == b'F'),
            b'%' => out.put(b"%"),
            // Not supported: the conversion as it stands, and no argument read.
            _ => out.put(unsafe { core::slice::from_raw_parts(at, p as usize - at as usize) }),
        }
    }
}

/// `vsnprintf`: at most `n` − 1 bytes and a terminating zero into `buf` (nothing when `n` is 0); the length the whole would have.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vsnprintf(buf: *mut c_char, n: usize, fmt: *const c_char, mut ap: VaList) -> c_int {
    let mut b = Buf { at: buf as *mut u8, room: n.saturating_sub(1), len: 0 };
    unsafe { format(&mut b, fmt, &mut ap) };
    if n != 0 {
        unsafe { *b.at.add(core::cmp::min(b.len, b.room)) = 0 };
    }
    b.len as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snprintf(buf: *mut c_char, n: usize, fmt: *const c_char, ap: ...) -> c_int {
    unsafe { vsnprintf(buf, n, fmt, ap) }
}

/// To the console; the bytes written.
unsafe fn console(fmt: *const c_char, ap: &mut VaList<'_>) -> c_int {
    let mut c = Console::new();
    unsafe { format(&mut c, fmt, ap) };
    c.flush();
    c.len as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn printf(fmt: *const c_char, mut ap: ...) -> c_int {
    unsafe { console(fmt, &mut ap) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vfprintf(f: *const File, fmt: *const c_char, mut ap: VaList) -> c_int {
    if !is_console(f) {
        return -1;
    }
    unsafe { console(fmt, &mut ap) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fprintf(f: *const File, fmt: *const c_char, ap: ...) -> c_int {
    unsafe { vfprintf(f, fmt, ap) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fwrite(p: *const c_void, size: usize, n: usize, f: *const File) -> usize {
    if !is_console(f) {
        return 0;
    }
    crate::console::write(unsafe { core::slice::from_raw_parts(p as *const u8, size * n) });
    n
}

/// No files: null, as C's `fopen` says for a file it cannot open.
#[unsafe(no_mangle)]
pub extern "C" fn fopen(_path: *const c_char, _mode: *const c_char) -> *mut File {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn fclose(_f: *mut File) -> c_int {
    0
}

/// The console is written as it is formatted: nothing waits to be flushed.
#[unsafe(no_mangle)]
pub extern "C" fn fflush(_f: *mut File) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fseek(_f: *mut File, _off: i64, _whence: c_int) -> c_int {
    -1
}
