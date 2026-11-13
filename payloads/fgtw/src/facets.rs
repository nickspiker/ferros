//! The facets (docs/FACETS.md "To build" 2 and 4, 2026-11-13): what a kist may know of the person, served on the `facets` endpoint to the kists the person granted be you or people, and the person's people, added on the system's add-a-person sheet and listed on the People page.
//!
//! **The endpoint** (grant slot 11, the receive right the kernel hands the server of `fgtw` as the slot after its last). The kernel stamps every call with the caller's kist identity, which facets the person granted it, whether the person is present, and its alias's Ed25519 key (ferros_sys::facets::Stamp); nothing the caller says names a kist or a grant.
//! - Op 0, be you: the kist hands its alias's public bundle and its possession eggs; the service checks the bundle is the alias the kernel stamped and that the eggs hold it, then signs the binding (fgtw::binding) with this device's member key, every egg the chain knows it holds. The call is held until the loop signs it (SPHINCS+ is slow), and the answer kept for the run, so a relaunch is answered at once. The answer carries the handle proof and the pinned identity key (Ed25519 of the identity seed, public: what friends pinned, PHOTON.md M2) beside the binding.
//! - Op 1, the people: all of them, or the ones the person chose for that kist (ferros_space::people_for), as fgtw::people::doc, without their fleets.
//! - Each answer only while the person is present (the kernel's word), and only while this service holds a fleet it is a member of. It never hands a root, the person key or an alias: it holds no alias, and a kist's alias is the kernel's to give.
//!
//! **Adding a person** (FACETS.md "Adding a person is the person's act, never a kist's"; photon identity-profile.md "type it once, derive, pin, drop"). The People page asks for the keyboard (op 15 [2]); the attest screen's keyboard (ahau) takes the handle as typed, shown as typed (a handle is not a password), computes the identity seed and the memory-hard proof there (its 24 MiB scratch is the attest screen's; this service's heap is 2 MiB), hands both in (op 30) and wipes them with the text. Here the seed gives the identity key it must pin and is wiped at once; the chain is asked of fgtw.org by the proof in the loop; its genesis must name that key (the derivation is the verification); then the person is written into the space (`PERSON`) with the name typed next (op 31, kind 3 on the keyboard). The handle reaches no disk and no wire, and this service never sees it.

use super::*;
use fgtw::people::Person;
use ferros_sys::facets as fx;

/// Grant slot 11: serve facets (the kernel's, after slot 10).
pub const S_FACETS: u64 = 11;

/// Where an add stands, for the People page (op 25's first byte).
pub mod add {
    pub const NONE: u8 = 0;
    pub const LOOKING: u8 = 1;
    pub const ADDED: u8 = 2;
    pub const NOBODY: u8 = 3;
    pub const UNREACHABLE: u8 = 4;
    pub const NOT_THEIRS: u8 = 5;
    pub const YOU: u8 = 6;
}

/// A person being added: what the handle gave (its proof, the identity key to pin), the chain once found, and the name once typed.
pub struct Adding {
    hp: [u8; 32],
    identity: [u8; 32],
    found: Option<Person>,
    looked: bool,
    name: Option<alloc::string::String>,
}

/// The facets' state on the service.
pub struct Facets {
    /// Op 0 calls held for the loop to sign: the reply token and the caller's request (the stamp first).
    held: Vec<([u8; 32], Vec<u8>)>,
    /// Bindings made this run, by (developer, option, the alias bundle's commitment), each its whole answer.
    made: Vec<([u8; 32], [u8; 32], [u8; 32], Vec<u8>)>,
    /// The add under way, and how the last one ended.
    adding: Option<Adding>,
    pub state: u8,
}

impl Facets {
    pub fn new() -> Facets {
        Facets { held: Vec::new(), made: Vec::new(), adding: None, state: add::NONE }
    }
}

impl Client {
    fn reply_to(&self, tok: &[u8; 32], r: &[u8]) {
        svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
    }

    /// The fleet this service may speak for: its handle proof and chain, while it is a bound member. None otherwise.
    fn member_chain(&self) -> Option<([u8; 32], fgtw::fleet::MembershipBlob)> {
        let hp = self.hp?;
        if !self.bound {
            return None;
        }
        let chain = self.held.borrow().get(&hp).cloned()?;
        chain.is_member(&self.me).then_some((hp, chain))
    }

