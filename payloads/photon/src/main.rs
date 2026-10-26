//! Photon on ferros, first layer: LAN presence. Opens Photon's ports on the `ip` process (4383 for frames, 4384 for the LAN beacons the stock app sends to 239.104.199.144 and ff02::68c7:9014 every few seconds) and reports, once per sender, what it hears: the sender's address and port and the frame's VSF section name (`pt_disc` for a beacon). The beacon's handle proof stays off the console. Runs 32 s, then a count.
//!
//! Spawn after `ip`: ferros-bridge spawn photon.bin --name photon --use udp
//! Grant slots: 0 console, 1 udp (use). Next layers (STATUS.md, "Enrollment / pairing"): the ira-derived device key, the fleet join, then notes-to-self pages to a sibling.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use ferros_sys::nr::CALL;
use ferros_sys::{now, svc, ERR};
use photon_core::wire::{self, Frame};

/// The heap lives in `.data`, not `.bss` (the kernel sizes an arc from the flat image, which carries no `.bss`): frame parsing's few small allocations, all freed before the next frame.
const HEAP_BYTES: usize = 64 * 1024;

#[repr(C, align(16))]
struct Heap(UnsafeCell<[u8; HEAP_BYTES]>);
unsafe impl Sync for Heap {}
#[unsafe(link_section = ".data.heap")]
static HEAP: Heap = Heap(UnsafeCell::new([1; HEAP_BYTES]));

/// A bump allocator that starts over whenever every allocation has been freed (once per frame here). Single-threaded, so plain cells suffice.
struct Bump {
    next: UnsafeCell<usize>,
    live: UnsafeCell<usize>,
}
unsafe impl Sync for Bump {}

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let base = HEAP.0.get() as usize;
        let start = (base + *self.next.get() + l.align() - 1) & !(l.align() - 1);
        if start + l.size() > base + HEAP_BYTES {
            return core::ptr::null_mut();
        }
        *self.next.get() = start + l.size() - base;
        *self.live.get() += 1;
        start as *mut u8
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
        *self.live.get() -= 1;
        if *self.live.get() == 0 {
            *self.next.get() = 0;
        }
    }
}

#[global_allocator]
static ALLOC: Bump = Bump { next: UnsafeCell::new(0), live: UnsafeCell::new(0) };

const PORTS: [u16; 2] = [4383, 4384];

struct App {
    grants: u64,
    base: u64,
    log: [u8; 512],
    n: usize,
}

impl App {
    fn off<T>(&self, p: *const T) -> u64 {
        p as u64 - self.base
    }
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.log.len() {
                self.log[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn flush(&mut self) {
        if self.n > 0 {
            let mut r = [0u8; 16];
            svc(CALL, self.grants + 4, self.off(self.log.as_ptr()), self.n as u64, self.off(r.as_mut_ptr()), 16);
            self.n = 0;
        }
    }
    fn dec(&mut self, mut v: u64) {
        let mut b = [0u8; 20];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let d = b;
        self.put(&d[i..]);
    }
    fn addr(&mut self, fam: u8, a: &[u8]) {
        if fam == 4 {
            self.put(b"A#");
            for k in 0..4 {
                if k > 0 {
                    self.put(b".");
                }
                self.dec(a[k] as u64);
            }
        } else {
            let h = b"0123456789abcdef";
            self.put(b"G#");
            for g in 0..8 {
                if g > 0 {
                    self.put(b":");
                }
                for k in 0..2 {
                    let b = a[2 * g + k];
                    self.put(&[h[(b >> 4) as usize], h[(b & 15) as usize]]);
                }
            }
        }
    }
    fn udp(&self, req: &[u8], out: &mut [u8]) -> u64 {
        svc(CALL, self.grants + 4 + 32, self.off(req.as_ptr()), req.len() as u64, self.off(out.as_mut_ptr()), out.len() as u64)
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    let base = ferros_sys::arc_base();
    let mut a = App { grants, base, log: [0; 512], n: 0 };
    let mut out = [0u8; 1600];
    let mut seen = [([0u8; 16], 0u16, 0u16); 16];
    let mut seen_n = 0;
    let mut total = [0u64; 2];
    let freq = ferros_sys::freq();
    let t0 = now();
    // Open both ports before anything can arrive.
    for p in PORTS {
        let _ = a.udp(&[2, (p >> 8) as u8, p as u8], &mut out);
    }
    a.put(b"photon: listening on 4383 and 4384\n");
    a.flush();
    while now() - t0 < 32 * freq {
        let mut any = false;
        for (pi, p) in PORTS.iter().enumerate() {
            let n = a.udp(&[2, (p >> 8) as u8, *p as u8], &mut out);
            if n == 0 || n == ERR || n < 21 {
                continue;
            }
            any = true;
            total[pi] += 1;
            let o = &out;
            let n = (n as usize).min(o.len());
            let fam = o[0];
            let src: [u8; 16] = o[1..17].try_into().unwrap();
            let sport = u16::from_be_bytes([o[17], o[18]]);
            if seen[..seen_n].iter().any(|e| e.0 == src && e.1 == sport && e.2 == *p) {
                continue;
            }
            if seen_n < seen.len() {
                seen[seen_n] = (src, sport, *p);
                seen_n += 1;
            }
            // What the frame is: photon_core's parse, the section name for kinds it doesn't handle (`pt_disc`, the beacon).
            let mut name = [0u8; 24];
            let sl = {
                let kind: &[u8] = match wire::parse(&o[21..n]) {
                    None => b"(not VSF)",
                    Some(Frame::Ping { .. }) => b"ping",
                    Some(Frame::Pong { .. }) => b"pong",
                    Some(Frame::Photon { .. }) => b"msg",
                    Some(Frame::Ack { .. }) => b"ack",
                    Some(Frame::PtAck { .. }) => b"pt_ack",
                    Some(Frame::Other(sec)) => {
                        let k = sec.len().min(name.len());
                        name[..k].copy_from_slice(&sec.as_bytes()[..k]);
                        &[]
                    }
                };
                if kind.is_empty() {
                    name.iter().position(|&c| c == 0).unwrap_or(name.len())
                } else {
                    name[..kind.len()].copy_from_slice(kind);
                    kind.len()
                }
            };
            a.put(b"photon: port ");
            a.dec(*p as u64);
            a.put(b" from ");
            a.addr(fam, &src);
            a.put(b" port ");
            a.dec(sport as u64);
            a.put(b": ");
            a.dec((n - 21) as u64);
            a.put(b" bytes, section ");
            a.put(&name[..sl]);
            a.put(b"\n");
            a.flush();
        }
        if !any {
            ferros_sys::sleep_until(now() + freq / 1000 * 4);
        }
    }
    a.put(b"photon: 32 s: ");
    a.dec(total[0]);
    a.put(b" frames on 4383, ");
    a.dec(total[1]);
    a.put(b" beacons on 4384\n");
    a.flush();
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
