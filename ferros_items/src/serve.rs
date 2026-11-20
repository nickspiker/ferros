//! The vault process's side of the hold's calls (docs/STORE.md "The calls"; payloads/vault `OP_ITEMS`): one stamped call in, its value and bytes out.
//!
//! Kept here, not in the vault, so the whole path a kist's call takes (its stamp, the call, the store, the extents) is tested on the host (tests/serve.rs) and the vault only wires its disk to it.
//! The vault hands each call the person's items domain the kernel stamped, the call's rest (the caller's stamp, then the call as `wire` makes it), the reply's room, the time, its randomness and its clips area (`Clips`).
//!
//! What lives here between calls: the hold's index as `Items` holds it, the bodies being kept (`makings`, each named by a token drawn for it and held for the caller that made it), and the clips being recorded (`recording`).
//! All of it is the person's: it goes when the person locks or another person's domain comes (`forget`).

use alloc::vec::Vec;
use ferros_store::{Domain, Nonces, Store};
use manifestus::BlockDev;

use crate::{wire, Caller, Card, Error, Extent, Ht, Items, Making, Read};

/// The syscall's answers for a refusal and an absence (ferros_sys `ERR`, `NOTHING`).
pub const ERR: u64 = u64::MAX;
pub const NOTHING: u64 = u64::MAX - 1;

/// Bodies one caller may have in the making at once.
/// WHY: a making holds a ferros_store writer in this process until it is finished or the person locks; a kist that starts makings and never finishes them must not fill the vault's heap. A caller past it is refused a new one, never another caller.
pub const MAKINGS_MOST: usize = 1 << 3;

/// The blocks of the clips area one read or write moves at most (a UFS command: payloads/vault RUN_MAX).
pub const RUN_BLOCKS: u64 = 1 << 8;

/// An extent's record as the vault's table holds it (manifestus `Extent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rec {
    pub extent: Extent,
    pub bytes: u64,
    pub hash: [u8; 32],
    pub sealed: bool,
}

/// The vault's clips area and its extents' table, as the hold uses them (payloads/vault `ItemClips`; tests/serve.rs over memory). Block numbers are the area's.
pub trait Clips<S> {
    /// Reserve `blocks` under `name` for `owner` and commit the table: where it lies, or None (no stretch that long free, or the table did not commit).
    fn open(&mut self, s: &mut S, name: [u8; 32], owner: [u8; 32], blocks: u64, now: i64) -> Option<Extent>;
    /// The extent's blocks given to the caller for its own writes straight to the flash: its capability, or None (the caller writes thru the vault, `OP_WRITE_EXTENT`; the vault keeps the extent off its recoverer then).
    fn give(&mut self, name: &[u8; 32], x: Extent) -> Option<[u8; 32]>;
    /// The caller's own writes end (its clip sealed).
    fn take_back(&mut self);
    /// Whole blocks written at block `at` of the area, or read from it; whether the flash took or gave them.
    fn write(&mut self, at: u64, bytes: &[u8]) -> bool;
    fn read(&mut self, at: u64, buf: &mut [u8]) -> bool;
    /// Sealed at `len` bytes with `hash` and the table committed: the extent as it now stands (its tail given back), or None (refused).
    fn seal(&mut self, s: &mut S, name: &[u8; 32], len: u64, hash: [u8; 32], now: i64) -> Option<Extent>;
    /// Dropped, its blocks free again, the table committed. Err when it may not go now (a live writer still records into it) or the table did not commit; Ok when it is gone or was not there.
    fn remove(&mut self, s: &mut S, name: &[u8; 32], now: i64) -> Result<(), ()>;
    /// Its record, None when the table has none.
    fn record(&mut self, s: &mut S, name: &[u8; 32]) -> Option<Rec>;
    /// The area's blocks, the blocks every extent takes, the longest free stretch; None without a table.
    fn space(&mut self, s: &mut S) -> Option<wire::ClipsUse>;
}

