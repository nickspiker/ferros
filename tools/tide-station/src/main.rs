//! tide-station: tide stations as VSF for the tide puck (docs/PUCKS.md).
//!
//! - `station`: one station's published harmonic constants (NOAA CO-OPS's harcon.json, datums.json and the station's own record, fetched on the host by fetch.sh) to a station document (ferros_tide::station).
//! - `catalog`: the stations' catalog (id, name, place) from NOAA's station list (stations.json?type=harcon) or from station documents.
//! - `show`: a station document or a catalog, read back as the device reads it.
//! - `predict`: a station document's highs and lows over days from a UTC date, to compare with the source's own predictions.
//!
//! JSON is read here, on the host, and never on the device. A station's place is public (the source publishes it); the person's place never passes thru this tool.

use ferros_tide::station::{Catalog, Constituent, Entry, Station, phase_q32, um};
use ferros_tide::{Extreme, eagle_from_unix};
use serde_json::Value;
use std::path::{Path, PathBuf};
use vsf::WorldCoord;

/// NOAA CO-OPS data: works of the US Government, public domain in the US (17 USC 105).
const NOAA_LICENCE: &str = "US public domain: NOAA CO-OPS data, a work of the US Government (17 USC 105)";
const NOAA_API: &str = "api.tidesandcurrents.noaa.gov/mdapi/prod/webapi";

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("tide-station: {msg}");
    std::process::exit(1);
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  tide-station station --harcon <harcon.json> --datums <datums.json> --station <station.json> [--datum MLLW] --out <file.tide.vsf>\n  tide-station catalog --out <catalog.vsf> (--noaa <stations.json> | <station.tide.vsf>...)\n  tide-station show <file.vsf>\n  tide-station predict <station.tide.vsf> <year> <month> <day> [days]\n(fetch.sh beside this tool fetches NOAA's files for a station id, or the station list.)"
    );
    std::process::exit(2);
}

fn json(path: &Path) -> Value {
    let b = std::fs::read(path).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
    serde_json::from_slice(&b).unwrap_or_else(|e| die(format!("{}: not JSON: {e}", path.display())))
}

/// A field that must be there.
fn field<'a>(v: &'a Value, name: &str, from: &Path) -> &'a Value {
    v.get(name).unwrap_or_else(|| die(format!("{}: no `{name}`", from.display())))
}

fn num(v: &Value, name: &str, from: &Path) -> f64 {
    field(v, name, from).as_f64().unwrap_or_else(|| die(format!("{}: `{name}` is not a number", from.display())))
}

fn string(v: &Value, name: &str, from: &Path) -> String {
    field(v, name, from).as_str().map(str::to_string).unwrap_or_else(|| die(format!("{}: `{name}` is not text", from.display())))
}

/// Text for a field the puck reads as ASCII: Latin letters with marks lose their marks (NOAA's names are English and Spanish place names); anything else refuses the station.
fn ascii(s: &str, what: &str) -> String {
    let fold = |c: char| -> Option<char> {
        Some(match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' => 'A',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' => 'O',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
            'ñ' => 'n',
            'Ñ' => 'N',
            'ç' => 'c',
            'Ç' => 'C',
            c if c.is_ascii() => c,
            _ => return None,
        })
    };
    s.chars().map(|c| fold(c).unwrap_or_else(|| die(format!("{what} {s:?}: a letter with no ASCII form")))).collect()
}

fn now() -> i64 {
    vsf::eagle_time_oscillations()
}

