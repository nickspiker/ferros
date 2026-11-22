//! Remove this device from its identity (owner, 2026-11-22; docs/AUTH.md "Remove this device"), on the device being removed: Remove on the Fleet page, the person's handle typed on the attest screen's keyboard (ahau, words kind 7), enter.
//! No Fleet page needs to be open anywhere and nothing waits on another device before the erase.
//!
//! 0. **The check.** The typed handle's seed against the roots in hand here (op 33), and again in the kernel at the erase (SYS_UNLOCK op 15, compared in registers).
//! 1. **The fleet told at once**, with what fgtw has today: the departure machine's signed request (depart.rs), intent 1 (to a new owner), handed by Photon Transport to every fleet device heard here and put in the identity space, whose push goes to the fleet and to fgtw.org in the same pass (space.rs `space_put_depart`).
//!    On today's fgtw the chain still drops this device only when a surviving member approves (its Approve publishes the consented Remove and frees this device's brand on fgtw.org, which another identity's add needs: fgtw-bootstrap refuses a bare self-departure and a device branded to another identity).
//!    fgtw's shared departure machine (the Photon session's) takes this step over when it lands: `remove_tell` is the one place that starts it, and `remove_step` the one place that decides it has gone out.
//! 2. **The erase**, once the request is out and the fleet's devices heard here have had `TELL_US` to take it: every kist's own domain of the person's (from their list, `kists.list`: the kists stowed and those kept after an unstow), the person's domain in every scope (map., gps., wifi., fgtw., ahau., kists. and the hold), the kernel's identity class, tags, kept seed and bound mark, the device-only secret, the person key and the roots. Never the device class, another identity's data, or devinfo.
//! 3. **The power off**: the kernel sends its drops to the vault process, then PSCI SYSTEM_OFF (the killswitch's), so the next power-on is a fresh attest screen.
//!
//! A dry run (op 33 [1]) runs every check and says on the console what each step would take, changing nothing and needing no handle; `ferros-bridge spawn` of payloads/removedry asks for it.

use super::*;

/// How long the fleet's devices heard here have to take the request once it is out, before the erase starts: 2^22 us (about 4.2 s), past Photon Transport's retries on a LAN.
const TELL_US: u64 = 1 << 22;

/// A removal under way.
pub struct Removing {
    /// The typed handle's seed, held for the kernel's own check at the erase, then wiped.
    seed: [u8; 32],
    /// When the request was out (built, handed to the fleet's devices heard here, pushed into the space); 0 not yet.
    told_us: u64,
}

impl Client {
    /// Op 33 [0][seed 32]: the person typed their handle to remove this device. → 0 started (or already under way), 1 not a member of a fleet, 4 the identity's last device (an identity must live somewhere: nothing to tell, nothing done), 6 the handle is not this identity's (nothing done), 7 the kernel cannot remove (nothing told), else the departure's own refusal (depart.rs `leave`).
    /// `typed` is wiped here whatever the answer.
    pub fn remove_start(&mut self, typed: &mut [u8; 32]) -> u8 {
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
        let r = self.remove_tell();
        if r != 0 {
            wipe(typed);
            self.log.put(b"fgtw: remove: not started (");
            self.log.dec(r as u64);
            self.log.put(b"); nothing changed");
            self.log.line();
            return r;
        }
        self.removing = Some(Removing { seed: *typed, told_us: 0 });
        wipe(typed);
        self.log.put(b"fgtw: remove: the handle matches; telling the fleet, then this identity's data erased here and the power off");
        self.log.line();
        0
    }

    /// Step 1's start: the departure request, intent 1 (to a new owner, so the approval frees this device's brand), on the departure machine; one of ours already standing is used as it is. 0 started, else `leave`'s refusal.
    fn remove_tell(&mut self) -> u8 {
        if self.depart_asking() {
            return 0;
        }
        self.leave(join::DEPART_NEW_OWNER)
    }

