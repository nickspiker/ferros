//! Which zone the clock keeps, and where it comes from (owner, 2026-11-10: "Proper zone too, cellular, then GPS with regions ideally or whatever the standard practice is there. and it's per identity, you can uncheck and set per device tho"; then "no reason not to knock out the fleet preference, auto/manual zone, picker, all that"). docs/TIME.md.
//!
//! - **Two choices.** The identity's (written here by its identity service, payloads/fgtw, from the person's space: settings flow identity to device only) and this device's own. Each is automatic (the zone found here) or by hand (an IANA id; none, the fixed offset `tz`). The device follows the identity's while "follow my identity" is on (the default) and the identity has said something; else its own.
//! - **Found here**, when the choice is automatic: the cellular network's word first (NITZ's offset and daylight flag, its MCC's country), then a GPS position; ferros_tz::find names the zone, from the `zones` document (tools/tzvsf). Until something is found, the zone by hand stands in.
//! - **Pushed, never polled.** The modem hands each NITZ and each MCC in (SYS_ZONE op 0) and the GPS driver each new position (op 1), each thru a narrow right of its own (the caller's name, as SYS_STATUS), never the settings right; the service loop's next pass finds the zone (`step`).
//! - **Kept** in the device settings document (settings.rs): the device's choice, the follow switch, the identity's choice as last written, and the zone last found, so a boot has its zone before any network.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// The longest IANA id held (tzdata's longest is 30 bytes).
pub const ID_MAX: usize = 1 << 6;

/// An IANA id, or none (length 0).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Id {
    b: [u8; ID_MAX],
    n: u8,
}

impl Id {
    pub const NONE: Id = Id { b: [0; ID_MAX], n: 0 };

    /// An id from its bytes: printable ASCII without spaces, at most ID_MAX bytes (empty: none). None for anything else.
    pub fn new(s: &[u8]) -> Option<Id> {
        // WHY: the bytes come from processes and the vault, and the id is copied into ID_MAX bytes.
        if s.len() > ID_MAX || !s.iter().all(|c| c.is_ascii_graphic()) {
            return None;
        }
        let mut id = Id::NONE;
        id.b[..s.len()].copy_from_slice(s);
        id.n = s.len() as u8;
        Some(id)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.b[..self.n as usize]
    }

    pub fn is_none(&self) -> bool {
        self.n == 0
    }
}

/// One side's choice: automatic (the zone found here; `id` stands in until one is) or by hand (`id`; none, the fixed offset).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub auto: bool,
    pub id: Id,
}

/// What found the zone (`found_by`), kept and shown.
pub const BY_NONE: u8 = 0;
pub const BY_NETWORK: u8 = 1;
pub const BY_POSITION: u8 = 2;

/// Where the zone in effect comes from (`source`): a fixed offset, a zone by hand, or found by the network or a position.
pub const FROM_OFFSET: u8 = 0;
pub const FROM_HAND: u8 = 1;
pub const FROM_NETWORK: u8 = 2;
pub const FROM_POSITION: u8 = 3;

pub struct State {
    pub device: Choice,
    /// The identity's, as its service last wrote it; None until it has.
    pub identity: Option<Choice>,
    pub follow: bool,
    pub found: Id,
    pub found_by: u8,
    /// What the radios have said this boot (not kept: a boot hears them again).
    pub clue: ferros_tz::Clue,
    pub mcc: u16,
}

static mut STATE: State = State { device: Choice { auto: true, id: Id::NONE }, identity: None, follow: true, found: Id::NONE, found_by: BY_NONE, clue: ferros_tz::Clue { country: None, offset: None, at: None }, mcc: 0 };
/// A clue came in: the zone is found again at the service loop's next pass.
static CLUE: AtomicBool = AtomicBool::new(false);
/// The `zones` document is to be read again (stored from the bridge, or never read yet).
static TABLE_STALE: AtomicBool = AtomicBool::new(true);

/// A position moved less than this (arc-seconds, latitude and longitude added: 1 << 10 is about 17 minutes of arc, some 30 km) is not a new clue: a fix a second never makes the zone found again for a walk across town.
const MOVED: i64 = 1 << 10;

/// The `zones` document, read once: each zone's id, rule, countries and principal place, and each country's mobile country codes.
struct Table {
    ids: Vec<Vec<u8>>,
    rules: Vec<ferros_tz::Rule>,
    /// Each zone's countries: a run of `countries`.
    runs: Vec<(usize, usize)>,
    at: Vec<(i32, i32)>,
    countries: Vec<[u8; 2]>,
    mcc: Vec<(u16, [u8; 2])>,
}

static mut TABLE: Option<Table> = None;

