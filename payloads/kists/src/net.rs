//! A plain HTTP GET through the `ip` process (payloads/ip, its `udp` endpoint): the host's A record from the DNS server ip reports, a client TCP connection, the request, and the response decoded as it arrives (ferros_kist::fetch::Response), its body handed on a piece at a time and never held whole.
//!
//! Every wait is a receive ip holds until something comes (the wait byte of its ops 2 and 6), never a poll (owner, 2026-11-10: "No polling!"): the worker blocks in the call. The deadlines are real ones: DNS's second per try, and the stall below.
//! Two retries remain, each with its reason: a question ip could not send yet because it is resolving the next hop (its answer 1, nothing rings when the neighbour answers) is asked again a millisecond on, a few dozen times; a connection refused for want of a free one (ip holds sixteen) is asked again each second, a few times.

use ferros_kist::fetch::{split_url, Response};
use ferros_sys::{call, freq, now, sleep_until, ERR};

/// The port this side's DNS questions leave from and their answers come to, its own: ip hands a port's datagrams to whoever asks for that port, and fgtw's 53054 was this one's too until 2026-11-12, each taking the other's answers off the port when both asked at once (httptest 53053, fgtw 53054, Mahere 53055).
const DNS_PORT: u16 = 53056;
/// How long a connection may bring nothing before it is given up (WHY a deadline: ip fails a connection only when what it sent goes unanswered, and a GET's long body is all receiving; a server that falls silent would hold the stower's one worker for ever). 32 s, ip's own idle reaping time: the wait of each held receive.
const STALL_S: u8 = 1 << 5;
/// The most data one receive brings: 64 KiB, on the worker's 512 KiB stack (ip answers up to 256 KiB; a bigger reply means fewer round trips while a capsule streams in).
const RECV_MOST: usize = 1 << 16;

/// Why a GET did not bring its body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fail {
    /// No address, no DNS server, the name not found, or no connection free.
    Network,
    /// The server answered without it (any status but 200: the status).
    Missing(u16),
    /// The transfer broke, or the caller stopped it (its reason).
    Broken(&'static str),
}

fn ask(cap: u64, req: &[u8], out: &mut [u8]) -> u64 {
    call(cap, req, out)
}

/// A millisecond's wait before asking ip to send again while it resolves the next hop (the module's WHY).
fn pause() {
    sleep_until(now() + (freq() >> 10));
}

/// The IPv4 DNS server ip uses, once it has an IPv4 address; None when it has none.
fn dns_server(cap: u64) -> Option<[u8; 4]> {
    let mut out = [0u8; 128];
    let n = ask(cap, &[3], &mut out);
    // INFO: flags at 50 (bit 0 IPv4 bound), the DNS server at 51.
    if n == ERR || n < 55 || out[50] & 1 == 0 || out[51..55] == [0; 4] {
        return None;
    }
    Some(out[51..55].try_into().unwrap())
}

