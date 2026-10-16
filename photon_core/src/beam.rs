//! The beam's sending side as a packetizer: Info, Orient and Data payloads around RaptorQ fountain symbols, sealed on the beam's own step chain, mirrored from photon/src/wave/beam.rs (BEAM.md). It returns wire datagrams; the caller owns the socket.

use crate::wave::{self, Direction, StepChain};
use alloc::vec::Vec;
use zeroize::Zeroize;

/// One fountain symbol: keeps a full datagram (5 + 1 + 4 + 4 + 8 + 4 + 1160 + 4 = 1190 bytes) under the IPv6 minimum MTU.
pub const SYMBOL_BYTES: u16 = 1160;
/// One repair symbol per this many source symbols, never fewer than one.
pub const REPAIR_PER_SOURCE: usize = 8;
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
