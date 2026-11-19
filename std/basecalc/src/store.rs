//! What the kist keeps in its own vault domain: one complete VSF document, its settings and every line evaluated, rewritten after each `=` and each change of a setting (AGENT.md "Persistence Rule").
//! A section `basecalc`: `base` (u), `digits` (u), `radians` (u0).
//! Then a section `line` per history line, oldest first: `expr` (x, the expression as typed: the person's own words in basecalc's syntax), `base`, `digits`, `radians` (what it was typed under, so it means the same when it is run again).
//! No result is kept: a result is the engine's to give, and running the lines again in order gives every one, `&` and basecalc's random draws included (`Calc::restore`).

use vsf::file_format::{VsfHeader, VsfSection};
use vsf::VsfType;

/// The vault name of the document, in the kist's own domain.
pub const NAME: &str = "basecalc";

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub expr: String,
    pub base: u8,
    pub digits: usize,
    pub radians: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub base: u8,
    /// The base was chosen over the device's (`base_override`, a flag; absent: following the device's, as every document from before it).
    pub base_overridden: bool,
    pub digits: usize,
    pub radians: bool,
    pub lines: Vec<Line>,
}

/// An unsigned field's value, by its type marker.
fn unsigned(s: &VsfSection, name: &str) -> Option<u64> {
    s.get_field(name)?.values.iter().find_map(|v| match v {
        VsfType::u(n, _) => Some(*n as u64),
        VsfType::u3(n) => Some(*n as u64),
        VsfType::u4(n) => Some(*n as u64),
        VsfType::u5(n) => Some(*n as u64),
        VsfType::u6(n) => Some(*n),
        _ => None,
    })
}

fn flag(s: &VsfSection, name: &str) -> Option<bool> {
    s.get_field(name)?.values.iter().find_map(|v| match v {
        VsfType::u0(b) => Some(*b),
        _ => None,
    })
}

/// The settings a section carries, held to what the engine takes: a base of 2 to 36 and at least one digit (the document is read from the vault, so it is checked like any input).
fn settings(s: &VsfSection) -> Result<(u8, usize, bool), &'static str> {
    let base = unsigned(s, "base").ok_or("no base")?;
    let digits = unsigned(s, "digits").ok_or("no digits")?;
    let radians = flag(s, "radians").ok_or("no angle unit")?;
    if !(2..=36).contains(&base) {
        return Err("a base the engine does not take");
    }
    if digits == 0 {
        return Err("no digits to show");
    }
    Ok((base as u8, digits as usize, radians))
}

impl Saved {
    pub fn parse(doc: &[u8]) -> Result<Saved, &'static str> {
        let (header, end) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
        let sections = header.sections(doc, end).map_err(|_| "sections that do not parse")?;
        let top = sections.iter().find(|s| s.name == "basecalc").ok_or("no basecalc section")?;
        let (base, digits, radians) = settings(top)?;
        let base_overridden = flag(top, "base_override").unwrap_or(false);
        let mut lines = Vec::new();
        for s in sections.iter().filter(|s| s.name == "line") {
            let expr = s.get_field("expr").and_then(|f| f.values.iter().find_map(|v| match v {
                VsfType::x(t) => Some(t.clone()),
                _ => None,
            }));
            let expr = expr.ok_or("a line without its expression")?;
            let (base, digits, radians) = settings(s)?;
            lines.push(Line { expr, base, digits, radians });
        }
        Ok(Saved { base, base_overridden, digits, radians, lines })
    }

    /// As a complete VSF document stamped `now` (Eagle Time oscillations), with its provenance hash.
    pub fn build(&self, now: i64) -> Result<Vec<u8>, String> {
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only();
        let mut top = VsfSection::new("basecalc");
        top.add_field("base", VsfType::u(self.base as usize, false));
        top.add_field("digits", VsfType::u(self.digits, false));
        top.add_field("radians", VsfType::u0(self.radians));
        if self.base_overridden {
            top.add_field("base_override", VsfType::u0(true));
        }
        b = b.add_section_direct(top);
        for line in &self.lines {
            let mut s = VsfSection::new("line");
            s.add_field("expr", VsfType::x(line.expr.clone()));
            s.add_field("base", VsfType::u(line.base as usize, false));
            s.add_field("digits", VsfType::u(line.digits, false));
            s.add_field("radians", VsfType::u0(line.radians));
            b = b.add_section_direct(s);
        }
        b.build()
    }
}
