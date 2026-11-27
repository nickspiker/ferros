//! The pre-boot buffer (BOOT.md "Pre-Boot Log Buffer", LEDGER.md "Boot sequence"): system entries made before the ledger can seal them, held in RAM and written, sealed and in order, the moment it can.
//!
//! Nothing reaches the flash unsealed (owner, 2026-11-17), and the seal needs the ira, which the kernel derives after the UFS link is up: what the kernel says before then waits here.
//! A fixed ring of `N` slots in the kernel's static memory: when it is full the oldest is overwritten and counted, and the count is written as the ledger's own entry when the buffer is drained.
//! A boot that dies before the drain is covered by the glass console and the RAM log (ramoops), not the flash log.
//! Personal entries never wait here: one made while nobody is present is dropped (LEDGER.md "Classes").

use alloc::string::String;

use crate::category::Category;

/// One entry waiting: what it says and when it was said (the counter's tick, kept only in RAM: the entry's Eagle time is worked out from it when it is written, if the clock has been read by then).
pub struct Pending {
    pub tick: u64,
    pub category: Category,
    pub writer: String,
    pub words: String,
    /// A death's typed fields, when the entry is one.
    pub death: Option<crate::death::Death>,
}

/// The buffer: `N` slots, a power of two.
pub struct Preboot<const N: usize> {
    slots: [Option<Pending>; N],
    /// The next slot to fill.
    next: usize,
    /// How many slots hold an entry.
    held: usize,
    /// Entries overwritten before they could be written.
    lost: u64,
}

impl<const N: usize> Preboot<N> {
    pub const fn new() -> Self {
        Preboot { slots: [const { None }; N], next: 0, held: 0, lost: 0 }
    }

    /// Hold an entry; the oldest goes when the buffer is full.
    pub fn push(&mut self, p: Pending) {
        if self.slots[self.next].is_some() {
            self.lost += 1;
        } else {
            self.held += 1;
        }
        self.slots[self.next] = Some(p);
        self.next = (self.next + 1) % N;
    }

    pub fn len(&self) -> usize {
        self.held
    }

    pub fn is_empty(&self) -> bool {
        self.held == 0
    }

    /// Hand every held entry to `f`, oldest first, and empty the buffer.
    /// Returns how many were overwritten before this drain.
    pub fn drain(&mut self, mut f: impl FnMut(Pending)) -> u64 {
        // The oldest is at `next` once the ring has wrapped, else at slot 0 (empty slots are passed over either way).
        for k in 0..N {
            if let Some(p) = self.slots[(self.next + k) % N].take() {
                f(p);
            }
        }
        self.next = 0;
        self.held = 0;
        let lost = self.lost;
        self.lost = 0;
        lost
    }
}

impl<const N: usize> Default for Preboot<N> {
    fn default() -> Self {
        Self::new()
    }
}
