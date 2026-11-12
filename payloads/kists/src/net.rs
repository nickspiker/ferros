//! A plain HTTP GET through the `ip` process (payloads/ip, its `udp` endpoint): the host's IPv4 address from ip's resolver (op 10 RESOLVE), a client TCP connection, the request, and the response decoded as it arrives (ferros_kist::fetch::Response), its body handed on a piece at a time and never held whole.
//!
//! Every wait is a receive ip holds until something comes (the wait byte of its ops 2 and 6), never a poll (owner, 2026-11-10: "No polling!"): the worker blocks in the call. The deadline is a real one: the stall below (DNS's are ip's own, op 10).
//! Two waits remain, each with its reason: a request bigger than ip's send buffer takes, given more of it a millisecond on (nothing rings when the buffer drains); a connection refused for want of a free one (ip holds sixteen) is asked again each second, a few times.

use ferros_kist::fetch::{split_url, Response};
use ferros_sys::{call, freq, now, sleep_until, ERR};

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

/// A millisecond's wait before giving ip's full send buffer more of the request (the module's WHY).
fn pause() {
    sleep_until(now() + (freq() >> 10));
}

/// How long a lookup's call is held at most: 16 s (ip asks four times a second apart; the stower asks for IPv4 alone).
const RESOLVE_WAIT_S: u8 = 1 << 4;

/// `host`'s IPv4 address from ip's resolver (op 10 RESOLVE: the device's one cache, so a restore of several capsules asks the server once); None when it has none (nobody present, no route or server, no such name, no answer).
fn address(cap: u64, host: &str) -> Option<[u8; 4]> {
    // WHY: a DNS name is at most 253 bytes (ip refuses a longer one too); the request holds this many.
    if host.len() > 253 {
        return None;
    }
    let mut q = [0u8; 3 + 256];
    q[0] = 10;
    q[1] = 4;
    q[2] = RESOLVE_WAIT_S;
    q[3..3 + host.len()].copy_from_slice(host.as_bytes());
    let mut r = [0u8; 22];
    let n = ask(cap, &q[..3 + host.len()], &mut r);
    // Status 0, found: the family and the address follow (an empty reply: nobody present).
    if n == ERR || n < 22 || r[0] != 0 || r[1] != 4 {
        return None;
    }
    Some(r[2..6].try_into().unwrap())
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
