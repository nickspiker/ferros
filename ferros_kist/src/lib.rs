//! Kists (docs/KISTS.md): what an installed app may be handed, how its developer declares it, and the person's answers.
//!
//! - **A right** is one thing a kist can be handed at spawn: a grant (owner, 2026-11-08: "a grant is the permission"). The vocabulary is fixed here, a right named and numbered by its place in `RIGHTS`; the kernel hands each one out as its own object (process.rs `kist_object`).
//! - **The declaration** is in the signed capsule: a section `kist` (`name`, the app's own name, text) and a section `wants`, one field per right the kist asks for, named by the right, its value one line of the developer's reason (text, a human's words). The fields' order is the kist's grant slot order: slot 1 is the first want, and a right the person said no to is an empty slot, never a fake. Nothing undeclared can be granted: the kernel checks every launch against the capsule it runs.
//! - **The list** is the person's (`kists.list`, identity class, so it follows them to every device of their fleet): one VSF document, a section `defaults` (a field per right, 1 on or 0 off: what the first-launch question starts each switch at; out of the box every one is off) and a section `kist` per installed kist: `capsule` (hb, the blake3 of the signed capsule, its vault address), `developer` (ke) and `option` (hp; in the capsule it is the `kist` section's `option`, hR), together the kist's identity as the gate sees it, `name` (text), and a field per want named by its right: the reason (ASCII text, `a`: vsf encodes UTF-8 text, `x`, only with std, and the map and init are no_std), and once the person answered, the answer (unsigned, 1 granted or 0 not) and when (Eagle time). A want never answered has no answer and is asked at the next launch, once (owner, 2026-11-08: "asked once, ever, per person, per kist, per permission").
#![no_std]

extern crate alloc;

pub use vsf;

use alloc::string::String;
use alloc::vec::Vec;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::types::EtType;
use vsf::VsfType;

/// The rights, by code (the index): the screen (`use display`), touch (`use touch`), a surface of its own, a key derived from the device's ira, its own vault domain, randomness, the GPS fix (`use gps`), the network (`use udp`).
pub const RIGHTS: [&str; 8] = ["display", "touch", "surface", "ira", "domain", "entropy", "gps", "udp"];
/// What each right is, in a person's words, for the settings pages and the first-launch question.
pub const MEANS: [&str; 8] = ["the screen", "touch", "a window of its own", "a key of this device", "its own storage", "randomness", "where you are", "the network"];
/// A want the person said no to: an empty slot in the launch (process.rs `Object::Empty`).
pub const EMPTY: u8 = 0xFF;
/// The person's list's name in its scope (`kists.`).
pub const LIST: &[u8] = b"list";

/// The `launch` endpoint init serves (payloads/init) and the map calls: [OP_LAUNCH][the capsule's 32-byte address] → [one of the answers below].
pub mod launch {
    pub const OP_LAUNCH: u8 = 0;
    /// [OP_RELAUNCH][address]: a running kist killed (one shot dead) and launched again with its answers as they now stand, after the person changed one (a revoke). STARTED relaunched; RUNNING when it was not running (nothing done); ASK, NOT_INSTALLED, REFUSED as for a launch.
    pub const OP_RELAUNCH: u8 = 1;
    /// Queued: the kernel streams the capsule from the vault and starts it.
    pub const STARTED: u8 = 0;
    /// A process of that kist is alive, or starting.
    pub const RUNNING: u8 = 1;
    /// A want is unanswered: the first-launch question first (init never asks; the map does).
    pub const ASK: u8 = 2;
    /// No such kist in the person's list.
    pub const NOT_INSTALLED: u8 = 3;
    /// The kernel refused the launch (the reason on the console).
    pub const REFUSED: u8 = 4;
    /// The person's list could not be read (nobody present).
    pub const UNREADABLE: u8 = 5;
}

/// A right's code from its name; None for a name outside the vocabulary.
pub fn code(name: &str) -> Option<u8> {
    RIGHTS.iter().position(|r| *r == name).map(|i| i as u8)
}

/// One want: its right, the developer's reason, and the person's answer (granted, and when in Eagle oscillations) once they gave one.
#[derive(Clone, PartialEq, Debug)]
pub struct Want {
    pub right: u8,
    pub reason: String,
    pub answer: Option<(bool, i64)>,
}

