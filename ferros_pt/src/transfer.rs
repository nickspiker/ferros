//! Transfer state machines — inbound and outbound.
//!
//! No hardcoded limits. The protocol accepts any size transfer.
//! The caller provides buffers — if allocation fails, send NAK.
//! Runtime memory is the only constraint, not compile-time constants.
//!
//! ## USB transport mode (blast)
//!
//! USB bulk guarantees delivery. Sender blasts all DATA packets without
//! waiting for per-packet ACKs. Each DATA carries its own BLAKE3 hash.
//! Receiver verifies inline and responds only at the end:
//!   - COMPLETE(success) if root hash matches
//!   - NAK(bad_seqs) if any chunks failed hash verification
//!
//! Sender sends FIN after last DATA. If no response, retries FIN with
//! binary backoff starting at 1/256s (~4ms), doubling each attempt.

use crate::CHUNK_SIZE;
use crate::packet::{self, Ack, CHUNK_HASH_SIZE, Complete, NAK_MAX_SEQS, Nak, Spec, StreamId};
use ferros_ledger::ewe;

/// Transfer state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferState {
    /// No active transfer.
    Idle,
    /// SPEC received/sent, awaiting DATA/COMPLETE.
    Active,
    /// All chunks received/sent, awaiting final verification.
    Completing,
    /// Transfer done (success or failure).
    Done,
}

// ---------------------------------------------------------------------------
// Bitmap — tracks received/verified chunks, caller-provided storage
// ---------------------------------------------------------------------------

/// Bitmap word type — u64 for native 64-bit register ops, 64 chunks per word.
pub type BitmapWord = u64;
const BITS_PER_WORD: usize = 64;

/// Set a bit in a bitmap slice. Returns true if it was previously unset.
fn bitmap_set(bmap: &mut [BitmapWord], idx: usize) -> bool {
    let word = idx / BITS_PER_WORD;
    let bit = idx % BITS_PER_WORD;
    if word >= bmap.len() {
        return false;
    }
    let was_unset = (bmap[word] >> bit) & 1 == 0;
    bmap[word] |= 1 << bit;
    was_unset
}

/// Test a bit in a bitmap slice.
fn bitmap_test(bmap: &[BitmapWord], idx: usize) -> bool {
    let word = idx / BITS_PER_WORD;
    let bit = idx % BITS_PER_WORD;
    if word >= bmap.len() {
        return false;
    }
    (bmap[word] >> bit) & 1 != 0
}

/// Clear all bits in a bitmap slice.
fn bitmap_clear(bmap: &mut [BitmapWord]) {
    for w in bmap.iter_mut() {
        *w = 0;
    }
}

/// Number of words needed for `count` bits.
pub fn bitmap_words(count: u64) -> usize {
    ((count as usize) + BITS_PER_WORD - 1) / BITS_PER_WORD
}

/// Calculate the exact bitmap words needed for an outbound transfer of `data_len` bytes.
/// Use this to pre-allocate the bitmap buffer before calling `OutboundTransfer::start()`.
pub fn outbound_bitmap_words(data_len: usize) -> usize {
    if data_len == 0 {
        return bitmap_words(1);
    }
    // Compute psize iteratively (same logic as OutboundTransfer::start)
    let rough_psize = CHUNK_SIZE - 1 - 1 - CHUNK_HASH_SIZE;
    let rough_count = (data_len + rough_psize - 1) / rough_psize;
    let sw = ewe::seq_width(rough_count as u64);
    let psize = CHUNK_SIZE - 1 - sw - CHUNK_HASH_SIZE;
    let count = (data_len + psize - 1) / psize;
    let final_sw = ewe::seq_width(count as u64);
    let final_psize = CHUNK_SIZE - 1 - final_sw - CHUNK_HASH_SIZE;
    let final_count = (data_len + final_psize - 1) / final_psize;
    bitmap_words(final_count as u64)
}

// ---------------------------------------------------------------------------
// InboundTransfer — receiving DATA packets, assembling payload
// ---------------------------------------------------------------------------

