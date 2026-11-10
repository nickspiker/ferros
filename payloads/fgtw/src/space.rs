//! The person's identity space on this device (docs/IDENTITY-SPACE.md; ferros_space over fgtw::space): what of theirs follows them across their fleet, kept here, joined to the device's own copy, and handed to their other devices and to fgtw.org.
//!
//! - **Kept** in this service's scope, the person's class: `fgtw.space` (the space, a VSF document) and `fgtw.space_base` (the baseline: what this device's copy held when it last agreed with the space; device-local, never sent). Neither is a root: they stay whatever unlock-at-boot says.
//! - **First, a pull.** Once the device is a member with the fleet key, the space is asked of every fleet device heard (`space_want`) and of fgtw.org (`space_get`). The device's own copy is joined to it only after fgtw.org answered (a space, none, or no slot yet) or a fleet device's copy arrived: before that, a fresh device's empty copy holds nothing of the person, and its defaults (all off) must not go out as their choice.
//! - **Then every 4 s** the device's copy (kists.list, map.place.*, map.view) is joined to the space (ferros_space::sync): the person's changes here go into the space, the space's into the copy. A change goes to every fleet device heard (`space_push`) and to fgtw.org (pulled, merged, put back, so a sibling's copy there is never written over).
//! - **Every 64 s** fgtw.org is asked again, for a sibling's change made where this device could not hear it.
//! - **The map** reads the generation (op 17), bumped whenever the device's copy was written from the space, and reloads its circles; the Fleet page's switch "sync this device's home screen" is op 18.

use super::*;
use ferros_space::space::{Baseline, Sealed, Space};
use ferros_space::Scope;

/// Grant slot 9: the vault scope `map.` (the arrangement and the view); 10: the vault scope `kists.` (the person's list).
pub const S_MAP: u64 = 9;
pub const S_KISTS: u64 = 10;

const SPACE_KEY: &[u8] = b"space";
const BASE_KEY: &[u8] = b"space_base";
/// How often the device's copy is joined to the space, and fgtw.org asked again (µs).
const LOCAL_US: u64 = 4_000_000;
const SEED_US: u64 = 64_000_000;

/// The sync's state, in the client.
pub struct SpaceSync {
    space: Space,
    base: Baseline,
    /// Loaded from the vault (once the person is present).
    loaded: bool,
    /// The first pull is done: fgtw.org answered, or a fleet device's copy arrived. Until then the device's copy is not joined.
    settled: bool,
    /// The digest fgtw.org was last seen or made to hold, and the one last handed to the fleet.
    seed_digest: [u8; 32],
    fleet_digest: [u8; 32],
    last_local_us: u64,
    last_seed_us: u64,
    /// Bumped whenever the device's copy was written from the space: the map reloads its circles when it moves (op 17).
    generation: u32,
    /// Said once: no clock, no slot at fgtw.org.
    said_clock: bool,
    said_slot: bool,
    /// The last join failed and said why: said again only once one has succeeded (a vault that refuses every 4 s would otherwise fill the console).
    said_join: bool,
}

impl SpaceSync {
    pub fn new() -> SpaceSync {
        SpaceSync { space: Space::new(), base: Baseline::default(), loaded: false, settled: false, seed_digest: [0; 32], fleet_digest: [0; 32], last_local_us: 0, last_seed_us: 0, generation: 0, said_clock: false, said_slot: false, said_join: false }
    }
}

/// The device's copy through this process's `map.` and `kists.` grants. A refused read is an error, never absent (ferros_space::Store).
struct VaultCopy {
    map: u64,
    kists: u64,
}

impl VaultCopy {
    fn cap(&self, s: Scope) -> u64 {
        match s {
            Scope::Map => self.map,
            Scope::Kists => self.kists,
        }
    }
}

