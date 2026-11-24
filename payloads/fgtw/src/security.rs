//! The Security page's acts this service carries out (fgtw::security, shared with Photon; owner, 2026-11-24: "build the lock, revoke, shred, all that stuff needs to work on both"), each approved on the attest screen first (docs/AUTH.md "Approve on the attest screen").
//!
//! - **Shred** (op 33 [2]): the attest screen hands the seed of the handle typed at the approve; the kernel erases this identity's data here exactly as a removal does (each kist's own domain, every scope of the person's, the kernel's identity class, tags, kept seed, bound mark, the device-only secret, the person key), with no departure and under its own approval (ahau_core::approve::SHRED), then restarts at the attest screen. The device stays in its fleet: the chain still lists it and the fan-out still wraps to its key, so the next attest with the same handle binds it again and brings the space, the fleet key and the kists back.
//! - **Revoke** and **reinstate** (ahau_core::approve::REVOKE, REINSTATE): the kernel holds the approval with the device's number (the first seven bytes of its key); this service takes it once it holds the roots and the fleet key again (the approve's lock wiped them), on that edge alone, never on a timer (`sec_check`).
//!   A revoke signs the chain's Lock op and the worker's refusal (`fgtw::security::revoke`), mints a fresh fleet key wrapped to the live members alone (`fgtw::security::rotate`), adopts it, and seals the person's space and person key again under it at once, to the fleet and to fgtw.org (`space_reseal`): no device keeps the key before (there is no chain of keys), so nothing may stay sealed only under it.
//!   A reinstatement signs Unlock and lifts the refusal (`fgtw::security::reinstate`), then grows the fan-out to the live members with the key held (never a mint).
//! - **The page's line** (op 41): the last act and where it stands, for the map's Security page (`fgtw::security::Status`).

use super::*;
use ahau_core::approve as ap;

/// A Security act this service carried out or is carrying out, for op 41.
#[derive(Clone, Copy)]
pub struct Sec {
    /// ahau_core::approve's kind: SHRED, REVOKE, REINSTATE.
    pub kind: u8,
    /// 2 running, 3 done, 4 failed (fgtw::security::Phase's order after Asked, which the map knows itself).
    pub phase: u8,
    /// The device a revoke or a reinstatement names.
    pub device: [u8; 32],
}

