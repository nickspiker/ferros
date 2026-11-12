//! The person's identity space on ferros (docs/IDENTITY-SPACE.md; owner, 2026-11-08: "when I attest, the key matches, it syncs with my fleet, pulls a kist roster … and throws it on my home screen in the same exact spot and rotation it was before I factory reset this thing"; "on all my ferros devices the homescreen layout should sync, unless of course I go into settings and uncheck that").
//!
//! The space itself (facts, merge, seal, reconcile) is fgtw's (`fgtw::space`), shared by every client of a fleet. This crate is ferros's vocabulary for it and the sync between it and the device's own copy, the files ferros's programs read and write:
//!
//! | Kind | Id | Value | The device's copy |
//! |---|---|---|---|
//! | `KIST` | developer `ke`, option `hp` | capsule `hb`, name `a`, then per want (slot order) right `u`, reason `a` | a `kist` section of `kists.list` |
//! | `ANSWER` | developer, option, right `u` | granted `u` (1 or 0); the entry's stamp is when | a want's answer in `kists.list` |
//! | `DEFAULT` | right `u` | on `u` | `kists.list`'s `defaults` |
//! | `PLACE` | scope `ke`, then `d` "settings" or developer, option | x, y `i` (world ×256), radius `i`, turn cos, sin `i` (Q14) | `map.place.<label>` (EWE, the map's) |
//! | `VIEW` | scope `ke` | x, y, cos, sin `i` | `map.view` |
//! | `DECK_SYNC` | a device `ke` | on `u` | none: the space's own, set from settings |
//! | `DEPART` | the leaving device `ke` | its signed departure request `v`, or once denied the signed deny | none: the identity service's own (payloads/fgtw), so a member answers it wherever it is |
//! | `ZONE` | the fleet's scope `ke` | automatic `u` (1 or 0), then the IANA id by hand `a` when there is one | none: the identity service writes it to the kernel (SYS_ZONE op 4) on each device, which follows it unless its settings say otherwise |
//! | `LANG` | the fleet's scope `ke` | the language `wl` (its ISO 639-3 code) | none: the identity service writes it to the kernel (SYS_LANG op 2) on each device, which follows it unless its settings say otherwise |
//! | `DND` | the fleet's scope `ke` | on `u` (1 or 0) | none: the identity service writes it to the kernel (preference 40) on each device |
//! | `PIN` | developer `ke` | the bundle's commitment `hp`, then the developer's name `a` when their first kist gave one; the entry's stamp is when it was pinned | a `pin` section of `kists.list` (trust on first install) |
//!
//! The scope of a place and the view is the fleet's (`FLEET`, 32 zero bytes) on every device whose deck sync is on (the default), and the device's own key on one where the person switched it off, so that device keeps an arrangement of its own, which follows it through a reset all the same.
//!
//! Device firmware is never here: it is the device's, re-read from its own partitions.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use ferros_kist::{Kist, Kists, RIGHTS};
use fgtw::space::{reconcile, Baseline, Entry, Fact, Space, Write};
use vsf::VsfType;

pub use fgtw::space;

/// The kinds of fact in the person's space, as ferros writes them (the table above).
pub mod kind {
    pub const KIST: u8 = 0;
    pub const ANSWER: u8 = 1;
    pub const DEFAULT: u8 = 2;
    pub const PLACE: u8 = 3;
    pub const VIEW: u8 = 4;
    pub const DECK_SYNC: u8 = 5;
    /// A member leaving the fleet: its signed departure request, carried by the space to every member wherever it is (owner, 2026-11-09: "broadcast to the devices like a regular message"), approved or denied on any of them, no words (owner, 2026-11-10); once denied, the signed deny in its place; gone once the Remove is published or the device is no longer a member.
    pub const DEPART: u8 = 6;
    /// The person's time zone (owner, 2026-11-10: "it's per identity, you can uncheck and set per device tho"): automatic (each device finds its own from its network, then GPS) or a zone by hand; written to each device that follows it, never read back from one (settings flow identity to device only).
    pub const ZONE: u8 = 7;
    /// The person's language (SETTINGS.md, as the zone: "per identity, you can uncheck and set per device"): chosen on the language page of a device that follows it, written to each device that follows it, never read back from one.
    pub const LANG: u8 = 8;
    /// A developer the person trusted on first install (owner, 2026-11-10): the commitment of the key bundle that developer's first kist carried, so every device of theirs refuses a kist from that key with another bundle. An uninstall keeps it.
    pub const PIN: u8 = 9;
    /// Do not disturb (owner, 2026-11-11: "a fleet/identity preference so it follows the person"): the person's one switch, set from the quick row of any of their devices, written to each device (the kernel's preference 40), never read back from one.
    pub const DND: u8 = 10;
}

/// The scope of the fleet's shared arrangement.
pub const FLEET: [u8; 32] = [0; 32];
/// The map's turn and the place's turn in Q14 (payloads/map `ONE`).
const ONE: i64 = 1 << 14;
/// A vault name in the map's scope is at most 32 bytes, its label at most 26 after `place.` (payloads/map `place_key`).
const LABEL_MAX: usize = 26;

/// The two vault scopes the device's copy lives in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    /// `kists.`: the person's list (ferros_kist).
    Kists,
    /// `map.`: the arrangement and the view (payloads/map).
    Map,
}

/// The device's vault as the sync sees it. A read refused (nobody present, no grant) is an error, never "absent": an absent fact the device once had is a removal, and a sync that took a locked vault for an empty one would remove everything the person has across their fleet.
pub trait Store {
    /// The whole value, Ok(None) when absent, Err when refused.
    fn get(&mut self, scope: Scope, key: &[u8]) -> Result<Option<Vec<u8>>, ()>;
    /// True when the vault took it.
    fn set(&mut self, scope: Scope, key: &[u8], value: &[u8]) -> bool;
    /// True when it is gone (or never was).
    fn remove(&mut self, scope: Scope, key: &[u8]) -> bool;
}

/// Which circle a place is of.
#[derive(Clone, Debug, PartialEq)]
pub enum Circle {
    /// The system's settings circle (the one built in).
    Settings,
    /// A kist's circle, by the kist's identity (developer, option).
    Kist([u8; 32], [u8; 32]),
}

fn k32(v: &VsfType) -> Option<[u8; 32]> {
    v.as_bytes().and_then(|b| <[u8; 32]>::try_from(b).ok())
}

pub fn kist_id(developer: &[u8; 32], option: &[u8; 32]) -> Vec<VsfType> {
    vec![VsfType::ke(developer.to_vec()), VsfType::hp(option.to_vec())]
}

pub fn answer_id(developer: &[u8; 32], option: &[u8; 32], right: u8) -> Vec<VsfType> {
    vec![VsfType::ke(developer.to_vec()), VsfType::hp(option.to_vec()), VsfType::u(right as usize, false)]
}

/// A pin's id: the developer's key.
pub fn pin_id(developer: &[u8; 32]) -> Vec<VsfType> {
    vec![VsfType::ke(developer.to_vec())]
}

/// A pin's fact value: the bundle's commitment, then the developer's name when there is one.
fn pin_value(p: &ferros_kist::Pin) -> Vec<VsfType> {
    let mut v = vec![VsfType::hp(p.bundle.to_vec())];
    if let Some(n) = &p.name {
        v.push(VsfType::a(vsf::Ascii::checked("name", n.clone())));
    }
    v
}

