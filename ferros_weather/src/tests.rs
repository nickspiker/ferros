use super::*;
use alloc::vec;
use grid::{columns, POLE_ROW};

/// An hour in Eagle oscillations.
const HOUR: i64 = 3600 * vsf::OSCILLATIONS_PER_SECOND as i64;

/// The great-circle distance in km between two places in degrees (mean Earth radius).
fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = core::f64::consts::PI / 180.;
    let (la, lb) = (a.0 * r, b.0 * r);
    let dlat = lb - la;
    let dlon = (b.1 - a.1) * r;
    let h = libm::pow(libm::sin(dlat / 2.), 2.) + libm::cos(la) * libm::cos(lb) * libm::pow(libm::sin(dlon / 2.), 2.);
    2. * 6371.0088 * libm::asin(libm::sqrt(h))
}

/// The vectors fgtw-bootstrap's src/weather.rs tests its copy of `columns` against: both sides must cut every row alike.
pub(crate) const COLUMN_VECTORS: [(i32, u32); 8] = [(0, 4097), (1, 4096), (-1, 4096), (512, 2897), (-683, 2047), (1000, 151), (1023, 7), (1024, 1)];

#[test]
fn the_rows_are_cut_alike_on_both_sides() {
    for (row, n) in COLUMN_VECTORS {
        assert_eq!(columns(row), n, "row {row}");
    }
    assert_eq!(columns(-POLE_ROW), 1);
    // Every row's cells are at most a latitude step wide (9.77 km) and, off the caps, at least half of one.
    let step = 2. * core::f64::consts::PI * 6371.0088 / 4096.;
    for row in -POLE_ROW + 1..POLE_ROW {
        let lat = row as f64 * (core::f64::consts::TAU / 4096.);
        let width = 2. * core::f64::consts::PI * 6371.0088 * libm::cos(lat) / columns(row) as f64;
        assert!(width <= step && width >= step / 2., "row {row}: {width} km");
    }
}

#[test]
fn a_place_is_near_its_cell_and_the_centre_is_its_own_cell() {
    let mut worst: f64 = 0.;
    let mut lat = -90.;
    while lat <= 90. {
        let mut lon = -180.;
        while lon <= 180. {
            let c = Cell::of(lat, lon).unwrap();
            assert!(c.is_real(), "{lat} {lon}");
            let centre = c.centre();
            assert_eq!(Cell::of(centre.0, centre.1), Some(c), "{lat} {lon}");
            worst = worst.max(km((lat, lon), centre));
            lon += 0.0371;
        }
        lat += 0.0293;
    }
    // Half a cell's diagonal: about 6.9 km.
    assert!(worst < 7., "{worst} km");
}

#[test]
fn the_antimeridian_has_no_seam() {
    for lat in [-89.95, -60., -12.3, 0., 33.3, 64.2, 89.95] {
        assert_eq!(Cell::of(lat, 180.), Cell::of(lat, -180.));
        let (e, w) = (Cell::of(lat, 179.9999).unwrap(), Cell::of(lat, -179.9999).unwrap());
        assert_eq!(e.row, w.row);
        let n = columns(e.row);
        // The same cell or neighbours across the line, never a sliver between.
        assert!(e.column == w.column || (e.column + 1) % n == w.column, "{lat}: {e:?} {w:?}");
        assert!(km((lat, 179.9999), (lat, -179.9999)) < 0.1);
        assert!(km((lat, 179.9999), e.centre()) < 7. && km((lat, -179.9999), w.centre()) < 7.);
    }
    // A hair west of the prime meridian is column 0's, not column n.
    assert_eq!(Cell::of(10., -1e-12).unwrap().column, 0);
    assert_eq!(Cell::of(10., 360.).unwrap(), Cell::of(10., 0.).unwrap());
}

#[test]
fn a_bad_fix_names_no_cell() {
    assert_eq!(Cell::of(f64::NAN, 0.), None);
    assert_eq!(Cell::of(90.0001, 0.), None);
    assert_eq!(Cell::of(0., f64::INFINITY), None);
    assert_eq!(Cell::of(90., 123.).unwrap(), Cell { row: POLE_ROW, column: 0 });
    assert!(!Cell { row: POLE_ROW + 1, column: 0 }.is_real());
    assert!(!Cell { row: 1023, column: 7 }.is_real());
}