/// A single-copy body's name in the extents' table: the items domain's address key and the item's id, never stored (the table carries no keys).
pub fn clip_name(d: &Domain, id: &Ht) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("ferros items 2026-11-19 clip name");
    h.update(&d.addr);
    h.update(id.as_bytes());
    *h.finalize().as_bytes()
}

/// The extents' owner for a domain, as the vault tags every extent of a domain (payloads/vault: BLAKE3 of its address key), so a domain dropped whole at release drops its clips.
pub fn clip_owner(d: &Domain) -> [u8; 32] {
    *blake3::hash(&d.addr).as_bytes()
}

/// One call's answer: the syscall's value, how many bytes of the reply's room it filled, and on a refusal what refused it (for the vault's console).
#[derive(Debug)]
pub struct Answer {
    pub value: u64,
    pub data: usize,
    pub why: Option<Error>,
}

fn refused(e: Error) -> Answer {
    let value = if matches!(e, Error::NotFound) { NOTHING } else { ERR };
    Answer { value, data: 0, why: Some(e) }
}

fn ok(value: u64, data: usize) -> Answer {
    Answer { value, data, why: None }
}

/// A body in the making: its token, for whom, the making.
struct Open {
    token: [u8; 32],
    caller: [u8; 32],
    m: Making,
}

/// A clip being recorded into its extent: the item's handle (the caller's), for whom, its name in the table, where it lies.
struct Recording {
    handle: [u8; 32],
    caller: [u8; 32],
    name: [u8; 32],
    x: Extent,
}

/// The hold as the vault process serves it.
pub struct Server {
    pub items: Items,
    /// The items domain served now (its owner tag): another one, and everything held goes first.
    domain: Option<[u8; 32]>,
    makings: Vec<Open>,
    recording: Vec<Recording>,
    /// A read of a single-copy body: its blocks, a run at a time.
    scratch: Vec<u8>,
}

impl Server {
    /// `secret` keys the handles (drawn when the vault process starts).
    pub fn new(secret: [u8; 32]) -> Server {
        Server { items: Items::new(secret), domain: None, makings: Vec::new(), recording: Vec::new(), scratch: Vec::new() }
    }

    /// The person locked: their index, their makings and their recordings go (the vault drops its plaintext the same way).
    pub fn forget(&mut self) {
        self.items.forget();
        self.makings.clear();
        self.recording.clear();
        self.domain = None;
    }