/// What a capsule declares: its app's name and its wants, in slot order.
#[derive(Clone, PartialEq, Debug)]
pub struct Decl {
    pub name: String,
    pub wants: Vec<(u8, String)>,
}

/// One installed kist in the person's list.
#[derive(Clone, PartialEq, Debug)]
pub struct Kist {
    pub capsule: [u8; 32],
    pub developer: [u8; 32],
    pub option: [u8; 32],
    pub name: String,
    pub wants: Vec<Want>,
}

/// The person's list: their defaults (bit k: right k starts on at the first-launch question) and their kists.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Kists {
    pub defaults: u32,
    pub kists: Vec<Kist>,
}

/// A section of a VSF file by its table of contents, parsed alone: a capsule's code (tens of megabytes) is never read to find its declaration.
fn section(file: &[u8], name: &str) -> Result<Option<VsfSection>, &'static str> {
    let (header, _) = VsfHeader::decode(file).map_err(|_| "not a VSF file")?;
    let Some(entry) = header.fields.iter().find(|f| f.name == name) else { return Ok(None) };
    let mut at = entry.offset_bytes;
    // WHY: the offset is the file's own; one past its end has no section to parse.
    if at >= file.len() {
        return Err("a section past the file's end");
    }
    VsfSection::parse(file, &mut at).map(Some).map_err(|_| "a section that does not parse")
}

/// The wants of a `wants` section in order: each field's name a right of the vocabulary, its value the reason. A name outside it, a right twice or a reason that is not text refuses the whole declaration.
fn wants_of(s: &VsfSection) -> Result<Vec<(u8, String)>, &'static str> {
    let mut out: Vec<(u8, String)> = Vec::with_capacity(s.fields.len());
    for f in &s.fields {
        let c = code(&f.name).ok_or("a want outside the rights' vocabulary")?;
        if out.iter().any(|w| w.0 == c) {
            return Err("a right wanted twice");
        }
        let reason = f.values.iter().find_map(|v| v.as_string()).ok_or("a want without its reason")?;
        out.push((c, String::from(reason)));
    }
    Ok(out)
}

/// The declaration a signed capsule carries (its `kist` and `wants` sections; a capsule without them is a kist that wants nothing, named "kist"). The signature is the gate's to check (the kernel's, or the installer's before it writes the list): this only reads.
pub fn declaration(capsule: &[u8]) -> Result<Decl, &'static str> {
    let name = section(capsule, "kist")?.and_then(|s| s.text("name").map(String::from)).unwrap_or_else(|| String::from("kist"));
    let wants = match section(capsule, "wants")? {
        Some(s) => wants_of(&s)?,
        None => Vec::new(),
    };
    Ok(Decl { name, wants })
}

/// Just the codes of a capsule's wants, in slot order (the kernel's check at launch).
pub fn want_codes(capsule: &[u8]) -> Result<Vec<u8>, &'static str> {
    Ok(declaration(capsule)?.wants.into_iter().map(|w| w.0).collect())
}

/// The developer's key (the header's `ke`) and the option (the `kist` section's `option`, `hR`): the kist's identity across its versions. Until 2026-11-10 the option rode in the header's provenance hash, which every VSF reader then called tampered (it is the content's BLAKE3 by definition); a capsule that carries it there is not read.
pub fn identity(capsule: &[u8]) -> Result<([u8; 32], [u8; 32]), &'static str> {
    let (h, _) = VsfHeader::decode(capsule).map_err(|_| "not a VSF file")?;
    let developer = match &h.signer_pubkey {
        Some(VsfType::ke(k)) if k.len() == 32 => k[..].try_into().unwrap(),
        _ => return Err("no Ed25519 developer key"),
    };
    let option = option_of(capsule)?.ok_or("no kist option")?;
    Ok((developer, option))
}

/// The `kist` section's `option` (`hR`, 32 bytes), or None for a capsule with no kist section or no option in it.
pub fn option_of(capsule: &[u8]) -> Result<Option<[u8; 32]>, &'static str> {
    Ok(section(capsule, "kist")?.and_then(|s| s.get_field("option").and_then(|f| f.values.first()).and_then(|v| match v {
        VsfType::hR(o) if o.len() == 32 => Some(o[..].try_into().unwrap()),
        _ => None,
    })))
}

