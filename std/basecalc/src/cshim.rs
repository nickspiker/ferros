//! The C library symbols GMP, MPFR and MPC name beyond ferros_rt's (the allocator, strlen) and compiler_builtins' (the mem routines, the libm subset, the quad-precision float routines), for basecalc's engine on ferros.
//! Built against std/basecalc/c/include (tools/basecalc-build.sh); the engine never reads or writes a C stream, so:
//! - the string routines are real;
//! - a write to a stream (an assertion's report before its abort, a debug print) goes to the console as its format string, unexpanded, so a failure says where it came from;
//! - a read from a stream finds its end, as a process with no stream does;
//! - a formatting or scanning call, whose result the caller would use, ends the process loud rather than hand back something made up.
//! The variadic ones are defined with their named arguments alone: AArch64's procedure call standard passes those in the same registers either way, and none here reads past them.

use core::ffi::{c_char, c_int, c_long, CStr};

/// MPFR's printf family sets it on an overflowing length; one for the process (the engine runs on the UI thread alone).
#[no_mangle]
pub static mut errno: c_int = 0;

/// The three streams: names only, never opened, never read.
#[no_mangle]
pub static mut stdin: *mut u8 = core::ptr::null_mut();
#[no_mangle]
pub static mut stdout: *mut u8 = core::ptr::null_mut();
#[no_mangle]
pub static mut stderr: *mut u8 = core::ptr::null_mut();

const EOF: c_int = -1;

unsafe fn text<'a>(s: *const c_char) -> std::borrow::Cow<'a, str> {
    unsafe { CStr::from_ptr(s) }.to_string_lossy()
}

fn said(what: &str, text: &str) {
    println!("basecalc: the C wrote ({what}): {}", text.trim_end());
}

fn refused(what: &str) -> ! {
    panic!("basecalc: the C called {what}, which ferros does not give");
}

/// C's abort, which rug calls by name (its random-state guards) and ferros-libc declares: the process ends as std's own abort ends it.
#[no_mangle]
pub extern "C" fn abort() -> ! {
    println!("basecalc: the C aborted");
    std::process::abort()
}

#[no_mangle]
pub unsafe extern "C" fn fprintf(_f: *mut u8, fmt: *const c_char) -> c_int {
    let t = unsafe { text(fmt) };
    said("fprintf", &t);
    t.len() as c_int
}

#[no_mangle]
pub unsafe extern "C" fn printf(fmt: *const c_char) -> c_int {
    let t = unsafe { text(fmt) };
    said("printf", &t);
    t.len() as c_int
}

#[no_mangle]
pub unsafe extern "C" fn vfprintf(_f: *mut u8, fmt: *const c_char, _ap: *mut u8) -> c_int {
    let t = unsafe { text(fmt) };
    said("vfprintf", &t);
    t.len() as c_int
}

#[no_mangle]
pub unsafe extern "C" fn fputs(s: *const c_char, _f: *mut u8) -> c_int {
    said("fputs", &unsafe { text(s) });
    0
}

#[no_mangle]
pub unsafe extern "C" fn fputc(c: c_int, _f: *mut u8) -> c_int {
    said("fputc", &char::from(c as u8).to_string());
    c
}

#[no_mangle]
pub unsafe extern "C" fn putc(c: c_int, f: *mut u8) -> c_int {
    unsafe { fputc(c, f) }
}

#[no_mangle]
pub unsafe extern "C" fn putchar(c: c_int) -> c_int {
    unsafe { fputc(c, core::ptr::null_mut()) }
}

#[no_mangle]
pub unsafe extern "C" fn fwrite(p: *const u8, size: usize, n: usize, _f: *mut u8) -> usize {
    let bytes = unsafe { core::slice::from_raw_parts(p, size * n) };
    said("fwrite", &String::from_utf8_lossy(bytes));
    n
}

#[no_mangle]
pub extern "C" fn fgetc(_f: *mut u8) -> c_int {
    EOF
}

#[no_mangle]
pub extern "C" fn getc(_f: *mut u8) -> c_int {
    EOF
}

#[no_mangle]
pub extern "C" fn ungetc(_c: c_int, _f: *mut u8) -> c_int {
    EOF
}

#[no_mangle]
pub extern "C" fn fread(_p: *mut u8, _size: usize, _n: usize, _f: *mut u8) -> usize {
    0
}