pub fn default_id(right: u8) -> Vec<VsfType> {
    vec![VsfType::u(right as usize, false)]
}

pub fn place_id(scope: &[u8; 32], c: &Circle) -> Vec<VsfType> {
    let mut id = vec![VsfType::ke(scope.to_vec())];
    match c {
        Circle::Settings => id.push(VsfType::d(vsf::Ascii::lit("settings"))),
        Circle::Kist(d, o) => id.extend(kist_id(d, o)),
    }
    id
}

pub fn view_id(scope: &[u8; 32]) -> Vec<VsfType> {
    vec![VsfType::ke(scope.to_vec())]
}

pub fn deck_sync_id(device: &[u8; 32]) -> Vec<VsfType> {
    vec![VsfType::ke(device.to_vec())]
}

/// A departure's id: the leaving device's key.
pub fn depart_id(device: &[u8; 32]) -> Vec<VsfType> {
    vec![VsfType::ke(device.to_vec())]
}

/// `device`'s departure request into the space (written by that device). Whether the space changed.
pub fn put_depart(space: &mut Space, device: &[u8; 32], request: &[u8], now: i64) -> bool {
    let value = vec![VsfType::v(b'b', request.to_vec())];
    if space.get(kind::DEPART, &depart_id(device)).is_some_and(|e| !e.gone && e.value == value) {
        return false;
    }
    let updated = space.get(kind::DEPART, &depart_id(device)).map_or(now, |e| now.max(e.updated + 1));
    space.put(Entry { kind: kind::DEPART, id: depart_id(device), value, updated, by: *device, gone: false })
}

/// The departures in the space: each leaving device and its entry's bytes, its signed request or, once denied, the signed deny (the reader tells them apart by the frame's section name).
pub fn departures(space: &Space) -> Vec<([u8; 32], Vec<u8>)> {
    space.live(kind::DEPART).filter_map(|e| Some((e.id.first().and_then(k32)?, e.value.first()?.as_bytes()?.to_vec()))).collect()
}

/// `device`'s departure denied by `by` (owner, 2026-11-10): the entry's value becomes the signed `depart_deny` frame, so every fleet device that joins the space clears the request and the leaving device learns of it, wherever each is. A new request from the leaving device (a new salt) replaces it, as the newer write. Whether the space changed.
pub fn deny_depart(space: &mut Space, device: &[u8; 32], by: &[u8; 32], deny: &[u8], now: i64) -> bool {
    let value = vec![VsfType::v(b'b', deny.to_vec())];
    let Some(e) = space.get(kind::DEPART, &depart_id(device)) else { return false };
    if e.gone || e.value == value {
        return false;
    }
    let updated = now.max(e.updated + 1);
    space.put(Entry { kind: kind::DEPART, id: depart_id(device), value, updated, by: *by, gone: false })
}

/// `device`'s departure marked gone (by `by`): published, or no longer a member. Whether the space changed.
pub fn drop_depart(space: &mut Space, device: &[u8; 32], by: &[u8; 32], now: i64) -> bool {
    let Some(e) = space.get(kind::DEPART, &depart_id(device)) else { return false };
    if e.gone {
        return false;
    }
    let updated = now.max(e.updated + 1);
    space.put(Entry { kind: kind::DEPART, id: depart_id(device), value: Vec::new(), updated, by: *by, gone: true })
}

/// A place's id read back: (scope, circle).
fn place_of(id: &[VsfType]) -> Option<([u8; 32], Circle)> {
    let scope = k32(id.first()?)?;
    match id.get(1)? {
        VsfType::d(n) if n == "settings" => Some((scope, Circle::Settings)),
        d => Some((scope, Circle::Kist(k32(d)?, k32(id.get(2)?)?))),
    }
}

/// Whether this device's deck follows the fleet's (on until the person switches it off for this device).
pub fn deck_sync(space: &Space, device: &[u8; 32]) -> bool {
    match space.get(kind::DECK_SYNC, &deck_sync_id(device)) {
        Some(e) if !e.gone => e.value.first().and_then(|v| v.as_u64()) != Some(0),
        _ => true,
    }
}

/// The person switched this device's deck sync on or off (settings): written straight into the space, stamped past everything it holds. True when the space changed.
pub fn set_deck_sync(space: &mut Space, device: &[u8; 32], on: bool, now: i64) -> bool {
    if deck_sync(space, device) == on && space.get(kind::DECK_SYNC, &deck_sync_id(device)).is_some() {
        return false;
    }
    // Past the switch's last write, so this one wins the merge whatever the clocks say (a Lamport step: see fgtw::space::reconcile).
    let updated = space.get(kind::DECK_SYNC, &deck_sync_id(device)).map_or(now, |e| now.max(e.updated + 1));
    space.put(Entry { kind: kind::DECK_SYNC, id: deck_sync_id(device), value: vec![VsfType::u(on as usize, false)], updated, by: *device, gone: false })
}

/// The zone's id: the fleet's scope, one choice for the person.
pub fn zone_id() -> Vec<VsfType> {
    vec![VsfType::ke(FLEET.to_vec())]
}

/// The person's zone choice: (automatic, the IANA id by hand, empty for none); None until they have made one.
pub fn zone(space: &Space) -> Option<(bool, String)> {
    let e = space.get(kind::ZONE, &zone_id()).filter(|e| !e.gone)?;
    let auto = e.value.iter().find_map(|v| v.as_u64())? == 1;
    let id = e.value.iter().find_map(|v| v.as_string()).map(String::from).unwrap_or_default();
    Some((auto, id))
}

/// The person chose their zone on one of their devices (its settings, following them): written straight into the space, stamped past its last write so it wins the merge whatever the clocks say. True when the space changed.
pub fn set_zone(space: &mut Space, by: &[u8; 32], auto: bool, id: &str, now: i64) -> bool {
    if zone(space).is_some_and(|z| z.0 == auto && z.1 == id) {
        return false;
    }
    let mut value = vec![VsfType::u(auto as usize, false)];
    if !id.is_empty() {
        value.push(VsfType::a(vsf::Ascii::checked("zone", String::from(id))));
    }
    let updated = space.get(kind::ZONE, &zone_id()).map_or(now, |e| now.max(e.updated + 1));
    space.put(Entry { kind: kind::ZONE, id: zone_id(), value, updated, by: *by, gone: false })
}

/// The language's id: the fleet's scope, one choice for the person.
pub fn language_id() -> Vec<VsfType> {
    vec![VsfType::ke(FLEET.to_vec())]
}

/// The person's language, its ISO 639-3 code; None until they have chosen one.
pub fn language(space: &Space) -> Option<[u8; 3]> {
    let e = space.get(kind::LANG, &language_id()).filter(|e| !e.gone)?;
    e.value.iter().find_map(|v| match v {
        VsfType::wl(l) => Some(l.code()),
        _ => None,
    })
}

