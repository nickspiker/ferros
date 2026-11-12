//! A TCP client on ferros's network stack: looks a host up with ip's resolver (op 10 RESOLVE: AAAA first where IPv6 has a route, then A), fetches a path over plain HTTP (the `ip` process's client TCP), and prints the status line, the byte count, the time and an FNV-1a 64 of the body to compare with a fetch on the host.
//!
//! Spawn after `ip`: ferros-bridge spawn httptest.bin [hex of "host/path"] --name httptest --use udp
//! With no input it fetches Broadcom's GPS orbit file, gllto2.glpals.com/7day/v5/latest/lto2.dat (GPS.md). Grant slots: 0 console, 1 udp (use).
//! Prefixes on the input: `!` the cellular link only (ip's LINK mode, given back at the end), `~` IPv4 only.

#![no_std]
#![no_main]

use ferros_sys::nr::CALL;
use ferros_sys::{now, svc, ERR};

const DEFAULT: &[u8] = b"gllto2.glpals.com/7day/v5/latest/lto2.dat";
/// A `!` run set ip to the cellular link only.
static CELL_ONLY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

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
            svc(CALL, self.grants + 0x204, self.off(self.log.as_ptr()), self.n as u64, self.off(r.as_mut_ptr()), 16);
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
        svc(CALL, self.grants + 0x204 + 32, self.off(req.as_ptr()), req.len() as u64, self.off(out.as_mut_ptr()), out.len() as u64)
    }
    /// The line, then exit, one shot (owner, 2026-11-08): a `!` run leaves ip's link mode as it set it (ip's own to put back).
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

/// `host`'s address from ip's resolver (op 10 RESOLVE): family `want` (0 either, IPv6 first where it has a route; 4 IPv4 only), the call held until the lookup ends, 16 s at most (two types of four one-second tries each take 8). Ok((family, address)), or Err(the line to print).
fn resolve(a: &App, want: u8, host: &[u8]) -> Result<(u8, [u8; 16]), &'static [u8]> {
    let mut q = [0u8; 3 + 256];
    // WHY: a DNS name is at most 253 bytes (ip refuses a longer one too); the request holds this many.
    if host.len() > 253 {
        return Err(b"httptest: not a host name");
    }
    q[0] = 10;
    q[1] = want;
    q[2] = 1 << 4;
    q[3..3 + host.len()].copy_from_slice(host);
    let mut r = [0u8; 22];
    let n = a.udp(&q[..3 + host.len()], &mut r);
    // `if`s, not a `match` yielding the words (the native payload rule: no pointer tables).
    if n == 0 || n == ERR {
        return Err(b"httptest: nobody present: ip serves applications once someone attests");
    }
    if n < 22 {
        return Err(b"httptest: ip has no resolver (op 10)");
    }
    if r[0] == 0 {
        return Ok((r[1], r[2..18].try_into().unwrap()));
    }
    Err(if r[0] == 2 {
        b"httptest: DNS gave no address (no such name)".as_slice()
    } else if r[0] == 3 {
        b"httptest: not a host name"
    } else if r[0] == 4 {
        b"httptest: no way to ask DNS (no route or no server)"
    } else if r[0] == 5 {
        b"httptest: DNS gave no answer"
    } else {
        b"httptest: DNS still asking"
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
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
    // Test prefixes: `!` the cellular link only (ip's LINK 1, back to both at the end), `~` IPv4 only.
    let mut spec = &spec[..spec_len];
    let mut v4_only = false;
    let mut r = [0u8; 16];
    while let Some(&c) = spec.first() {
        match c {
            b'!' => {
                let _ = a.udp(&[9, 1], &mut r);
                CELL_ONLY.store(true, core::sync::atomic::Ordering::Relaxed);
            }
            b'~' => v4_only = true,
            _ => break,
        }
        spec = &spec[1..];
    }
    let slash = spec.iter().position(|&c| c == b'/').unwrap_or(spec.len());
    let (host, path) = (&spec[..slash], if slash < spec.len() { &spec[slash..] } else { &b"/"[..] });

    // An address of either family and a DNS server (INFO), waiting up to 16 s for DHCP, SLAAC or the cellular call; then the name from ip's resolver (op 10), IPv6 first where it has a route (`~`: IPv4 only).
    let mut out = [0u8; 1 << 14];
    let t0 = now();
    loop {
        let n = a.udp(&[3], &mut out);
        // Nobody present: ip serves an application nothing (payloads/ip, INFO bit 3).
        if n >= 71 && out[50] & 8 == 0 {
            a.done(b"httptest: nobody present: ip serves applications once someone attests");
        }
        // A server of either family will do: a network may name only one (DHCP's, or the router advertisement's).
        if n >= 71 && out[50] & 3 != 0 && (out[51..55] != [0; 4] || out[55..71] != [0; 16]) {
            break;
        }
        if now() - t0 > 16 * ferros_sys::freq() {
            a.done(b"httptest: no address or DNS server");
        }
        sleep_ms(100);
    }
    let t = now();
    let (fam, ip) = match resolve(&a, if v4_only { 4 } else { 0 }, host) {
        Ok(f) => f,
        Err(why) => a.done(why),
    };
    a.put(b"httptest: ");
    a.put(host);
    if fam == 6 {
        a.put(b" is G#");
        for g in 0..8 {
            if g > 0 {
                a.put(b":");
            }
            let d = b"0123456789abcdef";
            for b in [ip[2 * g], ip[2 * g + 1]] {
                a.put(&[d[(b >> 4) as usize], d[(b & 15) as usize]]);
            }
        }
    } else {
        a.put(b" is A#");
        for k in 0..4 {
            if k > 0 {
                a.put(b".");
            }
            a.dec(ip[k] as u64);
        }
    }
    a.put(b" (");
    a.dec(ferros_sys::us_since(t) / 1000);
    a.put(b" ms)\n");
    a.flush();

    // Connect.
    let mut c = [0u8; 20];
    c[0] = 4;
    c[1] = fam;
    c[2..18].copy_from_slice(&ip);
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
    // TCP RECV: held a second at most for data or a change of state once the request is all in ip's send buffer (never a poll), the most data the buffer takes after the state byte.
    let mut q = [6, h, 0, 0, 0, 0, 0];
    q[3..7].copy_from_slice(&(out.len() as u32 - 1).to_be_bytes());
    loop {
        if sent < body {
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
        q[2] = if sent < body { 0 } else { 1 };
        let n = a.udp(&q, &mut out);
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
        // The send buffer took none of the request yet: a millisecond before more of it.
        if n == 1 && q[2] == 0 {
            sleep_ms(1);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
