//! A cell table as VSF (feature `vsf`, needs a heap): one complete VSF file (Eagle-stamped, its provenance hash) with one section `cells`:
//! - `region` (a, what the table covers, as its maker named it: a country, a state; never a place finer than that), `source` (a, where the cells came from), `licence` (a), `kind` (u, [`Kind`]: 0 centroids, 1 sites), `default_range_m` (u) and `count` (u, the rows);
//! - the columns, one typed tensor each, row k one cell, sorted as [`Cells`] needs: `plmn` (t_u5), `cell` (t_u6), `area` (t_u5), `radio` (t_u3), `lat` and `lon` (t_i5, 2^-23 degrees), `range_m` (t_u5).
//!
//! Every value is read by its field's name and checked for its type marker; a table missing a field, with a value of another type, with columns of other lengths than `count`, or out of order, is refused whole.
//! tools/cell-table writes it from a source's export, converted on the host: the device reads VSF only.

use crate::{Cells, Kind, Refused};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::{Ascii, Tensor, VsfType};

/// A table, owned.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub region: String,
    pub source: String,
    pub licence: String,
    pub kind: Kind,
    pub default_range_m: u32,
    pub plmn: Vec<u32>,
    pub cell: Vec<u64>,
    pub area: Vec<u32>,
    pub radio: Vec<u8>,
    pub lat: Vec<i32>,
    pub lon: Vec<i32>,
    pub range_m: Vec<u32>,
}

fn text(what: &str, s: &str) -> Result<VsfType, String> {
    Ascii::new(s).map(VsfType::a).map_err(|_| alloc::format!("{what} {s:?} is not ASCII (a table's text is read by a no_std service)"))
}

impl Table {
    /// The columns, borrowed, for the search.
    pub fn cells(&self) -> Cells<'_> {
        Cells { plmn: &self.plmn, cell: &self.cell, area: &self.area, radio: &self.radio, lat: &self.lat, lon: &self.lon, range_m: &self.range_m, kind: self.kind, default_range_m: self.default_range_m }
    }

    /// The table as a complete VSF document stamped `made` (Eagle oscillations). Refused when its rows would be refused on reading.
    pub fn build(&self, made: i64) -> Result<Vec<u8>, String> {
        self.cells().check().map_err(|e| alloc::format!("the table would be refused: {e:?}"))?;
        let n = self.plmn.len();
        let mut s = VsfSection::new("cells");
        s.add_field("region", text("region", &self.region)?);
        s.add_field("source", text("source", &self.source)?);
        s.add_field("licence", text("licence", &self.licence)?);
        s.add_field("kind", VsfType::u(self.kind as usize, false));
        s.add_field("default_range_m", VsfType::u(self.default_range_m as usize, false));
        s.add_field("count", VsfType::u(n, false));
        s.add_field("plmn", VsfType::t_u5(Tensor::new(vec![n], self.plmn.clone())));
        s.add_field("cell", VsfType::t_u6(Tensor::new(vec![n], self.cell.clone())));
        s.add_field("area", VsfType::t_u5(Tensor::new(vec![n], self.area.clone())));
        s.add_field("radio", VsfType::t_u3(Tensor::new(vec![n], self.radio.clone())));
        s.add_field("lat", VsfType::t_i5(Tensor::new(vec![n], self.lat.clone())));
        s.add_field("lon", VsfType::t_i5(Tensor::new(vec![n], self.lon.clone())));
        s.add_field("range_m", VsfType::t_u5(Tensor::new(vec![n], self.range_m.clone())));
        vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(s).build()
    }

    /// A table read back, and when it was made (Eagle oscillations).
    pub fn parse(doc: &[u8]) -> Result<(Table, i64), &'static str> {
        // A verified read: the bytes are the ones the converter hashed.
        vsf::verification::is_original(doc).map_err(|_| "a table whose provenance hash does not match")?;
        let (header, end) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
        let made = header.creation_time.as_ref().and_then(|t| t.as_eagle()).ok_or("no creation time")?;
        let sections = header.sections(doc, end).map_err(|_| "sections that do not parse")?;
        let mut cs = sections.iter().filter(|s| s.name == "cells");
        let s = cs.next().ok_or("no cells section")?;
        if cs.next().is_some() {
            return Err("two cells sections");
        }
        let count = unsigned(s, "count").ok_or("a table without its count")? as usize;
        let kind = Kind::from_code(unsigned(s, "kind").ok_or("a table without its kind")?).ok_or("a kind of table not known")?;
        let default_range_m = u32::try_from(unsigned(s, "default_range_m").ok_or("a table without its default range")?).map_err(|_| "a default range past 32 bits")?;
        let t = Table {
            region: ascii(s, "region").ok_or("a table without its region")?,
            source: ascii(s, "source").ok_or("a table without its source")?,
            licence: ascii(s, "licence").ok_or("a table without its licence")?,
            kind,
            default_range_m,
            plmn: column(s, "plmn", count, |v| if let VsfType::t_u5(t) = v { Some(&t.data) } else { None })?,
            cell: column(s, "cell", count, |v| if let VsfType::t_u6(t) = v { Some(&t.data) } else { None })?,
            area: column(s, "area", count, |v| if let VsfType::t_u5(t) = v { Some(&t.data) } else { None })?,
            radio: column(s, "radio", count, |v| if let VsfType::t_u3(t) = v { Some(&t.data) } else { None })?,
            lat: column(s, "lat", count, |v| if let VsfType::t_i5(t) = v { Some(&t.data) } else { None })?,
            lon: column(s, "lon", count, |v| if let VsfType::t_i5(t) = v { Some(&t.data) } else { None })?,
            range_m: column(s, "range_m", count, |v| if let VsfType::t_u5(t) = v { Some(&t.data) } else { None })?,
        };
        t.cells().check().map_err(|e| match e {
            Refused::Lengths => "columns of different lengths",
            Refused::Order(_) => "rows out of order",
            Refused::Radio(_) => "a radio not known",
            Refused::Latitude(_) | Refused::Longitude(_) => "a place off the Earth",
        })?;
        Ok((t, made))
    }
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

