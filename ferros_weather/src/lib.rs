//! The weather puck's documents (docs/PUCKS.md): the ask the device sends fgtw.org and the small forecast it answers, read by field name and type marker. no_std + alloc.
//!
//! **The ask** is one POST to fgtw.org's `/` (plain HTTP, port 80): a complete VSF document (Eagle-stamped, its provenance hash) with one section `weather_ask`:
//! - `row` (i) and `column` (u): the place's weather cell ([`grid`]), rounded on the device before anything leaves it;
//! - `day_start` (e): the device's local midnight, so "today's" high and low are the person's day, not UTC's.
//!
//! **The answer** is a complete VSF document with:
//! - one section `forecast`: `row` (i) and `column` (u), the cell it answers; `issued` (e), when the upstream made the forecast; `next_update` (e), when the upstream has a newer one (its Expires: the puck's deadline to ask again); `high_mk` and `low_mk` (u, millikelvin), the highest and lowest temperature over what is left of the day from `day_start`; `source` and `licence` (a, human words: who made the forecast and on what terms);
//! - one section `now`: `time` (e, the hour the values are for), `temperature_mk` (u, millikelvin), `symbol` (u, a [`Symbol`]), `wind_mm_s` (u, millimetres a second), `wind_from_q16` (u, where the wind blows from, 2^-16 of a turn clockwise from true north, below 2^16), `precipitation_um_h` (u, micrometres an hour), `cloud_q16` (u, the cloud-covered fraction of the sky in 2^-16, at most 2^16);
//! - up to 2^4 sections `hour`, in time order, each after `now`: `time` (e, the hour's start), `temperature_mk` (u), `symbol` (u), `precipitation_um` (u, over the hour) and, where the upstream gives it, `probability_q16` (u, the chance of precipitation in 2^-16, at most 2^16).
//!
//! A refusal is a document with a section `error` (`reason`, `detail`, both a), fgtw.org's one failure frame.
//! Every value is read by its field's name and checked for its type marker; a document whose provenance hash does not match, or that misses a field or holds a value of another type, is refused whole.

#![no_std]

extern crate alloc;

pub mod grid;
#[cfg(test)]
mod tests;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::types::EtType;
use vsf::{Ascii, VsfType};

pub use grid::Cell;

/// The section name fgtw.org routes the ask by.
pub const ASK: &str = "weather_ask";

/// The most `hour` sections an answer carries: 2^4.
pub const MAX_HOURS: usize = 1 << 4;

/// A whole in the `_q16` fractions: 2^16.
pub const Q16: u64 = 1 << 16;

/// The weather, as a small enumerated set the face draws (fgtw.org maps the upstream's symbols onto it; the numbers are the wire's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Symbol {
    Clear = 0,
    Fair = 1,
    PartlyCloudy = 2,
    Cloudy = 3,
    Fog = 4,
    /// Rain showers of any strength.
    Showers = 5,
    LightRain = 6,
    Rain = 7,
    HeavyRain = 8,
    /// Sleet or sleet showers of any strength.
    Sleet = 9,
    SnowShowers = 10,
    LightSnow = 11,
    Snow = 12,
    HeavySnow = 13,
    /// Thunder with anything.
    Thunder = 14,
    /// The upstream sent a symbol fgtw.org does not know.
    Unknown = 15,
}

impl Symbol {
    /// The symbol a wire number names; None past the set.
    pub fn from_wire(n: u64) -> Option<Symbol> {
        use Symbol::*;
        const ALL: [Symbol; 16] = [Clear, Fair, PartlyCloudy, Cloudy, Fog, Showers, LightRain, Rain, HeavyRain, Sleet, SnowShowers, LightSnow, Snow, HeavySnow, Thunder, Unknown];
        // A symbol is network input: a number past the set is a document from a newer worker or a damaged one, refused rather than drawn as something else.
        ALL.get(n as usize).copied()
    }
}

/// What the device asks: its cell and when its day began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    pub cell: Cell,
    /// The device's local midnight, Eagle oscillations.
    pub day_start: i64,
}

/// The weather at the hour the forecast calls now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    /// Eagle oscillations.
    pub time: i64,
    pub temperature_mk: u64,
    pub symbol: Symbol,
    pub wind_mm_s: u64,
    /// Where the wind blows from, 2^-16 of a turn clockwise from true north.
    pub wind_from_q16: u64,
    pub precipitation_um_h: u64,
    /// The cloud-covered sky, 2^-16.
    pub cloud_q16: u64,
}

/// One hour ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hour {
    /// The hour's start, Eagle oscillations.
    pub time: i64,
    pub temperature_mk: u64,
    pub symbol: Symbol,
    /// Over the hour.
    pub precipitation_um: u64,
    /// The chance of precipitation, 2^-16, where the upstream gives it.
    pub probability_q16: Option<u64>,
}

/// A forecast for one cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forecast {
    pub cell: Cell,
    pub now: Now,
    /// In time order, each after `now`, at most [`MAX_HOURS`].
    pub hours: Vec<Hour>,
    pub high_mk: u64,
    pub low_mk: u64,
    /// When the upstream made it, Eagle oscillations.
    pub issued: i64,
    /// When the upstream has a newer one, Eagle oscillations.
    pub next_update: i64,
    pub source: String,
    pub licence: String,
}

