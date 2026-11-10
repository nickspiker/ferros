//! Input injection (docs/INPUT-INJECT.md): fingers on the glass and key presses, driven from the bridge in development builds, as a session drives Android with `adb shell input`.
//!
//! **A script is a complete VSF document**: a header with its provenance hash, one section `input` (`events`, how many `event` sections follow, and `text`, what the script does in a person's words), then one section `event` per event, in order. VSF has no sections inside sections; repeated sections of one name, in the order the header's table lists them, are its way of holding a list (ferros_kist's `kist` sections are the same).
//!
//! An `event` section's fields, every one typed and named, nothing read by position:
//! - `kind` (unsigned): `kind::DOWN`, `MOVE`, `UP`, `KEY` or `WAIT`.
//! - `after` (unsigned): how many shown frames after the previous event this one happens (0: in the same frame). A frame is one of the panel's: a frame start, or one refresh period when no frame is in flight (the display's `frame wait`, op 24). Never wall-clock time: motion follows shown frames, so a frame the compositor skips does not slow it down.
//! - `finger` (unsigned, from 0, below `FINGERS`): which finger, for down, move and up.
//! - Where, for down and move, as one of two pairs: `x` and `y`, fractions of the panel's width and height in Q16 (`ONE` is the whole side), so one script drives the Pro and the 8a alike; or `px` and `py`, the panel's own pixels, for one device's exact spot.
//! - `key` (unsigned): `key::POWER`, `VOL_UP` or `VOL_DOWN`, for a key event. A key event is a press, which the kernel takes (development `diag key`); the bridge sends it between the touch parts of a script.
//! - `idle` (a field with no value): on a wait, wait until the display is idle (`IDLE_FRAMES` refresh periods in a row with no frame started) instead of for `after` frames alone.
//!
//! Validation is the same on both ends: the bridge checks what it builds, the inject process checks what it receives (the provenance hash, each section against its schema, strictly, then the order: no finger down twice, none moved or lifted that is not down, every finger up at the end).
#![no_std]

extern crate alloc;

pub use vsf;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use vsf::schema::{FieldSchema, SectionBuilder, SectionSchema, TypeConstraint};
use vsf::{VsfSection, VsfType};

/// The event kinds (the `kind` field).
pub mod kind {
    pub const DOWN: u64 = 0;
    pub const MOVE: u64 = 1;
    pub const UP: u64 = 2;
    pub const KEY: u64 = 3;
    pub const WAIT: u64 = 4;
}

/// The keys (the `key` field).
pub mod key {
    pub const POWER: u64 = 0;
    pub const VOL_UP: u64 = 1;
    pub const VOL_DOWN: u64 = 2;
    pub const NAMES: [&str; 3] = ["power", "volume up", "volume down"];
}

/// How many fingers a script may hold at once. The touch process gives injected finger f the contact slot 15 − f, from the top of its 16 so real fingers (the controller numbers them from 0) rarely meet one.
pub const FINGERS: u64 = 1 << 3;
/// A fraction's whole: `x` = ONE would be the panel's right edge, one past its last pixel; every fraction is below it.
pub const ONE: u64 = 1 << 16;
/// A wait for idle ends after this many refresh periods in a row with no frame started. At rest the display flips at 10 Hz (a frame every 12 periods at 120 Hz); under a finger or a fling it starts one every period or every other.
pub const IDLE_FRAMES: u32 = 1 << 3;

/// Where a finger is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum At {
    /// Fractions of the panel's width and height, Q16.
    Frac(u16, u16),
    /// The panel's pixels.
    Px(u16, u16),
}

impl At {
    /// Panel pixels, for a panel whose largest coordinates are `x_max`, `y_max` (the touch endpoint's INFO). None for pixels off the panel.
    pub fn on_panel(self, x_max: u16, y_max: u16) -> Option<(u16, u16)> {
        match self {
            // A fraction below ONE times the side, shifted down, is below the side: always on the panel.
            At::Frac(x, y) => Some((((x as u64 * (x_max as u64 + 1)) >> 16) as u16, ((y as u64 * (y_max as u64 + 1)) >> 16) as u16)),
            // WHY: a pixel past the panel's largest coordinate is a contact no consumer could have been touched at (fluor hit-tests in these coordinates); a script made for a larger panel is refused rather than clamped onto this one's edge.
            At::Px(x, y) if x <= x_max && y <= y_max => Some((x, y)),
            At::Px(..) => None,
        }
    }
}