#[test]
fn an_ask_round_trips_and_a_damaged_one_is_refused() {
    let ask = Ask { cell: Cell::of(47.6062, -122.3321).unwrap(), day_start: 77 * HOUR };
    let doc = ask.build(78 * HOUR).unwrap();
    assert_eq!(Ask::parse(&doc), Ok(ask));
    let (header, _) = VsfHeader::decode(&doc).unwrap();
    assert_eq!(header.fields.first().map(|f| f.name.as_str()), Some(ASK), "fgtw.org routes by the first section's name");
    let mut bad = doc.clone();
    let at = bad.len() - 3;
    bad[at] ^= 1;
    assert!(Ask::parse(&bad).is_err());
    let off = Ask { cell: Cell { row: 1023, column: 9 }, day_start: 0 };
    assert_eq!(Ask::parse(&off.build(0).unwrap()), Err("a cell off the grid"));
}

pub(crate) fn sample() -> Forecast {
    let t0 = 1_000 * HOUR;
    Forecast {
        cell: Cell::of(-41.29, 174.78).unwrap(),
        now: Now { time: t0, temperature_mk: 285_150, symbol: Symbol::PartlyCloudy, wind_mm_s: 7_400, wind_from_q16: 49_152, precipitation_um_h: 200, cloud_q16: 40_000 },
        hours: (1..5).map(|k| Hour { time: t0 + k * HOUR, temperature_mk: 285_150 + k as u64 * 300, symbol: Symbol::Showers, precipitation_um: 100 * k as u64, probability_q16: if k == 2 { None } else { Some(1 << 14) } }).collect(),
        high_mk: 288_000,
        low_mk: 281_000,
        issued: t0 - HOUR / 2,
        next_update: t0 + HOUR / 4,
        source: "MET Norway Locationforecast 2.0".to_string(),
        licence: "CC BY 4.0".to_string(),
    }
}

#[test]
fn a_forecast_round_trips() {
    let f = sample();
    let doc = f.build(5).unwrap();
    assert_eq!(Forecast::parse(&doc), Ok(f.clone()));
    let empty = Forecast { hours: vec![], ..f };
    assert_eq!(Forecast::parse(&empty.build(5).unwrap()), Ok(empty));
}

#[test]
fn a_damaged_forecast_is_refused() {
    let f = sample();
    let doc = f.build(5).unwrap();
    // Any byte changed after the build fails the provenance hash or the parse.
    for at in (0..doc.len()).step_by(7) {
        let mut bad = doc.clone();
        bad[at] ^= 1 << 2;
        assert!(Forecast::parse(&bad).is_err(), "byte {at}");
    }
    assert!(Forecast::parse(&doc[..doc.len() - 1]).is_err());
    let mut late = f.clone();
    late.hours.swap(0, 1);
    assert_eq!(Forecast::parse(&late.build(5).unwrap()), Err("hours out of time order"));
    let mut many = f.clone();
    many.hours = (1..18).map(|k| Hour { time: f.now.time + k * HOUR, ..f.hours[0] }).collect();
    assert_eq!(Forecast::parse(&many.build(5).unwrap()), Err("more hours than an answer carries"));
    let mut wind = f.clone();
    wind.now.wind_from_q16 = Q16;
    assert_eq!(Forecast::parse(&wind.build(5).unwrap()), Err("now with a fraction past a whole"));
    let mut text = f;
    text.source = "M\u{e9}t".to_string();
    assert!(text.build(5).is_err());
}

