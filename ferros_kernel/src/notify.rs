//! Notes (docs/NOTIFY.md; owner, 2026-11-12: "I think we try it with the in view, it can notify me, out of view, silent"): what kists have waiting on the person, kept by the kernel, read by the deck (the map) on a bell.
//!
//! - **Why the kernel keeps them:** it alone knows which kist is calling (its capsule, as launched), so a note's source is never a kist's word; it decides the kind (a kist can lower its note, never raise it: `decide`); it holds a kist to its counts (`WAITING`, `POSTS`); and the deck reads one list, rung when it changes, never asked on a timer.
//! - **Where they live:** the person's (identity class, kvault `IDENTITY`'s `\0notes.`), one vault entry a note, written at every change (AGENT.md: every change hits disk). A note is about the person (who wrote to them, what about): it is never readable before attest, it follows the person (the fleet clears it on every device of theirs), and a release wipes it with the rest of the identity. Loaded the first time any op runs with a person present; dropped from RAM when nobody is.
//! - **What is stored and served** is the kernel's own rebuild of what it parsed (`ferros_kist::note::build`), never the kist's bytes.
//! - **Counts, never time:** a kist has at most `WAITING` notes waiting, and posts at most `POSTS` between two looks from the person (`seen` gives its budget back). Past either, a post is refused (NOTHING) until the person has looked.
//! - **System questions** (source `note::SYSTEM`, posted by the map: docs/NOTIFY.md "System questions") are personal at most and live in RAM alone, as long as what asks them.
//! - **A deadline** is the kist's (one a note): a note past it goes as if withdrawn, at the next op. The deck waits for the earliest one as for an alarm (its watch's deadline), then lists, which drops it here and rings.
use super::*;
use ferros_kist::note::{self, Note, Poster, LIVE, PERSONAL, POSTS, STATUS, WAITING};
/// A record of op 3: [source 32][number u8][kind u8][asked u8][flags u8: bit 0 names a person, bit 1 has a target][count u32 LE][seq u64 LE, the notes' bell count it was posted at][deadline i64 LE, Eagle oscillations, 0 none].
pub const REC: usize = 56;
/// The vault prefix (kvault `IDENTITY`): reserved (NUL first), so no process's store scope reaches it, and identity class.
const PREFIX: &str = "\0notes.";
const ADDR_CONTEXT: &str = "ferros 2026-11-12 note address";

struct Kept {
    note: Note,
    seq: u64,
}

/// The notes waiting, oldest first. Under the kernel lock.
static mut NOTES: Vec<Kept> = Vec::new();
/// Each source's posts since the person last looked at it (`seen`). Under the kernel lock.
static mut BUDGET: Vec<([u8; 32], u16)> = Vec::new();
/// The note the person last opened, per source (`seen` with `opened`): its document, kept in RAM for its kist to read (op 2) until the next one opened replaces it.
static mut OPENED: Vec<([u8; 32], u8, Vec<u8>)> = Vec::new();
/// Whether `NOTES` holds the person's (loaded from the vault this unlock).
static mut LOADED: bool = false;

fn notes() -> &'static mut Vec<Kept> {
    unsafe { &mut *(&raw mut NOTES) }
}

fn addr(source: &[u8; 32], number: u8) -> [u8; 32] {
    *blake3::Hasher::new_derive_key(ADDR_CONTEXT).update(source).update(&[number]).finalize().as_bytes()
}

/// A note kept in the vault (`doc`) or gone from it (None). A vault that refuses says so on the console: the note stands in RAM, unkept.
/// A system question (source `note::SYSTEM`) is never written: it lasts as long as what asks it (an offer, an open sheet, a device asking), none of which outlives the map or the device restarting, and the map posts again whatever still waits (docs/NOTIFY.md "System questions"). Its removal still goes to the vault, for one a kernel from before kept.
fn keep(source: &[u8; 32], number: u8, doc: Option<&[u8]>) {
    if *source == note::SYSTEM && doc.is_some() {
        return;
    }
    let Some(store) = (unsafe { STORE_ADDR }) else { return };
    let a = addr(source, number);
    let ok = match doc {
        Some(d) => store(PREFIX, &a, AddrOp::Set(d)).is_ok(),
        None => store(PREFIX, &a, AddrOp::Remove).is_ok(),
    };
    if !ok {
        crate::flashlog::line("notify: the vault did not take a note's change");
    }
}

