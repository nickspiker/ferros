//! tzvsf: the world's time zones as one VSF document for ferros (owner, 2026-11-10: "Proper zone too"), from the host's tz database: every zone `zone1970.tab` names (the canonical ones, one per region whose clocks have agreed since 1970), each as a section `zone` with its `id` (the IANA name, `a`), its current `rule` (the POSIX TZ string tzdata writes at the end of the zone's file, `a`; ferros_tz reads it), its `country` codes (`a`, one value each: what the cellular network's country narrows a zone to), and `at`, its principal place (latitude, longitude in arc-seconds, `i`: GPS's nearest-zone fallback before region boundaries exist).
//! Every rule is parsed by ferros_tz before it is written: one it cannot read stops the tool.
//! Usage: tzvsf <out.vsf> [zoneinfo dir, default /usr/share/zoneinfo]   (stored on a device as `zones`: ferros-bridge store put zones @out.vsf)

use vsf::VsfType;
use vsf::file_format::VsfSection;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let Some(out) = a.get(1) else {
        eprintln!("usage: tzvsf <out.vsf> [zoneinfo dir]");
        std::process::exit(2);
    };
    let dir = std::path::PathBuf::from(a.get(2).map(String::as_str).unwrap_or("/usr/share/zoneinfo"));
    let tab = std::fs::read_to_string(dir.join("zone1970.tab")).expect("zone1970.tab");
    let mut b = vsf::VsfBuilder::new().provenance_only();
    let mut n = 0;
    for line in tab.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let (countries, coords, id) = (f[0], f[1], f[2]);
        let file = std::fs::read(dir.join(id)).unwrap_or_else(|e| panic!("{id}: {e}"));
        let rule = footer(&file).unwrap_or_else(|| panic!("{id}: no POSIX rule at its end"));
        if ferros_tz::Rule::parse(&rule).is_none() {
            panic!("{id}: ferros_tz does not read its rule {rule:?}");
        }
        let (lat, lon) = iso6709(coords).unwrap_or_else(|| panic!("{id}: coordinates {coords:?}"));
        let mut s = VsfSection::new("zone");
        s.add_field("id", VsfType::a(vsf::Ascii::checked("id", id.to_string())));
        s.add_field("rule", VsfType::a(vsf::Ascii::checked("rule", rule)));
        s.add_field_multi("country", countries.split(',').map(|c| VsfType::a(vsf::Ascii::checked("country", c.to_string()))).collect());
        s.add_field_multi("at", vec![VsfType::i(lat as isize), VsfType::i(lon as isize)]);
        b = b.add_section_direct(s);
        n += 1;
    }
    let bytes = b.build().expect("zones document");
    std::fs::write(out, &bytes).expect("write");
    println!("{out}: {n} zones, {} bytes", bytes.len());
}

/// The POSIX TZ string at the end of a TZif file (version 2 and later): between its last two newlines.
fn footer(f: &[u8]) -> Option<String> {
    let end = f.len().checked_sub(1)?;
    if f[end] != b'\n' {
        return None;
    }
    let start = f[..end].iter().rposition(|&c| c == b'\n')? + 1;
    String::from_utf8(f[start..end].to_vec()).ok().filter(|s| !s.is_empty())
}

/// ISO 6709 as zone1970.tab writes it (`+DDMM+DDDMM` or `+DDMMSS+DDDMMSS`) to arc-seconds.
fn iso6709(s: &str) -> Option<(i64, i64)> {
    let split = s[1..].find(['+', '-'])? + 1;
    let part = |p: &str, deg: usize| -> Option<i64> {
        let sign = if p.starts_with('-') { -1 } else { 1 };
        let d = &p[1..];
        let (dd, mm, ss) = match d.len() - deg {
            2 => (&d[..deg], &d[deg..], "0"),
            4 => (&d[..deg], &d[deg..deg + 2], &d[deg + 2..]),
            _ => return None,
        };
        Some(sign * (dd.parse::<i64>().ok()? * 3600 + mm.parse::<i64>().ok()? * 60 + ss.parse::<i64>().ok()?))
    };
    Some((part(&s[..split], 2)?, part(&s[split..], 3)?))
}
