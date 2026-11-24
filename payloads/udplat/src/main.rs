//! UDP latency bench (docs/WIFI.md "Datagrams at speed"): the device half of tools/udplat, measuring the datagram path between a process and the wire, each way, on ip's `udp` endpoint.
//!
//! Spawn after `ip`: ferros-bridge spawn udplat.bin --name udplat --use udp
//! Grant slots: 0 console, 1 udp (use).
//!
//! It listens on port 7778 (one held receive at a time) and answers by the first byte of what lands:
//! - `E` echo: `[E][seq u32 LE][host time u64 LE][class u8][pad to 16]...` (class: ferros_netring CLASS_*, sent with ip's op 17) comes back at once, the same bytes with four u64 LE written at 16..48: the counter when the receive returned, the counter just before the send, the counter's ticks a second, and the previous echo's send call in ticks (call to reply).
//! - `B` bulk alongside: `[B][burst u16 LE][size u16 LE][period us u32 LE][bursts u32 LE][port u16 LE]`: a second thread sends `bursts` bursts of `burst` datagrams of `size` bytes, a burst every `period` µs, to the asker's address at `port`, as a beam's slices would go (`[b][seq u32 LE]` then zeros).
//! - `H`: nothing (the host opening its firewall's flow to a port of its own).
//!
//! The bulk thread's pacing is the bench's own load shape, the one timer here; the echo path has none (a held receive, then a send).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::nr::CALL;
use ferros_sys::{now, svc, ERR};

const PORT: u16 = 7778;
/// The receive's request: op 2, the port, a wait of 255 s (held until a datagram lands).
const RECV_REQ: [u8; 4] = [2, (PORT >> 8) as u8, PORT as u8, 255];
/// ip's reply header ahead of the payload: family, address (16), source port, destination port.
const HEAD: usize = 21;
/// The echo's stamps: four u64 at 16..48.
const STAMPS: usize = 48;

/// The bulk thread's orders: family, address (16), port, burst, size, period µs, bursts; `GO` counts orders given.
static GO: AtomicU64 = AtomicU64::new(0);
static mut ORDER: [u8; 32] = [0; 32];
static mut BULK_STACK: [u8; 1 << 15] = [0; 1 << 15];

fn cap(grants: u64, slot: u64) -> u64 {
    grants + 0x204 + 32 * slot
}

fn off<T>(p: *const T) -> u64 {
    p as u64 - ferros_sys::arc_base()
}

fn udp(grants: u64, req: &[u8], out: &mut [u8]) -> u64 {
    svc(CALL, cap(grants, 1), off(req.as_ptr()), req.len() as u64, off(out.as_mut_ptr()), out.len() as u64)
}

/// One datagram out: op 17 (SEND with a class, ferros_netring CLASS_*), retried while ip resolves the next hop (status 1), as nettest does.
fn send(grants: u64, class: u8, fam: u8, addr: &[u8], dport: u16, payload: &[u8], s: &mut [u8]) {
    s[0] = 17;
    s[1] = class;
    s[2] = fam;
    s[3..19].copy_from_slice(addr);
    s[19..21].copy_from_slice(&dport.to_be_bytes());
    s[21..23].copy_from_slice(&PORT.to_be_bytes());
    s[23..23 + payload.len()].copy_from_slice(payload);
    let mut r = [0u8; 4];
    for _ in 0..1 << 6 {
        let n = udp(grants, &s[..23 + payload.len()], &mut r);
        if !(n == 1 && r[0] == 1) {
            break;
        }
    }
}

fn say(grants: u64, s: &[u8]) {
    let mut r = [0u8; 16];
    svc(CALL, cap(grants, 0), off(s.as_ptr()), s.len() as u64, off(r.as_mut_ptr()), 16);
}

