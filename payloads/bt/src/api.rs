//! The client ops on the `bt` endpoint (docs/BT.md "The client ops"): sessions, scanning, connections, credit-based channels, and the held call that delivers what happens.
//!
//! A client opens a session and gets its id (eight random bytes: another holder of the endpoint cannot guess it, so cannot act in it); every op after names it.
//! What happens arrives as events on a held call (op 7 EVENTS), answered when there is something to say: never a poll.
//! A session's end, any way (op 15 CLOSE, its process ending), takes everything of it down at once: its links ended, its scan stopped, its pending connection cancelled.

use crate::link::{ChanState, Conn, Pending, Send, WHY_ASKED};
use crate::{Bt, Todo};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Event kinds, the first byte of each event: [kind u8][length u16][body].
pub const EV_ADV: u8 = 0;
pub const EV_CONNECTED: u8 = 1;
pub const EV_DISCONNECTED: u8 = 2;
pub const EV_COC_OPENED: u8 = 3;
pub const EV_SDU: u8 = 4;
pub const EV_COC_CLOSED: u8 = 5;
pub const EV_UPDATED: u8 = 6;
pub const EV_LENGTH: u8 = 7;
pub const EV_PHY: u8 = 8;

/// The ops (the first byte of a request); 0 to 5 are the driver's own (BT.md "The `bt` endpoint").
pub const OP_OPEN: u8 = 6;
pub const OP_EVENTS: u8 = 7;
pub const OP_SCAN: u8 = 8;
pub const OP_SCAN_STOP: u8 = 9;
pub const OP_CONNECT: u8 = 10;
pub const OP_DISCONNECT: u8 = 11;
pub const OP_COC_OPEN: u8 = 12;
pub const OP_SEND: u8 = 13;
pub const OP_COC_CLOSE: u8 = 14;
pub const OP_CLOSE: u8 = 15;

/// A connection's parameters when the client names none: an interval of 30 to 50 ms, no latency, a 5.12 s supervision timeout (BlueZ's and Android's balanced defaults are 24 to 40 and 0; the timeout is the next power of two over Android's 500).
const DEFAULT_PARAMS: [u16; 4] = [24, 40, 0, 1 << 9];
/// The MPS a channel receives with when the client names none: one LE data PDU at the longest data length (251 bytes) less the L2CAP header.
const DEFAULT_MPS: u16 = 247;

/// One event for a client, as it goes out, with what its delivery means to the driver.
pub struct Ev {
    pub bytes: Vec<u8>,
    /// An SDU's credits (link, channel, K-frames): handed back to the peer once the client has taken it.
    pub credit: Option<(u16, u16, u32)>,
    /// An advertising report's advertiser (event type, address type, address): a newer report from it replaces one the client has not taken.
    pub key: Option<[u8; 8]>,
}

impl Ev {
    pub fn new(kind: u8, body: &[u8]) -> Ev {
        let mut bytes = Vec::with_capacity(3 + body.len());
        bytes.push(kind);
        bytes.extend_from_slice(&(body.len() as u16).to_le_bytes());
        bytes.extend_from_slice(body);
        Ev { bytes, credit: None, key: None }
    }
}

pub struct Session {
    pub id: u64,
    /// EVENTS calls held, oldest first, each with the room its caller gave.
    pub held: VecDeque<([u8; 32], usize)>,
    pub events: VecDeque<Ev>,
    /// Scanning, and whether actively (scan requests on the air).
    pub scan: Option<bool>,
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

impl Bt {
    pub fn session_at(&self, id: u64) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    /// An event for session `id` (none: a link whose session ended, nothing to tell); a newer advertising report from the same advertiser replaces one not yet taken, so a client slow to read a busy room holds one report per advertiser, never a growing list.
    pub fn event(&mut self, id: u64, e: Ev) {
        let Some(si) = self.session_at(id) else { return };
        let s = &mut self.sessions[si];
        if let Some(key) = e.key {
            if let Some(old) = s.events.iter_mut().find(|o| o.key == Some(key)) {
                *old = e;
                return self.answer(si);
            }
        }
        s.events.push_back(e);
        self.answer(si);
    }

