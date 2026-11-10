//! The wall clock: the main PMIC's real-time clock (s2mpg14, its RTC block at I2C address G#02 behind ACPM; rtc-s2mpg14.c). Binary, 24-hour (the bootloader and Android leave it so: RTC_CTRL bit MODEL24), counting UTC as Android keeps it. A read pulses the update register's read bit (RUDR, bit 0: clear, 50 µs, set, 1 ms) so the counters are latched into SEC..YEAR (G#06-G#0C), then reads those seven.
//!
//! The kernel reads it at service start and every hour after, and keeps the result as a base in counter ticks: unix seconds × frequency − the counter at the read, so a process gets wall time from its own counter with no syscall (SYS_FACTS slot 8; slot 9 the time zone, the settings document's `tz`).

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

const RTC: u32 = 0x02;
const UPDATE: u32 = 0x01;
const SEC: u32 = 0x06;

/// Unix seconds × cntfrq − cntpct at the last read; 0 = never read.
pub static BASE: AtomicU64 = AtomicU64::new(0);
/// When the base was taken (cntpct), for the hourly re-read.
pub static READ_AT: AtomicU64 = AtomicU64::new(0);
/// The local time zone, seconds east of UTC (the settings document's `tz`, settings.rs; absent = 0).
pub static TZ: AtomicI64 = AtomicI64::new(0);

/// A new manual offset (used when no zone is set, or its rule is not found): kept and handed to every process with the base.
pub fn set_tz(secs: i64) {
    TZ.store(secs, Ordering::Relaxed);
    crate::process::set_wall(BASE.load(Ordering::Relaxed), packed());
}
/// The same with the kernel lock held (the store command runs under it).
pub fn set_tz_held(secs: i64) {
    TZ.store(secs, Ordering::Relaxed);
    crate::process::set_wall_held(BASE.load(Ordering::Relaxed), packed());
}

/// The zone in effect (owner, 2026-11-10: "arabic needs to match wall clock with DST legal, dozenal stays"; "Proper zone too"): its IANA id as zone.rs chose it, its rule once read from zone.rs's table (the `zones` document, tools/tzvsf), and whether that read is still to do (`resolve_zone`, from the service loop).
static mut ZONE_ID: ([u8; 64], usize) = ([0; 64], 0);
static mut RULE: Option<ferros_tz::Rule> = None;
static ZONE_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The next instant (Unix seconds) the legal offset changes, 0 for none: the deadline `poll` acts on.
static NEXT: AtomicI64 = AtomicI64::new(0);

/// The zone's id as set, empty for none.
pub fn zone_id() -> &'static [u8] {
    unsafe {
        let z = &*(&raw const ZONE_ID);
        &z.0[..z.1]
    }
}

/// The zone in effect set by its id (zone.rs `apply`; empty: none, the fixed offset): kept, and its rule read at the service loop's next pass. An id longer than the slot is not a zone tzdata has (the longest is 30 bytes).
pub fn set_zone(id: &[u8]) -> bool {
    // WHY the bounds: the slot holds 64 bytes, and the id reaches here from the settings document and the processes (zone.rs checks it the same way first).
    if id.len() > 64 || !id.iter().all(|&b| b.is_ascii_graphic()) {
        return false;
    }
    unsafe {
        let z = &mut *(&raw mut ZONE_ID);
        z.0[..id.len()].copy_from_slice(id);
        z.1 = id.len();
        RULE = None;
    }
    ZONE_PENDING.store(true, Ordering::Release);
    true
}

/// Unix seconds now, from the base; None until the RTC has been read.
pub fn utc_now() -> Option<i64> {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        return None;
    }
    let (f, now) = (crate::platform::cntfrq(), crate::platform::cntpct());
    Some((base.wrapping_add(now) / f) as i64)
}