impl ferros_space::Store for VaultCopy {
    fn get(&mut self, s: Scope, key: &[u8]) -> Result<Option<Vec<u8>>, ()> {
        let c = self.cap(s);
        let mut piece = [0u8; STORE_PIECE];
        let total = match ferros_sys::store_read_at(c, key, 0, &mut piece) {
            ferros_sys::NOTHING => return Ok(None),
            ERR => return Err(()),
            n => n as usize,
        };
        let mut v = Vec::with_capacity(total);
        loop {
            // WHY the min: the last piece is shorter than a transfer.
            let n = (total - v.len()).min(STORE_PIECE);
            v.extend_from_slice(&piece[..n]);
            if v.len() >= total {
                return Ok(Some(v));
            }
            if ferros_sys::store_get_at(c, key, v.len(), &mut piece).is_none() {
                return Err(());
            }
        }
    }

    fn set(&mut self, s: Scope, key: &[u8], value: &[u8]) -> bool {
        let c = self.cap(s);
        if value.len() <= STORE_PIECE {
            return ferros_sys::store_set(c, key, value);
        }
        let mut ok = true;
        for (k, piece) in value.chunks(STORE_PIECE).enumerate() {
            let at = k * STORE_PIECE;
            ok &= ferros_sys::store_set_at(c, key, at, piece, at + piece.len() < value.len());
        }
        ok
    }

    fn remove(&mut self, s: Scope, key: &[u8]) -> bool {
        ferros_sys::store_delete(self.cap(s), key).is_some()
    }
}

impl Client {
    fn copy(&self) -> VaultCopy {
        VaultCopy { map: cap(self.log.grants, S_MAP), kists: cap(self.log.grants, S_KISTS) }
    }

    /// The fleet's facts in hand: a member with the key.
    fn space_ready(&self) -> Option<([u8; 32], [u8; 32])> {
        match (self.hp, self.bound, self.fleet_key) {
            (Some(hp), true, Some(k)) => Some((hp, k)),
            _ => None,
        }
    }

    /// The space and its baseline from the vault, once.
    fn space_load(&mut self) {
        if self.space.loaded {
            return;
        }
        self.space.loaded = true;
        if let Some(doc) = self.store_get_whole(SPACE_KEY) {
            match Space::from_doc(&doc) {
                Ok(s) => self.space.space = s,
                Err(e) => {
                    self.log.put(b"fgtw: space: the kept copy does not read (");
                    self.log.put(e.as_bytes());
                    self.log.put(b"); starting from the fleet's");
                    self.log.line();
                }
            }
        }
        if let Some(doc) = self.store_get_whole(BASE_KEY) {
            if let Ok(b) = Baseline::from_doc(&doc) {
                self.space.base = b;
            }
        }
        self.log.put(b"fgtw: space: ");
        self.log.dec(self.space.space.entries().len() as u64);
        self.log.put(b" fact(s) kept here");
        self.log.line();
    }

    fn space_keep(&mut self) {
        let t = eagle();
        let (Ok(s), Ok(b)) = (self.space.space.to_doc(t), self.space.base.to_doc(t)) else { return };
        if !self.store_set_whole(SPACE_KEY, &s) || !self.store_set_whole(BASE_KEY, &b) {
            self.log.put(b"fgtw: space: the vault did not take the space");
            self.log.line();
        }
    }

    /// A sealed copy merged in (a sibling's, or fgtw.org's): the digest of that copy as it came; None when it does not open or read.
    fn space_take(&mut self, sealed: &Sealed, from: &[u8]) -> Option<[u8; 32]> {
        let (hp, key) = self.space_ready()?;
        let doc = match fgtw::space::open(sealed, &hp, &key) {
            Ok(d) => d,
            Err(e) => {
                self.log.put(b"fgtw: space: a copy from ");
                self.log.put(from);
                self.log.put(b" does not open: ");
                self.log.put(e.as_bytes());
                self.log.line();
                return None;
            }
        };
        let theirs = match Space::from_doc(&doc) {
            Ok(s) => s,
            Err(e) => {
                self.log.put(b"fgtw: space: a copy from ");
                self.log.put(from);
                self.log.put(b" does not read: ");
                self.log.put(e.as_bytes());
                self.log.line();
                return None;
            }
        };
        let digest = theirs.digest();
        if self.space.space.merge(theirs) {
            self.log.put(b"fgtw: space: ");
            self.log.put(from);
            self.log.put(b"'s copy merged, ");
            self.log.dec(self.space.space.entries().len() as u64);
            self.log.put(b" fact(s)");
            self.log.line();
            self.space_keep();
            // Joined at once, not at the next tick: a reset device puts the person's home screen back as soon as it has it.
            self.space.last_local_us = 0;
        }
        Some(digest)
    }