impl Ask {
    /// The ask as a complete VSF document stamped `made` (Eagle oscillations).
    pub fn build(&self, made: i64) -> Result<Vec<u8>, String> {
        let mut s = VsfSection::new(ASK);
        s.add_field("row", VsfType::i(self.cell.row as isize));
        s.add_field("column", VsfType::u(self.cell.column as usize, false));
        s.add_field("day_start", time(self.day_start));
        vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(s).build()
    }

    /// An ask read back (what fgtw.org does with it), refused unless it names a real cell.
    pub fn parse(doc: &[u8]) -> Result<Ask, &'static str> {
        let sections = verified(doc)?;
        let s = only(&sections, ASK)?;
        let cell = cell(s)?;
        Ok(Ask { cell, day_start: eagle(s, "day_start").ok_or("an ask without its day_start")? })
    }
}

impl Forecast {
    /// The answer as a complete VSF document stamped `made` (Eagle oscillations): what fgtw.org writes, here for the tests and as the schema's other half.
    pub fn build(&self, made: i64) -> Result<Vec<u8>, String> {
        let mut f = VsfSection::new("forecast");
        f.add_field("row", VsfType::i(self.cell.row as isize));
        f.add_field("column", VsfType::u(self.cell.column as usize, false));
        f.add_field("issued", time(self.issued));
        f.add_field("next_update", time(self.next_update));
        f.add_field("high_mk", unsigned_value(self.high_mk));
        f.add_field("low_mk", unsigned_value(self.low_mk));
        f.add_field("source", text(&self.source)?);
        f.add_field("licence", text(&self.licence)?);
        let mut n = VsfSection::new("now");
        n.add_field("time", time(self.now.time));
        n.add_field("temperature_mk", unsigned_value(self.now.temperature_mk));
        n.add_field("symbol", unsigned_value(self.now.symbol as u64));
        n.add_field("wind_mm_s", unsigned_value(self.now.wind_mm_s));
        n.add_field("wind_from_q16", unsigned_value(self.now.wind_from_q16));
        n.add_field("precipitation_um_h", unsigned_value(self.now.precipitation_um_h));
        n.add_field("cloud_q16", unsigned_value(self.now.cloud_q16));
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(f).add_section_direct(n);
        for h in &self.hours {
            let mut s = VsfSection::new("hour");
            s.add_field("time", time(h.time));
            s.add_field("temperature_mk", unsigned_value(h.temperature_mk));
            s.add_field("symbol", unsigned_value(h.symbol as u64));
            s.add_field("precipitation_um", unsigned_value(h.precipitation_um));
            if let Some(p) = h.probability_q16 {
                s.add_field("probability_q16", unsigned_value(p));
            }
            b = b.add_section_direct(s);
        }
        b.build()
    }

    /// fgtw.org's answer read: the forecast, or the reason it is refused.
    /// The caller checks `cell` is the one it asked for.
    pub fn parse(doc: &[u8]) -> Result<Forecast, &'static str> {
        let sections = verified(doc)?;
        if sections.iter().any(|s| s.name == "error") {
            return Err("fgtw.org answered with an error");
        }
        let f = only(&sections, "forecast")?;
        let n = only(&sections, "now")?;
        let now = Now {
            time: eagle(n, "time").ok_or("now without its time")?,
            temperature_mk: unsigned(n, "temperature_mk").ok_or("now without its temperature")?,
            symbol: symbol(n)?,
            wind_mm_s: unsigned(n, "wind_mm_s").ok_or("now without its wind speed")?,
            wind_from_q16: unsigned(n, "wind_from_q16").ok_or("now without its wind direction")?,
            precipitation_um_h: unsigned(n, "precipitation_um_h").ok_or("now without its precipitation")?,
            cloud_q16: unsigned(n, "cloud_q16").ok_or("now without its cloud")?,
        };
        // Network input: a direction is a fraction of a turn below a whole, a fraction of the sky at most a whole; past either the document is damaged.
        if now.wind_from_q16 >= Q16 || now.cloud_q16 > Q16 {
            return Err("now with a fraction past a whole");
        }
        let mut hours: Vec<Hour> = Vec::new();
        for s in sections.iter().filter(|s| s.name == "hour") {
            // Network input: the puck keeps at most MAX_HOURS, so a longer answer is refused before it is held.
            if hours.len() == MAX_HOURS {
                return Err("more hours than an answer carries");
            }
            let h = Hour {
                time: eagle(s, "time").ok_or("an hour without its time")?,
                temperature_mk: unsigned(s, "temperature_mk").ok_or("an hour without its temperature")?,
                symbol: symbol(s)?,
                precipitation_um: unsigned(s, "precipitation_um").ok_or("an hour without its precipitation")?,
                probability_q16: match s.get_field("probability_q16") {
                    None => None,
                    Some(_) => Some(unsigned(s, "probability_q16").ok_or("an hour whose probability is not unsigned")?),
                },
            };
            if h.probability_q16.is_some_and(|p| p > Q16) {
                return Err("an hour with a probability past a whole");
            }
            // next_deadline takes the first hour after now, so the hours must run forward from now.
            let before = hours.last().map_or(now.time, |p| p.time);
            if h.time <= before {
                return Err("hours out of time order");
            }
            hours.push(h);
        }
        Ok(Forecast {
            cell: cell(f)?,
            now,
            hours,
            high_mk: unsigned(f, "high_mk").ok_or("a forecast without its high")?,
            low_mk: unsigned(f, "low_mk").ok_or("a forecast without its low")?,
            issued: eagle(f, "issued").ok_or("a forecast without its issue time")?,
            next_update: eagle(f, "next_update").ok_or("a forecast without its next update")?,
            source: ascii(f, "source").ok_or("a forecast without its source")?,
            licence: ascii(f, "licence").ok_or("a forecast without its licence")?,
        })
    }

    /// When the puck next has something to do: the earliest of the next hour the face shows (the first hour after `now`) and the upstream's next update.
    /// A deadline at or before `now` means ask again now.
    pub fn next_deadline(&self, now: i64) -> i64 {
        match self.hours.iter().map(|h| h.time).find(|&t| t > now) {
            // min is the meaning here, the earlier of two deadlines, not a limit on a value.
            Some(hour) => hour.min(self.next_update),
            None => self.next_update,
        }
    }
}