/// The notes' bell rung: the deck reads the list again.
fn ring() -> u64 {
    bell_count(BELL_NOTES);
    BELL_COUNT[BELL_NOTES].load(core::sync::atomic::Ordering::Acquire)
}

/// The person's notes in RAM while one is present, loaded once an unlock; nothing while nobody is (wiped, with what each kist had opened and its budget). Then every note past its deadline dropped. Under the kernel lock.
fn ensure() {
    let present = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire);
    unsafe {
        if !present {
            if LOADED || !notes().is_empty() {
                notes().clear();
                (*(&raw mut BUDGET)).clear();
                (*(&raw mut OPENED)).clear();
                LOADED = false;
                ring();
            }
            return;
        }
        if !LOADED {
            LOADED = true;
            load();
        }
    }
    expire();
}

/// Every note kept for the person, read from the vault in the order they were posted.
fn load() {
    let Some(store) = (unsafe { STORE_ADDR }) else { return };
    let Ok(Some(list)) = store(PREFIX, &[0; 32], AddrOp::List) else { return };
    let mut got: Vec<Note> = Vec::new();
    for c in list.chunks_exact(32) {
        let a: [u8; 32] = c.try_into().unwrap();
        // A record that does not read as a note (a vault from a later kernel, a torn write) is not shown; it stays until the person's next look at its kist removes it by its address.
        if let Ok(Some(doc)) = store(PREFIX, &a, AddrOp::Get) {
            if let Some(n) = note::parse(&doc).filter(|n| n.source.is_some() && n.number.is_some_and(|k| (k as usize) < WAITING)) {
                // A system question a kernel from before kept: nothing asks it now (`keep`), so it goes.
                if n.source == Some(note::SYSTEM) {
                    let _ = store(PREFIX, &a, AddrOp::Remove);
                    continue;
                }
                got.push(n);
            }
        }
    }
    got.sort_by_key(|n| n.posted.unwrap_or(0));
    for n in got {
        let seq = ring();
        notes().push(Kept { note: n, seq });
    }
    if !notes().is_empty() {
        crate::flashlog::line(&alloc::format!("notify: A#{} notes waiting on the person, from the vault", notes().len()));
    }
}

/// Notes past their deadline gone, from RAM and the vault, and the bell rung if any went.
fn expire() {
    let now = crate::settings::eagle_now();
    // Before the clock is read (0), no deadline has passed.
    if now == 0 {
        return;
    }
    let mut gone = false;
    notes().retain(|k| {
        let past = k.note.deadline.is_some_and(|d| d <= now);
        if past {
            keep(&k.note.source.unwrap(), k.note.number.unwrap(), None);
            gone = true;
        }
        !past
    });
    if gone {
        ring();
    }
}

/// Whether process `p` holds a send right on the endpoint named `name` (the network's, `udp`: a person's words reach a kist only over a link).
fn holds_endpoint(p: &Process, name: &[u8]) -> bool {
    let Some(e) = k().endpoints.iter().position(|e| e.name == name) else { return false };
    p.cspace.iter().any(|c| c.object == Object::Endpoint(e) && c.rights & RIGHT_SEND != 0)
}

/// What the system sees of a note's poster (ferros_kist::note `decide` lowers the kind by it): the system's own (`kist` None), its question (the system's, from `note::SYSTEM`: personal at most, never the screen), or a kist and whether it holds the network.
fn poster(source: &[u8; 32], kist: Option<&Process>) -> Poster {
    match kist {
        None if *source == note::SYSTEM => Poster::Question,
        None => Poster::System,
        Some(p) => Poster::Kist { network: holds_endpoint(p, b"udp") },
    }
}

