//! The futex std's locks are built on: the kernel's WAIT/WAKE on a 64-bit word of the arc (KERNEL.md "Threads"). A wait blocks while the word still holds `expected`, until a wake, a timeout, or — spuriously — anything else; std tolerates spurious wakes.

use core::sync::atomic::AtomicU64;
use core::time::Duration;

use crate::sys::{self, ERR, nr, off};

/// The kernel compares the whole 64-bit word, so both futex widths are that word.
pub type Futex = AtomicU64;
pub type Primitive = u64;
pub type SmallFutex = AtomicU64;
pub type SmallPrimitive = u64;

/// Returns false only on timeout.
pub fn futex_wait(futex: &AtomicU64, expected: u64, timeout: Option<Duration>) -> bool {
    let deadline = match timeout {
        None => 0,
        Some(d) => sys::now().saturating_add(crate::time::ticks(d)).max(1),
    };
    sys::svc(nr::WAIT, off(futex), expected, deadline, 0, 0) != 2
}

/// Wake one waiter; true when there was one.
#[inline]
pub fn futex_wake(futex: &AtomicU64) -> bool {
    let n = sys::svc(nr::WAKE, off(futex), 1, 0, 0, 0);
    n != ERR && n > 0
}

#[inline]
pub fn futex_wake_all(futex: &AtomicU64) {
    sys::svc(nr::WAKE, off(futex), u64::MAX, 0, 0, 0);
}