    /// Session `si`'s held EVENTS calls answered while it has events: each reply as many whole events as fit the caller's room (one at least, whatever its size). Credits for the SDUs taken go back to their peers; a reply that fails means the caller ended: its session is taken down.
    pub fn answer(&mut self, si: usize) {
        loop {
            let s = &mut self.sessions[si];
            if s.held.is_empty() || s.events.is_empty() {
                return;
            }
            let (token, room) = s.held.pop_front().unwrap();
            let mut out = Vec::new();
            let mut credits: Vec<(u16, u16, u32)> = Vec::new();
            while let Some(e) = s.events.front() {
                if !out.is_empty() && out.len() + e.bytes.len() > room {
                    break;
                }
                let e = s.events.pop_front().unwrap();
                out.extend_from_slice(&e.bytes);
                if let Some(c) = e.credit {
                    credits.push(c);
                }
            }
            let id = s.id;
            if ferros_sys::reply(&token, &out) == ferros_sys::ERR {
                self.todo.push_back(Todo::End(id));
                return;
            }
            for (handle, cid, frames) in credits {
                self.give_back(handle, cid, frames);
            }
        }
    }

    /// Everything of session `id` taken down at once (OP_CLOSE, or its process ended): its links ended (their channels' held sends refused, nothing more told), its pending connection cancelled, its scan stopped, its held calls answered empty.
    pub fn end_session(&mut self, id: u64) {
        let Some(si) = self.session_at(id) else { return };
        let s = self.sessions.remove(si);
        for (t, _) in s.held {
            ferros_sys::reply(&t, &[]);
        }
        for ci in 0..self.conns.len() {
            if self.conns[ci].session != id {
                continue;
            }
            self.conns[ci].session = 0;
            for ch in core::mem::take(&mut self.conns[ci].chans) {
                for sd in ch.sends {
                    ferros_sys::reply(&sd.token, &[1]);
                }
            }
            if !self.conns[ci].ending {
                self.conns[ci].ending = true;
                self.todo.push_back(Todo::Disconnect(self.conns[ci].handle, 0x13));
            }
        }
        if let Some(p) = &mut self.connecting {
            if p.session == id {
                p.session = 0;
                self.todo.push_back(Todo::Cancel);
            }
        }
        if s.scan.is_some() {
            self.todo.push_back(Todo::Scan);
        }
        if self.watched == id {
            self.watched = 0;
        }
        say!(self.grants, "bt: session ended; A#{} left\n", self.sessions.len());
    }