/// Post `doc` for `source`: a kist's note (`kist` its process) or the system's (None). The number | the kind decided << 8; NOTHING when nobody is present or the kist is at a count; ERR for a document that is not a note.
fn post(source: [u8; 32], doc: &[u8], kist: Option<&Process>) -> u64 {
    if !crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) {
        return NOTHING;
    }
    let Some(mut n) = note::parse(doc) else { return ERR };
    let budget = unsafe { &mut *(&raw mut BUDGET) };
    let spent = budget.iter().find(|b| b.0 == source).map_or(0, |b| b.1);
    if spent >= POSTS {
        return NOTHING;
    }
    let asked = n.kind;
    n.kind = note::decide(asked, n.person.is_some(), poster(&source, kist));
    n.asked = Some(asked);
    n.source = Some(source);
    n.posted = Some(crate::settings::eagle_now());
    // A note with a key the kist has waiting already: that note revised, its number kept. Else the lowest number free.
    let list = notes();
    let same = n.key.and_then(|key| list.iter().position(|k| k.note.source == Some(source) && k.note.key == Some(key)));
    let number = match same {
        Some(i) => list[i].note.number.unwrap(),
        None => {
            let mut used = 0u16;
            for k in list.iter().filter(|k| k.note.source == Some(source)) {
                used |= 1 << k.note.number.unwrap();
            }
            // Every number in use: the kist has its count of notes waiting.
            match note::free_number(used) {
                Some(n) => n,
                None => return NOTHING,
            }
        }
    };
    n.number = Some(number);
    let Some(built) = note::build(&n, n.posted.unwrap()) else { return ERR };
    if let Some(i) = same {
        list.remove(i);
    }
    match budget.iter_mut().find(|b| b.0 == source) {
        Some(b) => b.1 += 1,
        None => budget.push((source, 1)),
    }
    keep(&source, number, Some(&built));
    let kind = n.kind;
    let seq = ring();
    list.push(Kept { note: n, seq });
    number as u64 | (kind as u64) << 8
}

/// `source`'s note `number` gone (G#FF: all of its), from RAM and the vault: a kist withdrawing its own, or the person having seen it (`seen`). How many went.
fn drop_notes(source: &[u8; 32], number: u8) -> u64 {
    let mut n = 0;
    notes().retain(|k| {
        let hit = k.note.source.as_ref() == Some(source) && (number == 0xFF || k.note.number == Some(number));
        if hit {
            keep(source, k.note.number.unwrap(), None);
            n += 1;
        }
        !hit
    });
    if n > 0 {
        ring();
    }
    n
}

/// The person looked at `source` (its circle opened, its window brought to the front: docs/NOTIFY.md "Seen"): its notes cleared (`number`, or G#FF for all), its budget back. `opened`: the person tapped note `number` itself, kept for its kist to read where to land (op 2).
fn seen(source: &[u8; 32], number: u8, opened: bool) -> u64 {
    if opened {
        if let Some(k) = notes().iter().find(|k| k.note.source.as_ref() == Some(source) && k.note.number == Some(number)) {
            if let Some(doc) = note::build(&k.note, k.note.posted.unwrap_or(0)) {
                let o = unsafe { &mut *(&raw mut OPENED) };
                o.retain(|x| x.0 != *source);
                o.push((*source, number, doc));
            }
        }
    }
    unsafe { (*(&raw mut BUDGET)).retain(|b| b.0 != *source) };
    drop_notes(source, number)
}

/// The notes as op 3's records, as many whole ones as fit in `room` bytes.
fn records(room: usize) -> Vec<u8> {
    let list = notes();
    // WHY the bound: the buffer is the caller's; whole records only, and the count says how many there are.
    let fit = (room / REC).min(list.len());
    let mut out = Vec::with_capacity(fit * REC);
    for k in &list[..fit] {
        let n = &k.note;
        out.extend_from_slice(&n.source.unwrap());
        out.push(n.number.unwrap());
        out.push(n.kind);
        out.push(n.asked.unwrap_or(n.kind));
        out.push(n.person.is_some() as u8 | (n.target.is_some() as u8) << 1);
        out.extend_from_slice(&n.count.to_le_bytes());
        out.extend_from_slice(&k.seq.to_le_bytes());
        out.extend_from_slice(&n.deadline.unwrap_or(0).to_le_bytes());
    }
    out
}

