//! A device leaving its fleet, both halves, on fgtw's shared machine (`fgtw::flow::depart`, FLOWS.md stage 2, the one Photon runs): the last device cannot ask, a denied salt is refused for good, an NFC tap of the leaving device approves, and the leaver wipes once the fold no longer lists it.
//! This file decides nothing about a departure: it tells the machine what happened and carries out what it asks.
//!
//! - **Made** once the roots are in hand (`depart_start`), from its state in the vault when there is one, resumed at once (our request handed again, a request asked about offered again), then told the fold of every chain adopted (`depart_fold`).
//! - **Kept** in the `fgtw.` scope as `depart` (`DEPART_KEY`), whatever unlock-at-boot says: the machine's state as one VSF document (`Depart::to_vsf`), which holds no handle proof (a root, kept on disk only with unlock-at-boot); the roots in hand give it back when it is read.
//! - **Migrated** at start: a departure kept by the build before the machine (section `depart`, field `rq`: the signed request alone) becomes the machine's own request with the same stamp, salt and signed frame, checked against these roots as before; the denied list kept beside it (`denied`) is told to the machine once and then deleted.
//! - **Slow work** (signing with every egg, the approval's publish, a chain read) is queued and run by the loop (`depart_work`), pinned to the X3 where it signs, never inside a call another process waits on.
//! - **Sends** go by Photon Transport to the address the device was last heard at (its beacon, a ping, any frame from it); PT's answer names the frame by its hash, mapped back to the device it was for (route.rs), so another frame's acknowledgement never counts as this one taken. Every send's outcome is reported (`kept::REPORTS_SENT`): a send with no address yet is reported given up at once, so the machine hands it when the device is next heard. The sponsor named at spawn gets each frame once.
//! - **The identity space** carries the departure too (space.rs), so devices away from this network see it: our request once built, a deny made here, and the entry dropped when our request is given up or an approval here publishes.
//! - **An approval here** hands the leaving device our new chain (`PushChain`, a `fleet_resp`), so on this network it learns it is out at once. The leaving device still reads fgtw.org's chain every 16 s while its request stands (`depart_seed_check`): ferros has no relay, so an approval made where the push cannot reach it (another network, this device asleep or off, the push given up) reaches it only thru fgtw.org.
//! - **The Fleet page** (payloads/map) reads op 1 (state 8, leaving), op 13 and the doorbell; its words are fgtw's (`fgtw::text`), picked by the map from those bytes, so each `Note` here sets what those bytes say and puts a line on the console.

use super::*;
use fgtw::flow::depart::Depart;
use fgtw::flow::{Action, Event as Flow, Frame as FlowFrame, Job, Note, To, UserOp};

/// The kept departure's name in the `fgtw.` scope (`depart_keep`).
pub const DEPART_KEY: &[u8] = b"depart";
/// The denied list kept before the machine (section `denied`, a `request` field each: the leaving device `ke`, the salt `hb`, the consent stamp `e`): read once into the machine, then deleted.
const DENIED_KEY: &[u8] = b"denied";

/// Slow work the machine asked for, run by the loop.
pub enum Work {
    /// Sign this frame with every egg (`fgtw::flow::build`) and answer `Built`.
    Build { id: u32, frame: FlowFrame },
    /// Countersign and publish a departure approved here (`fgtw::sponsor::approve_departure`) and answer `Approved`.
    Approve { leaver: [u8; 32], t: i64, consent_sig: Vec<u8>, intent: u8 },
    /// Hand our chain as it stands to this device (`fleet_resp`): a leaving device approved here learns it is out.
    PushChain { to: [u8; 32] },
    /// Read the chain again (the approval changed it).
    RefreshChain,
}

