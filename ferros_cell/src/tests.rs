//! Synthetic cells only: made-up networks (MCC 001, the test network), identities and places on round degrees, never a real cell or anyone's place.

use super::*;

const TEST: u32 = 1 << 16 | 1;
const OTHER: u32 = 1 << 16 | 2;

fn deg(d: i64) -> i32 {
    (d * DEGREE) as i32
}

/// Six synthetic cells, sorted: two networks; an LTE ECI in one area; a GSM id that repeats in two areas; an NR NCI past 32 bits; a cell with no range.
struct Fixture {
    plmn: [u32; 6],
    cell: [u64; 6],
    area: [u32; 6],
    radio: [u8; 6],
    lat: [i32; 6],
    lon: [i32; 6],
    range_m: [u32; 6],
}

fn fixture() -> Fixture {
    Fixture {
        plmn: [TEST, TEST, TEST, TEST, TEST, OTHER],
        cell: [7, 7, 0x01_2345, 0x03_0000, 0x02_0000_0001, 0x01_2345],
        area: [10, 11, 10, 10, 12, 10],
        radio: [Radio::Gsm.code(), Radio::Gsm.code(), Radio::Lte.code(), Radio::Lte.code(), Radio::Nr.code(), Radio::Lte.code()],
        lat: [deg(10), deg(11), deg(0), deg(-30), deg(45), deg(60)],
        lon: [deg(20), deg(21), deg(0), deg(-179), deg(90), deg(179)],
        range_m: [3000, 3000, 2000, 0, 500, 1000],
    }
}

fn cells(f: &Fixture, kind: Kind) -> Cells<'_> {
    Cells { plmn: &f.plmn, cell: &f.cell, area: &f.area, radio: &f.radio, lat: &f.lat, lon: &f.lon, range_m: &f.range_m, kind, default_range_m: 1 << 12 }
}

#[test]
fn a_cell_is_found_by_its_network_and_identity() {
    let f = fixture();
    let c = cells(&f, Kind::Centroid);
    assert_eq!(c.check(), Ok(()));
    assert_eq!(c.find(Radio::Lte, TEST, 0x01_2345, Some(10)), Some(2));
    // An LTE ECI needs no area.
    assert_eq!(c.find(Radio::Lte, TEST, 0x01_2345, None), Some(2));
    // The same identity in another network is another cell.
    assert_eq!(c.find(Radio::Lte, OTHER, 0x01_2345, None), Some(5));
    // The wrong area, the wrong radio, an identity nobody has: nothing.
    assert_eq!(c.find(Radio::Lte, TEST, 0x01_2345, Some(11)), None);
    assert_eq!(c.find(Radio::Nr, TEST, 0x01_2345, None), None);
    assert_eq!(c.find(Radio::Lte, TEST, 0x01_2346, None), None);
    // An NR NCI past 32 bits.
    assert_eq!(c.find(Radio::Nr, TEST, 0x02_0000_0001, Some(12)), Some(4));
}

#[test]
fn a_repeated_id_needs_its_area() {
    let f = fixture();
    let c = cells(&f, Kind::Centroid);
    assert_eq!(c.find(Radio::Gsm, TEST, 7, Some(10)), Some(0));
    assert_eq!(c.find(Radio::Gsm, TEST, 7, Some(11)), Some(1));
    // Two rows answer without the area: no place rather than a guess.
    assert_eq!(c.find(Radio::Gsm, TEST, 7, None), None);
}

#[test]
fn the_error_is_the_range_and_the_timing_advance_bounds_it_at_a_site() {
    let f = fixture();
    let s = Serving { radio: Radio::Lte, plmn: TEST, area: Some(10), cell: 0x01_2345, ta: Some(3) };
    // A centroid: the range alone (the timing advance is a distance from the antenna, not from the centroid).
    assert_eq!(cells(&f, Kind::Centroid).place(&s), Some(Place { lat: 0, lon: 0, err_m: 2000 }));
    // A site: four steps of 78.071 m, rounded up.
    assert_eq!(cells(&f, Kind::Site).place(&s), Some(Place { lat: 0, lon: 0, err_m: 313 }));
    // A timing advance past the range leaves the range; one past what LTE can set is not used.
    assert_eq!(cells(&f, Kind::Site).place(&Serving { ta: Some(100), ..s }).map(|p| p.err_m), Some(2000));
    assert_eq!(cells(&f, Kind::Site).place(&Serving { ta: Some(TA_MAX + 1), ..s }).map(|p| p.err_m), Some(2000));
    // A cell with no range takes the table's default.
    let s0 = Serving { radio: Radio::Lte, plmn: TEST, area: None, cell: 0x03_0000, ta: None };
    assert_eq!(cells(&f, Kind::Centroid).place(&s0).map(|p| p.err_m), Some(1 << 12));
}

#[test]
fn the_timing_advance_steps() {
    assert_eq!(ta_radius_m(0), Some(79));
    assert_eq!(ta_radius_m(TA_MAX), Some(100_166));
    assert_eq!(ta_radius_m(TA_MAX + 1), None);
}

#[test]
fn a_damaged_table_is_refused() {
    let mut f = fixture();
    f.cell.swap(2, 3);
    assert_eq!(cells(&f, Kind::Centroid).check(), Err(Refused::Order(3)));
    let mut f = fixture();
    f.cell[1] = 7;
    f.area[1] = 10;
    assert_eq!(cells(&f, Kind::Centroid).check(), Err(Refused::Order(1)));
    let mut f = fixture();
    f.radio[3] = 9;
    assert_eq!(cells(&f, Kind::Centroid).check(), Err(Refused::Radio(3)));
    let mut f = fixture();
    f.lat[0] = deg(91);
    assert_eq!(cells(&f, Kind::Centroid).check(), Err(Refused::Latitude(0)));
    let mut f = fixture();
    f.lon[5] = i32::MIN;
    assert_eq!(cells(&f, Kind::Centroid).check(), Err(Refused::Longitude(5)));
    let f = fixture();
    let c = Cells { lat: &f.lat[..5], ..cells(&f, Kind::Centroid) };
    assert_eq!(c.check(), Err(Refused::Lengths));
}

#[test]
fn distances_on_the_sphere() {
    // A degree of a great circle.
    let d = distance_m((0, 0), (deg(1), 0));
    assert!((d - EARTH_M * core::f64::consts::PI / 180.).abs() < 1e-6, "{d}");
    // Across the antimeridian, the short way.
    let d = distance_m((0, deg(179)), (0, deg(-179)));
    assert!((d - 2. * EARTH_M * core::f64::consts::PI / 180.).abs() < 1e-6, "{d}");
    // Antipodes.
    let d = distance_m((deg(45), deg(90)), (deg(-45), deg(-90)));
    assert!((d - EARTH_M * core::f64::consts::PI).abs() < 1e-3, "{d}");
}
