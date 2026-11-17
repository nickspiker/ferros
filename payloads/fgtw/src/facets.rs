//! The facets (docs/FACETS.md "To build" 2 and 4, 2026-11-13): what a kist may know of the person, served on the `facets` endpoint to the kists the person granted be you or people, and the person's people, added on the system's add-a-person sheet and listed on the People page.
//!
//! **The endpoint** (grant slot 11, the receive right the kernel hands the server of `fgtw` as the slot after its last). The kernel stamps every call with the caller's kist identity, which facets the person granted it, whether the person is present, and its alias's Ed25519 key (ferros_sys::facets::Stamp); nothing the caller says names a kist or a grant.
//! - Op 0, be you: the kist hands its alias's public bundle and its possession eggs; the service checks the bundle is the alias the kernel stamped and that the eggs hold it, then signs the binding (fgtw::binding) with this device's member key, every egg the chain knows it holds. The call is held until the loop signs it (SPHINCS+ is slow), and the answer kept for the run, so a relaunch is answered at once. The answer carries the handle proof and the pinned identity key (Ed25519 of the identity seed, public: what friends pinned, PHOTON.md M2) beside the binding.
//! - Op 1, the people: all of them, or the ones the person chose for that kist (ferros_space::people_for), as fgtw::people::doc, without their fleets.
//! - Op 2, wait for be you (2026-11-14): the doorbell of a kist launched before the person was in (Photon, which shows "waiting for your identity" until then). Answered with a count and whether be you would be answered now, at once when the count moved on since the caller's, else held until it does: the service coming to hold a fleet it is a member of with the person key handed to the kernel (so the caller's alias exists), or losing them. At most 8 held, as op 21's; a ninth is answered at once.
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
    /// Op 0 calls held for the loop to sign: the reply token, the caller's request (the stamp first) and when it came in (now_us), so the line that answers it says how long it waited and how long the signing took.
    held: Vec<([u8; 32], Vec<u8>, u64)>,
    /// Bindings made this run, by (developer, option, the alias bundle's commitment), each its whole answer.
    made: Vec<([u8; 32], [u8; 32], [u8; 32], Vec<u8>)>,
    /// The bindings kept in the vault (`KEPT_KEY`), read once a run at the first be you that is not in `made`: None until then.
    kept: Option<Vec<fgtw::binding::Binding>>,
    /// The add under way, and how the last one ended.
    adding: Option<Adding>,
    pub state: u8,
    /// Op 2 calls held (their reply tokens) until the count moves on.
    waiting: Vec<[u8; 32]>,
    /// Op 2's count, moved on each time be you's readiness changes, and the readiness it was last counted at.
    ready_gen: u32,
    ready_seen: bool,
}

impl Facets {
    pub fn new() -> Facets {
        Facets { held: Vec::new(), made: Vec::new(), kept: None, adding: None, state: add::NONE, waiting: Vec::new(), ready_gen: 0, ready_seen: false }
    }
}

impl Client {
    /// Work the loop's next pass does at once: a binding held to be signed (`facets_work` signs one a pass), or an add's chain to look up now the network is up (`person_step`).
    pub fn facets_pending(&self) -> bool {
        !self.facets.held.is_empty() || (self.net_up && self.facets.adding.as_ref().is_some_and(|a| !a.looked))
    }

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