/// The person chose their language on one of their devices (its settings, following them): written straight into the space as a vsf `wl`, stamped past its last write so it wins the merge whatever the clocks say. `code` is three lowercase letters (an ISO 639-3 code: fgtw::text::Lang::iso); anything else is refused (false). True when the space changed.
pub fn set_language(space: &mut Space, by: &[u8; 32], code: [u8; 3], now: i64) -> bool {
    // WHY: vsf::Language::new panics on anything but a-z, and the code reaches here from a process's request.
    if !code.iter().all(u8::is_ascii_lowercase) || language(space) == Some(code) {
        return false;
    }
    let updated = space.get(kind::LANG, &language_id()).map_or(now, |e| now.max(e.updated + 1));
    space.put(Entry { kind: kind::LANG, id: language_id(), value: vec![VsfType::wl(vsf::Language::new(code))], updated, by: *by, gone: false })
}

/// Do not disturb's id: the fleet's scope, one switch for the person.
pub fn dnd_id() -> Vec<VsfType> {
    vec![VsfType::ke(FLEET.to_vec())]
}

/// Whether the person has do not disturb on; None until they have ever switched it.
pub fn dnd(space: &Space) -> Option<bool> {
    let e = space.get(kind::DND, &dnd_id()).filter(|e| !e.gone)?;
    e.value.iter().find_map(|v| v.as_u64()).map(|v| v == 1)
}

/// The person switched do not disturb on one of their devices (its quick row): written straight into the space, stamped past its last write so it wins the merge whatever the clocks say. True when the space changed.
pub fn set_dnd(space: &mut Space, by: &[u8; 32], on: bool, now: i64) -> bool {
    if dnd(space) == Some(on) {
        return false;
    }
    let updated = space.get(kind::DND, &dnd_id()).map_or(now, |e| now.max(e.updated + 1));
    space.put(Entry { kind: kind::DND, id: dnd_id(), value: vec![VsfType::u(on as usize, false)], updated, by: *by, gone: false })
}

// ── The map's own forms (payloads/map: `place_key`, `save_place`, `save_view`) ──

/// A circle's vault name in the map's scope: `place.` and its label, a space as an underscore, cut at 26 bytes.
pub fn place_key(label: &str) -> Vec<u8> {
    let mut k = b"place.".to_vec();
    // WHY: the map's names hold 32 bytes; it cuts a longer label the same way, so both name the same entry.
    for &c in label.as_bytes().iter().take(LABEL_MAX) {
        k.push(if c == b' ' { b'_' } else { c });
    }
    k
}

fn zig(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzig(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// EWE integers, as the map keeps its places and view.
fn ewe(values: &[u64]) -> Vec<u8> {
    let mut b = [0u8; 64];
    let mut n = 0;
    for &v in values {
        n += ferros_ledger::ewe::encode_u64(&mut b[n..], v);
    }
    b[..n].to_vec()
}

fn unewe(bytes: &[u8], most: usize) -> Vec<u64> {
    let mut out = Vec::new();
    let mut at = 0;
    while out.len() < most {
        match ferros_ledger::ewe::decode_u64(&bytes[at..]) {
            Some((v, k)) => {
                out.push(v);
                at += k;
            }
            None => break,
        }
    }
    out
}

/// A kept place as the map reads it: x, y (×256), radius, cos, sin (Q14); a place kept before turns existed (three values) stands upright. None for what the map would not take either.
pub fn decode_place(bytes: &[u8]) -> Option<[i64; 5]> {
    let v = unewe(bytes, 5);
    if v.len() < 3 || v[2] == 0 || v[2] >= 1 << 16 {
        return None;
    }
    let (c, s) = if v.len() == 5 { (unzig(v[3]), unzig(v[4])) } else { (ONE, 0) };
    Some([unzig(v[0]), unzig(v[1]), v[2] as i64, c, s])
}

pub fn encode_place(p: &[i64; 5]) -> Vec<u8> {
    ewe(&[zig(p[0]), zig(p[1]), p[2] as u64, zig(p[3]), zig(p[4])])
}

/// The view as the map keeps it: x, y, cos, sin.
pub fn decode_view(bytes: &[u8]) -> Option<[i64; 4]> {
    let v = unewe(bytes, 4);
    if v.len() < 4 {
        return None;
    }
    Some([unzig(v[0]), unzig(v[1]), unzig(v[2]), unzig(v[3])])
}

pub fn encode_view(v: &[i64; 4]) -> Vec<u8> {
    ewe(&[zig(v[0]), zig(v[1]), zig(v[2]), zig(v[3])])
}

fn ints(v: &[i64]) -> Vec<VsfType> {
    v.iter().map(|&x| VsfType::i(x as isize)).collect()
}

fn ints_of<const N: usize>(v: &[VsfType]) -> Option<[i64; N]> {
    if v.len() != N {
        return None;
    }
    let mut out = [0i64; N];
    for (o, x) in out.iter_mut().zip(v) {
        *o = x.as_i64()?;
    }
    Some(out)
}

// ── The device's copy as facts, and back ──

/// A kist's fact value: its capsule, name, and wants in slot order (each right and the developer's reason), then where its capsule was fetched from when the installer fetched it (`nu`, last: a reader from before 2026-11-10 takes no URL for a name or a reason and skips it), then its version, a `z` per part as written (a reader from before takes no `z` for a right, a name or a reason and skips them).
fn kist_value(k: &Kist) -> Vec<VsfType> {
    let mut v = vec![VsfType::hb(k.capsule.to_vec()), VsfType::a(vsf::Ascii::checked("name", k.name.clone()))];
    for w in &k.wants {
        v.push(VsfType::u(w.right as usize, false));
        v.push(VsfType::a(vsf::Ascii::checked("reason", w.reason.clone())));
    }
    if let Some(u) = &k.source {
        v.push(VsfType::nu(u.clone()));
    }
    if let Some(ver) = &k.version {
        v.extend(ver.values());
    }
    v
}

/// A kist fact's version: its `z` values in order, or None when it carries none.
fn version_of_fact(v: &[VsfType]) -> Option<ferros_kist::Version> {
    let parts: Vec<u64> = v
        .iter()
        .filter_map(|x| match x {
            VsfType::z(n) => Some(*n as u64),
            _ => None,
        })
        .collect();
    (!parts.is_empty()).then_some(ferros_kist::Version(parts))
}

/// Where a kist fact's capsule was fetched from, if it says.
fn source_of(v: &[VsfType]) -> Option<String> {
    v.iter().find_map(|x| match x {
        VsfType::nu(u) => Some(u.clone()),
        _ => None,
    })
}

/// A kist fact's value read back: (capsule, name, wants), each value by its type marker and the wants as (right, reason) pairs in order.
fn kist_of(v: &[VsfType]) -> Option<([u8; 32], String, Vec<(u8, String)>)> {
    let capsule = v.iter().find_map(|x| match x {
        VsfType::hb(b) => <[u8; 32]>::try_from(b.as_slice()).ok(),
        _ => None,
    })?;
    let mut name = None;
    let mut wants = Vec::new();
    let mut right: Option<u8> = None;
    for x in v {
        if let Some(n) = x.as_u64() {
            right = u8::try_from(n).ok().filter(|r| (*r as usize) < RIGHTS.len());
        } else if let Some(t) = x.as_string() {
            match right.take() {
                Some(r) => wants.push((r, String::from(t))),
                None if name.is_none() => name = Some(String::from(t)),
                None => {}
            }
        }
    }
    Some((capsule, name?, wants))
}

/// The label a circle has on this device: "settings", or its kist's name in the person's list.
fn label_of<'a>(list: &'a Kists, c: &Circle) -> Option<&'a str> {
    match c {
        Circle::Settings => Some("settings"),
        Circle::Kist(d, o) => list.kists.iter().find(|k| k.developer == *d && k.option == *o).map(|k| k.name.as_str()),
    }
}

