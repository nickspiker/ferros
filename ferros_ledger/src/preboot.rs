//! Pre-boot buffer — fixed ring in BSS for early kernel events.
//!
//! Before the ledger daemon is initialized, events are buffered here.
//! Raw serialized entries, unchained (no prev_hash linking).
//! On ledger init, the buffer is flushed into the chain as entries 0..N.
//!
//! The buffer is a fixed-size ring: overwrites oldest on full.
//! No alloc, no heap — pure BSS.

use crate::event::Event;

/// Max events in the pre-boot buffer.
pub const PREBOOT_CAPACITY: usize = 64;

/// A buffered pre-boot event (just the event, chain linking happens at flush).
pub struct PrebootBuffer {
    events: [Option<Event>; PREBOOT_CAPACITY],
    /// Write position (next slot to write).
    write_pos: usize,
    /// Number of events stored (saturates at PREBOOT_CAPACITY).
    count: usize,
    /// Total events ever written (including overwritten).
    total_written: u64,
}

impl PrebootBuffer {
    /// Create an empty buffer. Must be const for static init.
    pub const fn new() -> Self {
        // Can't use [None; N] for non-Copy Option<Event>, so we do this:
        Self {
            events: [
                None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None,
            ],
            write_pos: 0,
            count: 0,
            total_written: 0,
        }
    }

    /// Buffer an event for later chain insertion.
    pub fn push(&mut self, event: Event) {
        self.events[self.write_pos] = Some(event);
        self.write_pos = (self.write_pos + 1) % PREBOOT_CAPACITY;
        if self.count < PREBOOT_CAPACITY {
            self.count += 1;
        }
        self.total_written += 1;
    }

    /// Number of events currently buffered.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether any events were overwritten (ring wrapped).
    pub fn overflowed(&self) -> bool {
        self.total_written > PREBOOT_CAPACITY as u64
    }

    /// Total events ever pushed (including overwritten).
    pub fn total_written(&self) -> u64 {
        self.total_written
    }

    /// Drain all buffered events in order (oldest first).
    /// Returns events as an iterator-like callback pattern (no alloc).
    ///
    /// After drain, the buffer is empty.
    pub fn drain(&mut self, mut f: impl FnMut(Event)) {
        if self.count == 0 { return; }

        // Start position: if ring wrapped, start at write_pos (oldest surviving).
        // If not wrapped, start at 0.
        let start = if self.count == PREBOOT_CAPACITY {
            self.write_pos // oldest surviving entry
        } else {
            0
        };

        for i in 0..self.count {
            let idx = (start + i) % PREBOOT_CAPACITY;
            if let Some(event) = self.events[idx].take() {
                f(event);
            }
        }

        self.count = 0;
        self.write_pos = 0;
    }
}