/// Receive state for an inbound transfer.
///
/// Does NOT own the reassembly buffer or bitmap — caller provides them.
/// The protocol crate is limit-free. The device allocates from its heap
/// on SPEC arrival; if allocation fails, it sends NAK.
pub struct InboundTransfer<'a> {
    /// Reassembly buffer (caller-provided, length = spec.total).
    pub data: &'a mut [u8],
    /// Bitmap of received chunks (caller-provided).
    received: &'a mut [BitmapWord],
    /// How many bytes are valid in `data`.
    pub data_len: usize,
    /// Expected total bytes from SPEC.
    pub expected_total: u64,
    /// Expected chunk count from SPEC.
    pub expected_count: u64,
    /// Expected final hash from SPEC.
    pub expected_hash: [u8; 32],
    /// Payload size per chunk from SPEC.
    pub psize: u16,
    /// Sequence width (derived from count).
    pub seq_width: usize,
    /// Stream ID.
    pub sid: StreamId,
    /// Number of chunks received so far.
    pub chunks_received: u64,
    /// Number of chunks that failed hash verification.
    pub bad_chunks: u64,
    /// Current state.
    pub state: TransferState,
}

impl<'a> InboundTransfer<'a> {
    /// Create from a SPEC and caller-provided buffers.
    ///
    /// `data_buf` must be at least `spec.total` bytes.
    /// `bitmap_buf` must be at least `bitmap_words(spec.count)` u32s.
    ///
    /// Returns None if buffers are too small (caller should NAK).
    pub fn new(spec: &Spec, data_buf: &'a mut [u8], bitmap_buf: &'a mut [BitmapWord]) -> Option<Self> {
        let needed_data = spec.total as usize;
        let needed_bitmap = bitmap_words(spec.count);

        if data_buf.len() < needed_data {
            return None;
        }
        if bitmap_buf.len() < needed_bitmap {
            return None;
        }

        bitmap_clear(&mut bitmap_buf[..needed_bitmap]);

        Some(Self {
            data: data_buf,
            received: bitmap_buf,
            data_len: 0,
            expected_total: spec.total,
            expected_count: spec.count,
            expected_hash: spec.hash,
            psize: spec.psize,
            seq_width: ewe::seq_width(spec.count),
            sid: spec.sid,
            chunks_received: 0,
            bad_chunks: 0,
            state: TransferState::Active,
        })
    }

    /// Handle a received DATA chunk. Verifies per-chunk BLAKE3 hash.
    ///
    /// Returns true if chunk was accepted (new and hash valid).
    /// Bad hash chunks are counted but NOT stored — they'll be NAK'd.
    /// Sends nothing — receiver is silent during transfer.
    pub fn handle_data(&mut self, seq: u64, chunk_hash: &[u8; 32], payload: &[u8]) -> bool {
        if self.state != TransferState::Active {
            return false;
        }
        if seq >= self.expected_count {
            return false;
        }

        let seq_idx = seq as usize;

        // Skip duplicates
        if bitmap_test(self.received, seq_idx) {
            return true; // already have it
        }

        // Verify per-chunk BLAKE3 hash
        let computed = blake3::hash(payload);
        if computed.as_bytes() != chunk_hash {
            self.bad_chunks += 1;
            // Don't store bad data, don't mark in bitmap — will be NAK'd
            return false;
        }

        // Place chunk data at correct offset
        let offset = seq_idx * self.psize as usize;
        let end = (offset + payload.len()).min(self.data.len());
        let copy_len = end - offset;
        self.data[offset..end].copy_from_slice(&payload[..copy_len]);

        bitmap_set(self.received, seq_idx);
        self.chunks_received += 1;

        if end > self.data_len {
            self.data_len = end;
        }

        // Check if all chunks received
        if self.chunks_received == self.expected_count {
            self.state = TransferState::Completing;
        }

        true
    }

