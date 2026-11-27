//! The weather cell: a place rounded to about 10 km before it leaves the device.
//!
//! Rows are latitude in steps of 2^-12 of a turn (360/4096 degrees, 9.77 km of the meridian), numbered from 0 at the equator, -2^10 at the south pole and 2^10 at the north pole.
//! The two pole rows are caps of one cell each, 4.9 km across.
//! Row `r` is cut into `columns(r) = 1 + floor(2^12 cos(lat_r))` equal cells of longitude, numbered eastward from 0 at the prime meridian, so every row tiles its circle exactly: no sliver cell at the antimeridian and no seam anywhere.
//! A cell is at most 9.77 km wide and tall and at least 4.9 km wide (the widths shrink to 1/(1 + 1/x) of the step, x = 2^12 cos(lat), which is at least 1/2 for x of at least 1, and x is under 1 only on the cap rows).
//! A place is never more than about 7 km from its cell's centre.
//!
//! fgtw.org reads the same two numbers and computes the same centre from them, so the ask can name nothing finer than a cell.
//! The worker carries its own copy of `columns` (fgtw-bootstrap src/weather.rs) and both crates test it against the same vectors; libm's cos is software, so both compute it bit for bit alike.

/// Rows from the equator to a pole: 2^10 (a quarter turn of 2^12 steps).
pub const POLE_ROW: i32 = 1 << 10;

/// Latitude steps in a whole turn: 2^12.
const STEPS: f64 = (1u32 << 12) as f64;

/// A weather cell: its row (latitude) and its column (longitude within the row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub row: i32,
    pub column: u32,
}

/// The number of cells in row `row`, which must be within [-2^10, 2^10].
pub fn columns(row: i32) -> u32 {
    let lat = row as f64 * (core::f64::consts::TAU / STEPS);
    // The 1 + keeps the pole caps one cell (cos there is 6e-17, floor 0) and makes every other row's cells no wider than the latitude step.
    1 + libm::floor(STEPS * libm::cos(lat)) as u32
}

impl Cell {
    /// The cell holding a place given in degrees (the GPS fix, which never leaves the device unrounded).
    /// None for a latitude outside [-90, 90] or a place that is not finite: a fix is a sensor's word, validated here before it is used to index a row (a NaN would cast to row 0 and name a wrong cell silently).
    pub fn of(lat_deg: f64, lon_deg: f64) -> Option<Cell> {
        if !(lat_deg >= -90. && lat_deg <= 90.) || !lon_deg.is_finite() {
            return None;
        }
        let row = libm::round(lat_deg * (STEPS / 360.)) as i32;
        let n = columns(row);
        let turns = lon_deg * (1. / 360.);
        let turns = turns - libm::floor(turns);
        // `% n` is the circle closing: a place a hair west of the prime meridian rounds to column n, which is column 0 (and `turns` itself can round up to 1.0 for a tiny negative longitude).
        let column = (libm::round(turns * n as f64) as u32) % n;
        Some(Cell { row, column })
    }

    /// Does this cell exist on the grid? The worker asks this of every cell it is sent; the device makes only real ones.
    pub fn is_real(&self) -> bool {
        // An ask is network input: a row past a pole has no columns to count, and a column past its row's count names no place.
        self.row >= -POLE_ROW && self.row <= POLE_ROW && self.column < columns(self.row)
    }

    /// The cell's centre in degrees: latitude, and longitude in (-180, 180].
    pub fn centre(&self) -> (f64, f64) {
        let lat = self.row as f64 * (360. / STEPS);
        let lon = self.column as f64 * (360. / columns(self.row) as f64);
        (lat, if lon > 180. { lon - 360. } else { lon })
    }
}