/// The sections a capsule's declaration adds, for the signer (ferros-bridge `capsule --kist`): `kist` with the name, `wants` with a field per want, or no `wants` at all for a kist that wants nothing (2026-11-10: an empty section was written and did not parse back, so the clock's capsule could not be installed; a capsule without one already reads as wanting nothing).
pub fn declaration_sections(d: &Decl, option: Option<[u8; 32]>) -> (VsfSection, Option<VsfSection>) {
    let mut k = VsfSection::new("kist");
    k.add_field("name", VsfType::a(vsf::Ascii::checked("name", d.name.clone())));
    // The kist's identity across versions (ORACLE.md §5: minted once by the developer, kept), as its own field: the header's `hp` stays the content's provenance.
    if let Some(o) = option {
        k.add_field("option", VsfType::hR(o.to_vec()));
    }
    if d.wants.is_empty() {
        return (k, None);
    }
    let mut w = VsfSection::new("wants");
    for (c, reason) in &d.wants {
        w.add_field(RIGHTS[*c as usize], VsfType::a(vsf::Ascii::checked("reason", reason.clone())));
    }
    (k, Some(w))
}

impl Kists {
    /// The person's list from its document; an empty list for a document that does not parse (a list never written reads as none installed).
    pub fn parse(doc: &[u8]) -> Result<Kists, &'static str> {
        let (header, end) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
        let sections = header.sections(doc, end).map_err(|_| "sections that do not parse")?;
        let mut out = Kists::default();
        for s in &sections {
            match s.name.as_str() {
                "defaults" => {
                    for f in &s.fields {
                        if let (Some(c), Some(v)) = (code(&f.name), f.values.first().and_then(|v| v.as_u64())) {
                            if v == 1 {
                                out.defaults |= 1 << c;
                            }
                        }
                    }
                }
                "kist" => {
                    let (Some(capsule), Some(developer), Some(option)) = (s.bytes32("capsule"), s.bytes32("developer"), s.bytes32("option")) else {
                        return Err("a kist without its capsule or identity");
                    };
                    let name = String::from(s.text("name").unwrap_or("kist"));
                    let mut wants = Vec::new();
                    for f in &s.fields {
                        let Some(right) = code(&f.name) else { continue };
                        // Each value by its type marker (AGENT.md): the text is the reason, the unsigned the answer, the Eagle time when.
                        let mut reason = String::new();
                        let (mut answer, mut when) = (None, 0i64);
                        for v in &f.values {
                            if let Some(t) = v.as_string() {
                                reason = String::from(t);
                            } else if let VsfType::e(_) = v {
                                when = v.as_eagle().unwrap_or(0);
                            } else if let Some(n) = v.as_u64() {
                                answer = Some(n == 1);
                            }
                        }
                        wants.push(Want { right, reason, answer: answer.map(|a| (a, when)) });
                    }
                    out.kists.push(Kist { capsule, developer, option, name, wants });
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// The list as a complete VSF document stamped `now` (Eagle oscillations), with its provenance hash.
    pub fn build(&self, now: i64) -> Result<Vec<u8>, String> {
        let mut d = VsfSection::new("defaults");
        for (c, r) in RIGHTS.iter().enumerate() {
            d.add_field(*r, VsfType::u((self.defaults >> c & 1) as usize, false));
        }
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_section_direct(d);
        for k in &self.kists {
            let mut s = VsfSection::new("kist");
            s.add_field("capsule", VsfType::hb(k.capsule.to_vec()));
            s.add_field("developer", VsfType::ke(k.developer.to_vec()));
            s.add_field("option", VsfType::hp(k.option.to_vec()));
            s.add_field("name", VsfType::a(vsf::Ascii::checked("name", k.name.clone())));
            for w in &k.wants {
                let mut values = alloc::vec![VsfType::a(vsf::Ascii::checked("reason", w.reason.clone()))];
                if let Some((granted, when)) = w.answer {
                    values.push(VsfType::u(granted as usize, false));
                    values.push(VsfType::e(EtType::e6(when)));
                }
                s.add_field_multi(RIGHTS[w.right as usize], values);
            }
            b = b.add_section_direct(s);
        }
        b.build()
    }

    /// The kist whose capsule has this address.
    pub fn by_capsule(&self, capsule: &[u8; 32]) -> Option<&Kist> {
        self.kists.iter().find(|k| k.capsule == *capsule)
    }

    /// An install (or an update) of a capsule with this identity and declaration: the same kist (developer and option) keeps every answer it had for a right it still wants; a want that is new to it is unanswered, off until the person answers (owner, 2026-11-08). Returns the kist's index.
    pub fn install(&mut self, capsule: [u8; 32], developer: [u8; 32], option: [u8; 32], d: &Decl) -> usize {
        let old = self.kists.iter().position(|k| k.developer == developer && k.option == option);
        let wants = d
            .wants
            .iter()
            .map(|(right, reason)| {
                let answer = old.and_then(|i| self.kists[i].wants.iter().find(|w| w.right == *right)).and_then(|w| w.answer);
                Want { right: *right, reason: reason.clone(), answer }
            })
            .collect();
        let k = Kist { capsule, developer, option, name: d.name.clone(), wants };
        match old {
            Some(i) => {
                self.kists[i] = k;
                i
            }
            None => {
                self.kists.push(k);
                self.kists.len() - 1
            }
        }
    }
}

impl Kist {
    /// Whether any want is still unanswered (the first-launch question is due).
    pub fn unanswered(&self) -> bool {
        self.wants.iter().any(|w| w.answer.is_none())
    }

    /// The launch's slots for init: each want's right where the person granted it, `EMPTY` where they did not. None while a want is unanswered: nothing launches before its question.
    pub fn launch_codes(&self) -> Option<Vec<u8>> {
        self.wants.iter().map(|w| w.answer.map(|(g, _)| if g { w.right } else { EMPTY })).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    fn decl() -> Decl {
        Decl { name: String::from("Mahere"), wants: alloc::vec![(0, String::from("to draw the map")), (6, String::from("to show where you are"))] }
    }

    #[test]
    fn list_round_trips_and_updates_keep_answers() {
        let mut k = Kists::default();
        k.defaults = 1 << 7;
        let i = k.install([1; 32], [2; 32], [3; 32], &decl());
        assert!(k.kists[i].unanswered());
        assert_eq!(k.kists[i].launch_codes(), None);
        k.kists[i].wants[0].answer = Some((true, 5));
        k.kists[i].wants[1].answer = Some((false, 6));
        assert_eq!(k.kists[i].launch_codes(), Some(alloc::vec![0, EMPTY]));
        let doc = k.build(7).unwrap();
        let back = Kists::parse(&doc).unwrap();
        assert_eq!(back, k);
        // An update that adds a want: the old answers stand, the new want is unanswered.
        let mut d2 = decl();
        d2.wants.push((4, String::from("to keep your places")));
        let mut k2 = back.clone();
        let j = k2.install([9; 32], [2; 32], [3; 32], &d2);
        assert_eq!(j, i);
        assert_eq!(k2.kists[j].capsule, [9; 32]);
        assert_eq!(k2.kists[j].wants[0].answer, Some((true, 5)));
        assert_eq!(k2.kists[j].wants[1].answer, Some((false, 6)));
        assert_eq!(k2.kists[j].wants[2].answer, None);
        // Another developer's app of the same name is a kist of its own.
        let o = k2.install([8; 32], [4; 32], [3; 32], &decl());
        assert_ne!(o, j);
    }

    #[test]
    fn declaration_reads_from_a_capsule_without_its_code() {
        let (kist, wants) = declaration_sections(&decl(), Some([5; 32]));
        let mut code = VsfSection::new("aarch64");
        code.add_field("code", VsfType::v(b'b', alloc::vec![7u8; 1 << 12]));
        let file = vsf::VsfBuilder::new().provenance_only().add_section_direct(kist).add_section_direct(wants.unwrap()).add_section_direct(code).build().unwrap();
        let d = declaration(&file).unwrap();
        assert_eq!(d, decl());
        assert_eq!(want_codes(&file).unwrap(), alloc::vec![0, 6]);
        // The option is the kist section's, and the header's provenance hash is the content's own.
        assert_eq!(option_of(&file).unwrap(), Some([5; 32]));
        let (h, _) = VsfHeader::decode(&file).unwrap();
        assert!(matches!(&h.provenance_hash, VsfType::hp(p) if p[..] != [5u8; 32]));
        // A capsule with no declaration wants nothing.
        let mut c = VsfSection::new("aarch64");
        c.add_field("code", VsfType::v(b'b', alloc::vec![1u8; 16]));
        let bare = vsf::VsfBuilder::new().add_section_direct(c).build().unwrap();
        assert_eq!(declaration(&bare).unwrap().wants.len(), 0);
    }

    #[test]
    fn a_want_outside_the_vocabulary_refuses_the_declaration() {
        let mut w = VsfSection::new("wants");
        w.add_field("camera", VsfType::a(vsf::Ascii::lit("photos")));
        let file = vsf::VsfBuilder::new().add_section_direct(w).build().unwrap();
        assert!(declaration(&file).is_err());
    }
}

/// The person's list through a process's Store grant on `kists.` (init, the map), a transfer at a time through a buffer on the stack (a heap may be a DMA region outside the arc, where a syscall's bytes would not land).
#[cfg(feature = "process")]
pub mod store {
    use super::*;

    /// The list as the vault holds it now: an empty list when none was ever written; None when the grant is refused (nobody present) or the document does not parse.
    pub fn read(cap: u64) -> Option<Kists> {
        let mut buf = [0u8; 4096];
        let mut doc = Vec::new();
        // Absent is an empty list; refused (nobody present, no grant) is None, so nothing is ever written over a list that could not be read.
        let whole = match ferros_sys::store_read_at(cap, LIST, 0, &mut buf) {
            ferros_sys::NOTHING => return Some(Kists::default()),
            ferros_sys::ERR => return None,
            n => n as usize,
        };
        loop {
            let at = doc.len();
            // WHY: the length is the vault's; the last piece is shorter than the buffer.
            let n = (whole - at).min(buf.len());
            doc.extend_from_slice(&buf[..n]);
            if doc.len() >= whole {
                break;
            }
            ferros_sys::store_get_at(cap, LIST, doc.len(), &mut buf)?;
        }
        Kists::parse(&doc).ok()
    }

    /// The list written whole, stamped `now` (Eagle oscillations); false when the vault did not take it.
    pub fn write(cap: u64, list: &Kists, now: i64) -> bool {
        let Ok(doc) = list.build(now) else { return false };
        let mut buf = [0u8; 4096];
        let mut at = 0;
        while at < doc.len() {
            let n = (doc.len() - at).min(buf.len());
            buf[..n].copy_from_slice(&doc[at..at + n]);
            if !ferros_sys::store_set_at(cap, LIST, at, &buf[..n], at + n < doc.len()) {
                return false;
            }
            at += n;
        }
        true
    }
}

/// A signed capsule from the host, if one was built (tools/mahere-kist.sh writes target/mahere.capsule.vsf): its declaration, identity and code section read as the kernel reads them. `cargo test -p ferros_kist -- --ignored`.
#[cfg(test)]
mod capsule_on_disk {
    extern crate std;
    use super::*;

    #[test]
    #[ignore]
    fn mahere_capsule_reads_as_the_kernel_reads_it() {
        let Ok(c) = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/mahere.capsule.vsf")) else { return };
        let d = declaration(&c).unwrap();
        std::println!("{} wants {:?}", d.name, d.wants.iter().map(|w| RIGHTS[w.0 as usize]).collect::<Vec<_>>());
        let (_, option) = identity(&c).unwrap();
        std::println!("option {:02x?}", &option[..4]);
        let (h, end) = VsfHeader::decode(&c).unwrap();
        let e = h.fields.iter().find(|f| f.name == "aarch64").unwrap();
        let at = (e.offset_bytes..e.offset_bytes + 64).find(|&p| c[p] == b'v' && c[p + 1] == b'b').unwrap();
        std::println!("header ends {end}, code section at {}, vb at {at}", e.offset_bytes);
        assert!(at < 1 << 16);
    }
}
