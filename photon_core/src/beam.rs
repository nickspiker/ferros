//! The beam on the wire: the sender as a packetizer (Info, Orient and Data payloads around RaptorQ fountain symbols, sealed on the beam's own step chain) and the receiver that reassembles frames, mirrored from photon/src/wave/beam.rs (BEAM.md). Neither touches a socket.

use crate::wave::{self, Direction, StepChain};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use zeroize::Zeroize;

/// One fountain symbol: keeps a full datagram (5 + 1 + 4 + 4 + 8 + 4 + 1160 + 4 = 1190 bytes) under the IPv6 minimum MTU.
pub const SYMBOL_BYTES: u16 = 1160;
/// One repair symbol per this many source symbols, never fewer than one.
pub const REPAIR_PER_SOURCE: usize = 8;
/// Frames held open on the receiver awaiting symbols: older than this behind the newest completed frame, a frame is given up and counted lost.
pub const RX_HORIZON: u32 = 8;
/// Info is re-sent every this many frames.
pub const INFO_EVERY_FRAMES: u32 = 30;

const KIND_DATA: u8 = 0;
const KIND_INFO: u8 = 1;
const KIND_ORIENT: u8 = 2;
/// The only codec on the wire today.
pub const CODEC_H264: u8 = 1;

/// The stream's shape: coded size, frame rate, codec, and a VSF colour_profile document (empty means identity + gamma 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub width: u16,
    pub height: u16,
    pub fps: u8,
    pub codec: u8,
    pub colour: Vec<u8>,
}

/// Per-frame white-balance gains, Q12 (R, Gr, Gb, B); 4096 is no gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gains(pub [u16; 4]);

impl Gains {
    pub const NONE: Gains = Gains([4096; 4]);
}

/// The beam sender: one step chain and seq space for this wave and direction, which must never restart (a restarted chain is undecryptable to the receiver).
pub struct BeamTx {
    chain: StepChain,
    seq: u32,
}

impl BeamTx {
    pub fn new(wave_secret: &[u8; 32], dir: Direction) -> Self {
        let mut s = wave::beam_secret(wave_secret);
        let chain = StepChain::new(&s, dir);
        s.zeroize();
        Self { chain, seq: 0 }
    }

    /// The next seq this sender will use.
    pub fn seq(&self) -> u32 {
        self.seq
    }

    fn seal(&mut self, payload: &[u8]) -> Vec<u8> {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.chain.advance_to(StepChain::step_for_seq(seq));
        wave::seal(&self.chain, seq, payload, wave::BEAM_MAGIC)
    }

    /// The picture's rotation, in quarter turns clockwise.
    pub fn orient(&mut self, quarter_turns: u8) -> Vec<u8> {
        self.seal(&[KIND_ORIENT, quarter_turns & 3])
    }

    /// The stream's shape (send at start and every `INFO_EVERY_FRAMES`).
    pub fn info(&mut self, info: &Info) -> Vec<u8> {
        let mut p = Vec::with_capacity(7 + info.colour.len());
        p.push(KIND_INFO);
        p.extend_from_slice(&info.width.to_le_bytes());
        p.extend_from_slice(&info.height.to_le_bytes());
        p.push(info.fps);
        p.push(info.codec);
        p.extend_from_slice(&info.colour);
        self.seal(&p)
    }

    /// One access unit (an H.264 Annex-B frame) as one RaptorQ object: every source symbol plus ⌈source / 8⌉ repair symbols, each sealed as its own datagram.
    pub fn frame(&mut self, frame_no: u32, gains: Gains, bytes: &[u8]) -> Vec<Vec<u8>> {
        if bytes.is_empty() || bytes.len() > u32::MAX as usize {
            return Vec::new();
        }
        let len = bytes.len() as u32;
        let mut head = [0u8; 17];
        head[0] = KIND_DATA;
        head[1..5].copy_from_slice(&frame_no.to_le_bytes());
        head[5..9].copy_from_slice(&len.to_le_bytes());
        for (i, g) in gains.0.iter().enumerate() {
            head[9 + i * 2..11 + i * 2].copy_from_slice(&g.to_le_bytes());
        }
        let enc = raptorq::Encoder::new(bytes, raptorq::ObjectTransmissionInformation::with_defaults(len as u64, SYMBOL_BYTES));
        let source_n = bytes.len().div_ceil(SYMBOL_BYTES as usize);
        let repair_n = source_n.div_ceil(REPAIR_PER_SOURCE);
        let mut out = Vec::new();
        for pkt in enc.get_encoded_packets(repair_n as u32) {
            let sym = pkt.serialize();
            let mut payload = Vec::with_capacity(head.len() + sym.len());
            payload.extend_from_slice(&head);
            payload.extend_from_slice(&sym);
            out.push(self.seal(&payload));
        }
        out
    }
}

/// A reassembled access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_no: u32,
    pub gains: Gains,
    pub bytes: Vec<u8>,
}

