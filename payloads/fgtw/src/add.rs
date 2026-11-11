//! Adding a device to the fleet, the sponsor's side, on fgtw's shared machine (`fgtw::flow::add`, FLOWS.md stage 3, the one Photon runs): the Fleet page shown opens the flow and the radios, an NFC tap or fully typed words add at once, a touch on a row selects (the device turns green, Yes adds, No drops the pick), only a member holding the fleet key adds, locked and owned-elsewhere devices are refused, and a grow never reaches a locked device.
//! This file decides nothing about an add: it tells the machine what happened and carries out what it asks.
//!
//! - **Made** once the roots are in hand (`add_start`), from its state in the vault when there is one, then told the fold of every chain adopted (`add_fold`: members, locked devices, locked plates) and whether this device holds the fleet key (`add_key`). An add under way when the device stopped is resumed only once a fold has been told (`add_resume`): a grow resumed before the members are known would seal the key to the newcomer alone.
//! - **Kept** in the `fgtw.` scope as `add` (`ADD_KEY`), whatever unlock-at-boot says: the machine's state as one VSF document (`Add::to_vsf`), the newcomer's key and how far its add got; no root is in it, the roots in hand give it back when it is read (kept.rs).
//! - **Fed**: the Fleet page shown or gone (op 8), a `bind_req` from the LAN with its source address, a beacon under our handle from a device not in the fold (`LanHeard`), the BLE beacon ids the Bluetooth driver lists (op 5: a new id at once, every listed id again at the machine's wake), a tag's secret the NFC reader read (op 6), the person's touch (op 10), Yes (op 12), No (op 8 [1] while a device is selected, the map's No), words typed (op 11), "type its words" (op 15 [1], the edge that reads fgtw.org's registry), PT's word on each send, a member reachable, a network up, and each job's answer.
//! - **Slow work** (signing a frame, the registry's read, the bind, the grow, our chain to a device or the fleet) is queued and run by the loop (`add_work`), pinned to the X3 where it signs or publishes, never inside a call another process waits on.
//! - **Sends** go by Photon Transport to the address the device was last heard at: a member's from the fleet devices heard (`sponsors`), a newcomer's from its `bind_req` (`Seen`). PT's answer names the frame by its hash, mapped back to the device (route.rs). A device with no address here is reported given up at once (ferros has no relay): a selected device so reached is `PickUnreachable`, and a fan-out member is handed the key when it is next heard.
//! - **The Fleet page** (payloads/map) reads op 9, built from the machine's view (addview.rs), and the doorbell; each `Note` sets what op 9's note says and puts a line on the console.

use super::*;
use alloc::string::String;
use fgtw::flow::add::{Add, Phase, View};
use fgtw::flow::{Action, Event as Flow, Frame as FlowFrame, Job, Note, To, UserOp};

/// The kept add's name in the `fgtw.` scope.
pub const ADD_KEY: &[u8] = b"add";

/// Slow work the machine asked for, run by the loop.
pub enum Work {
    /// Sign this frame with every egg (`fgtw::flow::build`) and answer `Built`.
    Build { id: u32, frame: FlowFrame },
    /// Read fgtw.org's binding requests for our fleet (`fgtw::client::bindreq_list`) and which belong to another identity's fleet, and answer `Listed`.
    List,
    /// Add the device (`fgtw::sponsor::bind_device_held`) and answer `Bound`.
    Bind { req: fgtw::fleet::BindRequest },
    /// Grow the fan-out to `members` (`fgtw::held::fanout_grow_held`) and answer `Grown`.
    Grow { device: [u8; 32], members: Vec<[u8; 32]> },
    /// Our chain as held (`fleet_resp`) to this device, before a select so it can check the selecting device.
    PushChain { to: [u8; 32] },
    /// Our chain as held to every fleet device heard and to `extra` (the newcomer).
    ChainNews { extra: Option<[u8; 32]> },
}

