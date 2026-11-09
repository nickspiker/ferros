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
//! | `HOME_SYNC` | a device `ke` | on `u` | none: the space's own, set from settings |
//! | `DEPART` | the leaving device `ke` | its signed departure request `v` | none: the identity service's own (payloads/fgtw), so a member approves it wherever it is |
//!
//! The scope of a place and the view is the fleet's (`FLEET`, 32 zero bytes) on every device whose home-screen sync is on (the default), and the device's own key on one where the person switched it off, so that device keeps an arrangement of its own, which follows it through a reset all the same.
//!
//! Device firmware is never here: it is the device's, re-read from its own partitions.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use ferros_kist::{Kist, Kists, Want, RIGHTS};
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
    pub const HOME_SYNC: u8 = 5;
    /// A member leaving the fleet: its signed departure request, carried by the space to every member wherever it is (owner, 2026-11-09: "broadcast to the devices like a regular message"), approved on any of them with the words the leaver shows; gone once the Remove is published or the device is no longer a member.
    pub const DEPART: u8 = 6;
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

pub fn default_id(right: u8) -> Vec<VsfType> {
    vec![VsfType::u(right as usize, false)]
}

pub fn place_id(scope: &[u8; 32], c: &Circle) -> Vec<VsfType> {
    let mut id = vec![VsfType::ke(scope.to_vec())];
    match c {
        Circle::Settings => id.push(VsfType::d(String::from("settings"))),
        Circle::Kist(d, o) => id.extend(kist_id(d, o)),
    }
    id
}

pub fn view_id(scope: &[u8; 32]) -> Vec<VsfType> {
    vec![VsfType::ke(scope.to_vec())]
}

pub fn home_sync_id(device: &[u8; 32]) -> Vec<VsfType> {
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

/// The departures in the space: each leaving device and its request's bytes.
pub fn departures(space: &Space) -> Vec<([u8; 32], Vec<u8>)> {
    space.live(kind::DEPART).filter_map(|e| Some((e.id.first().and_then(k32)?, e.value.first()?.as_bytes()?.to_vec()))).collect()
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

/// Whether this device's home screen follows the fleet's (on until the person switches it off for this device).
pub fn home_sync(space: &Space, device: &[u8; 32]) -> bool {
    match space.get(kind::HOME_SYNC, &home_sync_id(device)) {
        Some(e) if !e.gone => e.value.first().and_then(|v| v.as_u64()) != Some(0),
        _ => true,
    }
}

/// The person switched this device's home-screen sync on or off (settings): written straight into the space, stamped past everything it holds. True when the space changed.
pub fn set_home_sync(space: &mut Space, device: &[u8; 32], on: bool, now: i64) -> bool {
    if home_sync(space, device) == on && space.get(kind::HOME_SYNC, &home_sync_id(device)).is_some() {
        return false;
    }
    // Past the switch's last write, so this one wins the merge whatever the clocks say (a Lamport step: see fgtw::space::reconcile).
    let updated = space.get(kind::HOME_SYNC, &home_sync_id(device)).map_or(now, |e| now.max(e.updated + 1));
    space.put(Entry { kind: kind::HOME_SYNC, id: home_sync_id(device), value: vec![VsfType::u(on as usize, false)], updated, by: *device, gone: false })
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

/// A kist's fact value: its capsule, name, and wants in slot order (each right and the developer's reason).
fn kist_value(k: &Kist) -> Vec<VsfType> {
    let mut v = vec![VsfType::hb(k.capsule.to_vec()), VsfType::a(k.name.clone())];
    for w in &k.wants {
        v.push(VsfType::u(w.right as usize, false));
        v.push(VsfType::a(w.reason.clone()));
    }
    v
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
        facts.push(Fact { kind: kind::KIST, id: kist_id(&k.developer, &k.option), value: kist_value(k), stamp: None });
        for w in &k.wants {
            if let Some((granted, when)) = w.answer {
                facts.push(Fact { kind: kind::ANSWER, id: answer_id(&k.developer, &k.option, w.right), value: vec![VsfType::u(granted as usize, false)], stamp: Some(when) });
            }
        }
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
        kind::KIST | kind::DEFAULT => true,
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
}

/// One write from the space applied to the device's copy (the list in hand; places and the view straight to the vault). False when the vault refused.
fn apply(w: Write, list: &mut Kists, list_changed: &mut bool, store: &mut impl Store) -> bool {
    match w {
        Write::Set { kind: kind::KIST, id, value, .. } => {
            let (Some(d), Some(o), Some((capsule, name, wants))) = (id.first().and_then(k32), id.get(1).and_then(k32), kist_of(&value)) else { return true };
            let old = list.kists.iter().position(|k| k.developer == d && k.option == o);
            let answers = |r: u8| old.and_then(|i| list.kists[i].wants.iter().find(|w| w.right == r)).and_then(|w| w.answer);
            let wants: Vec<Want> = wants.into_iter().map(|(right, reason)| Want { right, reason, answer: answers(right) }).collect();
            match old {
                Some(i) => {
                    // A kist renamed on another device: its circle's kept place moves with its label.
                    if list.kists[i].name != name {
                        let (from, to) = (place_key(&list.kists[i].name), place_key(&name));
                        if let Ok(Some(p)) = store.get(Scope::Map, &from) {
                            if !store.set(Scope::Map, &to, &p) || !store.remove(Scope::Map, &from) {
                                return false;
                            }
                        }
                    }
                    list.kists[i] = Kist { capsule, developer: d, option: o, name, wants };
                }
                None => list.kists.push(Kist { capsule, developer: d, option: o, name, wants }),
            }
            *list_changed = true;
            true
        }
        Write::Remove { kind: kind::KIST, id } => {
            let (Some(d), Some(o)) = (id.first().and_then(k32), id.get(1).and_then(k32)) else { return true };
            list.kists.retain(|k| !(k.developer == d && k.option == o));
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
    for _ in 0..4 {
        let mut list = read_list(store).map_err(|_| "the person's list is not readable")?;
        let scope = if home_sync(space, me) { FLEET } else { *me };
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
    Ok(Outcome { space_changed: space.digest() != start, wrote })
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
    fn the_fleet_shares_one_home_screen_unless_a_device_opts_out() {
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
        // The other device switches its home-screen sync off, then the Pro moves settings: the other keeps its own.
        set_home_sync(&mut sb, &OTHER, false, 2500);
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
