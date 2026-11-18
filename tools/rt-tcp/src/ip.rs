//! A fake ip server (payloads/ip's `udp` endpoint) for the client's ops: 4 CONNECT, 5 SEND, 6 RECV (both forms), 7 CLOSE, 10 RESOLVE, 11 WATCH (no ports), 14 ROOM, served as ip's header says, a held call blocking its caller's thread until ip would answer it.
//! The test plays the peer and the person (`open`, `fail`, `push`, `peer_close`, `drain`, `set_person`), and reads what the client did (`sent`, `calls`, `closed`).
//! One test at a time owns it (`take`), from a fresh state.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};

use crate::sys::now;

pub const TCPN: usize = 1 << 4;

#[derive(Clone, Default)]
pub struct Conn {
    /// 0 free, 1 connecting, 2 open, 3 the peer closed, 4 failed.
    pub state: u8,
    /// Why it failed: 1 reset, 2 unanswered.
    pub why: u8,
    pub family: u8,
    pub port: u16,
    pub rx: VecDeque<u8>,
    /// The send buffer: bytes taken and not yet drained by the peer, and every byte ever taken, in order.
    pub tx_len: u32,
    pub sent: Vec<u8>,
    pub fin_queued: bool,
    /// Holds made on it, a receive's and a wait for room's: a new one answers the one before (one reader and one writer a connection).
    rd_gen: u64,
    wr_gen: u64,
}

pub struct State {
    pub person: bool,
    pub conns: Vec<Conn>,
    /// The send buffer's size (ip's `TXB`), small so a test fills it.
    pub txb: u32,
    /// False: an ip server from before op 14 (answers [3]).
    pub room_op: bool,
    /// RESOLVE's answer: status, family, address.
    pub resolve: (u8, u8, [u8; 16]),
    /// Calls by op (index = op).
    pub calls: [u32; 16],
    /// Changes since the start (any of the peer's or the person's): a held call waits for this to move.
    changes: u64,
}

impl State {
    fn fresh() -> State {
        State { person: true, conns: vec![Conn::default(); TCPN], txb: 1 << 12, room_op: true, resolve: (0, 4, [0; 16]), calls: [0; 16], changes: 0 }
    }

    fn flags(&self) -> u8 {
        1 | (self.person as u8) << 3
    }

    fn state_byte(&self, h: usize, long: bool) -> u8 {
        let c = &self.conns[h];
        match c.state {
            4 if long => 4 | c.why << 4,
            s => s,
        }
    }
}

struct Fake {
    state: Mutex<Option<State>>,
    changed: Condvar,
}

static FAKE: Fake = Fake { state: Mutex::new(None), changed: Condvar::new() };
/// One test at a time.
static TEST: Mutex<()> = Mutex::new(());

/// The fake for one test, from a fresh state; held for the test's life.
pub fn take() -> MutexGuard<'static, ()> {
    let g = TEST.lock().unwrap_or_else(|e| e.into_inner());
    *FAKE.state.lock().unwrap() = Some(State::fresh());
    g
}

/// Look at or change the state; a change wakes every held call to look again.
pub fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = FAKE.state.lock().unwrap();
    let s = g.as_mut().unwrap();
    let r = f(s);
    s.changes += 1;
    FAKE.changed.notify_all();
    r
}

pub fn open(h: usize) {
    with(|s| s.conns[h].state = 2);
}

pub fn fail(h: usize, why: u8) {
    with(|s| {
        s.conns[h].state = 4;
        s.conns[h].why = why;
    });
}

pub fn push(h: usize, data: &[u8]) {
    with(|s| s.conns[h].rx.extend(data));
}

pub fn peer_close(h: usize) {
    with(|s| s.conns[h].state = 3);
}

/// The peer ACKs `n` bytes: they leave the send buffer.
pub fn drain(h: usize, n: u32) {
    with(|s| {
        let c = &mut s.conns[h];
        c.tx_len -= n.min(c.tx_len);
    });
}

pub fn set_person(p: bool) {
    with(|s| s.person = p);
}

pub fn calls(op: usize) -> u32 {
    with(|s| s.calls[op])
}

pub fn conn(h: usize) -> Conn {
    with(|s| s.conns[h].clone())
}

/// Hold the caller until `ready` says so or the counter passes `until` (u64::MAX never), as ip holds a call; then `answer`.
fn hold(mut g: MutexGuard<'static, Option<State>>, until: u64, ready: impl Fn(&State) -> bool, answer: impl FnOnce(&mut State, &mut [u8]) -> usize, out: &mut [u8]) -> usize {
    loop {
        let s = g.as_ref().unwrap();
        if ready(s) || now() > until {
            break;
        }
        g = if until == u64::MAX {
            FAKE.changed.wait(g).unwrap()
        } else {
            let d = std::time::Duration::from_nanos(until.saturating_sub(now()) + 1);
            FAKE.changed.wait_timeout(g, d).unwrap().0
        };
    }
    answer(g.as_mut().unwrap(), out)
}

