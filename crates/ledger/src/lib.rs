//! # ferros ledger
//!
//! The ledger (docs/LEDGER.md): every entry a complete VSF document, its words sealed, each one chained to the one before by BLAKE3.
//!
//! `ewe` is the EWE integer helpers PT and the map take on their own; everything else is behind the `ring` feature (alloc, vsf, the seal):
//! - `category`: the categories and the class each one is sealed under (system or personal);
//! - `entry`: one entry, built, verified and opened;
//! - `death`: a process's end or the kernel's as typed fields (message, place, call chain), carried in the entry and in what `diag log` answers;
//! - `ring`: the flash ring the kernel keeps it in, its head found by the chain, a torn last write skipped;
//! - `preboot`: the RAM buffer that holds entries made before the ledger can seal them;
//! - `view`: the documents `diag log` answers with (an entry opened, a finding, the ring's state) and their render.

#![no_std]

#[cfg(feature = "ring")]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod ewe;

#[cfg(feature = "ring")]
pub mod category;
#[cfg(feature = "ring")]
pub mod death;
#[cfg(feature = "ring")]
pub mod entry;
#[cfg(feature = "ring")]
pub mod preboot;
#[cfg(feature = "ring")]
pub mod ring;
#[cfg(feature = "ring")]
pub mod view;

#[cfg(all(test, feature = "ring"))]
mod tests;

#[cfg(feature = "ring")]
pub use category::{Category, Class};
#[cfg(feature = "ring")]
pub use entry::{Payload, SealKey, Stored};
#[cfg(feature = "ring")]
pub use ring::{Blocks, Finding, Item, Ring, Why};