    fn space_sealed(&mut self) -> Option<Sealed> {
        let (hp, key) = self.space_ready()?;
        let doc = self.space.space.to_doc(eagle()).ok()?;
        fgtw::space::seal(&doc, &hp, &key).ok()
    }

    /// fgtw.org's copy pulled and merged; with `put`, ours put back when it holds what the seed's lacks. Settles the first pull on any answer the seed gives (a copy, none, no slot yet); a seed that does not answer leaves it unsettled.
    fn space_seed(&mut self, put: bool) {
        let Some((hp, _)) = self.space_ready() else { return };
        if !self.net_up {
            return;
        }
        fgtw::set_clock(eagle());
        let (pulled, ip) = {
            let s = self.seed();
            let r = fgtw::client::pull_space(&s, &hp);
            (r, s.ip.get())
        };
        self.seed_ip = ip;
        match pulled {
            Ok(Some(sealed)) => {
                self.seed_heard_us = now_us();
                // What the seed holds, as it came: what ours is compared with before a put.
                self.space.seed_digest = self.space_take(&sealed, b"fgtw.org").unwrap_or([0; 32]);
                if !self.space.settled {
                    self.space.settled = true;
                    self.log.put(b"fgtw: space: fgtw.org's copy in hand; joining this device's copy to it");
                    self.log.line();
                }
            }
            Ok(None) => {
                self.seed_heard_us = now_us();
                if !self.space.settled {
                    self.space.settled = true;
                    self.log.put(b"fgtw: space: fgtw.org keeps none for this identity yet");
                    self.log.line();
                }
                self.space.seed_digest = [0; 32];
            }
            Err(e) if e.contains("unknown_op") => {
                self.seed_heard_us = now_us();
                if !self.space.said_slot {
                    self.space.said_slot = true;
                    self.log.put(b"fgtw: space: fgtw.org has no space slot yet (fgtw-bootstrap space_put/space_get not deployed): the space stays on this device and its fleet");
                    self.log.line();
                }
                self.space.settled = true;
                return;
            }
            Err(e) => {
                self.log.put(b"fgtw: space: fgtw.org: ");
                self.log.put(e.as_bytes());
                self.log.line();
                return;
            }
        }
        if put && self.space.seed_digest != self.space.space.digest() {
            let Some(sealed) = self.space_sealed() else { return };
            // Signed with everything this device holds (Falcon and SLH-DSA beside Ed25519), as every other write to fgtw.org is: a fleet whose members declared them refuses a bare Ed25519 egg (`below_floor`), and until 2026-11-10 every push from here was refused so, the departure request with it. On the X3, as the binding request signs: SLH-DSA-128s takes seconds on a worker.
            ferros_sys::pin(ferros_sys::CORES_BIG);
            ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
            let r = {
                let s = self.seed();
                fgtw::client::push_space(&s, &hp, &self.keys, &sealed)
            };
            ferros_sys::pin(0);
            match r {
                Ok(()) => {
                    self.space.seed_digest = self.space.space.digest();
                    self.log.put(b"fgtw: space: fgtw.org holds ours (");
                    self.log.dec(self.space.space.entries().len() as u64);
                    self.log.put(b" fact(s))");
                    self.log.line();
                }
                Err(e) => {
                    self.log.put(b"fgtw: space: fgtw.org did not take ours: ");
                    self.log.put(e.as_bytes());
                    self.log.line();
                }
            }
        }
    }

    /// Ours to every fleet device heard, when it moved since they last had it.
    fn space_to_fleet(&mut self) {
        let digest = self.space.space.digest();
        if digest == self.space.fleet_digest {
            return;
        }
        let Some((hp, _)) = self.space_ready() else { return };
        let Some(sealed) = self.space_sealed() else { return };
        let Ok(frame) = fgtw::protocol::space_push(&hp, &sealed, self.keys.keypair(), eagle()) else { return };
        self.space.fleet_digest = digest;
        self.push_to_fleet(frame, None);
    }