/// A document's sections, after its provenance hash is checked.
fn verified(doc: &[u8]) -> Result<Vec<VsfSection>, &'static str> {
    vsf::verification::is_original(doc).map_err(|_| "a document whose provenance hash does not match")?;
    let (header, end) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
    header.sections(doc, end).map_err(|_| "sections that do not parse")
}

/// The one section named `name`.
fn only<'a>(sections: &'a [VsfSection], name: &str) -> Result<&'a VsfSection, &'static str> {
    let mut it = sections.iter().filter(|s| s.name == name);
    let s = it.next().ok_or("a section is missing")?;
    if it.next().is_some() {
        return Err("a section given twice");
    }
    Ok(s)
}

fn cell(s: &VsfSection) -> Result<Cell, &'static str> {
    let row = signed(s, "row").ok_or("no row")?;
    let column = unsigned(s, "column").ok_or("no column")?;
    // Network input: a row or column too big for its type names no cell, and is_real then checks the grid.
    let c = Cell { row: i32::try_from(row).map_err(|_| "a row off the grid")?, column: u32::try_from(column).map_err(|_| "a column off the grid")? };
    if !c.is_real() {
        return Err("a cell off the grid");
    }
    Ok(c)
}

fn symbol(s: &VsfSection) -> Result<Symbol, &'static str> {
    Symbol::from_wire(unsigned(s, "symbol").ok_or("no symbol")?).ok_or("a symbol past the set")
}

fn time(t: i64) -> VsfType {
    VsfType::e(EtType::e6(t))
}

fn unsigned_value(n: u64) -> VsfType {
    VsfType::u(n as usize, false)
}

fn text(s: &str) -> Result<VsfType, String> {
    Ascii::new(s).map(VsfType::a).map_err(|_| alloc::format!("{s:?} is not ASCII (the puck reads ASCII)"))
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

fn signed(s: &VsfSection, name: &str) -> Option<i64> {
    value(s, name, |v| match v {
        VsfType::i(_) | VsfType::i3(_) | VsfType::i4(_) | VsfType::i5(_) | VsfType::i6(_) => v.as_i64(),
        // A writer may size a non-negative row as unsigned; the reader does not care which form it chose (vsf's as_i64 doctrine).
        VsfType::u(..) | VsfType::u0(_) | VsfType::u3(_) | VsfType::u4(_) | VsfType::u5(_) | VsfType::u6(_) => v.as_i64(),
        _ => None,
    })
}

fn eagle(s: &VsfSection, name: &str) -> Option<i64> {
    value(s, name, |v| v.as_eagle())
}

/// The place a puck keeps in its own vault domain (docs/PUCKS.md, the location step): the ~10 km cell of its last fix and when that fix was, never anything finer, so the next start has a place without the radio.
/// A complete VSF document, section `place`: `row` (i), `column` (u), `when` (e).
pub const KEPT: &str = "place";

/// The kept place as a document stamped `made`.
pub fn kept_build(cell: Cell, when: i64, made: i64) -> Result<Vec<u8>, String> {
    let mut s = VsfSection::new(KEPT);
    s.add_field("row", VsfType::i(cell.row as isize));
    s.add_field("column", VsfType::u(cell.column as usize, false));
    s.add_field("when", time(when));
    vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(s).build()
}

/// The kept place read back by its fields' names and types, refused whole on anything else.
pub fn kept_parse(doc: &[u8]) -> Result<(Cell, i64), &'static str> {
    let sections = verified(doc)?;
    let s = only(&sections, KEPT)?;
    Ok((cell(s)?, eagle(s, "when").ok_or("a place without its when")?))
}