/// The departure's executor, in the client.
pub struct Departing {
    /// The machine, while the roots are in hand.
    pub m: Option<Depart>,
    work: Vec<Work>,
    sends: route::Sends,
    /// The last frame handed to the sponsor named at spawn (its BLAKE3), so each frame goes to it once.
    given_sent: Option<[u8; 32]>,
    /// How many ops the newest chain told to the machine held: a chain only grows, so one with fewer is older than the fleet as already told and is not told.
    ops_told: usize,
    /// Op 13's result byte for the request asked about here: 0 waiting, 1 approving, 3 the approval failed.
    pub result: u8,
    /// This device's own request denied: the device that denied it (op 13 [2]), until the person asks again.
    pub denied_by: Option<[u8; 32]>,
    /// When the seed's chain was last read while our request stands (`depart_seed_check`).
    pub checked_us: u64,
    /// Sends the machine asked for that never reached PT (no address for the device yet): reported given up once the step's actions are carried out.
    unsent: Vec<[u8; 32]>,
    /// Leaving devices approved here, handed our chain once it is read again (`depart_push_chain`).
    push_to: Vec<[u8; 32]>,
}

impl Departing {
    pub const fn new() -> Departing {
        Departing { m: None, work: Vec::new(), sends: route::Sends::new(), given_sent: None, ops_told: 0, result: 0, denied_by: None, checked_us: 0, unsent: Vec::new(), push_to: Vec::new() }
    }
}

impl Client {
    /// Our own request stands (op 1's state 8).
    pub fn depart_asking(&self) -> bool {
        self.dep.m.as_ref().is_some_and(|m| m.asking())
    }

    /// The request this device's person is asked about: (leaving device, intent, salt).
    pub fn depart_asked(&self) -> Option<([u8; 32], u8, [u8; 32])> {
        self.dep.m.as_ref().and_then(|m| m.asked())
    }

    /// One event to the machine, its actions carried out.
    pub fn depart_event(&mut self, ev: Flow) {
        let Some(m) = self.dep.m.as_mut() else { return };
        let step = m.step(ev, eagle());
        self.depart_carry(step.actions);
        self.depart_unsent();
    }

    /// The sends that never reached PT, told to the machine as given up: each device is handed its frame when it is next heard or a network comes up.
    fn depart_unsent(&mut self) {
        while let Some(to) = self.dep.unsent.pop() {
            let Some(m) = self.dep.m.as_mut() else { return };
            let step = m.step(Flow::Sent { to, ok: false }, eagle());
            self.depart_carry(step.actions);
        }
    }