/// One event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ev {
    Down { finger: u8, at: At },
    Move { finger: u8, at: At },
    Up { finger: u8 },
    Key { key: u8 },
    Wait { idle: bool },
}

/// An event and when: `after` shown frames after the one before it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Event {
    pub after: u32,
    pub ev: Ev,
}

/// A script: what it does in words, and its events in order.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Script {
    pub text: String,
    pub events: Vec<Event>,
}

/// The `input` section's schema.
pub fn input_schema() -> SectionSchema {
    let mut s = SectionSchema::new("input").description("an input injection script: how many event sections follow, and what it does in words");
    s.fields.push(FieldSchema::new("events", TypeConstraint::AnyUnsigned).required());
    s.fields.push(FieldSchema::new("text", TypeConstraint::AnyString));
    s
}

/// The `event` section's schema.
pub fn event_schema() -> SectionSchema {
    let mut s = SectionSchema::new("event").description("one finger or key event, in shown frames after the event before it");
    s.fields.push(FieldSchema::new("kind", TypeConstraint::AnyUnsigned).required());
    s.fields.push(FieldSchema::new("after", TypeConstraint::AnyUnsigned).required());
    for f in ["finger", "x", "y", "px", "py", "key"] {
        s.fields.push(FieldSchema::new(f, TypeConstraint::AnyUnsigned));
    }
    // A flag: present with no value.
    s.fields.push(FieldSchema::new("idle", TypeConstraint::AnyUnsigned));
    s
}

/// A field's single unsigned value, by its type marker.
fn uint(b: &SectionBuilder, name: &str) -> Option<u64> {
    b.get(name).ok()?.first()?.as_u64()
}

impl Script {
    /// The order a script must keep: fingers below `FINGERS`, none down twice, none moved or lifted that is not down, every one up at the end, a known key, at least one event.
    pub fn check(&self) -> Result<(), String> {
        if self.events.is_empty() {
            return Err(String::from("no events"));
        }
        let mut down = 0u64;
        for (k, e) in self.events.iter().enumerate() {
            match e.ev {
                Ev::Down { finger, .. } | Ev::Move { finger, .. } | Ev::Up { finger } => {
                    // WHY: the finger becomes a contact slot (15 − finger) and a bit of `down`; a finger past FINGERS has neither.
                    if finger as u64 >= FINGERS {
                        return Err(format!("event {k}: finger {finger} (fingers count from 0, below {FINGERS})"));
                    }
                    let held = down >> finger & 1 == 1;
                    match e.ev {
                        Ev::Down { .. } if held => return Err(format!("event {k}: finger {finger} down while it is down")),
                        Ev::Down { .. } => down |= 1 << finger,
                        Ev::Move { .. } if !held => return Err(format!("event {k}: finger {finger} moved while it is up")),
                        Ev::Up { .. } if !held => return Err(format!("event {k}: finger {finger} lifted while it is up")),
                        Ev::Up { .. } => down &= !(1 << finger),
                        _ => {}
                    }
                }
                Ev::Key { key } => {
                    // WHY: the key names a kernel entry; anything else names none.
                    if key as usize >= key::NAMES.len() {
                        return Err(format!("event {k}: no key {key}"));
                    }
                }
                Ev::Wait { .. } => {}
            }
        }
        if down != 0 {
            return Err(format!("fingers still down at the end (mask G#{down:X}): a script lifts every finger it puts down"));
        }
        Ok(())
    }

    /// The script as a complete VSF document stamped `now` (Eagle oscillations), with its provenance hash. Checked first.
    pub fn build(&self, now: i64) -> Result<Vec<u8>, String> {
        self.check()?;
        let mut input = VsfSection::new("input");
        input.add_field("events", VsfType::u(self.events.len(), false));
        if !self.text.is_empty() {
            input.add_field("text", VsfType::a(vsf::Ascii::new(self.text.clone()).map_err(|e| format!("text: {e}"))?));
        }
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_section_direct(input);
        for e in &self.events {
            let mut s = VsfSection::new("event");
            let u = |v: u64| VsfType::u(v as usize, false);
            let (kind, finger, at, key, idle) = match e.ev {
                Ev::Down { finger, at } => (kind::DOWN, Some(finger), Some(at), None, false),
                Ev::Move { finger, at } => (kind::MOVE, Some(finger), Some(at), None, false),
                Ev::Up { finger } => (kind::UP, Some(finger), None, None, false),
                Ev::Key { key } => (kind::KEY, None, None, Some(key), false),
                Ev::Wait { idle } => (kind::WAIT, None, None, None, idle),
            };
            s.add_field("kind", u(kind));
            s.add_field("after", u(e.after as u64));
            if let Some(f) = finger {
                s.add_field("finger", u(f as u64));
            }
            match at {
                Some(At::Frac(x, y)) => {
                    s.add_field("x", u(x as u64));
                    s.add_field("y", u(y as u64));
                }
                Some(At::Px(x, y)) => {
                    s.add_field("px", u(x as u64));
                    s.add_field("py", u(y as u64));
                }
                None => {}
            }
            if let Some(k) = key {
                s.add_field("key", u(k as u64));
            }
            if idle {
                s.add_flag("idle");
            }
            b = b.add_section_direct(s);
        }
        let doc = b.build()?;
        // The document read back as the device will read it: what is sent has passed the same schema.
        let back = Script::parse(&doc)?;
        if back != *self {
            return Err(String::from("the document does not read back as the script it was built from"));
        }
        Ok(doc)
    }