extern "C" fn bulk_thread(grants: u64) -> ! {
    let mut seen = 0;
    let mut s = [0u8; 2048];
    let mut d = [0u8; 1500];
    let f = ferros_sys::freq();
    loop {
        while GO.load(Ordering::Acquire) == seen {
            ferros_sys::wait_world(&GO, seen, 0);
        }
        seen = GO.load(Ordering::Acquire);
        // SAFETY: written by the main thread before it counted GO up, read here after.
        let o = unsafe { *(&raw const ORDER) };
        let port = u16::from_le_bytes([o[17], o[18]]);
        let burst = u16::from_le_bytes([o[19], o[20]]) as u32;
        let size = u16::from_le_bytes([o[21], o[22]]) as usize;
        let period = u32::from_le_bytes([o[23], o[24], o[25], o[26]]) as u64 * f / 1_000_000;
        let bursts = u32::from_le_bytes([o[27], o[28], o[29], o[30]]);
        // WHY the min: the host's word on a size, against the frame this sends from (an IPv4 datagram in one 1500-byte frame).
        let size = size.min(1472).max(5);
        let mut seq = 0u32;
        let mut due = now();
        for _ in 0..bursts {
            for _ in 0..burst {
                d[0] = b'b';
                d[1..5].copy_from_slice(&seq.to_le_bytes());
                send(grants, 0, o[0], &o[1..17], port, &d[..size], &mut s);
                seq += 1;
            }
            due += period;
            ferros_sys::sleep_until(due);
        }
        say(grants, b"udplat: bulk done\n");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, input_len: u64) -> ! {
    // Any spawn input: the echo thru a socket ring (op 15, ferros_netring) in place of op 2 and op 17.
    if input_len != 0 {
        ring_main(grants);
    }
    let mut out = [0u8; 2048];
    let mut s = [0u8; 2048];
    let mut prev_call = 0u64;
    let f = ferros_sys::freq();
    // SAFETY: the bulk thread's stack, given to it alone, once.
    let threaded = ferros_sys::thread_spawn(bulk_thread, unsafe { &mut *(&raw mut BULK_STACK) }, grants) != ERR;
    // The address to aim at, said once: INFO's IPv4 (6..10), as A#a.b.c.d.
    let n = udp(grants, &[3], &mut out);
    if n != ERR && n >= 10 {
        let mut l = *b"udplat: IPv4 A#___.___.___.___\n";
        for k in 0..4 {
            let v = out[6 + k];
            l[15 + 4 * k] = b'0' + v / 100;
            l[16 + 4 * k] = b'0' + v / 10 % 10;
            l[17 + 4 * k] = b'0' + v % 10;
        }
        say(grants, &l);
    }
    say(grants, if threaded { b"udplat: listening on 7778\n".as_slice() } else { b"udplat: listening on 7778, no bulk thread\n" });
    loop {
        let n = udp(grants, &RECV_REQ, &mut out);
        let t_recv = now();
        if n == ERR || (n as usize) < HEAD + 1 {
            continue;
        }
        // WHY the min: the length is the kernel's, the reply cut at the buffer.
        let n = (n as usize).min(out.len());
        let fam = out[0];
        let mut addr = [0u8; 16];
        addr.copy_from_slice(&out[1..17]);
        let sport = u16::from_be_bytes([out[17], out[18]]);
        let p = &mut out[HEAD..n];
        match p[0] {
            b'E' if p.len() >= STAMPS => {
                p[16..24].copy_from_slice(&t_recv.to_le_bytes());
                p[32..40].copy_from_slice(&f.to_le_bytes());
                p[40..48].copy_from_slice(&prev_call.to_le_bytes());
                let t_send = now();
                p[24..32].copy_from_slice(&t_send.to_le_bytes());
                let pl = p.len();
                let mut d = [0u8; 1500];
                // WHY the min: the host's datagram, echoed from a frame-sized buffer.
                let pl = pl.min(d.len());
                d[..pl].copy_from_slice(&p[..pl]);
                // The host's byte 13: the echo's class (0 best effort, 2 voice).
                send(grants, d[13], fam, &addr, sport, &d[..pl], &mut s);
                prev_call = now() - t_send;
            }
            b'B' if p.len() >= 15 => {
                // SAFETY: the bulk thread reads ORDER only after GO counts up, and is between runs (a new order while one runs takes effect at its next).
                let o = unsafe { &mut *(&raw mut ORDER) };
                o[0] = fam;
                o[1..17].copy_from_slice(&addr);
                o[17..19].copy_from_slice(&p[13..15]);
                o[19..31].copy_from_slice(&p[1..13]);
                GO.fetch_add(1, Ordering::Release);
                ferros_sys::wake(&GO, 1);
            }
            _ => {}
        }
    }
}

/// A word of the ring's run at `addr` (its address in the shared window, the same in ip).
fn word(addr: u64) -> &'static AtomicU64 {
    // SAFETY: the run is mapped read-write for this process while it holds the view (docs/KERNEL.md "Shared regions"), and `addr` is an aligned word inside it.
    unsafe { &*(addr as *const AtomicU64) }
}