fn take_rx(s: &mut State, h: usize, most: usize, long: bool, out: &mut [u8]) -> usize {
    out[0] = s.state_byte(h, long);
    let c = &mut s.conns[h];
    let n = c.rx.len().min(most).min(out.len() - 1);
    for k in 0..n {
        out[1 + k] = c.rx.pop_front().unwrap();
    }
    1 + n
}

fn room(s: &State, h: usize, out: &mut [u8]) -> usize {
    out[0] = s.state_byte(h, true);
    out[1..5].copy_from_slice(&(s.txb - s.conns[h].tx_len).to_be_bytes());
    5
}

fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes(b.try_into().unwrap())
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b.try_into().unwrap())
}

/// One request; the reply's length in `out`.
pub fn serve(m: &[u8], out: &mut [u8]) -> usize {
    let mut g = FAKE.state.lock().unwrap();
    let s = g.as_mut().unwrap();
    let op = m[0] as usize;
    s.calls[op & 15] += 1;
    if !s.person && !matches!(op, 3 | 11 | 12 | 13) {
        return 0;
    }
    match op {
        4 => {
            let Some(h) = s.conns.iter().position(|c| c.state == 0) else {
                out[0] = 4;
                return 1;
            };
            s.conns[h] = Conn { state: 1, family: m[1], port: u16::from_be_bytes([m[18], m[19]]), ..Conn::default() };
            out[0] = 0;
            out[1] = h as u8;
            2
        }
        5 => {
            let h = m[1] as usize;
            let c = &mut s.conns[h];
            if c.state == 0 || c.state == 4 || c.fin_queued {
                out[0] = 5;
                return 1;
            }
            let n = (m.len() - 2).min((s.txb - c.tx_len) as usize);
            c.tx_len += n as u32;
            c.sent.extend_from_slice(&m[2..2 + n]);
            out[0] = 0;
            out[1..3].copy_from_slice(&(n as u16).to_be_bytes());
            s.changes += 1;
            FAKE.changed.notify_all();
            3
        }
        6 => {
            let h = m[1] as usize;
            if s.conns[h].state == 0 {
                out[0] = 5;
                return 1;
            }
            let most = be32(&m[3..7]) as usize;
            let long = m.len() >= 15;
            let until = if long {
                match be64(&m[7..15]) {
                    0 => None,
                    d => Some(d),
                }
            } else {
                (m[2] > 0).then(|| now() + m[2] as u64 * 1_000_000_000)
            };
            let c = &s.conns[h];
            let Some(until) = until.filter(|_| c.rx.is_empty() && (c.state == 1 || c.state == 2)) else {
                return take_rx(s, h, most, long, out);
            };
            s.conns[h].rd_gen += 1;
            let (mine, held) = (s.conns[h].rd_gen, s.conns[h].state);
            hold(
                g,
                until,
                |s| {
                    let c = &s.conns[h];
                    !s.person || c.rd_gen != mine || c.state != held || c.state >= 3 || !c.rx.is_empty()
                },
                |s, out| {
                    if !s.person {
                        0
                    } else if s.conns[h].state == 0 {
                        out[0] = 5;
                        1
                    } else {
                        take_rx(s, h, most, long, out)
                    }
                },
                out,
            )
        }
        7 => {
            let h = m[1] as usize;
            s.conns[h].fin_queued = true;
            s.changes += 1;
            FAKE.changed.notify_all();
            out[0] = 0;
            1
        }
        10 => {
            let (status, fam, a) = s.resolve;
            out[0] = status;
            out[1] = fam;
            out[2..18].copy_from_slice(&a);
            out[18..22].copy_from_slice(&64u32.to_be_bytes());
            22
        }
        11 => {
            let seen = m[2];
            let until = match be64(&m[11..19]) {
                0 => u64::MAX,
                d => d,
            };
            hold(
                g,
                until,
                |s| s.flags() != seen,
                |s, out| {
                    out[0] = s.flags();
                    out[1..6].fill(0);
                    6
                },
                out,
            )
        }
        14 if s.room_op => {
            let h = m[1] as usize;
            if s.conns[h].state == 0 {
                out[0] = 5;
                return 1;
            }
            let want = be32(&m[2..6]).min(s.txb);
            let until = be64(&m[6..14]);
            let c = &s.conns[h];
            if until == 0 || s.txb - c.tx_len >= want || !(1..=3).contains(&c.state) || c.fin_queued {
                return room(s, h, out);
            }
            s.conns[h].wr_gen += 1;
            let (mine, held) = (s.conns[h].wr_gen, s.conns[h].state);
            hold(
                g,
                until,
                |s| {
                    let c = &s.conns[h];
                    !s.person || c.wr_gen != mine || c.state != held || c.fin_queued || s.txb - c.tx_len >= want
                },
                |s, out| {
                    if !s.person {
                        0
                    } else if s.conns[h].state == 0 {
                        out[0] = 5;
                        1
                    } else {
                        room(s, h, out)
                    }
                },
                out,
            )
        }
        _ => {
            out[0] = 3;
            1
        }
    }
}