    /// A script from its document: the provenance hash verified, the `input` section and every `event` section parsed strictly against their schemas (an unknown field or a value of the wrong type refuses the whole document), then `check`.
    pub fn parse(doc: &[u8]) -> Result<Script, String> {
        let (header, _) = vsf::verification::read_verified(doc, None)?;
        let body = |f: &vsf::HeaderField| -> Result<&[u8], String> {
            // WHY: offset and size come from the document's own table; a sum past its end (or one that wraps) names bytes that are not there.
            match f.offset_bytes.checked_add(f.size_bytes) {
                Some(end) if end <= doc.len() => Ok(&doc[f.offset_bytes..end]),
                _ => Err(format!("section {} lies past the document's end", f.name)),
            }
        };
        let err = |what: &str, e: vsf::schema::ValidationError| format!("{what}: {e:?}");
        let input = header.fields.iter().find(|f| f.name == "input").ok_or("no input section")?;
        let input = SectionBuilder::parse_strict(input_schema(), body(input)?).map_err(|e| err("input", e))?;
        let count = uint(&input, "events").ok_or("input without its event count")?;
        let text = input.get("text").ok().and_then(|v| v.first()).and_then(|v| v.as_string()).map(String::from).unwrap_or_default();
        let mut events = Vec::new();
        for (k, f) in header.fields.iter().filter(|f| f.name == "event").enumerate() {
            let s = SectionBuilder::parse_strict(event_schema(), body(f)?).map_err(|e| err("event", e))?;
            let need = |name: &str| uint(&s, name).ok_or_else(|| format!("event {k}: no {name}"));
            // WHY: a frame count past u32 is over a year of frames at 120 Hz; refused as malformed rather than cut short.
            let after = u32::try_from(need("after")?).map_err(|_| format!("event {k}: after past u32"))?;
            // WHY: each narrowing below refuses a value that does not fit its field rather than wrapping it into another finger, key or spot.
            let small = |name: &str| -> Result<u8, String> { u8::try_from(need(name)?).map_err(|_| format!("event {k}: {name} out of range")) };
            let coord = |name: &str| -> Result<u16, String> { u16::try_from(need(name)?).map_err(|_| format!("event {k}: {name} out of range")) };
            let at = || -> Result<At, String> {
                match (s.get("x").is_ok(), s.get("px").is_ok()) {
                    (true, false) => Ok(At::Frac(coord("x")?, coord("y")?)),
                    (false, true) => Ok(At::Px(coord("px")?, coord("py")?)),
                    _ => Err(format!("event {k}: a position is x and y (fractions) or px and py (pixels), one pair")),
                }
            };
            let ev = match need("kind")? {
                kind::DOWN => Ev::Down { finger: small("finger")?, at: at()? },
                kind::MOVE => Ev::Move { finger: small("finger")?, at: at()? },
                kind::UP => Ev::Up { finger: small("finger")? },
                kind::KEY => Ev::Key { key: small("key")? },
                kind::WAIT => Ev::Wait { idle: s.get("idle").is_ok() },
                other => return Err(format!("event {k}: no kind {other}")),
            };
            events.push(Event { after, ev });
        }
        if events.len() as u64 != count {
            return Err(format!("{} event sections where the input section says {count}", events.len()));
        }
        let script = Script { text, events };
        script.check()?;
        Ok(script)
    }