/// Every fact the device's copy holds now, in `scope` for the arrangement.
fn snapshot(list: &Kists, store: &mut impl Store, scope: &[u8; 32]) -> Result<Vec<Fact>, ()> {
    let mut facts = Vec::new();
    for k in &list.kists {
        // Stamped with when it was installed: an install after an uninstall (here or on another device) is newer than the uninstall's tombstone and wins it; with no stamp the tombstone took it straight back out.
        facts.push(Fact { kind: kind::KIST, id: kist_id(&k.developer, &k.option), value: kist_value(k), stamp: (k.installed != 0).then_some(k.installed) });
        for w in &k.wants {
            if let Some((granted, when)) = w.answer {
                facts.push(Fact { kind: kind::ANSWER, id: answer_id(&k.developer, &k.option, w.right), value: vec![VsfType::u(granted as usize, false)], stamp: Some(when) });
            }
        }
    }
    for p in &list.pins {
        // No stamp of its own: a device that pinned another bundle for a developer the space already pins takes the space's (the first to reach the person's space stands, as trust on first use wants), rather than the later pin winning by its time. Two pins made apart and merged before either device synced are the space's merge to settle (the later write).
        facts.push(Fact { kind: kind::PIN, id: pin_id(&p.developer), value: pin_value(p), stamp: None });
    }
    for r in 0..RIGHTS.len() as u8 {
        facts.push(Fact { kind: kind::DEFAULT, id: default_id(r), value: vec![VsfType::u((list.defaults >> r & 1) as usize, false)], stamp: None });
    }
    let mut circles = vec![Circle::Settings];
    circles.extend(list.kists.iter().map(|k| Circle::Kist(k.developer, k.option)));
    for c in circles {
        let label = label_of(list, &c).unwrap_or("settings");
        if let Some(p) = store.get(Scope::Map, &place_key(label))?.as_deref().and_then(decode_place) {
            facts.push(Fact { kind: kind::PLACE, id: place_id(scope, &c), value: ints(&p), stamp: None });
        }
    }
    if let Some(v) = store.get(Scope::Map, b"view")?.as_deref().and_then(decode_view) {
        facts.push(Fact { kind: kind::VIEW, id: view_id(scope), value: ints(&v), stamp: None });
    }
    Ok(facts)
}

/// Which of the space's entries this device's copy mirrors: every kist and default; an answer only for a want its kist declares here (an answer to a want an older capsule lacks waits in the space, untouched, for the update that brings it); a place only in this device's scope and only for a circle it has; the view in its scope.
fn managed(list: &Kists, scope: &[u8; 32], k: u8, id: &[VsfType]) -> bool {
    match k {
        kind::KIST | kind::DEFAULT | kind::PIN => true,
        kind::ANSWER => match (id.first().and_then(k32), id.get(1).and_then(k32), id.get(2).and_then(|v| v.as_u64())) {
            (Some(d), Some(o), Some(r)) => list.kists.iter().any(|x| x.developer == d && x.option == o && x.wants.iter().any(|w| w.right as u64 == r)),
            _ => false,
        },
        kind::PLACE => place_of(id).is_some_and(|(s, c)| s == *scope && label_of(list, &c).is_some()),
        kind::VIEW => id.first().and_then(k32) == Some(*scope),
        _ => false,
    }
}

/// What a sync did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outcome {
    /// The space gained a change made on this device: it goes to the fleet.
    pub space_changed: bool,
    /// Writes made to the device's copy from the space: the map reads its places again.
    pub wrote: usize,
    /// The kinds written (bit k: kind k), for the identity service's line: a sync that writes on every pass names what does not stay (2026-11-10: four writes every cycle on the Pro, its copy unchanged).
    pub kinds: u16,
}

/// One write from the space applied to the device's copy (the list in hand; places and the view straight to the vault). False when the vault refused.
fn apply(w: Write, list: &mut Kists, list_changed: &mut bool, store: &mut impl Store) -> bool {
    match w {
        Write::Set { kind: kind::KIST, id, value, .. } => {
            let (Some(d), Some(o), Some((capsule, name, wants))) = (id.first().and_then(k32), id.get(1).and_then(k32), kist_of(&value)) else { return true };
            let old = list.kists.iter().position(|k| k.developer == d && k.option == o);
            // A kist renamed on another device: its circle's kept place moves with its label.
            if let Some(i) = old.filter(|&i| list.kists[i].name != name) {
                let (from, to) = (place_key(&list.kists[i].name), place_key(&name));
                if let Ok(Some(p)) = store.get(Scope::Map, &from) {
                    if !store.set(Scope::Map, &to, &p) || !store.remove(Scope::Map, &from) {
                        return false;
                    }
                }
            }
            // The answers it had here, or kept from an uninstall here (Kists::install); its source the space's. Its install time stays this device's own (none for a kist that only arrived): the stamp matters only to an install made here, which sets it.
            let decl = ferros_kist::Decl { name, wants };
            let i = list.install(capsule, d, o, &decl);
            list.kists[i].source = source_of(&value);
            list.kists[i].version = version_of_fact(&value);
            *list_changed = true;
            true
        }
        Write::Remove { kind: kind::KIST, id } => {
            let (Some(d), Some(o)) = (id.first().and_then(k32), id.get(1).and_then(k32)) else { return true };
            // Uninstalled on another device: gone from this list too, its answers kept here for an install again (Kists::remove).
            list.remove(&d, &o);
            *list_changed = true;
            true
        }
        Write::Set { kind: kind::ANSWER, id, value, updated } => {
            let granted = value.first().and_then(|v| v.as_u64()) == Some(1);
            set_answer(list, &id, Some((granted, updated)));
            *list_changed = true;
            true
        }
        Write::Remove { kind: kind::ANSWER, id } => {
            set_answer(list, &id, None);
            *list_changed = true;
            true
        }
        Write::Set { kind: kind::DEFAULT, id, value, .. } => {
            if let Some(r) = id.first().and_then(|v| v.as_u64()).filter(|r| (*r as usize) < RIGHTS.len()) {
                let on = value.first().and_then(|v| v.as_u64()) == Some(1);
                list.defaults = (list.defaults & !(1 << r)) | ((on as u32) << r);
                *list_changed = true;
            }
            true
        }
        Write::Set { kind: kind::PIN, id, value, updated } => {
            // Each value by its type marker: the commitment `hp`, the name `a`. WHY `try_from`: the length is the space's (another device wrote it); it refuses any but 32 bytes.
            let bundle = value.iter().find_map(|v| match v {
                VsfType::hp(h) => <[u8; 32]>::try_from(h.as_slice()).ok(),
                _ => None,
            });
            let (Some(developer), Some(bundle)) = (id.first().and_then(k32), bundle) else { return true };
            let name = value.iter().find_map(|v| match v {
                VsfType::a(t) => Some(String::from(t.as_str())),
                _ => None,
            });
            let p = ferros_kist::Pin { developer, bundle, name, pinned: updated };
            match list.pins.iter_mut().find(|x| x.developer == developer) {
                Some(x) => *x = p,
                None => list.pins.push(p),
            }
            *list_changed = true;
            true
        }
        Write::Remove { kind: kind::PIN, id } => {
            if let Some(d) = id.first().and_then(k32) {
                list.pins.retain(|p| p.developer != d);
                *list_changed = true;
            }
            true
        }
        Write::Set { kind: kind::PLACE, id, value, .. } => match (place_of(&id), ints_of::<5>(&value)) {
            (Some((_, c)), Some(p)) => match label_of(list, &c) {
                Some(l) => store.set(Scope::Map, &place_key(l), &encode_place(&p)),
                None => true,
            },
            _ => true,
        },
        Write::Remove { kind: kind::PLACE, id } => match place_of(&id).and_then(|(_, c)| label_of(list, &c).map(place_key)) {
            Some(key) => store.remove(Scope::Map, &key),
            None => true,
        },
        Write::Set { kind: kind::VIEW, value, .. } => match ints_of::<4>(&value) {
            Some(v) => store.set(Scope::Map, b"view", &encode_view(&v)),
            None => true,
        },
        Write::Remove { kind: kind::VIEW, .. } => store.remove(Scope::Map, b"view"),
        // A default is never removed (every right has one); other kinds are not the device's copy.
        _ => true,
    }
}

