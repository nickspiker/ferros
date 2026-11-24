//! Unbind this device (owner, 2026-11-22 and 2026-11-24; docs/AUTH.md "Unbind this device"; fgtw::security::UNBIND), on the device being unbound, in the owner's order:
//!
//! 0. **The handle.** The approve on the attest screen (ahau_core::approve::REMOVE_DEVICE): the device locks and the handle typed there unlocks it; the attest screen hands the roots here again and then the typed seed and its memory-hard proof (op 33). The seed is checked against the roots in hand, the proof by fgtw's removal gate (`UserOp::Remove`, then `Event::Unbind`), and the seed again in the kernel at the end (SYS_UNLOCK op 15, compared in registers).
//! 1. **The request to the fleet**: the departure machine's signed request (depart.rs), intent 1 (to a new owner), handed to every fleet device heard and put in the identity space. The identity's last device refuses here.
//! 2. **Consent**: a member's person approves it. Waited for however long it takes; nothing acts in its place on a timer (until 2026-11-24 the erase ran about 4 s after the request went out, consent or not).
//! 3. **The sign-out**: the approver publishes the consented Remove, which carries this device's own signature over its departure.
//! 4. **The rotation**: the approver mints a fresh fleet key without this device (depart.rs `rotate_after`, on the approving device).
//! 5. **Forgetting**: once the fold no longer lists this device (`Action::Wipe`, depart.rs `depart_wipe`), the kernel checks the handle again, renews the device-only secret (its keys wiped from memory, one write, nothing deleted: owner, 2026-11-24, "Sand on a beach my friend.") and powers off.
//!
//! A deny, no device reachable, a power cut or a lock before step 5 changes nothing: the device stays bound with its keys and its secret (owner, 2026-11-24: "if it does not get my consent from my other devices or a correct handle entry, nothing gets wiped. Lock is fine, but no wipe and certainly no unbind."). The typed seed is held in RAM only, so a lock or a power cut drops the unbind here; if the fleet's consent lands after that, the device stays bound and locked out of nothing until its person unbinds again (the kernel then renews with that approve held, op 4).
//!
//! A dry run (op 33 [1]) runs every check and says on the console what each step would take, changing nothing and needing no handle; `ferros-bridge spawn` of payloads/removedry asks for it.

use super::*;

/// A removal under way.
pub struct Removing {
    /// The typed handle's seed, held for the kernel's own check at the erase, then wiped.
    seed: [u8; 32],
    /// Whether the request is out (built, handed to the fleet's devices heard here, pushed into the space).
    told: bool,
}