fn oti(len: u32) -> raptorq::ObjectTransmissionInformation {
    raptorq::ObjectTransmissionInformation::with_defaults(len as u64, SYMBOL_BYTES)
}

fn decode_info(p: &[u8]) -> Option<Info> {
    if p.len() < 7 || p[0] != KIND_INFO || p[6] != CODEC_H264 {
        return None;
    }
    Some(Info { width: u16::from_le_bytes([p[1], p[2]]), height: u16::from_le_bytes([p[3], p[4]]), fps: p[5], codec: p[6], colour: p[7..].to_vec() })
}

/// The beam receiver: opens datagrams on the beam's chain, feeds symbols to one fountain decoder per frame, hands out frames as they complete, and gives up on frames `RX_HORIZON` behind the newest.
pub struct BeamRx {
    chain: StepChain,
    open: BTreeMap<u32, (u32, Gains, raptorq::Decoder)>,
    done: BTreeMap<u32, ()>,
    newest_done: u32,
    info: Option<Info>,
    info_changed: bool,
    pub rotation: u8,
    pub frames_done: u64,
    pub frames_lost: u64,
    pub bad_packets: u64,
}

impl BeamRx {
    pub fn new(wave_secret: &[u8; 32], dir: Direction) -> Self {
        let mut s = wave::beam_secret(wave_secret);
        let chain = StepChain::new(&s, dir);
        s.zeroize();
        Self { chain, open: BTreeMap::new(), done: BTreeMap::new(), newest_done: 0, info: None, info_changed: false, rotation: 0, frames_done: 0, frames_lost: 0, bad_packets: 0 }
    }

    pub fn info(&self) -> Option<&Info> {
        self.info.as_ref()
    }

    /// The stream's Info, once each time it changes.
    pub fn take_info_change(&mut self) -> Option<Info> {
        if core::mem::take(&mut self.info_changed) {
            self.info.clone()
        } else {
            None
        }
    }

    /// One datagram in; a frame out when it completes one.
    pub fn push(&mut self, datagram: &[u8]) -> Option<Frame> {
        let opened = match wave::parse_header(datagram) {
            Some((wave::BEAM_MAGIC, seq, sealed)) => wave::open(&mut self.chain, seq, sealed),
            _ => None,
        };
        let Some(payload) = opened else {
            self.bad_packets += 1;
            return None;
        };
        match payload.first() {
            Some(&KIND_INFO) => {
                match decode_info(&payload) {
                    Some(info) if self.info.as_ref() != Some(&info) => {
                        self.info = Some(info);
                        self.info_changed = true;
                    }
                    Some(_) => {}
                    None => self.bad_packets += 1,
                }
                None
            }
            Some(&KIND_DATA) => self.push_symbol(&payload),
            Some(&KIND_ORIENT) if payload.len() >= 2 => {
                self.rotation = payload[1] & 3;
                None
            }
            _ => {
                self.bad_packets += 1;
                None
            }
        }
    }

    fn push_symbol(&mut self, p: &[u8]) -> Option<Frame> {
        if p.len() <= 17 {
            return None;
        }
        let frame_no = u32::from_le_bytes([p[1], p[2], p[3], p[4]]);
        let len = u32::from_le_bytes([p[5], p[6], p[7], p[8]]);
        let gains = Gains(core::array::from_fn(|i| u16::from_le_bytes([p[9 + i * 2], p[10 + i * 2]])));
        if len == 0 {
            self.bad_packets += 1;
            return None;
        }
        // A late duplicate of a frame already handed out, or one already given up on.
        if self.done.contains_key(&frame_no) || frame_no.saturating_add(RX_HORIZON) < self.newest_done {
            return None;
        }
        let pkt = raptorq::EncodingPacket::deserialize(&p[17..]);
        let entry = self.open.entry(frame_no).or_insert_with(|| (len, gains, raptorq::Decoder::new(oti(len))));
        // Two symbols disagreeing about the frame's length cannot both be this frame: keep the first.
        if entry.0 != len {
            self.bad_packets += 1;
            return None;
        }
        let bytes = entry.2.decode(pkt)?;
        self.open.remove(&frame_no);
        self.done.insert(frame_no, ());
        self.frames_done += 1;
        self.newest_done = self.newest_done.max(frame_no);
        self.reap();
        Some(Frame { frame_no, gains, bytes })
    }

    fn reap(&mut self) {
        let floor = self.newest_done.saturating_sub(RX_HORIZON);
        let stale: Vec<u32> = self.open.range(..floor).map(|(k, _)| *k).collect();
        for k in stale {
            self.open.remove(&k);
            self.frames_lost += 1;
        }
        let forget: Vec<u32> = self.done.range(..floor.saturating_sub(RX_HORIZON)).map(|(k, _)| *k).collect();
        for k in forget {
            self.done.remove(&k);
        }
    }

    pub fn open_frames(&self) -> usize {
        self.open.len()
    }
}