    /// A client op (6 to 15). The caller holds a send right on `bt`: a system process granted it, or a kist granted the Bluetooth right (KISTS.md). Nothing goes on the air before a person has attested.
    pub fn op(&mut self, m: &[u8], token: &[u8; 32]) {
        let op = m[0];
        if op == OP_OPEN {
            if ferros_sys::facts().0[83] != 2 {
                ferros_sys::reply(token, &[1]);
                return;
            }
            let mut b = [0u8; 8];
            ferros_sys::entropy(ferros_sys::cap(self.grants, crate::S_ENTROPY), &mut b);
            let id = u64::from_le_bytes(b) | 1;
            self.sessions.push(Session { id, held: VecDeque::new(), events: VecDeque::new(), scan: None });
            // Its process's end watched from now: the kernel rings the loop's word (`RING` bit 63) when it ends, any way, and the session goes at once. The kernel keeps one watch a watcher, so the newest session is the one watched; an older one is found gone when a reply to it fails or at the 8 s look (`sweep`).
            if let ferros_sys::Watch::Watched(_) = ferros_sys::caller_watch(token, &crate::RING) {
                self.watched = id;
            }
            let mut r = [0u8; 9];
            r[1..].copy_from_slice(&id.to_le_bytes());
            ferros_sys::reply(token, &r);
            say!(self.grants, "bt: session opened; A#{} open\n", self.sessions.len());
            return;
        }
        // Every other op names its session: [op][session u64].
        let Some(si) = m.get(1..9).and_then(|b| self.session_at(u64::from_le_bytes(b.try_into().unwrap()))) else {
            ferros_sys::reply(token, &[]);
            return;
        };
        let id = self.sessions[si].id;
        let a = &m[9..];
        let r: u8 = match op {
            OP_EVENTS if a.len() >= 4 => {
                let room = u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as usize;
                self.sessions[si].held.push_back((*token, room));
                self.answer(si);
                return;
            }
            OP_SCAN if !a.is_empty() => {
                self.sessions[si].scan = Some(a[0] != 0);
                !self.scan_update() as u8
            }
            OP_SCAN_STOP => {
                self.sessions[si].scan = None;
                !self.scan_update() as u8
            }
            OP_CONNECT if a.len() >= 7 => {
                let mut addr = [0u8; 6];
                addr.copy_from_slice(&a[1..7]);
                let asked = a.len() >= 15;
                let p = if asked { [le16(a, 7), le16(a, 9), le16(a, 11), le16(a, 13)] } else { DEFAULT_PARAMS };
                self.connect(id, a[0], addr, p, asked)
            }
            OP_DISCONNECT if a.len() >= 2 => {
                let handle = le16(a, 0);
                if handle == 0xFFFF {
                    match &mut self.connecting {
                        Some(p) if p.session == id => {
                            self.todo.push_back(Todo::Cancel);
                            0
                        }
                        _ => 1,
                    }
                } else {
                    match self.conn_at(handle) {
                        Some(ci) if self.conns[ci].session == id && !self.conns[ci].ending => {
                            self.chans_gone(ci, WHY_ASKED);
                            self.conns[ci].ending = true;
                            self.todo.push_back(Todo::Disconnect(handle, 0x13));
                            0
                        }
                        _ => 1,
                    }
                }
            }
            OP_COC_OPEN if a.len() >= 6 => {
                let (handle, psm, mtu) = (le16(a, 0), le16(a, 2), le16(a, 4));
                let mps = if a.len() >= 8 { le16(a, 6) } else { DEFAULT_MPS };
                // The spec's least MTU and MPS on LE (Core Vol 3 Part A 4.22: 23), and its largest MPS (65533): the client's numbers, refused outside them rather than sent for the peer to refuse.
                match self.conn_at(handle) {
                    Some(ci) if self.conns[ci].session == id && !self.conns[ci].ending && mtu >= 23 && (23..=65533).contains(&mps) => match self.coc_open(ci, psm, mtu, mps) {
                        Some(cid) => {
                            let mut r = [0u8; 3];
                            r[1..].copy_from_slice(&cid.to_le_bytes());
                            ferros_sys::reply(token, &r);
                            return;
                        }
                        None => 1,
                    },
                    _ => 1,
                }
            }
            OP_SEND if a.len() >= 4 => {
                let (handle, cid) = (le16(a, 0), le16(a, 2));
                let sdu = &a[4..];
                let ch = self.conn_at(handle).filter(|&ci| self.conns[ci].session == id && !self.conns[ci].ending).and_then(|ci| self.conns[ci].chans.iter_mut().find(|c| c.cid == cid && c.state == ChanState::Open));
                match ch {
                    // An SDU past the peer's MTU is refused whole, never cut.
                    Some(ch) if sdu.len() <= ch.peer_mtu as usize => {
                        ch.sends.push_back(Send { token: *token, sdu: sdu.to_vec(), at: 0, started: false });
                        self.flush_out();
                        return;
                    }
                    _ => 1,
                }
            }
            OP_COC_CLOSE if a.len() >= 4 => {
                let (handle, cid) = (le16(a, 0), le16(a, 2));
                match self.conn_at(handle).filter(|&ci| self.conns[ci].session == id) {
                    Some(ci) => match self.conns[ci].chans.iter().position(|c| c.cid == cid && matches!(c.state, ChanState::Open | ChanState::Opening(_))) {
                        Some(k) => {
                            self.chan_close(ci, k, WHY_ASKED);
                            0
                        }
                        None => 1,
                    },
                    None => 1,
                }
            }
            OP_CLOSE => {
                self.end_session(id);
                0
            }
            _ => 1,
        };
        ferros_sys::reply(token, &[r]);
    }