    /// Ask every fleet device heard for its copy.
    fn space_ask_fleet(&mut self) {
        let Some((hp, _)) = self.space_ready() else { return };
        let Ok(frame) = fgtw::protocol::space_want(&hp, &self.space.space.digest(), self.keys.keypair(), eagle()) else { return };
        self.push_to_fleet(frame, None);
    }

    /// A sibling's `space_push`: a member's copy merged in, and ours handed back when it holds what theirs lacked.
    pub fn on_space_push(&mut self, from: Addr, bytes: &[u8]) {
        let Ok((hp, sealed, sender)) = fgtw::protocol::parse_space_push(bytes) else { return };
        if Some(hp) != self.hp || !self.members.contains(&sender) || self.space_ready().is_none() {
            return;
        }
        self.space_load();
        let mut name = [0u8; 8];
        let h = b"0123456789abcdef";
        for (k, &c) in sender[..4].iter().enumerate() {
            name[2 * k] = h[(c >> 4) as usize];
            name[2 * k + 1] = h[(c & 15) as usize];
        }
        let Some(theirs) = self.space_take(&sealed, &name) else { return };
        if !self.space.settled {
            self.space.settled = true;
            self.log.put(b"fgtw: space: a fleet device's copy in hand; joining this device's copy to it");
            self.log.line();
        }
        // Theirs lacked something of ours: hand ours back to that device.
        if theirs != self.space.space.digest() {
            if let Some(mine) = self.space_sealed() {
                if let Ok(frame) = fgtw::protocol::space_push(&hp, &mine, self.keys.keypair(), eagle()) {
                    let out = self.pt.send(from, frame, now_us(), eagle());
                    self.send_all(out);
                }
            }
        }
    }

    /// A sibling's `space_want`: ours, unless it already holds the same.
    pub fn on_space_want(&mut self, from: Addr, bytes: &[u8]) {
        let Ok((hp, digest, sender)) = fgtw::protocol::parse_space_want(bytes) else { return };
        if Some(hp) != self.hp || !self.members.contains(&sender) || self.space_ready().is_none() || !self.space.settled {
            return;
        }
        self.space_load();
        if digest == self.space.space.digest() {
            return;
        }
        if let Some(mine) = self.space_sealed() {
            if let Ok(frame) = fgtw::protocol::space_push(&hp, &mine, self.keys.keypair(), eagle()) {
                let out = self.pt.send(from, frame, now_us(), eagle());
                self.send_all(out);
            }
        }
    }

    /// The device's copy joined to the space (ferros_space::sync), and a change handed on.
    fn space_join(&mut self) {
        if eagle() == 0 {
            if !self.space.said_clock {
                self.space.said_clock = true;
                self.log.put(b"fgtw: space: no wall clock yet; the home screen syncs once there is one");
                self.log.line();
            }
            return;
        }
        let mut copy = self.copy();
        let me = self.me;
        match ferros_space::sync(&mut self.space.space, &mut self.space.base, &mut copy, &me, eagle()) {
            Ok(o) => {
                self.space.said_join = false;
                if o.wrote > 0 {
                    self.space.generation = self.space.generation.wrapping_add(1);
                    self.log.put(b"fgtw: space: ");
                    self.log.dec(o.wrote as u64);
                    self.log.put(b" write(s) to this device's kists and home screen from the fleet's");
                    self.log.line();
                }
                if o.wrote > 0 || o.space_changed {
                    self.space_keep();
                }
                if o.space_changed {
                    self.log.put(b"fgtw: space: a change made here, to the fleet");
                    self.log.line();
                    self.space_to_fleet();
                    self.space_seed(true);
                }
            }
            Err(why) => {
                if !self.space.said_join {
                    self.space.said_join = true;
                    self.log.put(b"fgtw: space: not joined: ");
                    self.log.put(why.as_bytes());
                    self.log.line();
                }
            }
        }
    }

