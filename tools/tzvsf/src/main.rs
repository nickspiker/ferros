//! tzvsf: the world's time zones as one VSF document for ferros (owner, 2026-11-10: "Proper zone too"), from the host's tz database: every zone `zone1970.tab` names (the canonical ones, one per region whose clocks have agreed since 1970), each as a section `zone` with its `id` (the IANA name, `a`), its current `rule` (the POSIX TZ string tzdata writes at the end of the zone's file, `a`; ferros_tz reads it), its `country` codes (`a`, one value each: what the cellular network's country narrows a zone to), and `at`, its principal place (latitude, longitude in arc-seconds, `i`: GPS's nearest-zone fallback before region boundaries exist).
//! Then the networks' countries (owner, 2026-11-10: NITZ "with the country from the MCC, as Android does"): a section `country` per country with its `id` (ISO 3166 alpha-2, upper case as zone1970.tab writes it, `a`) and its mobile country codes (`mcc`, `u`, one value each), from mobile-broadband-provider-info's serviceproviders.xml (public domain, CC-PDDC; Fedora's and Debian's package of it), every country it lists a network for. A network whose country is not there names no zone; GPS and the hand then do.
//! Every rule is parsed by ferros_tz before it is written: one it cannot read stops the tool.
//! Usage: tzvsf <out.vsf> [zoneinfo dir, default /usr/share/zoneinfo] [serviceproviders.xml, default /usr/share/mobile-broadband-provider-info/serviceproviders.xml]   (stored on a device as `zones`: ferros-bridge store put zones @out.vsf)

use vsf::VsfType;
use vsf::file_format::VsfSection;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let Some(out) = a.get(1) else {
        eprintln!("usage: tzvsf <out.vsf> [zoneinfo dir] [serviceproviders.xml]");
        std::process::exit(2);
    };
    let dir = std::path::PathBuf::from(a.get(2).map(String::as_str).unwrap_or("/usr/share/zoneinfo"));
    let providers = a.get(3).map(String::as_str).unwrap_or("/usr/share/mobile-broadband-provider-info/serviceproviders.xml");
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
    let xml = std::fs::read_to_string(providers).unwrap_or_else(|e| panic!("{providers}: {e}"));
    let nets = mccs(&xml);
    for (country, codes) in &nets {
        let mut s = VsfSection::new("country");
        s.add_field("id", VsfType::a(vsf::Ascii::checked("id", country.clone())));
        s.add_field_multi("mcc", codes.iter().map(|&m| VsfType::u(m as usize, false)).collect());
        b = b.add_section_direct(s);
    }
    let bytes = b.build().expect("zones document");
    std::fs::write(out, &bytes).expect("write");
    println!("{out}: {n} zones, {} countries' networks ({} mobile country codes), {} bytes", nets.len(), nets.iter().map(|c| c.1.len()).sum::<usize>(), bytes.len());
}

/// Each country serviceproviders.xml lists a network for, and its mobile country codes, sorted and each once: its `<country code="xx">` elements and the `mcc="nnn"` of every `<network-id>` inside one (the only two things read; an MCC outside 200-799, ITU E.212's range, stops the tool).
fn mccs(xml: &str) -> Vec<(String, Vec<u16>)> {
    let mut out: Vec<(String, Vec<u16>)> = Vec::new();
    let mut country: Option<String> = None;
    for line in xml.lines() {
        if let Some(code) = attr(line, "<country", "code") {
            assert!(code.len() == 2 && code.bytes().all(|c| c.is_ascii_alphabetic()), "a country code {code:?}");
            country = Some(code.to_ascii_uppercase());
        } else if line.trim_start().starts_with("</country>") {
            country = None;
        } else if let (Some(c), Some(m)) = (&country, attr(line, "<network-id", "mcc")) {
            let m: u16 = m.parse().unwrap_or_else(|_| panic!("{c}: an MCC {m:?}"));
            assert!((200..800).contains(&m), "{c}: an MCC {m} outside E.212's range");
            match out.iter_mut().find(|e| e.0 == *c) {
                Some(e) => e.1.push(m),
                None => out.push((c.clone(), vec![m])),
            }
        }
    }
    for e in out.iter_mut() {
        e.1.sort_unstable();
        e.1.dedup();
    }
    out.sort();
    out
}

/// The value of attribute `name` on a line holding element `tag`, None if it does not.
fn attr<'a>(line: &'a str, tag: &str, name: &str) -> Option<&'a str> {
    let rest = &line[line.find(tag)? + tag.len()..];
    let key = format!(" {name}=\"");
    let start = rest.find(&key)? + key.len();
    let len = rest[start..].find('"')?;
    Some(&rest[start..start + len])
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
