//! The kernel core's idle while dark (POWER.md "Deep idle", 2026-11-13: the owner lost half the battery overnight with the screen off).
//!
//! Tickless: with the screen off and no worker up, the service loop sleeps in `wfi` until the next thing due (a thread's deadline, its own quarter-second duties), not on the event stream every 2.7 ms and the 2 ms backstop.
//! The core's power-down (PSCI c2) and the keeper that holds the keys meanwhile are on the deep-sleep branch until EL3 accepts ferros's CPU_SUSPEND (POWER.md "c2 is refused by EL3").

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// Tickless while dark (`diag idle tickless on|off`).
pub static TICKLESS: AtomicBool = AtomicBool::new(true);
/// The dark loop's own period: the watchdog wants a pet within its second, the keys' check and the clock, gauge and thermal polls are a quarter second or slower.
pub const TICK_SHIFT: u32 = 2;
/// The dark loop's sleeps, for `diag idle`.
pub static SHALLOW: AtomicU64 = AtomicU64::new(0);

/// `diag idle [tickless on|off]`.
pub fn diag(params: &str, out: &mut alloc::vec::Vec<u8>) {
    if params == "tickless on" || params == "tickless off" {
        TICKLESS.store(params.ends_with("on"), Relaxed);
    }
    out.extend_from_slice(alloc::format!("tickless {}, dark sleeps A#{}\n", TICKLESS.load(Relaxed), SHALLOW.load(Relaxed)).as_bytes());
}