fn set_answer(list: &mut Kists, id: &[VsfType], answer: Option<(bool, i64)>) {
    let (Some(d), Some(o), Some(r)) = (id.first().and_then(k32), id.get(1).and_then(k32), id.get(2).and_then(|v| v.as_u64())) else { return };
    if let Some(w) = list.kists.iter_mut().find(|k| k.developer == d && k.option == o).and_then(|k| k.wants.iter_mut().find(|w| w.right as u64 == r)) {
        w.answer = answer;
    }
}

/// The person's list from the vault: an empty one when none was written; Err when refused or unreadable (never written over).
pub fn read_list(store: &mut impl Store) -> Result<Kists, ()> {
    match store.get(Scope::Kists, ferros_kist::LIST)? {
        None => Ok(Kists::default()),
        Some(doc) => Kists::parse(&doc).map_err(|_| ()),
    }
}

/// Join the device's copy to the space: the person's changes made here go into the space, the space's into the device's copy. A pass at a time until nothing moves: a kist that arrives in one pass has its answers and its circle's place in the next (at most four; a fifth would mean the copy does not keep what it is given). `me` is this device's key, `now` the Eagle time.
///
/// Fails without writing anything more when the vault refuses a read or a write; the baseline is then left as it was before the pass, so the next sync sees the copy as it really is and never mistakes a write that did not land for the person removing something.
pub fn sync(space: &mut Space, base: &mut Baseline, store: &mut impl Store, me: &[u8; 32], now: i64) -> Result<Outcome, &'static str> {
    let start = space.digest();
    let mut wrote = 0;
    let mut kinds = 0u16;
    for _ in 0..4 {
        let mut list = read_list(store).map_err(|_| "the person's list is not readable")?;
        let scope = if deck_sync(space, me) { FLEET } else { *me };
        let facts = snapshot(&list, store, &scope).map_err(|_| "the map's places are not readable")?;
        let before = base.clone();
        let writes = reconcile(space, base, &facts, |k, id| managed(&list, &scope, k, id), *me, now);
        if writes.is_empty() {
            break;
        }
        let mut list_changed = false;
        // Kists before answers and places: an answer or a place names a kist the list must already hold.
        let (first, rest): (Vec<Write>, Vec<Write>) = writes.into_iter().partition(|w| matches!(w, Write::Set { kind: kind::KIST, .. } | Write::Remove { kind: kind::KIST, .. }));
        for w in first.into_iter().chain(rest) {
            // Kinds are 0 to 9 (`kind`): a bit each in a u16.
            kinds |= 1 << match &w { Write::Set { kind, .. } | Write::Remove { kind, .. } => *kind };
            if !apply(w, &mut list, &mut list_changed, store) {
                *base = before;
                return Err("the vault did not take a write");
            }
            wrote += 1;
        }
        if list_changed {
            let doc = list.build(now).map_err(|_| "the person's list does not build")?;
            if !store.set(Scope::Kists, ferros_kist::LIST, &doc) {
                *base = before;
                return Err("the vault did not take the person's list");
            }
        }
    }
    Ok(Outcome { space_changed: space.digest() != start, wrote, kinds })
}