/// The add's executor, in the client.
pub struct Sponsoring {
    /// The machine, while the roots are in hand.
    pub m: Option<Add>,
    work: Vec<Work>,
    sends: route::Sends,
    /// Sends the machine asked for that never reached PT (no address for the device here): reported given up once the step's actions are carried out.
    unsent: Vec<[u8; 32]>,
    /// Newcomers' addresses, from their `bind_req` on this network (`Seen`).
    addrs: Vec<([u8; 32], Addr)>,
    /// The Fleet page shown, as op 8 last said: told to a machine made after it.
    shown: bool,
    /// The radios the machine wants: (the NFC reader, the BLE scan).
    pub radios: (bool, bool),
    /// The machine's one deadline (Eagle oscillations): a request or a nearby mark lapses then.
    wake: Option<i64>,
    /// The fold told last (members, locked) and its chain's op count: a chain with fewer ops is older than the fleet as told and is not told.
    members: Vec<[u8; 32]>,
    locked: Vec<[u8; 32]>,
    ops_told: usize,
    /// A kept add waits for the first fold before it resumes.
    resumed: bool,
    /// The beacon ids the Bluetooth driver listed at its last read, and how many and how many of the rows matched one when last said.
    ble: Vec<[u8; fgtw::pair::BEACON_UUID_LEN]>,
    ble_said: (usize, usize),
    /// What op 9's note says (the last `Note`), and the page refused for want of the fleet key.
    note: String,
    refused: bool,
    /// The device the add under way is for (op 9's `ROW_ADDING`).
    adding: Option<[u8; 32]>,
}

impl Sponsoring {
    pub const fn new() -> Sponsoring {
        Sponsoring { m: None, work: Vec::new(), sends: route::Sends::new(), unsent: Vec::new(), addrs: Vec::new(), shown: false, radios: (false, false), wake: None, members: Vec::new(), locked: Vec::new(), ops_told: 0, resumed: false, ble: Vec::new(), ble_said: (0, 0), note: String::new(), refused: false, adding: None }
    }

    /// Everything of an identity forgotten (new roots, a departure's wipe); whether the page shows is the map's and stays.
    pub fn reset(&mut self) {
        let shown = self.shown;
        *self = Sponsoring::new();
        self.shown = shown;
    }
}

impl Client {
    /// The machine's view (the screen as it stands), when it is made.
    pub fn add_view(&self) -> Option<View> {
        self.add.m.as_ref().map(|m| m.view())
    }

    /// The add flow's phase, Closed with no machine.
    pub fn add_phase(&self) -> Phase {
        self.add.m.as_ref().map_or(Phase::Closed, |m| m.view().phase)
    }

    /// One event to the machine, its actions carried out. Its notes are returned too, for the op that caused them to read its answer from.
    pub fn add_event(&mut self, ev: Flow) -> Vec<Note> {
        let Some(m) = self.add.m.as_mut() else { return Vec::new() };
        let step = m.step(ev, eagle());
        self.add.wake = step.wake.map(|(t, _)| t);
        let notes = step.actions.iter().filter_map(|a| if let Action::Note(n) = a { Some(n.clone()) } else { None }).collect();
        self.add_carry(step.actions);
        self.add_unsent();
        notes
    }

    /// The sends that never reached PT, told to the machine as given up.
    fn add_unsent(&mut self) {
        while let Some(to) = self.add.unsent.pop() {
            let Some(m) = self.add.m.as_mut() else { return };
            let step = m.step(Flow::Sent { to, ok: false }, eagle());
            self.add.wake = step.wake.map(|(t, _)| t);
            self.add_carry(step.actions);
        }
    }

    /// The machine made, once the roots are in hand: from the vault or fresh, told whether this device holds the fleet key, the fold of the chain held, and the page if it shows.
    pub fn add_start(&mut self) {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        if self.add.m.is_some() {
            return;
        }
        let kept = self.store_get_whole(ADD_KEY).and_then(|doc| match kept::read_add(&doc, &hp, &seed, &self.me) {
            kept::KeptAdd::Machine(m) => Some(m),
            kept::KeptAdd::Foreign => {
                self.log.put(b"fgtw: a kept add is not this device's: starting fresh");
                self.log.line();
                None
            }
            kept::KeptAdd::Unreadable(e) => {
                self.log.put(b"fgtw: a kept add does not read (");
                self.log.put(e.as_bytes());
                self.log.put(b"): starting fresh");
                self.log.line();
                None
            }
        });
        // A kept add resumes once a fold is told; a fresh machine has nothing to resume.
        self.add.resumed = kept.is_none();
        self.add.m = Some(kept.unwrap_or_else(|| Add::new(hp, seed, self.me, kept::REPORTS_SENT)));
        self.add_key();
        let held = self.held.borrow().get(&hp).cloned();
        if let Some(blob) = held {
            self.add_fold(&blob);
        }
        if self.add.shown {
            self.add_event(Flow::Page { shown: true });
        }
    }

