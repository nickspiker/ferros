//! A TCP client on ferros's network stack: looks a host up through the DHCP-given DNS server (UDP), fetches a path over plain HTTP (the `ip` process's client TCP), and prints the status line, the byte count, the time and an FNV-1a 64 of the body to compare with a fetch on the host.
//!
//! Spawn after `ip`: ferros-bridge spawn httptest.bin [hex of "host/path"] --name httptest --use udp
//! With no input it fetches Broadcom's GNSS orbit file, gllto2.glpals.com/7day/v5/latest/lto2.dat (GNSS.md). Grant slots: 0 console, 1 udp (use).

#![no_std]
#![no_main]

use ferros_sys::nr::CALL;
use ferros_sys::{now, svc, ERR};

const DEFAULT: &[u8] = b"gllto2.glpals.com/7day/v5/latest/lto2.dat";
const DNS_PORT: u16 = 53053;

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
    fn hex(&mut self, v: u64, digits: u32) {
        let h = b"0123456789ABCDEF";
        self.put(b"G#");
        for k in (0..digits).rev() {
            self.put(&[h[((v >> (4 * k)) & 15) as usize]]);
        }
    }
    fn udp(&self, req: &[u8], out: &mut [u8]) -> u64 {
        svc(CALL, self.grants + 4 + 32, self.off(req.as_ptr()), req.len() as u64, self.off(out.as_mut_ptr()), out.len() as u64)
    }
    fn done(&mut self, msg: &[u8]) -> ! {
        self.put(msg);
        self.put(b"\n");
        self.flush();
        ferros_sys::exit(0)
    }
}

fn sleep_ms(ms: u64) {
    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000 * ms);
}

