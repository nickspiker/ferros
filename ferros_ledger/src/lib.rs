//! # Ferros Ledger
//!
//! Append-only event chain. Every entry is a BLAKE3-chained record with
//! identity, ordering, and structured payload sections.
//!
//! `no_std`, no alloc. Fixed-capacity chains backed by static buffers.
//! The kernel is the ledger daemon — subsystems get handles with
//! category-scoped capabilities.

#![no_std]

pub mod ewe;
pub mod category;
pub mod entry;
pub mod chain;
pub mod event;
pub mod preboot;

pub use category::Category;
pub use chain::Chain;
pub use entry::Entry;
pub use event::Event;
