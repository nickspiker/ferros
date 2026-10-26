//! gtrace: a freestanding ptrace syscall tracer for aarch64 Linux (Android, no libc, no NDK), for capturing what Broadcom's lhd says to the GNSS chip.
//! Usage: gtrace <out file> <seconds> <program> [args...]
//! Spawns the program under ptrace (following its threads and children), logs every openat/close, and for files whose path names the chip (ttyBCM, bbd, gnss, gps, nstandby, spi) every read/write/pread/pwrite with its bytes and every ioctl, as text lines:
//!   <monotonic ns> <tid> <call> fd=<fd> <path> ret=<ret> [<hex bytes>]
//! After <seconds> it kills the program and exits.
#![no_std]
#![no_main]

use core::arch::{asm, global_asm};

global_asm!(".global _start", "_start:", "mov x0, sp", "bl entry", "1: b 1b");

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(99)
}

unsafe fn sys6(n: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> i64 {
    let r: i64;
    asm!("svc #0", inlateout("x0") a as i64 => r, in("x1") b, in("x2") c, in("x3") d, in("x4") e, in("x5") f, in("x8") n, options(nostack));
    r
}
fn sys(n: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    unsafe { sys6(n, a, b, c, d, 0, 0) }
}
fn exit(code: i32) -> ! {
    sys(94, code as u64, 0, 0, 0);
    loop {}
}

const OPENAT: u64 = 56;
const CLOSE: u64 = 57;
const READ: u64 = 63;
const WRITE: u64 = 64;
const PREAD: u64 = 67;
const PWRITE: u64 = 68;
const IOCTL: u64 = 29;
const PTRACE: u64 = 117;
const WAIT4: u64 = 260;
const KILL: u64 = 129;
const GETPID: u64 = 172;
const CLONE: u64 = 220;
const EXECVE: u64 = 221;
const VMREAD: u64 = 270;

const PT_TRACEME: u64 = 0;
const PT_SYSCALL: u64 = 24;
const PT_SETOPTIONS: u64 = 0x4200;
const PT_GETREGSET: u64 = 0x4204;

struct Out {
    fd: u64,
    buf: [u8; 8192],
    n: usize,
}
impl Out {
    fn b(&mut self, s: &[u8]) {
        for &c in s {
            if self.n == self.buf.len() {
                self.flush();
            }
            self.buf[self.n] = c;
            self.n += 1;
        }
    }
    fn flush(&mut self) {
        let mut off = 0;
        while off < self.n {
            let r = sys(WRITE, self.fd, self.buf.as_ptr() as u64 + off as u64, (self.n - off) as u64, 0);
            if r <= 0 {
                break;
            }
            off += r as usize;
        }
        self.n = 0;
    }
    fn dec(&mut self, v: i64) {
        if v < 0 {
            self.b(b"-");
        }
        let mut u = v.unsigned_abs();
        let mut d = [0u8; 20];
        let mut i = 20;
        loop {
            i -= 1;
            d[i] = b'0' + (u % 10) as u8;
            u /= 10;
            if u == 0 {
                break;
            }
        }
        self.b(&d.clone()[i..]);
    }
    fn hex(&mut self, s: &[u8]) {
        const H: &[u8; 16] = b"0123456789abcdef";
        for &c in s {
            self.b(&[H[(c >> 4) as usize], H[(c & 15) as usize]]);
        }
    }
}

fn now_ns() -> i64 {
    let mut ts = [0i64; 2];
    sys(113, 1, ts.as_mut_ptr() as u64, 0, 0);
    ts[0] * 1_000_000_000 + ts[1]
}

/// Read `len` bytes of a tracee's memory.
fn peek(tid: i64, addr: u64, out: &mut [u8]) -> usize {
    let local = [out.as_mut_ptr() as u64, out.len() as u64];
    let remote = [addr, out.len() as u64];
    let r = unsafe { sys6(VMREAD, tid as u64, local.as_ptr() as u64, 1, remote.as_ptr() as u64, 1, 0) };
    if r < 0 { 0 } else { r as usize }
}

fn cstr(p: *const u8) -> &'static [u8] {
    let mut n = 0;
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

fn interesting(path: &[u8]) -> bool {
    for k in [&b"ttyBCM"[..], b"bbd", b"gnss", b"gps", b"nstandby", b"spi"] {
        if path.windows(k.len()).any(|w| w == k) {
            return true;
        }
    }
    false
}

static mut BIG: [u8; 1 << 17] = [0; 1 << 17];
static mut PATHS: [([u8; 96], usize); 256] = [([0; 96], 0); 256];
/// Per traced thread: (tid, in a syscall, nr, x0, x1, x2).
static mut TIDS: [(i64, bool, u64, u64, u64, u64); 128] = [(0, false, 0, 0, 0, 0); 128];

fn slot(tid: i64) -> usize {
    unsafe {
        let t = &mut *(&raw mut TIDS);
        if let Some(i) = t.iter().position(|e| e.0 == tid) {
            return i;
        }
        let i = t.iter().position(|e| e.0 == 0).unwrap_or(127);
        t[i] = (tid, false, 0, 0, 0, 0);
        i
    }
}

#[no_mangle]
extern "C" fn entry(sp: *const u64) -> ! {
    let argc = unsafe { *sp } as usize;
    let argv = unsafe { sp.add(1) } as *const *const u8;
    let envp = unsafe { argv.add(argc + 1) };
    if argc < 4 {
        exit(2);
    }
    let arg = |i: usize| cstr(unsafe { *argv.add(i) });
    let mut secs: i64 = 0;
    for &c in arg(2) {
        secs = secs * 10 + (c - b'0') as i64;
    }
    // The output file, NUL-terminated for openat.
    let mut path = [0u8; 256];
    let o = arg(1);
    path[..o.len()].copy_from_slice(o);
    let fd = sys(OPENAT, (-100i64) as u64, path.as_ptr() as u64, 0x241, 0o644);
    if fd < 0 {
        exit(3);
    }
    let mut out = Out { fd: fd as u64, buf: [0; 8192], n: 0 };

    let child = sys(CLONE, 17, 0, 0, 0);
    if child == 0 {
        sys(PTRACE, PT_TRACEME, 0, 0, 0);
        let me = sys(GETPID, 0, 0, 0, 0);
        sys(KILL, me as u64, 19, 0, 0);
        unsafe { sys6(EXECVE, *argv.add(3) as u64, argv.add(3) as u64, envp as u64, 0, 0, 0) };
        exit(4);
    }
    let mut st: i32 = 0;
    sys(WAIT4, child as u64, &mut st as *mut i32 as u64, 0x4000_0000, 0);
    // TRACESYSGOOD | TRACEFORK | TRACEVFORK | TRACECLONE | TRACEEXEC | EXITKILL
    sys(PTRACE, PT_SETOPTIONS, child as u64, 0, 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x10_0000);
    sys(PTRACE, PT_SYSCALL, child as u64, 0, 0);
    slot(child);
    let t_end = now_ns() + secs * 1_000_000_000;
    out.b(b"# gtrace started\n");
    loop {
        if now_ns() > t_end {
            out.b(b"# time up\n");
            out.flush();
            sys(KILL, child as u64, 9, 0, 0);
            exit(0);
        }
        let tid = sys(WAIT4, (-1i64) as u64, &mut st as *mut i32 as u64, 0x4000_0000, 0);
        if tid < 0 {
            out.flush();
            exit(0);
        }
        let s = st as u32;
        if s & 0x7f != 0x7f {
            // Exited or killed: forget the thread.
            unsafe { (*(&raw mut TIDS))[slot(tid)].0 = 0 };
            if tid == child {
                out.b(b"# the program ended\n");
                out.flush();
                exit(0);
            }
            continue;
        }
        let sig = (s >> 8) & 0xff;
        let event = s >> 16;
        let mut inject = 0u64;
        if sig == 0x85 {
            let mut regs = [0u64; 34];
            let iov = [regs.as_mut_ptr() as u64, 34 * 8];
            sys(PTRACE, PT_GETREGSET, tid as u64, 1, iov.as_ptr() as u64);
            let i = slot(tid);
            let e = unsafe { &mut (*(&raw mut TIDS))[i] };
            if !e.1 {
                // Entry: note the call and its arguments; a write's bytes are logged now.
                *e = (tid, true, regs[8], regs[0], regs[1], regs[2]);
                if matches!(regs[8], WRITE | PWRITE) {
                    let p = unsafe { (*(&raw const PATHS))[(regs[0] & 255) as usize] };
                    if interesting(&p.0[..p.1]) {
                        let big = unsafe { &mut *(&raw mut BIG) };
                        let n = peek(tid, regs[1], &mut big[..(regs[2] as usize).min(1 << 17)]);
                        out.dec(now_ns());
                        out.b(b" ");
                        out.dec(tid);
                        out.b(b" write fd=");
                        out.dec(regs[0] as i64);
                        out.b(b" ");
                        out.b(&p.0[..p.1]);
                        out.b(b" n=");
                        out.dec(n as i64);
                        out.b(b" ");
                        out.hex(&big[..n]);
                        out.b(b"\n");
                    }
                }
                if regs[8] == OPENAT {
                    let mut pb = [0u8; 96];
                    let n = peek(tid, regs[1], &mut pb);
                    let l = pb[..n].iter().position(|&c| c == 0).unwrap_or(n);
                    unsafe { (*(&raw mut PATHS))[255] = (pb, l) };
                }
            } else {
                e.1 = false;
                let (nr, a0, a1, a2) = (e.2, e.3, e.4, e.5);
                let ret = regs[0] as i64;
                match nr {
                    OPENAT => {
                        let p = unsafe { (*(&raw const PATHS))[255] };
                        if ret >= 0 {
                            unsafe { (*(&raw mut PATHS))[(ret & 255) as usize] = p };
                        }
                        out.dec(now_ns());
                        out.b(b" ");
                        out.dec(tid);
                        out.b(b" openat ");
                        out.b(&p.0[..p.1]);
                        out.b(b" ret=");
                        out.dec(ret);
                        out.b(b"\n");
                    }
                    CLOSE => {
                        let p = unsafe { (*(&raw const PATHS))[(a0 & 255) as usize] };
                        if interesting(&p.0[..p.1]) {
                            out.dec(now_ns());
                            out.b(b" ");
                            out.dec(tid);
                            out.b(b" close fd=");
                            out.dec(a0 as i64);
                            out.b(b" ");
                            out.b(&p.0[..p.1]);
                            out.b(b"\n");
                        }
                    }
                    READ | PREAD | IOCTL | WRITE | PWRITE => {
                        let p = unsafe { (*(&raw const PATHS))[(a0 & 255) as usize] };
                        if interesting(&p.0[..p.1]) {
                            out.dec(now_ns());
                            out.b(b" ");
                            out.dec(tid);
                            out.b(match nr {
                                READ => &b" read fd="[..],
                                PREAD => b" pread fd=",
                                IOCTL => b" ioctl fd=",
                                _ => b" wrote fd=",
                            });
                            out.dec(a0 as i64);
                            out.b(b" ");
                            out.b(&p.0[..p.1]);
                            if nr == IOCTL {
                                out.b(b" cmd=");
                                out.dec(a1 as i64);
                                out.b(b" arg=");
                                out.dec(a2 as i64);
                            }
                            out.b(b" ret=");
                            out.dec(ret);
                            if matches!(nr, READ | PREAD) && ret > 0 {
                                let big = unsafe { &mut *(&raw mut BIG) };
                                let n = peek(tid, a1, &mut big[..(ret as usize).min(1 << 17)]);
                                out.b(b" ");
                                out.hex(&big[..n]);
                            }
                            out.b(b"\n");
                        }
                    }
                    _ => {}
                }
            }
        } else if sig == 5 && event != 0 {
            // A ptrace event (a new thread, fork or exec): the new task is traced already.
        } else if sig == 19 {
            // A new thread's first stop: swallow it.
        } else {
            inject = sig as u64;
        }
        sys(PTRACE, PT_SYSCALL, tid as u64, 0, inject);
    }
}

// No libc: the memory routines the compiler calls.
#[no_mangle]
unsafe extern "C" fn memcpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        core::ptr::write_volatile(d.add(i), *s.add(i));
        i += 1;
    }
    d
}
#[no_mangle]
unsafe extern "C" fn memmove(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
    if (d as usize) < (s as usize) {
        memcpy(d, s, n)
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            core::ptr::write_volatile(d.add(i), *s.add(i));
        }
        d
    }
}
#[no_mangle]
unsafe extern "C" fn memset(d: *mut u8, c: i32, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        core::ptr::write_volatile(d.add(i), c as u8);
        i += 1;
    }
    d
}
#[no_mangle]
unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let (x, y) = (*a.add(i), *b.add(i));
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}
#[no_mangle]
unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    memcmp(a, b, n)
}
#[no_mangle]
unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n
}
// panic = abort: nothing unwinds, but the prebuilt core still names the personality routine.
#[no_mangle]
extern "C" fn rust_eh_personality() {}