pub fn state() -> &'static State {
    unsafe { &*(&raw const STATE) }
}

fn state_mut() -> &'static mut State {
    unsafe { &mut *(&raw mut STATE) }
}

/// The choice that rules: the identity's while following it and it has said something, else this device's.
pub fn ruling() -> Choice {
    let s = state();
    match (s.follow, s.identity) {
        (true, Some(c)) => c,
        _ => s.device,
    }
}

/// The zone in effect, and where it comes from.
pub fn effective() -> (Id, u8) {
    let s = state();
    let c = ruling();
    if c.auto && !s.found.is_none() {
        return (s.found, if s.found_by == BY_POSITION { FROM_POSITION } else { FROM_NETWORK });
    }
    (c.id, if c.id.is_none() { FROM_OFFSET } else { FROM_HAND })
}

/// The zone in effect handed to the clock (rtc.rs), when it is not the one it has.
fn apply() {
    let (id, _) = effective();
    if id.as_bytes() != crate::rtc::zone_id() {
        crate::rtc::set_zone(id.as_bytes());
    }
}

/// Loaded from the settings document at boot, or a document stored from the bridge (settings.rs `apply`).
pub fn load(device: Choice, identity: Option<Choice>, follow: bool, found: Id, found_by: u8) {
    let s = state_mut();
    s.device = device;
    s.identity = identity;
    s.follow = follow;
    s.found = found;
    s.found_by = found_by;
    apply();
}

/// The three things a person sets (ops 2-4), each kept in the settings document by `keep` (SYS_CONTROL's store, registered by the vault's owner) and put back when the vault refuses it. With the kernel lock held.
pub fn set_device(c: Choice, keep: impl FnOnce() -> bool) -> bool {
    let was = state().device;
    state_mut().device = c;
    settle(keep, |s| s.device = was)
}

pub fn set_follow(on: bool, keep: impl FnOnce() -> bool) -> bool {
    let was = state().follow;
    state_mut().follow = on;
    settle(keep, |s| s.follow = was)
}

pub fn set_identity(c: Option<Choice>, keep: impl FnOnce() -> bool) -> bool {
    let was = state().identity;
    if was == c {
        return true;
    }
    state_mut().identity = c;
    settle(keep, |s| s.identity = was)
}

fn settle(keep: impl FnOnce() -> bool, undo: impl FnOnce(&mut State)) -> bool {
    if keep() {
        apply();
        if ruling().auto {
            // Automatic now: find the zone from what has been heard.
            CLUE.store(true, Ordering::Release);
        }
        true
    } else {
        undo(state_mut());
        false
    }
}

/// The cellular network's word (op 0, the modem): its MCC (0 none), and from NITZ the legal offset and whether daylight time is in it.
pub fn network(mcc: u16, offset: Option<(i32, Option<bool>)>) {
    let s = state_mut();
    let country = if mcc == 0 { s.clue.country } else { country_of(mcc) };
    if mcc != 0 {
        s.mcc = mcc;
    }
    let clue = ferros_tz::Clue { country, offset: offset.or(s.clue.offset), at: s.clue.at };
    if clue != s.clue {
        s.clue = clue;
        CLUE.store(true, Ordering::Release);
    }
}

/// A new position (op 1, the GPS driver), in 2^-23 degrees: a clue once it has moved past `MOVED` from the last one taken.
pub fn position(lat: i64, lon: i64) {
    // 2^-23 degrees to arc-seconds: 3600 / 2^23.
    let at = ((lat * 3600 >> 23) as i32, (lon * 3600 >> 23) as i32);
    let s = state_mut();
    let moved = s.clue.at.is_none_or(|p| (p.0 as i64 - at.0 as i64).abs() + (p.1 as i64 - at.1 as i64).abs() >= MOVED);
    if moved {
        s.clue.at = Some(at);
        CLUE.store(true, Ordering::Release);
    }
}

/// The country a mobile country code belongs to, from the table (None before it is read: the network's next word, or `step` once it is, finds it).
fn country_of(mcc: u16) -> Option<[u8; 2]> {
    unsafe { (*(&raw const TABLE)).as_ref() }?.mcc.iter().find(|m| m.0 == mcc).map(|m| m.1)
}

/// The `zones` document read again at the next pass (one was stored from the bridge).
pub fn table_stale() {
    TABLE_STALE.store(true, Ordering::Release);
}

