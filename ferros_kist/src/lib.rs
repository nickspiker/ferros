//! Kists (docs/KISTS.md): what a stowed app may be handed, how its developer declares it, and the person's answers.
//!
//! - **A right** is one thing a kist can be handed at spawn: a grant (owner, 2026-11-08: "a grant is the permission"). The vocabulary is fixed here, a right named and numbered by its place in `RIGHTS`; the kernel hands each one out as its own object (process.rs `kist_object`).
//! - **The declaration** is in the signed capsule: a section `kist` (`name`, the app's own name, text) and a section `wants`, one field per right the kist asks for, named by the right, its value one line of the developer's reason (text, a human's words). The fields' order is the kist's grant slot order: slot 1 is the first want, and a right the person said no to is an empty slot, never a fake. Nothing undeclared can be granted: the kernel checks every launch against the capsule it runs. Since 2026-11-10 the `kist` section also carries the kist's `version` ([`Version`]: a `z` per part, as many parts as its developer uses), so the version is signed with everything else.
//! - **The list** is the person's (`kists.list`, identity class, so it follows them to every device of their fleet): one VSF document, a section `defaults` (a field per right, 1 on or 0 off: what the first-launch question starts each switch at; out of the box every one is off) and a section `kist` per stowed kist: `capsule` (hb, the blake3 of the signed capsule, its vault address), `developer` (ke) and `option` (hp; in the capsule it is the `kist` section's `option`, hR), together the kist's identity as the gate sees it, `name` (text), and a field per want named by its right: the reason (ASCII text, `a`: vsf encodes UTF-8 text, `x`, only with std, and the map and init are no_std), and once the person answered, the answer (unsigned, 1 granted or 0 not) and when (Eagle time). A want never answered has no answer and is asked at the next launch, once (owner, 2026-11-08: "asked once, ever, per person, per kist, per permission"). Since 2026-11-10 a kist may carry `source` (`nu`, the URL its capsule was fetched from, for a restore) and `installed` (Eagle time, when it was stowed: the stored field keeps its name from before the word was stow, so a list written then keeps its times) and `version` (a `z` per part, the stowed capsule's as its declaration gives it), and a section `kept` per unstowed kist holds its answers (`developer`, `option`, a field per right: the answer and when) for the day it is stowed again; readers from before skip them all. Since 2026-11-10 (trust on first stow) a section `pin` per developer holds the commitment of the key bundle their first kist carried (`Pin`, `Kists::trust`); readers from before skip it.
//! - **Fetching** a capsule (the stower, payloads/kists) is `fetch`: a plain HTTP GET's response decoded as it arrives, and the capsule hashed and its header read as it streams into the vault, never held whole.
#![no_std]

extern crate alloc;

pub use vsf;
/// The number bases a version is shown in (`write_version`).
pub use tatau;

pub mod fetch;
/// A signing key kept as VSF: its one reader and writer (the host tools).
pub mod secret;

use alloc::string::String;
use alloc::vec::Vec;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::types::EtType;
use vsf::VsfType;

/// The rights, by code (the index): the screen (`use display`), touch (`use touch`), a surface of its own, a key derived from the device's ira, its own vault domain, randomness, the GPS fix (`use gps`), the network (`use udp`), sound out (`use audio`, docs/AUDIO.md; 2026-11-11, appended so every code before it keeps its meaning).
pub const RIGHTS: [&str; 9] = ["display", "touch", "surface", "ira", "domain", "entropy", "gps", "udp", "audio"];
/// What each right is, in a person's words, for the settings pages and the first-launch question.
pub const MEANS: [&str; 9] = ["the screen", "touch", "a window of its own", "a key of this device", "its own storage", "randomness", "where you are", "the network", "sound"];
/// A want the person said no to: an empty slot in the launch (process.rs `Object::Empty`).
pub const EMPTY: u8 = 0xFF;
/// The person's list's name in its scope (`kists.`).
pub const LIST: &[u8] = b"list";

/// The `launch` endpoint init serves (payloads/init) and the map calls: [OP_LAUNCH][the capsule's 32-byte address] → [one of the answers below].
pub mod launch {
    pub const OP_LAUNCH: u8 = 0;
    /// [OP_RELAUNCH][address]: a running kist killed (one shot dead) and launched again with its answers as they now stand, after the person changed one (a revoke). STARTED relaunched; RUNNING when it was not running (nothing done); ASK, NOT_STOWED, REFUSED as for a launch.
    pub const OP_RELAUNCH: u8 = 1;
    /// [OP_STOP][address]: the person's Stop on the kist's card (owner, 2026-11-12; docs/KISTS.md "The deck and the card"): a running kist killed (one shot dead) and nothing launched, no record kept (the person's own act, as a relaunch's kill). STOPPED when it was running; NOT_RUNNING when it was not (nothing done); NOT_STOWED, REFUSED, UNREADABLE as for a launch.
    pub const OP_STOP: u8 = 2;
    /// Queued: the kernel streams the capsule from the vault and starts it.
    pub const STARTED: u8 = 0;
    /// A process of that kist is alive, or starting. Followed by [surface u8][generation u8] when it runs on a surface of its own: its window, which the map brings to the front (display op 59, ferros_sys::window::front; owner, 2026-11-11: a running kist's circle tapped raises it).
    pub const RUNNING: u8 = 1;
    /// A want is unanswered: the first-launch question first (init never asks; the map does).
    pub const ASK: u8 = 2;
    /// No such kist in the person's list.
    pub const NOT_STOWED: u8 = 3;
    /// The kernel refused the launch (the reason on the console).
    pub const REFUSED: u8 = 4;
    /// The person's list could not be read (nobody present).
    pub const UNREADABLE: u8 = 5;
    /// OP_STOP: it was running, and is ended.
    pub const STOPPED: u8 = 6;
    /// OP_STOP: it was not running; nothing done.
    pub const NOT_RUNNING: u8 = 7;

    /// Why a launch init answered STARTED did not start after all (owner, 2026-11-11: Mahere's circle tapped, nothing opened, and only the console said "the arc pool has no room for the kist"). The kernel's kist job keeps the reason per capsule until that kist next starts, and rings the Settings holder's word (ferros_sys::not_started).
    pub mod why {
        /// No room in the memory pool for the kist's arc, and nothing is coming back to it.
        pub const ROOM: u8 = 0;
        /// Not stowed on this device: no capsule at its address in the vault, or a masked kist with no seal here.
        pub const NOT_HERE: u8 = 1;
        /// Its checks refused it: its signature or the gate, a right it does not declare, native code in a production build.
        pub const CHECKS: u8 = 2;
        /// Its capsule could not be read: the vault process gone or busy, a read that failed, a length that changed, bytes that do not frame a capsule, or not the capsule its address names.
        pub const READ: u8 = 3;
        /// Anything else (no system secret, an arc of the wrong kind, the process not built).
        pub const OTHER: u8 = 4;
        /// It started, then ended abnormally (owner, 2026-11-11: Mahere started, faulted at once, "no toast or anything, just no indication at all"): a fault (a data or instruction abort, anything not a syscall, or a syscall buffer it does not own).
        pub const FAULT: u8 = 5;
        /// It started, then panicked (its exit value G#DEAD, ferros_sys::panic_exit and std's abort; or 101, std's main panicking).
        pub const PANIC: u8 = 6;
        /// It started, then was ended from outside, not by the person's own relaunch (the bridge's `diag kill`).
        pub const KILLED: u8 = 7;
        /// Whether a reason is a stop after a start (`FAULT`, `PANIC`, `KILLED`) rather than a start that did not happen.
        pub const fn stopped(why: u8) -> bool {
            why >= FAULT
        }
    }
}

/// Stowing a kist (docs/KISTS.md "Stowing"; owner, 2026-11-11: "Can we make a standard method so the end user doesn't have to get them from the [catalog]", "Just seems to me that we should just make a proper checked path and start using it", and "the proper term for installing a kist is stow"). One stower (payloads/kists) for every source, one set of checks: a kist fetched from the directory, and a kist offered to the device over USB (ferros_pt `caps::OFFER`, tools/ferros-send). What the kernel keeps of an offer and answers the host with, why a stow is refused, and those reasons in a person's words, shared by the kernel, the stower, the map and `ferros send`.
pub mod offer {
    /// An offer's states, as the kernel keeps them and answers a host's read (`[state][why][flags]`): carried in whole and waiting for the stower to take it; the stower checking it (its signature with every egg, the person's pin, the kernel's gate, checker and seal); on the glass, waiting for the person's Stow or Cancel; then one of the three ends.
    pub const OFFERED: u8 = 0;
    pub const CHECKING: u8 = 1;
    pub const ASKING: u8 = 2;
    /// In the person's list: their kist now.
    pub const STOWED: u8 = 3;
    /// Refused by a check (the why says which); its bytes dropped.
    pub const REFUSED: u8 = 4;
    /// The person said Cancel; its bytes dropped.
    pub const CANCELLED: u8 = 5;
    /// No such offer: another replaced it, or the device restarted.
    pub const GONE: u8 = 6;
    /// Whether a state is an end (stowed, refused, cancelled, gone): the host stops asking.
    pub const fn finished(state: u8) -> bool {
        state >= STOWED
    }
    /// Flags beside the state: bit 0 its code is native, unchecked machine code, which only a development build stows (SANDBOX.md: production refuses it).
    pub const NATIVE: u8 = 1;
    /// The part of a capsule one command carries (the bridge's `store` part form took the same): under the kernel's 6 MiB receive region with room for the command's head.
    pub const PART: usize = 1 << 22;
    /// The longest the kernel holds a host's read of an offer's state before it answers with the state unchanged, in seconds: the USB driver gives up on a kernel reply after 16 (payloads/usb `give_up`), so a hold must end well inside it, and the host asks again at once (a held call with a lease, never a timer of the host's).
    pub const HOLD_SECONDS: u64 = 1 << 3;

