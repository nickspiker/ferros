//! Stations as VSF (feature `vsf`, needs a heap): a station's harmonic constants in a document of their own, and a catalog of stations for finding the nearest one.
//! tools/tide-station writes both from a source's published tables (NOAA CO-OPS's JSON, converted on the host: the device reads VSF only); the tide puck reads them.
//!
//! **A station document** is a complete VSF file (Eagle-stamped, its provenance hash) with:
//! - one section `station`: `id` (a, the source's own id), `name` (a), `position` (w, the station's place, Dymaxion), `datum` (a, what the heights are above: "MLLW" for NOAA), `msl_um` (i, mean sea level above that datum, micrometres), `source` (a, where the constants were published) and `licence` (a);
//! - a section `constituent` per constituent: `name` (a, the source's spelling, "M2"), `amplitude_um` (u, micrometres) and `phase_q32` (u, the Greenwich phase lag in 2^-32 of a turn).
//!
//! **A catalog** is a complete VSF file with a section `catalog` (`source`, `licence`, `count` u) and a section `station` per station: `id`, `name`, `position`.
//!
//! Every value is read by its field's name and checked for its type marker; a document missing a field, or with a value of another type, is refused whole.

use crate::{Tide, index_of};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::{Ascii, WorldCoord};
use vsf::VsfType;

/// One constituent as a source lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Constituent {
    pub name: String,
    /// Micrometres.
    pub amplitude_um: u64,
    /// The Greenwich phase lag, 2^-32 of a turn.
    pub phase_q32: u64,
}

/// One station's harmonic constants and where they came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub position: WorldCoord,
    pub datum: String,
    /// Mean sea level above `datum`, micrometres.
    pub msl_um: i64,
    pub source: String,
    pub licence: String,
    pub constituents: Vec<Constituent>,
}

/// A turn in phase_q32 units.
const TURN_Q32: f64 = (1u64 << 32) as f64;

/// Degrees (as sources publish phases) to 2^-32 of a turn, in [0, 2^32).
pub fn phase_q32(degrees: f64) -> u64 {
    let t = degrees / 360.;
    let t = t - libm::floor(t);
    // A phase a hair under 360° rounds up to a whole turn: that is 0.
    (libm::round(t * TURN_Q32) as u64) & ((1u64 << 32) - 1)
}

/// Metres to micrometres, rounded.
pub fn um(metres: f64) -> i64 {
    libm::round(metres * 1e6) as i64
}

fn text(what: &str, s: &str) -> Result<VsfType, String> {
    Ascii::new(s).map(VsfType::a).map_err(|_| alloc::format!("{what} {s:?} is not ASCII (a station's text is read by a no_std puck)"))
}

impl Station {
    /// The engine's model of this station, and the constituents it does not know (their names), which are left out.
    pub fn tide(&self) -> (Tide, Vec<String>) {
        let mut t = Tide::flat(self.msl_um as f64 * 1e-6);
        let mut unknown = Vec::new();
        for c in &self.constituents {
            match index_of(&c.name) {
                Some(k) => t.set(k, c.amplitude_um as f64 * 1e-6, c.phase_q32 as f64 * (360. / TURN_Q32)),
                None => unknown.push(c.name.clone()),
            }
        }
        (t, unknown)
    }

    fn section(&self) -> Result<VsfSection, String> {
        let mut s = VsfSection::new("station");
        s.add_field("id", text("id", &self.id)?);
        s.add_field("name", text("name", &self.name)?);
        s.add_field("position", VsfType::w(self.position));
        s.add_field("datum", text("datum", &self.datum)?);
        s.add_field("msl_um", VsfType::i(self.msl_um as isize));
        s.add_field("source", text("source", &self.source)?);
        s.add_field("licence", text("licence", &self.licence)?);
        Ok(s)
    }

    /// The station as a complete VSF document stamped `made` (Eagle oscillations).
    pub fn build(&self, made: i64) -> Result<Vec<u8>, String> {
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(self.section()?);
        for c in &self.constituents {
            let mut s = VsfSection::new("constituent");
            s.add_field("name", text("constituent", &c.name)?);
            s.add_field("amplitude_um", VsfType::u(c.amplitude_um as usize, false));
            s.add_field("phase_q32", VsfType::u(c.phase_q32 as usize, false));
            b = b.add_section_direct(s);
        }
        b.build()
    }