/// NOAA's three files for one station to a station document.
fn station(args: &[String]) {
    let (mut harcon, mut datums, mut record, mut out) = (None, None, None, None);
    let mut datum = String::from("MLLW");
    for pair in args.chunks(2) {
        let [flag, v] = pair else { usage() };
        match flag.as_str() {
            "--harcon" => harcon = Some(PathBuf::from(v)),
            "--datums" => datums = Some(PathBuf::from(v)),
            "--station" => record = Some(PathBuf::from(v)),
            "--datum" => datum = v.clone(),
            "--out" => out = Some(PathBuf::from(v)),
            _ => usage(),
        }
    }
    let (Some(harcon), Some(datums), Some(record), Some(out)) = (harcon, datums, record, out) else { usage() };

    let h = json(&harcon);
    let units = string(&h, "units", &harcon);
    if units != "meters" {
        die(format!("{}: units {units:?}; fetch with units=metric", harcon.display()));
    }
    let list = field(&h, "HarmonicConstituents", &harcon).as_array().unwrap_or_else(|| die("HarmonicConstituents is not a list"));
    let mut constituents = Vec::new();
    for c in list {
        let name = string(c, "name", &harcon);
        let amp = num(c, "amplitude", &harcon);
        let g = num(c, "phase_GMT", &harcon);
        // A constituent NOAA lists with nothing in it (amplitude 0) is kept: the table is the station's whole record.
        constituents.push(Constituent { name: ascii(&name, "constituent"), amplitude_um: um(amp) as u64, phase_q32: phase_q32(g) });
    }

    let d = json(&datums);
    let units = string(&d, "units", &datums);
    if units != "meters" {
        die(format!("{}: units {units:?}; fetch with units=metric", datums.display()));
    }
    let table = field(&d, "datums", &datums).as_array().unwrap_or_else(|| die("datums is not a list"));
    let level = |n: &str| table.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(n)).map(|x| num(x, "value", &datums)).unwrap_or_else(|| die(format!("{}: no {n} datum", datums.display())));
    let msl = level("MSL") - level(&datum);

    let r = json(&record);
    let st = field(&r, "stations", &record).as_array().and_then(|a| a.first()).unwrap_or_else(|| die(format!("{}: no station in it", record.display())));
    let id = string(st, "id", &record);
    let name = ascii(&string(st, "name", &record), "name");
    let (lat, lon) = (num(st, "lat", &record), num(st, "lng", &record));

    let s = Station {
        id: id.clone(),
        name,
        position: WorldCoord::from_lat_lon(lat, lon),
        datum,
        msl_um: um(msl),
        source: format!("NOAA CO-OPS harmonic constituents and datums, {NOAA_API}/stations/{id}/harcon.json and datums.json"),
        licence: NOAA_LICENCE.to_string(),
        constituents,
    };
    let (_, unknown) = s.tide();
    if !unknown.is_empty() {
        eprintln!("tide-station: warning: {} not among the engine's constituents, kept in the document and left out of the prediction: {}", id, unknown.join(" "));
    }
    let doc = s.build(now()).unwrap_or_else(|e| die(e));
    // Read back as the puck reads it before it is written.
    let (back, _) = Station::parse(&doc).unwrap_or_else(|e| die(format!("the document does not read back: {e}")));
    if back != s {
        die("the document reads back different");
    }
    std::fs::write(&out, &doc).unwrap_or_else(|e| die(format!("{}: {e}", out.display())));
    println!("{}: {} {}, G#{:X} constituents, mean sea level A#{:.3} m above {} (G#{:X} bytes)", out.display(), s.id, s.name, s.constituents.len(), msl, s.datum, doc.len());
}

/// The catalog from NOAA's station list, or from station documents.
fn catalog(args: &[String]) {
    let (mut out, mut noaa, mut files) = (None, None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--noaa" => noaa = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            f => files.push(PathBuf::from(f)),
        }
    }
    let Some(out) = out else { usage() };
    let c = match noaa {
        Some(p) => {
            let v = json(&p);
            let list = field(&v, "stations", &p).as_array().unwrap_or_else(|| die("stations is not a list"));
            let stations = list.iter().map(|s| Entry { id: string(s, "id", &p), name: ascii(&string(s, "name", &p), "name"), position: WorldCoord::from_lat_lon(num(s, "lat", &p), num(s, "lng", &p)) }).collect();
            Catalog { source: format!("NOAA CO-OPS stations with harmonic constituents, {NOAA_API}/stations.json?type=harcon"), licence: NOAA_LICENCE.to_string(), stations }
        }
        None if !files.is_empty() => {
            let mut stations = Vec::new();
            let mut licences: Vec<String> = Vec::new();
            for f in &files {
                let b = std::fs::read(f).unwrap_or_else(|e| die(format!("{}: {e}", f.display())));
                let (s, _) = Station::parse(&b).unwrap_or_else(|e| die(format!("{}: {e}", f.display())));
                if !licences.contains(&s.licence) {
                    licences.push(s.licence.clone());
                }
                stations.push(Entry { id: s.id, name: s.name, position: s.position });
            }
            Catalog { source: String::from("station documents (each names its own source)"), licence: licences.join("; "), stations }
        }
        None => usage(),
    };
    let doc = c.build(now()).unwrap_or_else(|e| die(e));
    if Catalog::parse(&doc).unwrap_or_else(|e| die(format!("the catalog does not read back: {e}"))) != c {
        die("the catalog reads back different");
    }
    std::fs::write(&out, &doc).unwrap_or_else(|e| die(format!("{}: {e}", out.display())));
    println!("{}: G#{:X} stations (G#{:X} bytes)", out.display(), c.stations.len(), doc.len());
}