/// `host`'s first A record from `dns`, three tries a second apart.
fn resolve(cap: u64, dns: [u8; 4], host: &[u8]) -> Option<[u8; 4]> {
    // WHY: a DNS name is at most 253 bytes; the question below holds it with room.
    if host.is_empty() || host.len() > 253 {
        return None;
    }
    let mut q = [0u8; 300];
    q[0] = 1;
    q[1] = 4;
    q[2..6].copy_from_slice(&dns);
    q[18..20].copy_from_slice(&53u16.to_be_bytes());
    q[20..22].copy_from_slice(&DNS_PORT.to_be_bytes());
    let id = (now() as u16) | 1;
    let p = &mut q[22..];
    p[0..2].copy_from_slice(&id.to_be_bytes());
    // Recursion desired, one question.
    p[2] = 1;
    p[5] = 1;
    let mut k = 12;
    for label in host.split(|&c| c == b'.') {
        // WHY: a DNS label is 1 to 63 bytes; anything else is no name a server answers.
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        p[k] = label.len() as u8;
        p[k + 1..k + 1 + label.len()].copy_from_slice(label);
        k += 1 + label.len();
    }
    p[k] = 0;
    p[k + 1..k + 3].copy_from_slice(&1u16.to_be_bytes());
    p[k + 3..k + 5].copy_from_slice(&[0, 1]);
    let qlen = 22 + k + 5;
    let mut out = [0u8; 1024];
    for _ in 0..3 {
        let mut r = [0u8; 16];
        // ip resolving its next hop answers 1: asked again a millisecond on, a few dozen times.
        for _ in 0..1 << 6 {
            if ask(cap, &q[..qlen], &mut r) == 1 && r[0] != 1 {
                break;
            }
            pause();
        }
        // Answers held for up to a second (the wait byte), until ours; another's answer on the port (an old try's) is passed over.
        let t = now();
        while now() - t < freq() {
            let n = ask(cap, &[2, (DNS_PORT >> 8) as u8, DNS_PORT as u8, 1], &mut out);
            if n == 0 || n == ERR {
                break;
            }
            if n < 21 + 12 {
                continue;
            }
            // WHY: the length is ip's, at most what the buffer took.
            let m = &out[21..(n as usize).min(out.len())];
            // WHY the sender's check: the answer is the network's; only the server asked, from port 53, answers this question (anyone on the network can send to the port).
            if m[0..2] != id.to_be_bytes() || out[0] != 4 || out[1..5] != dns || out[17..19] != 53u16.to_be_bytes() {
                continue;
            }
            return a_record(m);
        }
    }
    None
}