    /// Called when FIN is received or all chunks arrived.
    /// Returns a COMPLETE or NAK packet written to `buf`.
    ///
    /// - If all chunks present and root hash matches → COMPLETE(success)
    /// - If chunks missing or bad → NAK with their sequence numbers
    /// - If all chunks present but root hash fails → COMPLETE(fail)
    pub fn finish(&mut self, buf: &mut [u8]) -> usize {
        if self.state == TransferState::Done {
            return 0;
        }

        // Check for missing/bad chunks
        let mut missing = [0u64; NAK_MAX_SEQS];
        let mut missing_count = 0usize;

        for i in 0..self.expected_count as usize {
            if !bitmap_test(self.received, i) {
                if missing_count < NAK_MAX_SEQS {
                    missing[missing_count] = i as u64;
                    missing_count += 1;
                }
            }
        }

        if missing_count > 0 {
            // Missing chunks — send NAK
            let nak = Nak {
                sid: self.sid,
                seqs: missing,
                count: missing_count,
            };
            return nak.encode(buf);
        }

        // All chunks present — verify root hash
        self.data_len = self.expected_total as usize;
        let final_hash = blake3::hash(&self.data[..self.data_len]);
        let success = final_hash.as_bytes() == &self.expected_hash;

        let complete = Complete {
            sid: self.sid,
            success,
            data_hash: *final_hash.as_bytes(),
        };

        self.state = TransferState::Done;
        complete.encode(buf)
    }

    /// Get the reassembled data (only valid after successful completion).
    pub fn payload(&self) -> &[u8] {
        &self.data[..self.data_len]
    }

    /// Whether all expected chunks have been received.
    pub fn all_received(&self) -> bool {
        self.chunks_received == self.expected_count
    }

    /// Whether a particular chunk has been received.
    pub fn has_chunk(&self, seq: u64) -> bool {
        bitmap_test(self.received, seq as usize)
    }
}

// ---------------------------------------------------------------------------
// OutboundTransfer — sending DATA packets from a source buffer
// ---------------------------------------------------------------------------

/// Outbound transfer state. Source data is borrowed from the caller.
pub struct OutboundTransfer<'a> {
    /// Bitmap for tracking retransmit requests (caller-provided).
    retransmit: &'a mut [BitmapWord],
    /// Stream ID.
    pub sid: StreamId,
    /// Total data bytes.
    pub total: u64,
    /// Chunk count.
    pub count: u64,
    /// Payload per chunk.
    pub psize: u16,
    /// Sequence width.
    pub seq_width: usize,
    /// BLAKE3 of complete data.
    pub data_hash: [u8; 32],
    /// Next sequence to send in initial blast.
    pub next_seq: u64,
    /// Current state.
    pub state: TransferState,
}

impl<'a> OutboundTransfer<'a> {
    /// Begin a new outbound transfer. Computes BLAKE3 and builds SPEC.
    ///
    /// `bitmap_buf` must be at least `bitmap_words(chunk_count)` u32s.
    /// Returns (OutboundTransfer, SPEC packet length written to `spec_buf`).
    pub fn start(
        sid: StreamId,
        data: &[u8],
        bitmap_buf: &'a mut [BitmapWord],
        spec_buf: &mut [u8],
    ) -> Option<(Self, usize)> {
        // Compute seq_width and psize iteratively.
        // DATA overhead: 1 (sid) + seq_width + 32 (blake3 hash)
        let rough_psize = CHUNK_SIZE - 1 - 1 - CHUNK_HASH_SIZE;
        let rough_count = if data.is_empty() {
            1
        } else {
            (data.len() + rough_psize - 1) / rough_psize
        };
        let sw = ewe::seq_width(rough_count as u64);
        let psize = CHUNK_SIZE - 1 - sw - CHUNK_HASH_SIZE;
        let count = if data.is_empty() {
            1
        } else {
            (data.len() + psize - 1) / psize
        };

        // Refine (seq_width might change with actual count)
        let final_sw = ewe::seq_width(count as u64);
        let final_psize = CHUNK_SIZE - 1 - final_sw - CHUNK_HASH_SIZE;
        let final_count = if data.is_empty() {
            1
        } else {
            (data.len() + final_psize - 1) / final_psize
        };

        let needed_bitmap = bitmap_words(final_count as u64);
        if bitmap_buf.len() < needed_bitmap {
            return None;
        }
        bitmap_clear(&mut bitmap_buf[..needed_bitmap]);

        let hash = blake3::hash(data);

        let spec = Spec {
            sid,
            count: final_count as u64,
            psize: final_psize as u16,
            total: data.len() as u64,
            hash: *hash.as_bytes(),
        };
        let spec_len = spec.encode(spec_buf);

        let xfer = Self {
            retransmit: bitmap_buf,
            sid,
            total: data.len() as u64,
            count: final_count as u64,
            psize: final_psize as u16,
            seq_width: final_sw,
            data_hash: *hash.as_bytes(),
            next_seq: 0,
            state: TransferState::Active,
        };

        Some((xfer, spec_len))
    }