/// The A record for `host` from the server `dns`, or None after three tries a second apart.
fn resolve(a: &App, dns: [u8; 4], host: &[u8]) -> Option<[u8; 4]> {
    let mut q = [0u8; 300];
    q[0] = 1;
    q[1] = 4;
    q[2..6].copy_from_slice(&dns);
    q[18..20].copy_from_slice(&53u16.to_be_bytes());
    q[20..22].copy_from_slice(&DNS_PORT.to_be_bytes());
    let id = (now() as u16) | 1;
    let p = &mut q[22..];
    p[0..2].copy_from_slice(&id.to_be_bytes());
    p[2] = 1; // recursion desired
    p[5] = 1; // one question
    let mut k = 12;
    for label in host.split(|&c| c == b'.') {
        p[k] = label.len() as u8;
        p[k + 1..k + 1 + label.len()].copy_from_slice(label);
        k += 1 + label.len();
    }
    p[k] = 0;
    p[k + 1..k + 5].copy_from_slice(&[0, 1, 0, 1]);
    let qlen = k + 5;
    let mut out = [0u8; 1024];
    for _ in 0..3 {
        let mut r = [0u8; 16];
        for _ in 0..50 {
            if a.udp(&q[..22 + qlen], &mut r) == 1 && r[0] != 1 {
                break;
            }
            sleep_ms(2);
        }
        let t = now();
        while now() - t < ferros_sys::freq() {
            let n = a.udp(&[2, (DNS_PORT >> 8) as u8, DNS_PORT as u8], &mut out);
            if n == 0 || n == ERR || n < 21 + 12 {
                sleep_ms(2);
                continue;
            }
            let m = &out[21..(n as usize).min(out.len())];
            if m[0..2] != id.to_be_bytes() {
                continue;
            }
            let an = u16::from_be_bytes([m[6], m[7]]);
            // Past the question, then each answer: a name (labels or a pointer), type, class, TTL, length, data.
            let mut i = 12;
            let skip = |m: &[u8], mut i: usize| -> usize {
                while i < m.len() {
                    if m[i] == 0 {
                        return i + 1;
                    }
                    if m[i] & 0xC0 == 0xC0 {
                        return i + 2;
                    }
                    i += 1 + m[i] as usize;
                }
                m.len()
            };
            i = skip(m, i) + 4;
            for _ in 0..an {
                i = skip(m, i);
                if i + 10 > m.len() {
                    break;
                }
                let ty = u16::from_be_bytes([m[i], m[i + 1]]);
                let len = u16::from_be_bytes([m[i + 8], m[i + 9]]) as usize;
                i += 10;
                if ty == 1 && len == 4 && i + 4 <= m.len() {
                    return Some([m[i], m[i + 1], m[i + 2], m[i + 3]]);
                }
                i += len;
            }
            return None;
        }
    }
    None
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    let base = ferros_sys::arc_base();
    let mut a = App { grants, base, log: [0; 512], n: 0 };
    let mut spec = [0u8; 256];
    let spec_len = if input_len > 0 {
        let n = (input_len as usize).min(spec.len());
        for k in 0..n {
            spec[k] = unsafe { core::ptr::read_volatile((base + input + k as u64) as *const u8) };
        }
        n
    } else {
        spec[..DEFAULT.len()].copy_from_slice(DEFAULT);
        DEFAULT.len()
    };
    let spec = &spec[..spec_len];
    let slash = spec.iter().position(|&c| c == b'/').unwrap_or(spec.len());
    let (host, path) = (&spec[..slash], if slash < spec.len() { &spec[slash..] } else { &b"/"[..] });

    // The DNS server and an IPv4 lease (INFO), waiting up to 16 s for DHCP.
    let mut out = [0u8; 4096];
    let mut dns = [0u8; 4];
    let t0 = now();
    loop {
        let n = a.udp(&[3], &mut out);
        if n >= 55 && out[50] & 1 != 0 && out[51..55] != [0; 4] {
            dns.copy_from_slice(&out[51..55]);
            break;
        }
        if now() - t0 > 16 * ferros_sys::freq() {
            a.done(b"httptest: no IPv4 lease or DNS server");
        }
        sleep_ms(100);
    }
    let t = now();
    let Some(ip) = resolve(&a, dns, host) else { a.done(b"httptest: DNS gave no address") };
    a.put(b"httptest: ");
    a.put(host);
    a.put(b" is A#");
    for k in 0..4 {
        if k > 0 {
            a.put(b".");
        }
        a.dec(ip[k] as u64);
    }
    a.put(b" (");
    a.dec(ferros_sys::us_since(t) / 1000);
    a.put(b" ms)\n");
    a.flush();

    // Connect.
    let mut c = [0u8; 20];
    c[0] = 4;
    c[1] = 4;
    c[2..6].copy_from_slice(&ip);
    c[18..20].copy_from_slice(&80u16.to_be_bytes());
    let t = now();
    let mut r = [0u8; 16];
    let n = a.udp(&c, &mut r);
    if n != 2 || r[0] != 0 {
        a.put(b"httptest: connect refused, status ");
        a.dec(r[0] as u64);
        a.done(b"");
    }
    let h = r[1];

    // The request, through the send buffer as it takes it.
    let mut req = [0u8; 600];
    req[0] = 5;
    req[1] = h;
    let mut k = 2;
    for part in [&b"GET "[..], path, b" HTTP/1.1\r\nHost: ", host, b"\r\nUser-Agent: ferros\r\nConnection: close\r\n\r\n"] {
        req[k..k + part.len()].copy_from_slice(part);
        k += part.len();
    }
    let body = req[2..k].len();
    let mut sent = 0;
    let mut fnv: u64 = 0xcbf2_9ce4_8422_2325;
    let mut total = 0u64;
    let mut head = [0u8; 512];
    let mut head_len = 0usize;
    let mut in_body = false;
    let mut tail = 0u32;
    let mut head_total = 0u64;
    let mut connected = None;
    loop {
        if sent < body && connected.is_some() {
            let mut s = [0u8; 600];
            s[0] = 5;
            s[1] = h;
            let left = body - sent;
            s[2..2 + left].copy_from_slice(&req[2 + sent..2 + body]);
            let n = a.udp(&s[..2 + left], &mut r);
            if n == 3 && r[0] == 0 {
                sent += u16::from_be_bytes([r[1], r[2]]) as usize;
            }
        }
        let n = a.udp(&[6, h], &mut out);
        if n == 0 || n == ERR {
            a.done(b"httptest: recv failed");
        }
        let n = n as usize;
        let state = out[0];
        if state >= 2 && connected.is_none() {
            connected = Some(ferros_sys::us_since(t));
        }
        for &b in &out[1..n] {
            if in_body {
                fnv = (fnv ^ b as u64).wrapping_mul(0x100_0000_01b3);
                total += 1;
            } else {
                // The headers' end by a rolling window of the last four bytes (the headers themselves can outrun the buffer kept for the status line).
                if head_len < head.len() {
                    head[head_len] = b;
                    head_len += 1;
                }
                head_total += 1;
                tail = tail << 8 | b as u32;
                if tail == 0x0D0A_0D0A {
                    in_body = true;
                }
            }
        }
        if state >= 3 && n == 1 {
            let ms = ferros_sys::us_since(t) / 1000;
            let _ = a.udp(&[7, h], &mut r);
            let line_end = head[..head_len].iter().position(|&c| c == b'\r').unwrap_or(head_len);
            a.put(b"httptest: ");
            let hl = head;
            a.put(&hl[..line_end]);
            a.put(b", connected in ");
            a.dec(connected.unwrap_or(0) / 1000);
            a.put(b" ms, headers ");
            a.dec(head_total);
            a.put(b" bytes, body ");
            a.dec(total);
            a.put(b" bytes in ");
            a.dec(ms);
            a.put(b" ms, fnv64 ");
            a.hex(fnv, 16);
            a.put(if state == 4 { b" (connection failed)" } else { b"" });
            a.done(b"");
        }
        if n == 1 {
            sleep_ms(1);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