fn show(args: &[String]) {
    let [f] = args else { usage() };
    let b = std::fs::read(f).unwrap_or_else(|e| die(format!("{f}: {e}")));
    if let Ok((s, _)) = Station::parse(&b) {
        let (_, unknown) = s.tide();
        println!("{} {}: G#{:X} constituents ({} unknown to the engine), mean sea level A#{:.3} m above {}\n  source: {}\n  licence: {}", s.id, s.name, s.constituents.len(), unknown.len(), s.msl_um as f64 * 1e-6, s.datum, s.source, s.licence);
        for c in &s.constituents {
            println!("  {:5} A#{:.3} m  A#{:.1} deg", c.name, c.amplitude_um as f64 * 1e-6, c.phase_q32 as f64 * 360. / (1u64 << 32) as f64);
        }
    } else {
        let c = Catalog::parse(&b).unwrap_or_else(|e| die(format!("{f}: neither a station nor a catalog: {e}")));
        println!("catalog: G#{:X} stations\n  source: {}\n  licence: {}", c.stations.len(), c.source, c.licence);
        for e in &c.stations {
            println!("  {} {}", e.id, e.name);
        }
    }
}

/// POSIX seconds at a civil UTC date (for the command line only; everything inside is Eagle time).
fn unix_day(y: i64, m: i64, d: i64) -> i64 {
    ferros_tide::days_from_civil(y, m, d) * 86_400
}

/// Eagle seconds shown as UTC (display only).
fn utc(t: f64) -> String {
    let u = t.round() as i64 - eagle_from_unix(0) as i64;
    let days = u.div_euclid(86_400);
    let s = u.rem_euclid(86_400);
    // Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", s / 3600, s % 3600 / 60)
}

fn predict(args: &[String]) {
    if args.len() < 4 {
        usage();
    }
    let n = |k: usize| args.get(k).map(|a| a.parse::<i64>().unwrap_or_else(|_| usage()));
    let b = std::fs::read(&args[0]).unwrap_or_else(|e| die(format!("{}: {e}", args[0])));
    let (s, _) = Station::parse(&b).unwrap_or_else(|e| die(format!("{}: {e}", args[0])));
    let (tide, _) = s.tide();
    let from = eagle_from_unix(unix_day(n(1).unwrap(), n(2).unwrap(), n(3).unwrap()));
    let days = n(4).unwrap_or(1);
    println!("{} {}, heights above {}:", s.id, s.name, s.datum);
    let mut e = [Extreme { t: 0., h: 0., high: false }; 1 << 4];
    for d in 0..days {
        let a = from + (d * 86_400) as f64;
        let k = tide.extremes(a, a + 86_400., &mut e);
        for x in &e[..k] {
            println!("  {}  A#{:.3} m  {}", utc(x.t), x.h, if x.high { "high" } else { "low" });
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    match cmd.as_str() {
        "station" => station(&args[1..]),
        "catalog" => catalog(&args[1..]),
        "show" => show(&args[1..]),
        "predict" => predict(&args[1..]),
        _ => usage(),
    }
}
