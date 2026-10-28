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

/// A new zone: kept and handed to every process with the base.
pub fn set_tz(secs: i64) {
    TZ.store(secs, Ordering::Relaxed);
    crate::process::set_wall(BASE.load(Ordering::Relaxed), secs);
}
/// The same with the kernel lock held (the store command runs under it).
pub fn set_tz_held(secs: i64) {
    TZ.store(secs, Ordering::Relaxed);
    crate::process::set_wall_held(BASE.load(Ordering::Relaxed), secs);
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
    crate::process::set_wall(base, TZ.load(Ordering::Relaxed));
    Ok(unix)
}

/// From the service loop: the first read, then one an hour.
pub fn poll() {
    let (f, now) = (crate::platform::cntfrq(), crate::platform::cntpct());
    if BASE.load(Ordering::Relaxed) == 0 || now.wrapping_sub(READ_AT.load(Ordering::Relaxed)) > 3600 * f {
        if sync().is_err() {
            // Try again in a minute rather than on every pass.
            READ_AT.store(now.wrapping_sub(3540 * f), Ordering::Relaxed);
        }
    }
}