/// The first A record of a DNS answer.
fn a_record(m: &[u8]) -> Option<[u8; 4]> {
    let answers = u16::from_be_bytes([m[6], m[7]]);
    // A name: labels to a zero, or a pointer. WHY the bounds: the message is the server's.
    let skip = |mut i: usize| -> usize {
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
    let mut i = skip(12) + 4;
    for _ in 0..answers {
        i = skip(i);
        if i + 10 > m.len() {
            return None;
        }
        let ty = u16::from_be_bytes([m[i], m[i + 1]]);
        let len = u16::from_be_bytes([m[i + 8], m[i + 9]]) as usize;
        i += 10;
        if ty == 1 && len == 4 && i + 4 <= m.len() {
            return Some(m[i..i + 4].try_into().unwrap());
        }
        i += len;
    }
    None
}

/// The last name resolved and its address, so a restore of several capsules asks once.
static mut CACHE: ([u8; 64], usize, [u8; 4]) = ([0; 64], 0, [0; 4]);

fn address(cap: u64, host: &str) -> Option<[u8; 4]> {
    let c = unsafe { &mut *(&raw mut CACHE) };
    if c.1 == host.len() && &c.0[..c.1] == host.as_bytes() {
        return Some(c.2);
    }
    let ip = resolve(cap, dns_server(cap)?, host.as_bytes())?;
    // WHY: only names that fit the cache are kept; a longer one is resolved each time.
    if host.len() <= c.0.len() {
        c.0[..host.len()].copy_from_slice(host.as_bytes());
        c.1 = host.len();
        c.2 = ip;
    }
    Some(ip)
}

/// A TCP connection to `ip`:`port`. ip holds sixteen; a full table frees as others finish, so it is asked again each second a few times.
fn connect(cap: u64, ip: [u8; 4], port: u16) -> Option<u8> {
    let mut c = [0u8; 20];
    c[0] = 4;
    c[1] = 4;
    c[2..6].copy_from_slice(&ip);
    c[18..20].copy_from_slice(&port.to_be_bytes());
    for _ in 0..1 << 3 {
        let mut r = [0u8; 16];
        let n = ask(cap, &c, &mut r);
        if n == 2 && r[0] == 0 {
            return Some(r[1]);
        }
        // 4: no free connection; anything else will not change by asking.
        if n != 1 || r[0] != 4 {
            return None;
        }
        sleep_until(now() + freq());
    }
    None
}

/// GET `url` (`http://host[:port]/path`): `head` sees the response once its headers are in (it may refuse it: a length that is not the one expected), then `body` takes the body a piece at a time. Ok once the whole body came.
pub fn get(cap: u64, url: &str, mut head: impl FnMut(&Response) -> Result<(), &'static str>, mut body: impl FnMut(&[u8]) -> Result<(), &'static str>) -> Result<(), Fail> {
    let (host, port, path) = split_url(url).ok_or(Fail::Broken("not a plain http URL"))?;
    let ip = address(cap, host).ok_or(Fail::Network)?;
    let h = connect(cap, ip, port).ok_or(Fail::Network)?;
    let r = transfer(cap, h, host, path, &mut head, &mut body);
    let mut x = [0u8; 4];
    let _ = ask(cap, &[7, h], &mut x);
    r
}

fn transfer(cap: u64, h: u8, host: &str, path: &str, head: &mut impl FnMut(&Response) -> Result<(), &'static str>, body: &mut impl FnMut(&[u8]) -> Result<(), &'static str>) -> Result<(), Fail> {
    // The request: one line, the host, nothing kept alive.
    let mut req = [0u8; 600];
    req[0] = 5;
    req[1] = h;
    let mut k = 2;
    for part in [b"GET ".as_slice(), path.as_bytes(), b" HTTP/1.1\r\nHost: ", host.as_bytes(), b"\r\nUser-Agent: ferros\r\nConnection: close\r\n\r\n"] {
        // WHY: the path is the directory's or the person's list's; one longer than the request holds is no capsule this fetches.
        if k + part.len() > req.len() {
            return Err(Fail::Broken("a URL too long to ask for"));
        }
        req[k..k + part.len()].copy_from_slice(part);
        k += part.len();
    }
    let total = k - 2;
    let mut sent = 0;
    let mut resp = Response::new();
    let mut headed = false;
    let mut out = [0u8; 1 + RECV_MOST];
    let mut hold = [6, h, STALL_S, 0, 0, 0, 0];
    hold[3..7].copy_from_slice(&(RECV_MOST as u32).to_be_bytes());
    // The state ip answered last: a held receive is also answered when it changes (the connection opened).
    let mut was = 1u8;
    loop {
        // What the send buffer has not taken yet, each pass until it is all out.
        if sent < total {
            let mut s = [0u8; 600];
            s[0] = 5;
            s[1] = h;
            let left = total - sent;
            s[2..2 + left].copy_from_slice(&req[2 + sent..2 + total]);
            let mut r = [0u8; 4];
            if ask(cap, &s[..2 + left], &mut r) == 3 && r[0] == 0 {
                sent += u16::from_be_bytes([r[1], r[2]]) as usize;
            }
        }
        // Held until data comes, the connection's state changes, or STALL_S with nothing; at once while the request is still going out.
        hold[2] = if sent < total { 0 } else { STALL_S };
        let n = ask(cap, &hold, &mut out);
        if n == 0 || n == ERR {
            return Err(Fail::Broken("the connection was lost"));
        }
        // WHY: the length is ip's, at most what the buffer took.
        let n = (n as usize).min(out.len());
        let state = out[0];
        if n > 1 {
            resp.feed(&out[1..n], body).map_err(|e| if resp.status != 0 && resp.status != 200 { Fail::Missing(resp.status) } else { Fail::Broken(e) })?;
            if !headed && resp.body.is_some() {
                headed = true;
                head(&resp).map_err(Fail::Broken)?;
            }
            if resp.done {
                return Ok(());
            }
            continue;
        }
        match state {
            // The peer closed and nothing is left: whole, or not.
            3 => return resp.closed().map_err(Fail::Broken),
            4 => return Err(Fail::Broken("the connection failed")),
            5 => return Err(Fail::Broken("the connection is gone")),
            _ => {}
        }
        if state != was {
            was = state;
            continue;
        }
        // Nothing came for the whole wait: the server stopped sending. With the request still going out (ip's send buffer full), a moment before more of it.
        if hold[2] != 0 {
            return Err(Fail::Broken("the server stopped sending"));
        }
        pause();
    }
}