impl Client {
    /// Op 33 [2][seed 32]: shred this device's copy of the identity, approved on the attest screen (the seed of the handle typed there). → 0 the kernel erases and restarts, 6 the handle is not this identity's, 7 the kernel refused (no approve held, a kernel before 2026-11-24), 8 a removal is under way. `typed` is wiped here whatever the answer.
    pub fn shred_start(&mut self, typed: &mut [u8; 32]) -> u8 {
        let same = self.identity_seed.as_ref().is_some_and(|s| s.iter().zip(typed.iter()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0);
        if !same {
            wipe(typed);
            self.log.put(b"fgtw: shred: the handle typed is not this identity's; nothing changed");
            self.log.line();
            return 6;
        }
        if self.removing.is_some() {
            wipe(typed);
            return 8;
        }
        self.sec = Some(Sec { kind: ap::SHRED, phase: 2, device: self.me });
        let kists = self.remove_kists();
        // Flag bit 1: the shred (no departure, the SHRED approval, a restart).
        let mut m = Self::remove_message(2, typed, &kists, 0);
        wipe(typed);
        let len = m.len();
        let got = ferros_sys::remove_device(cap(unsafe { GRANTS }, S_UNLOCK), &mut m, len);
        wipe(&mut m);
        match got {
            Some(1) => {
                self.log.put(b"fgtw: shred: the kernel erases this identity's data here (A#");
                self.log.dec(kists.len() as u64);
                self.log.put(b" kists' own domains and every scope of the person's) and restarts at the attest screen; the device stays in its fleet");
                self.log.line();
                self.forget_identity();
                0
            }
            Some(0) => {
                self.sec_end(false);
                self.log.put(b"fgtw: shred: the kernel says the handle typed is not the person present's; nothing erased");
                self.log.line();
                6
            }
            _ => {
                self.sec_end(false);
                self.log.put(b"fgtw: shred: the kernel refused (no approve held for it, nobody present, or a kernel before 2026-11-24); nothing erased");
                self.log.line();
                7
            }
        }
    }

    fn sec_end(&mut self, ok: bool) {
        if let Some(s) = self.sec.as_mut() {
            s.phase = if ok { 3 } else { 4 };
        }
    }

    /// The loop's turn: once after each handing of the roots (op 4 sets `sec_check`), as soon as this device is a bound member holding the fleet key, an approved revoke or reinstatement the kernel holds is taken and carried out. Nothing is asked of the kernel at any other time.
    pub fn security_step(&mut self) {
        if !self.sec_check || !self.bound || self.fleet_key.is_none() || self.identity_seed.is_none() || self.hp.is_none() {
            return;
        }
        self.sec_check = false;
        for kind in [ap::REVOKE, ap::REINSTATE] {
            if let ferros_sys::approve::Taken::Approved(word) = ferros_sys::approve::take(cap(unsafe { GRANTS }, S_UNLOCK), kind) {
                let arg = word >> 8;
                self.sec_run(kind, arg);
            }
        }
    }

    /// A revoke or a reinstatement of the one member the number names, checked against the chain held here first (fgtw::security::check), then carried out over fgtw.org.
    fn sec_run(&mut self, kind: u8, arg: u64) {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        let members = self.fleet_members();
        let named: Vec<[u8; 32]> = members.iter().copied().filter(|m| ap::device_is(arg, m)).collect();
        let revoke = kind == ap::REVOKE;
        let word: &[u8] = if revoke { b"revoke" } else { b"reinstate" };
        // WHY exactly one: the number is seven bytes of a key; two members sharing them (a 2^-56 chance a pair) is not a device to act on.
        let [device] = named.as_slice() else {
            self.sec = Some(Sec { kind, phase: 4, device: [0; 32] });
            self.log.put(b"fgtw: security: ");
            self.log.put(word);
            self.log.put(b": the approved number names no single member of the fleet; nothing done");
            self.log.line();
            return;
        };
        let device = *device;
        self.sec = Some(Sec { kind, phase: 2, device });
        self.watch_ring();
        let act = if revoke { fgtw::security::Act::Revoke(device) } else { fgtw::security::Act::Reinstate(device) };
        if let Ok(blob) = fgtw::fleet::MembershipBlob::from_vsf_bytes(&self.chain_held) {
            if let Err(why) = fgtw::security::check(&blob, &self.me, act) {
                self.sec_end(false);
                self.log.put(b"fgtw: security: ");
                self.log.put(word);
                self.log.put(b" refused: ");
                self.log.put(why.as_bytes());
                self.log.line();
                return;
            }
        }
        self.log.put(b"fgtw: security: ");
        self.log.put(word);
        self.log.put(b" ");
        self.say_device(&device, Some(seed));
        self.log.put(b", approved on the attest screen: signing the chain op (every egg, seconds on the X3)");
        self.log.line();
        // Signed with every egg on the big cores, as every chain op from here is.
        ferros_sys::pin(ferros_sys::CORES_BIG);
        ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
        fgtw::set_clock(eagle());
        let done = {
            let s = self.seed();
            if revoke {
                fgtw::security::revoke(&s, &self.keys, &hp, &device).and_then(|live| {
                    fgtw::set_clock(eagle());
                    fgtw::security::rotate(&s, &self.keys, &hp, &live, &seed).map(|k| (live, Some(k)))
                })
            } else {
                fgtw::security::reinstate(&s, &self.keys, &hp, &device).and_then(|live| {
                    let key = self.fleet_key.ok_or_else(|| alloc::string::String::from("no fleet key held"))?;
                    fgtw::set_clock(eagle());
                    fgtw::client::grow_fleet_wraps(&s, &hp, &self.keys, &key, &live, &seed).map(|_| (live, None))
                })
            }
        };
        ferros_sys::pin(0);
        match done {
            Ok((live, rotated)) => {
                self.log.put(b"fgtw: security: ");
                self.log.put(word);
                self.log.put(b" done; the fan-out wraps to A#");
                self.log.dec(live.len() as u64);
                self.log.put(b" live members");
                self.log.line();
                if let Some((rev, k)) = rotated {
                    // The new key held from now: everything sealed is sealed again under it, at once.
                    if let Some(mut old) = self.fleet_key.replace(k) {
                        wipe(&mut old);
                    }
                    self.fleet_rev = rev;
                    self.persist_secret(ferros_kept::secret::Kind::FleetKey, b"fleet_key", &k);
                    self.log.put(b"fgtw: security: the fleet key rotated to revision A#");
                    self.log.dec(rev);
                    self.log.put(b"; the person's space and person key sealed again under it");
                    self.log.line();
                    if !self.space_reseal() {
                        self.log.put(b"fgtw: security: the space was not sealed again yet (not in hand, or fgtw.org did not take it); its next push seals it under the new key");
                        self.log.line();
                    }
                }
                // The chain as it stands now, to the fleet: siblings adopt it by extension.
                self.refresh_chain();
                self.sec_end(true);
            }
            Err(e) => {
                self.sec_end(false);
                self.log.put(b"fgtw: security: ");
                self.log.put(word);
                self.log.put(b" failed: ");
                self.log.put(e.as_bytes());
                self.log.line();
            }
        }
        self.watch_ring();
    }

    /// Op 41: the Security page's line. → [phase: 0 none, 2 running, 3 done, 4 failed][kind][the device's first 8 bytes].
    pub fn security_reply(&self) -> Vec<u8> {
        match self.sec {
            Some(s) => {
                let mut r = alloc::vec![s.phase, s.kind];
                r.extend_from_slice(&s.device[..8]);
                r
            }
            None => alloc::vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        }
    }
}