/// The echo thru a socket ring: RING OPEN on 7778 (4 KiB each way), the run's address from the kernel, then each datagram answered from the ring with the stamps as in op 2's echo, the reader asleep on the ring's doorbell between them.
fn ring_main(grants: u64) -> ! {
    let req = [15u8, (PORT >> 8) as u8, PORT as u8, 12, 12];
    let mut r = [0u8; 17];
    let n = udp(grants, &req, &mut r);
    let id = u64::from_be_bytes(r[1..9].try_into().unwrap());
    let view = if n == 17 && r[0] == 0 { ferros_sys::share::view(id) } else { None };
    let Some((addr, bytes, _)) = view else {
        say(grants, b"udplat: ring open refused\n");
        loop {
            ferros_sys::wait_world(&GO, GO.load(Ordering::Acquire), 0);
        }
    };
    let Some(mut app) = ferros_netring::App::open(ferros_netring::Raw(addr as *mut u64), bytes as usize) else {
        say(grants, b"udplat: the ring is not laid out\n");
        loop {
            ferros_sys::wait_world(&GO, GO.load(Ordering::Acquire), 0);
        }
    };
    say(grants, b"udplat: echoing thru a ring on 7778\n");
    // SAFETY: the bulk thread's stack, given to it alone, once.
    ferros_sys::thread_spawn(bulk_thread, unsafe { &mut *(&raw mut BULK_STACK) }, grants);
    let f = ferros_sys::freq();
    let mut buf = [0u8; 2048];
    let mut prev_call = 0u64;
    loop {
        match app.recv(&mut buf) {
            Ok(rec) => {
                let t_recv = now();
                let p = &mut buf[..rec.len];
                if p.len() >= STAMPS && p[0] == b'E' {
                    p[16..24].copy_from_slice(&t_recv.to_le_bytes());
                    p[32..40].copy_from_slice(&f.to_le_bytes());
                    p[40..48].copy_from_slice(&prev_call.to_le_bytes());
                    let t_send = now();
                    p[24..32].copy_from_slice(&t_send.to_le_bytes());
                    if let Ok(true) = app.send(rec.fam, rec.addr, rec.port, p[13], t_send, p) {
                        ferros_sys::wake(word(addr + app.wake_word() as u64), 1);
                    }
                    prev_call = now() - t_send;
                } else if p.len() >= 15 && p[0] == b'B' {
                    // SAFETY: as in the op 2 loop.
                    let o = unsafe { &mut *(&raw mut ORDER) };
                    o[0] = rec.fam;
                    o[1..17].copy_from_slice(&rec.addr);
                    o[17..19].copy_from_slice(&p[13..15]);
                    o[19..31].copy_from_slice(&p[1..13]);
                    GO.fetch_add(1, Ordering::Release);
                    ferros_sys::wake(&GO, 1);
                }
            }
            Err(ferros_netring::RecvErr::Empty) => {
                if let Some((w, v)) = app.arm_rx() {
                    ferros_sys::wait_world(word(addr + w as u64), v, 0);
                    app.disarm();
                }
            }
            Err(_) => {
                say(grants, b"udplat: the ring closed\n");
                loop {
                    ferros_sys::wait_world(&GO, GO.load(Ordering::Acquire), 0);
                }
            }
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

ferros_sys::entry!(payload_main);