    /// The domain dropped whole (release): every item's card and stored body, the hold's lists; what was held goes. The clips' extents are the vault's to drop, by `clip_owner`. How many items went.
    pub fn drop_domain<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain) -> crate::Result<u64> {
        self.forget();
        self.items.drop_all(store, d)
    }

    /// Open clips whose extent is sealed by now (their writer finished without saying so, or died and the vault's recoverer closed them) get their sealed card; one whose extent is gone from the table loses its card (it would name blocks nobody holds). How many cards changed.
    pub fn reconcile<A: BlockDev, B: BlockDev, N: Nonces, X: Clips<Store<A, B, N>>>(&mut self, store: &mut Store<A, B, N>, d: &Domain, now: i64, clips: &mut X) -> u64 {
        let Ok(open) = self.items.open_clips(store, d) else { return 0 };
        let mut n = 0;
        for (id, made) in open {
            let name = clip_name(d, &id);
            match clips.record(store, &name) {
                Some(r) if r.sealed => {
                    if self.items.amend_extent(store, d, &id, made, r.extent, r.bytes, r.hash, now).is_ok() {
                        n += 1;
                    }
                    self.recording.retain(|x| x.name != name);
                }
                Some(_) => {}
                None => {
                    if self.items.remove_id(store, d, &id, made, now).is_ok() {
                        n += 1;
                    }
                    self.recording.retain(|x| x.name != name);
                }
            }
        }
        n
    }

    /// Before every call: another person's domain, or the first call since the vault started or the person locked, and what was held goes and the domain is swept once (a body whose card never landed); then the open clips reconciled.
    fn enter<A: BlockDev, B: BlockDev, N: Nonces, X: Clips<Store<A, B, N>>>(&mut self, store: &mut Store<A, B, N>, d: &Domain, now: i64, clips: &mut X) {
        let owner = clip_owner(d);
        if self.domain != Some(owner) {
            self.forget();
            self.domain = Some(owner);
            let _ = self.items.sweep(store, d, now);
        }
        self.reconcile(store, d, now, clips);
    }

    /// What is on the device (`wire::OP_USAGE`, read only): the hold's totals by kind, the clips area, and `kists` as the vault found them (each stowed kist from the person's list, sized where the call named it and the kernel stamped its domain). The call's rights were the kernel's to check (the `items` right, the person present).
    pub fn usage<A: BlockDev, B: BlockDev, N: Nonces, X: Clips<Store<A, B, N>>>(&mut self, store: &mut Store<A, B, N>, d: &Domain, kists: Vec<wire::KistUse>, out: &mut [u8], room: usize, now: i64, clips: &mut X) -> Answer {
        self.enter(store, d, now, clips);
        let kinds = match self.items.totals(store, d) {
            Ok(t) => t.into_iter().map(|(kind, items, bytes)| wire::KindUse { kind, items, bytes }).collect(),
            Err(e) => return refused(e),
        };
        let u = wire::Usage { kinds, clips: clips.space(store), kists };
        match wire::usage(&u, now) {
            Ok(doc) => {
                // WHY: the room is the caller's; the value says the whole length, so a short room asks again with more.
                let n = doc.len().min(room).min(out.len());
                out[..n].copy_from_slice(&doc[..n]);
                ok(doc.len() as u64, n)
            }
            Err(e) => refused(e),
        }
    }

    /// One call: `rest` is [the caller's stamp][the call] (process.rs `items_forward`), `out` the reply's room after its value word; `random` draws 32 fresh bytes.
    pub fn serve<A: BlockDev, B: BlockDev, N: Nonces, X: Clips<Store<A, B, N>>>(&mut self, store: &mut Store<A, B, N>, d: &Domain, rest: &[u8], out: &mut [u8], now: i64, random: &mut dyn FnMut() -> [u8; 32], clips: &mut X) -> Answer {
        let Some((caller, call)) = Caller::from_stamp(rest) else { return refused(Error::Format) };
        let Some((op, handle, offset, len, payload)) = wire::parse_call(call) else { return refused(Error::Format) };
        // WHY: the length is the caller's: the reply fills no more than the room the kernel gave it.
        let room = out.len().min(len as usize);
        let owner = clip_owner(d);
        self.enter(store, d, now, clips);
        let tag = caller.tag();
        match op {
            wire::OP_MAKE | wire::OP_KEEP_AGAIN => {
                if room < 32 {
                    return refused(Error::TooLarge);
                }
                if self.makings.iter().filter(|o| o.caller == tag).count() >= MAKINGS_MOST {
                    return refused(Error::Refused);
                }
                let m = if op == wire::OP_MAKE {
                    match wire::parse_propose(payload).and_then(|p| self.items.make(&caller, p, now, random())) {
                        Ok(m) => m,
                        Err(e) => return refused(e),
                    }
                } else {
                    match self.items.keep_again(store, d, &caller, &handle, now) {
                        Ok(m) => m,
                        Err(e) => return refused(e),
                    }
                };
                let token = random();
                self.makings.push(Open { token, caller: tag, m });
                out[..32].copy_from_slice(&token);
                ok(0, 32)
            }
            wire::OP_WRITE => {
                let Some(i) = self.makings.iter().position(|o| o.token == handle && o.caller == tag) else { return refused(Error::Refused) };
                match self.items.write(store, d, &mut self.makings[i].m, payload, now) {
                    Ok(()) => ok(0, 0),
                    Err(e) => {
                        // A refused write ends the making: its pieces are on the pending list for the sweep.
                        self.makings.swap_remove(i);
                        refused(e)
                    }
                }
            }
            wire::OP_FINISH => {
                let Some(i) = self.makings.iter().position(|o| o.token == handle && o.caller == tag) else { return refused(Error::Refused) };
                let o = self.makings.swap_remove(i);
                match self.items.finish(store, d, &caller, o.m, now) {
                    Ok((h, c)) => card_answer(&c, &h, now, out, room),
                    Err(e) => refused(e),
                }
            }
            wire::OP_READ => {
                let c = match self.items.card(store, d, &caller, &handle) {
                    Ok(c) => c,
                    Err(e) => return refused(e),
                };
                match c.extent {
                    None => match self.items.read(store, d, &caller, &handle, offset, &mut out[..room]) {
                        Ok(Read::Bytes(n, size)) => ok(size, n),
                        Ok(Read::Extent(..)) => refused(Error::Format),
                        Err(e) => refused(e),
                    },
                    Some(x) => self.read_clip(store, d, &c, x, offset, &mut out[..room], clips),
                }
            }
            wire::OP_CARD => match self.items.card(store, d, &caller, &handle) {
                Ok(c) => card_answer(&c, &handle, now, out, room),
                Err(e) => refused(e),
            },
            wire::OP_FIND => {
                let f = match wire::parse_find(payload) {
                    Ok(f) => f,
                    Err(e) => return refused(e),
                };
                match self.items.find(store, d, &caller, &f).and_then(|r| wire::found(&r, now)) {
                    Ok(doc) => {
                        // The value is the whole answer's length: a caller whose room was short asks again with more, or for fewer.
                        let n = doc.len().min(room);
                        out[..n].copy_from_slice(&doc[..n]);
                        ok(doc.len() as u64, n)
                    }
                    Err(e) => refused(e),
                }
            }
            wire::OP_REMOVE => {
                let c = match self.items.card(store, d, &caller, &handle) {
                    Ok(c) => c,
                    Err(e) => return refused(e),
                };
                // A single-copy body's extent goes first: a kill between the two leaves a card whose body reads as absent, which the person sees and removes again, never blocks nobody can find.
                if c.extent.is_some() {
                    let name = clip_name(d, &c.id);
                    if clips.remove(store, &name, now).is_err() {
                        return refused(Error::Refused);
                    }
                    self.recording.retain(|r| r.name != name);
                }
                match self.items.remove(store, d, &caller, &handle, now) {
                    Ok(_) => ok(0, 0),
                    Err(e) => refused(e),
                }
            }
            wire::OP_MAKE_EXTENT => {
                if room < 64 {
                    return refused(Error::TooLarge);
                }
                let m = match wire::parse_propose(payload).and_then(|p| self.items.make(&caller, p, now, random())) {
                    Ok(m) => m,
                    Err(e) => return refused(e),
                };
                let name = clip_name(d, m.id());
                // The extent is reserved and committed before the card names it; a kill between the two leaves an open extent with no card, which the vault's recoverer seals and `store extent-list` / `extent-drop` find by its owner.
                let Some(x) = clips.open(store, name, owner, offset, now) else { return refused(Error::TooLarge) };
                let (h, _) = match self.items.begin_extent(store, d, &caller, m, x, now) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = clips.remove(store, &name, now);
                        return refused(e);
                    }
                };
                // One clip at a time a caller (the kernel takes a process's earlier extent back when it gives the next).
                self.recording.retain(|r| r.caller != tag);
                let cap = clips.give(&name, x);
                self.recording.push(Recording { handle: h, caller: tag, name, x });
                out[..32].copy_from_slice(&h);
                out[32..64].copy_from_slice(&cap.unwrap_or([0; 32]));
                ok(x.blocks, 64)
            }
            wire::OP_WRITE_EXTENT => {
                let Some(r) = self.recording.iter().find(|r| r.handle == handle && r.caller == tag) else { return refused(Error::Refused) };
                let n = (payload.len() >> 12) as u64;
                // WHY: the block and the bytes are the caller's: whole blocks, one run at most, inside its own extent.
                if payload.is_empty() || payload.len() & 4095 != 0 || n > RUN_BLOCKS || !r.x.holds(offset, n) {
                    return refused(Error::Refused);
                }
                if clips.write(r.x.first + offset, payload) { ok(0, 0) } else { refused(Error::Refused) }
            }
            wire::OP_FINISH_EXTENT => {
                let Some(i) = self.recording.iter().position(|r| r.handle == handle && r.caller == tag) else { return refused(Error::Refused) };
                if payload.len() != 40 {
                    return refused(Error::Format);
                }
                let len = u64::from_le_bytes(payload[..8].try_into().unwrap());
                let hash: [u8; 32] = payload[8..40].try_into().unwrap();
                let name = self.recording[i].name;
                let Some(x) = clips.seal(store, &name, len, hash, now) else { return refused(Error::TooLarge) };
                clips.take_back();
                self.recording.swap_remove(i);
                match self.items.seal_extent(store, d, &caller, &handle, x, len, hash, now) {
                    Ok(c) => card_answer(&c, &handle, now, out, room),
                    Err(e) => refused(e),
                }
            }
            // The kists' part is the vault's (it reads the person's list and sizes the domains the kernel stamped, then calls `usage`); here the hold and the clips area alone.
            wire::OP_USAGE => self.usage(store, d, Vec::new(), out, room, now, clips),
            _ => refused(Error::Format),
        }
    }

    /// A single-copy body's bytes from `offset`, at most `out.len()`, read from its blocks: only while the table still records that extent where the card says, so a stale card never reads another clip's blocks.
    fn read_clip<A: BlockDev, B: BlockDev, N: Nonces, X: Clips<Store<A, B, N>>>(&mut self, store: &mut Store<A, B, N>, d: &Domain, c: &Card, x: Extent, offset: u64, out: &mut [u8], clips: &mut X) -> Answer {
        match clips.record(store, &clip_name(d, &c.id)) {
            Some(r) if r.extent.first == x.first => {}
            _ => return refused(Error::NotFound),
        }
        if offset >= c.size || out.is_empty() {
            return ok(c.size, 0);
        }
        // WHY: the offset and room are the caller's; the read ends at the body's end.
        let n = out.len().min((c.size - offset) as usize);
        let (b0, b1) = (offset >> 12, (offset + n as u64).div_ceil(1 << 12));
        let lead = (offset & 4095) as usize;
        let mut done = 0usize;
        let mut b = b0;
        while b < b1 {
            let k = (b1 - b).min(RUN_BLOCKS);
            if self.scratch.len() < (k as usize) << 12 {
                self.scratch.resize((RUN_BLOCKS as usize) << 12, 0);
            }
            if !clips.read(x.first + b, &mut self.scratch[..(k as usize) << 12]) {
                return refused(Error::Refused);
            }
            // The run's bytes that fall in [offset, offset + n): past the lead in the first run, up to the end in the last.
            let from = if b == b0 { lead } else { 0 };
            let take = ((k as usize) << 12).min(n - done + from) - from;
            out[done..done + take].copy_from_slice(&self.scratch[from..from + take]);
            done += take;
            b += k;
        }
        ok(c.size, n)
    }
}

/// A card answered as a document of one `card` with the caller's handle: the value its length, its bytes as many as the room holds.
fn card_answer(c: &Card, h: &[u8; 32], now: i64, out: &mut [u8], room: usize) -> Answer {
    match wire::card_doc(c, h, now) {
        Ok(doc) => {
            let n = doc.len().min(room);
            out[..n].copy_from_slice(&doc[..n]);
            ok(doc.len() as u64, n)
        }
        Err(e) => refused(e),
    }
}