/// The two offsets in the processes' one wall slot (preferences bytes 24-31; the page has no room for another: the ihi follows at 32): the standard offset in the low 32 bits, the legal one (daylight time while in effect) in the high 32, each seconds east of UTC. With no zone's rule, both are the manual offset. Sets `NEXT` to the legal offset's next change.
fn packed() -> i64 {
    let (s, l) = match (unsafe { RULE }, utc_now()) {
        (Some(r), Some(t)) => {
            let n = r.at(t);
            NEXT.store(n.next.unwrap_or(0), Ordering::Relaxed);
            (n.standard, n.legal)
        }
        _ => {
            NEXT.store(0, Ordering::Relaxed);
            let z = TZ.load(Ordering::Relaxed) as i32;
            (z, z)
        }
    };
    (s as u32 as u64 | (l as u32 as u64) << 32) as i64
}

/// The zone's rule read again at the next pass (zone.rs read the `zones` document anew).
pub fn zone_again() {
    ZONE_PENDING.store(true, Ordering::Release);
}

/// From the service loop, after zone.rs's step: a zone set and not yet read gets its rule from the zones table (zone.rs, the vault's `zones` document), and every process its offsets. No zone: the fixed offset.
pub fn resolve_zone() {
    if !ZONE_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let id = zone_id();
    let rule = crate::zone::rule_of(id);
    unsafe { RULE = rule };
    if !id.is_empty() {
        crate::flashlog::line(&alloc::format!("zone {}: {}", core::str::from_utf8(id).unwrap_or("?"), match (crate::zone::table_read(), rule.is_some()) {
            (_, true) => "its rule read",
            (true, false) => "not in the zones document; the fixed offset stands",
            (false, false) => "no zones document read; the fixed offset stands",
        }));
    }
    crate::process::set_wall(BASE.load(Ordering::Relaxed), packed());
}

/// The RTC's fields: (year, month 1-12, day 1-31, hour, minute, second).
pub fn read() -> Result<(u32, u32, u32, u32, u32, u32), u32> {
    let m = crate::acpm::MAIN;
    let upd = crate::acpm::read(m, RTC, UPDATE)?;
    crate::acpm::write(m, RTC, UPDATE, upd & !1)?;
    spin_us(50);
    crate::acpm::write(m, RTC, UPDATE, upd | 1)?;
    spin_us(1000);
    let mut t = [0u32; 7];
    for (i, v) in t.iter_mut().enumerate() {
        *v = crate::acpm::read(m, RTC, SEC + i as u32)? as u32;
    }
    // SEC, MIN, HOUR, WEEK (a one-hot day of the week), DAY, MON, YEAR (since 2000).
    Ok((2000 + (t[6] & 0x7F), t[5] & 0x0F, t[4] & 0x1F, t[2] & 0x1F, t[1] & 0x7F, t[0] & 0x7F))
}

fn spin_us(us: u64) {
    let f = crate::platform::cntfrq();
    let t = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t) < us * f / 1_000_000 {
        core::hint::spin_loop();
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's days_from_civil).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Read the RTC and set the base; returns the unix seconds read.
pub fn sync() -> Result<u64, u32> {
    let (y, mo, d, h, mi, s) = read()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return Err(u32::MAX);
    }
    let unix = (days_from_civil(y as i64, mo, d) * 86400 + (h * 3600 + mi * 60 + s) as i64) as u64;
    let (f, now) = (crate::platform::cntfrq(), crate::platform::cntpct());
    let base = (unix * f).wrapping_sub(now);
    BASE.store(base, Ordering::Release);
    READ_AT.store(now, Ordering::Relaxed);
    crate::process::set_wall(base, packed());
    Ok(unix)
}

/// From the service loop: the first read, then one an hour; and at the zone's next change (a known instant, not a poll of anything), the offsets again.
pub fn poll() {
    let (f, now) = (crate::platform::cntfrq(), crate::platform::cntpct());
    let next = NEXT.load(Ordering::Relaxed);
    if next != 0 && utc_now().is_some_and(|t| t >= next) {
        crate::process::set_wall(BASE.load(Ordering::Relaxed), packed());
    }
    if BASE.load(Ordering::Relaxed) == 0 || now.wrapping_sub(READ_AT.load(Ordering::Relaxed)) > 3600 * f {
        if sync().is_err() {
            // Try again in a minute rather than on every pass.
            READ_AT.store(now.wrapping_sub(3540 * f), Ordering::Relaxed);
        }
    }
}