    /// Calls on `facets` taken in (one pass of the loop): the people answered at once, a binding held for `facets_work`. A slot with no grant behind it (a kernel from before) answers ERR, and nothing is served.
    pub fn facets_recv(&mut self) -> bool {
        let mut busy = false;
        let mut buf = alloc::vec![0u8; fx::STAMP + 4096];
        loop {
            let mut tok = [0u8; 32];
            let n = ferros_sys::recv(cap(self.log.grants, S_FACETS), &mut buf, &mut tok, true);
            if n == ERR || n == ferros_sys::NOTHING {
                return busy;
            }
            busy = true;
            // WHY the min: `n` is what the kernel delivered, at most the stamp and a call's 4096 bytes, which is what the buffer holds.
            let msg = buf[..(n as usize).min(buf.len())].to_vec();
            let Some((st, body)) = fx::Stamp::read(&msg) else {
                self.reply_to(&tok, &[fx::MALFORMED]);
                continue;
            };
            match body.first().copied() {
                Some(fx::BE_YOU) => match self.binding_check(&st, body) {
                    Ok(Some(r)) => self.reply_to(&tok, &r),
                    Ok(None) => self.facets.held.push((tok, msg)),
                    Err(code) => {
                        self.facets_said(&st, "be you", code);
                        self.reply_to(&tok, &[code]);
                    }
                },
                Some(fx::PEOPLE) => {
                    let r = self.people_answer(&st);
                    self.facets_said(&st, "people", r[0]);
                    self.reply_to(&tok, &r);
                }
                _ => self.reply_to(&tok, &[fx::MALFORMED]),
            }
        }
    }

    /// One line a facet answered or refused, naming the kist by its developer key's fingerprint and never a byte of what was handed.
    fn facets_said(&mut self, st: &fx::Stamp, what: &str, code: u8) {
        self.log.put(b"fgtw: facets: ");
        self.log.put(what.as_bytes());
        self.log.put(b" to the kist ");
        self.log.fp(&st.developer);
        self.log.put(match code {
            fx::OK => b": given".as_slice(),
            fx::NOT_GRANTED => b": refused, not granted",
            fx::NOT_PRESENT => b": refused, nobody present or no fleet in hand",
            fx::NOT_ALIAS => b": refused, not its alias",
            _ => b": refused, malformed",
        });
        self.log.line();
    }

    /// Op 0 checked: Err a refusal, Ok(Some) the answer made before, Ok(None) to be signed in the loop.
    fn binding_check(&self, st: &fx::Stamp, body: &[u8]) -> Result<Option<Vec<u8>>, u8> {
        if !st.kist || !st.be_you {
            return Err(fx::NOT_GRANTED);
        }
        if !st.present || self.member_chain().is_none() {
            return Err(fx::NOT_PRESENT);
        }
        let alias = st.alias.ok_or(fx::NOT_ALIAS)?;
        let (bundle, eggs) = parse_be_you(body).ok_or(fx::MALFORMED)?;
        if bundle.ed25519() != alias || !fgtw::binding::holds(&bundle, eggs) {
            return Err(fx::NOT_ALIAS);
        }
        let commit = bundle.commit();
        Ok(self.facets.made.iter().find(|m| m.0 == st.developer && m.1 == st.option && m.2 == commit).map(|m| m.3.clone()))
    }

    /// The loop's turn: one held binding signed and answered (every egg; SPHINCS+ takes its time, so never inside the call's own receive).
    pub fn facets_work(&mut self) {
        if self.facets.held.is_empty() {
            return;
        }
        let (tok, msg) = self.facets.held.remove(0);
        let Some((st, body)) = fx::Stamp::read(&msg) else { return };
        let r = match self.binding_check(&st, body) {
            Ok(Some(r)) => r,
            Ok(None) => self.binding_make(&st, body),
            Err(code) => alloc::vec![code],
        };
        self.facets_said(&st, "be you", r[0]);
        self.reply_to(&tok, &r);
    }

    /// The binding signed with this device's member key, and the answer kept for the run.
    fn binding_make(&mut self, st: &fx::Stamp, body: &[u8]) -> Vec<u8> {
        let (Some((hp, chain)), Some((bundle, _))) = (self.member_chain(), parse_be_you(body)) else { return alloc::vec![fx::NOT_PRESENT] };
        let Some(identity) = chain.genesis_identity_pubkey() else { return alloc::vec![fx::NOT_PRESENT] };
        let commit = bundle.commit();
        let mask = chain.declared_mask(&self.me);
        let b = fgtw::binding::Binding::sign(&self.keys, mask, &hp, &st.developer, &st.option, bundle, eagle());
        let Ok(doc) = b.to_vsf_bytes() else { return alloc::vec![fx::NOT_PRESENT] };
        let mut r = Vec::with_capacity(65 + doc.len());
        r.push(fx::OK);
        r.extend_from_slice(&hp);
        r.extend_from_slice(&identity);
        r.extend_from_slice(&doc);
        self.facets.made.push((st.developer, st.option, commit, r.clone()));
        r
    }