#[test]
fn a_document_missing_a_field_or_typed_wrong_is_refused() {
    let f = sample();
    let build = |forecast: VsfSection| {
        let mut n = VsfSection::new("now");
        n.add_field("time", time(f.now.time));
        n.add_field("temperature_mk", unsigned_value(1));
        n.add_field("symbol", unsigned_value(0));
        n.add_field("wind_mm_s", unsigned_value(1));
        n.add_field("wind_from_q16", unsigned_value(1));
        n.add_field("precipitation_um_h", unsigned_value(1));
        n.add_field("cloud_q16", unsigned_value(1));
        vsf::VsfBuilder::new().creation_time_oscillations(1).add_section_direct(forecast).add_section_direct(n).build().unwrap()
    };
    let forecast = |skip: &str, low: VsfType| {
        let mut s = VsfSection::new("forecast");
        let fields = [
            ("row", VsfType::i(f.cell.row as isize)),
            ("column", unsigned_value(f.cell.column as u64)),
            ("issued", time(1)),
            ("next_update", time(2)),
            ("high_mk", unsigned_value(3)),
            ("low_mk", low),
            ("source", text("s").unwrap()),
            ("licence", text("l").unwrap()),
        ];
        for (name, v) in fields {
            if name != skip {
                s.add_field(name, v);
            }
        }
        s
    };
    assert!(Forecast::parse(&build(forecast("", unsigned_value(4)))).is_ok());
    assert_eq!(Forecast::parse(&build(forecast("low_mk", unsigned_value(4)))), Err("a forecast without its low"));
    assert_eq!(Forecast::parse(&build(forecast("next_update", unsigned_value(4)))), Err("a forecast without its next update"));
    // A temperature written as a time is not a temperature.
    assert_eq!(Forecast::parse(&build(forecast("", time(4)))), Err("a forecast without its low"));
    let error = vsf::VsfBuilder::new().add_section("error", vec![("reason".to_string(), text("bad_place").unwrap())]).build().unwrap();
    assert_eq!(Forecast::parse(&error), Err("fgtw.org answered with an error"));
}

#[test]
fn the_deadline_is_the_next_hour_or_the_update() {
    let f = sample();
    let t0 = f.now.time;
    // The update (t0 + 15 min) comes before the next hour.
    assert_eq!(f.next_deadline(t0), t0 + HOUR / 4);
    let later = Forecast { next_update: t0 + 3 * HOUR, ..f.clone() };
    assert_eq!(later.next_deadline(t0), t0 + HOUR);
    assert_eq!(later.next_deadline(t0 + HOUR), t0 + 2 * HOUR);
    assert_eq!(later.next_deadline(t0 + 10 * HOUR), t0 + 3 * HOUR);
    // An update already past means ask now.
    assert!(f.next_deadline(t0 + HOUR) <= t0 + HOUR);
}

/// The two documents shared with fgtw-bootstrap's src/weather.rs, both ways.
/// The ask is this crate's, and that worker's tests read it (its tests/fixtures/weather-ask.vsf): the bytes here are what `Ask::build` makes, so a change on either side shows.
/// The answer is the worker's build_reply output for a real MET Norway forecast (Seattle, 2026-11-18 06:04 UTC), read here by the device's parser.
#[test]
fn the_workers_documents_are_read() {
    extern crate std;
    let ask = Ask { cell: Cell::of(47.6062, -122.3321).unwrap(), day_start: 77 * HOUR }.build(78 * HOUR).unwrap();
    if let Ok(path) = std::env::var("WEATHER_ASK_OUT") {
        std::fs::write(path, &ask).unwrap();
    }
    assert_eq!(&ask[..], &include_bytes!("../tests/fixtures/weather-ask.vsf")[..]);

    let f = Forecast::parse(include_bytes!("../tests/fixtures/fgtw-reply.vsf")).unwrap();
    assert_eq!(f.cell, Cell::of(47.6062, -122.3321).unwrap());
    assert!(f.source.contains("MET Norway") && f.licence.contains("CC BY 4.0"));
    // 6.3 C at 06:00 UTC, from 62 degrees at 1.8 m/s, fair.
    assert_eq!((f.now.temperature_mk, f.now.wind_mm_s, f.now.symbol), (279_450, 1_800, Symbol::Fair));
    assert_eq!(f.hours.len(), MAX_HOURS);
    assert!(f.hours.iter().all(|h| h.probability_q16.is_none()));
    assert!(f.low_mk <= f.now.temperature_mk && f.now.temperature_mk <= f.high_mk);
    // MET's Expires (06:34:46) comes before the next hour (07:00): that is the deadline.
    assert_eq!(f.next_deadline(f.now.time + HOUR / 16), f.next_update);
    assert!(f.next_update < f.hours[0].time && f.hours[0].time == f.now.time + HOUR);
}