    /// A station document read back: the station and when it was made (Eagle oscillations).
    pub fn parse(doc: &[u8]) -> Result<(Station, i64), &'static str> {
        let (header, end) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
        let made = header.creation_time.as_ref().and_then(|t| t.as_eagle()).ok_or("no creation time")?;
        let sections = header.sections(doc, end).map_err(|_| "sections that do not parse")?;
        let mut st = sections.iter().filter(|s| s.name == "station");
        let s = st.next().ok_or("no station section")?;
        if st.next().is_some() {
            return Err("two station sections");
        }
        let mut station = Station {
            id: ascii(s, "id").ok_or("a station without its id")?,
            name: ascii(s, "name").ok_or("a station without its name")?,
            position: position(s).ok_or("a station without its position")?,
            datum: ascii(s, "datum").ok_or("a station without its datum")?,
            msl_um: value(s, "msl_um", |v| match v {
                VsfType::i(_) | VsfType::i3(_) | VsfType::i4(_) | VsfType::i5(_) | VsfType::i6(_) => v.as_i64(),
                _ => None,
            })
            .ok_or("a station without mean sea level")?,
            source: ascii(s, "source").ok_or("a station without its source")?,
            licence: ascii(s, "licence").ok_or("a station without its licence")?,
            constituents: Vec::new(),
        };
        for c in sections.iter().filter(|s| s.name == "constituent") {
            let name = ascii(c, "name").ok_or("a constituent without its name")?;
            if station.constituents.iter().any(|x| x.name.eq_ignore_ascii_case(&name)) {
                return Err("a constituent listed twice");
            }
            let phase = unsigned(c, "phase_q32").ok_or("a constituent without its phase")?;
            if phase >> 32 != 0 {
                return Err("a phase past a whole turn");
            }
            station.constituents.push(Constituent { name, amplitude_um: unsigned(c, "amplitude_um").ok_or("a constituent without its amplitude")?, phase_q32: phase });
        }
        Ok((station, made))
    }
}

/// A catalog entry: which station, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub position: WorldCoord,
}

/// The stations' catalog: where the list came from, and each station.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    pub source: String,
    pub licence: String,
    pub stations: Vec<Entry>,
}

impl Catalog {
    /// The catalog as a complete VSF document stamped `made` (Eagle oscillations).
    pub fn build(&self, made: i64) -> Result<Vec<u8>, String> {
        let mut c = VsfSection::new("catalog");
        c.add_field("source", text("source", &self.source)?);
        c.add_field("licence", text("licence", &self.licence)?);
        c.add_field("count", VsfType::u(self.stations.len(), false));
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(c);
        for e in &self.stations {
            let mut s = VsfSection::new("station");
            s.add_field("id", text("id", &e.id)?);
            s.add_field("name", text("name", &e.name)?);
            s.add_field("position", VsfType::w(e.position));
            b = b.add_section_direct(s);
        }
        b.build()
    }

    /// A catalog read back. Its `count` must be the stations it holds, and no id is listed twice.
    pub fn parse(doc: &[u8]) -> Result<Catalog, &'static str> {
        let (header, end) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
        let sections = header.sections(doc, end).map_err(|_| "sections that do not parse")?;
        let c = sections.iter().find(|s| s.name == "catalog").ok_or("no catalog section")?;
        let count = unsigned(c, "count").ok_or("a catalog without its count")?;
        let mut stations: Vec<Entry> = Vec::new();
        for s in sections.iter().filter(|s| s.name == "station") {
            let e = Entry { id: ascii(s, "id").ok_or("a station without its id")?, name: ascii(s, "name").ok_or("a station without its name")?, position: position(s).ok_or("a station without its position")? };
            if stations.iter().any(|x| x.id == e.id) {
                return Err("a station listed twice");
            }
            stations.push(e);
        }
        if stations.len() as u64 != count {
            return Err("a catalog whose count is not its stations");
        }
        Ok(Catalog { source: ascii(c, "source").ok_or("a catalog without its source")?, licence: ascii(c, "licence").ok_or("a catalog without its licence")?, stations })
    }

    /// The station nearest a place (unit vector on the sphere), by the largest dot product (the smallest great-circle angle): its index, and the angle's cosine. None for an empty catalog.
    pub fn nearest(&self, at: (f64, f64, f64)) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (k, e) in self.stations.iter().enumerate() {
            let (x, y, z) = e.position.to_xyz();
            let d = x * at.0 + y * at.1 + z * at.2;
            if best.is_none_or(|(_, b)| d > b) {
                best = Some((k, d));
            }
        }
        best
    }
}