    /// Begin a new outbound transfer with automatic bitmap allocation.
    /// The Vec is resized to exactly the needed capacity.
    #[cfg(feature = "alloc")]
    pub fn start_vec(
        sid: StreamId,
        data: &[u8],
        bitmap_vec: &'a mut alloc::vec::Vec<BitmapWord>,
        spec_buf: &mut [u8],
    ) -> Option<(Self, usize)> {
        let needed = outbound_bitmap_words(data.len());
        bitmap_vec.clear();
        bitmap_vec.resize(needed, 0);
        let bmap = unsafe {
            core::slice::from_raw_parts_mut(bitmap_vec.as_mut_ptr(), bitmap_vec.len())
        };
        Self::start(sid, data, bmap, spec_buf)
    }

    /// Encode the next DATA packet from source data (initial blast).
    /// Returns bytes written to `pkt_buf`, or 0 if all sent.
    pub fn next_data_packet(&mut self, src: &[u8], pkt_buf: &mut [u8]) -> usize {
        if self.state != TransferState::Active {
            return 0;
        }
        if self.next_seq >= self.count {
            return 0;
        }

        let seq = self.next_seq;
        let offset = seq as usize * self.psize as usize;
        let end = (offset + self.psize as usize).min(src.len());
        if offset >= src.len() {
            return 0;
        }
        let payload = &src[offset..end];

        let n = packet::encode_data(pkt_buf, self.sid, seq, self.seq_width, payload);
        if n > 0 {
            self.next_seq += 1;
        }
        n
    }

    /// Encode a specific DATA packet for retransmission.
    /// Returns bytes written, or 0 on error.
    pub fn retransmit_packet(&self, seq: u64, src: &[u8], pkt_buf: &mut [u8]) -> usize {
        if seq >= self.count {
            return 0;
        }

        let offset = seq as usize * self.psize as usize;
        let end = (offset + self.psize as usize).min(src.len());
        if offset >= src.len() {
            return 0;
        }
        let payload = &src[offset..end];

        packet::encode_data(pkt_buf, self.sid, seq, self.seq_width, payload)
    }

    /// Encode a FIN packet.
    pub fn encode_fin(&self, buf: &mut [u8]) -> usize {
        packet::encode_fin(buf, self.sid)
    }

    /// Handle received COMPLETE. Verifies hash matches.
    pub fn handle_complete(&mut self, complete: &Complete) -> bool {
        self.state = TransferState::Done;
        complete.success && complete.data_hash == self.data_hash
    }

    /// Handle received NAK — returns the list of sequences to retransmit.
    pub fn handle_nak<'b>(&mut self, nak: &'b Nak) -> &'b [u64] {
        // State stays Active for retransmit
        &nak.seqs[..nak.count]
    }

    /// Whether all initial DATA packets have been sent.
    pub fn all_sent(&self) -> bool {
        self.next_seq >= self.count
    }
}
