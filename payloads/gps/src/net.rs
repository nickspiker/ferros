//! A plain HTTP GET through the `ip` process (its `udp` endpoint, grant slot 9), the answer decoded as it arrives (ferros_kist::fetch), as the GPS puck's sibling the weather puck POSTs (payloads/pucks net.rs, whose calls and waits this keeps).
//! Used once a day at most, for the orbits (CelesTrak's GNSS elements over plain HTTP: ferros speaks no TLS yet).
//!
//! Every wait is a receive ip holds until something comes, never a poll; the one deadline is the stall below, a real one: ip fails a connection only when what it sent goes unanswered, and a server that falls silent would hold the driver forever.

use alloc::vec::Vec;
use ferros_kist::fetch::Response;
use ferros_sys::{call, freq, now, sleep_until, ERR};

/// How long a connection may bring nothing before it is given up.
const STALL_S: u8 = 1 << 4;
/// The most one receive brings.
const RECV_MOST: usize = 1 << 13;
/// The resolver's hold.
const RESOLVE_WAIT_S: u8 = 1 << 4;
/// The most of an answer kept: the GNSS elements are some 28 KB (2026-11-18), a few times that is no answer of theirs.
const BODY_MOST: usize = 1 << 17;

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

fn connect(cap: u64, ip: [u8; 4], port: u16) -> Option<u16> {
    let mut c = [0u8; 20];
    c[0] = 4;
    c[1] = 4;
    c[2..6].copy_from_slice(&ip);
    c[18..20].copy_from_slice(&port.to_be_bytes());
    // ip refuses a connection only for want of memory for its block, which frees as others finish (the stower's reason for its few tries a second apart).
    for _ in 0..1 << 3 {
        let mut r = [0u8; 16];
        let n = call(cap, &c, &mut r);
        if n == 3 && r[0] == 0 {
            return Some(u16::from_be_bytes([r[1], r[2]]));
        }
        if n != 1 || r[0] != 6 {
            return None;
        }
        sleep_until(now() + freq());
    }
    None
}

/// GET `path` from `host` port 80 and return the whole answer's body, or why not (no network, a status other than 200, a broken or silent transfer).
pub fn get(cap: u64, host: &str, path: &str) -> Result<Vec<u8>, &'static str> {
    let ip = address(cap, host).ok_or("the name did not resolve (no network?)")?;
    let h = connect(cap, ip, 80).ok_or("no connection")?;
    let r = transfer(cap, h, host, path);
    let mut x = [0u8; 4];
    let _ = call(cap, &[7, (h >> 8) as u8, h as u8], &mut x);
    r
}

fn transfer(cap: u64, h: u16, host: &str, path: &str) -> Result<Vec<u8>, &'static str> {
    let mut req: Vec<u8> = Vec::new();
    req.extend_from_slice(&[5, (h >> 8) as u8, h as u8]);
    req.extend_from_slice(b"GET ");
    req.extend_from_slice(path.as_bytes());
    req.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(b"\r\nUser-Agent: ferros\r\nConnection: close\r\n\r\n");
    let total = req.len() - 3;
    let mut sent = 0;
    let mut resp = Response::new();
    let mut out_body: Vec<u8> = Vec::new();
    let mut buf = alloc::vec![0u8; 1 + RECV_MOST];
    let mut hold = [6, (h >> 8) as u8, h as u8, STALL_S, 0, 0, 0, 0];
    hold[4..8].copy_from_slice(&(RECV_MOST as u32).to_be_bytes());
    let mut was = 1u8;
    loop {
        if sent < total {
            let mut s = alloc::vec![5u8, (h >> 8) as u8, h as u8];
            s.extend_from_slice(&req[3 + sent..3 + total]);
            let mut r = [0u8; 4];
            if call(cap, &s, &mut r) == 3 && r[0] == 0 {
                sent += u16::from_be_bytes([r[1], r[2]]) as usize;
            }
        }
        hold[3] = if sent < total { 0 } else { STALL_S };
        let n = call(cap, &hold, &mut buf);
        if n == 0 || n == ERR {
            return Err("the connection failed");
        }
        // WHY: the length is ip's, at most what the buffer took.
        let n = (n as usize).min(buf.len());
        let state = buf[0];
        if n > 1 {
            let mut take = |piece: &[u8]| -> Result<(), &'static str> {
                // WHY the bound: the answer is held whole in the driver's heap; past BODY_MOST it is no orbits file.
                if out_body.len() + piece.len() > BODY_MOST {
                    return Err("an answer too long");
                }
                out_body.extend_from_slice(piece);
                Ok(())
            };
            resp.feed(&buf[1..n], &mut take)?;
            if resp.body.is_some() && resp.status != 200 {
                return Err("the server answered other than 200");
            }
            if resp.done {
                return Ok(out_body);
            }
            continue;
        }
        match state {
            3 => return resp.closed().map(|_| out_body),
            4 | 5 => return Err("the connection was refused or reset"),
            _ => {}
        }
        if state != was {
            was = state;
            continue;
        }
        if hold[3] != 0 {
            return Err("the server fell silent");
        }
        // The request still going out (ip's send buffer full): a millisecond on, as the stower does.
        sleep_until(now() + (freq() >> 10));
    }
}
