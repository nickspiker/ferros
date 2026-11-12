//! cell-table: cell tables as VSF for the place service (docs/LOCATION.md, "Cellular").
//!
//! - `make`: a source's export to one table: OpenCelliD's CSV (radio, mcc, net, area, cell, unit, lon, lat, range, samples, changeable, created, updated, averageSignal), cut to the networks of the MCCs given, optionally to radios, a box and a least number of samples. A cell listed twice keeps the row with more samples.
//! - `show`: a table read back as the device reads it (ferros_cell::table): its region, source, licence, rows by network and radio, its size. Never a position.
//!
//! The CSV is read here, on the host, and never on the device.
//! A cell's place is the source's (public, under its licence); the person's place never passes thru this tool, and it prints no position.
//!
//! OpenCelliD's data is licensed CC BY-SA 4.0 (opencellid.org, Unwired Labs): a table made from it is a derived database under the same licence, and anything that shows a place from it credits OpenCelliD. Whether ferros ships such a table is the owner's call (docs/LOCATION.md, open questions); this tool only makes one on the host.

use ferros_cell::table::Table;
use ferros_cell::{DEGREE, Kind, Radio, plmn};
use std::collections::BTreeMap;
use std::path::PathBuf;

const OPENCELLID_LICENCE: &str = "CC BY-SA 4.0: derived from OpenCelliD (opencellid.org, Unwired Labs); shared under the same licence; credit OpenCelliD wherever a place from it is shown";

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("cell-table: {msg}");
    std::process::exit(1);
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  cell-table make --region <name> --mcc <mcc>[,<mcc>...] [--radio LTE,NR,...] [--box <south> <west> <north> <east>] [--min-samples <n>] [--default-range <m>] [--source <text>] --out <table.cells.vsf> <cells.csv>...\n  cell-table show <table.cells.vsf>\n(the CSV is OpenCelliD's export, gunzipped; its download needs an account's token, opencellid.org/downloads.php)"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("make") => make(&args[1..]),
        Some("show") => show(&args[1..]),
        _ => usage(),
    }
}

fn radio_of(s: &str) -> Option<Radio> {
    match s {
        "GSM" => Some(Radio::Gsm),
        "UMTS" => Some(Radio::Umts),
        "LTE" => Some(Radio::Lte),
        "NR" => Some(Radio::Nr),
        "CDMA" => Some(Radio::Cdma),
        _ => None,
    }
}

fn radio_name(r: u8) -> &'static str {
    match Radio::from_code(r) {
        Some(Radio::Gsm) => "GSM",
        Some(Radio::Umts) => "UMTS",
        Some(Radio::Lte) => "LTE",
        Some(Radio::Nr) => "NR",
        Some(Radio::Cdma) => "CDMA",
        None => "?",
    }
}

/// One row as it will be stored, and the samples it rests on (to keep the better of two).
struct Row {
    lat: i32,
    lon: i32,
    range_m: u32,
    samples: u64,
}