    /// Once a pass of the loop: the request out (the machine holds our signed frame, which `depart_work` handed to the fleet's devices heard here and pushed into the space as it built it), then `TELL_US` for them to take it, then the erase.
    /// A request that did not build (the machine dropped it) ends the removal with nothing erased.
    pub fn remove_step(&mut self, t: u64) {
        let Some(told) = self.removing.as_ref().map(|r| r.told_us) else { return };
        if told == 0 {
            if self.dep.m.as_ref().and_then(|m| m.own_frame()).is_some() {
                if let Some(r) = self.removing.as_mut() {
                    r.told_us = t;
                }
                let heard = self.sponsors.iter().filter(|s| s.online).count();
                self.log.put(b"fgtw: remove: the fleet is told (the request to A#");
                self.log.dec(heard as u64);
                self.log.put(b" devices heard here and into the identity space); the erase in about 4 s");
                self.log.line();
            } else if !self.depart_asking() {
                if let Some(mut r) = self.removing.take() {
                    wipe(&mut r.seed);
                }
                self.log.put(b"fgtw: remove: the departure request did not build; nothing erased");
                self.log.line();
            }
            return;
        }
        if t - told >= TELL_US {
            self.remove_erase();
        }
    }

    /// The person's kists, each (developer, option, name): the stowed ones and those kept with answers after an unstow (their domains may hold data still), from the person's list (ferros_kist `domain_owners`). Empty when the list does not read (the kernel's release still renews the device-only secret, so a domain missed here can never be opened again).
    fn remove_kists(&mut self) -> Vec<([u8; 32], [u8; 32], alloc::string::String)> {
        let mut c = self.copy();
        let Ok(list) = ferros_space::read_list(&mut c) else {
            self.log.put(b"fgtw: remove: the person's list of kists does not read; their own domains are left unopenable rather than dropped");
            self.log.line();
            return Vec::new();
        };
        list.domain_owners()
    }

    /// The kernel's message for SYS_UNLOCK op 15: [flags][seed][each kist (developer, option)].
    fn remove_message(flags: u8, seed: &[u8; 32], kists: &[([u8; 32], [u8; 32], alloc::string::String)], room: usize) -> Vec<u8> {
        let mut m = Vec::with_capacity(room.max(33 + (kists.len() << 6)));
        m.push(flags);
        m.extend_from_slice(seed);
        for (d, o, _) in kists {
            m.extend_from_slice(d);
            m.extend_from_slice(o);
        }
        m
    }

    /// Steps 2 and 3: the kernel checks the handle again, drops every domain of the person's and releases the identity, then powers off.
    fn remove_erase(&mut self) {
        let Some(mut r) = self.removing.take() else { return };
        let kists = self.remove_kists();
        let mut m = Self::remove_message(0, &r.seed, &kists, 0);
        wipe(&mut r.seed);
        let len = m.len();
        let got = ferros_sys::remove_device(cap(unsafe { GRANTS }, S_UNLOCK), &mut m, len);
        wipe(&mut m);
        match got {
            Some(1) => {
                self.log.put(b"fgtw: remove: the kernel erases this identity's data here (A#");
                self.log.dec(kists.len() as u64);
                self.log.put(b" kists' own domains and every scope of the person's) and powers off");
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
            self.log.put(b"fgtw: remove (dry run): the request, to a new owner, signed with every egg (seconds on the X3), then A#4 s for the fleet to take it");
        }
        self.log.line();
        self.log.put(b"fgtw: remove (dry run): on today's fgtw a surviving member's Approve then publishes the Remove and frees this device's brand on fgtw.org (another identity's add needs it)");
        self.log.line();
        let kists = self.remove_kists();
        self.log.put(b"fgtw: remove (dry run): the kists' own domains of the person's to drop: A#");
        self.log.dec(kists.len() as u64);
        self.log.line();
        for (d, o, name) in &kists {
            self.log.put(b"fgtw: remove (dry run): kist ");
            if name.is_empty() {
                self.log.put(b"(unstowed, its answers kept)");
            } else {
                self.log.put(name.as_bytes());
            }
            self.log.put(b", developer ");
            self.log.fp(d);
            self.log.put(b", option ");
            self.log.fp(o);
            self.log.line();
        }
        self.log.put(b"fgtw: remove (dry run): this service forgets the roots, the chain, the fleet key, the person key and its departure and add state (its fgtw. scope goes with the person's domains)");
        self.log.line();
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