/// From the service loop, with the store: the table read when stale, and the zone found when a clue came and the ruling choice is automatic; a zone found that differs from the last is kept and handed to the clock.
pub fn step(store: &mut Option<crate::vault_boot::ShimStore>) {
    // Only with a vault open: before it, the read is not yet possible, and the table stays stale for the pass after.
    if store.is_some() && TABLE_STALE.swap(false, Ordering::AcqRel) {
        let doc = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("zones").ok().flatten()));
        let t = doc.as_deref().and_then(read_table);
        crate::flashlog::line(&match &t {
            Some(t) => alloc::format!("zones: A#{} zones, A#{} mobile country codes", t.ids.len(), t.mcc.len()),
            None if doc.is_some() => alloc::string::String::from("zones: the zones document does not read"),
            None => alloc::string::String::from("zones: no zones document in the vault (tools/tzvsf); zones by id and found ones wait for it"),
        });
        crate::process::with_lock(|| {
            unsafe { TABLE = t };
            // The network's country again, now its MCC can be read.
            let s = state_mut();
            if s.mcc != 0 && s.clue.country.is_none() {
                s.clue.country = country_of(s.mcc);
            }
        });
        CLUE.store(true, Ordering::Release);
        crate::rtc::zone_again();
    }
    if !CLUE.load(Ordering::Acquire) || !ruling().auto {
        return;
    }
    let Some(now) = crate::rtc::utc_now() else { return };
    CLUE.store(false, Ordering::Release);
    let changed = crate::process::with_lock(|| {
        let Some(t) = (unsafe { (*(&raw const TABLE)).as_ref() }) else { return false };
        let zones: Vec<ferros_tz::Zone> = (0..t.ids.len()).map(|i| ferros_tz::Zone { rule: t.rules[i], countries: &t.countries[t.runs[i].0..t.runs[i].0 + t.runs[i].1], at: t.at[i] }).collect();
        let s = state_mut();
        let (found, by) = match ferros_tz::find(&zones, &s.clue, now) {
            Some((i, src)) => (Id::new(&t.ids[i]).unwrap_or(Id::NONE), if src == ferros_tz::Source::Network { BY_NETWORK } else { BY_POSITION }),
            // Nothing named: the zone found before stands, unless the network now says another country, which it is not in.
            None => match s.clue.country {
                Some(c) if !t.ids.iter().position(|id| id.as_slice() == s.found.as_bytes()).is_some_and(|i| zones[i].countries.contains(&c)) => (Id::NONE, BY_NONE),
                _ => (s.found, s.found_by),
            },
        };
        if found == s.found && by == s.found_by {
            return false;
        }
        s.found = found;
        s.found_by = by;
        apply();
        true
    });
    if changed {
        let s = state();
        crate::flashlog::line(&alloc::format!("zone found: {} (by {})", core::str::from_utf8(s.found.as_bytes()).unwrap_or("?"), match s.found_by {
            BY_NETWORK => "the cellular network",
            BY_POSITION => "GPS, the nearest principal place",
            _ => "nothing: none found",
        }));
        let kept = crate::settings::document(None).is_some_and(|d| crate::process::with_lock(|| store.as_mut().is_some_and(|s| s.set(crate::settings::KEY, &d).is_ok())));
        if !kept {
            crate::flashlog::line("zone found: NOT kept (the vault refused the settings document)");
        }
    }
}

/// A zone's rule by its id, from the table (rtc.rs `resolve_zone`); None when it is not there or the table is not read.
pub fn rule_of(id: &[u8]) -> Option<ferros_tz::Rule> {
    let t = unsafe { (*(&raw const TABLE)).as_ref() }?;
    t.ids.iter().position(|z| z.as_slice() == id).map(|i| t.rules[i])
}

pub fn table_read() -> bool {
    unsafe { (*(&raw const TABLE)).is_some() }
}

/// Every zone's id for the picker (op 6): each as its length (a byte) then its bytes, in the document's order (zone1970.tab's: by country, the most populous first). Empty before the table is read.
pub fn list() -> Vec<u8> {
    let mut v = Vec::new();
    if let Some(t) = unsafe { (*(&raw const TABLE)).as_ref() } {
        for id in &t.ids {
            v.push(id.len() as u8);
            v.extend_from_slice(id);
        }
    }
    v
}

