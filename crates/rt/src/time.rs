//! Time from the generic counter (monotonic) and the kernel's wall-clock base in the preferences block (UTC, once the kernel has read the PMIC's clock).

use core::time::Duration;

use crate::sys;

const NANOS: u128 = 1_000_000_000;

/// The counter's rate.
#[inline]
pub fn hz() -> u64 {
    sys::freq()
}

/// Counter ticks now.
#[inline]
pub fn now() -> u64 {
    sys::now()
}

/// A duration in counter ticks, saturating.
pub fn ticks(d: Duration) -> u64 {
    let hz = hz() as u128;
    let t = d.as_secs() as u128 * hz + d.subsec_nanos() as u128 * hz / NANOS;
    t.min(u64::MAX as u128) as u64
}

/// Counter ticks as a duration.
pub fn duration(t: u64) -> Duration {
    let hz = hz();
    Duration::new(t / hz, ((t % hz) as u128 * NANOS / hz as u128) as u32)
}

/// Park this thread until the counter reaches `deadline` (a deadline already passed is a yield).
pub fn sleep_until(deadline: u64) {
    sys::svc(sys::nr::SLEEP, deadline, 0, 0, 0, 0);
}

/// UTC now as (seconds since 1970, nanoseconds into the second), None until the kernel knows the wall clock. The base sits in the preferences block at arc offset G#FC0 + 16 (the kernel's PREF_WALL): base + counter = UTC ticks.
pub fn wall() -> Option<(u64, u32)> {
    let at = sys::arc_base() + 0xFC0 + 16;
    let base = unsafe { core::ptr::read_volatile(at as *const u64) };
    if base == 0 {
        return None;
    }
    let hz = hz();
    let t = base.wrapping_add(now());
    Some((t / hz, ((t % hz) as u128 * NANOS / hz as u128) as u32))
}