    /// The machine made, once the roots are in hand: from the vault (either form) or fresh, the old denied list told to it, resumed, and told the fold of the chain held.
    pub fn depart_start(&mut self) {
        let Some(hp) = self.hp else { return };
        if self.dep.m.is_some() || self.identity_seed.is_none() {
            return;
        }
        let kept = self.store_get_whole(DEPART_KEY).and_then(|doc| self.depart_read(&doc, &hp));
        let migrated = kept.as_ref().is_some_and(|(_, old)| *old);
        self.dep.m = Some(match kept {
            Some((m, _)) => m,
            None => Depart::new(hp, self.me, kept::REPORTS_SENT),
        });
        if migrated {
            self.log.put(b"fgtw: a departure kept by the build before fgtw's machine: taken up by the machine with its stamp, salt and signed request");
            self.log.line();
            self.depart_keep();
        }
        // The denied list from before the machine: told once, kept in the machine's state, then deleted.
        if let Some(denied) = self.store_get_whole(DENIED_KEY).map(|doc| read_denied(&doc)) {
            self.depart_event(Flow::DeniedKnown { denied: &denied });
            if self.depart_keep() {
                let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), DENIED_KEY);
            }
        }
        if let Some(m) = self.dep.m.as_mut() {
            // Before the first fold: our request's delivery starts with no targets and the fold adds each member once (resumed after it, each would be handed the request twice).
            let step = m.resume();
            if m.asking() {
                self.log.put(b"fgtw: resuming the departure kept from before the restart: asking this identity's devices again");
                self.log.line();
            }
            self.depart_carry(step.actions);
            self.depart_unsent();
        }
        // The chain this device holds, if any (the vault's, with unlock at boot): the machine's first fold; a departure whose chain already lacks this device completes here.
        let held = self.held.borrow().get(&hp).cloned();
        if let Some(blob) = held {
            self.depart_fold(&blob);
        }
        // Our request, kept before the restart: into the space again (it holds it already unless the space was reset).
        self.depart_space_own();
    }

    /// A kept departure read (kept.rs): the machine's state, or the old form converted (the bool says which). None when there is nothing to take up.
    fn depart_read(&mut self, doc: &[u8], hp: &[u8; 32]) -> Option<(Depart, bool)> {
        match kept::read(doc, hp, &self.me) {
            kept::Kept::Machine(m) => Some((m, false)),
            kept::Kept::Old(m) => Some((m, true)),
            kept::Kept::Stale => {
                // A request kept by a build from before 2026-11-10 (words, no salt): dropped, and the person asks again.
                let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), DEPART_KEY);
                None
            }
            kept::Kept::Foreign => {
                self.log.put(b"fgtw: a kept departure is not this identity's or this device's: starting fresh");
                self.log.line();
                None
            }
            kept::Kept::Unreadable(e) => {
                self.log.put(b"fgtw: a kept departure does not read (");
                self.log.put(e.as_bytes());
                self.log.put(b"): starting fresh");
                self.log.line();
                None
            }
        }
    }

    /// The machine's state into the vault (`DEPART_KEY`): no handle proof in it. Whether it was kept.
    fn depart_keep(&mut self) -> bool {
        let Some(m) = self.dep.m.as_ref() else { return false };
        let kept = match m.to_vsf(eagle()) {
            Ok(doc) => self.store_set_whole(DEPART_KEY, &doc),
            Err(_) => false,
        };
        if !kept {
            self.log.put(b"fgtw: the departure state was not kept in the vault: a restart before it completes drops it");
            self.log.line();
        }
        kept
    }

    /// A chain's fold for the machine (every adoption: the vault's, a sibling's, the seed's): its members and the locked ones. A chain with fewer ops than one already told is older than the fleet as told and is left out.
    pub fn depart_fold(&mut self, blob: &fgtw::fleet::MembershipBlob) {
        if self.dep.m.is_none() || blob.ops.len() < self.dep.ops_told {
            return;
        }
        let (Ok(members), Ok(locked)) = (blob.fold(), blob.locked_out()) else { return };
        self.dep.ops_told = blob.ops.len();
        self.depart_event(Flow::Members { members: &members, locked: &locked });
    }

    /// A departure frame (`depart_req`, `depart_deny`) from the LAN (its source address known) or the space (none).
    pub fn depart_frame(&mut self, from: Option<Addr>, bytes: &[u8]) {
        let from = from.as_ref().map(route::to_socket);
        self.depart_event(Flow::Frame { bytes, from });
    }

    /// PT's word on a frame it carried: the machine's own sends only, by address and the frame's hash.
    pub fn depart_sent(&mut self, to: &Addr, hash: &[u8; 32], ok: bool) {
        if let Some(device) = self.dep.sends.answered(to, hash) {
            self.depart_event(Flow::Sent { to: device, ok });
        }
    }

    /// A member showed it is reachable (its pong, its beacon, a frame from it): handed what it has not taken.
    pub fn depart_reachable(&mut self, from: [u8; 32]) {
        if from != self.me {
            self.depart_event(Flow::Reachable { from });
        }
    }

    /// Carry out the machine's actions.
    fn depart_carry(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Send { to: To::Device(k), frame } => self.depart_send(k, frame),
                Action::Send { to: To::Addr(s), frame } => {
                    let out = self.pt.send(route::from_socket(&s), frame, now_us(), eagle());
                    self.send_all(out);
                }
                Action::Seen { device, addr } => self.learn(device, route::from_socket(&addr)),
                Action::Build { id, frame } => self.dep.work.push(Work::Build { id, frame }),
                Action::Job(Job::Approve { leaver, t, consent_sig, intent }) => {
                    self.dep.result = 1;
                    self.dep.work.push(Work::Approve { leaver, t, consent_sig, intent });
                }
                Action::NfcServe(s) => self.depart_nfc(s),
                Action::Persist => {
                    self.depart_keep();
                }
                // ferros's fleet-shared record of a deny is the space's entry, which takes the signed deny once it is built (`depart_work`).
                Action::RecordDenied { leaver, .. } => {
                    self.log.put(b"fgtw: the departure of ");
                    self.say_device(&leaver, self.identity_seed);
                    self.log.put(b" refused for good: the fleet's devices are told");
                    self.log.line();
                }
                // ferros keeps no list of released devices: the release is fgtw.org's (its device-owner index), said here.
                Action::RecordReleased { device } => {
                    self.log.put(b"fgtw: ");
                    self.say_device(&device, self.identity_seed);
                    self.log.put(b" released for its new owner");
                    self.log.line();
                }
                Action::PushChain { to } => self.dep.work.push(Work::PushChain { to }),
                Action::RefreshChain => self.dep.work.push(Work::RefreshChain),
                Action::Note(n) => self.depart_note(n),
                Action::Wipe => {
                    self.depart_wipe();
                    return;
                }
                // The Add machine's actions (radios, chain news): never from the Depart machine, which shares the Action type.
                _ => {}
            }
        }
    }

    /// A frame for device `k`: by PT to where it was last heard, its answer mapped back to `k`. With no address yet it is reported given up (the machine hands it when the device is heard), and the sponsor named at spawn gets it once.
    fn depart_send(&mut self, k: [u8; 32], frame: Vec<u8>) {
        let hash = *blake3::hash(&frame).as_bytes();
        match self.sponsors.iter().find(|s| s.key == k).map(|s| s.addr) {
            Some(to) => {
                self.dep.sends.record(to, hash, k);
                self.log.put(b"fgtw: departure: a frame to ");
                self.log.fp(&k);
                self.log.put(b" at ");
                self.log.addr(&to);
                self.log.line();
                let out = self.pt.send(to, frame, now_us(), eagle());
                self.send_all(out);
            }
            None => {
                self.dep.unsent.push(k);
                let Some(to) = self.given else { return };
                if self.dep.given_sent == Some(hash) {
                    return;
                }
                self.dep.given_sent = Some(hash);
                self.log.put(b"fgtw: departure: a frame to the sponsor named at spawn, ");
                self.log.addr(&to);
                self.log.line();
                let out = self.pt.send(to, frame, now_us(), eagle());
                self.send_all(out);
            }
        }
    }

    /// A member's address learnt from a frame it sent: kept as a fleet device heard, so frames reach it there.
    pub fn learn(&mut self, key: [u8; 32], addr: Addr) {
        if key == self.me {
            return;
        }
        let t = now_us();
        match self.sponsors.iter_mut().find(|s| s.key == key) {
            Some(s) => {
                s.addr = addr;
                s.heard_us = t;
            }
            None => {
                self.log.put(b"fgtw: fleet device ");
                self.log.fp(&key);
                self.log.put(b" heard at ");
                self.log.addr(&addr);
                self.log.line();
                self.sponsors.push(Sponsor { key, addr, asked_t: 0, heard_us: t, online: false });
            }
        }
    }

    /// The departure's salt served on the NFC tag while our request stands (a tap of a fleet device approves it there), or the tag stopped. CALL_LIVE: a driver that is down fails at once.
    fn depart_nfc(&mut self, salt: Option<[u8; 32]>) {
        let nfc = cap(unsafe { GRANTS }, S_NFC);
        let mut r = [0u8; 4];
        let n = match salt {
            Some(s) => {
                let mut m = [0u8; 33];
                m[0] = 1;
                m[1..].copy_from_slice(&s);
                ferros_sys::call_live(nfc, &m, &mut r)
            }
            None => ferros_sys::call_live(nfc, &[2], &mut r),
        };
        self.log.put(if salt.is_some() { b"fgtw: departure: our request's salt on the NFC tag: ".as_slice() } else { b"fgtw: departure: the NFC tag stopped: " });
        self.log.put(if n == ERR { b"not running".as_slice() } else if n >= 1 && r[0] != 0 { b"refused" } else { b"ok" });
        self.log.line();
    }

    /// What the machine says the person should be told: the bytes the Fleet page reads (op 13's result, our request denied) and a line on the console.
    fn depart_note(&mut self, n: Note) {
        use fgtw::sponsor::Approved;
        let seed = self.identity_seed;
        match n {
            // The Fleet page says it in the person's language (op 5's 4, `fgtw::text::Msg::LastDevice`).
            Note::LastDevice => self.log.put(b"fgtw: this is the identity's last device (or its fleet is not known yet): it cannot ask to leave"),
            Note::DepartAsking => {
                self.dep.denied_by = None;
                self.log.put(b"fgtw: our departure request is out: Approve or Deny on any of this identity's devices");
            }
            Note::DepartNotSent(e) => {
                self.log.put(b"fgtw: the departure request did not build: ");
                self.log.put(e.as_bytes());
            }
            Note::DepartDenied { by } => {
                self.dep.denied_by = Some(by);
                self.log.put(b"fgtw: our departure was denied by ");
                self.say_device(&by, seed);
                self.log.put(b": this device stays in the fleet");
            }
            Note::DepartDone => self.log.put(b"fgtw: the fleet's chain no longer holds this device: departed"),
            Note::DepartAsked { leaver } => {
                self.dep.result = 0;
                self.log.put(b"fgtw: ");
                self.say_device(&leaver, seed);
                self.log.put(b" asks to leave the fleet: Approve or Deny on the Fleet page");
            }
            Note::DepartDeniedHere { leaver } => {
                self.log.put(b"fgtw: the departure of ");
                self.say_device(&leaver, seed);
                self.log.put(b" denied here");
            }
            Note::DepartDeniedElsewhere { leaver, by } => {
                self.log.put(b"fgtw: the departure of ");
                self.say_device(&leaver, seed);
                self.log.put(b" was denied by ");
                self.say_device(&by, seed);
            }
            Note::DepartSettled { leaver } => {
                self.log.put(b"fgtw: the departure of ");
                self.say_device(&leaver, seed);
                self.log.put(b" was settled on another device");
            }
            Note::DepartApproved { leaver, outcome } => {
                self.dep.result = 0;
                self.log.put(b"fgtw: the departure of ");
                self.say_device(&leaver, seed);
                match outcome {
                    Approved::BrandKept => self.log.put(b" approved: the consented Remove is published"),
                    Approved::Released(Ok(())) => self.log.put(b" approved: the consented Remove is published and the device released for its new owner"),
                    Approved::Released(Err(e)) => {
                        self.log.put(b" approved: the consented Remove is published; the release failed: ");
                        self.log.put(e.as_bytes());
                    }
                }
            }
            Note::DepartWithdrawn { leaver } => {
                self.dep.result = 0;
                self.log.put(b"fgtw: the departure of ");
                self.say_device(&leaver, seed);
                self.log.put(b" was given up on that device: no longer asked here");
            }
            Note::DepartApproveFailed { leaver, reason } => {
                self.dep.result = 3;
                self.log.put(b"fgtw: the departure of ");
                self.say_device(&leaver, seed);
                self.log.put(b" was not published: ");
                self.log.put(reason.as_bytes());
            }
            // The Add machine's notes: never from the Depart machine, which shares the Note type.
            _ => {}
        }
        self.log.line();
    }

    /// This device has left the fleet: the identity's roots, chain and key forgotten here, on disk and in RAM, and the kernel's binding released.
    fn depart_wipe(&mut self) {
        self.log.put(b"fgtw: departed; the identity is forgotten here");
        self.log.line();
        let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), DEPART_KEY);
        self.forget_disk();
        self.forget_identity();
        if !ferros_sys::release_identity(cap(unsafe { GRANTS }, S_UNLOCK)) {
            self.log.put(b"fgtw: the kernel did not release the device's identity space");
            self.log.line();
        }
    }

    /// The machine's slow work, once a pass of the loop: signing, an approval's publish, a chain read. Each answer goes back to the machine, which may queue more.
    pub fn depart_work(&mut self) {
        while !self.dep.work.is_empty() {
            match self.dep.work.remove(0) {
                Work::Build { id, frame } => {
                    // On the X3, as every signing with the SLH-DSA egg here: seconds on a worker.
                    let t0 = now();
                    ferros_sys::pin(ferros_sys::CORES_BIG);
                    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
                    let r = fgtw::flow::build(&frame, &self.keys, eagle());
                    ferros_sys::pin(0);
                    self.log.put(b"fgtw: departure: a frame signed in ");
                    self.log.dec(ferros_sys::us_since(t0) / 1000);
                    self.log.put(b" ms");
                    self.log.line();
                    match &r {
                        Ok(bytes) => self.depart_event(Flow::Built { id, result: Ok(bytes) }),
                        Err(e) => self.depart_event(Flow::Built { id, result: Err(e) }),
                    }
                    // The space carries it too, for devices away from this network.
                    if let Ok(bytes) = r {
                        match frame {
                            FlowFrame::DepartReq { .. } => {
                                if self.depart_asking() {
                                    self.space_put_depart(&bytes);
                                }
                            }
                            FlowFrame::DepartDeny { leaver, .. } => self.space_deny_depart(&leaver, &bytes),
                            // The Add machine's frames (bind_select, fanout_resp): the Depart machine builds none.
                            _ => {}
                        }
                    }
                }
                Work::Approve { leaver, t, consent_sig, intent } => {
                    let Some(hp) = self.hp else { continue };
                    ferros_sys::pin(ferros_sys::CORES_BIG);
                    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
                    fgtw::set_clock(eagle());
                    let r = {
                        let s = self.seed();
                        let r = fgtw::sponsor::approve_departure(&s, &self.keys, &hp, &leaver, t, &consent_sig, intent);
                        self.seed_ip = s.ip.get();
                        r
                    };
                    ferros_sys::pin(0);
                    if r.is_ok() {
                        self.seed_heard_us = now_us();
                        self.space_drop_depart(&leaver);
                    }
                    self.depart_event(Flow::Approved { leaver, result: r.as_ref().map_err(|e| e.as_str()) });
                }
                // The chain the approval just published is what the leaving device must see: handed after the next read (`RefreshChain`, which the machine asks for right behind it).
                Work::PushChain { to } => self.dep.push_to.push(to),
                Work::RefreshChain => {
                    // The members without the leaver: the chain as the seed (or the held copy) now has it.
                    let Some(hp) = self.hp else { continue };
                    fgtw::set_clock(eagle());
                    let fetched = {
                        let s = self.seed();
                        fgtw::held::fetch(&s, &self.held, &hp)
                    };
                    if let Ok(Some(b)) = fetched {
                        if let Ok(bytes) = b.to_vsf_bytes() {
                            self.persist_set(b"chain", &bytes);
                        }
                        self.note_members(&b);
                    }
                    self.depart_push_chain(&hp);
                }
            }
        }
    }

    /// Our chain as held now to each leaving device approved here (`fleet_resp` by PT to where it was last heard): its fold no longer lists it, and it completes its departure at once. One with no address here learns it from fgtw.org (`depart_seed_check` on its side).
    fn depart_push_chain(&mut self, hp: &[u8; 32]) {
        let to: Vec<[u8; 32]> = core::mem::take(&mut self.dep.push_to);
        if to.is_empty() {
            return;
        }
        let Some(chain) = self.held.borrow().get(hp).and_then(|b| b.to_vsf_bytes().ok()) else { return };
        let Ok(frame) = fgtw::protocol::fleet_resp(hp, &chain, &self.keys, eagle()) else { return };
        for k in to {
            let Some(addr) = self.sponsors.iter().find(|s| s.key == k).map(|s| s.addr) else {
                self.log.put(b"fgtw: departure: no address here for ");
                self.say_device(&k, self.identity_seed);
                self.log.put(b": it reads the new chain from fgtw.org");
                self.log.line();
                continue;
            };
            self.log.put(b"fgtw: departure: our new chain to ");
            self.say_device(&k, self.identity_seed);
            self.log.line();
            let out = self.pt.send(addr, frame.clone(), now_us(), eagle());
            self.send_all(out);
        }
    }

    /// Our request while it stands: the seed's copy of the chain every 16 s, for the approver's consented Remove. An approver on this network hands us its new chain (`PushChain`); one this device cannot hear (another network, with no relay here, or a push given up while this device slept) publishes it only to fgtw.org, so it is read there. Its fold goes to the machine, which completes the departure once this device is not in it.
    pub fn depart_seed_check(&mut self, t: u64) {
        if !self.depart_asking() || t - self.dep.checked_us < 16_000_000 {
            return;
        }
        self.dep.checked_us = t;
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        fgtw::set_clock(eagle());
        let (fetched, ip) = {
            let s = self.seed();
            let r = fgtw::client::fetch(&s, &hp);
            (r, s.ip.get())
        };
        self.seed_ip = ip;
        if fetched.is_ok() {
            self.seed_heard_us = now_us();
        }
        let Ok(Some(b)) = fetched else { return };
        let Ok(bytes) = b.to_vsf_bytes() else { return };
        if let Ok((blob, _)) = join::adopt_chain(&hp, &seed, &self.me, &bytes) {
            self.depart_fold(&blob);
        }
    }

    /// Our request, once built, into the space.
    fn depart_space_own(&mut self) {
        let Some(frame) = self.dep.m.as_ref().and_then(|m| m.own_frame()).map(|f| f.to_vec()) else { return };
        self.space_put_depart(&frame);
    }

    /// The request asked about here, withdrawn: its leaving device gave it up (the space's entry for it gone after the request's stamp, so an entry left from an earlier request does not count). The machine stops asking (`GaveUp`).
    pub fn depart_withdrawn(&mut self, leaver: [u8; 32]) {
        let Some(t) = self.dep.m.as_ref().and_then(|m| m.asked_t()) else { return };
        if self.space_depart_gone(&leaver).is_some_and(|at| at > t) {
            self.depart_event(Flow::GaveUp { leaver, t });
        }
    }

    /// Op 5: leave the fleet. A fresh salt from the kernel's entropy names the request; the machine refuses on the last device and asks for the frame to be signed, which the loop does (`depart_work`). `intent` is 1 (to a new owner: the approver also releases the device, so another fleet can bind it) or 2 (to the desk). Returns 0 started, 1 not a member of a fleet, 2 an intent that is neither, 4 the identity's last device (or its fleet not known yet).
    pub fn leave(&mut self, intent: u8) -> u8 {
        if self.hp.is_none() || !self.bound || self.dep.m.is_none() {
            return 1;
        }
        if intent != join::DEPART_NEW_OWNER && intent != join::DEPART_DESK {
            return 2;
        }
        // A fresh salt names this request (owner, 2026-11-10: "upon deny another request will have to use a new salt").
        let mut salt = [0u8; 32];
        kernel_entropy(&mut salt);
        self.dep.denied_by = None;
        let Some(m) = self.dep.m.as_mut() else { return 1 };
        let step = m.step(Flow::User(UserOp::Leave { intent, salt }), eagle());
        let last = step.actions.iter().any(|a| matches!(a, Action::Note(Note::LastDevice)));
        self.depart_carry(step.actions);
        if last {
            return 4;
        }
        self.log.put(if intent == join::DEPART_NEW_OWNER { b"fgtw: leaving the fleet (to a new owner): signing the request".as_slice() } else { b"fgtw: leaving the fleet: signing the request" });
        self.log.line();
        0
    }

    /// Op 20: our request given up: forgotten here, in the vault and in the space, so no member is asked to approve it. 0, or 1 when there is none.
    pub fn give_up_depart(&mut self) -> u8 {
        if !self.depart_asking() {
            return 1;
        }
        self.depart_event(Flow::User(UserOp::GiveUp));
        let me = self.me;
        self.space_drop_depart(&me);
        self.log.put(b"fgtw: the departure given up: this device stays in the fleet");
        self.log.line();
        0
    }

    /// Op 14, the person's answer to the request asked about here (owner, 2026-11-10: a yes or no question): `[14][1]` Approve (the loop countersigns), `[14][0]` Deny. Returns 0 done, 1 none waiting.
    pub fn answer_depart(&mut self, approve: bool) -> u8 {
        if self.depart_asked().is_none() {
            return 1;
        }
        self.depart_event(Flow::User(if approve { UserOp::Approve } else { UserOp::Deny }));
        0
    }

    /// Op 13, a request asked about here: [1][intent][result: 0 waiting, 1 approving, 3 failed][name length][name]; or our own request denied: [2][0][0][name length][the name of the device that denied it]; else [0][0][0][0]. An approved request is gone from here at once (the map says it left).
    pub fn depart_ask_reply(&self) -> Vec<u8> {
        let mut r = Vec::with_capacity(48);
        match (self.depart_asked(), self.dep.denied_by, self.identity_seed) {
            (Some((leaver, intent, _)), _, Some(seed)) => {
                let name = fgtw::pair::device_name_default(&leaver, &seed);
                r.extend_from_slice(&[1, intent, self.dep.result, name.len() as u8]);
                r.extend_from_slice(name.as_bytes());
            }
            (None, Some(by), Some(seed)) => {
                let name = fgtw::pair::device_name_default(&by, &seed);
                r.extend_from_slice(&[2, 0, 0, name.len() as u8]);
                r.extend_from_slice(name.as_bytes());
            }
            _ => r.extend_from_slice(&[0, 0, 0, 0]),
        }
        r
    }
}

/// The denied list kept before the machine (`DENIED_KEY`), each request by type marker: the leaving device `ke`, the salt `hb`, the consent stamp `e`. A document that does not verify reads as empty.
fn read_denied(doc: &[u8]) -> Vec<([u8; 32], [u8; 32], i64)> {
    let Ok((header, end)) = vsf::verification::read_verified(doc, None) else { return Vec::new() };
    let Ok(section) = header.primary_section(doc, end) else { return Vec::new() };
    let mut out = Vec::new();
    for f in section.get_fields("request") {
        let (mut leaver, mut salt, mut t) = (None, None, None);
        for v in &f.values {
            match v {
                vsf::VsfType::ke(b) => leaver = <[u8; 32]>::try_from(b.as_slice()).ok(),
                vsf::VsfType::hb(b) => salt = <[u8; 32]>::try_from(b.as_slice()).ok(),
                vsf::VsfType::e(_) => t = v.as_eagle(),
                _ => {}
            }
        }
        if let (Some(l), Some(s), Some(t)) = (leaver, salt, t) {
            out.push((l, s, t));
        }
    }
    out
}