    /// Op 1: the people this kist may know, as a document.
    fn people_answer(&self, st: &fx::Stamp) -> Vec<u8> {
        if !st.kist || !st.people {
            return alloc::vec![fx::NOT_GRANTED];
        }
        if !st.present || self.space_ready().is_none() || !self.space.settled {
            return alloc::vec![fx::NOT_PRESENT];
        }
        let people = ferros_space::people_for(&self.space.space, &st.developer, &st.option);
        match fgtw::people::doc(&people, eagle()) {
            Ok(doc) => {
                let mut r = alloc::vec![fx::OK];
                r.extend_from_slice(&doc);
                r
            }
            Err(_) => alloc::vec![fx::NOT_PRESENT],
        }
    }

    // ── The person's people: the add-a-person sheet and the People page (the map, over `fgtw` ops 25 to 31) ──

    /// Op 30 [seed 32][proof 32]: the handle typed on the keyboard, as its seed and proof. The seed gives the identity key to pin and is wiped here; the name is asked next (the keyboard's kind 3) while the loop looks the chain up. → [0 taken, 1 nobody present or no fleet in hand].
    pub fn person_roots(&mut self, seed: &mut [u8; 32], hp: &[u8; 32]) -> u8 {
        let identity = fgtw::keys::Keypair::from_seed(seed).public.to_bytes();
        seed.fill(0);
        if self.space_ready().is_none() || !self.space.settled {
            self.entry = 0;
            return 1;
        }
        if self.hp == Some(*hp) {
            self.facets.adding = None;
            self.facets.state = add::YOU;
            self.entry = 0;
            return 0;
        }
        self.facets.adding = Some(Adding { hp: *hp, identity, found: None, looked: false, name: None });
        self.facets.state = add::LOOKING;
        self.entry = 3;
        self.log.put(b"fgtw: people: a person's handle typed (its proof ");
        self.log.fp(hp);
        self.log.put(b"); looking them up");
        self.log.line();
        0
    }

    /// Op 31 [name, UTF-8]: the name the person gives them (empty: none). The keyboard is done; the person is added once their chain is found. → [0].
    pub fn person_name(&mut self, name: &[u8]) -> u8 {
        self.entry = 0;
        if let Some(a) = self.facets.adding.as_mut() {
            a.name = Some(alloc::string::String::from(core::str::from_utf8(name).unwrap_or("")));
        }
        self.person_finish();
        0
    }

    /// The keyboard closed without a name (op 15 [0] while kind 3): the person is added with none.
    pub fn person_no_name(&mut self) {
        if let Some(a) = self.facets.adding.as_mut() {
            if a.name.is_none() {
                a.name = Some(alloc::string::String::new());
            }
        }
        self.person_finish();
    }

    /// The loop's turn: the chain looked up once per add, fgtw.org by the proof (the existing lookup; nothing new on the server).
    pub fn person_step(&mut self) {
        let Some(a) = self.facets.adding.as_ref() else { return };
        if a.looked || !self.net_up {
            return;
        }
        let (hp, identity) = (a.hp, a.identity);
        fgtw::set_clock(eagle());
        let t = self.seed();
        let fetched = fgtw::client::fetch(&t, &hp);
        let outcome = match &fetched {
            Ok(Some(chain)) => {
                let theirs = chain.genesis_handle_proof() == Some(hp) && chain.genesis_identity_pubkey() == Some(identity);
                match (theirs, chain.fold(), chain.genesis_hash()) {
                    (true, Ok(members), Some(genesis)) => Ok(Person { handle_proof: hp, identity, genesis, members, name: alloc::string::String::new() }),
                    _ => Err(add::NOT_THEIRS),
                }
            }
            Ok(None) => Err(add::NOBODY),
            Err(_) => Err(add::UNREACHABLE),
        };
        let Some(a) = self.facets.adding.as_mut() else { return };
        a.looked = true;
        match outcome {
            Ok(p) => {
                a.found = Some(p);
                self.person_finish();
            }
            Err(s) => {
                self.facets.adding = None;
                self.facets.state = s;
                if self.entry == 3 {
                    self.entry = 0;
                }
                self.log.put(match s {
                    add::NOBODY => b"fgtw: people: nobody holds that handle".as_slice(),
                    add::NOT_THEIRS => b"fgtw: people: the chain at that handle proof is not the one the handle derives; not added",
                    _ => b"fgtw: people: fgtw.org did not answer; not added",
                });
                self.log.line();
            }
        }
    }

