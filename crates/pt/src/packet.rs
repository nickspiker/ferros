//! PT packet types — encoding and decoding.
//!
//! Two packet families, discriminated by first byte: DATA: `[sid:1][seq:N][blake3:32][payload]` — first byte 'a'-'z' Control: `[TAG:1][...]` — first byte uppercase letter (S/A/N/C/D/F)
//!
//! Sequence width N is implicit from SPEC count (no per-packet tag overhead).

use ferros_ledger::ewe;

/// Stream identifier: 'a'-'z' (0x61-0x7A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamId(pub u8);

impl StreamId {
    pub const FIRST: StreamId = StreamId(b'a');

    pub fn new(id: u8) -> Option<Self> {
        if id >= b'a' && id <= b'z' {
            Some(StreamId(id))
        } else {
            None
        }
    }

    /// Index 0-25 for array indexing.
    pub fn index(self) -> usize {
        (self.0 - b'a') as usize
    }

    /// Next stream ID, wrapping z → a.
    pub fn next(self) -> Self {
        if self.0 >= b'z' {
            StreamId(b'a')
        } else {
            StreamId(self.0 + 1)
        }
    }
}

/// Decoded packet kind.
#[derive(Debug)]
pub enum PacketKind<'a> {
    /// DATA: stream_id, sequence number, chunk BLAKE3, payload slice.
    Data {
        sid: StreamId,
        seq: u64,
        chunk_hash: [u8; 32],
        payload: &'a [u8],
    },
    /// SPEC: transfer initiation.
    Spec(Spec),
    /// ACK: SPEC acknowledgment (seq=MAX) or retransmit ack.
    Ack(Ack),
    /// NAK: retransmit request with specific bad sequence numbers.
    Nak(Nak),
    /// CONTROL: flow control command.
    Control(ControlCmd),
    /// COMPLETE: transfer verification.
    Complete(Complete),
    /// FIN: end-of-blast signal.
    Fin(StreamId),
}

// Control packet tags — single uppercase byte, outside DATA range ('a'-'z').
const TAG_SPEC: u8 = b'S';
const TAG_ACK: u8 = b'A';
const TAG_NAK: u8 = b'N';
const TAG_CTRL: u8 = b'C';
const TAG_DONE: u8 = b'D';
const TAG_FIN: u8 = b'F';

/// Per-chunk BLAKE3 hash size in DATA packets.
pub const CHUNK_HASH_SIZE: usize = 32;

// ---------------------------------------------------------------------------
// DATA packet
// ---------------------------------------------------------------------------

/// Encode a DATA packet into `buf`. Format: [sid:1][seq:N][blake3:32][payload] The blake3 hash is computed over the payload. Returns bytes written.
pub fn encode_data(
    buf: &mut [u8],
    sid: StreamId,
    seq: u64,
    seq_width: usize,
    payload: &[u8],
) -> usize {
    let total = 1 + seq_width + CHUNK_HASH_SIZE + payload.len();
    if buf.len() < total {
        return 0;
    }

    buf[0] = sid.0;
    ewe::encode_seq(&mut buf[1..], seq, seq_width);

    let hash = blake3::hash(payload);
    let hash_start = 1 + seq_width;
    buf[hash_start..hash_start + 32].copy_from_slice(hash.as_bytes());

    let payload_start = hash_start + 32;
    buf[payload_start..payload_start + payload.len()].copy_from_slice(payload);
    total
}

/// Decode a DATA packet. Caller must provide seq_width (from SPEC). Returns (sid, seq, chunk_hash, payload).
pub fn decode_data(buf: &[u8], seq_width: usize) -> Option<(StreamId, u64, [u8; 32], &[u8])> {
    let min_len = 1 + seq_width + CHUNK_HASH_SIZE;
    if buf.len() < min_len {
        return None;
    }

    let sid = StreamId::new(buf[0])?;
    let seq = ewe::decode_seq(&buf[1..], seq_width)?;

    let hash_start = 1 + seq_width;
    let mut chunk_hash = [0u8; 32];
    chunk_hash.copy_from_slice(&buf[hash_start..hash_start + 32]);

    let payload = &buf[hash_start + 32..];
    Some((sid, seq, chunk_hash, payload))
}

