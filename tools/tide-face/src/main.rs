//! tide-face: the tide puck's face rendered on the host (docs/PUCKS.md "The tide puck"), to look at it without a device and to set it beside the owner's tide widget (inksurf tide-widget).
//! The square is drawn as the puck draws it (ferros_faces::tide::paint), then trimmed by the circle's C¹ edge over black, as the display trims it on the deck, and written as one Display plane, VSF RGB, gamma2tanh at 16 bits (tools/screencap's form; tools/vsf2web makes it a web image).
//! Usage: tide-face <station.tide.vsf> <lat> <lon> <year> <month> <day> <hour> <minute> [--d 600] [--base 10|12|16] [--zone seconds east] [--large] [--turn degrees] --out <face.vsf>
//! The moment is UTC on the command line and Eagle time from there on.

use ferros_faces::tide::{paint, scale, Cache, Chart};
use ferros_tide::station::Station;
use std::time::Instant;
use vsf::types::BitPackedTensor;
use vsf::visual::{Encoding, Plane, Primaries, Visual};

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("tide-face: {msg}");
    std::process::exit(1);
}

fn num<T: std::str::FromStr>(s: &str, what: &str) -> T {
    s.parse().unwrap_or_else(|_| die(format!("{what}: not a number: {s}")))
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let (mut d, mut base, mut zone, mut large, mut turn, mut out) = (600usize, 12u8, 0i64, false, 0f32, None);
    let mut i = 0;
    while i < a.len() {
        let v = || a.get(i + 1).cloned().unwrap_or_else(|| die(format!("{} wants a value", a[i])));
        match a[i].as_str() {
            "--d" => (d, i) = (num(&v(), "--d"), i + 1),
            "--base" => (base, i) = (num(&v(), "--base"), i + 1),
            "--zone" => (zone, i) = (num(&v(), "--zone"), i + 1),
            "--turn" => (turn, i) = (num(&v(), "--turn"), i + 1),
            "--out" => (out, i) = (Some(v()), i + 1),
            "--large" => large = true,
            _ => pos.push(a[i].clone()),
        }
        i += 1;
    }
    let (Some(out), [station, lat, lon, y, mo, da, h, mi]) = (out, pos.as_slice()) else {
        die("usage: tide-face <station.tide.vsf> <lat> <lon> <year> <month> <day> <hour> <minute> [--d 600] [--base 12] [--zone seconds] [--large] [--turn degrees] --out <face.vsf>");
    };
    let doc = std::fs::read(station).unwrap_or_else(|e| die(format!("{station}: {e}")));
    let (s, _) = Station::parse(&doc).unwrap_or_else(|e| die(format!("{station}: {e:?}")));
    let (tide, _) = s.tide();
    let place = ferros_sky::Place::deg(num(lat, "lat"), num(lon, "lon"));
    // UTC on the command line to Eagle seconds: days from 1970 by the civil calendar, then the Eagle epoch.
    let days = ferros_tide::days_from_civil(num(y, "year"), num(mo, "month"), num(da, "day"));
    let unix = days * 86400 + num::<i64>(h, "hour") * 3600 + num::<i64>(mi, "minute") * 60;
    let now = ferros_tide::eagle_from_unix(unix);
    let mut glyphs = ferros_glyphs::Glyphs::with_arena(ferros_glyphs::FONT, 1 << 15).unwrap_or_else(|| die("the typeface does not load"));
    let chart = Chart { tide: &tide, scale: scale(&tide), place, now, zone, base, station: large.then_some((s.name.as_str(), 3200.)) };
    let mut px = vec![0u64; d * d];
    let mut cache = Cache::new();
    let rad = turn.to_radians();
    let t0 = Instant::now();
    paint(&mut px, d, (rad.cos(), rad.sin()), &chart, &mut cache, &mut glyphs);
    let first = t0.elapsed();
    px.fill(0);
    let t1 = Instant::now();
    paint(&mut px, d, (rad.cos(), rad.sin()), &chart, &mut cache, &mut glyphs);
    let again = t1.elapsed();
    // Trimmed by the circle's edge over black: the light over black of an α + darkness pixel is α − darkness (memory feedback_darkness_space), a g2t code already.
    let r = d as f32 * 0.5;
    let mut samples = vec![0u16; 3 * d * d];
    for y in 0..d {
        for x in 0..d {
            let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
            let cov = ferros_puck::draw::edge((dx * dx + dy * dy).sqrt() - r);
            let p = px[y * d + x];
            let alpha = p >> 48;
            for c in 0..3 {
                let dark = p >> (32 - 16 * c) & 0xFFFF;
                samples[c * d * d + y * d + x] = ((alpha - dark) as f32 * cov) as u16;
            }
        }
    }
    let mut plane = Plane::encoded("display", "puck", d, d, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, d, d], &samples));
    plane.name = "tide".into();
    let v = Visual { title: "ferros tide puck".into(), text: "The tide puck's square, trimmed to its circle over black.".into(), planes: vec![plane], ..Default::default() };
    let file = v.write().unwrap_or_else(|e| die(format!("visual: {e:?}")));
    std::fs::write(&out, &file).unwrap_or_else(|e| die(format!("{out}: {e}")));
    println!("{out}: {} at {d} px, base {base}, the year and the face in {} ms, the face again in {} ms", s.name, first.as_millis(), again.as_millis());
}