    /// The facets served now, at the top of the loop's pass and between its slow steps (2026-11-17, the Pro: a kist's first wait for be you was answered 5 to 6 s after it came, behind the pass's other work): calls taken in, every held wait rung if the readiness changed since (`facets_ring`), and a held be you checked and signed. A call is so answered within one slow step of its coming, never a whole pass; nothing here waits or runs on a clock. True when a call came.
    pub fn facets_now(&mut self, lap: &mut Lap) -> bool {
        let busy = self.facets_recv();
        self.facets_ring();
        self.facets_work();
        lap.mark(b"facets");
        busy
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
                    Ok(None) => self.facets.held.push((tok, msg, now_us())),
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
                Some(fx::WAIT) if body.len() >= 5 => {
                    if !st.kist || !st.be_you {
                        self.facets_said(&st, "the wait for be you", fx::NOT_GRANTED);
                        self.reply_to(&tok, &[fx::NOT_GRANTED]);
                        continue;
                    }
                    self.ready_count();
                    let theirs = u32::from_le_bytes([body[1], body[2], body[3], body[4]]);
                    if theirs != self.facets.ready_gen || self.facets.waiting.len() >= 8 {
                        self.reply_to(&tok, &self.ready_reply());
                    } else {
                        self.facets.waiting.push(tok);
                    }
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

    /// Be you would be answered now: a fleet this service is a member of, and the person key in the kernel, so a kist's alias exists. The kernel's present word follows the roots, which the member chain needs.
    fn be_you_ready(&self) -> bool {
        self.member_chain().is_some() && self.person_in_kernel()
    }

    /// Op 2's count moved on when be you's readiness differs from when it was last counted.
    fn ready_count(&mut self) {
        let ready = self.be_you_ready();
        if ready != self.facets.ready_seen {
            self.facets.ready_seen = ready;
            self.facets.ready_gen = self.facets.ready_gen.wrapping_add(1);
        }
    }

    /// Op 2's answer: [OK][count u32 LE][ready u8].
    fn ready_reply(&self) -> [u8; 6] {
        let mut r = [fx::OK, 0, 0, 0, 0, self.facets.ready_seen as u8];
        r[1..5].copy_from_slice(&self.facets.ready_gen.to_le_bytes());
        r
    }

    /// The loop's turn for op 2: every held wait answered once the readiness changed in this pass.
    pub fn facets_ring(&mut self) {
        if self.facets.waiting.is_empty() {
            return;
        }
        let was = self.facets.ready_gen;
        self.ready_count();
        if self.facets.ready_gen == was {
            return;
        }
        let r = self.ready_reply();
        for tok in core::mem::take(&mut self.facets.waiting) {
            self.reply_to(&tok, &r);
        }
    }

    /// The identity forgotten (a lock): the bindings and the add go with it; a be you still held is refused (nobody present now), so its kist is not left waiting on a call nobody answers; the op 2 waits and their count stay, and the next pass rings them with the readiness gone.
    pub fn facets_forget(&mut self) {
        for (tok, _, _) in core::mem::take(&mut self.facets.held) {
            self.reply_to(&tok, &[fx::NOT_PRESENT]);
        }
        self.facets.made.clear();
        self.facets.kept = None;
        self.facets.adding = None;
        self.facets.state = add::NONE;
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
        let (tok, msg, came) = self.facets.held.remove(0);
        let Some((st, body)) = fx::Stamp::read(&msg) else { return };
        let start = now_us();
        let r = match self.binding_check(&st, body) {
            Ok(Some(r)) => r,
            Ok(None) => match self.binding_kept(&st, body) {
                Some(r) => r,
                None => self.binding_make(&st, body),
            },
            Err(code) => alloc::vec![code],
        };
        self.facets_said(&st, "be you", r[0]);
        // Where a launch's wait for be you goes (the owner, 2026-11-16: Photon's window took 5 to 10 s on the Pro): held behind the loop's other work, then the check and the signing with every egg.
        let end = now_us();
        self.log.put(b"fgtw: facets: be you held ");
        self.log.dec((start - came) / 1000);
        self.log.put(b" ms in the loop, checked and signed in ");
        self.log.dec((end - start) / 1000);
        self.log.put(b" ms");
        self.log.line();
        self.reply_to(&tok, &r);
    }

    /// The binding signed with this device's member key, the answer kept for the run and the binding kept in the vault (`binding_keep`).
    /// Signed on the X3 (2026-11-17: 6.3 s on a worker the first launch after a boot, nearly all of it SPHINCS+), as the add and the departure sign there.
    fn binding_make(&mut self, st: &fx::Stamp, body: &[u8]) -> Vec<u8> {
        let (Some((hp, chain)), Some((bundle, _))) = (self.member_chain(), parse_be_you(body)) else { return alloc::vec![fx::NOT_PRESENT] };
        let mask = chain.declared_mask(&self.me);
        ferros_sys::pin(ferros_sys::CORES_BIG);
        let b = fgtw::binding::Binding::sign(&self.keys, mask, &hp, &st.developer, &st.option, bundle, eagle());
        ferros_sys::pin(0);
        let r = self.binding_answer(st, &chain, &hp, &b);
        if r[0] == fx::OK {
            self.binding_keep(b);
        }
        r
    }

    /// A binding's answer, [OK][handle proof][the pinned identity key][the binding], kept for the run.
    fn binding_answer(&mut self, st: &fx::Stamp, chain: &fgtw::fleet::MembershipBlob, hp: &[u8; 32], b: &fgtw::binding::Binding) -> Vec<u8> {
        let Some(identity) = chain.genesis_identity_pubkey() else { return alloc::vec![fx::NOT_PRESENT] };
        let Ok(doc) = b.to_vsf_bytes() else { return alloc::vec![fx::NOT_PRESENT] };
        let mut r = Vec::with_capacity(65 + doc.len());
        r.push(fx::OK);
        r.extend_from_slice(hp);
        r.extend_from_slice(&identity);
        r.extend_from_slice(&doc);
        self.facets.made.push((st.developer, st.option, b.alias.commit(), r.clone()));
        r
    }

    /// A binding this device made on an earlier run, if one names this kist and this alias and still verifies against the chain as it stands now (its signer, this device, a current member holding the schemes it signed with): the binding has no expiry (fgtw `Binding::verify` reads no time), so it holds until this device leaves the fleet or the chain asks more eggs of it, and the SPHINCS+ signature is made once per kist, not once per boot.
    fn binding_kept(&mut self, st: &fx::Stamp, body: &[u8]) -> Option<Vec<u8>> {
        let (hp, chain) = self.member_chain()?;
        let (bundle, _) = parse_be_you(body)?;
        if self.facets.kept.is_none() {
            let list = self.store_get_whole(KEPT_KEY).map(|d| kept_read(&d, &hp)).unwrap_or_default();
            self.facets.kept = Some(list);
        }
        let commit = bundle.commit();
        let me = self.me;
        let b = self.facets.kept.as_ref()?.iter().find(|b| b.developer == st.developer && b.option == st.option && b.alias.commit() == commit && b.device == me)?.clone();
        if b.verify(&chain).is_err() {
            self.log.put(b"fgtw: facets: a kept binding no longer verifies against the chain; signing a new one");
            self.log.line();
            return None;
        }
        Some(self.binding_answer(st, &chain, &hp, &b))
    }

    /// A new binding into the kept list (one per kist and alias: an older one for the same is replaced), written whole to the vault in the person's class (`fgtw.` is the identity class, AUTH.md), without the handle proof, which is a root: it is put back from the roots in hand when the list is read (`kept_read`), and the signature covers it, so a kept binding read under another identity fails to verify.
    fn binding_keep(&mut self, b: fgtw::binding::Binding) {
        let mut list = match self.facets.kept.take() {
            Some(l) => l,
            None => self.hp.and_then(|hp| self.store_get_whole(KEPT_KEY).map(|d| kept_read(&d, &hp))).unwrap_or_default(),
        };
        let commit = b.alias.commit();
        list.retain(|k| !(k.developer == b.developer && k.option == b.option && k.alias.commit() == commit));
        list.push(b);
        match kept_doc(&list) {
            Some(doc) if self.store_set_whole(KEPT_KEY, &doc) => {}
            _ => {
                self.log.put(b"fgtw: facets: the binding was not kept in the vault; the next boot signs it again");
                self.log.line();
            }
        }
        self.facets.kept = Some(list);
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

/// The bindings this device made, kept in the person's class (2026-11-17): one VSF document, a section `be_you` for each, its fields by their honest types and no handle proof.
const KEPT_KEY: &[u8] = b"be_you";
const KEPT_SECTION: &str = "be_you";

/// The kept list as a document; None when it does not build.
fn kept_doc(list: &[fgtw::binding::Binding]) -> Option<Vec<u8>> {
    use vsf::types::EtType;
    use vsf::VsfType;
    let mut doc = vsf::VsfBuilder::new().creation_time_oscillations(eagle());
    for b in list {
        let mut sec = vsf::VsfSection::new(KEPT_SECTION);
        sec.add_field("developer", VsfType::ke(b.developer.to_vec()));
        sec.add_field("option", VsfType::hp(b.option.to_vec()));
        sec.add_field("alias", VsfType::ge(b.alias.to_bytes()));
        sec.add_field("when", VsfType::e(EtType::e6(b.when)));
        sec.add_field("device", VsfType::ke(b.device.to_vec()));
        sec.add_field("eggs", VsfType::gm(b.sig.clone()));
        doc = doc.add_section_direct(sec);
    }
    doc.build().ok()
}

/// The kept list read back, each binding given the handle proof in hand (`hp`); a document that does not verify, or a section that does not read, gives nothing of it (the binding is signed again).
fn kept_read(doc: &[u8], hp: &[u8; 32]) -> Vec<fgtw::binding::Binding> {
    use vsf::VsfType;
    let Ok((header, end)) = vsf::verification::read_verified(doc, None) else { return Vec::new() };
    let Ok(sections) = header.sections(doc, end) else { return Vec::new() };
    sections
        .iter()
        .filter(|s| s.name == KEPT_SECTION)
        .filter_map(|s| {
            let first = |name: &str| s.get_field(name).and_then(|f| f.values.first());
            let key = |name: &str| match first(name) {
                Some(VsfType::ke(b) | VsfType::hp(b)) => <[u8; 32]>::try_from(b.as_slice()).ok(),
                _ => None,
            };
            let alias = match first("alias") {
                Some(VsfType::ge(b)) => fgtw::pq::KeyBundle::from_bytes(b).ok()?,
                _ => return None,
            };
            let sig = match first("eggs") {
                Some(VsfType::gm(b)) => b.clone(),
                _ => return None,
            };
            Some(fgtw::binding::Binding { handle_proof: *hp, developer: key("developer")?, option: key("option")?, alias, when: s.eagle("when")?, device: key("device")?, sig })
        })
        .collect()
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
