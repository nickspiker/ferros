//! The weather puck's face: the sky's symbol drawn (sun, cloud, rain, snow, fog, thunder: shapes, no words), the temperature now, today's high and low; in focus the next hours' temperatures as a curve with their rain under it, and the time of the forecast.

use crate::{u, CLOUD, RAIN, SNOW, SUN, TEXT, TEXT2, WATER_LINE};
use ferros_puck::{fmt::unit, q14, Full, Writer};
use ferros_weather::{Forecast, Symbol};

/// A temperature in millikelvin as Q16 of a degree Celsius.
fn celsius(mk: u64) -> i64 {
    ((mk as i64 - 273_150) << 16) / 1000
}

/// The symbol at (cx, cy), size s: front first, its rain or snow, then its cloud, then its sun.
fn symbol(w: &mut Writer, sym: Symbol, cx: f32, cy: f32, s: f32) -> Result<(), Full> {
    use Symbol::*;
    let (sun, cloud, drops, flakes, fog, bolt) = match sym {
        Clear => (true, false, 0, 0, false, false),
        Fair => (true, false, 0, 0, false, false),
        PartlyCloudy => (true, true, 0, 0, false, false),
        Cloudy | Unknown => (false, true, 0, 0, false, false),
        Fog => (false, false, 0, 0, true, false),
        Showers => (true, true, 2, 0, false, false),
        LightRain => (false, true, 1, 0, false, false),
        Rain => (false, true, 2, 0, false, false),
        HeavyRain => (false, true, 3, 0, false, false),
        Sleet => (false, true, 1, 1, false, false),
        SnowShowers => (true, true, 0, 2, false, false),
        LightSnow => (false, true, 0, 1, false, false),
        Snow => (false, true, 0, 2, false, false),
        HeavySnow => (false, true, 0, 3, false, false),
        Thunder => (false, true, 2, 0, false, true),
    };
    if bolt {
        w.capsule((q14(cx + 0.1 * s), q14(cy + 0.3 * s)), (q14(cx - 0.05 * s), q14(cy + 0.65 * s)), u(0.06 * s), SUN)?;
        w.capsule((q14(cx - 0.05 * s), q14(cy + 0.65 * s)), (q14(cx + 0.12 * s), q14(cy + 0.65 * s)), u(0.06 * s), SUN)?;
    }
    for k in 0..drops {
        let x = cx + (k as f32 - (drops - 1) as f32 * 0.5) * 0.3 * s;
        w.capsule((q14(x), q14(cy + 0.4 * s)), (q14(x - 0.08 * s), q14(cy + 0.65 * s)), u(0.05 * s), RAIN)?;
    }
    for k in 0..flakes {
        let x = cx + (k as f32 - (flakes - 1) as f32 * 0.5) * 0.3 * s + 0.15 * s * (drops > 0) as i32 as f32;
        w.disc(q14(x), q14(cy + 0.55 * s), u(0.07 * s), SNOW)?;
    }
    if fog {
        for k in 0..3 {
            let y = cy + (k as f32 - 1.) * 0.22 * s;
            w.capsule((q14(cx - 0.55 * s), q14(y)), (q14(cx + 0.55 * s), q14(y)), u(0.06 * s), CLOUD)?;
        }
    }
    if cloud {
        w.disc(q14(cx - 0.25 * s), q14(cy + 0.1 * s), u(0.28 * s), CLOUD)?;
        w.disc(q14(cx + 0.1 * s), q14(cy - 0.05 * s), u(0.38 * s), CLOUD)?;
        w.capsule((q14(cx - 0.35 * s), q14(cy + 0.22 * s)), (q14(cx + 0.4 * s), q14(cy + 0.22 * s)), u(0.16 * s), CLOUD)?;
    }
    if sun {
        let (sx, sy) = if cloud { (cx + 0.3 * s, cy - 0.3 * s) } else { (cx, cy) };
        w.disc(q14(sx), q14(sy), u(0.32 * s), SUN)?;
    }
    Ok(())
}

pub fn face(w: &mut Writer, f: &Forecast, focus: bool) -> Result<(), Full> {
    w.number(0, q14(0.3), u(0.2), TEXT, celsius(f.now.temperature_mk), 0, unit::CELSIUS)?;
    w.number(q14(-0.32), q14(0.62), u(0.1), TEXT2, celsius(f.low_mk), 0, unit::CELSIUS)?;
    w.number(q14(0.32), q14(0.62), u(0.1), TEXT, celsius(f.high_mk), 0, unit::CELSIUS)?;
    if focus {
        w.moment(0, q14(0.82), u(0.07), TEXT2, f.issued, 1)?;
        // The next hours' temperatures on the day's own scale (low to high), their rain as bars under them.
        let n = f.hours.len();
        if n >= 2 {
            let mut values = [(0i32, 0i32); ferros_weather::MAX_HOURS];
            for k in 0..n {
                let t = celsius(f.hours[k].temperature_mk) as i32;
                // The slope at each hour from its neighbours (one-sided at the ends), per the whole span: a C¹ curve thru every hour.
                // WHY the two branches: the first and last hours have one neighbour; the difference is taken one-sided there instead of reading past the list.
                let (a, b) = (if k == 0 { 0 } else { k - 1 }, if k + 1 == n { k } else { k + 1 });
                let m = (celsius(f.hours[b].temperature_mk) - celsius(f.hours[a].temperature_mk)) as f32 / (b - a) as f32 * (n - 1) as f32;
                values[k] = (t, m as i32);
            }
            let (lo, hi) = (celsius(f.low_mk) as i32 - (1 << 16), celsius(f.high_mk) as i32 + (1 << 16));
            w.curve(q14(-0.8), q14(0.8), q14(-0.05), q14(-0.62), lo, hi, u(0.016), WATER_LINE, None, 0, &values[..n])?;
            for (k, h) in f.hours.iter().enumerate() {
                if h.precipitation_um > 0 {
                    let x = -0.8 + 1.6 * k as f32 / (n - 1) as f32;
                    // A millimetre an hour a tenth of the radius, drawn as rain's height grows toward its cube root so a downpour stays on the face.
                    let len = 0.1 * libm::cbrtf(h.precipitation_um as f32 / 1000.);
                    w.capsule((q14(x), q14(0.12)), (q14(x), q14(0.12 + len)), u(0.018), RAIN)?;
                }
            }
        }
    } else {
        symbol(w, f.now.symbol, 0., -0.3, 0.7)?;
    }
    crate::sky(w)?;
    Ok(())
}

/// The face's deadline: the forecast's own (the next hour it shows, or the upstream's next update).
pub fn deadline(f: &Forecast, now: i64) -> i64 {
    f.next_deadline(now)
}