    /// Why a stow (offered by a host, from the directory, or a restore) was refused: the stower's reasons (payloads/kists), the map's words for them, and `ferros send`'s.
    pub mod why {
        pub const NETWORK: u8 = 1;
        pub const MISSING: u8 = 2;
        pub const SIZE: u8 = 3;
        pub const NOT_NAMED: u8 = 4;
        pub const EGGS: u8 = 5;
        /// It carries no bundle of its developer's keys (a capsule from before 2026-11-10).
        pub const NO_BUNDLE: u8 = 6;
        /// Native code from the network: a development build, never stowed from the directory.
        pub const NATIVE: u8 = 7;
        pub const CHECKER: u8 = 8;
        pub const SEAL: u8 = 9;
        pub const VAULT: u8 = 10;
        pub const BROKEN: u8 = 11;
        pub const MANIFEST: u8 = 12;
        pub const NOT_LISTED: u8 = 13;
        pub const NOBODY: u8 = 14;
        pub const ROOM: u8 = 15;
        pub const NOT_KIST: u8 = 16;
        /// The person trusted another bundle for this developer key at its first kist.
        pub const OTHER_KEY: u8 = 17;
        /// The bundle it carries is not its developer's (its Ed25519 key is not the kist's).
        pub const NOT_ITS_KEYS: u8 = 18;
        /// Native code offered to a production build, which stows masked kists only.
        pub const NATIVE_HERE: u8 = 19;
    }

    /// A reason in a person's words (English: the host tool's, and the map's Kists page until it speaks ferros_lang).
    pub fn words(w: u8) -> &'static str {
        match w {
            why::NETWORK => "no network",
            why::MISSING => "it is not where it is kept",
            why::SIZE => "its size is not the one given",
            why::NOT_NAMED => "it is not the capsule it was named as",
            why::EGGS => "its signature does not check out",
            why::NO_BUNDLE => "it does not carry its developer's keys",
            why::NATIVE => "this kist is a development build and can't be stowed from the network",
            why::CHECKER => "its code failed the check",
            why::SEAL => "it could not be sealed to this device",
            why::VAULT => "the vault did not keep it",
            why::BROKEN => "the download broke off",
            why::MANIFEST => "it comes thru its developer's own list, which this device does not read yet",
            why::NOT_LISTED => "it is not in the list of kists",
            why::NOBODY => "nobody is attested",
            why::ROOM => "there is no room to check it now",
            why::NOT_KIST => "it is not a kist",
            why::OTHER_KEY => "this kist is signed by a different key than its developer's other kists",
            why::NOT_ITS_KEYS => "the keys it carries are not its developer's",
            why::NATIVE_HERE => "it is unchecked native code, which a production build does not stow",
            _ => "something went wrong",
        }
    }

    /// A state's name, for the device's log lines (one a stage, so the next run shows where an offer stopped).
    pub fn state_name(state: u8) -> &'static str {
        match state {
            OFFERED => "offered",
            CHECKING => "checking",
            ASKING => "asking the person",
            STOWED => "stowed",
            REFUSED => "refused",
            CANCELLED => "cancelled",
            GONE => "gone",
            _ => "a state not known",
        }
    }
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

/// The field of the capsule's `kist` section, and of a directory entry, that carries the version.
pub const VERSION: &str = "version";

/// A kist's version: its parts, most significant first, as many as its developer uses (owner, 2026-11-10: "we don't limit the .'s like the major, minor, patch"). Written as one field `version` with a `z` value per part: `z` is VSF's own marker for a version, the one Photon's manifest gives its `major`, `minor` and `patch` in.
///
/// Kept as its developer wrote it (the signed parts); read only thru its significant parts ([`significant`]), so what a person is shown ([`write_version`]) is what is compared. The order (owner, 2026-11-10):
/// - **Leading zero parts mean nothing**: "00000123=123", and 0.1.0 is 1.0.
/// - **The number of parts is the stage, and more parts is newer**: "If I change my clock from ver 12 to ver 1.0 because I think it's ready to go from alpha to beta and then from 45.0 to 1.0.0 when I think it's ready to go mainstream". So 12 < 1.0 < 45.0 < 1.0.0: any two-part version is newer than every one-part version, however large.
/// - **The same number of parts** compares left to right, as numbers.
/// - **Equal** when the significant parts are the same.
#[derive(Clone, Debug, Default)]
pub struct Version(pub Vec<u64>);

/// A version's significant parts: its parts from the first that is not zero; the single part 0 for a version all zeros or with no parts.
pub fn significant(parts: &[u64]) -> &[u64] {
    match parts.iter().position(|&p| p != 0) {
        Some(at) => &parts[at..],
        None => &[0],
    }
}

impl Version {
    /// Its significant parts ([`significant`]): what is shown and what is compared.
    pub fn significant(&self) -> &[u64] {
        significant(&self.0)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Version) -> bool {
        self.significant() == other.significant()
    }
}
impl Eq for Version {}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> core::cmp::Ordering {
        let (a, b) = (self.significant(), other.significant());
        a.len().cmp(&b.len()).then_with(|| a.cmp(b))
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Version {
    /// A version as a person types it (`0.1.0`): one or more parts of decimal digits, separated by dots. None for anything else: an empty part, a sign, a part past 64 bits.
    pub fn parse(s: &str) -> Option<Version> {
        let mut parts = Vec::new();
        for p in s.split('.') {
            // WHY: `u64::from_str` takes a leading `+`; a version part is digits only.
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            parts.push(p.parse().ok()?);
        }
        Some(Version(parts))
    }

    /// The `version` field's values: a `z` per part (usize is 64 bits on every target ferros and its tools build for, so a part is kept whole).
    pub fn values(&self) -> Vec<VsfType> {
        self.0.iter().map(|&p| VsfType::z(p as usize)).collect()
    }

    /// The `version` field's values back, each by its type marker: a value that is not `z`, or no value at all, refuses it.
    pub fn from_values(values: &[VsfType]) -> Result<Version, &'static str> {
        if values.is_empty() {
            return Err("a version with no parts");
        }
        let mut parts = Vec::with_capacity(values.len());
        for v in values {
            match v {
                VsfType::z(n) => parts.push(*n as u64),
                _ => return Err("a version part that is not a version number (z)"),
            }
        }
        Ok(Version(parts))
    }
}

/// A version as a person reads it, in their number base (`f`, tatau; ferros_sys::numbers gives the device's): its significant parts ([`significant`]) joined by dots, leading parts that are zero left out (owner, 2026-11-10: "don't show leading .'s that are 0"), so 0.1.0 reads 1.0 and 0.0.3 reads 3; a version all zeros (or with no parts) reads 0. What is shown is exactly what [`Version`]'s order compares. A part is written whole in the base's digits: decimal for arabic, hexadecimal for hex, dozenal digits for DMS and DLS. It is a name, not a quantity, so DMS's doubling count and DLS's sign and unit do not apply.
pub fn write_version(w: &mut impl core::fmt::Write, f: tatau::Fmt, parts: &[u64]) -> core::fmt::Result {
    for (i, &p) in significant(parts).iter().enumerate() {
        if i > 0 {
            w.write_char('.')?;
        }
        match f.base {
            tatau::Base::Arabic => write!(w, "{p}")?,
            tatau::Base::Hex => write!(w, "{p:X}")?,
            tatau::Base::Dms | tatau::Base::Dls => tatau::dozenal_whole(w, f.style, p)?,
        }
    }
    Ok(())
}

/// The version a capsule's `kist` section carries; None for a capsule without one (signed before 2026-11-10, or by the bridge without `--version`).
pub fn version_of(capsule: &[u8]) -> Result<Option<Version>, &'static str> {
    match section(capsule, "kist")?.and_then(|s| s.get_field(VERSION).cloned()) {
        Some(f) => Version::from_values(&f.values).map(Some),
        None => Ok(None),
    }
}

/// One stowed kist in the person's list.
#[derive(Clone, PartialEq, Debug)]
pub struct Kist {
    pub capsule: [u8; 32],
    pub developer: [u8; 32],
    pub option: [u8; 32],
    pub name: String,
    pub wants: Vec<Want>,
    /// Where the capsule was fetched from (`source`, a VSF URL `nu`): a restore on another device, or after a reset, fetches it there again (payloads/kists). None for a capsule that came another way (the bridge).
    pub source: Option<String>,
    /// When this kist was stowed or updated (the stored field `installed`, its name from before the word was stow; Eagle oscillations; 0 unknown, as a list from before 2026-11-10): the stamp of its fact in the person's space (ferros_space), so a stow after an unstow wins over the unstow's tombstone.
    pub stowed: i64,
    /// The stowed capsule's version as its signed declaration gives it (`version`, a `z` per part, kept as written; since 2026-11-10): what the Kists page compares a listed version against, so it offers an update only when the listed one is newer. None for a kist stowed before versions, or from a capsule without one: any listed version is newer.
    pub version: Option<Version>,
}

/// The answers of a kist the person unstowed, kept in their list (a section `kept`: `developer`, `option`, then a field per right answered, the answer and when), so stowing the same kist again finds them and asks nothing (owner, 2026-11-07: "asked once, ever").
#[derive(Clone, PartialEq, Debug)]
pub struct Kept {
    pub developer: [u8; 32],
    pub option: [u8; 32],
    /// Each answer: (right, granted, when).
    pub answers: Vec<(u8, bool, i64)>,
}