#[no_mangle]
pub extern "C" fn feof(_f: *mut u8) -> c_int {
    1
}

#[no_mangle]
pub extern "C" fn ferror(_f: *mut u8) -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn sprintf(_s: *mut c_char, _fmt: *const c_char) -> c_int {
    refused("sprintf")
}

#[no_mangle]
pub extern "C" fn vsprintf(_s: *mut c_char, _fmt: *const c_char, _ap: *mut u8) -> c_int {
    refused("vsprintf")
}

#[no_mangle]
pub extern "C" fn sscanf(_s: *const c_char, _fmt: *const c_char) -> c_int {
    refused("sscanf")
}

#[no_mangle]
pub extern "C" fn fscanf(_f: *mut u8, _fmt: *const c_char) -> c_int {
    refused("fscanf")
}

/// C's strtol: optional space, an optional sign, then digits of `base` (2 to 36; 0 reads a `0x` or a leading `0` as C does), stopping at the first that is not one.
#[no_mangle]
pub unsafe extern "C" fn strtol(s: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long {
    let mut p = s as *const u8;
    let at = |p: *const u8| unsafe { p.read() };
    while matches!(at(p), b' ' | b'\t'..=b'\r') {
        p = unsafe { p.add(1) };
    }
    let negative = at(p) == b'-';
    if matches!(at(p), b'-' | b'+') {
        p = unsafe { p.add(1) };
    }
    let mut base = base as u32;
    let hex_prefix = at(p) == b'0' && matches!(at(unsafe { p.add(1) }), b'x' | b'X');
    if (base == 0 || base == 16) && hex_prefix {
        p = unsafe { p.add(2) };
        base = 16;
    } else if base == 0 {
        base = if at(p) == b'0' { 8 } else { 10 };
    }
    let mut n: c_long = 0;
    let start = p;
    while let Some(d) = (at(p) as char).to_digit(36).filter(|&d| d < base) {
        n = n * base as c_long + d as c_long;
        p = unsafe { p.add(1) };
    }
    // C's contract: a caller that wants no end back passes a null `end` (GMP's printf parser does).
    if !end.is_null() {
        unsafe { end.write(if p == start { s as *mut c_char } else { p as *mut c_char }) };
    }
    if negative {
        -n
    } else {
        n
    }
}

#[no_mangle]
pub unsafe extern "C" fn strchr(s: *const c_char, c: c_int) -> *mut c_char {
    let mut p = s;
    loop {
        let b = unsafe { p.read() };
        if b as u8 == c as u8 {
            return p as *mut c_char;
        }
        if b == 0 {
            return core::ptr::null_mut();
        }
        p = unsafe { p.add(1) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn strcpy(dst: *mut c_char, src: *const c_char) -> *mut c_char {
    let n = unsafe { CStr::from_ptr(src) }.to_bytes_with_nul().len();
    unsafe { core::ptr::copy_nonoverlapping(src, dst, n) };
    dst
}

/// C's strncpy: at most `n` bytes, the rest of the `n` filled with zeros.
#[no_mangle]
pub unsafe extern "C" fn strncpy(dst: *mut c_char, src: *const c_char, n: usize) -> *mut c_char {
    let mut i = 0;
    while i < n && unsafe { src.add(i).read() } != 0 {
        unsafe { dst.add(i).write(src.add(i).read()) };
        i += 1;
    }
    while i < n {
        unsafe { dst.add(i).write(0) };
        i += 1;
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn strcat(dst: *mut c_char, src: *const c_char) -> *mut c_char {
    let at = unsafe { CStr::from_ptr(dst) }.to_bytes().len();
    unsafe { strcpy(dst.add(at), src) };
    dst
}

/// C's strncat: at most `n` bytes of `src` after `dst`'s own, then the end.
#[no_mangle]
pub unsafe extern "C" fn strncat(dst: *mut c_char, src: *const c_char, n: usize) -> *mut c_char {
    let at = unsafe { CStr::from_ptr(dst) }.to_bytes().len();
    let mut i = 0;
    while i < n && unsafe { src.add(i).read() } != 0 {
        unsafe { dst.add(at + i).write(src.add(i).read()) };
        i += 1;
    }
    unsafe { dst.add(at + i).write(0) };
    dst
}