/// A column of exactly `count` values, from a tensor of its one type.
fn column<'a, T: Clone + 'a>(s: &'a VsfSection, name: &'static str, count: usize, f: impl Fn(&'a VsfType) -> Option<&'a Vec<T>>) -> Result<Vec<T>, &'static str> {
    let c = value(s, name, f).ok_or("a column missing or of another type")?;
    if c.len() != count {
        return Err("a column whose length is not the count");
    }
    Ok(c.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEGREE, Place, Radio, Serving, plmn};

    fn sample() -> Table {
        let t = plmn(1, 1);
        Table {
            region: "Test network".to_string(),
            source: "synthetic".to_string(),
            licence: "none: made up for the tests".to_string(),
            kind: Kind::Centroid,
            default_range_m: 1 << 12,
            plmn: vec![t, t, t],
            cell: vec![7, 0x01_2345, 0x02_0000_0001],
            area: vec![10, 10, 12],
            radio: vec![Radio::Gsm.code(), Radio::Lte.code(), Radio::Nr.code()],
            lat: vec![(10 * DEGREE) as i32, 0, (-45 * DEGREE) as i32],
            lon: vec![(20 * DEGREE) as i32, 0, (-90 * DEGREE) as i32],
            range_m: vec![3000, 2000, 0],
        }
    }

    #[test]
    fn a_table_round_trips() {
        let t = sample();
        let doc = t.build(7).unwrap();
        let (back, made) = Table::parse(&doc).unwrap();
        assert_eq!((&back, made), (&t, 7));
        let s = Serving { radio: Radio::Nr, plmn: plmn(1, 1), area: Some(12), cell: 0x02_0000_0001, ta: None };
        assert_eq!(back.cells().place(&s), Some(Place { lat: (-45 * DEGREE) as i32, lon: (-90 * DEGREE) as i32, err_m: 1 << 12 }));
    }

    #[test]
    fn a_damaged_table_is_refused() {
        let mut t = sample();
        t.cell.swap(0, 1);
        assert!(t.build(7).is_err());
        let mut t = sample();
        t.lat.pop();
        assert!(t.build(7).is_err());
        // Bytes changed after the build fail the provenance hash or the parse.
        let mut doc = sample().build(7).unwrap();
        let k = doc.len() - 3;
        doc[k] ^= 1;
        assert!(Table::parse(&doc).is_err());
    }
}