/// A developer the person trusted on first stow (owner, 2026-11-10): the commitment of the key bundle (`fgtw::pq::KeyBundle::commit`, every scheme) that developer's first kist carried, kept in their list (a section `pin`: `developer` ke, `bundle` hp, `name` a when the kist named its developer, `pinned` e), so it follows them to every device of theirs. Every later kist or update from that developer key must carry a bundle with the same commitment. An unstow keeps it, as it keeps answers.
#[derive(Clone, PartialEq, Debug)]
pub struct Pin {
    /// The developer's Ed25519 key, as the capsule's header and the kist's identity name it.
    pub developer: [u8; 32],
    /// The commitment of the bundle first seen for it.
    pub bundle: [u8; 32],
    /// The developer's name as their first kist gave it (its declaration's `developer`), for the person's words; None when it gave none.
    pub name: Option<String>,
    /// When it was pinned (Eagle oscillations): its fact's stamp in the person's space.
    pub pinned: i64,
}

/// The person's list: their defaults (bit k: right k starts on at the first-launch question), their kists, the answers of the kists they unstowed, and the developers they trusted on first stow.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Kists {
    pub defaults: u32,
    pub kists: Vec<Kist>,
    pub kept: Vec<Kept>,
    pub pins: Vec<Pin>,
}

/// What [`Kists::trust`] found for a developer's bundle.
#[derive(Clone, PartialEq, Debug)]
pub enum Trust {
    /// No pin yet: this is the developer's first kist here, and the bundle is now pinned (the list must be written for it to hold).
    First,
    /// The pin's own bundle.
    Same,
    /// Another bundle is pinned for this developer key: the kist is refused. The pin as it stands, for its words.
    Other(Pin),
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

/// The declaration a signed capsule carries (its `kist` and `wants` sections; a capsule without them is a kist that wants nothing, named "kist"). The signature is the gate's to check (the kernel's, or the stower's before it writes the list): this only reads.
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

/// The developer's name, when the capsule's `kist` section carries one (`developer`, `a`): what the kist directory shows beside the key (tools/kistcat), and the name a pin keeps for the person's words. `capsule::unsigned` writes it when the signer gives one; a capsule without it is named by its key alone.
pub fn developer_name_of(capsule: &[u8]) -> Result<Option<String>, &'static str> {
    Ok(section(capsule, "kist")?.and_then(|s| s.get_field("developer").and_then(|f| f.values.iter().find_map(|v| match v {
        VsfType::a(t) => Some(String::from(t.as_str())),
        _ => None,
    }))))
}