impl Client {
    /// Op 33 [0][seed 32][proof 32]: the person approved removing this device on the attest screen (the seed of the handle typed there, and its memory-hard proof for fgtw's removal gate, both made by the attest screen). → 0 started (or already under way), 1 not a member of a fleet, 4 the identity's last device (an identity must live somewhere: nothing to tell, nothing done), 6 the handle is not this identity's (nothing done), 7 the kernel cannot remove (nothing told), else the departure's own refusal (depart.rs `leave`).
    /// `typed` is wiped here whatever the answer.
    pub fn remove_start(&mut self, typed: &mut [u8; 32], proof: &[u8; 32]) -> u8 {
        let same = self.identity_seed.as_ref().is_some_and(|s| s.iter().zip(typed.iter()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0);
        if !same {
            wipe(typed);
            self.log.put(b"fgtw: remove: the handle typed is not this identity's; nothing changed");
            self.log.line();
            return 6;
        }
        if self.removing.is_some() {
            wipe(typed);
            return 0;
        }
        // The kernel asked first whether it has the removal at all (its dry run, no kists named): with a kernel from before 2026-11-22 the fleet is never told of a removal that could not happen.
        let mut probe = Self::remove_message(1, &[0; 32], &[], 1 << 12);
        let len = probe.len();
        probe.resize(1 << 12, 0);
        if ferros_sys::remove_device(cap(unsafe { GRANTS }, S_UNLOCK), &mut probe, len).is_none() {
            wipe(typed);
            self.log.put(b"fgtw: remove: this kernel has no removal (one before 2026-11-22); nothing told, nothing changed");
            self.log.line();
            return 7;
        }
        let r = self.remove_tell(proof);
        if r != 0 {
            wipe(typed);
            self.log.put(b"fgtw: remove: not started (");
            self.log.dec(r as u64);
            self.log.put(b"); nothing changed");
            self.log.line();
            return r;
        }
        self.removing = Some(Removing { seed: *typed, told: false });
        wipe(typed);
        self.log.put(b"fgtw: unbind: the handle matches; asking the fleet: once a member consents and the Remove lands, the device-only secret is renewed here and the power off");
        self.log.line();
        0
    }

    /// Step 1's start: on fgtw's removal gate (Remove, then Unbind with the typed handle's memory-hard proof), the departure request, intent 1 (to a new owner, so the approval frees this device's brand); one of ours already standing is used as it is. 0 started, else `remove_unbind`'s refusal.
    fn remove_tell(&mut self, proof: &[u8; 32]) -> u8 {
        self.remove_unbind(*proof)
    }

    /// Once a pass of the loop: says when the request is out, and ends the unbind with nothing done if the request did not build. The erase waits for the fleet's consent (depart.rs `depart_wipe`, on the fold that no longer lists this device), never for a time.
    pub fn remove_step(&mut self, _t: u64) {
        let Some(told) = self.removing.as_ref().map(|r| r.told) else { return };
        if told {
            return;
        }
        if self.dep.m.as_ref().and_then(|m| m.own_frame()).is_some() {
            if let Some(r) = self.removing.as_mut() {
                r.told = true;
            }
            let heard = self.sponsors.iter().filter(|s| s.online).count();
            self.log.put(b"fgtw: unbind: the fleet is asked (the request to A#");
            self.log.dec(heard as u64);
            self.log.put(b" devices heard here and into the identity space); waiting for a member to consent");
            self.log.line();
        } else if !self.depart_asking() {
            if let Some(mut r) = self.removing.take() {
                wipe(&mut r.seed);
            }
            self.log.put(b"fgtw: unbind: the departure request did not build; nothing changed");
            self.log.line();
        }
    }

    /// The kernel's message for SYS_UNLOCK op 15: [flags][seed][each kist (developer, option)].
    pub(crate) fn remove_message(flags: u8, seed: &[u8; 32], kists: &[([u8; 32], [u8; 32], alloc::string::String)], room: usize) -> Vec<u8> {
        let mut m = Vec::with_capacity(room.max(33 + (kists.len() << 6)));
        m.push(flags);
        m.extend_from_slice(seed);
        for (d, o, _) in kists {
            m.extend_from_slice(d);
            m.extend_from_slice(o);
        }
        m
    }

    /// Step 5, once the fleet consented: the kernel checks the handle again, renews the device-only secret (nothing deleted) and powers off.
    pub(crate) fn remove_erase(&mut self) {
        let Some(mut r) = self.removing.take() else { return };
        let mut m = Self::remove_message(0, &r.seed, &[], 0);
        wipe(&mut r.seed);
        let len = m.len();
        let got = ferros_sys::remove_device(cap(unsafe { GRANTS }, S_UNLOCK), &mut m, len);
        wipe(&mut m);
        match got {
            Some(1) => {
                self.log.put(b"fgtw: unbind: the kernel renewed the device-only secret (everything of the person's here unreadable, nothing deleted) and powers off");
                self.log.line();
                self.forget_identity();
            }
            Some(0) => {
                self.log.put(b"fgtw: remove: the kernel says the handle typed is not the person present's; nothing erased");
                self.log.line();
            }
            _ => {
                self.log.put(b"fgtw: remove: the kernel refused (nobody present, or a kernel before 2026-11-22); nothing erased");
                self.log.line();
            }
        }
    }

    /// Op 33 [1]: the dry run. Every check of a removal and what each step would take, said on the console; nothing changed, no handle needed. → 0 said, 1 no roots in hand.
    pub fn remove_dry(&mut self) -> u8 {
        self.log.put(b"fgtw: remove (dry run): nothing is changed, no handle typed");
        self.log.line();
        let Some(seed) = self.identity_seed else {
            self.log.put(b"fgtw: remove (dry run): no identity in hand here (nobody attested): nothing to remove");
            self.log.line();
            return 1;
        };
        self.log.put(b"fgtw: remove (dry run): ");
        let me = self.me;
        self.say_device(&me, Some(seed));
        self.log.put(if self.bound { b", a member of its identity's fleet of A#".as_slice() } else { b", NOT a member of a fleet (Remove refuses: nothing to tell) of A#" });
        self.log.dec(self.members.len() as u64);
        self.log.put(b" devices");
        self.log.line();
        let others: Vec<[u8; 32]> = self.members.iter().copied().filter(|k| *k != self.me).collect();
        if self.bound && others.is_empty() {
            self.log.put(b"fgtw: remove (dry run): the identity's last device: Remove refuses (an identity must live somewhere)");
            self.log.line();
        }
        for k in &others {
            let heard = self.sponsors.iter().any(|s| s.key == *k && s.online);
            self.log.put(b"fgtw: remove (dry run): told: ");
            self.say_device(k, Some(seed));
            self.log.put(if heard { b" (heard here: by Photon Transport at once, and the space)".as_slice() } else { b" (not heard here: thru the identity space, pushed to the fleet and fgtw.org)" });
            self.log.line();
        }
        if self.depart_asking() {
            self.log.put(b"fgtw: remove (dry run): a departure request of ours already stands: it would be the one told");
        } else {
            self.log.put(b"fgtw: remove (dry run): fgtw's removal gate checks the typed handle's memory-hard proof, then the request, to a new owner, signed with every egg (seconds on the X3), then handed to the fleet");
        }
        self.log.line();
        self.log.put(b"fgtw: remove (dry run): on today's fgtw a surviving member's Approve then publishes the Remove and frees this device's brand on fgtw.org (another identity's add needs it)");
        self.log.line();
        self.log.put(b"fgtw: remove (dry run): then the wait for a member's consent, however long (no timer); on its Remove the approver rotates the fleet key, and this device renews its device-only secret and powers off: nothing deleted, everything of the person's here unreadable");
        self.log.line();
        let kists: Vec<([u8; 32], [u8; 32], alloc::string::String)> = Vec::new();
        // The kernel's half, thru the same call the removal makes, with the dry bit: it reads, says, and changes nothing.
        let mut m = Self::remove_message(1, &[0; 32], &kists, 1 << 12);
        let len = m.len();
        m.resize(len.max(1 << 12), 0);
        match ferros_sys::remove_device(cap(unsafe { GRANTS }, S_UNLOCK), &mut m, len) {
            Some(n) => {
                // WHY the min: the length is the kernel's whole account; only what fit was written.
                let n = (n as usize).min(m.len());
                for line in m[..n].split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
                    self.log.put(b"fgtw: remove (dry run): kernel: ");
                    self.log.put(line);
                    self.log.line();
                }
            }
            None => {
                self.log.put(b"fgtw: remove (dry run): the kernel gave no account (nobody present, or a kernel before 2026-11-22)");
                self.log.line();
            }
        }
        0
    }
}
