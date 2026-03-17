// FERROS PT SOURCE MAP — keep updated when pub items or files change
//
// lib.rs ── constants, packet dispatch helpers, module re-exports
//   CHUNK_SIZE=512, MAX_STREAMS=26
//   is_data_packet(byte) — 'a'-'z' = DATA stream
//   is_control_packet(byte) — S/A/N/C/D/F = control tags
//
// packet.rs ── packet encode/decode (lean format, no VSF_MAGIC)
//   struct StreamId(u8) — 'a'-'z' stream identifier
//   struct Spec { sid, count, psize, total, data_hash }
//     ::encode(buf), decode(buf)
//   struct Ack { sid, seq }
//     ::encode_spec_ack(buf, sid), decode(buf)
//   struct Nak { sid, count, seqs }
//     ::encode(buf), decode(buf)
//   struct Complete { sid, success, data_hash }
//     ::encode(buf), decode(buf)
//   encode_data(buf, sid, seq, seq_width, payload) → usize
//   encode_fin(buf, sid) → usize
//   decode_data_header(buf) → (sid, seq, hash, payload_offset)
//   CHUNK_HASH_SIZE=32, TAG_SPEC='S', TAG_ACK='A', TAG_NAK='N',
//   TAG_COMPLETE='D', TAG_FIN='F'
//
// transfer.rs ── inbound/outbound transfer state machines
//   type BitmapWord = u64
//   bitmap_words(count) → usize
//
//   struct InboundTransfer<'a> { received bitmap, data buffer }
//     ::new(spec, data_buf, bitmap_buf) → Option<Self>
//     ::handle_data(buf, data_buf) → bool
//     ::all_received() → bool
//     ::finish(complete_buf) → usize — BLAKE3 verify + encode COMPLETE
//     ::bad_chunks() → count
//
//   struct OutboundTransfer<'a> { retransmit bitmap, source data }
//     ::start(sid, data, psize, bitmap_buf) → Self
//     ::start_vec(sid, data, psize, bitmap_vec) → Self  [alloc feature]
//     ::encode_spec(buf) → usize
//     ::next_data_packet(src, buf) → usize
//     ::all_sent() → bool
//     ::encode_fin(buf) → usize
//     ::handle_complete(complete) → bool
//     ::handle_nak(nak) → &[u64]
//     outbound_bitmap_words(data_len, psize) → usize
//
// command.rs ── cap-addressed command protocol
//   struct Command { cap, op, params }
//   enum Op { DiagRead, MemRead, MemWrite, Reboot, ReloadWrite, ReloadExec }
//   mod caps { DIAG, MEM, REBOOT, RELOAD }
//   dev_cap(name) → [u8; 32]  (BLAKE3 of cap name)

//! # Photon Transport (PT)
//!
//! Reliable chunked transfer for VSF payloads over USB bulk endpoints.
//! `no_std`, no alloc. No hardcoded size limits — the protocol accepts
//! any transfer size. Callers provide buffers; if allocation fails, NAK.

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod command;
pub mod packet;
pub mod transfer;

pub use command::{Command, Op};
pub use packet::{PacketKind, StreamId};
pub use transfer::{BitmapWord, InboundTransfer, OutboundTransfer, TransferState};

/// Maximum chunk payload size (USB HS bulk MPS).
pub const CHUNK_SIZE: usize = 512;

/// Maximum number of concurrent streams.
pub const MAX_STREAMS: usize = 26;

/// Returns true if the first byte indicates a DATA packet (stream_id 'a'-'z').
#[inline]
pub fn is_data_packet(first_byte: u8) -> bool {
    first_byte >= b'a' && first_byte <= b'z'
}

/// Returns true if the first byte indicates a control packet (uppercase tag).
#[inline]
pub fn is_control_packet(first_byte: u8) -> bool {
    matches!(first_byte, b'S' | b'A' | b'N' | b'C' | b'D' | b'F')
}