    /// The script cut at its key events, for the bridge: touch parts the inject process runs, and keys the kernel takes between them. A key's `after` becomes a wait at the end of the part before it, so it still lands that many frames after the event before it.
    pub fn parts(&self) -> Vec<Part> {
        let mut out = Vec::new();
        let mut cur: Vec<Event> = Vec::new();
        for e in &self.events {
            match e.ev {
                Ev::Key { key } => {
                    if e.after > 0 {
                        cur.push(Event { after: e.after, ev: Ev::Wait { idle: false } });
                    }
                    if !cur.is_empty() {
                        out.push(Part::Touch(Script { text: self.text.clone(), events: core::mem::take(&mut cur) }));
                    }
                    out.push(Part::Key(key));
                }
                _ => cur.push(*e),
            }
        }
        if !cur.is_empty() {
            out.push(Part::Touch(Script { text: self.text.clone(), events: cur }));
        }
        out
    }

    /// Frames from the first event to the last, idle waits counted as none.
    pub fn frames(&self) -> u64 {
        self.events.iter().map(|e| e.after as u64).sum()
    }
}

/// A piece of a script as the bridge sends it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Part {
    Touch(Script),
    Key(u8),
}

/// The `touch` endpoint's injection ops (payloads/touch), development builds only.
pub mod touch {
    /// `[OP_INJECT][count u8][count records]`, a record as the endpoint serves one (12 bytes: id = the finger, phase 0 enter, 1 move, 2 leave, kind, pressure, x, y, major, minor; u16 little-endian) → `[status][injected mask u16 LE]`: the records go into the same table real contacts do, in order.
    pub const OP_INJECT: u8 = 3;
    /// `[OP_INJECTED]` → `[injected mask u16 LE][injected records taken u32 LE]`: which contacts on the glass now are injected (by slot), for diagnostics; consumers never see the difference.
    pub const OP_INJECTED: u8 = 4;
    pub const OK: u8 = 0;
    /// A record that broke the order (a finger entered twice, moved while up) or whose slot a real finger holds.
    pub const REFUSED: u8 = 1;
    pub const REC: usize = 12;
    /// The FTM5's finger type: injected contacts are fingers.
    pub const FINGER_KIND: u8 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    fn tap() -> Script {
        Script {
            text: String::from("tap"),
            events: alloc::vec![
                Event { after: 0, ev: Ev::Down { finger: 0, at: At::Frac(1 << 15, 1 << 15) } },
                Event { after: 8, ev: Ev::Up { finger: 0 } },
                Event { after: 0, ev: Ev::Wait { idle: true } },
            ],
        }
    }

    #[test]
    fn round_trips() {
        let doc = tap().build(7).unwrap();
        assert_eq!(Script::parse(&doc).unwrap(), tap());
        let mut px = tap();
        px.events[0].ev = Ev::Down { finger: 0, at: At::Px(672, 1496) };
        px.events.insert(1, Event { after: 1, ev: Ev::Key { key: key::POWER as u8 } });
        assert_eq!(Script::parse(&px.build(7).unwrap()).unwrap(), px);
    }

    #[test]
    fn refuses_bad_order_and_tampering() {
        let mut s = tap();
        s.events.remove(1);
        assert!(s.build(7).is_err());
        let mut doc = tap().build(7).unwrap();
        let n = doc.len();
        doc[n - 3] ^= 1;
        assert!(Script::parse(&doc).is_err());
    }

    #[test]
    fn refuses_an_unknown_field() {
        let mut input = VsfSection::new("input");
        input.add_field("events", VsfType::u(1, false));
        let mut e = VsfSection::new("event");
        e.add_field("kind", VsfType::u(kind::WAIT as usize, false));
        e.add_field("after", VsfType::u(1, false));
        e.add_field("speed", VsfType::u(1, false));
        let doc = vsf::VsfBuilder::new().provenance_only().add_section_direct(input).add_section_direct(e).build().unwrap();
        assert!(Script::parse(&doc).is_err());
    }

    #[test]
    fn parts_cut_at_keys() {
        let mut s = tap();
        s.events.insert(2, Event { after: 4, ev: Ev::Key { key: 0 } });
        let p = s.parts();
        assert_eq!(p.len(), 3);
        assert_eq!(p[1], Part::Key(0));
        let Part::Touch(a) = &p[0] else { panic!() };
        assert_eq!(a.events.last().unwrap(), &Event { after: 4, ev: Ev::Wait { idle: false } });
    }

    #[test]
    fn fractions_land_on_the_panel() {
        assert_eq!(At::Frac(0xFFFF, 0xFFFF).on_panel(1343, 2991), Some((1343, 2991)));
        assert_eq!(At::Frac(1 << 15, 1 << 15).on_panel(1343, 2991), Some((672, 1496)));
        assert_eq!(At::Px(1344, 0).on_panel(1343, 2991), None);
    }
}