/// The sections a capsule's declaration adds, for the signer (ferros-bridge `capsule --kist`): `kist` with the name, `wants` with a field per want, or no `wants` at all for a kist that wants nothing (2026-11-10: an empty section was written and did not parse back, so the clock's capsule could not be stowed; a capsule without one already reads as wanting nothing).
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
    /// The person's list from its document; an empty list for a document that does not parse (a list never written reads as none stowed).
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
                    // Each by its type marker: the source a URL (`nu`), when it was stowed an Eagle time.
                    let source = s.get_field("source").and_then(|f| f.values.first()).and_then(|v| match v {
                        VsfType::nu(u) => Some(u.clone()),
                        _ => None,
                    });
                    let stowed = s.get_field("installed").and_then(|f| f.values.first()).and_then(|v| v.as_eagle()).unwrap_or(0);
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
                    let version = match s.get_field(VERSION) {
                        Some(f) => Some(Version::from_values(&f.values)?),
                        None => None,
                    };
                    out.kists.push(Kist { capsule, developer, option, name, wants, source, stowed, version });
                }
                "kept" => {
                    let (Some(developer), Some(option)) = (s.bytes32("developer"), s.bytes32("option")) else {
                        return Err("kept answers without their kist's identity");
                    };
                    let mut answers = Vec::new();
                    for f in &s.fields {
                        let Some(right) = code(&f.name) else { continue };
                        let (mut answer, mut when) = (None, 0i64);
                        for v in &f.values {
                            if let VsfType::e(_) = v {
                                when = v.as_eagle().unwrap_or(0);
                            } else if let Some(n) = v.as_u64() {
                                answer = Some(n == 1);
                            }
                        }
                        if let Some(a) = answer {
                            answers.push((right, a, when));
                        }
                    }
                    out.kept.push(Kept { developer, option, answers });
                }
                "pin" => {
                    // Each by its type marker: the developer's key `ke`, the bundle's commitment `hp`. WHY `try_from`: the lengths are the document's; it refuses any but 32 bytes rather than reading past them.
                    let developer = s.get_field("developer").and_then(|f| f.values.iter().find_map(|v| match v {
                        VsfType::ke(k) => <[u8; 32]>::try_from(k.as_slice()).ok(),
                        _ => None,
                    }));
                    let bundle = s.get_field("bundle").and_then(|f| f.values.iter().find_map(|v| match v {
                        VsfType::hp(h) => <[u8; 32]>::try_from(h.as_slice()).ok(),
                        _ => None,
                    }));
                    let (Some(developer), Some(bundle)) = (developer, bundle) else { return Err("a pin without its developer or bundle") };
                    let name = s.get_field("name").and_then(|f| f.values.iter().find_map(|v| match v {
                        VsfType::a(t) => Some(String::from(t.as_str())),
                        _ => None,
                    }));
                    let pinned = s.get_field("pinned").and_then(|f| f.values.first()).and_then(|v| v.as_eagle()).unwrap_or(0);
                    if out.pins.iter().any(|p| p.developer == developer) {
                        return Err("a developer pinned twice");
                    }
                    out.pins.push(Pin { developer, bundle, name, pinned });
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
            if let Some(u) = &k.source {
                s.add_field("source", VsfType::nu(u.clone()));
            }
            if k.stowed != 0 {
                s.add_field("installed", VsfType::e(EtType::e6(k.stowed)));
            }
            if let Some(v) = k.version.as_ref().filter(|v| !v.0.is_empty()) {
                s.add_field_multi(VERSION, v.values());
            }
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
        for k in &self.kept {
            let mut s = VsfSection::new("kept");
            s.add_field("developer", VsfType::ke(k.developer.to_vec()));
            s.add_field("option", VsfType::hp(k.option.to_vec()));
            for (right, granted, when) in &k.answers {
                s.add_field_multi(RIGHTS[*right as usize], alloc::vec![VsfType::u(*granted as usize, false), VsfType::e(EtType::e6(*when))]);
            }
            b = b.add_section_direct(s);
        }
        for p in &self.pins {
            let mut s = VsfSection::new("pin");
            s.add_field("developer", VsfType::ke(p.developer.to_vec()));
            s.add_field("bundle", VsfType::hp(p.bundle.to_vec()));
            if let Some(n) = &p.name {
                s.add_field("name", VsfType::a(vsf::Ascii::checked("name", n.clone())));
            }
            if p.pinned != 0 {
                s.add_field("pinned", VsfType::e(EtType::e6(p.pinned)));
            }
            b = b.add_section_direct(s);
        }
        b.build()
    }

    /// The pin for this developer key, if the person trusted one.
    pub fn pin_of(&self, developer: &[u8; 32]) -> Option<&Pin> {
        self.pins.iter().find(|p| p.developer == *developer)
    }

    /// Trust on first stow (owner, 2026-11-10): the bundle a kist carries (its commitment, `fgtw::pq::KeyBundle::commit`) set against the pin for its developer key. With no pin it is pinned now, named `name` and stamped `now` (the caller writes the list); the pinned bundle passes; any other is refused, the pin left as it was. The caller has already checked every egg against that bundle and that its Ed25519 key is the developer.
    pub fn trust(&mut self, developer: &[u8; 32], bundle: &[u8; 32], name: Option<&str>, now: i64) -> Trust {
        match self.pin_of(developer) {
            Some(p) if p.bundle == *bundle => Trust::Same,
            Some(p) => Trust::Other(p.clone()),
            None => {
                self.pins.push(Pin { developer: *developer, bundle: *bundle, name: name.map(String::from), pinned: now });
                Trust::First
            }
        }
    }

    /// The pin as [`trust`](Self::trust) would judge it, changing nothing: a check before anything is kept.
    pub fn judge(&self, developer: &[u8; 32], bundle: &[u8; 32]) -> Trust {
        match self.pin_of(developer) {
            Some(p) if p.bundle == *bundle => Trust::Same,
            Some(p) => Trust::Other(p.clone()),
            None => Trust::First,
        }
    }

    /// The kist whose capsule has this address.
    pub fn by_capsule(&self, capsule: &[u8; 32]) -> Option<&Kist> {
        self.kists.iter().find(|k| k.capsule == *capsule)
    }

    /// A stow (or an update) of a capsule with this identity and declaration: the same kist (developer and option) keeps every answer it had for a right it still wants, and a kist stowed again after an unstow gets back the answers kept for it (`kept`); a want that is new to it is unanswered, off until the person answers (owner, 2026-11-08). Its source and stow time stay as they were (a stower that fetched it sets them). Returns the kist's index.
    pub fn stow(&mut self, capsule: [u8; 32], developer: [u8; 32], option: [u8; 32], d: &Decl) -> usize {
        let old = self.kists.iter().position(|k| k.developer == developer && k.option == option);
        let kept = self.kept.iter().position(|k| k.developer == developer && k.option == option);
        let wants = d
            .wants
            .iter()
            .map(|(right, reason)| {
                let had = old.and_then(|i| self.kists[i].wants.iter().find(|w| w.right == *right)).and_then(|w| w.answer);
                let answer = had.or_else(|| kept.and_then(|i| self.kept[i].answers.iter().find(|a| a.0 == *right)).map(|a| (a.1, a.2)));
                Want { right: *right, reason: reason.clone(), answer }
            })
            .collect();
        // Back in the list, the kept answers are its own again.
        if let Some(i) = kept {
            self.kept.swap_remove(i);
        }
        let (source, stowed) = old.map_or((None, 0), |i| (self.kists[i].source.clone(), self.kists[i].stowed));
        // The version is the new capsule's, which the caller sets from its declaration (`version_of`); the one before stays only while the capsule is the same.
        let version = old.filter(|&i| self.kists[i].capsule == capsule).and_then(|i| self.kists[i].version.clone());
        let k = Kist { capsule, developer, option, name: d.name.clone(), wants, source, stowed, version };
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

impl Kists {
    /// An unstow: the kist with this identity out of the list, every answer the person gave it kept (`kept`), so stowing it again asks nothing (owner, 2026-11-07: "1 means 1"). The space keeps them as well (ferros_space leaves alone an answer whose kist is not in the list). The kist removed, or None when it was not in the list.
    pub fn remove(&mut self, developer: &[u8; 32], option: &[u8; 32]) -> Option<Kist> {
        let i = self.kists.iter().position(|k| k.developer == *developer && k.option == *option)?;
        let k = self.kists.remove(i);
        let at = match self.kept.iter().position(|e| e.developer == *developer && e.option == *option) {
            Some(j) => j,
            None => {
                self.kept.push(Kept { developer: *developer, option: *option, answers: Vec::new() });
                self.kept.len() - 1
            }
        };
        // This version's answers win; an answer kept from before stays for a right this version no longer wanted.
        for w in &k.wants {
            let Some((granted, when)) = w.answer else { continue };
            match self.kept[at].answers.iter_mut().find(|e| e.0 == w.right) {
                Some(e) => *e = (w.right, granted, when),
                None => self.kept[at].answers.push((w.right, granted, when)),
            }
        }
        if self.kept[at].answers.is_empty() {
            self.kept.swap_remove(at);
        }
        Some(k)
    }

    /// The kist with this identity (developer and option).
    pub fn by_identity(&self, developer: &[u8; 32], option: &[u8; 32]) -> Option<&Kist> {
        self.kists.iter().find(|k| k.developer == *developer && k.option == *option)
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
        let i = k.stow([1; 32], [2; 32], [3; 32], &decl());
        assert!(k.kists[i].unanswered());
        assert_eq!(k.kists[i].launch_codes(), None);
        k.kists[i].wants[0].answer = Some((true, 5));
        k.kists[i].wants[1].answer = Some((false, 6));
        assert_eq!(k.kists[i].launch_codes(), Some(alloc::vec![0, EMPTY]));
        k.kists[i].version = Some(Version(alloc::vec![0, 1, 0]));
        let doc = k.build(7).unwrap();
        let back = Kists::parse(&doc).unwrap();
        assert_eq!(back, k);
        assert_eq!(back.kists[i].version.as_ref().unwrap().0, alloc::vec![0, 1, 0]);
        // The same capsule again keeps its version; a new capsule's is its own, set by the stower.
        let mut same = back.clone();
        same.stow([1; 32], [2; 32], [3; 32], &decl());
        assert_eq!(same.kists[i].version, Some(Version(alloc::vec![1, 0])));
        // An update that adds a want: the old answers stand, the new want is unanswered.
        let mut d2 = decl();
        d2.wants.push((4, String::from("to keep your places")));
        let mut k2 = back.clone();
        let j = k2.stow([9; 32], [2; 32], [3; 32], &d2);
        assert_eq!(j, i);
        assert_eq!(k2.kists[j].capsule, [9; 32]);
        assert_eq!(k2.kists[j].version, None);
        assert_eq!(k2.kists[j].wants[0].answer, Some((true, 5)));
        assert_eq!(k2.kists[j].wants[1].answer, Some((false, 6)));
        assert_eq!(k2.kists[j].wants[2].answer, None);
        // Another developer's app of the same name is a kist of its own.
        let o = k2.stow([8; 32], [4; 32], [3; 32], &decl());
        assert_ne!(o, j);
    }

    #[test]
    fn unstow_keeps_the_answers_and_a_restow_asks_nothing() {
        let mut k = Kists::default();
        let i = k.stow([1; 32], [2; 32], [3; 32], &decl());
        k.kists[i].wants[0].answer = Some((true, 5));
        k.kists[i].source = Some(String::from("http://kists.fgtw.org/mahere.vsf"));
        k.kists[i].stowed = 9;
        // The source and the stow time round trip.
        let back = Kists::parse(&k.build(10).unwrap()).unwrap();
        assert_eq!(back, k);
        let gone = k.remove(&[2; 32], &[3; 32]).unwrap();
        assert_eq!(gone.capsule, [1; 32]);
        assert!(k.kists.is_empty());
        assert!(k.remove(&[2; 32], &[3; 32]).is_none());
        // Only the answered want is kept, and the kept answers round trip.
        assert_eq!(k.kept, alloc::vec![Kept { developer: [2; 32], option: [3; 32], answers: alloc::vec![(0, true, 5)] }]);
        let back = Kists::parse(&k.build(11).unwrap()).unwrap();
        assert_eq!(back, k);
        // Stowed again (a newer capsule): the kept answer is back, the unanswered want still unanswered, nothing kept any more.
        let j = k.stow([7; 32], [2; 32], [3; 32], &decl());
        assert_eq!(k.kists[j].wants[0].answer, Some((true, 5)));
        assert_eq!(k.kists[j].wants[1].answer, None);
        assert!(k.kept.is_empty());
        // An update keeps the source and stow time it had (the stower sets new ones itself).
        k.kists[j].source = Some(String::from("http://a/b"));
        k.kists[j].stowed = 12;
        let j = k.stow([8; 32], [2; 32], [3; 32], &decl());
        assert_eq!((k.kists[j].source.as_deref(), k.kists[j].stowed), (Some("http://a/b"), 12));
        // Another kist's unstow keeps nothing of one that was never answered.
        let o = k.stow([6; 32], [4; 32], [3; 32], &decl());
        let (d, op) = (k.kists[o].developer, k.kists[o].option);
        k.remove(&d, &op);
        assert!(k.kept.is_empty());
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

    fn shown(f: tatau::Fmt, parts: &[u64]) -> String {
        let mut s = String::new();
        write_version(&mut s, f, parts).unwrap();
        s
    }

    #[test]
    fn a_version_drops_its_leading_zero_parts_only() {
        let a = tatau::Fmt::new(tatau::Base::Arabic, tatau::Style::Words);
        assert_eq!(shown(a, &[0, 1, 0]), "1.0");
        assert_eq!(shown(a, &[0, 0, 3]), "3");
        assert_eq!(shown(a, &[0, 0, 0]), "0");
        assert_eq!(shown(a, &[]), "0");
        assert_eq!(shown(a, &[1, 2]), "1.2");
        assert_eq!(shown(a, &[2, 0, 1]), "2.0.1");
        assert_eq!(shown(a, &[0, 0, 2, 0, 0, 7, 1, 0, 0, 0, 65536, 0]), "2.0.0.7.1.0.0.0.65536.0");
        assert_eq!(shown(a, &[u64::MAX]), "18446744073709551615");
    }

    #[test]
    fn a_version_reads_in_the_persons_base() {
        assert_eq!(shown(tatau::Fmt::new(tatau::Base::Hex, tatau::Style::Words), &[0, 1, 10, 255]), "1.A.FF");
        // Dozenal digits, whole: twelve is Zila Zil, never a doubling count.
        let w = tatau::Fmt::new(tatau::Base::Dms, tatau::Style::Words);
        assert_eq!(shown(w, &[0, 1, 0]), "Zila.Zil");
        assert_eq!(shown(w, &[0, 0, 12]), "Zila Zil");
        assert_eq!(shown(w, &[0, 0, 0]), "Zil");
        assert_eq!(shown(tatau::Fmt::new(tatau::Base::Dls, tatau::Style::Words), &[1, 23]), "Zila.Zila Stelor");
        // Glyphs: the face's digit characters, packed.
        assert_eq!(shown(tatau::Fmt::new(tatau::Base::Dms, tatau::Style::Glyphs), &[0, 1, 13]), "\u{11}.\u{11}\u{11}");
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn versions_order_by_stage_then_parts() {
        // The owner's example: alpha 12, beta 1.0, beta 45.0, mainstream 1.0.0.
        assert!(v("12") < v("1.0"));
        assert!(v("1.0") < v("45.0"));
        assert!(v("45.0") < v("1.0.0"));
        assert!(v("9.9") < v("1.0.0"));
        assert!(v("999999") < v("0.1.0"));
        assert!(v("1.2.3") > v("1.2.2"));
        assert!(v("1.10") > v("1.9"));
        // Leading zero parts mean nothing.
        assert_eq!(v("0.1.0"), v("1.0"));
        assert_eq!(v("00000123"), v("123"));
        assert_eq!(v("0.0.0"), v("0"));
        assert_eq!(Version(Vec::new()), v("0"));
        assert_eq!(v("0.0.3").cmp(&v("3")), core::cmp::Ordering::Equal);
        assert!(v("0.0.3") < v("0.1.0"));
        // Inner and trailing zeros do count: 1.0 is a stage past 1.
        assert!(v("1") < v("1.0"));
        assert_ne!(v("1.0.0"), v("1.0"));
        // What is shown is what is compared.
        let a = tatau::Fmt::new(tatau::Base::Arabic, tatau::Style::Words);
        assert_eq!(shown(a, &v("0.1.0").0), shown(a, &v("1.0").0));
    }

    #[test]
    fn a_version_parses_and_round_trips_as_z_values() {
        assert_eq!(Version::parse("0.1.0"), Some(Version(alloc::vec![0, 1, 0])));
        assert_eq!(Version::parse("7"), Some(Version(alloc::vec![7])));
        assert_eq!(Version::parse("1.2.3.4.5.6.7.8.9"), Some(Version((1..10).collect())));
        for bad in ["", "1.", ".1", "1..2", "+1", "-1", "1.a", "1 .2", "18446744073709551616"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        // Kept as written: the leading zeros ride in the signed bytes.
        let v = Version(alloc::vec![0, 1, 0, u64::MAX]);
        assert_eq!(Version::from_values(&v.values()).unwrap().0, v.0);
        assert!(Version::from_values(&[]).is_err());
        assert!(Version::from_values(&[VsfType::z(1), VsfType::u(2, false)]).is_err());
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

/// A kist's capsule as its developer signs it (docs/KISTS.md "The capsule"; owner, 2026-11-10: "trust on first install"): the declaration, the developer's whole public bundle, the code, and an egg per scheme.
///
/// - **The bundle** rides in the `kist` section as one field `kb`, a value per scheme, each by its own key type: `ke` (Ed25519), `kvf` (Falcon-512), `kvs` (SPHINCS+). Read back by type marker, never by position, it is rebuilt into an `fgtw::pq::KeyBundle` whose commitment (`KeyBundle::commit`) is what the person's list pins for that developer. It is part of the file every egg signs.
/// - **One builder** ([`unsigned`]): the bridge (`capsule --kist`) and tools/kistcat (`kist`) both build a kist thru it, then sign it with `fgtw::release::sign_release`, so the two write the same capsule.
/// - A capsule from before 2026-11-10 carries no `kb`: it still reads (`bundle_of` gives None) and the bridge's development path still stows it as before; the network stower refuses it, as it has no bundle to check.
#[cfg(feature = "bundle")]
pub mod capsule {
    use super::*;
    use fgtw::fleet::scheme;
    use fgtw::pq::KeyBundle;

    /// The `kist` section's field that carries the developer's public bundle.
    pub const BUNDLE: &str = "kb";

    /// The code a capsule carries and how the kernel is to treat it (the bridge's `capsule` options; SANDBOX.md "Modes").
    #[derive(Clone, Copy, Debug)]
    pub struct Code<'a> {
        /// The flat image (`objcopy -O binary`).
        pub image: &'a [u8],
        /// A sandboxed image (tools/ferros-masked-build.sh): section `masked`; else native code, section `aarch64`.
        pub masked: bool,
        /// How many bytes of the image are instructions (the checker covers exactly those).
        pub text: Option<usize>,
        /// Fixed mode at arc size 2^K (masked only).
        pub fixed: Option<u32>,
        /// BTI landing pads (masked only).
        pub bti: bool,
        /// Exact mode (masked only; needs `bti`).
        pub exact: bool,
    }

    /// The code's section: `masked` or `aarch64`, the image as `code` (`v` b), then the masked modes the kernel reads.
    pub fn code_section(c: &Code) -> VsfSection {
        let mut s = VsfSection::new(if c.masked { "masked" } else { "aarch64" });
        s.add_field("code", VsfType::v(b'b', c.image.to_vec()));
        if let Some(t) = c.text {
            s.add_field("text", VsfType::u(t, false));
        }
        if let Some(k) = c.fixed.filter(|_| c.masked) {
            s.add_field("fixed", VsfType::u(k as usize, false));
        }
        if c.masked && c.bti {
            s.add_field("bti", VsfType::u(1, false));
        }
        if c.masked && c.exact {
            s.add_field("exact", VsfType::u(1, false));
        }
        s
    }

    /// A bundle's keys as the `kb` field's values, each by its own type: Err for a scheme VSF has no key type for here.
    pub fn bundle_values(b: &KeyBundle) -> Result<Vec<VsfType>, &'static str> {
        let mut v = Vec::with_capacity(3);
        for s in [scheme::ED25519, scheme::FALCON512, scheme::SPHINCS_PLUS] {
            let Some(k) = b.pubkey(s) else { continue };
            v.push(match s {
                scheme::ED25519 => VsfType::ke(k.to_vec()),
                scheme::FALCON512 => VsfType::kvf(k.to_vec()),
                _ => VsfType::kvs(k.to_vec()),
            });
        }
        if v.len() as u32 != b.mask().count_ones() {
            return Err("a bundle with a scheme a kist cannot carry");
        }
        Ok(v)
    }

    /// The `kb` field's values back into a bundle, each key by its type marker. Anything else in the field, a scheme twice, a key of the wrong length or a bundle without every scheme refuses it (owner, 2026-11-10: a kist carries its developer's whole bundle, all three schemes).
    pub fn bundle_from_values(values: &[VsfType]) -> Result<KeyBundle, &'static str> {
        let mut keys = Vec::with_capacity(values.len());
        for v in values {
            keys.push(match v {
                VsfType::ke(k) => (scheme::ED25519, k.clone()),
                VsfType::kvf(k) => (scheme::FALCON512, k.clone()),
                VsfType::kvs(k) => (scheme::SPHINCS_PLUS, k.clone()),
                _ => return Err("a value in the bundle that is no signing key"),
            });
        }
        let b = KeyBundle::new(keys)?;
        if b.mask() != fgtw::release::RELEASE_MASK {
            return Err("a bundle without every scheme");
        }
        Ok(b)
    }

    /// The developer's bundle a capsule carries (`kist`'s `kb`), read from its first bytes; None for a capsule without one (before 2026-11-10). That the bundle's Ed25519 key is the capsule's developer, and that every egg verifies against it, is the caller's check ([`carried`] does the first).
    pub fn bundle_of(capsule: &[u8]) -> Result<Option<KeyBundle>, &'static str> {
        let Some(s) = section(capsule, "kist")? else { return Ok(None) };
        match s.get_field(BUNDLE) {
            Some(f) => bundle_from_values(&f.values).map(Some),
            None => Ok(None),
        }
    }

    /// The bundle a capsule carries, if it is its developer's: present, every scheme, and its Ed25519 key the header's (the kist's `developer`).
    pub fn carried(capsule: &[u8]) -> Result<KeyBundle, &'static str> {
        let b = bundle_of(capsule)?.ok_or("no developer bundle (kb)")?;
        let (developer, _) = identity(capsule)?;
        if b.ed25519() != developer {
            return Err("its bundle is not its developer's key");
        }
        Ok(b)
    }

    /// A kist's capsule, unsigned: stamped `made` (Eagle oscillations), its header naming the bundle's Ed25519 key with an empty egg slot per scheme (`fgtw::release::release_slots`); then the `kist` section (name, option, the version when given, the developer's name when given, `kb`), the `wants` section, and the code. Sign it with `fgtw::release::sign_release` and the developer's `SigningBundle`.
    pub fn unsigned(d: &Decl, option: [u8; 32], version: Option<&Version>, developer_name: Option<&str>, bundle: &KeyBundle, code: &Code, made: i64) -> Result<Vec<u8>, String> {
        let (mut k, w) = declaration_sections(d, Some(option));
        if let Some(v) = version {
            if v.0.is_empty() {
                return Err(String::from("a version with no parts"));
            }
            k.add_field_multi(VERSION, v.values());
        }
        if let Some(n) = developer_name {
            k.add_field("developer", VsfType::a(vsf::Ascii::new(n).map_err(|e| alloc::format!("the developer's name {n:?}: {e} (a kist's text is ASCII, read by a no_std stower)"))?));
        }
        k.add_field_multi(BUNDLE, bundle_values(bundle).map_err(String::from)?);
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(made).signed_only_eggs(VsfType::ke(bundle.ed25519().to_vec()), &fgtw::release::release_slots()).add_section_direct(k);
        if let Some(w) = w {
            b = b.add_section_direct(w);
        }
        b.add_section_direct(code_section(code)).build()
    }

    /// Why [`admit`] refused a capsule.
    #[derive(Clone, PartialEq, Debug)]
    pub enum Refused {
        /// It carries no bundle of its developer (`kb`), or one that does not read: nothing to check its eggs against.
        NoBundle,
        /// The bundle it carries is not its developer's: its Ed25519 key is not the header's, or (for Photon's release key) it is not the bundle fgtw pins.
        NotItsKey,
        /// An egg is missing or does not verify against the bundle.
        Eggs,
        /// The person trusted another bundle for this developer key on its first stow: the pin, for its words ("this kist is signed by a different key than <name>'s other kists").
        OtherKey(Pin),
    }

    /// A capsule admitted: the commitment to pin for its developer (None for Photon's release key, which fgtw pins for everyone) and the developer's name as the capsule gives it.
    #[derive(Clone, PartialEq, Debug)]
    pub struct Admitted {
        pub bundle: Option<[u8; 32]>,
        pub name: Option<String>,
    }

    /// The stow-time check of a kist's signature, the one rule for the network stower (payloads/kists) and the bridge's development path (owner, 2026-11-10: trust on first stow). From the capsule's first bytes (`first`, its header and declaration), its header's signer and eggs, and the hash the eggs sign (`file_hash`):
    /// 0. the bundle is the one the capsule carries (`kb`, every scheme), its Ed25519 key the kist's developer; for Photon's release key, the bundle fgtw pins (a carried one must be that one);
    /// 1. every egg verifies against it (`fgtw::pq::verify_eggs`, the release mask: no single eggs);
    /// 2. its commitment is the one the person's list pins for that developer, or none is pinned yet (then the caller pins it with [`Kists::trust`] when it writes the list).
    ///
    /// A capsule without `kb` (before 2026-11-10) is [`Refused::NoBundle`] unless its developer is the release key.
    pub fn admit(first: &[u8], signer: &[u8; 32], eggs: &[vsf::eggs::Egg], file_hash: &[u8; 32], list: &Kists) -> Result<Admitted, Refused> {
        let (developer, _) = identity(first).map_err(|_| Refused::NoBundle)?;
        if *signer != developer {
            return Err(Refused::NotItsKey);
        }
        let carried = bundle_of(first).map_err(|_| Refused::NoBundle)?;
        let name = developer_name_of(first).ok().flatten();
        let release = developer == fgtw::release::RELEASE_ED25519;
        let bundle = match (release, carried) {
            (true, None) => fgtw::release::release_bundle(),
            (true, Some(b)) if b == fgtw::release::release_bundle() => b,
            (true, Some(_)) => return Err(Refused::NotItsKey),
            (false, None) => return Err(Refused::NoBundle),
            (false, Some(b)) if b.ed25519() == developer => b,
            (false, Some(_)) => return Err(Refused::NotItsKey),
        };
        if !fgtw::pq::verify_eggs(eggs, &bundle, file_hash, fgtw::release::RELEASE_MASK) {
            return Err(Refused::Eggs);
        }
        if release {
            return Ok(Admitted { bundle: None, name });
        }
        let commit = bundle.commit();
        match list.judge(&developer, &commit) {
            Trust::Other(p) => Err(Refused::OtherKey(p)),
            _ => Ok(Admitted { bundle: Some(commit), name }),
        }
    }

    /// The person's words for a refusal, with the pinned developer's name when there is one (else their key's first bytes, G# hex).
    pub fn refusal_words(r: &Refused) -> String {
        match r {
            Refused::NoBundle => String::from("it does not carry its developer's keys"),
            Refused::NotItsKey => String::from("the keys it carries are not its developer's"),
            Refused::Eggs => String::from("its signature does not check out"),
            Refused::OtherKey(p) => alloc::format!("this kist is signed by a different key than {}'s other kists", pin_name(p)),
        }
    }

    /// A pin's developer as the person reads it: the name their first kist gave, or their key's first four bytes.
    pub fn pin_name(p: &Pin) -> String {
        match &p.name {
            Some(n) => n.clone(),
            None => alloc::format!("G#{:02X}{:02X}_{:02X}{:02X}", p.developer[0], p.developer[1], p.developer[2], p.developer[3]),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        extern crate std;
        use fgtw::pq::{FleetSigner, SigningBundle};

        fn decl() -> Decl {
            Decl { name: String::from("Clock"), wants: alloc::vec![(0, String::from("to draw the rings"))] }
        }

        fn signed(key: &SigningBundle, name: Option<&str>) -> Vec<u8> {
            let code = Code { image: &[7u8; 256], masked: true, text: Some(128), fixed: None, bti: false, exact: false };
            fgtw::release::sign_release(unsigned(&decl(), [5; 32], Some(&Version(alloc::vec![0, 1, 0])), name, &key.public(), &code, 9).unwrap(), key).unwrap()
        }

        #[test]
        fn the_carried_bundle_round_trips_and_verifies() {
            let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
            let c = signed(&key, Some("Test Developer"));
            let b = carried(&c).unwrap();
            assert_eq!(b, key.public());
            assert_eq!(b.to_bytes(), key.public().to_bytes());
            assert_eq!(b.commit(), key.public().commit());
            // Every egg against the carried bundle, as the stower checks it.
            fgtw::release::verify_signed_by(&c, &b.ed25519(), &b).unwrap();
            assert_eq!(declaration(&c).unwrap(), decl());
            assert_eq!(identity(&c).unwrap(), (key.keypair().public.to_bytes(), [5; 32]));
            assert_eq!(developer_name_of(&c).unwrap().as_deref(), Some("Test Developer"));
            // The version is in the signed declaration.
            assert_eq!(version_of(&c).unwrap(), Some(Version(alloc::vec![0, 1, 0])));
            assert_eq!(want_codes(&c).unwrap(), alloc::vec![0]);
        }

        #[test]
        fn a_tampered_bundle_fails() {
            let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
            let c = signed(&key, None);
            // A byte of the Falcon key in `kb` changed: the eggs no longer verify over the file, and against the changed bundle they never could.
            let falcon = key.public().pubkey(scheme::FALCON512).unwrap().to_vec();
            let at = (0..c.len() - falcon.len()).find(|&i| c[i..i + falcon.len()] == falcon[..]).unwrap() + (falcon.len() >> 1);
            let mut t = c.clone();
            t[at] ^= 1;
            let b = carried(&t).unwrap();
            assert_ne!(b.commit(), key.public().commit());
            assert!(fgtw::release::verify_signed_by(&t, &b.ed25519(), &b).is_err());
            assert!(fgtw::release::verify_signed_by(&t, &key.public().ed25519(), &key.public()).is_err());
            // Another developer's bundle put in whole (signed by them): its Ed25519 key is not the header's.
            let other = SigningBundle::developer(&[4; 32], "TEST_DEVELOPER");
            let code = Code { image: &[7u8; 256], masked: true, text: Some(128), fixed: None, bti: false, exact: false };
            let mut u = unsigned(&decl(), [5; 32], None, None, &other.public(), &code, 9).unwrap();
            // The header names this key, the bundle another: built by hand, as an attacker would.
            let ed_other = other.public().ed25519();
            let ed_mine = key.public().ed25519();
            let h = (0..u.len() - 32).find(|&i| u[i..i + 32] == ed_other[..]).unwrap();
            u[h..h + 32].copy_from_slice(&ed_mine);
            let forged = fgtw::release::sign_release(u, &key).unwrap();
            assert_eq!(carried(&forged), Err("its bundle is not its developer's key"));
        }

        #[test]
        fn an_old_capsule_without_a_bundle_still_reads() {
            let (k, w) = declaration_sections(&decl(), Some([5; 32]));
            let code = Code { image: &[1u8; 16], masked: false, text: None, fixed: None, bti: false, exact: false };
            let key = SigningBundle::release(&[6; 32]);
            let u = vsf::VsfBuilder::new().signed_only_eggs(VsfType::ke(key.keypair().public.to_bytes().to_vec()), &fgtw::release::release_slots()).add_section_direct(k).add_section_direct(w.unwrap()).add_section_direct(code_section(&code)).build().unwrap();
            let c = fgtw::release::sign_release(u, &key).unwrap();
            assert_eq!(bundle_of(&c), Ok(None));
            assert!(carried(&c).is_err());
            assert_eq!(declaration(&c).unwrap(), decl());
            assert_eq!(version_of(&c), Ok(None));
        }

        /// The stower's steps for one capsule: admit, then (as its store stage does) pin and keep the kist in the list.
        fn stow(list: &mut Kists, c: &[u8]) -> Result<Trust, Refused> {
            // vsf's reader checks the Ed25519 egg as it reads the header: a capsule changed anywhere fails here already.
            let Ok((signer, eggs, file_hash)) = vsf::verification::header_eggs(c) else { return Err(Refused::Eggs) };
            let a = admit(c, &signer, &eggs, &file_hash, list)?;
            let (developer, option) = identity(c).unwrap();
            let t = match a.bundle {
                Some(b) => list.trust(&developer, &b, a.name.as_deref(), 7),
                None => Trust::Same,
            };
            list.stow(*blake3::hash(c).as_bytes(), developer, option, &declaration(c).unwrap());
            Ok(t)
        }

        #[test]
        fn trust_on_first_stow() {
            let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
            let mut list = Kists::default();
            // The first kist by this developer pins its bundle, under the name it gives.
            let first = signed(&key, Some("Test Developer"));
            assert_eq!(stow(&mut list, &first), Ok(Trust::First));
            assert_eq!(list.pins, alloc::vec![Pin { developer: key.public().ed25519(), bundle: key.public().commit(), name: Some(String::from("Test Developer")), pinned: 7 }]);
            // The pin round trips thru the list's document, and a list from before (no pin section) reads as none pinned.
            let back = Kists::parse(&list.build(8).unwrap()).unwrap();
            assert_eq!(back, list);
            // A second kist (an update) with the same bundle stows.
            let update = signed(&key, Some("Someone Else"));
            assert_eq!(stow(&mut list, &update), Ok(Trust::Same));
            assert_eq!(list.pins.len(), 1);
            assert_eq!(list.pins[0].name.as_deref(), Some("Test Developer"), "the first kist's name stands");
            // The same Ed25519 key with other post-quantum halves (another label: another bundle, every egg its own and valid) is refused, named by the pin.
            let other = SigningBundle::developer(&[3; 32], "ANOTHER_LABEL");
            assert_eq!(other.public().ed25519(), key.public().ed25519());
            assert_ne!(other.public().commit(), key.public().commit());
            let c = signed(&other, Some("Test Developer"));
            let r = stow(&mut list, &c).unwrap_err();
            assert_eq!(refusal_words(&r), "this kist is signed by a different key than Test Developer's other kists");
            assert_eq!(list.pins[0].bundle, key.public().commit(), "the pin stands");
            // An unstow keeps the pin, as it keeps answers: the other bundle is still refused.
            let (d, o) = identity(&first).unwrap();
            list.remove(&d, &o);
            assert!(list.kists.is_empty());
            assert!(matches!(stow(&mut list, &c), Err(Refused::OtherKey(_))));
            // Without the bundle the eggs cannot be checked: a capsule with no kb is not admitted (not the release key's).
            let (k, w) = declaration_sections(&decl(), Some([5; 32]));
            let code = Code { image: &[1u8; 16], masked: true, text: Some(16), fixed: None, bti: false, exact: false };
            let u = vsf::VsfBuilder::new().signed_only_eggs(VsfType::ke(key.public().ed25519().to_vec()), &fgtw::release::release_slots()).add_section_direct(k).add_section_direct(w.unwrap()).add_section_direct(code_section(&code)).build().unwrap();
            let bare = fgtw::release::sign_release(u, &key).unwrap();
            assert_eq!(stow(&mut Kists::default(), &bare), Err(Refused::NoBundle));
            // A capsule whose eggs do not verify against the bundle it carries.
            let mut t = first.clone();
            let n = t.len();
            t[n - 1] ^= 1;
            assert_eq!(stow(&mut Kists::default(), &t), Err(Refused::Eggs));
            // The stower's own path (eggs from the streamed head, the hash it computed): eggs over another hash do not verify.
            let (signer, eggs, mut file_hash) = vsf::verification::header_eggs(&first).unwrap();
            file_hash[0] ^= 1;
            assert_eq!(admit(&first, &signer, &eggs, &file_hash, &Kists::default()), Err(Refused::Eggs));
            // Eggs of another bundle over the right hash: the post-quantum eggs do not verify against the carried bundle.
            let (_, other_eggs, other_hash) = vsf::verification::header_eggs(&c).unwrap();
            let (_, _, first_hash) = vsf::verification::header_eggs(&first).unwrap();
            assert_ne!(other_hash, first_hash);
            let mixed: Vec<vsf::eggs::Egg> = eggs.iter().filter(|e| e.scheme == scheme::ED25519).cloned().chain(other_eggs.into_iter().filter(|e| e.scheme != scheme::ED25519)).collect();
            assert_eq!(admit(&first, &signer, &mixed, &first_hash, &Kists::default()), Err(Refused::Eggs));
        }

        #[test]
        fn a_bundle_short_of_a_scheme_is_refused() {
            let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
            let v = bundle_values(&key.public()).unwrap();
            assert_eq!(v.len(), 3);
            assert!(bundle_from_values(&v[..2]).is_err());
            assert!(bundle_from_values(&[v[0].clone(), v[0].clone(), v[1].clone(), v[2].clone()]).is_err());
            assert!(bundle_from_values(&[v[0].clone(), v[1].clone(), VsfType::hb(alloc::vec![0; 32])]).is_err());
        }

        /// The Clock's capsule as tools/clock-kist.sh writes it (target/kists/clock.capsule.vsf), streamed thru the stower's intake in pieces and admitted as the stower admits it: `cargo test -p ferros_kist --features bundle -- --ignored`.
        #[test]
        #[ignore]
        fn the_clock_capsule_admits_as_the_stower_streams_it() {
            let Ok(c) = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/kists/clock.capsule.vsf")) else { return };
            let mut intake = crate::fetch::Intake::new();
            for piece in c.chunks(1 << 12) {
                intake.update(piece).unwrap();
            }
            let t = intake.finish().unwrap();
            assert_eq!(t.address, *blake3::hash(&c).as_bytes());
            let mut list = Kists::default();
            let a = admit(&t.first, &t.head.signer, &t.head.eggs, &t.file_hash, &list).unwrap();
            let (developer, option) = identity(&t.first).unwrap();
            assert_eq!(list.trust(&developer, &a.bundle.unwrap(), a.name.as_deref(), 1), Trust::First);
            std::println!("{} by {:?}: developer {:02X?}, option {:02X?}, wants {:?}, bundle {:02X?}", declaration(&t.first).unwrap().name, a.name, &developer[..4], &option[..4], want_codes(&t.first).unwrap(), &a.bundle.unwrap()[..4]);
        }
    }
}

/// The kist directory (docs/KISTS.md "The directory"; owner, 2026-11-10): the public list of kists anyone may stow, at `kists.fgtw.org/kists.vsf` over plain HTTP, built by tools/kistcat and read by the device's stower.
///
/// - **It vouches for nothing.** Anyone may list a kist and no account stows one. The device checks the capsule's blake3 against its entry and the capsule's developer signature at stow; the directory's own signature (every egg of the ferros project key) only keeps a mirror, or anything on the path between, from editing the list.
/// - **One VSF document**, signed with every scheme ([`fgtw::release::release_slots`], the slots the bridge reserves for a kist) and stamped with its Eagle time: a section `kist` per entry and a section `metrics` per entry, each field named and typed and read by its type marker, never by position.
/// - **Text is ASCII** (`a`): the device's stower is no_std, and vsf decodes UTF-8 text (`x`, Huffman coded) only with std, as the person's list and the capsule's declaration already have it.
#[cfg(feature = "directory")]
pub mod directory {
    use super::*;
    use vsf::types::Ascii;

    /// The host the device fetches from (plain HTTP: the device has no TLS), and the directory's exact path on it.
    pub const HOST: &str = "kists.fgtw.org";
    pub const PATH: &str = "/kists.vsf";

    /// Where the device gets the capsule.
    #[derive(Clone, PartialEq, Debug)]
    pub enum Source {
        /// `path` (`a`): an exact path on [`HOST`], `/capsules/<blake3>.vsf`, the capsule's bytes kept by their own hash.
        Path(String),
        /// `manifest` (`nu`, a plain-http URL), `platform` and `arch` (`a`): the developer's own signed manifest (Photon's `manifest-release.vsf`). The device fetches it, checks that the entry's developer signed it with every egg, and takes the artefact section whose `platform` and `arch` are these; that artefact's blake3 must be the entry's `capsule`, or the manifest has moved past the directory and nothing is stowed until the directory is rebuilt.
        Manifest { url: String, platform: String, arch: String },
    }

    /// The kist's measures: many dimensions, never one score (owner, 2026-11-10: "not a single dimensional star system"). None is defined yet; the section is there, keyed by the kist's identity so its measures carry across versions, and a field per dimension comes later.
    #[derive(Clone, PartialEq, Debug, Default)]
    pub struct Metrics {}

    /// One listed kist.
    #[derive(Clone, PartialEq, Debug)]
    pub struct Entry {
        /// `name` (`a`): the app's own name, as its capsule's declaration gives it.
        pub name: String,
        /// `developer` (`ke`): the Ed25519 key the capsule is signed with; with `option`, the kist's identity across versions.
        pub developer: [u8; 32],
        /// `developer_name` (`a`), when the capsule's declaration names its developer.
        pub developer_name: Option<String>,
        /// `option` (`hR`): the kist's identity across versions, minted once by its developer.
        pub option: [u8; 32],
        /// `version` (a `z` per part, as the capsule's own declaration carries it). A manifest entry maps the artefact's `major`, `minor`, `patch` into three parts.
        pub version: Version,
        pub source: Source,
        /// `capsule` (`hb`): the blake3 of the signed capsule, as the person's list and the vault name it.
        pub capsule: [u8; 32],
        /// `size` (`u`): the capsule's length in bytes.
        pub size: u64,
        /// `about` (`a`): one line of what the kist is.
        pub about: String,
        /// `icon` (`a`): a path on [`HOST`] to its icon (a VSF image, as tools/app-icons makes them), when it has one.
        pub icon: Option<String>,
        /// Its `metrics` section.
        pub metrics: Metrics,
    }

    /// The directory: when it was made (Eagle oscillations), the key that signed it (None for a directory made unsigned, `build_open`, while the project key is not chosen), and its kists in listed order.
    #[derive(Clone, PartialEq, Debug)]
    pub struct Directory {
        pub made: i64,
        pub signer: Option<[u8; 32]>,
        pub kists: Vec<Entry>,
    }

    fn text(what: &str, s: &str) -> Result<VsfType, String> {
        Ascii::new(s).map(VsfType::a).map_err(|e| alloc::format!("{what} {s:?}: {e} (the directory's text is ASCII, read by a no_std stower)"))
    }

    /// The `kist` section of one entry.
    fn entry_section(e: &Entry) -> Result<VsfSection, String> {
        let mut s = VsfSection::new("kist");
        s.add_field("name", text("name", &e.name)?);
        s.add_field("developer", VsfType::ke(e.developer.to_vec()));
        if let Some(n) = &e.developer_name {
            s.add_field("developer_name", text("developer_name", n)?);
        }
        s.add_field("option", VsfType::hR(e.option.to_vec()));
        if e.version.0.is_empty() {
            return Err(alloc::format!("{}: a version with no parts", e.name));
        }
        s.add_field_multi(VERSION, e.version.values());
        match &e.source {
            Source::Path(p) => s.add_field("path", text("path", p)?),
            Source::Manifest { url, platform, arch } => {
                s.add_field("manifest", VsfType::nu(url.clone()));
                s.add_field("platform", text("platform", platform)?);
                s.add_field("arch", text("arch", arch)?);
            }
        }
        s.add_field("capsule", VsfType::hb(e.capsule.to_vec()));
        s.add_field("size", VsfType::u(e.size as usize, false));
        s.add_field("about", text("about", &e.about)?);
        if let Some(i) = &e.icon {
            s.add_field("icon", text("icon", i)?);
        }
        Ok(s)
    }

    /// The `metrics` section of one entry: the kist's identity, and no dimension yet.
    fn metrics_section(e: &Entry) -> VsfSection {
        let mut s = VsfSection::new("metrics");
        s.add_field("developer", VsfType::ke(e.developer.to_vec()));
        s.add_field("option", VsfType::hR(e.option.to_vec()));
        s
    }

    /// The directory as a complete VSF document stamped `made` (Eagle oscillations), its header naming `signer` (the project key's Ed25519 half) with an empty egg slot per scheme for the signer to fill (`fgtw::release::sign_release`, as the bridge signs a kist).
    pub fn build_unsigned(kists: &[Entry], made: i64, signer: [u8; 32]) -> Result<Vec<u8>, String> {
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(made).signed_only_eggs(VsfType::ke(signer.to_vec()), &fgtw::release::release_slots());
        for e in kists {
            b = b.add_section_direct(entry_section(e)?).add_section_direct(metrics_section(e));
        }
        b.build()
    }

    /// The directory UNSIGNED: a complete VSF document stamped `made` with its provenance hash and no signer at all, for the time before the project key is chosen (owner, 2026-11-10: the FGTW kaitiakitanga decides it). The device reads it unchecked (`parse_directory`, `signer` None) and its page says so; [`verify_directory`] refuses it. A directory vouches for nothing either way: every capsule is checked against its own developer's bundle at stow.
    pub fn build_open(kists: &[Entry], made: i64) -> Result<Vec<u8>, String> {
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only();
        for e in kists {
            b = b.add_section_direct(entry_section(e)?).add_section_direct(metrics_section(e));
        }
        b.build()
    }

    /// A field's first value of the wanted type (AGENT.md: the type marker says what a value is).
    fn value<'a, T>(s: &'a VsfSection, name: &str, f: impl Fn(&'a VsfType) -> Option<T>) -> Option<T> {
        s.get_field(name)?.values.iter().find_map(f)
    }

    fn ascii(s: &VsfSection, name: &str) -> Option<String> {
        value(s, name, |v| match v {
            VsfType::a(t) => Some(String::from(t.as_str())),
            _ => None,
        })
    }

    fn key(s: &VsfSection, name: &str) -> Option<[u8; 32]> {
        // WHY: the length is the document's; `try_into` refuses a value of any other length rather than reading past it.
        value(s, name, |v| match v {
            VsfType::ke(k) => k[..].try_into().ok(),
            _ => None,
        })
    }

    fn option(s: &VsfSection) -> Option<[u8; 32]> {
        // WHY: as `key`.
        value(s, "option", |v| match v {
            VsfType::hR(o) => o[..].try_into().ok(),
            _ => None,
        })
    }

    fn entry_of(s: &VsfSection) -> Result<Entry, &'static str> {
        let manifest = value(s, "manifest", |v| match v {
            VsfType::nu(u) => Some(u.clone()),
            _ => None,
        });
        let source = match (ascii(s, "path"), manifest) {
            (Some(p), None) => Source::Path(p),
            (None, Some(url)) => Source::Manifest {
                url,
                platform: ascii(s, "platform").ok_or("a manifest entry without its platform")?,
                arch: ascii(s, "arch").ok_or("a manifest entry without its arch")?,
            },
            (Some(_), Some(_)) => return Err("an entry with both a path and a manifest"),
            (None, None) => return Err("an entry with neither a path nor a manifest"),
        };
        Ok(Entry {
            name: ascii(s, "name").ok_or("an entry without its name")?,
            developer: key(s, "developer").ok_or("an entry without its developer key")?,
            developer_name: ascii(s, "developer_name"),
            option: option(s).ok_or("an entry without its option")?,
            version: Version::from_values(&s.get_field(VERSION).ok_or("an entry without its version")?.values)?,
            source,
            // WHY: as `key`.
            capsule: value(s, "capsule", |v| match v {
                VsfType::hb(h) => h[..].try_into().ok(),
                _ => None,
            })
            .ok_or("an entry without its capsule's blake3")?,
            size: value(s, "size", |v| v.as_u64()).ok_or("an entry without its size")?,
            about: ascii(s, "about").ok_or("an entry without its description")?,
            icon: ascii(s, "icon"),
            metrics: Metrics::default(),
        })
    }

    /// The directory's entries, read and checked for shape only: who signed it is [`verify_directory`]'s. A section of another name is skipped (a later directory may carry more); a kist listed twice (one developer and option), an entry missing a field, or measures for a kist not listed refuse the whole document.
    pub fn parse_directory(doc: &[u8]) -> Result<Directory, &'static str> {
        let (header, end) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
        let signer = match &header.signer_pubkey {
            // WHY: the key's length is the document's; anything but 32 bytes is no Ed25519 key.
            Some(VsfType::ke(k)) => Some(k[..].try_into().map_err(|_| "a signer key that is not Ed25519")?),
            Some(_) => return Err("a signer key that is not Ed25519"),
            // Unsigned (`build_open`): read unchecked, as the device reads every directory until the project key is pinned.
            None => None,
        };
        let made = header.creation_time.as_ref().and_then(|t| t.as_eagle()).ok_or("no creation time")?;
        let sections = header.sections(doc, end).map_err(|_| "sections that do not parse")?;
        let mut kists: Vec<Entry> = Vec::new();
        for s in sections.iter().filter(|s| s.name == "kist") {
            let e = entry_of(s)?;
            if kists.iter().any(|k| k.developer == e.developer && k.option == e.option) {
                return Err("a kist listed twice");
            }
            kists.push(e);
        }
        for s in sections.iter().filter(|s| s.name == "metrics") {
            let (Some(developer), Some(option)) = (key(s, "developer"), option(s)) else { return Err("measures without their kist's identity") };
            // No dimension is defined yet: the section only has to name a listed kist.
            if !kists.iter().any(|k| k.developer == developer && k.option == option) {
                return Err("measures for a kist that is not listed");
            }
        }
        Ok(Directory { made, signer, kists })
    }

    /// The directory, if every egg of `project` (the ferros project key's public bundle, pinned by the reader) verifies over it and its header names that key: the bridge's rule for a kist (`fgtw::release::verify_signed_by`, owner 2026-11-10: "no single eggs"), then [`parse_directory`].
    pub fn verify_directory(doc: &[u8], project: &fgtw::pq::KeyBundle) -> Result<Directory, String> {
        fgtw::release::verify_signed_by(doc, &project.ed25519(), project)?;
        parse_directory(doc).map_err(String::from)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        extern crate std;
        use fgtw::pq::SigningBundle;

        fn entry(n: u8, source: Source) -> Entry {
            Entry {
                name: String::from("Clock"),
                developer: [n; 32],
                developer_name: None,
                option: [n ^ 1; 32],
                version: Version(alloc::vec![0, 1, 0, 7]),
                source,
                capsule: [n ^ 2; 32],
                size: 700,
                about: String::from("the time, drawn as rings"),
                icon: None,
                metrics: Metrics::default(),
            }
        }

        fn signed(kists: &[Entry], key: &SigningBundle) -> Vec<u8> {
            use fgtw::pq::FleetSigner;
            let unsigned = build_unsigned(kists, 5, key.keypair().public.to_bytes()).unwrap();
            fgtw::release::sign_release(unsigned, key).unwrap()
        }

        #[test]
        fn build_verify_round_trip() {
            let key = SigningBundle::developer(&[3; 32], "TEST_PROJECT");
            let kists = alloc::vec![
                entry(1, Source::Path(String::from("/capsules/ab.vsf"))),
                entry(4, Source::Manifest { url: String::from("http://example.org/manifest.vsf"), platform: String::from("ferros"), arch: String::from("arm64") }),
            ];
            let doc = signed(&kists, &key);
            let d = verify_directory(&doc, &key.public()).unwrap();
            assert_eq!(d.kists, kists);
            assert_eq!(d.made, 5);
            // Signed by another key, or with one byte changed anywhere, it is refused.
            let other = SigningBundle::developer(&[4; 32], "TEST_PROJECT");
            assert!(verify_directory(&doc, &other.public()).is_err());
            for at in [doc.len() >> 1, doc.len() - 1] {
                let mut t = doc.clone();
                t[at] ^= 1;
                assert!(verify_directory(&t, &key.public()).is_err(), "a byte changed at {at}");
            }
        }

        #[test]
        fn an_unsigned_directory_reads_unchecked_and_never_verifies() {
            let key = SigningBundle::developer(&[3; 32], "TEST_PROJECT");
            let kists = alloc::vec![entry(1, Source::Path(String::from("/capsules/ab.vsf")))];
            let doc = build_open(&kists, 5).unwrap();
            let d = parse_directory(&doc).unwrap();
            assert_eq!((d.signer, d.made, d.kists), (None, 5, kists));
            assert!(verify_directory(&doc, &key.public()).is_err());
            // A signed one names its signer.
            assert_eq!(parse_directory(&signed(&[], &key)).unwrap().signer, Some(key.public().ed25519()));
        }

        #[test]
        fn an_empty_directory_round_trips() {
            let key = SigningBundle::developer(&[3; 32], "TEST_PROJECT");
            let d = verify_directory(&signed(&[], &key), &key.public()).unwrap();
            assert!(d.kists.is_empty());
        }

        #[test]
        fn a_kist_listed_twice_is_refused() {
            let key = SigningBundle::developer(&[3; 32], "TEST_PROJECT");
            let e = entry(1, Source::Path(String::from("/a.vsf")));
            assert_eq!(parse_directory(&signed(&[e.clone(), e], &key)), Err("a kist listed twice"));
        }

        #[test]
        fn a_version_of_any_length_round_trips_and_major_minor_patch_is_not_read() {
            let mut e = entry(1, Source::Path(String::from("/a.vsf")));
            for v in [alloc::vec![3], alloc::vec![0, 0, 0], (0..32).collect::<Vec<u64>>()] {
                e.version = Version(v.clone());
                let back = parse_directory(&build_open(&[e.clone()], 5).unwrap()).unwrap().kists;
                assert_eq!(back, alloc::vec![e.clone()]);
                // As the developer wrote it, leading zeros and all.
                assert_eq!(back[0].version.0, v);
            }
            e.version = Version(Vec::new());
            assert!(build_open(&[e.clone()], 5).is_err());
            // The shape before 2026-11-10 (three fields): no reader is kept for it.
            let mut s = entry_section(&entry(1, Source::Path(String::from("/a.vsf")))).unwrap();
            s.fields.retain(|f| f.name != VERSION);
            for n in ["major", "minor", "patch"] {
                s.add_field(n, VsfType::z(1));
            }
            assert_eq!(entry_of(&s), Err("an entry without its version"));
        }

        #[test]
        fn text_that_is_not_ascii_is_refused_when_built() {
            let mut e = entry(1, Source::Path(String::from("/a.vsf")));
            e.about = String::from("the time \u{2192} drawn");
            assert!(build_unsigned(&[e], 5, [0; 32]).is_err());
        }
    }
}