fn make(args: &[String]) {
    let (mut region, mut mccs, mut radios, mut bbox, mut min_samples, mut default_range, mut source, mut out, mut inputs) =
        (None, Vec::<u16>::new(), Vec::<Radio>::new(), None::<[f64; 4]>, 0u64, 1u32 << 12, "OpenCelliD (opencellid.org), cell export".to_string(), None, Vec::<PathBuf>::new());
    let mut k = 0;
    let arg = |k: usize| args.get(k).cloned().unwrap_or_else(|| usage());
    while k < args.len() {
        match args[k].as_str() {
            "--region" => (region, k) = (Some(arg(k + 1)), k + 2),
            "--mcc" => {
                mccs = arg(k + 1).split(',').map(|m| m.trim().parse().unwrap_or_else(|_| die(format!("not an MCC: {m}")))).collect();
                k += 2;
            }
            "--radio" => {
                radios = arg(k + 1).split(',').map(|r| radio_of(r.trim()).unwrap_or_else(|| die(format!("not a radio: {r}")))).collect();
                k += 2;
            }
            "--box" => {
                let v: Vec<f64> = (1..=4).map(|i| arg(k + i).parse().unwrap_or_else(|_| die("--box takes four numbers: south west north east"))).collect();
                bbox = Some([v[0], v[1], v[2], v[3]]);
                k += 5;
            }
            "--min-samples" => (min_samples, k) = (arg(k + 1).parse().unwrap_or_else(|_| die("--min-samples takes a count")), k + 2),
            "--default-range" => (default_range, k) = (arg(k + 1).parse().unwrap_or_else(|_| die("--default-range takes metres")), k + 2),
            "--source" => (source, k) = (arg(k + 1), k + 2),
            "--out" => (out, k) = (Some(PathBuf::from(arg(k + 1))), k + 2),
            a if a.starts_with("--") => usage(),
            _ => {
                inputs.push(PathBuf::from(&args[k]));
                k += 1;
            }
        }
    }
    let (Some(region), Some(out)) = (region, out) else { usage() };
    if mccs.is_empty() || inputs.is_empty() {
        usage();
    }
    if !region.is_ascii() {
        die("the region's name must be ASCII (the device reads it no_std)");
    }
    // Keyed as the table sorts: (network, cell, area, radio).
    let mut rows: BTreeMap<(u32, u64, u32, u8), Row> = BTreeMap::new();
    let (mut read, mut skipped, mut twice) = (0u64, 0u64, 0u64);
    for path in &inputs {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
        for (n, line) in text.lines().enumerate() {
            if line.is_empty() || (n == 0 && line.starts_with("radio")) {
                continue;
            }
            read += 1;
            let f: Vec<&str> = line.split(',').collect();
            if f.len() < 10 {
                die(format!("{} line {}: {} fields, OpenCelliD's export has 14", path.display(), n + 1, f.len()));
            }
            let Some(radio) = radio_of(f[0]) else {
                skipped += 1;
                continue;
            };
            let num = |i: usize| -> Option<u64> { f[i].parse().ok() };
            let deg = |i: usize| -> Option<f64> { f[i].parse::<f64>().ok().filter(|v| v.is_finite()) };
            let (Some(mcc), Some(mnc), Some(area), Some(cell), Some(lon), Some(lat)) = (num(1), num(2), num(3), num(4), deg(6), deg(7)) else {
                skipped += 1;
                continue;
            };
            let range = f[8].parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.).unwrap_or(0.);
            let samples = num(9).unwrap_or(0);
            let wanted = mccs.contains(&(mcc as u16))
                && mcc >> 16 == 0
                && mnc >> 16 == 0
                && area >> 32 == 0
                && (radios.is_empty() || radios.contains(&radio))
                && samples >= min_samples
                && lat.abs() <= 90.
                && lon.abs() <= 180.
                && bbox.is_none_or(|[s, w, n, e]| lat >= s && lat <= n && if w <= e { lon >= w && lon <= e } else { lon >= w || lon <= e });
            if !wanted {
                skipped += 1;
                continue;
            }
            let key = (plmn(mcc as u16, mnc as u16), cell, area as u32, radio.code());
            let row = Row { lat: (lat * DEGREE as f64).round() as i32, lon: (lon * DEGREE as f64).round() as i32, range_m: range.round() as u32, samples };
            match rows.get(&key) {
                Some(old) => {
                    twice += 1;
                    if row.samples > old.samples {
                        rows.insert(key, row);
                    }
                }
                None => {
                    rows.insert(key, row);
                }
            }
        }
    }
    let mut t = Table {
        region,
        source,
        licence: OPENCELLID_LICENCE.to_string(),
        kind: Kind::Centroid,
        default_range_m: default_range,
        plmn: Vec::with_capacity(rows.len()),
        cell: Vec::with_capacity(rows.len()),
        area: Vec::with_capacity(rows.len()),
        radio: Vec::with_capacity(rows.len()),
        lat: Vec::with_capacity(rows.len()),
        lon: Vec::with_capacity(rows.len()),
        range_m: Vec::with_capacity(rows.len()),
    };
    for ((p, c, a, r), row) in rows {
        t.plmn.push(p);
        t.cell.push(c);
        t.area.push(a);
        t.radio.push(r);
        t.lat.push(row.lat);
        t.lon.push(row.lon);
        t.range_m.push(row.range_m);
    }
    let doc = t.build(vsf::eagle_time_oscillations()).unwrap_or_else(|e| die(e));
    std::fs::write(&out, &doc).unwrap_or_else(|e| die(format!("{}: {e}", out.display())));
    println!("{}: {} cells ({} bytes, {} a cell) from {read} rows read; {skipped} left out (other networks, radios, places or too few samples), {twice} listed twice", out.display(), t.plmn.len(), doc.len(), if t.plmn.is_empty() { 0 } else { doc.len() / t.plmn.len() });
}

fn show(args: &[String]) {
    let Some(path) = args.first() else { usage() };
    let doc = std::fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let (t, made) = Table::parse(&doc).unwrap_or_else(|e| die(format!("{path}: refused: {e}")));
    println!("region:  {}\nsource:  {}\nlicence: {}\nkind:    {:?}\nmade:    Eagle oscillations {made}\ncells:   {} ({} bytes, default range {} m)", t.region, t.source, t.licence, t.kind, t.plmn.len(), doc.len(), t.default_range_m);
    let mut by: BTreeMap<(u32, u8), u64> = BTreeMap::new();
    for k in 0..t.plmn.len() {
        *by.entry((t.plmn[k], t.radio[k])).or_default() += 1;
    }
    for ((p, r), n) in by {
        println!("  MCC {:03} MNC {:02}  {:4}  {n}", p >> 16, p & ((1 << 16) - 1), radio_name(r));
    }
}