// ---------------------------------------------------------------------------
// SPEC — transfer initiation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Spec {
    pub sid: StreamId,
    /// Total number of DATA packets in this transfer.
    pub count: u64,
    /// Payload size per packet (data only, excluding sid+seq+hash overhead).
    pub psize: u16,
    /// Total transfer size in bytes.
    pub total: u64,
    /// BLAKE3 hash of complete data.
    pub hash: [u8; 32],
}

impl Spec {
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        if buf.len() < 40 {
            return 0;
        }

        buf[pos] = TAG_SPEC;
        pos += 1;
        buf[pos] = self.sid.0;
        pos += 1;
        pos += ewe::encode_lean(&mut buf[pos..], self.count);
        pos += ewe::encode_lean(&mut buf[pos..], self.psize as u64);
        pos += ewe::encode_lean(&mut buf[pos..], self.total);

        if pos + 32 > buf.len() {
            return 0;
        }
        buf[pos..pos + 32].copy_from_slice(&self.hash);
        pos += 32;

        pos
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 2 || buf[0] != TAG_SPEC {
            return None;
        }

        let sid = StreamId::new(buf[1])?;
        let mut pos = 2;

        let (count, n) = ewe::decode_lean(&buf[pos..])?;
        pos += n;
        let (psize, n) = ewe::decode_lean(&buf[pos..])?;
        pos += n;
        let (total, n) = ewe::decode_lean(&buf[pos..])?;
        pos += n;

        if pos + 32 > buf.len() {
            return None;
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&buf[pos..pos + 32]);

        Some(Spec {
            sid,
            count,
            psize: psize as u16,
            total,
            hash,
        })
    }
}

// ---------------------------------------------------------------------------
// ACK — SPEC acknowledgment (seq=MAX means "SPEC ACK, ready to receive")
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Ack {
    pub sid: StreamId,
    pub seq: u64,
}

impl Ack {
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        if buf.len() < 4 {
            return 0;
        }

        buf[pos] = TAG_ACK;
        pos += 1;
        buf[pos] = self.sid.0;
        pos += 1;
        pos += ewe::encode_lean(&mut buf[pos..], self.seq);

        pos
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 2 || buf[0] != TAG_ACK {
            return None;
        }

        let sid = StreamId::new(buf[1])?;
        let (seq, _n) = ewe::decode_lean(&buf[2..])?;

        Some(Ack { sid, seq })
    }
}

// ---------------------------------------------------------------------------
// NAK — retransmit request (list of bad/missing sequences)
// ---------------------------------------------------------------------------

/// Maximum sequences in a single NAK packet.
pub const NAK_MAX_SEQS: usize = 32;

#[derive(Clone, Debug)]
pub struct Nak {
    pub sid: StreamId,
    pub seqs: [u64; NAK_MAX_SEQS],
    pub count: usize,
}

impl Nak {
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        if buf.len() < 4 {
            return 0;
        }

        buf[pos] = TAG_NAK;
        pos += 1;
        buf[pos] = self.sid.0;
        pos += 1;
        pos += ewe::encode_lean(&mut buf[pos..], self.count as u64);

        for i in 0..self.count {
            pos += ewe::encode_lean(&mut buf[pos..], self.seqs[i]);
        }

        pos
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 2 || buf[0] != TAG_NAK {
            return None;
        }

        let sid = StreamId::new(buf[1])?;
        let mut pos = 2;

        let (count_val, n) = ewe::decode_lean(&buf[pos..])?;
        pos += n;

        let count = count_val as usize;
        if count > NAK_MAX_SEQS {
            return None;
        }

        let mut seqs = [0u64; NAK_MAX_SEQS];
        for i in 0..count {
            let (seq, n) = ewe::decode_lean(&buf[pos..])?;
            pos += n;
            seqs[i] = seq;
        }