    /// A kept add carried on, once the members are known.
    fn add_resume(&mut self) {
        if self.add.resumed {
            return;
        }
        self.add.resumed = true;
        let Some(m) = self.add.m.as_mut() else { return };
        let step = m.resume();
        if !step.actions.is_empty() {
            self.log.put(b"fgtw: resuming the add kept from before the restart");
            self.log.line();
        }
        self.add_carry(step.actions);
        self.add_unsent();
    }

    /// Whether this device holds the fleet key, told to the machine (it opens a page already shown when the key arrives, and so when the fold that names this device a member arrives after the page was shown: `add_fold` tells it again). A refusal the page showed is gone once the flow opens.
    pub fn add_key(&mut self) {
        let held = self.fleet_key.is_some();
        self.add_event(Flow::FleetKey { held });
        if self.add.refused && self.add_phase() != Phase::Closed {
            self.add.refused = false;
            self.add.note.clear();
        }
    }

    /// A chain's fold for the machine (every adoption): its members, the locked devices and their plates. A chain with fewer ops than one already told is older and is left out.
    pub fn add_fold(&mut self, blob: &fgtw::fleet::MembershipBlob) {
        if self.add.m.is_none() || blob.ops.len() < self.add.ops_told {
            return;
        }
        let (Ok(members), Ok(locked), Ok(plates)) = (blob.fold(), blob.locked_out(), blob.locked_plates()) else { return };
        self.add.ops_told = blob.ops.len();
        self.add_event(Flow::LockedPlates { plates: &plates });
        self.add_event(Flow::Members { members: &members, locked: &locked });
        self.add.members = members;
        self.add.locked = locked;
        // The machine opens a shown page on the fleet key's edge only; a page shown before this device's fold was known (unlock-at-boot off: the chain comes after the roots) opens now.
        if self.add.shown {
            self.add_key();
        }
        self.add_resume();
    }

    /// A `bind_req` from the LAN, with the address it came from.
    pub fn add_frame(&mut self, from: Addr, bytes: &[u8]) {
        let from = Some(route::to_socket(&from));
        self.add_event(Flow::Frame { bytes, from });
    }

    /// A beacon under our handle from a device not in the fold: a newcomer on this network.
    pub fn add_lan_heard(&mut self, device: [u8; 32]) {
        if self.add.m.is_some() && !self.add.members.contains(&device) {
            self.add_event(Flow::LanHeard { device });
        }
    }

    /// PT's word on a frame it carried: the machine's own sends only, by address and the frame's hash.
    pub fn add_sent(&mut self, to: &Addr, hash: &[u8; 32], ok: bool) {
        if let Some(device) = self.add.sends.answered(to, hash) {
            self.add_event(Flow::Sent { to: device, ok });
        }
    }

    /// The Bluetooth driver's list (op 5): a new id told at once, the list kept for the wake.
    pub fn add_ble(&mut self, ids: Vec<[u8; fgtw::pair::BEACON_UUID_LEN]>) {
        let fresh: Vec<[u8; fgtw::pair::BEACON_UUID_LEN]> = ids.iter().filter(|id| !self.add.ble.contains(id)).copied().collect();
        self.add.ble = ids;
        for id in fresh {
            self.add_event(Flow::BeaconHeard { id });
        }
        // Measuring the nearby path: how many beacon ids Bluetooth hears, and how many devices asking match one, said when either changes.
        let Some(v) = self.add_view() else { return };
        let matched = v.rows.iter().filter(|r| r.ble).count();
        if (self.add.ble.len(), matched) != self.add.ble_said {
            self.add.ble_said = (self.add.ble.len(), matched);
            self.log.put(b"fgtw: Bluetooth hears ");
            self.log.dec(self.add.ble.len() as u64);
            self.log.put(b" service id(s); ");
            self.log.dec(matched as u64);
            self.log.put(b" of the ");
            self.log.dec(v.rows.len() as u64);
            self.log.put(b" device(s) asking to join match one");
            self.log.line();
        }
    }

