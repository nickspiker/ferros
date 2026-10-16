//! Photon's core without std, for a ferros process whose only I/O is UDP 4383 (WIFI.md; Claude memories photon_on_ferros_no_bootstrap). Vocabulary: a *photon* is a message, a *beam* is a video call.
//!
//! Modules follow the Photon sources they mirror (photon/src/network/fgtw/protocol.rs, pt/packets.rs, crypto/chain.rs, types/friendship.rs); byte compatibility with the stock app is the contract.

#![no_std]

extern crate alloc;

pub mod chain;
pub mod wave;
pub mod wire;
