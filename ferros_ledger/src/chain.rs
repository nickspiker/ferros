//! In-memory hash chain — one per category.
//!
//! Maintains chain head (prev_hash), category sequence counter, and a ring buffer of recent entry hashes for verification.

use crate::category::Category;
use crate::entry::{self, Entry, Hash};
use crate::event::Event;

/// Number of recent entry hashes to keep per chain (for walkback).
const HASH_RING_SIZE: usize = 64;

/// Per-category chain state.
pub struct CategoryChain {
    pub category: Category,
    /// Current chain head hash (prev_hash for next entry).
    pub head: Hash,
    /// Category-local sequence counter.
    pub cat_seq: u64,
    /// Ring of recent entry hashes.
    pub recent_hashes: [Hash; HASH_RING_SIZE],
    /// Write position in the ring.
    pub ring_pos: usize,
    /// Total entries appended to this chain.
    pub total_entries: u64,
}

impl CategoryChain {
    /// Create a new chain starting from genesis.
    pub const fn new(category: Category) -> Self {
        Self {
            category,
            head: [0u8; 32], // will be set to genesis_prev_hash on first use
            cat_seq: 0,
            recent_hashes: [[0u8; 32]; HASH_RING_SIZE],
            ring_pos: 0,
            total_entries: 0,
        }
    }

    /// Initialize with the genesis prev_hash.
    pub fn init_genesis(&mut self) {
        self.head = entry::genesis_prev_hash();
    }

    /// Append an event to this chain. Returns the entry if successful.
    pub fn append(
        &mut self,
        global_seq: u64,
        event: &Event,
        cap_hash: &Hash,
        sig: &Hash,
    ) -> Option<Entry> {
        let entry = entry::build_entry(
            self.category,
            global_seq,
            self.cat_seq,
            &self.head,
            event,
            cap_hash,
            sig,
        )?;

        // Update chain state
        self.head = entry.hash;
        self.recent_hashes[self.ring_pos] = entry.hash;
        self.ring_pos = (self.ring_pos + 1) % HASH_RING_SIZE;
        self.cat_seq += 1;
        self.total_entries += 1;

        Some(entry)
    }
}

/// The Chain — manages all category chains and the global sequence.
///
/// This is the ledger daemon's core state. Fixed-size, no alloc.
pub struct Chain {
    /// Per-category chains. Indexed by Category as u8. We use a flat array — category enum values are the indices.
    chains: [CategoryChain; Chain::NUM_CATEGORIES],
    /// Global sequence counter (across all categories).
    global_seq: u64,
    /// Total entries across all chains.
    total_entries: u64,
    /// Whether genesis has been established.
    initialized: bool,
}

impl Chain {
    /// Number of categories we support.
    const NUM_CATEGORIES: usize = 19;

    fn category_index(cat: Category) -> usize {
        match cat {
            Category::KernelBoot => 0,
            Category::KernelIpc => 1,
            Category::KernelMemory => 2,
            Category::KernelCapability => 3,
            Category::KernelKill => 4,
            Category::UsbPhy => 5,
            Category::UsbDwc3 => 6,
            Category::UsbEnumeration => 7,
            Category::UsbPt => 8,
            Category::PhotonTransport => 9,
            Category::PhotonToken => 10,
            Category::PhotonMessages => 11,
            Category::TokenAttestation => 12,
            Category::TokenAuth => 13,
            Category::VaultBoot => 14,
            Category::VaultWrite => 15,
            Category::VaultRepair => 16,
            Category::Vsf => 17,
        }
    }

    /// Create a new uninitialized chain set.
    pub const fn new() -> Self {
        Self {
            chains: [
                CategoryChain::new(Category::KernelBoot),
                CategoryChain::new(Category::KernelIpc),
                CategoryChain::new(Category::KernelMemory),
                CategoryChain::new(Category::KernelCapability),
                CategoryChain::new(Category::KernelKill),
                CategoryChain::new(Category::UsbPhy),
                CategoryChain::new(Category::UsbDwc3),
                CategoryChain::new(Category::UsbEnumeration),
                CategoryChain::new(Category::UsbPt),
                CategoryChain::new(Category::PhotonTransport),
                CategoryChain::new(Category::PhotonToken),
                CategoryChain::new(Category::PhotonMessages),
                CategoryChain::new(Category::TokenAttestation),
                CategoryChain::new(Category::TokenAuth),
                CategoryChain::new(Category::VaultBoot),
                CategoryChain::new(Category::VaultWrite),
                CategoryChain::new(Category::VaultRepair),
                CategoryChain::new(Category::Vsf),
                CategoryChain::new(Category::KernelBoot), // unused padding
            ],
            global_seq: 0,
            total_entries: 0,
            initialized: false,
        }
    }

    /// Initialize all chains with genesis prev_hash.
    pub fn init(&mut self) {
        for chain in self.chains.iter_mut() {
            chain.init_genesis();
        }
        self.initialized = true;
    }

    /// Post an event to the ledger. Routes to the correct category chain.
    ///
    /// Returns the entry's provenance hash, or None if encoding failed.
    pub fn post(&mut self, event: &Event) -> Option<Hash> {
        if !self.initialized {
            return None;
        }

        let cat = event.category();
        let idx = Self::category_index(cat);

        // Placeholder cap_hash and sig (zeros until cap system is live)
        let cap_hash = [0u8; 32];
        let sig = [0u8; 32];

        let entry = self.chains[idx].append(self.global_seq, event, &cap_hash, &sig)?;

        self.global_seq += 1;
        self.total_entries += 1;

        Some(entry.hash)
    }

    /// Global sequence counter.
    pub fn global_seq(&self) -> u64 {
        self.global_seq
    }

    /// Total entries across all chains.
    pub fn total_entries(&self) -> u64 {
        self.total_entries
    }

    /// Get chain state for a category.
    pub fn chain(&self, cat: Category) -> &CategoryChain {
        &self.chains[Self::category_index(cat)]
    }

    /// Whether the chain has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}