    /// A tag's secret read while the add flow listens: the machine adds the device whose request it opens.
    pub fn add_nfc(&mut self, secret: [u8; 32]) {
        let seen = self.add_event(Flow::NfcRead { secret });
        if !seen.iter().any(|n| matches!(n, Note::Adding { .. })) {
            self.log.put(b"fgtw: a tapped tag's secret matches no device asking to join (a stale request, or not a newcomer's)");
            self.log.line();
        }
    }

    /// The machine's deadline, checked once a pass of the loop: what lapses then is told by the fold again (the machine rebuilds its rows on it), after every beacon id the driver still lists, so a device still heard stays nearby.
    pub fn add_wake_check(&mut self) {
        let Some(at) = self.add.wake else { return };
        if eagle() < at {
            return;
        }
        self.add.wake = None;
        for id in self.add.ble.clone() {
            self.add_event(Flow::BeaconHeard { id });
        }
        // Before any fold there is no fleet to tell again (and the flow cannot be open, so no wake is asked for).
        if self.add.ops_told == 0 {
            return;
        }
        let (members, locked) = (self.add.members.clone(), self.add.locked.clone());
        self.add_event(Flow::Members { members: &members, locked: &locked });
    }

    /// Op 8: the Fleet page shown (1) or gone (0). The map's No on a selected device is also [8][1] (No does what Back does): it drops the pick. Answers 0.
    pub fn add_page(&mut self, on: bool) -> u8 {
        if on && self.add_phase() == Phase::Selected {
            self.add_event(Flow::User(UserOp::No));
            self.add.note.clear();
            return 0;
        }
        self.add.shown = on;
        self.add.refused = false;
        self.add.note.clear();
        self.add_event(Flow::Page { shown: on });
        0
    }

    /// Op 10: a row touched, by the first 8 bytes of its key: selected, nothing bound. Returns 0 selected, 1 no such pickable row (or refused: the note says why), 2 the flow is not open or an add is under way.
    pub fn add_pick(&mut self, key8: &[u8]) -> u8 {
        let Some(v) = self.add_view() else { return 2 };
        if v.phase != Phase::Open {
            return 2;
        }
        let Some(device) = v.rows.iter().find(|r| r.device[..8] == *key8).map(|r| r.device) else { return 1 };
        let seen = self.add_event(Flow::User(UserOp::Touch { device }));
        if seen.iter().any(|n| matches!(n, Note::Picked { .. })) {
            0
        } else if seen.contains(&Note::AddBusy) {
            2
        } else {
            1
        }
    }

    /// Op 12: Yes, the selected device is bright green: it is added. Returns 0 taken, 1 nothing selected (or refused: the note says why).
    pub fn add_yes(&mut self) -> u8 {
        if self.add_phase() != Phase::Selected {
            return 1;
        }
        let seen = self.add_event(Flow::User(UserOp::Yes));
        if seen.iter().any(|n| matches!(n, Note::Adding { .. })) { 0 } else { 1 }
    }

    /// Op 11: the words typed so far, matched by the machine; a full match adds at once. The answer as addview.rs `words` builds it.
    pub fn add_words(&mut self, text: &str) -> Vec<u8> {
        if self.add_phase() != Phase::Open {
            return alloc::vec![6, 0];
        }
        let seen = self.add_event(Flow::Words { text });
        let added = seen.iter().find_map(|n| match n {
            Note::Adding { device } => Some(*device),
            _ => None,
        });
        let name = match (added, self.identity_seed) {
            (Some(d), Some(seed)) => Some(fgtw::pair::device_name_default(&d, &seed)),
            _ => None,
        };
        match self.add_view() {
            Some(v) => addview::words(&v, name.as_deref()),
            None => alloc::vec![6, 0],
        }
    }

    /// Op 15 [1]: the person opens "type its words", the edge that reads fgtw.org's registry (a newcomer reached only by its words is known only there).
    pub fn add_look(&mut self) {
        self.add_event(Flow::User(UserOp::LookFarther));
    }

    /// Op 9, the add flow for the Fleet page (addview.rs `reply`).
    pub fn add_reply(&self) -> Vec<u8> {
        match self.add_view() {
            Some(v) => addview::reply(&v, self.add.refused, self.add.adding, &self.add.note, self.net_up),
            None => alloc::vec![0, 0, 0, 0, 0, 0],
        }
    }