        Some(Nak { sid, seqs, count })
    }
}

// ---------------------------------------------------------------------------
// CONTROL — flow control
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlCmd {
    Pause = 0,
    Resume = 1,
    SlowDown = 2,
    Abort = 3,
}

impl ControlCmd {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Pause),
            1 => Some(Self::Resume),
            2 => Some(Self::SlowDown),
            3 => Some(Self::Abort),
            _ => None,
        }
    }
}

pub fn encode_control(buf: &mut [u8], sid: StreamId, cmd: ControlCmd) -> usize {
    if buf.len() < 3 {
        return 0;
    }
    buf[0] = TAG_CTRL;
    buf[1] = sid.0;
    buf[2] = cmd as u8;
    3
}

pub fn decode_control(buf: &[u8]) -> Option<(StreamId, ControlCmd)> {
    if buf.len() < 3 || buf[0] != TAG_CTRL {
        return None;
    }
    let sid = StreamId::new(buf[1])?;
    let cmd = ControlCmd::from_u8(buf[2])?;
    Some((sid, cmd))
}

// ---------------------------------------------------------------------------
// COMPLETE — transfer verification
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Complete {
    pub sid: StreamId,
    pub success: bool,
    /// BLAKE3 of reassembled data.
    pub data_hash: [u8; 32],
}

impl Complete {
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        if buf.len() < 35 {
            return 0;
        }
        let mut pos = 0;

        buf[pos] = TAG_DONE;
        pos += 1;
        buf[pos] = self.sid.0;
        pos += 1;
        buf[pos] = if self.success { 1 } else { 0 };
        pos += 1;
        buf[pos..pos + 32].copy_from_slice(&self.data_hash);
        pos += 32;

        pos
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 35 || buf[0] != TAG_DONE {
            return None;
        }

        let sid = StreamId::new(buf[1])?;
        let success = buf[2] != 0;

        let mut data_hash = [0u8; 32];
        data_hash.copy_from_slice(&buf[3..35]);

        Some(Complete {
            sid,
            success,
            data_hash,
        })
    }
}

// ---------------------------------------------------------------------------
// FIN — end-of-blast signal
// ---------------------------------------------------------------------------

pub fn encode_fin(buf: &mut [u8], sid: StreamId) -> usize {
    if buf.len() < 2 {
        return 0;
    }
    buf[0] = TAG_FIN;
    buf[1] = sid.0;
    2
}

pub fn decode_fin(buf: &[u8]) -> Option<StreamId> {
    if buf.len() < 2 || buf[0] != TAG_FIN {
        return None;
    }
    StreamId::new(buf[1])
}

// ---------------------------------------------------------------------------
// Top-level parse
// ---------------------------------------------------------------------------

/// Parse any PT packet from raw bytes. For DATA packets, `seq_width` must be provided (from active SPEC).
pub fn parse<'a>(buf: &'a [u8], seq_width: usize) -> Option<PacketKind<'a>> {
    if buf.is_empty() {
        return None;
    }

    let first = buf[0];

    // DATA packet: first byte 'a'-'z'
    if crate::is_data_packet(first) {
        let (sid, seq, chunk_hash, payload) = decode_data(buf, seq_width)?;
        return Some(PacketKind::Data {
            sid,
            seq,
            chunk_hash,
            payload,
        });
    }

    // Control packets: single uppercase tag byte
    match first {
        TAG_SPEC => Spec::decode(buf).map(PacketKind::Spec),
        TAG_ACK => Ack::decode(buf).map(PacketKind::Ack),
        TAG_NAK => Nak::decode(buf).map(PacketKind::Nak),
        TAG_CTRL => decode_control(buf).map(|(_, cmd)| PacketKind::Control(cmd)),
        TAG_DONE => Complete::decode(buf).map(PacketKind::Complete),
        TAG_FIN => decode_fin(buf).map(PacketKind::Fin),
        _ => None,
    }
}