/// SYS_NOTIFY (`SYS_NOTIFY`'s doc has the ops). Under the kernel lock, in the caller's trap.
pub(super) fn syscall(f: &mut Frame, p: &Process, pi: usize) {
    ensure();
    let settings = |at: u64| {
        let token = p.read(at, 32);
        !p.kist && matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0)
    };
    // The caller's own source: a kist by its capsule as launched.
    let own = p.capsule.filter(|_| p.kist);
    let source_at = |at: u64| -> [u8; 32] { p.read(at, 32).try_into().unwrap() };
    f.x[0] = match f.x[0] {
        // Post: x1/x2 the note; a kist's own, or (x3 a Settings token, x4 a 32-byte source) the system's on a kist's behalf (the deck's development notes, NOTIFY.md "Seeing it").
        0 => {
            // WHY the bound: the length is the caller's; past note::MAX it is no note, and the kernel never copies it in.
            if f.x[2] as usize > note::MAX {
                ERR
            } else {
                let doc = p.read(f.x[1], f.x[2] as usize);
                match own {
                    Some(src) => post(src, &doc, Some(p)),
                    None if settings(f.x[3]) => post(source_at(f.x[4]), &doc, None),
                    None => ERR,
                }
            }
        }
        // Withdraw: x1 the number (G#FF all): a kist's own; the system's with x2 a Settings token and x3 the source.
        1 => match own {
            Some(src) => drop_notes(&src, f.x[1] as u8),
            None if settings(f.x[2]) => drop_notes(&source_at(f.x[3]), f.x[1] as u8),
            None => ERR,
        },
        // The note the person opened (a kist's: the display's EV_OPEN_AT names its number): x1 the number, x2/x3 a buffer → the document's length, written whole if it fits; NOTHING when the note opened last is not that one.
        2 => match own {
            Some(src) => match unsafe { (*(&raw const OPENED)).iter().find(|o| o.0 == src && o.1 as u64 == f.x[1]) } {
                Some((_, _, doc)) => {
                    if doc.len() <= f.x[3] as usize {
                        p.write(f.x[2], doc);
                    }
                    doc.len() as u64
                }
                None => NOTHING,
            },
            None => ERR,
        },
        // The list, for the deck: x1 a Settings token, x2/x3 a buffer → how many notes (whole records past the buffer are not written).
        3 if settings(f.x[1]) => {
            p.write(f.x[2], &records(f.x[3] as usize));
            notes().len() as u64
        }
        // The notes' doorbell: x1 a word, x2 a Settings token.
        4 if settings(f.x[2]) => bell_register(p, pi, BELL_NOTES, f.x[1], BELL_COUNT[BELL_NOTES].load(core::sync::atomic::Ordering::Acquire)),
        // Seen: x1 a Settings token, x2 the source (32 bytes), x3 the number (G#FF all), x4 1 when the person opened that note → how many went.
        5 if settings(f.x[1]) => seen(&source_at(f.x[2]), f.x[3] as u8, f.x[4] == 1),
        // One note's document, for the deck (a live note's words): x1 a Settings token, x2 [source 32][number u8], x3/x4 a buffer → its length, written whole if it fits; NOTHING for no such note.
        6 if settings(f.x[1]) => {
            let req = p.read(f.x[2], 33);
            match notes().iter().find(|k| k.note.source.as_ref().is_some_and(|s| s[..] == req[..32]) && k.note.number == Some(req[32])) {
                Some(k) => match note::build(&k.note, k.note.posted.unwrap_or(0)) {
                    Some(doc) => {
                        if doc.len() <= f.x[4] as usize {
                            p.write(f.x[3], &doc);
                        }
                        doc.len() as u64
                    }
                    None => ERR,
                },
                None => NOTHING,
            }
        }
        _ => ERR,
    };
}