    /// LE Create Connection to `peer` (address type 0 public, 1 random, the address as HCI carries it, least significant byte first) from our own random address, with connection interval min and max, latency and supervision timeout `p` (1.25 ms, events, 10 ms units). One at a time in the controller: 2 while another is pending. Its end is an `EV_CONNECTED` event. 0 asked, 1 refused (the controller's status on the console).
    fn connect(&mut self, session: u64, kind: u8, addr: [u8; 6], p: [u16; 4], asked: bool) -> u8 {
        if self.connecting.is_some() {
            return 2;
        }
        self.own_static();
        let mut c = [0u8; 25];
        let scan = crate::SCAN.to_le_bytes();
        c[0..2].copy_from_slice(&scan);
        c[2..4].copy_from_slice(&scan);
        c[4] = 0;
        c[5] = kind;
        c[6..12].copy_from_slice(&addr);
        c[12] = 1;
        for (k, v) in p.iter().enumerate() {
            c[13 + 2 * k..15 + 2 * k].copy_from_slice(&v.to_le_bytes());
        }
        let mut e = [0u8; 258];
        let mut st = self.cmd(0x200D, &c, 256, &mut e).map(|_| e[0]);
        // Some controllers refuse to initiate while scanning (Command Disallowed): the scan paused for the attempt, resumed once it ends.
        if st == Some(0x0C) && self.scan_on {
            self.scan_set(false, false);
            self.scan_paused = true;
            st = self.cmd(0x200D, &c, 256, &mut e).map(|_| e[0]);
        }
        if st != Some(0) {
            match st {
                Some(c) => say!(self.grants, "bt: connect refused: status G#{c:02X}\n"),
                None => say!(self.grants, "bt: connect refused: no answer\n"),
            }
            if self.scan_paused {
                self.scan_paused = false;
                self.scan_update();
            }
            return 1;
        }
        self.connecting = Some(Pending { session, peer: (kind, addr), asked });
        say!(self.grants, "bt: connecting: interval A#{}-A#{} latency A#{} timeout A#{}\n", p[0], p[1], p[2], p[3]);
        0
    }

    /// LE Connection Complete (LE Meta 1): [status][handle u16][role][peer type][peer address 6][interval u16][latency u16][timeout u16][clock accuracy]. The pending connection's session told either way; a link made for a session that has ended since is ended again at once.
    pub fn connected(&mut self, d: &[u8]) {
        if d.len() < 18 {
            return;
        }
        let (status, handle) = (d[0], le16(d, 1) & 0x0FFF);
        let p = self.connecting.take();
        if self.scan_paused {
            self.scan_paused = false;
            self.todo.push_back(Todo::Scan);
        }
        let Some(p) = p else {
            if status == 0 {
                self.todo.push_back(Todo::Disconnect(handle, 0x13));
            }
            return;
        };
        let mut body = [0u8; 16];
        body[0] = status;
        body[1..3].copy_from_slice(&handle.to_le_bytes());
        body[3..10].copy_from_slice(&d[4..11]);
        body[10..16].copy_from_slice(&d[11..17]);
        say!(self.grants, "bt: connected: status G#{status:02X}, link G#{handle:X}, interval A#{} latency A#{} timeout A#{}\n", le16(d, 11), le16(d, 13), le16(d, 15));
        if status != 0 {
            self.event(p.session, Ev::new(EV_CONNECTED, &body));
            return;
        }
        let session = if self.session_at(p.session).is_some() { p.session } else { 0 };
        self.conns.push(Conn { handle, session, peer: p.peer, asked: p.asked, reasm: Vec::new(), out: VecDeque::new(), out_at: 0, in_flight: 0, chans: Vec::new(), turn: 0, ending: session == 0, fault: None });
        if session == 0 {
            self.todo.push_back(Todo::Disconnect(handle, 0x13));
            return;
        }
        self.event(session, Ev::new(EV_CONNECTED, &body));
        self.todo.push_back(Todo::Phy(handle));
    }

