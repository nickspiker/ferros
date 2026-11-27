//! ferros_rt's TCP client on the host (x86_64 Linux): the runtime's own sources by path, not copies, so what is tested here is what a std app links on the device.
//! `sys` is a fake kernel whose CALL goes to `ip`, a fake ip server: the ops a client sends, held and answered as payloads/ip documents them, with the test playing the peer.
//!
//! cargo test

pub mod ip;
pub mod sys;

#[path = "../../rt-heap/src/futex.rs"]
pub mod futex;

#[path = "../../../crates/rt/src/lock.rs"]
pub mod lock;

#[path = "../../../crates/rt/src/time.rs"]
pub mod time;

#[path = "../../../crates/rt/src/tcp.rs"]
pub mod tcp;

/// A grant's token offset: the fake kernel takes the slot itself.
pub fn cap(slot: u64) -> u64 {
    slot
}
