//! A rough place from the serving cell (docs/LOCATION.md, "Cellular").
//! The modem knows, while it is registered, which cell serves the device: its network (MCC and MNC), its tracking area, its identity (the LTE ECI or the NR NCI) and, while connected, the timing advance.
//! A cell table says where each cell is; this crate finds the serving cell in one and gives its place with an error radius, on the device, offline: no request ever names a cell.
//!
//! **The table** is columns, one row a cell, sorted by (network, cell, area, radio) with no key twice, so a cell is found by binary search ([`Cells`]).
//! Positions are in 2^-23 degrees, as the GPS driver's fix ([`DEGREE`]); ranges in metres.
//! A table says what its positions are ([`Kind`]): the centroid of crowd measurements (OpenCelliD), or the antenna site (a regulator's register, where one maps cells to sites).
//!
//! **The error.** A centroid table's range is how far its measurements spread from the centroid: the error is that range.
//! A site table's position is the antenna, so the timing advance (how far the device is from it, in steps of [`TA_STEP_MM`]) bounds the error as well: the smaller of the two discs, which share a centre.
//! With a centroid table the timing advance says nothing about the distance from the centroid, and is not used.
//!
//! With the feature `vsf` the table is a VSF document ([`table`]).
#![no_std]

#[cfg(feature = "vsf")]
extern crate alloc;
/// The table as a VSF document.
#[cfg(feature = "vsf")]
pub mod table;
#[cfg(test)]
mod tests;

/// One degree in the unit positions are kept in: 2^-23 degrees, the GPS driver's fix (GPS.md).
pub const DEGREE: i64 = 1 << 23;

/// The Earth's mean radius in metres (IUGG R1, a published constant).
pub const EARTH_M: f64 = 6_371_008.8;

/// One step of LTE's timing advance as a one-way distance, in millimetres: N_TA = 16 T_s a step (3GPP TS 36.213 4.2.3), T_s = 1 / (15000 × 2048) s, so c × 16 T_s / 2 = 78.071 m.
pub const TA_STEP_MM: u64 = 78_071;

/// The largest LTE timing advance a random access response can set (an 11-bit field, 0 to 1282: TS 36.213 4.2.3); past it the modem's value is not a timing advance.
pub const TA_MAX: u32 = 1282;

/// The radio technology of a cell, by its code in the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Radio {
    Gsm = 0,
    Umts = 1,
    Lte = 2,
    Nr = 3,
    Cdma = 4,
}

impl Radio {
    pub fn from_code(c: u8) -> Option<Radio> {
        match c {
            0 => Some(Radio::Gsm),
            1 => Some(Radio::Umts),
            2 => Some(Radio::Lte),
            3 => Some(Radio::Nr),
            4 => Some(Radio::Cdma),
            _ => None,
        }
    }
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// A network as one number: its MCC in the high 16 bits, its MNC in the low (MNCs of two and three digits with the same value are the same number, as OpenCelliD keeps them).
pub fn plmn(mcc: u16, mnc: u16) -> u32 {
    (mcc as u32) << 16 | mnc as u32
}

/// What a table's positions are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// The centroid of measurements made in the cell (OpenCelliD's "estimated cell" position): where people were, not where the antenna is.
    Centroid = 0,
    /// The antenna's site.
    Site = 1,
}

impl Kind {
    pub fn from_code(c: u64) -> Option<Kind> {
        match c {
            0 => Some(Kind::Centroid),
            1 => Some(Kind::Site),
            _ => None,
        }
    }
}

/// A table's columns, borrowed: row k is one cell.
#[derive(Clone, Copy, Debug)]
pub struct Cells<'a> {
    pub plmn: &'a [u32],
    /// The cell's identity within its network: the LTE ECI (28 bits), the NR NCI (36 bits), the GSM or UMTS cell id.
    pub cell: &'a [u64],
    /// The tracking area (LTE, NR) or location area (GSM, UMTS).
    pub area: &'a [u32],
    pub radio: &'a [u8],
    /// 2^-23 degrees.
    pub lat: &'a [i32],
    pub lon: &'a [i32],
    /// Metres; 0 where the source gave none.
    pub range_m: &'a [u32],
    pub kind: Kind,
    /// The error for a cell whose range is 0 (a source that keeps a cell seen once without a spread).
    pub default_range_m: u32,
}

/// Why a table was refused (it is the device's input: a table that breaks these would make the search silently wrong).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The columns are not all the same length.
    Lengths,
    /// Row k is not after row k − 1 by (network, cell, area, radio).
    Order(usize),
    /// Row k's radio code is unknown.
    Radio(usize),
    /// Row k's latitude is past a pole.
    Latitude(usize),
    /// Row k's longitude is past ±180°.
    Longitude(usize),
}