    /// Found and named: into the space, out to the fleet and fgtw.org.
    fn person_finish(&mut self) {
        let Some(a) = self.facets.adding.as_ref() else { return };
        let (Some(found), Some(name)) = (a.found.as_ref(), a.name.as_ref()) else { return };
        let p = Person { name: name.clone(), ..found.clone() };
        self.facets.adding = None;
        let me = self.me;
        if ferros_space::put_person(&mut self.space.space, &me, &p, eagle()) {
            self.space_keep();
            self.space_to_fleet();
            self.space_seed(true);
        }
        self.facets.state = add::ADDED;
        self.log.put(b"fgtw: people: added (their proof ");
        self.log.fp(&p.handle_proof);
        self.log.put(b"), to the fleet");
        self.log.line();
    }

    /// Op 25: the People page: [the last add's state][count u8][count × ([handle proof 32][name length u8][name, UTF-8])], in the space's order.
    pub fn people_reply(&self) -> Vec<u8> {
        let people = ferros_space::people(&self.space.space);
        let mut r = alloc::vec![self.facets.state, people.len().min(255) as u8];
        for p in people.iter().take(255) {
            r.extend_from_slice(&p.handle_proof);
            let name = fit_utf8(&p.name, 63);
            r.push(name.len() as u8);
            r.extend_from_slice(name.as_bytes());
        }
        r
    }

    /// Op 26 [handle proof 32]: a person removed from the person's people. → [0 done, 1 before the first pull].
    pub fn person_remove(&mut self, hp: &[u8; 32]) -> u8 {
        if self.space_ready().is_none() || !self.space.settled {
            return 1;
        }
        let me = self.me;
        if ferros_space::drop_person(&mut self.space.space, &me, hp, eagle()) {
            self.space_keep();
            self.space_to_fleet();
            self.space_seed(true);
            self.log.put(b"fgtw: people: a person removed, to the fleet");
            self.log.line();
        }
        self.facets.state = add::NONE;
        0
    }

    /// Op 27 [developer 32][option 32]: a kist's card's people: [all u8][count u8][count × ([handle proof 32][chosen u8][name length u8][name])], every one of the person's people with whether the kist has them picked.
    pub fn kist_people_reply(&self, developer: &[u8; 32], option: &[u8; 32]) -> Vec<u8> {
        let s = &self.space.space;
        let picked = ferros_space::chosen(s, developer, option);
        let people = ferros_space::people(s);
        let mut r = alloc::vec![ferros_space::people_all(s, developer, option) as u8, people.len().min(255) as u8];
        for p in people.iter().take(255) {
            r.extend_from_slice(&p.handle_proof);
            r.push(picked.contains(&p.handle_proof) as u8);
            let name = fit_utf8(&p.name, 63);
            r.push(name.len() as u8);
            r.extend_from_slice(name.as_bytes());
        }
        r
    }

    /// Op 28 [developer 32][option 32][all u8]: a kist's people set to all (1) or chosen (0). Op 29 [developer][option][handle proof 32][on u8]: a person picked or unpicked for it. → [0 done, 1 before the first pull]. A running kist reads its people again when it asks; no relaunch.
    pub fn kist_people_set(&mut self, developer: &[u8; 32], option: &[u8; 32], all: Option<bool>, pick: Option<([u8; 32], bool)>) -> u8 {
        if self.space_ready().is_none() || !self.space.settled {
            return 1;
        }
        let (me, now) = (self.me, eagle());
        let changed = match (all, pick) {
            (Some(all), _) => ferros_space::set_people_all(&mut self.space.space, &me, developer, option, all, now),
            (None, Some((hp, on))) => ferros_space::set_chosen(&mut self.space.space, &me, developer, option, &hp, on, now),
            (None, None) => false,
        };
        if changed {
            self.space_keep();
            self.space_to_fleet();
            self.space_seed(true);
        }
        0
    }
}

/// Op 0's body read: [0][the bundle's length u16 LE][the bundle][the eggs]. None when it is short or the bundle does not parse.
fn parse_be_you(body: &[u8]) -> Option<(fgtw::pq::KeyBundle, &[u8])> {
    let n = u16::from_le_bytes([*body.get(1)?, *body.get(2)?]) as usize;
    let bundle = body.get(3..3 + n)?;
    Some((fgtw::pq::KeyBundle::from_bytes(bundle).ok()?, body.get(3 + n..)?))
}

/// `s` cut to at most `n` bytes at a character's edge.
fn fit_utf8(s: &str, n: usize) -> &str {
    let mut k = s.len().min(n);
    // WHY: a name is the person's own and any length; the page's field is a byte's length. PROOF: index 0 is always a boundary, so the loop ends.
    while !s.is_char_boundary(k) {
        k -= 1;
    }
    &s[..k]
}