/// The capsules the person's list names that this device lacks (`has` asks the device's `capsules.`), in list order: what the fetch must bring (docs/IDENTITY-SPACE.md "Capsules").
pub fn missing_capsules(list: &Kists, mut has: impl FnMut(&[u8; 32]) -> bool) -> Vec<[u8; 32]> {
    list.kists.iter().map(|k| k.capsule).filter(|c| !has(c)).collect()
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::collections::BTreeMap;

    /// A vault in memory: its two scopes by name. `locked` refuses every read, as a vault with nobody present does.
    #[derive(Default, Clone)]
    struct Vault {
        kists: BTreeMap<Vec<u8>, Vec<u8>>,
        map: BTreeMap<Vec<u8>, Vec<u8>>,
        locked: bool,
    }

    impl Vault {
        fn scope(&mut self, s: Scope) -> &mut BTreeMap<Vec<u8>, Vec<u8>> {
            match s {
                Scope::Kists => &mut self.kists,
                Scope::Map => &mut self.map,
            }
        }
    }

    impl Store for Vault {
        fn get(&mut self, s: Scope, key: &[u8]) -> Result<Option<Vec<u8>>, ()> {
            if self.locked {
                return Err(());
            }
            Ok(self.scope(s).get(key).cloned())
        }
        fn set(&mut self, s: Scope, key: &[u8], value: &[u8]) -> bool {
            self.scope(s).insert(key.to_vec(), value.to_vec());
            true
        }
        fn remove(&mut self, s: Scope, key: &[u8]) -> bool {
            self.scope(s).remove(key);
            true
        }
    }

    const PRO: [u8; 32] = [0x11; 32];
    const FRESH: [u8; 32] = [0x22; 32];
    const OTHER: [u8; 32] = [0x33; 32];
    const MAHERE: ([u8; 32], [u8; 32]) = ([0x4D; 32], [0x01; 32]);
    const PHOTON: ([u8; 32], [u8; 32]) = ([0x50; 32], [0x02; 32]);

    fn decl(name: &str, wants: &[(u8, &str)]) -> ferros_kist::Decl {
        ferros_kist::Decl { name: String::from(name), wants: wants.iter().map(|(r, s)| (*r, String::from(*s))).collect() }
    }

    /// The Pro as the owner had it: Mahere and Photon installed and answered, a default on, every circle moved and turned by hand, the view turned.
    fn arranged_pro() -> Vault {
        let mut l = Kists::default();
        l.defaults = 1 << 7;
        let m = l.install([0xA1; 32], MAHERE.0, MAHERE.1, &decl("Mahere", &[(0, "to draw the map"), (6, "to show where you are")]));
        l.kists[m].wants[0].answer = Some((true, 1000));
        l.kists[m].wants[1].answer = Some((false, 1001));
        let p = l.install([0xA2; 32], PHOTON.0, PHOTON.1, &decl("Photon", &[(0, "to draw"), (7, "to reach your friends")]));
        l.kists[p].wants[0].answer = Some((true, 1002));
        l.kists[p].wants[1].answer = Some((true, 1003));
        let mut v = Vault::default();
        v.set(Scope::Kists, ferros_kist::LIST, &l.build(1004).unwrap());
        v.set(Scope::Map, &place_key("settings"), &encode_place(&[-5 << 8, 7 << 8, 300, 0, ONE]));
        v.set(Scope::Map, &place_key("Mahere"), &encode_place(&[900 << 8, -40 << 8, 180, -ONE, 0]));
        v.set(Scope::Map, &place_key("Photon"), &encode_place(&[-700 << 8, 650 << 8, 220, 11585, -11585]));
        v.set(Scope::Map, b"view", &encode_view(&[12 << 8, -3 << 8, 11585, 11585]));
        v
    }

    /// The device's copy as the person sees it: the list (answers and defaults), every place and the view.
    fn seen(v: &mut Vault) -> (Kists, BTreeMap<Vec<u8>, Vec<u8>>) {
        (read_list(v).unwrap(), v.map.clone())
    }

    /// Sealed on one device, opened on another: the bytes that cross the fleet (or sit at fgtw.org).
    fn hand_over(space: &Space) -> Space {
        let (hp, key) = ([0x77; 32], [0x88; 32]);
        let sealed = fgtw::space::seal(&space.to_doc(5000).unwrap(), &hp, &key).unwrap();
        Space::from_doc(&fgtw::space::open(&sealed, &hp, &key).unwrap()).unwrap()
    }

    /// The end test on the host: arrange, reset, attest. The Pro's arrangement goes into its space; a factory-reset device (an empty vault) takes the sealed space and comes back with the same kists, the same answers (nothing to ask), the same defaults, every circle in the same spot and turn, the same view.
    #[test]
    fn a_reset_device_comes_back_as_it_was() {
        let mut pro = arranged_pro();
        let (mut sp, mut bp) = (Space::new(), Baseline::default());
        let o = sync(&mut sp, &mut bp, &mut pro, &PRO, 2000).unwrap();
        assert!(o.space_changed);
        assert_eq!(o.wrote, 0, "the first device's copy is the space");
        let mut fresh = Vault::default();
        let (mut sf, mut bf) = (hand_over(&sp), Baseline::default());
        let o = sync(&mut sf, &mut bf, &mut fresh, &FRESH, 3000).unwrap();
        assert!(!o.space_changed, "a fresh device adds nothing of its own");
        assert!(o.wrote > 0);
        let (lp, mp) = seen(&mut arranged_pro());
        let (lf, mf) = seen(&mut fresh);
        assert_eq!(lf, lp, "the same kists, answers and defaults");
        assert_eq!(mf, mp, "every place and the view, byte for byte");
        assert!(lf.kists.iter().all(|k| !k.unanswered()), "nothing asked again");
        assert_eq!(missing_capsules(&lf, |_| false), vec![[0xA1; 32], [0xA2; 32]]);
        // Settled: a second sync moves nothing.
        assert_eq!(sync(&mut sf, &mut bf, &mut fresh, &FRESH, 3001).unwrap(), Outcome::default());
    }

    /// Two devices with the sync on: a move on one reaches the other; an answer changed on the other reaches the first; one switched off keeps its own arrangement while kists and answers still follow.
    #[test]
    fn the_fleet_shares_one_deck_unless_a_device_opts_out() {
        let mut a = arranged_pro();
        let (mut sa, mut ba) = (Space::new(), Baseline::default());
        sync(&mut sa, &mut ba, &mut a, &PRO, 2000).unwrap();
        let mut b = Vault::default();
        let (mut sb, mut bb) = (sa.clone(), Baseline::default());
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2100).unwrap();
        // The Pro moves Mahere; the other device revokes Photon's network.
        a.set(Scope::Map, &place_key("Mahere"), &encode_place(&[1 << 8, 2 << 8, 190, 0, ONE]));
        let mut lb = read_list(&mut b).unwrap();
        lb.kists[1].wants[1].answer = Some((false, 2200));
        b.set(Scope::Kists, ferros_kist::LIST, &lb.build(2200).unwrap());
        assert!(sync(&mut sa, &mut ba, &mut a, &PRO, 2300).unwrap().space_changed);
        assert!(sync(&mut sb, &mut bb, &mut b, &OTHER, 2201).unwrap().space_changed);
        sa.merge(hand_over(&sb));
        sb.merge(hand_over(&sa));
        sync(&mut sa, &mut ba, &mut a, &PRO, 2400).unwrap();
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2401).unwrap();
        assert_eq!(seen(&mut a), seen(&mut b));
        assert_eq!(read_list(&mut a).unwrap().kists[1].wants[1].answer, Some((false, 2200)));
        assert_eq!(decode_place(&a.map[&place_key("Mahere")]).unwrap()[0], 1 << 8);
        // The other device switches its deck sync off, then the Pro moves settings: the other keeps its own.
        set_deck_sync(&mut sb, &OTHER, false, 2500);
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2501).unwrap();
        a.set(Scope::Map, &place_key("settings"), &encode_place(&[0, 0, 300, 0, ONE]));
        sync(&mut sa, &mut ba, &mut a, &PRO, 2600).unwrap();
        sa.merge(hand_over(&sb));
        sb.merge(hand_over(&sa));
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2700).unwrap();
        assert_ne!(b.map[&place_key("settings")], a.map[&place_key("settings")], "opted out: its own arrangement");
        assert_eq!(read_list(&mut b).unwrap(), read_list(&mut a).unwrap(), "kists and answers still follow");
        // Its own arrangement is in the space too: reset that device and it comes back to its own, not the fleet's.
        let mut reset = Vault::default();
        let (mut sr, mut br) = (hand_over(&sb), Baseline::default());
        sync(&mut sr, &mut br, &mut reset, &OTHER, 2800).unwrap();
        assert_eq!(reset.map, b.map);
    }

    /// Uninstalled on one device: gone from the other too, the answers kept on both and in the space; installed again later (the installer stamps when), it wins over the uninstall's tombstone and asks nothing anywhere. Its source and version travel with it.
    #[test]
    fn an_uninstall_travels_and_an_install_again_asks_nothing() {
        let mut a = arranged_pro();
        let (mut sa, mut ba) = (Space::new(), Baseline::default());
        sync(&mut sa, &mut ba, &mut a, &PRO, 2000).unwrap();
        let mut b = Vault::default();
        let (mut sb, mut bb) = (sa.clone(), Baseline::default());
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2100).unwrap();
        // The Pro uninstalls Photon.
        let mut la = read_list(&mut a).unwrap();
        assert!(la.remove(&PHOTON.0, &PHOTON.1).is_some());
        a.set(Scope::Kists, ferros_kist::LIST, &la.build(2200).unwrap());
        assert!(sync(&mut sa, &mut ba, &mut a, &PRO, 2201).unwrap().space_changed);
        sb.merge(hand_over(&sa));
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2300).unwrap();
        let lb = read_list(&mut b).unwrap();
        assert!(lb.by_identity(&PHOTON.0, &PHOTON.1).is_none(), "gone on the other device");
        assert_eq!(lb.kept.len(), 1, "its answers kept there");
        assert!(sa.live(kind::ANSWER).any(|e| e.id.first().and_then(k32) == Some(PHOTON.0)), "and in the space");
        // Installed again on the Pro from the directory, after the uninstall: a newer capsule, its source, its install time.
        let mut la = read_list(&mut a).unwrap();
        let p = la.install([0xB2; 32], PHOTON.0, PHOTON.1, &decl("Photon", &[(0, "to draw"), (7, "to reach your friends")]));
        assert!(!la.kists[p].unanswered(), "its answers back from the kept ones");
        la.kists[p].source = Some(String::from("http://kists.fgtw.org/capsules/b2.vsf"));
        la.kists[p].installed = 2400;
        la.kists[p].version = Some(ferros_kist::Version(vec![0, 2, 0]));
        a.set(Scope::Kists, ferros_kist::LIST, &la.build(2400).unwrap());
        sync(&mut sa, &mut ba, &mut a, &PRO, 2401).unwrap();
        assert!(read_list(&mut a).unwrap().by_identity(&PHOTON.0, &PHOTON.1).is_some(), "not taken back out by the tombstone");
        sb.merge(hand_over(&sa));
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2500).unwrap();
        let lb = read_list(&mut b).unwrap();
        let k = lb.by_identity(&PHOTON.0, &PHOTON.1).expect("back on the other device");
        assert_eq!(k.capsule, [0xB2; 32]);
        assert_eq!(k.source.as_deref(), Some("http://kists.fgtw.org/capsules/b2.vsf"));
        assert_eq!(k.version.as_ref().map(|v| v.0.clone()), Some(vec![0, 2, 0]), "its version, as written");
        assert!(!k.unanswered(), "nothing asked on the other device either");
        assert!(lb.kept.is_empty());
        // A fresh device restored from the space: the source comes with the list, for its restore to fetch from.
        let mut fresh = Vault::default();
        let (mut sf, mut bf) = (hand_over(&sa), Baseline::default());
        sync(&mut sf, &mut bf, &mut fresh, &FRESH, 2600).unwrap();
        let lf = read_list(&mut fresh).unwrap();
        assert_eq!(lf.by_identity(&PHOTON.0, &PHOTON.1).and_then(|k| k.source.clone()).as_deref(), Some("http://kists.fgtw.org/capsules/b2.vsf"));
        assert_eq!(missing_capsules(&lf, |c| *c == [0xA1; 32]), vec![[0xB2; 32]]);
        // Settled on every device.
        assert_eq!(sync(&mut sa, &mut ba, &mut a, &PRO, 2700).unwrap().wrote, 0);
        assert_eq!(sync(&mut sb, &mut bb, &mut b, &OTHER, 2700).unwrap().wrote, 0);
    }

    /// Trust on first install follows the person: a developer pinned on one device is pinned on the others and on a reset one; an uninstall keeps it; a device that pinned another bundle for that developer before it synced takes the space's, the first to reach it.
    #[test]
    fn a_pin_travels_and_the_first_one_stands() {
        let mut a = arranged_pro();
        let mut la = read_list(&mut a).unwrap();
        assert_eq!(la.trust(&MAHERE.0, &[0xC1; 32], Some("Mahere's developer"), 1500), ferros_kist::Trust::First);
        a.set(Scope::Kists, ferros_kist::LIST, &la.build(1500).unwrap());
        let (mut sa, mut ba) = (Space::new(), Baseline::default());
        sync(&mut sa, &mut ba, &mut a, &PRO, 2000).unwrap();
        // Another device pinned another bundle for the same key while apart (an attacker's capsule reached it first there).
        let mut b = Vault::default();
        let mut lb = Kists::default();
        lb.trust(&MAHERE.0, &[0xEE; 32], Some("someone"), 1900);
        b.set(Scope::Kists, ferros_kist::LIST, &lb.build(1900).unwrap());
        let (mut sb, mut bb) = (hand_over(&sa), Baseline::default());
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2100).unwrap();
        let lb = read_list(&mut b).unwrap();
        assert_eq!(lb.pin_of(&MAHERE.0).map(|p| p.bundle), Some([0xC1; 32]), "the space's pin stands");
        assert_eq!(lb.pin_of(&MAHERE.0).and_then(|p| p.name.clone()).as_deref(), Some("Mahere's developer"));
        // Uninstalled on the Pro: the pin stays there, in the space and on the other device.
        let mut la = read_list(&mut a).unwrap();
        la.remove(&MAHERE.0, &MAHERE.1);
        a.set(Scope::Kists, ferros_kist::LIST, &la.build(2200).unwrap());
        sync(&mut sa, &mut ba, &mut a, &PRO, 2201).unwrap();
        assert!(read_list(&mut a).unwrap().pin_of(&MAHERE.0).is_some());
        sb.merge(hand_over(&sa));
        sync(&mut sb, &mut bb, &mut b, &OTHER, 2300).unwrap();
        assert_eq!(read_list(&mut b).unwrap().pin_of(&MAHERE.0).map(|p| p.bundle), Some([0xC1; 32]));
        // A reset device comes back with it.
        let mut fresh = Vault::default();
        let (mut sf, mut bf) = (hand_over(&sa), Baseline::default());
        sync(&mut sf, &mut bf, &mut fresh, &FRESH, 2400).unwrap();
        assert_eq!(read_list(&mut fresh).unwrap().pin_of(&MAHERE.0).map(|p| p.bundle), Some([0xC1; 32]));
        // Settled.
        assert_eq!(sync(&mut sf, &mut bf, &mut fresh, &FRESH, 2500).unwrap().wrote, 0);
        assert_eq!(sync(&mut sb, &mut bb, &mut b, &OTHER, 2500).unwrap().wrote, 0);
    }

    /// A vault that will not read (nobody present) syncs nothing and removes nothing.
    #[test]
    fn a_locked_vault_is_never_taken_for_an_empty_one() {
        let mut a = arranged_pro();
        let (mut sa, mut ba) = (Space::new(), Baseline::default());
        sync(&mut sa, &mut ba, &mut a, &PRO, 2000).unwrap();
        let digest = sa.digest();
        a.locked = true;
        assert!(sync(&mut sa, &mut ba, &mut a, &PRO, 2100).is_err());
        assert_eq!(sa.digest(), digest);
        assert!(sa.entries().iter().all(|e| !e.gone));
    }

    /// The map's own form read and written alike: a place kept before turns stands upright; a cut label names the map's entry.
    #[test]
    fn the_maps_forms() {
        let p = [-123456, 789, 4000, -ONE, 3];
        assert_eq!(decode_place(&encode_place(&p)), Some(p));
        assert_eq!(decode_place(&ewe(&[zig(5), zig(-6), 70])), Some([5, -6, 70, ONE, 0]));
        assert_eq!(decode_place(&ewe(&[1, 2, 0])), None);
        assert_eq!(place_key("a very long kist name that goes on"), b"place.a_very_long_kist_name_that".to_vec());
        let v = [1, -2, 3, -4];
        assert_eq!(decode_view(&encode_view(&v)), Some(v));
    }

    /// The person's zone: none until chosen; chosen on one device, it crosses the fleet sealed; the later choice wins even from a device whose clock runs behind; the device's own copy is never touched by the sync (identity to device only, by the identity service).
    #[test]
    fn the_persons_zone_follows_them() {
        let mut sa = Space::new();
        assert_eq!(zone(&sa), None);
        assert!(set_zone(&mut sa, &PRO, false, "Pacific/Auckland", 2000));
        assert!(!set_zone(&mut sa, &PRO, false, "Pacific/Auckland", 2001), "the same choice changes nothing");
        let mut sb = hand_over(&sa);
        assert_eq!(zone(&sb), Some((false, String::from("Pacific/Auckland"))));
        // The other device's clock is behind: its choice is stamped past the last write all the same.
        assert!(set_zone(&mut sb, &OTHER, true, "", 1500));
        sa.merge(hand_over(&sb));
        assert_eq!(zone(&sa), Some((true, String::new())));
        // A sync of the device's copy neither reads nor writes the zone.
        let mut v = arranged_pro();
        let mut b = Baseline::default();
        let o = sync(&mut sa, &mut b, &mut v, &PRO, 3000).unwrap();
        assert_eq!(o.kinds >> kind::ZONE & 1, 0);
        assert_eq!(zone(&sa), Some((true, String::new())));
    }

    /// The person's language, as the zone: none until chosen; chosen on one device, it crosses the fleet sealed as a vsf `wl`; the later choice wins even from a device whose clock runs behind; a code that is not three letters is refused; the device's own copy is never touched by the sync.
    #[test]
    fn the_persons_language_follows_them() {
        let mut sa = Space::new();
        assert_eq!(language(&sa), None);
        assert!(set_language(&mut sa, &PRO, *b"mri", 2000));
        assert!(!set_language(&mut sa, &PRO, *b"mri", 2001), "the same choice changes nothing");
        assert!(!set_language(&mut sa, &PRO, *b"MRI", 2002), "not a code");
        let mut sb = hand_over(&sa);
        assert_eq!(language(&sb), Some(*b"mri"));
        assert!(set_language(&mut sb, &OTHER, *b"spa", 1500));
        sa.merge(hand_over(&sb));
        assert_eq!(language(&sa), Some(*b"spa"));
        let mut v = arranged_pro();
        let mut b = Baseline::default();
        let o = sync(&mut sa, &mut b, &mut v, &PRO, 3000).unwrap();
        assert_eq!(o.kinds >> kind::LANG & 1, 0);
        assert_eq!(language(&sa), Some(*b"spa"));
    }

    /// Do not disturb, as the language: never switched until it is; switched on one device, it crosses the fleet sealed; the later switch wins even from a device whose clock runs behind; the device's own copy is never touched by the sync.
    #[test]
    fn do_not_disturb_follows_the_person() {
        let mut sa = Space::new();
        assert_eq!(dnd(&sa), None);
        assert!(set_dnd(&mut sa, &PRO, true, 2000));
        assert!(!set_dnd(&mut sa, &PRO, true, 2001), "the same switch changes nothing");
        let mut sb = hand_over(&sa);
        assert_eq!(dnd(&sb), Some(true));
        assert!(set_dnd(&mut sb, &OTHER, false, 1500));
        sa.merge(hand_over(&sb));
        assert_eq!(dnd(&sa), Some(false));
        let mut v = arranged_pro();
        let mut b = Baseline::default();
        let o = sync(&mut sa, &mut b, &mut v, &PRO, 3000).unwrap();
        assert_eq!(o.kinds >> kind::DND & 1, 0);
        assert_eq!(dnd(&sa), Some(false));
    }
}

