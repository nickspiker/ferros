//! LE links (docs/BT.md "Links and channels"): connections the driver made as a central, their ACL data on the H4 transport in both directions, and L2CAP on them: the LE signalling channel (CID 5) and LE credit-based connection-oriented channels.
//!
//! Every byte here that came from the radio is external input: each length is checked before it is read, and anything that would lose or reorder an SDU (a K-frame past its credits, an SDU past its MTU, a fragment out of place) closes the channel or the link and says why, never delivering a part or skipping one (the accessories' rule, lumis ACCESSORIES.md: a drop or reorder ends the session).
//! What arrives on ATT (CID 4) and SMP (CID 6) is answered "not supported": the driver serves no GATT and pairs nothing yet.

use crate::api::{Ev, EV_COC_CLOSED, EV_COC_OPENED, EV_DISCONNECTED, EV_SDU};
use crate::{Bt, Todo};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// The LE signalling channel's codes (Core 5.4 Vol 3 Part A 4).
const SIG_REJECT: u8 = 0x01;
const SIG_DISCONN_REQ: u8 = 0x06;
const SIG_DISCONN_RSP: u8 = 0x07;
const SIG_PARAM_REQ: u8 = 0x12;
const SIG_PARAM_RSP: u8 = 0x13;
const SIG_LE_CONN_REQ: u8 = 0x14;
const SIG_LE_CONN_RSP: u8 = 0x15;
const SIG_CREDIT: u8 = 0x16;

/// The fixed channels on LE.
const CID_ATT: u16 = 4;
const CID_SIG: u16 = 5;
const CID_SMP: u16 = 6;
/// LE's dynamic channel ids (Core Vol 3 Part A 2.1): sixty-four on each link, the spec's range, not a choice of ours.
const CID_FIRST: u16 = 0x40;
const CID_LAST: u16 = 0x7F;

/// Why a channel closed (`EV_COC_CLOSED`'s third field), and why the driver ended a link (`EV_DISCONNECTED` with why 2).
pub const WHY_PEER: u8 = 0;
pub const WHY_ASKED: u8 = 1;
pub const WHY_LINK: u8 = 2;
pub const WHY_CREDITS: u8 = 3;
pub const WHY_OVER_MTU: u8 = 4;
pub const WHY_OVER_MPS: u8 = 5;
pub const WHY_OVER_LENGTH: u8 = 6;
pub const WHY_SHORT: u8 = 7;
pub const WHY_CREDIT_OVERFLOW: u8 = 8;
pub const WHY_REASSEMBLY: u8 = 9;
pub const WHY_TRANSPORT: u8 = 10;
pub const WHY_CHIP: u8 = 11;
pub const WHY_BAD_RESPONSE: u8 = 12;
pub const WHY_NO_ANSWER: u8 = 13;

/// The signalling request's first response timeout (RTX, Core Vol 3 Part A 6.2.1: 1 to 60 s), doubled at each duplicate the spec allows; past `RTX_LAST` the open is given up. Measured 2026-11-24: Linux (7.0.14, the dev box's BlueZ) drops an ACL packet that arrives before its host has taken the connection's completion ("ACL packet for unknown connection handle"), so a request sent the moment the link is up can be lost, and only a duplicate opens the channel.
const RTX_FIRST_MS: u64 = 1 << 10;
const RTX_LAST_MS: u64 = 1 << 15;