/// The development command's post (`diag "notify …"`, service.rs): a note from the system on behalf of `source`, of `kind`, with `words`. The number | kind << 8, or NOTHING / ERR as op 0. Takes the kernel lock.
pub fn post_system(source: [u8; 32], kind: u8, person: bool, words: &str) -> u64 {
    with_lock(|| {
        ensure();
        let n = Note { kind, person: person.then_some([1; 32]), words: Some(alloc::string::String::from(words)), count: 1, ..Default::default() };
        match note::build(&n, crate::settings::eagle_now()) {
            Some(doc) => post(source, &doc, None),
            None => ERR,
        }
    })
}

/// `diag "notify"`: the notes waiting and the kists alive (by name and capsule's first bytes, to name one in a post). Takes the kernel lock.
pub fn report() -> alloc::string::String {
    with_lock(|| {
        ensure();
        let mut s = alloc::format!("notify: A#{} notes waiting\n", notes().len());
        for k in notes().iter() {
            let n = &k.note;
            let src = n.source.unwrap();
            s.push_str(&alloc::format!("  G#{:02X}{:02X}{:02X}{:02X} note A#{} {} (asked {}) count A#{} seq A#{}{}\n", src[0], src[1], src[2], src[3], n.number.unwrap(), kind_name(n.kind), kind_name(n.asked.unwrap_or(n.kind)), n.count, k.seq, if n.deadline.is_some() { ", a deadline" } else { "" }));
        }
        for q in k().procs.iter().flatten().filter(|q| q.kist && !q.dying) {
            if let Some(c) = q.capsule {
                s.push_str(&alloc::format!("  kist {} G#{:02X}{:02X}{:02X}{:02X}\n", core::str::from_utf8(&q.name).unwrap_or("?"), c[0], c[1], c[2], c[3]));
            }
        }
        s
    })
}

/// A kind's name for the console.
pub fn kind_name(kind: u8) -> &'static str {
    match kind {
        STATUS => "status",
        PERSONAL => "personal",
        LIVE => "live",
        _ => "?",
    }
}

/// The capsule `diag "notify … <kist>"` names: a live kist by its process name, else a capsule address in hex, whole (64 digits) or its first bytes as the kernel knows them (a live kist, a kist the kernel keeps a record for, a note's source). Takes the kernel lock.
pub fn kist_named(name: &[u8]) -> Option<[u8; 32]> {
    with_lock(|| {
        if let Some(c) = k().procs.iter().flatten().find(|q| q.kist && !q.dying && q.name == name).and_then(|q| q.capsule) {
            return Some(c);
        }
        let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        // WHY the bounds: the text is the bridge's; an odd count of digits or more than 64 names no address.
        if name.is_empty() || name.len() % 2 != 0 || name.len() > 64 {
            return None;
        }
        let mut want = [0u8; 32];
        for (i, pair) in name.chunks_exact(2).enumerate() {
            want[i] = digit(pair[0])? << 4 | digit(pair[1])?;
        }
        let n = name.len() / 2;
        if n == 32 {
            return Some(want);
        }
        let known = k().procs.iter().flatten().filter(|q| q.kist).filter_map(|q| q.capsule).chain(unsafe { (*(&raw const NOT_STARTED)).iter().map(|r| r.0) }).chain(notes().iter().filter_map(|x| x.note.source));
        let mut hit = None;
        for c in known {
            if c[..n] == want[..n] {
                hit = Some(c);
            }
        }
        hit
    })
}

/// `diag "notify clear <kist>"`: every note of that kist gone, as if the person had looked. How many went. Takes the kernel lock.
pub fn clear(source: [u8; 32]) -> u64 {
    with_lock(|| {
        ensure();
        seen(&source, 0xFF, false)
    })
}