/// The end test on the host with a real device's data: `FERROS_SPACE_DUMP=<dir> cargo test -p ferros_space -- --ignored --nocapture`, the directory holding what `ferros-bridge store get` read off the device (`kists.list`, `map.place.<label>` for settings and each kist, `map.view`). The device's copy goes into a space, sealed and opened as it crosses the fleet, and an empty vault (a reset device) is brought back from it; the two copies must match byte for byte. Reads the device's files only; writes nothing to it.
#[cfg(test)]
mod device_dump {
    extern crate std;
    use super::*;
    use std::collections::BTreeMap;

    struct Files(BTreeMap<(u8, Vec<u8>), Vec<u8>>);

    impl Store for Files {
        fn get(&mut self, s: Scope, key: &[u8]) -> Result<Option<Vec<u8>>, ()> {
            Ok(self.0.get(&(s as u8, key.to_vec())).cloned())
        }
        fn set(&mut self, s: Scope, key: &[u8], value: &[u8]) -> bool {
            self.0.insert((s as u8, key.to_vec()), value.to_vec());
            true
        }
        fn remove(&mut self, s: Scope, key: &[u8]) -> bool {
            self.0.remove(&(s as u8, key.to_vec()));
            true
        }
    }

    #[test]
    #[ignore]
    fn a_devices_own_copy_comes_back_from_its_space() {
        let Ok(dir) = std::env::var("FERROS_SPACE_DUMP") else { return };
        let mut device = Files(BTreeMap::new());
        for e in std::fs::read_dir(&dir).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().into_string().unwrap();
            let bytes = std::fs::read(e.path()).unwrap();
            let (scope, key) = match name.split_once('.') {
                Some(("kists", k)) => (Scope::Kists, k),
                Some(("map", k)) => (Scope::Map, k),
                _ => continue,
            };
            device.set(scope, key.as_bytes(), &bytes);
        }
        let me = [0x11; 32];
        let (mut space, mut base) = (Space::new(), Baseline::default());
        let o = sync(&mut space, &mut base, &mut device, &me, 1 << 40).unwrap();
        let list = read_list(&mut device).unwrap();
        std::println!("the device's copy: {} kist(s) ({}), {} fact(s) in its space, {} place(s) and the view", list.kists.len(), list.kists.iter().map(|k| k.name.as_str()).collect::<Vec<_>>().join(", "), space.entries().len(), device.0.keys().filter(|k| k.1.starts_with(b"place.")).count());
        assert!(o.space_changed && o.wrote == 0);
        let (hp, key) = ([0x77; 32], [0x88; 32]);
        let doc = space.to_doc(1 << 40).unwrap();
        let sealed = fgtw::space::seal(&doc, &hp, &key).unwrap();
        std::println!("sealed: {} bytes of document, {} sealed", doc.len(), sealed.wrap.len());
        let mut fresh = Files(BTreeMap::new());
        let (mut sf, mut bf) = (Space::from_doc(&fgtw::space::open(&sealed, &hp, &key).unwrap()).unwrap(), Baseline::default());
        let o = sync(&mut sf, &mut bf, &mut fresh, &[0x22; 32], (1 << 40) + 1).unwrap();
        std::println!("the reset device: {} write(s)", o.wrote);
        assert_eq!(read_list(&mut fresh).unwrap(), list, "the same kists, answers and defaults");
        for (k, v) in device.0.iter().filter(|(k, _)| k.0 == Scope::Map as u8) {
            let label = std::string::String::from_utf8_lossy(&k.1);
            match fresh.0.get(k) {
                Some(w) if decode_place(w).is_some() && decode_place(w) == decode_place(v) || k.1 == b"view" && decode_view(w) == decode_view(v) => std::println!("  {label}: the same"),
                other => panic!("{label}: {:?} on the device, {:?} restored", v, other),
            }
        }
    }
}