/// The words for each `WHY_*`, for the console.
pub fn why_text(w: u8) -> &'static str {
    match w {
        WHY_PEER => "the peer closed it",
        WHY_ASKED => "the client closed it",
        WHY_LINK => "the link ended",
        WHY_CREDITS => "the peer sent a K-frame with no credit left",
        WHY_OVER_MTU => "an SDU longer than the channel's MTU",
        WHY_OVER_MPS => "a K-frame longer than the channel's MPS",
        WHY_OVER_LENGTH => "more bytes than the SDU's length",
        WHY_SHORT => "a first K-frame without its SDU length",
        WHY_CREDIT_OVERFLOW => "the peer's credits past 65535",
        WHY_REASSEMBLY => "an ACL fragment out of place (a PDU lost)",
        WHY_TRANSPORT => "the UART lost its H4 framing",
        WHY_CHIP => "the chip restarted",
        WHY_BAD_RESPONSE => "the peer's answer to the open was out of range",
        WHY_NO_ANSWER => "the peer never answered the open (the request sent A#6 times over 63 s)",
        _ => "?",
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ChanState {
    /// Our LE_CREDIT_BASED_CONNECTION_REQ is out with this identifier.
    Opening(u8),
    Open,
    /// Our DISCONNECTION_REQ is out with this identifier; the client was already told it closed.
    Closing(u8),
    /// Closed by the client while its open was still out (this identifier): an accepting answer is closed again at once, any answer forgets it.
    Abandoned(u8),
}

/// One SDU a client sent, its call held until every K-frame of it is handed to the transport.
pub struct Send {
    pub token: [u8; 32],
    pub sdu: Vec<u8>,
    /// SDU bytes put in K-frames so far.
    pub at: usize,
    /// Its first K-frame (the one with the SDU length) made.
    pub started: bool,
}

pub struct Chan {
    /// Ours: the peer sends to it.
    pub cid: u16,
    /// The peer's: we send to it.
    pub dcid: u16,
    pub psm: u16,
    pub state: ChanState,
    /// What we receive: our MTU and MPS.
    pub mtu: u16,
    pub mps: u16,
    pub peer_mtu: u16,
    pub peer_mps: u16,
    /// K-frames we may send.
    pub tx_credits: u32,
    /// K-frames the peer may send us: every one a frame of room the client has not taken yet.
    pub rx_credits: u32,
    /// Credits the client freed (SDUs it took) not yet handed back.
    pub owed: u32,
    /// The SDU coming in: its bytes, its length once its first K-frame came, the K-frames it took.
    pub sdu: Vec<u8>,
    pub sdu_len: Option<usize>,
    pub sdu_frames: u32,
    pub sends: VecDeque<Send>,
    /// The open's request (its data), when it last went out and its timeout now (counter ticks): sent again on the timeout (`rtx`).
    pub req: [u8; 10],
    pub asked_at: u64,
    pub rtx: u64,
    pub sdus_in: u64,
    pub sdus_out: u64,
}

pub struct Conn {
    pub handle: u16,
    /// The client session it belongs to; 0 once that session ended (the link is being taken down).
    pub session: u64,
    pub peer: (u8, [u8; 6]),
    /// The client asked for its own connection parameters: a peer's request to change them is refused.
    pub asked: bool,
    /// The L2CAP PDU being reassembled from ACL fragments: empty between PDUs.
    pub reasm: Vec<u8>,
    /// L2CAP PDUs waiting for the controller's ACL buffers, whole; the front one sent up to `out_at`.
    pub out: VecDeque<Vec<u8>>,
    pub out_at: usize,
    /// ACL packets handed to the controller and not yet reported done (Number_Of_Completed_Packets).
    pub in_flight: u16,
    pub chans: Vec<Chan>,
    /// The channel `flush_out` takes the next K-frame from (round robin).
    pub turn: usize,
    /// The driver asked the controller to end it; data still arriving is not read.
    pub ending: bool,
    /// Why the driver ended it (a `WHY_*`), said with its disconnection.
    pub fault: Option<u8>,
}

/// A connection asked for and not yet complete: the LE Create Connection is in the controller.
pub struct Pending {
    pub session: u64,
    pub peer: (u8, [u8; 6]),
    pub asked: bool,
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

impl Bt {
    pub fn conn_at(&self, handle: u16) -> Option<usize> {
        self.conns.iter().position(|c| c.handle == handle)
    }

    /// The next signalling identifier, never 0 (Core Vol 3 Part A 4: 0 is invalid).
    fn ident(&mut self) -> u8 {
        self.ident = if self.ident == 0xFF { 1 } else { self.ident + 1 };
        self.ident
    }

    /// An L2CAP PDU onto a link's queue: basic header (length, channel id), then the payload.
    pub fn pdu(&mut self, ci: usize, cid: u16, payload: &[u8]) {
        let mut p = Vec::with_capacity(4 + payload.len());
        p.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        p.extend_from_slice(&cid.to_le_bytes());
        p.extend_from_slice(payload);
        self.conns[ci].out.push_back(p);
    }

    fn signal(&mut self, ci: usize, code: u8, ident: u8, data: &[u8]) {
        let mut p = Vec::with_capacity(4 + data.len());
        p.push(code);
        p.push(ident);
        p.extend_from_slice(&(data.len() as u16).to_le_bytes());
        p.extend_from_slice(data);
        self.sig_out += 1;
        self.pdu(ci, CID_SIG, &p);
    }

    /// An HCI ACL data packet from the controller: [handle and flags u16][length u16][data]. Fragments are joined into L2CAP PDUs per link; a fragment out of place ends the link (a PDU was lost).
    pub fn on_acl(&mut self, a: &[u8]) {
        if a.len() < 4 {
            return;
        }
        self.acl_in += 1;
        let hf = le16(a, 0);
        let (handle, pb) = (hf & 0x0FFF, (hf >> 12) & 3);
        let data = &a[4..];
        let Some(ci) = self.conn_at(handle) else {
            self.stray_acl += 1;
            return;
        };
        if self.conns[ci].ending {
            return;
        }
        let c = &mut self.conns[ci];
        match pb {
            // A first fragment (G#2 from the controller, G#0 allowed): a PDU still being joined is one that lost its end.
            0 | 2 => {
                if !c.reasm.is_empty() {
                    return self.link_fault(ci, WHY_REASSEMBLY);
                }
                c.reasm.extend_from_slice(data);
            }
            1 => {
                if c.reasm.is_empty() {
                    return self.link_fault(ci, WHY_REASSEMBLY);
                }
                c.reasm.extend_from_slice(data);
            }
            _ => return self.link_fault(ci, WHY_REASSEMBLY),
        }
        let c = &mut self.conns[ci];
        if c.reasm.len() < 4 {
            return;
        }
        let need = 4 + le16(&c.reasm, 0) as usize;
        if c.reasm.len() < need {
            return;
        }
        if c.reasm.len() > need {
            return self.link_fault(ci, WHY_REASSEMBLY);
        }
        let p = core::mem::take(&mut c.reasm);
        let cid = le16(&p, 2);
        let k = match cid {
            CID_SIG => 0,
            CID_ATT => 1,
            CID_SMP => 2,
            _ => 3,
        };
        self.pdus_in[k] += 1;
        match cid {
            CID_ATT => self.att(ci, &p[4..]),
            CID_SIG => self.sig(ci, &p[4..]),
            CID_SMP => self.smp(ci, &p[4..]),
            CID_FIRST..=CID_LAST => self.kframe(ci, cid, &p[4..]),
            _ => self.stray_acl += 1,
        }
    }

    /// ATT from the peer: the driver serves no attributes. Each request is answered with an Error Response (Attribute Not Found for the discovery reads, Request Not Supported for the rest), an MTU exchange with the default 23, an indication confirmed; commands and notifications need nothing. Unanswered, a peer's GATT client waits 30 s and drops the link (BlueZ does).
    fn att(&mut self, ci: usize, p: &[u8]) {
        let Some(&op) = p.first() else { return };
        match op {
            // Exchange MTU Request: our ATT MTU, the default.
            0x02 => self.pdu(ci, CID_ATT, &[0x03, 23, 0]),
            // Find Information, Find By Type Value, Read By Type, Read By Group Type: nothing found, at the request's first handle.
            0x04 | 0x06 | 0x08 | 0x10 if p.len() >= 3 => self.pdu(ci, CID_ATT, &[0x01, op, p[1], p[2], 0x0A]),
            // Handle Value Indication: confirmed.
            0x1D => self.pdu(ci, CID_ATT, &[0x1E]),
            // Responses, notifications and commands (bit 6) answer nothing.
            0x01 | 0x03 | 0x05 | 0x07 | 0x09 | 0x0B | 0x0D | 0x0F | 0x11 | 0x13 | 0x17 | 0x19 | 0x1B | 0x1E => {}
            _ if op & 0x40 != 0 => {}
            _ => self.pdu(ci, CID_ATT, &[0x01, op, 0, 0, 0x06]),
        }
    }

    /// SMP from the peer: a Pairing Request or a Security Request is answered Pairing Failed, Pairing Not Supported (the driver pairs nothing yet: pairing belongs to the identity, BT.md "Next").
    fn smp(&mut self, ci: usize, p: &[u8]) {
        if matches!(p.first(), Some(0x01) | Some(0x0B)) {
            self.pdu(ci, CID_SMP, &[0x05, 0x05]);
        }
    }

    /// One command on the LE signalling channel: [code][identifier][length u16][data], one to a C-frame on LE.
    fn sig(&mut self, ci: usize, p: &[u8]) {
        if p.len() < 4 || p.len() != 4 + le16(p, 2) as usize {
            self.stray_acl += 1;
            return;
        }
        let (code, id, d) = (p[0], p[1], &p[4..]);
        match code {
            SIG_LE_CONN_RSP if d.len() >= 10 => {
                let (dcid, mtu, mps, credits, result) = (le16(d, 0), le16(d, 2), le16(d, 4), le16(d, 6), le16(d, 8));
                if let Some(k) = self.conns[ci].chans.iter().position(|c| c.state == ChanState::Abandoned(id)) {
                    let ch = &mut self.conns[ci].chans[k];
                    let cid = ch.cid;
                    if result == 0 {
                        let id = self.ident();
                        self.conns[ci].chans[k].state = ChanState::Closing(id);
                        self.signal(ci, SIG_DISCONN_REQ, id, &[(dcid & 0xFF) as u8, (dcid >> 8) as u8, (cid & 0xFF) as u8, (cid >> 8) as u8]);
                    } else {
                        self.conns[ci].chans.remove(k);
                    }
                    return;
                }
                let Some(k) = self.conns[ci].chans.iter().position(|c| c.state == ChanState::Opening(id)) else { return };
                let session = self.conns[ci].session;
                let handle = self.conns[ci].handle;
                let ch = &mut self.conns[ci].chans[k];
                let cid = ch.cid;
                // The peer's numbers are checked against the spec's ranges before they size anything: a channel id outside the dynamic range, or an MTU or MPS under 23 or an MPS past 65533 (Core Vol 3 Part A 4.23), cannot be used.
                let sane = (CID_FIRST..=CID_LAST).contains(&dcid);
                let ok = result == 0 && sane && mtu >= 23 && (23..=65533).contains(&mps);
                let mut body = Vec::with_capacity(16);
                body.extend_from_slice(&handle.to_le_bytes());
                body.extend_from_slice(&cid.to_le_bytes());
                body.extend_from_slice(&result.to_le_bytes());
                body.extend_from_slice(&mtu.to_le_bytes());
                body.extend_from_slice(&mps.to_le_bytes());
                body.extend_from_slice(&credits.to_le_bytes());
                body.extend_from_slice(&ch.mtu.to_le_bytes());
                body.extend_from_slice(&ch.mps.to_le_bytes());
                if ok {
                    ch.dcid = dcid;
                    ch.peer_mtu = mtu;
                    ch.peer_mps = mps;
                    ch.tx_credits = credits as u32;
                    ch.state = ChanState::Open;
                    say!(self.grants, "bt: channel G#{cid:X} on link G#{handle:X} open: PSM G#{:X}, the peer's MTU A#{mtu} MPS A#{mps} credits A#{credits}, ours MTU A#{} MPS A#{} credits A#{}\n", ch.psm, ch.mtu, ch.mps, ch.rx_credits);
                    self.event(session, Ev::new(EV_COC_OPENED, &body));
                } else {
                    self.conns[ci].chans.remove(k);
                    say!(self.grants, "bt: channel G#{cid:X} on link G#{handle:X} refused: result G#{result:X}\n");
                    if result == 0 {
                        // Accepted with numbers outside the spec: closed again at once, and the client told it failed.
                        let id = self.ident();
                        self.signal(ci, SIG_DISCONN_REQ, id, &[(dcid & 0xFF) as u8, (dcid >> 8) as u8, (cid & 0xFF) as u8, (cid >> 8) as u8]);
                        body[4..6].copy_from_slice(&0xFFFEu16.to_le_bytes());
                    }
                    self.event(session, Ev::new(EV_COC_OPENED, &body));
                }
            }
            SIG_CREDIT if d.len() >= 4 => {
                let (cid, n) = (le16(d, 0), le16(d, 2));
                let Some(k) = self.conns[ci].chans.iter().position(|c| c.dcid == cid && c.state == ChanState::Open) else { return };
                let ch = &mut self.conns[ci].chans[k];
                ch.tx_credits += n as u32;
                // The spec's ceiling on the credit count (Core Vol 3 Part A 10.1): a peer past it has lost count, and the channel cannot be trusted.
                if ch.tx_credits > 0xFFFF {
                    self.chan_fault(ci, k, WHY_CREDIT_OVERFLOW);
                }
            }
            SIG_DISCONN_REQ if d.len() >= 4 => {
                let (ours, theirs) = (le16(d, 0), le16(d, 2));
                match self.conns[ci].chans.iter().position(|c| c.cid == ours && c.dcid == theirs) {
                    Some(k) => {
                        self.signal(ci, SIG_DISCONN_RSP, id, &d[..4]);
                        let was_closing = matches!(self.conns[ci].chans[k].state, ChanState::Closing(_));
                        let ch = self.conns[ci].chans.remove(k);
                        if !was_closing {
                            self.chan_closed(ci, ch, WHY_PEER);
                        }
                    }
                    // Invalid CID in request: [reason 2][our cid][their cid].
                    None => self.signal(ci, SIG_REJECT, id, &[0x02, 0x00, d[0], d[1], d[2], d[3]]),
                }
            }
            SIG_DISCONN_RSP if d.len() >= 4 => {
                let ours = le16(d, 2);
                if let Some(k) = self.conns[ci].chans.iter().position(|c| c.cid == ours && matches!(c.state, ChanState::Closing(_))) {
                    self.conns[ci].chans.remove(k);
                }
            }
            SIG_REJECT => {
                if let Some(k) = self.conns[ci].chans.iter().position(|c| c.state == ChanState::Opening(id) || c.state == ChanState::Closing(id) || c.state == ChanState::Abandoned(id)) {
                    let ch = self.conns[ci].chans.remove(k);
                    if ch.state == ChanState::Opening(id) {
                        let (session, handle) = (self.conns[ci].session, self.conns[ci].handle);
                        let mut body = Vec::with_capacity(16);
                        body.extend_from_slice(&handle.to_le_bytes());
                        body.extend_from_slice(&ch.cid.to_le_bytes());
                        body.extend_from_slice(&0xFFFFu16.to_le_bytes());
                        body.extend_from_slice(&[0; 6]);
                        body.extend_from_slice(&ch.mtu.to_le_bytes());
                        body.extend_from_slice(&ch.mps.to_le_bytes());
                        say!(self.grants, "bt: channel G#{:X} on link G#{handle:X}: the peer rejected the open\n", ch.cid);
                        self.event(session, Ev::new(EV_COC_OPENED, &body));
                    }
                }
            }
            SIG_PARAM_REQ if d.len() >= 8 => {
                let v = [le16(d, 0), le16(d, 2), le16(d, 4), le16(d, 6)];
                // The spec's ranges (Core Vol 3 Part A 4.20, Vol 4 Part E 7.8.18): interval 6 to 3200 with min ≤ max, latency ≤ 499, timeout 10 to 3200 and longer than (1 + latency) × max × 2 intervals; a request outside them is refused rather than handed to the controller.
                let valid = (6..=3200).contains(&v[0]) && v[0] <= v[1] && v[1] <= 3200 && v[2] <= 499 && (10..=3200).contains(&v[3]) && v[3] as u32 * 4 > (1 + v[2] as u32) * v[1] as u32;
                let accept = valid && !self.conns[ci].asked;
                self.signal(ci, SIG_PARAM_RSP, id, &[!accept as u8, 0]);
                let handle = self.conns[ci].handle;
                say!(self.grants, "bt: link G#{handle:X}: the peer asked interval A#{}-A#{} latency A#{} timeout A#{}: {}\n", v[0], v[1], v[2], v[3], if accept { "accepted" } else if valid { "refused (the client chose its own)" } else { "refused (out of range)" });
                if accept {
                    self.todo.push_back(Todo::Update(handle, v));
                }
            }
            // A peer opening a channel to us: no LE PSM is served here (the driver is a central with no server role yet): LE_PSM not supported.
            SIG_LE_CONN_REQ => self.signal(ci, SIG_LE_CONN_RSP, id, &[0, 0, 0, 0, 0, 0, 0, 0, 0x02, 0]),
            // Any other request (Core Vol 3 Part A 4: BR/EDR's connection, configuration, echo, information and move requests, and the enhanced credit-based connection and reconfigure requests G#17 and G#19): not understood. Responses to nothing of ours, and malformed commands, are dropped.
            0x02 | 0x04 | 0x08 | 0x0A | 0x0C | 0x0E | 0x10 | 0x17 | 0x19 => self.signal(ci, SIG_REJECT, id, &[0, 0]),
            _ => {}
        }
    }

    /// A K-frame on a dynamic channel: one credit spent, and the SDU it carries joined (the first K-frame starts with the SDU's length). A complete SDU goes to the client whole; anything that would make it partial or skip one closes the channel.
    fn kframe(&mut self, ci: usize, cid: u16, p: &[u8]) {
        let Some(k) = self.conns[ci].chans.iter().position(|c| c.cid == cid) else {
            self.stray_acl += 1;
            return;
        };
        let ch = &mut self.conns[ci].chans[k];
        if ch.state != ChanState::Open {
            return;
        }
        if ch.rx_credits == 0 {
            return self.chan_fault(ci, k, WHY_CREDITS);
        }
        ch.rx_credits -= 1;
        if p.len() > ch.mps as usize {
            return self.chan_fault(ci, k, WHY_OVER_MPS);
        }
        let len = match ch.sdu_len {
            None => {
                if p.len() < 2 {
                    return self.chan_fault(ci, k, WHY_SHORT);
                }
                let len = le16(p, 0) as usize;
                if len > ch.mtu as usize {
                    return self.chan_fault(ci, k, WHY_OVER_MTU);
                }
                ch.sdu.clear();
                ch.sdu.reserve(len);
                ch.sdu.extend_from_slice(&p[2..]);
                ch.sdu_len = Some(len);
                ch.sdu_frames = 1;
                len
            }
            Some(len) => {
                ch.sdu.extend_from_slice(p);
                ch.sdu_frames += 1;
                len
            }
        };
        if ch.sdu.len() > len {
            return self.chan_fault(ci, k, WHY_OVER_LENGTH);
        }
        if ch.sdu.len() < len {
            return;
        }
        let sdu = core::mem::take(&mut ch.sdu);
        let frames = ch.sdu_frames;
        ch.sdu_len = None;
        ch.sdu_frames = 0;
        ch.sdus_in += 1;
        let (session, handle) = (self.conns[ci].session, self.conns[ci].handle);
        let mut body = Vec::with_capacity(4 + sdu.len());
        body.extend_from_slice(&handle.to_le_bytes());
        body.extend_from_slice(&cid.to_le_bytes());
        body.extend_from_slice(&sdu);
        let mut e = Ev::new(EV_SDU, &body);
        e.credit = Some((handle, cid, frames));
        self.event(session, e);
    }

    /// Credits for frames the client took, handed back to the peer in one FLOW_CONTROL_CREDIT_IND.
    pub fn give_back(&mut self, handle: u16, cid: u16, frames: u32) {
        let Some(ci) = self.conn_at(handle) else { return };
        let Some(k) = self.conns[ci].chans.iter().position(|c| c.cid == cid && c.state == ChanState::Open) else { return };
        let ch = &mut self.conns[ci].chans[k];
        ch.owed += frames;
        let n = ch.owed;
        ch.owed = 0;
        ch.rx_credits += n;
        let id = self.ident();
        self.signal(ci, SIG_CREDIT, id, &[(cid & 0xFF) as u8, (cid >> 8) as u8, (n & 0xFF) as u8, (n >> 8) as u8]);
    }

    /// A channel the driver must close for what the peer did: the client told why at once, the peer asked to disconnect it.
    pub fn chan_fault(&mut self, ci: usize, k: usize, why: u8) {
        let (handle, cid) = (self.conns[ci].handle, self.conns[ci].chans[k].cid);
        say!(self.grants, "bt: channel G#{cid:X} on link G#{handle:X} closed: {}\n", why_text(why));
        self.chan_close(ci, k, why);
    }

    /// Close channel `k` from our side: DISCONNECTION_REQ to the peer, the client told at once (with `why`), its held sends answered refused, anything still arriving on it dropped until the peer answers.
    pub fn chan_close(&mut self, ci: usize, k: usize, why: u8) {
        let id = self.ident();
        let (cid, dcid) = (self.conns[ci].chans[k].cid, self.conns[ci].chans[k].dcid);
        let was_open = self.conns[ci].chans[k].state == ChanState::Open;
        if was_open {
            self.signal(ci, SIG_DISCONN_REQ, id, &[(dcid & 0xFF) as u8, (dcid >> 8) as u8, (cid & 0xFF) as u8, (cid >> 8) as u8]);
        }
        let ch = &mut self.conns[ci].chans[k];
        let sends = core::mem::take(&mut ch.sends);
        ch.sdu.clear();
        ch.sdu_len = None;
        ch.state = match ch.state {
            ChanState::Open => ChanState::Closing(id),
            ChanState::Opening(asked) => ChanState::Abandoned(asked),
            s => s,
        };
        for s in sends {
            ferros_sys::reply(&s.token, &[1]);
        }
        let (session, handle) = (self.conns[ci].session, self.conns[ci].handle);
        let mut body = [0u8; 5];
        body[..2].copy_from_slice(&handle.to_le_bytes());
        body[2..4].copy_from_slice(&cid.to_le_bytes());
        body[4] = why;
        self.event(session, Ev::new(EV_COC_CLOSED, &body));
    }

    /// A channel already gone (the peer closed it, or its link went): its held sends refused and the client told.
    fn chan_closed(&mut self, ci: usize, ch: Chan, why: u8) {
        for s in ch.sends {
            ferros_sys::reply(&s.token, &[1]);
        }
        let (session, handle) = (self.conns[ci].session, self.conns[ci].handle);
        say!(self.grants, "bt: channel G#{:X} on link G#{handle:X} closed: {} (A#{} SDUs in, A#{} out)\n", ch.cid, why_text(why), ch.sdus_in, ch.sdus_out);
        let mut body = [0u8; 5];
        body[..2].copy_from_slice(&handle.to_le_bytes());
        body[2..4].copy_from_slice(&ch.cid.to_le_bytes());
        body[4] = why;
        self.event(session, Ev::new(EV_COC_CLOSED, &body));
    }

    /// Every channel of link `ci` closed for `why` (its link is going).
    pub fn chans_gone(&mut self, ci: usize, why: u8) {
        let chans = core::mem::take(&mut self.conns[ci].chans);
        for ch in chans {
            if matches!(ch.state, ChanState::Closing(_) | ChanState::Abandoned(_)) {
                continue;
            }
            self.chan_closed(ci, ch, why);
        }
    }

    /// The link itself cannot go on (a fragment out of place: a PDU lost): its channels closed with `why`, the controller asked to end it, the client's disconnection event carrying `why`.
    pub fn link_fault(&mut self, ci: usize, why: u8) {
        let handle = self.conns[ci].handle;
        say!(self.grants, "bt: link G#{handle:X} ended by the driver: {}\n", why_text(why));
        self.chans_gone(ci, why);
        let c = &mut self.conns[ci];
        c.reasm.clear();
        c.fault = Some(why);
        if !c.ending {
            c.ending = true;
            self.todo.push_back(Todo::Disconnect(handle, 0x13));
        }
    }

    /// Disconnection Complete for `handle`: its buffers in the controller are free again, its channels closed, the client told (with the driver's own reason when it ended the link for a fault).
    pub fn conn_gone(&mut self, handle: u16, reason: u8) {
        let Some(ci) = self.conn_at(handle) else { return };
        self.acl_free += self.conns[ci].in_flight;
        self.chans_gone(ci, WHY_LINK);
        let c = self.conns.remove(ci);
        say!(self.grants, "bt: link G#{handle:X} ended: reason G#{reason:02X}{}\n", match c.fault {
            Some(_) => " (the driver ended it)",
            None => "",
        });
        let mut body = [0u8; 4];
        body[..2].copy_from_slice(&handle.to_le_bytes());
        (body[2], body[3]) = match c.fault {
            Some(w) => (2, w),
            None => (0, reason),
        };
        self.event(c.session, Ev::new(EV_DISCONNECTED, &body));
    }

    /// Open a credit-based channel on link `ci` to `psm`, receiving SDUs up to `mtu` in K-frames up to `mps`: the request out, the channel's id ours. The credits we give are four whole SDUs' worth of K-frames, and each comes back once the client has taken the SDU that spent it, so the client's unread SDUs are all the memory a channel holds. None when the link's sixty-four ids are all in use.
    pub fn coc_open(&mut self, ci: usize, psm: u16, mtu: u16, mps: u16) -> Option<u16> {
        let cid = (CID_FIRST..=CID_LAST).find(|id| !self.conns[ci].chans.iter().any(|c| c.cid == *id))?;
        let frames = (mtu as u32 + 2).div_ceil(mps as u32);
        let credits = 4 * frames;
        let id = self.ident();
        let mut d = [0u8; 10];
        d[0..2].copy_from_slice(&psm.to_le_bytes());
        d[2..4].copy_from_slice(&cid.to_le_bytes());
        d[4..6].copy_from_slice(&mtu.to_le_bytes());
        d[6..8].copy_from_slice(&mps.to_le_bytes());
        d[8..10].copy_from_slice(&(credits as u16).to_le_bytes());
        self.signal(ci, SIG_LE_CONN_REQ, id, &d);
        let rtx = ferros_sys::freq() / 1000 * RTX_FIRST_MS;
        self.conns[ci].chans.push(Chan { cid, dcid: 0, psm, state: ChanState::Opening(id), mtu, mps, peer_mtu: 0, peer_mps: 0, tx_credits: 0, rx_credits: credits, owed: 0, sdu: Vec::new(), sdu_len: None, sdu_frames: 0, sends: VecDeque::new(), req: d, asked_at: ferros_sys::now(), rtx, sdus_in: 0, sdus_out: 0 });
        Some(cid)
    }

    /// Opens unanswered past their RTX: the same request again with the same identifier (the spec's duplicate, its timeout doubled), or past the last timeout the open given up and the client told (`WHY_NO_ANSWER`). Returns when the next one falls due (counter ticks), or None with no open out: the loop's wait ends then, the only deadline here, and the spec's.
    pub fn rtx(&mut self) -> Option<u64> {
        let t = ferros_sys::now();
        let last = ferros_sys::freq() / 1000 * RTX_LAST_MS;
        let mut next: Option<u64> = None;
        for ci in 0..self.conns.len() {
            let mut k = 0;
            while k < self.conns[ci].chans.len() {
                let ch = &mut self.conns[ci].chans[k];
                let ChanState::Opening(id) = ch.state else {
                    k += 1;
                    continue;
                };
                if t < ch.asked_at + ch.rtx {
                    let due = ch.asked_at + ch.rtx;
                    next = Some(next.map_or(due, |n| n.min(due)));
                    k += 1;
                    continue;
                }
                if ch.rtx >= last {
                    self.chan_fault(ci, k, WHY_NO_ANSWER);
                    continue;
                }
                ch.rtx *= 2;
                ch.asked_at = t;
                let (d, due, cid) = (ch.req, t + ch.rtx, ch.cid);
                next = Some(next.map_or(due, |n| n.min(due)));
                let handle = self.conns[ci].handle;
                say!(self.grants, "bt: channel G#{cid:X} on link G#{handle:X}: no answer to the open, sent again\n");
                self.signal(ci, SIG_LE_CONN_REQ, id, &d);
                k += 1;
            }
        }
        next
    }

    /// The next K-frame of link `ci` that may go: from its open channels in turn, one with an SDU queued and a credit to spend. The send's call is answered once its last K-frame is made (handed to the transport).
    fn next_kframe(&mut self, ci: usize) -> Option<Vec<u8>> {
        let n = self.conns[ci].chans.len();
        for step in 0..n {
            let k = (self.conns[ci].turn + step) % n;
            let ch = &mut self.conns[ci].chans[k];
            if ch.state != ChanState::Open || ch.tx_credits == 0 || ch.sends.is_empty() {
                continue;
            }
            let mps = ch.peer_mps as usize;
            let dcid = ch.dcid;
            let s = ch.sends.front_mut().unwrap();
            let mut f = Vec::with_capacity(4 + mps);
            f.extend_from_slice(&[0, 0]);
            f.extend_from_slice(&dcid.to_le_bytes());
            let room = if s.started { mps } else { mps - 2 };
            if !s.started {
                f.extend_from_slice(&(s.sdu.len() as u16).to_le_bytes());
                s.started = true;
            }
            // `room` bounds the K-frame by the peer's MPS (its receive size); the SDU's rest may be shorter.
            let take = room.min(s.sdu.len() - s.at);
            f.extend_from_slice(&s.sdu[s.at..s.at + take]);
            s.at += take;
            let l = (f.len() - 4) as u16;
            f[0..2].copy_from_slice(&l.to_le_bytes());
            ch.tx_credits -= 1;
            if s.at == s.sdu.len() {
                let s = ch.sends.pop_front().unwrap();
                ch.sdus_out += 1;
                ferros_sys::reply(&s.token, &[0]);
            }
            self.conns[ci].turn = k + 1;
            return Some(f);
        }
        None
    }

    /// Everything that may go, into the controller: each link's queued PDUs fragmented to the controller's ACL size, one fragment per free ACL buffer, the links in turn; a link with nothing queued makes its next K-frame first. Fragments of one PDU go in order and unmixed on their link (L2CAP's rule).
    pub fn flush_out(&mut self) {
        loop {
            let mut moved = false;
            for ci in 0..self.conns.len() {
                if self.acl_free == 0 {
                    return;
                }
                if self.conns[ci].ending {
                    continue;
                }
                if self.conns[ci].out.is_empty() {
                    match self.next_kframe(ci) {
                        Some(f) => self.conns[ci].out.push_back(f),
                        None => continue,
                    }
                }
                let c = &mut self.conns[ci];
                let pdu = c.out.front().unwrap();
                let first = c.out_at == 0;
                // One ACL packet carries at most the controller's buffer size (LE_Read_Buffer_Size).
                let n = self.acl_len.min(pdu.len() - c.out_at);
                let mut a = Vec::with_capacity(5 + n);
                a.push(2);
                let hf = c.handle | if first { 0 } else { 1 << 12 };
                a.extend_from_slice(&hf.to_le_bytes());
                a.extend_from_slice(&(n as u16).to_le_bytes());
                a.extend_from_slice(&pdu[c.out_at..c.out_at + n]);
                c.out_at += n;
                if c.out_at == pdu.len() {
                    c.out.pop_front();
                    c.out_at = 0;
                }
                c.in_flight += 1;
                self.acl_free -= 1;
                self.acl_out += 1;
                self.put(&a);
                moved = true;
            }
            if !moved {
                return;
            }
        }
    }

    /// Number_Of_Completed_Packets: [n][n × (handle u16, count u16)], the controller's ACL buffers free again.
    pub fn completed(&mut self, p: &[u8]) {
        let Some(&n) = p.first() else { return };
        for k in 0..n as usize {
            let Some(e) = p.get(1 + 4 * k..5 + 4 * k) else { return };
            let (handle, count) = (le16(e, 0) & 0x0FFF, le16(e, 2));
            let Some(ci) = self.conn_at(handle) else { continue };
            // WHY the min: the controller's count; one past what this link has in flight would raise the free count past the controller's buffers and overrun them on the next send.
            let c = count.min(self.conns[ci].in_flight);
            self.conns[ci].in_flight -= c;
            self.acl_free += c;
        }
    }
}
