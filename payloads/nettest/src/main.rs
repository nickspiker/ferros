//! A UDP application on ferros's network stack: prints the addresses the `ip` process holds, then echoes every UDP datagram sent to port 7777 (IPv4 or IPv6) back to its sender.
//!
//! Spawn after `ip`: ferros-bridge spawn nettest.bin --name nettest --use udp
//! Grant slots: 0 console, 1 udp (use). From another machine: `ping`, `ping -6`, and `echo hi | nc -u -w1 <address> 7777`.

#![no_std]
#![no_main]

use ferros_sys::nr::CALL;
use ferros_sys::{svc, ERR};
const PORT: u16 = 7777;

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
    fn dec(&mut self, mut v: u32) {
        let mut b = [0u8; 10];
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
                self.dec(a[k] as u32);
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
    let mut out = [0u8; 2048];
    let mut shown = 0u8;
    loop {
        // Addresses as they appear (INFO flags: bit 0 IPv4 bound, bit 1 IPv6 global).
        let n = a.udp(&[3], &mut out);
        let o = &out;
        if n >= 51 && o[50] != shown {
            shown = o[50];
            a.put(b"addresses: v4 ");
            a.addr(4, &o[6..10]);
            a.put(b" v6 link-local ");
            a.addr(6, &o[18..34]);
            a.put(b" v6 global ");
            a.addr(6, &o[34..50]);
            a.put(b"\n");
            a.flush();
        }
        // Echo port 7777.
        let req = [2u8, (PORT >> 8) as u8, PORT as u8];
        let n = a.udp(&req, &mut out);
        if n != 0 && n != ERR && n >= 21 {
            let o = &out;
            let n = (n as usize).min(o.len());
            let fam = o[0];
            let sport = u16::from_be_bytes([o[17], o[18]]);
            a.put(b"udp from ");
            a.addr(fam, &o[1..17]);
            a.put(b" port ");
            a.dec(sport as u32);
            a.put(b": ");
            for &c in &o[21..n] {
                if (0x20..0x7F).contains(&c) {
                    a.put(&[c]);
                }
            }
            a.put(b"\n");
            let mut s = [0u8; 2048];
            s[0] = 1;
            s[1] = fam;
            s[2..18].copy_from_slice(&o[1..17]);
            s[18..20].copy_from_slice(&sport.to_be_bytes());
            s[20..22].copy_from_slice(&PORT.to_be_bytes());
            let pl = n - 21;
            s[22..22 + pl].copy_from_slice(&o[21..n]);
            let mut r = [0u8; 16];
            // A first send may only resolve the next hop (status 1): try a few times.
            for _ in 0..50 {
                let st = a.udp(&s[..22 + pl], &mut r);
                let rr = &r;
                if st == 1 && rr[0] != 1 {
                    break;
                }
            }
            a.flush();
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