    /// LE Connection Update Complete (LE Meta 3): [status][handle u16][interval u16][latency u16][timeout u16], told to the link's client.
    pub fn updated(&mut self, d: &[u8]) {
        if d.len() < 9 {
            return;
        }
        let handle = le16(d, 1) & 0x0FFF;
        let Some(ci) = self.conn_at(handle) else { return };
        say!(self.grants, "bt: link G#{handle:X} parameters: status G#{:02X} interval A#{} latency A#{} timeout A#{}\n", d[0], le16(d, 3), le16(d, 5), le16(d, 7));
        let mut body = [0u8; 9];
        body[0..2].copy_from_slice(&handle.to_le_bytes());
        body[2] = d[0];
        body[3..9].copy_from_slice(&d[3..9]);
        let s = self.conns[ci].session;
        self.event(s, Ev::new(EV_UPDATED, &body));
    }

    /// LE Data Length Change (LE Meta 7): [handle u16][max TX octets][max TX time][max RX octets][max RX time], each u16; the octets told to the client.
    pub fn length(&mut self, d: &[u8]) {
        if d.len() < 10 {
            return;
        }
        let handle = le16(d, 0) & 0x0FFF;
        let Some(ci) = self.conn_at(handle) else { return };
        say!(self.grants, "bt: link G#{handle:X} data length: TX A#{} octets, RX A#{}\n", le16(d, 2), le16(d, 6));
        let mut body = [0u8; 6];
        body[0..2].copy_from_slice(&handle.to_le_bytes());
        body[2..4].copy_from_slice(&d[2..4]);
        body[4..6].copy_from_slice(&d[6..8]);
        let s = self.conns[ci].session;
        self.event(s, Ev::new(EV_LENGTH, &body));
    }

    /// A link's PHYs ([status][handle u16][TX PHY][RX PHY], from LE Read PHY or LE PHY Update Complete), told to its client.
    pub fn phy(&mut self, d: &[u8]) {
        if d.len() < 5 || d[0] != 0 {
            return;
        }
        let handle = le16(d, 1) & 0x0FFF;
        let Some(ci) = self.conn_at(handle) else { return };
        say!(self.grants, "bt: link G#{handle:X} PHY: TX A#{} RX A#{} (1 = 1M, 2 = 2M, 3 = coded)\n", d[3], d[4]);
        let body = [d[1], d[2], d[3], d[4]];
        let s = self.conns[ci].session;
        self.event(s, Ev::new(EV_PHY, &body));
    }

    /// Advertising reports for the sessions scanning: [event type][address type][address 6][RSSI i8][data], one per advertiser and event type pending at most.
    pub fn adv_out(&mut self, kind: u8, addr_type: u8, addr: &[u8], rssi: u8, data: &[u8]) {
        let mut key = [0u8; 8];
        key[0] = kind;
        key[1] = addr_type;
        key[2..8].copy_from_slice(addr);
        let mut body = Vec::with_capacity(9 + data.len());
        body.extend_from_slice(&key);
        body.push(rssi);
        body.extend_from_slice(data);
        for si in 0..self.sessions.len() {
            if self.sessions[si].scan.is_none() {
                continue;
            }
            let mut e = Ev::new(EV_ADV, &body);
            e.key = Some(key);
            let id = self.sessions[si].id;
            self.event(id, e);
        }
    }

    /// Sessions whose process has ended, taken down: the watched one when the kernel rang (`RING` bit 63), and any whose held call's caller is gone (asked at the 8 s look and at the ring, never on a clock of its own).
    pub fn sweep(&mut self, rang: bool) {
        if rang && self.watched != 0 {
            let id = self.watched;
            say!(self.grants, "bt: a client ended: its session taken down\n");
            self.end_session(id);
        }
        let mut gone = Vec::new();
        for s in &self.sessions {
            if s.held.iter().any(|(t, _)| ferros_sys::caller_alive(t) == Some(false)) {
                gone.push(s.id);
            }
        }
        for id in gone {
            say!(self.grants, "bt: a client ended: its session taken down\n");
            self.end_session(id);
        }
    }
}