/// The serving cell as the modem reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Serving {
    pub radio: Radio,
    pub plmn: u32,
    /// None where the modem gave none.
    pub area: Option<u32>,
    pub cell: u64,
    /// LTE's timing advance in its steps (0 to [`TA_MAX`]); None while idle or unknown.
    pub ta: Option<u32>,
}

/// A place: 2^-23 degrees, and the radius in metres it is good to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub lat: i32,
    pub lon: i32,
    pub err_m: u32,
}

impl<'a> Cells<'a> {
    /// The table checked once, as it is read: equal columns, the order the search needs, known radios, places on the Earth.
    pub fn check(&self) -> Result<(), Refused> {
        let n = self.plmn.len();
        if [self.cell.len(), self.area.len(), self.radio.len(), self.lat.len(), self.lon.len(), self.range_m.len()].iter().any(|&l| l != n) {
            return Err(Refused::Lengths);
        }
        for k in 0..n {
            if k > 0 && self.key(k - 1) >= self.key(k) {
                return Err(Refused::Order(k));
            }
            if Radio::from_code(self.radio[k]).is_none() {
                return Err(Refused::Radio(k));
            }
            if (self.lat[k] as i64).abs() > 90 * DEGREE {
                return Err(Refused::Latitude(k));
            }
            if (self.lon[k] as i64).abs() > 180 * DEGREE {
                return Err(Refused::Longitude(k));
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.plmn.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plmn.is_empty()
    }

    /// Row k's sort key.
    pub fn key(&self, k: usize) -> (u32, u64, u32, u8) {
        (self.plmn[k], self.cell[k], self.area[k], self.radio[k])
    }

    /// The row of a cell: its network and identity, of that radio, in that area when the area is known.
    /// Without the area, a cell is found only when its network has one cell of that radio with that identity (a GSM or UMTS id repeats across areas; an LTE ECI or NR NCI does not, within a network).
    /// For a table [`Cells::check`] passed.
    pub fn find(&self, radio: Radio, plmn: u32, cell: u64, area: Option<u32>) -> Option<usize> {
        // The first row at or after (plmn, cell): the run of rows with that network and identity starts there.
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if (self.plmn[mid], self.cell[mid]) < (plmn, cell) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut found = None;
        let mut k = lo;
        while k < self.len() && (self.plmn[k], self.cell[k]) == (plmn, cell) {
            if self.radio[k] == radio.code() && area.is_none_or(|a| a == self.area[k]) {
                if found.is_some() {
                    // Two rows answer and nothing tells them apart: no place rather than a guess.
                    return None;
                }
                found = Some(k);
            }
            k += 1;
        }
        found
    }

    /// The serving cell's place, or None when the table does not hold it.
    pub fn place(&self, s: &Serving) -> Option<Place> {
        let k = self.find(s.radio, s.plmn, s.cell, s.area)?;
        let range = if self.range_m[k] == 0 { self.default_range_m } else { self.range_m[k] };
        let err_m = match (self.kind, s.radio, s.ta.and_then(ta_radius_m)) {
            // Both discs are centred on the antenna, so the device is inside the smaller (the min is that intersection, not a bound on a value).
            (Kind::Site, Radio::Lte, Some(r)) => r.min(range),
            _ => range,
        };
        Some(Place { lat: self.lat[k], lon: self.lon[k], err_m })
    }
}

/// How far the device can be from the antenna at LTE timing advance `ta`, in metres: the far edge of its step, (ta + 1) steps, rounded up. None past [`TA_MAX`] (not a timing advance).
pub fn ta_radius_m(ta: u32) -> Option<u32> {
    (ta <= TA_MAX).then(|| ((ta as u64 + 1) * TA_STEP_MM).div_ceil(1000) as u32)
}

/// A place in 2^-23 degrees as a unit vector on the sphere.
pub fn unit(lat: i32, lon: i32) -> (f64, f64, f64) {
    let to_rad = core::f64::consts::PI / (180. * DEGREE as f64);
    let (la, lo) = (lat as f64 * to_rad, lon as f64 * to_rad);
    (libm::cos(la) * libm::cos(lo), libm::cos(la) * libm::sin(lo), libm::sin(la))
}

/// The great-circle distance between two places in metres (the angle from the cross and dot products, well conditioned at any distance).
pub fn distance_m(a: (i32, i32), b: (i32, i32)) -> f64 {
    let (p, q) = (unit(a.0, a.1), unit(b.0, b.1));
    let cross = (p.1 * q.2 - p.2 * q.1, p.2 * q.0 - p.0 * q.2, p.0 * q.1 - p.1 * q.0);
    let sin = libm::sqrt(cross.0 * cross.0 + cross.1 * cross.1 + cross.2 * cross.2);
    let cos = p.0 * q.0 + p.1 * q.1 + p.2 * q.2;
    libm::atan2(sin, cos) * EARTH_M
}