    /// This device's departure request into the space and out to the fleet and fgtw.org at once, so a member approves it wherever it is, on any network (owner, 2026-11-09: "broadcast to the devices like a regular message"); the local hand-over (`depart_hand`) stays the fast path.
    pub fn space_put_depart(&mut self, request: &[u8]) {
        if self.space_ready().is_none() {
            return;
        }
        if ferros_space::put_depart(&mut self.space.space, &self.me, request, eagle()) {
            self.log.put(b"fgtw: space: our departure request, to the fleet");
            self.log.line();
            self.space_keep();
            self.space_to_fleet();
            self.space_seed(true);
        }
    }

    /// A departure published (or its device gone from the fleet): marked gone in the space, so no member asks for it again.
    pub fn space_drop_depart(&mut self, leaver: &[u8; 32]) {
        if self.space_ready().is_none() {
            return;
        }
        if ferros_space::drop_depart(&mut self.space.space, leaver, &self.me, eagle()) {
            self.space_keep();
            self.space_to_fleet();
            self.space_seed(true);
        }
    }

    /// The departures the space carries: another member's goes to the intake a local hand-over takes (`on_depart_req`, which checks its signature and keeps it for the Fleet page); one whose device is no longer a member is marked gone.
    fn space_departures(&mut self) {
        for (leaver, request) in ferros_space::departures(&self.space.space) {
            if leaver == self.me {
                continue;
            }
            if self.bound && !self.members.is_empty() && !self.members.contains(&leaver) {
                self.space_drop_depart(&leaver);
                continue;
            }
            self.on_depart_req(&request);
        }
    }

    /// The loop's step: the first pull, then the join every 4 s and fgtw.org every 64 s. The clock only moves forward, so `t` is never behind a time taken from it.
    pub fn space_step(&mut self, t: u64) {
        if self.space_ready().is_none() {
            return;
        }
        if !self.space.loaded {
            self.space_load();
            self.space_ask_fleet();
            self.space_seed(false);
            self.space.last_seed_us = t;
        }
        if !self.space.settled {
            // The first pull again until fgtw.org answers or a sibling's copy arrives.
            if t - self.space.last_seed_us >= LOCAL_US * 4 {
                self.space.last_seed_us = t;
                self.space_ask_fleet();
                self.space_seed(false);
            }
            return;
        }
        if t - self.space.last_local_us >= LOCAL_US {
            self.space.last_local_us = t;
            self.space_join();
            self.space_departures();
        }
        if t - self.space.last_seed_us >= SEED_US {
            self.space.last_seed_us = t;
            self.space_seed(true);
            self.space_to_fleet();
        }
    }

    /// Op 17 → [generation u32 LE][this device's home-screen sync u8][the first pull done u8][facts u16 LE].
    pub fn space_reply(&self) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[..4].copy_from_slice(&self.space.generation.to_le_bytes());
        r[4] = ferros_space::home_sync(&self.space.space, &self.me) as u8;
        r[5] = self.space.settled as u8;
        // WHY the cap: two bytes on the wire; a space past 65535 facts is not one this page lists.
        r[6..8].copy_from_slice(&(self.space.space.entries().len().min(u16::MAX as usize) as u16).to_le_bytes());
        r
    }

    /// Op 18 [on]: this device's home screen follows the fleet's (1) or keeps its own (0). Refused (1) before the first pull: the switch is a fact of the person's space, and a fresh device's copy of it is not yet theirs.
    pub fn space_switch(&mut self, on: bool) -> u8 {
        if self.space_ready().is_none() || !self.space.settled {
            return 1;
        }
        let me = self.me;
        if ferros_space::set_home_sync(&mut self.space.space, &me, on, eagle()) {
            self.log.put(if on { b"fgtw: space: this device's home screen follows the fleet's".as_slice() } else { b"fgtw: space: this device keeps a home screen of its own" });
            self.log.line();
            self.space_keep();
            self.space_join();
            self.space_to_fleet();
            self.space_seed(true);
        }
        0
    }
}