/// A place on the unit sphere from latitude and longitude in degrees (the GPS fix's, kept on the device).
pub fn unit(lat_deg: f64, lon_deg: f64) -> (f64, f64, f64) {
    let (la, lo) = (lat_deg * (core::f64::consts::PI / 180.), lon_deg * (core::f64::consts::PI / 180.));
    (libm::cos(la) * libm::cos(lo), libm::cos(la) * libm::sin(lo), libm::sin(la))
}

/// A field's first value of the wanted type (AGENT.md: the type marker says what a value is).
fn value<'a, T>(s: &'a VsfSection, name: &str, f: impl Fn(&'a VsfType) -> Option<T>) -> Option<T> {
    s.get_field(name)?.values.iter().find_map(f)
}

fn ascii(s: &VsfSection, name: &str) -> Option<String> {
    value(s, name, |v| match v {
        VsfType::a(t) => Some(t.as_str().to_string()),
        _ => None,
    })
}

fn unsigned(s: &VsfSection, name: &str) -> Option<u64> {
    value(s, name, |v| match v {
        VsfType::u(..) | VsfType::u0(_) | VsfType::u3(_) | VsfType::u4(_) | VsfType::u5(_) | VsfType::u6(_) => v.as_u64(),
        _ => None,
    })
}

fn position(s: &VsfSection) -> Option<WorldCoord> {
    value(s, "position", |v| match v {
        VsfType::w(p) => Some(*p),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sample() -> Station {
        Station {
            id: "9414290".to_string(),
            name: "San Francisco".to_string(),
            position: WorldCoord::from_lat_lon(37.806305, -122.46589),
            datum: "MLLW".to_string(),
            msl_um: um(2.773 - 1.822),
            source: "NOAA CO-OPS".to_string(),
            licence: "US public domain".to_string(),
            constituents: vec![
                Constituent { name: "M2".to_string(), amplitude_um: 576_000, phase_q32: phase_q32(208.2) },
                Constituent { name: "K1".to_string(), amplitude_um: 370_000, phase_q32: phase_q32(225.4) },
                Constituent { name: "XX9".to_string(), amplitude_um: 1_000, phase_q32: phase_q32(359.99999999) },
            ],
        }
    }

    #[test]
    fn a_station_round_trips() {
        let s = sample();
        let doc = s.build(7).unwrap();
        let (back, made) = Station::parse(&doc).unwrap();
        assert_eq!((back, made), (s.clone(), 7));
        let (tide, unknown) = s.tide();
        assert_eq!(unknown, vec!["XX9".to_string()]);
        assert!((tide.amp[0] - 0.576).abs() < 1e-9 && (tide.phase[0] - 208.2_f64.to_radians()).abs() < 1e-8);
        assert!((tide.z0 - 0.951).abs() < 1e-9);
        // A phase a hair under a whole turn is 0, never 2^32.
        assert_eq!(s.constituents[2].phase_q32, 0);
    }

    #[test]
    fn a_damaged_station_is_refused() {
        let mut s = sample();
        s.constituents.push(s.constituents[0].clone());
        assert_eq!(Station::parse(&s.build(7).unwrap()), Err("a constituent listed twice"));
        s.name = "Fl\u{f8}r\u{f8}".to_string();
        assert!(s.build(7).is_err());
    }

    #[test]
    fn the_catalog_finds_the_nearest() {
        let at = |id: &str, lat: f64, lon: f64| Entry { id: id.to_string(), name: id.to_string(), position: WorldCoord::from_lat_lon(lat, lon) };
        let c = Catalog { source: "test".to_string(), licence: "test".to_string(), stations: vec![at("a", 37.8, -122.5), at("b", 47.6, -122.3), at("c", 21.3, -157.9), at("d", 40.7, -74.0)] };
        let back = Catalog::parse(&c.build(9).unwrap()).unwrap();
        assert_eq!(back, c);
        // Across the antimeridian and near the pole too: the sphere has no seams.
        assert_eq!(back.nearest(unit(48.4, -123.4)).unwrap().0, 1);
        assert_eq!(back.nearest(unit(20.9, -156.5)).unwrap().0, 2);
        assert_eq!(back.nearest(unit(41.5, -71.3)).unwrap().0, 3);
        assert!(Catalog { stations: vec![], ..c.clone() }.nearest(unit(0., 0.)).is_none());
    }
}