    /// Carry out the machine's actions.
    fn add_carry(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Send { to: To::Device(k), frame } => self.add_send(k, frame),
                Action::Send { to: To::Addr(s), frame } => {
                    let out = self.pt.send(route::from_socket(&s), frame, now_us(), eagle());
                    self.send_all(out);
                }
                Action::Seen { device, addr } => {
                    let addr = route::from_socket(&addr);
                    match self.add.addrs.iter_mut().find(|(k, _)| *k == device) {
                        Some(e) => e.1 = addr,
                        None => {
                            self.add.addrs.push((device, addr));
                            self.log.put(b"fgtw: ");
                            self.say_device(&device, self.identity_seed);
                            self.log.put(b" asks to join from ");
                            self.log.addr(&addr);
                            self.log.line();
                        }
                    }
                }
                Action::Build { id, frame } => self.add.work.push(Work::Build { id, frame }),
                Action::Job(Job::ListRequests) => self.add.work.push(Work::List),
                Action::Job(Job::Bind { req }) => self.add.work.push(Work::Bind { req }),
                Action::Job(Job::Grow { device, members }) => self.add.work.push(Work::Grow { device, members }),
                Action::PushChain { to } => self.add.work.push(Work::PushChain { to }),
                Action::ChainNews { extra } => self.add.work.push(Work::ChainNews { extra }),
                Action::Radios { ble_scan, nfc_reader } => self.add.radios = (nfc_reader, ble_scan),
                Action::Persist => self.add_keep(),
                Action::Note(n) => self.add_note(n),
                // The departure's actions: the add machine asks for none of them.
                Action::Job(Job::Approve { .. }) | Action::NfcServe(_) | Action::RecordDenied { .. } | Action::RecordReleased { .. } | Action::RefreshChain | Action::Wipe => {}
            }
        }
    }

    /// The machine's state into the vault (`ADD_KEY`): no root in it.
    fn add_keep(&mut self) {
        let Some(m) = self.add.m.as_ref() else { return };
        let kept = match m.to_vsf(eagle()) {
            Ok(doc) => self.store_set_whole(ADD_KEY, &doc),
            Err(_) => false,
        };
        if !kept {
            self.log.put(b"fgtw: the add's state was not kept in the vault: a restart before it completes drops it");
            self.log.line();
        }
    }

    /// Where device `k` was last heard: a fleet device's beacon, ping or frame, else a newcomer's request.
    fn add_addr(&self, k: &[u8; 32]) -> Option<Addr> {
        self.sponsors.iter().find(|s| s.key == *k).map(|s| s.addr).or_else(|| self.add.addrs.iter().find(|(d, _)| d == k).map(|(_, a)| *a))
    }

    /// A frame for device `k`: by PT to where it was last heard, its answer mapped back to `k`. With no address here it is reported given up (no relay on ferros).
    fn add_send(&mut self, k: [u8; 32], frame: Vec<u8>) {
        let Some(to) = self.add_addr(&k) else {
            self.add.unsent.push(k);
            return;
        };
        let hash = *blake3::hash(&frame).as_bytes();
        self.add.sends.record(to, hash, k);
        let out = self.pt.send(to, frame, now_us(), eagle());
        self.send_all(out);
    }

    /// What the machine says the person should be told: op 9's note and a line on the console.
    fn add_note(&mut self, n: Note) {
        let seed = self.identity_seed;
        match n {
            Note::AddNoFleetKey => {
                self.add.refused = self.add_phase() == Phase::Closed;
                self.add.note = String::from("this device does not hold the fleet key, so it cannot add devices");
                self.log.put(b"fgtw: this device does not hold the fleet key: it cannot add devices");
            }
            // Join notices are LAN only (owner, 2026-11-10: "LAN, fine. WAN, nope"): the machine says it only of a request heard on this network; the Fleet page says it from op 9's rows.
            Note::AsksToJoin { device } => {
                self.log.put(b"fgtw: ");
                self.say_device(&device, seed);
                self.log.put(b" asks to join on this network: open the Fleet page to add it");
            }
            Note::Picked { device } => {
                self.add.note.clear();
                self.log.put(b"fgtw: selected ");
                self.say_device(&device, seed);
                self.log.put(b": nothing bound, Yes adds it");
            }
            Note::PickUnreachable { device } => {
                self.add.note = String::from("it has no address on this network (no relay here): press No and type its words");
                self.log.put(b"fgtw: the selected ");
                self.say_device(&device, seed);
                self.log.put(b" could not be reached on this network: it is not told");
            }
            Note::AddBusy => {
                self.add.note = String::from("another add is under way");
                self.log.put(b"fgtw: another add is under way");
            }
            Note::AddLockedOut { device } => {
                self.add.note = String::from("this is a device you locked out");
                self.log.put(b"fgtw: ");
                self.say_device(&device, seed);
                self.log.put(b" is a device this fleet locked out: refused");
            }
            Note::AddOwnedElsewhere { device } => {
                self.add.note = String::from("this device belongs to another identity's fleet");
                self.log.put(b"fgtw: ");
                self.say_device(&device, seed);
                self.log.put(b" belongs to another identity's fleet: refused");
            }
            Note::Adding { device } => {
                self.add.adding = Some(device);
                self.add.note.clear();
                self.log.put(b"fgtw: adding ");
                self.say_device(&device, seed);
                self.log.put(b" to the fleet");
            }
            Note::Added { device } => {
                self.add.adding = None;
                self.add.note = String::from("added");
                self.log.put(b"fgtw: ");
                self.say_device(&device, seed);
                self.log.put(b" is in the fleet; the fleet key is on its way to it and the members");
            }
            Note::AddFailed { device, reason } => {
                self.add.adding = None;
                self.log.put(b"fgtw: adding ");
                self.say_device(&device, seed);
                self.log.put(b" failed: ");
                self.log.put(reason.as_bytes());
                self.add.note = reason;
            }
            // The departure's notes: the add machine says none of them.
            Note::LastDevice | Note::DepartAsking | Note::DepartNotSent(_) | Note::DepartDenied { .. } | Note::DepartDone | Note::DepartAsked { .. } | Note::DepartDeniedHere { .. } | Note::DepartDeniedElsewhere { .. } | Note::DepartSettled { .. } | Note::DepartWithdrawn { .. } | Note::DepartApproved { .. } | Note::DepartApproveFailed { .. } => return,
        }
        self.log.line();
    }

    /// Our chain as held, as a `fleet_resp` signed on the X3; None with no chain held.
    fn add_chain_frame(&mut self, hp: &[u8; 32]) -> Option<Vec<u8>> {
        let chain = self.held.borrow().get(hp).and_then(|b| b.to_vsf_bytes().ok())?;
        ferros_sys::pin(ferros_sys::CORES_BIG);
        ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
        let r = fgtw::protocol::fleet_resp(hp, &chain, &self.keys, eagle());
        ferros_sys::pin(0);
        match r {
            Ok(f) => Some(f),
            Err(e) => {
                self.log.put(b"fgtw: the chain frame did not build: ");
                self.log.put(e.as_bytes());
                self.log.line();
                None
            }
        }
    }

    /// The machine's slow work, once a pass of the loop. Each answer goes back to the machine, which may queue more.
    pub fn add_work(&mut self) {
        while !self.add.work.is_empty() {
            let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else {
                self.add.work.clear();
                return;
            };
            match self.add.work.remove(0) {
                Work::Build { id, frame } => {
                    // On the X3, as every signing with the SLH-DSA egg here: seconds on a worker.
                    ferros_sys::pin(ferros_sys::CORES_BIG);
                    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
                    let r = fgtw::flow::build(&frame, &self.keys, eagle());
                    ferros_sys::pin(0);
                    match &r {
                        Ok(bytes) => self.add_event(Flow::Built { id, result: Ok(bytes) }),
                        Err(e) => self.add_event(Flow::Built { id, result: Err(e) }),
                    };
                }
                Work::List => {
                    // Offline the read would only wait out its timeout: nothing listed, said so.
                    if !self.net_up {
                        self.log.put(b"fgtw: no network: fgtw.org's devices asking are not read");
                        self.log.line();
                        self.add_event(Flow::Listed { reqs: &[], owned_elsewhere: &[] });
                        continue;
                    }
                    ferros_sys::pin(ferros_sys::CORES_BIG);
                    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
                    fgtw::set_clock(eagle());
                    let (r, owned) = {
                        let s = self.seed();
                        let r = fgtw::client::bindreq_list(&s, &self.keys, &hp, &seed);
                        // Each request's device asked of fgtw.org's device-owner index: one owned by another identity's fleet is refused on the screen, not only at the bind. A question that fails counts as not owned; the bind refuses it then.
                        let owned: Vec<[u8; 32]> = match &r {
                            Ok(reqs) => reqs.iter().filter(|q| fgtw::client::device_owned_elsewhere(&s, &q.device_pubkey, &hp) == Ok(true)).map(|q| q.device_pubkey).collect(),
                            Err(_) => Vec::new(),
                        };
                        self.seed_ip = s.ip.get();
                        (r, owned)
                    };
                    ferros_sys::pin(0);
                    match r {
                        Ok(reqs) => {
                            self.seed_heard_us = now_us();
                            self.log.put(b"fgtw: fgtw.org lists ");
                            self.log.dec(reqs.len() as u64);
                            self.log.put(b" device(s) asking to join, ");
                            self.log.dec(owned.len() as u64);
                            self.log.put(b" of them another identity's");
                            self.log.line();
                            self.add_event(Flow::Listed { reqs: &reqs, owned_elsewhere: &owned });
                        }
                        Err(e) => {
                            self.log.put(b"fgtw: fgtw.org's list of devices asking failed: ");
                            self.log.put(e.as_bytes());
                            self.log.line();
                            self.add_event(Flow::Listed { reqs: &[], owned_elsewhere: &[] });
                        }
                    }
                }
                Work::Bind { req } => {
                    let device = req.device_pubkey;
                    let t0 = now();
                    ferros_sys::pin(ferros_sys::CORES_BIG);
                    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
                    fgtw::set_clock(eagle());
                    let r = {
                        let s = self.seed();
                        let r = fgtw::sponsor::bind_device_held(&s, &self.held, &self.keys, &hp, &req);
                        self.seed_ip = s.ip.get();
                        r
                    };
                    ferros_sys::pin(0);
                    match r.and_then(|blob| blob.to_vsf_bytes().map(|bytes| (blob, bytes)).map_err(|_| String::from("the extended chain did not encode"))) {
                        Ok((blob, bytes)) => {
                            self.log.put(b"fgtw: the Add is in the chain (");
                            self.log.dec(blob.ops.len() as u64);
                            self.log.put(b" ops) in ");
                            self.log.dec(ferros_sys::us_since(t0) / 1000);
                            self.log.put(b" ms");
                            self.log.line();
                            // Held (note_members), kept when unlock-at-boot is on, and the fold told to the machine before its answer.
                            self.persist_set(b"chain", &bytes);
                            self.note_members(&blob);
                            self.add_event(Flow::Bound { device, result: Ok(()) });
                        }
                        Err(e) => {
                            self.add_event(Flow::Bound { device, result: Err(&e) });
                        }
                    }
                }
                Work::Grow { device, members } => {
                    let Some(key) = self.fleet_key else {
                        self.add_event(Flow::Grown { device, result: Err("this device does not hold the fleet key") });
                        continue;
                    };
                    ferros_sys::pin(ferros_sys::CORES_BIG);
                    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
                    fgtw::set_clock(eagle());
                    let r = {
                        let s = self.seed();
                        let r = fgtw::held::fanout_grow_held(&s, &self.held_fan, &hp, &self.keys, &key, &members, &seed);
                        self.seed_ip = s.ip.get();
                        r
                    };
                    ferros_sys::pin(0);
                    match r {
                        Ok((revision, blob)) => {
                            self.log.put(b"fgtw: the fan-out grown to revision ");
                            self.log.dec(revision);
                            self.log.put(b" for ");
                            self.log.dec(members.len() as u64);
                            self.log.put(b" devices: handing it to the newcomer and the members");
                            self.log.line();
                            self.add_event(Flow::Grown { device, result: Ok(&blob) });
                        }
                        Err(e) => {
                            self.add_event(Flow::Grown { device, result: Err(&e) });
                        }
                    }
                }
                // The chain first, so a select's sender is a member of a chain the device already has (it holds a select that comes first until a chain arrives).
                Work::PushChain { to } => {
                    let Some(addr) = self.add_addr(&to) else { continue };
                    let Some(frame) = self.add_chain_frame(&hp) else { continue };
                    let out = self.pt.send(addr, frame, now_us(), eagle());
                    self.send_all(out);
                }
                Work::ChainNews { extra } => {
                    let Some(frame) = self.add_chain_frame(&hp) else { continue };
                    let to = extra.and_then(|k| self.add_addr(&k));
                    self.push_to_fleet(frame, to);
                }
            }
        }
    }
}