/// `diag zone`'s lines.
pub fn describe() -> alloc::string::String {
    let s = state();
    let name = |id: Id| if id.is_none() { alloc::string::String::from("none") } else { alloc::string::String::from(core::str::from_utf8(id.as_bytes()).unwrap_or("?")) };
    let choice = |c: Choice| alloc::format!("{}, by hand {}", if c.auto { "automatic" } else { "by hand" }, name(c.id));
    let (id, from) = effective();
    let mut out = alloc::format!("zone in effect: {} ({})\n", name(id), ["a fixed offset", "by hand", "found by the cellular network", "found by GPS, the nearest principal place"][from as usize]);
    out += &alloc::format!("this device: {}; follows the identity: {}\n", choice(s.device), if s.follow { "yes" } else { "no" });
    out += &alloc::format!("the identity: {}\n", s.identity.map_or(alloc::string::String::from("nothing written here yet"), choice));
    out += &alloc::format!("found: {} (by {})\n", name(s.found), ["nothing", "the network", "a position"][s.found_by as usize]);
    let c = s.clue;
    out += &alloc::format!("heard: MCC A#{}, country {}, offset {}, position {}\n", s.mcc, c.country.map_or(alloc::string::String::from("-"), |c| alloc::string::String::from(core::str::from_utf8(&c).unwrap_or("?"))), c.offset.map_or(alloc::string::String::from("-"), |(o, d)| alloc::format!("A#{o} s east{}", match d {
        Some(true) => ", daylight time",
        Some(false) => ", standard time",
        None => "",
    })), if c.at.is_some() { "known" } else { "-" });
    out += &alloc::format!("zones document: {}\n", if table_read() { "read" } else { "not read" });
    out
}

/// The bytes of op 5 (ferros_sys::zone::State reads them): STATE_LEN.
pub const STATE_LEN: usize = 12 + 4 * (1 + ID_MAX);

pub fn state_bytes() -> [u8; STATE_LEN] {
    let s = state();
    let (id, from) = effective();
    let mut b = [0u8; STATE_LEN];
    b[0] = s.follow as u8;
    b[1] = s.device.auto as u8;
    b[2] = match s.identity {
        None => 0,
        Some(c) if c.auto => 2,
        Some(_) => 1,
    };
    b[3] = from;
    b[4] = s.found_by;
    b[5] = table_read() as u8;
    if let Some(c) = s.clue.country {
        b[6..8].copy_from_slice(&c);
    }
    b[8..10].copy_from_slice(&s.mcc.to_le_bytes());
    let ids = [id, s.device.id, s.identity.map_or(Id::NONE, |c| c.id), s.found];
    for (k, z) in ids.iter().enumerate() {
        let at = 12 + k * (1 + ID_MAX);
        b[at] = z.n;
        b[at + 1..at + 1 + z.n as usize].copy_from_slice(z.as_bytes());
    }
    b
}

/// The `zones` document as a table: its `zone` sections (`id`, `rule`, `country` values, `at` latitude and longitude) and `country` sections (`id`, `mcc` values). A zone whose rule does not parse, or that lacks a field, is left out.
fn read_table(doc: &[u8]) -> Option<Table> {
    use vsf::types::VsfType;
    let (header, end) = vsf::file_format::VsfHeader::decode(doc).ok()?;
    let mut t = Table { ids: Vec::new(), rules: Vec::new(), runs: Vec::new(), at: Vec::new(), countries: Vec::new(), mcc: Vec::new() };
    let two = |v: &VsfType| match v {
        VsfType::a(a) if a.as_bytes().len() == 2 => Some([a.as_bytes()[0], a.as_bytes()[1]]),
        _ => None,
    };
    for s in header.sections(doc, end).ok()? {
        let values = |name: &str| s.get_field(name).map(|f| f.values.clone()).unwrap_or_default();
        let text = |name: &str| values(name).first().and_then(|v| match v {
            VsfType::a(a) => Some(a.as_bytes().to_vec()),
            _ => None,
        });
        match s.name.as_str() {
            "zone" => {
                let (Some(id), Some(rule)) = (text("id"), text("rule")) else { continue };
                let Some(rule) = core::str::from_utf8(&rule).ok().and_then(ferros_tz::Rule::parse) else { continue };
                let at = values("at");
                let (Some(lat), Some(lon)) = (at.first().and_then(|v| v.as_i64()), at.get(1).and_then(|v| v.as_i64())) else { continue };
                // WHY: an id is held in ID_MAX bytes (`Id`); one longer is no zone tzdata has, and the document is the vault's, written from outside.
                if id.len() > ID_MAX {
                    continue;
                }
                let start = t.countries.len();
                t.countries.extend(values("country").iter().filter_map(two));
                t.runs.push((start, t.countries.len() - start));
                t.ids.push(id);
                t.rules.push(rule);
                t.at.push((lat as i32, lon as i32));
            }
            "country" => {
                let Some(c) = text("id").filter(|i| i.len() == 2).map(|i| [i[0], i[1]]) else { continue };
                for m in values("mcc").iter().filter_map(|v| v.as_u64()) {
                    t.mcc.push((m as u16, c));
                }
            }
            _ => {}
        }
    }
    (!t.ids.is_empty()).then_some(t)
}
