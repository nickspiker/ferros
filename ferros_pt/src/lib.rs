//! # Photon Transport (PT)
//!
//! Reliable chunked transfer for VSF payloads over USB bulk endpoints.
//! Adapted from Photon's UDP PT — same protocol, 512B chunks (USB HS MPS).
//!
//! `no_std`, no alloc. No hardcoded size limits — the protocol accepts
//! any transfer size. Callers provide buffers; if allocation fails, NAK.

#![no_std]

pub mod command;
pub mod packet;
pub mod transfer;

pub use command::{Command, Op};
pub use packet::{PacketKind, StreamId};
pub use transfer::{InboundTransfer, OutboundTransfer, TransferState};

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
