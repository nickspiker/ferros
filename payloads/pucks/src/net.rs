//! A plain HTTP POST of one small VSF document to fgtw.org through the `ip` process (its `udp` endpoint), the answer decoded as it arrives (ferros_kist::fetch, as the stower's GET in payloads/kists net.rs, whose waits this keeps).
//!
//! Every wait is a receive ip holds until something comes, never a poll; the one deadline is the stall below, a real one: ip fails a connection only when what it sent goes unanswered, and a server that falls silent would hold the puck forever.

use alloc::vec::Vec;
use ferros_kist::fetch::Response;
use ferros_sys::{call, freq, now, sleep_until, ERR};

/// How long a connection may bring nothing before it is given up.
const STALL_S: u8 = 1 << 4;
/// The most one receive brings; the weather's answer is a few KiB.
const RECV_MOST: usize = 1 << 13;
/// The resolver's hold.
const RESOLVE_WAIT_S: u8 = 1 << 4;

fn address(cap: u64, host: &str) -> Option<[u8; 4]> {
    let mut q = [0u8; 3 + 64];
    q[0] = 10;
    q[1] = 4;
    q[2] = RESOLVE_WAIT_S;
    q[3..3 + host.len()].copy_from_slice(host.as_bytes());
    let mut r = [0u8; 22];
    let n = call(cap, &q[..3 + host.len()], &mut r);
    if n == ERR || n < 22 || r[0] != 0 || r[1] != 4 {
        return None;
    }
    Some(r[2..6].try_into().unwrap())
}

fn connect(cap: u64, ip: [u8; 4], port: u16) -> Option<u8> {
    let mut c = [0u8; 20];
    c[0] = 4;
    c[1] = 4;
    c[2..6].copy_from_slice(&ip);
    c[18..20].copy_from_slice(&port.to_be_bytes());
    // ip holds sixteen connections; a full table frees as others finish (the stower's reason for its few tries a second apart).
    for _ in 0..1 << 3 {
        let mut r = [0u8; 16];
        let n = call(cap, &c, &mut r);
        if n == 2 && r[0] == 0 {
            return Some(r[1]);
        }
        if n != 1 || r[0] != 4 {
            return None;
        }
        sleep_until(now() + freq());
    }
    None
}

/// POST `body` to `http://fgtw.org/` and return the whole answer's body, or None (no network, a status other than 200, a broken or silent transfer).
pub fn post(cap: u64, body: &[u8]) -> Option<Vec<u8>> {
    let host = "fgtw.org";
    let ip = address(cap, host)?;
    let h = connect(cap, ip, 80)?;
    let r = transfer(cap, h, host, body);
    let mut x = [0u8; 4];
    let _ = call(cap, &[7, h], &mut x);
    r
}

fn transfer(cap: u64, h: u8, host: &str, body: &[u8]) -> Option<Vec<u8>> {
    // The request: the head, then the document. Its length in decimal is HTTP's own (the legacy edge every device call to fgtw.org crosses), never kept.
    let mut req: Vec<u8> = Vec::new();
    req.extend_from_slice(&[5, h]);
    req.extend_from_slice(b"POST / HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(b"\r\nUser-Agent: ferros\r\nContent-Type: application/octet-stream\r\nContent-Length: ");
    let mut digits = [0u8; 20];
    let mut k = digits.len();
    let mut v = body.len();
    loop {
        k -= 1;
        digits[k] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    req.extend_from_slice(&digits[k..]);
    req.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    req.extend_from_slice(body);
    let total = req.len() - 2;
    let mut sent = 0;
    let mut resp = Response::new();
    let mut out_body: Vec<u8> = Vec::new();
    let mut buf = alloc::vec![0u8; 1 + RECV_MOST];
    let mut hold = [6, h, STALL_S, 0, 0, 0, 0];
    hold[3..7].copy_from_slice(&(RECV_MOST as u32).to_be_bytes());
    let mut was = 1u8;
    loop {
        if sent < total {
            let mut s = alloc::vec![5u8, h];
            s.extend_from_slice(&req[2 + sent..2 + total]);
            let mut r = [0u8; 4];
            if call(cap, &s, &mut r) == 3 && r[0] == 0 {
                sent += u16::from_be_bytes([r[1], r[2]]) as usize;
            }
        }
        hold[2] = if sent < total { 0 } else { STALL_S };
        let n = call(cap, &hold, &mut buf);
        if n == 0 || n == ERR {
            return None;
        }
        // WHY: the length is ip's, at most what the buffer took.
        let n = (n as usize).min(buf.len());
        let state = buf[0];
        if n > 1 {
            let mut take = |piece: &[u8]| -> Result<(), &'static str> {
                // WHY the bound: an answer is a few KiB; one past 2^16 is no weather of fgtw.org's, and is not held.
                if out_body.len() + piece.len() > 1 << 16 {
                    return Err("an answer too long");
                }
                out_body.extend_from_slice(piece);
                Ok(())
            };
            resp.feed(&buf[1..n], &mut take).ok()?;
            if resp.body.is_some() && resp.status != 200 {
                return None;
            }
            if resp.done {
                return Some(out_body);
            }
            continue;
        }
        match state {
            3 => return resp.closed().ok().map(|_| out_body),
            4 | 5 => return None,
            _ => {}
        }
        if state != was {
            was = state;
            continue;
        }
        if hold[2] != 0 {
            return None;
        }
        // The request still going out (ip's send buffer full): a millisecond on, as the stower does.
        sleep_until(now() + (freq() >> 10));
    }
}
