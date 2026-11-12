//! The installer as the map's Deck page knows it (payloads/kists, its `kists` endpoint; docs/KISTS.md "Install"): the directory's kists, and each install, update, uninstall and restore under way with its stage and bytes. Read on a thread of its own each time the installer's doorbell rings (op 0), never on a timer; the main loop takes a copy (`take`).

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use ferros_sys::{cap, freq, now, sleep_until, ERR};

use crate::{tr, Line, S_INSTALLER};
use ferros_lang::Msg;

/// A job's stages and why one failed, as the installer counts them (payloads/kists `STAGE_*`, `WHY_*`).
pub const QUEUED: u8 = 0;
pub const FETCHING: u8 = 1;
pub const SIGNATURE: u8 = 2;
pub const CODE: u8 = 3;
pub const SEALING: u8 = 4;
pub const STORING: u8 = 5;
pub const DONE: u8 = 6;
pub const FAILED: u8 = 7;
/// An offer to stow, checked and waiting on the person's Stow or Cancel (payloads/kists `STAGE_ASKING`): neither under way nor finished.
pub const ASKING: u8 = 8;
pub const KIND_INSTALL: u8 = 0;
pub const KIND_UNINSTALL: u8 = 1;
/// A kist offered over USB to stow (docs/KISTS.md "Stowing").
pub const KIND_OFFER: u8 = 3;

/// A kist offered over USB, checked and waiting on the person (the installer's op 6): what the Kists page shows above everything, with Stow and Cancel.
#[derive(Clone)]
pub struct Offer {
    pub capsule: [u8; 32],
    pub size: u64,
    /// Native, unchecked machine code: a development build stows it, and the page says so plainly.
    pub native: bool,
    pub developer: [u8; 32],
    pub version: ferros_kist::Version,
    pub name: String,
    /// The developer's name as the capsule gives it (empty when it gives none: the page shows the key's first bytes).
    pub developer_name: String,
    /// Each right it declares and the developer's reason, in its slot order.
    pub wants: Vec<(u8, String)>,
}

/// One install, update, uninstall or restore.
#[derive(Clone)]
pub struct Job {
    pub kind: u8,
    pub stage: u8,
    pub why: u8,
    pub capsule: [u8; 32],
    pub got: u64,
    pub total: u64,
    pub name: String,
    /// For a kist refused as signed by another key than its developer's other kists (why 17): that developer, as the person's pin names them.
    pub who: String,
}

/// One kist the directory lists.
#[derive(Clone)]
pub struct Row {
    pub capsule: [u8; 32],
    pub developer: [u8; 32],
    pub option: [u8; 32],
    pub size: u64,
    /// As its capsule's signed declaration writes it (leading zeros and all): shown and compared by its significant parts (ferros_kist `Version`, `write_version`).
    pub version: ferros_kist::Version,
    pub name: String,
    pub about: String,
}

/// The installer's state: whether it answered at all, the directory (0 not read, 1 reading, 2 read, 3 not reachable; its signature checked or not), its rows and the jobs.
#[derive(Clone, Default)]
pub struct Installer {
    pub here: bool,
    pub dir_state: u8,
    pub dir_checked: bool,
    /// The installer's directory generation (op 0's last word): the rows are read again whenever it moves.
    pub dir_gen: u32,
    pub dir: Vec<Row>,
    pub jobs: Vec<Job>,
    /// The offer waiting on the person, if one is.
    pub offer: Option<Offer>,
}

impl Installer {
    /// The latest job for a capsule.
    pub fn job_for(&self, capsule: &[u8; 32]) -> Option<&Job> {
        self.jobs.iter().rev().find(|j| j.capsule == *capsule)
    }

    /// A word for the finished jobs: it moves when one more finishes, so the map reads the person's list and its circles again.
    pub fn finished(&self) -> u64 {
        self.jobs.iter().filter(|j| j.stage == DONE).fold(0u64, |a, j| a.wrapping_mul(31).wrapping_add(u64::from_le_bytes(j.capsule[..8].try_into().unwrap()) ^ j.kind as u64))
    }
}

struct Snap(UnsafeCell<Option<Installer>>, AtomicBool);
unsafe impl Sync for Snap {}
static SNAP: Snap = Snap(UnsafeCell::new(None), AtomicBool::new(false));
/// Moved on each time the watcher publishes.
static SEQ: AtomicU32 = AtomicU32::new(0);

fn with<R>(f: impl FnOnce(&mut Option<Installer>) -> R) -> R {
    while SNAP.1.swap(true, Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let r = f(unsafe { &mut *SNAP.0.get() });
    SNAP.1.store(false, Ordering::Release);
    r
}

/// The copy the watcher last published, when it moved from `seen`.
pub fn take(seen: u32) -> Option<(u32, Installer)> {
    let s = SEQ.load(Ordering::Acquire);
    if s == seen {
        return None;
    }
    with(|x| x.clone()).map(|i| (s, i))
}

/// An op to the installer, its one-byte answer (None when it is not serving: it starts once a person has attested).
pub fn ask(grants: u64, msg: &[u8]) -> Option<u8> {
    let mut r = [0u8; 4];
    let n = ferros_sys::call_live(cap(grants, S_INSTALLER), msg, &mut r);
    (n != ERR && n >= 1).then_some(r[0])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Op 0's status read into `into` (the directory's rows kept as they were). False when it does not parse.
fn read_status(b: &[u8], into: &mut Installer) -> bool {
    if b.len() < 7 {
        return false;
    }
    into.dir_state = b[4];
    into.dir_checked = b[5] == 1;
    into.jobs.clear();
    let mut p = 7;
    for _ in 0..b[6] {
        // WHY: the bytes are the installer's; a record cut short ends the list.
        if p + 52 > b.len() {
            return false;
        }
        let r = &b[p..];
        let nl = r[51] as usize;
        if p + 52 + nl > b.len() {
            return false;
        }
        into.jobs.push(Job { kind: r[0], stage: r[1], why: r[2], capsule: r[3..35].try_into().unwrap(), got: u64_at(r, 35), total: u64_at(r, 43), name: String::from(core::str::from_utf8(&r[52..52 + nl]).unwrap_or("")), who: String::new() });
        p += 52 + nl;
    }
    // Then per job its `who` (an installer from before sends none: the names stay empty).
    for j in into.jobs.iter_mut() {
        // WHY: the bytes are the installer's; a record cut short ends the names.
        if p >= b.len() || p + 1 + b[p] as usize > b.len() {
            break;
        }
        let n = b[p] as usize;
        j.who = String::from(core::str::from_utf8(&b[p + 1..p + 1 + n]).unwrap_or(""));
        p += 1 + n;
    }
    // Last, the directory's generation (an installer from before sends none: it stays as it was).
    // WHY: the bytes are the installer's; four must be there to read.
    if p + 4 <= b.len() {
        into.dir_gen = u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
    }
    true
}

/// The directory's rows, op 2 a reply at a time from the first.
fn read_directory(svc: u64) -> Option<Vec<Row>> {
    let mut rows = Vec::new();
    let mut out = [0u8; 4096];
    loop {
        // WHY: one byte counts the rows asked from; a directory past 255 is shown to 255.
        let from = rows.len().min(255) as u8;
        let n = ferros_sys::call_live(svc, &[2, from], &mut out);
        if n == ERR || n < 2 {
            return None;
        }
        // WHY: the length is the installer's, at most what the buffer took.
        let b = &out[..(n as usize).min(out.len())];
        let (all, got) = (b[0] as usize, b[1] as usize);
        let mut p = 2;
        for _ in 0..got {
            // WHY (each length check here): the bytes are the installer's; a record cut short ends the read rather than indexing past the reply.
            // The fixed head: capsule, developer, option, size, the version's part count.
            if p + 32 * 3 + 8 + 1 > b.len() {
                return None;
            }
            let r = &b[p..];
            let parts = r[104] as usize;
            // The parts (u64 LE each), then the name's length.
            let v_end = 105 + (parts << 3);
            if p + v_end + 1 > b.len() {
                return None;
            }
            let version = ferros_kist::Version((0..parts).map(|k| u64_at(r, 105 + (k << 3))).collect());
            let nl = r[v_end] as usize;
            if p + v_end + 1 + nl + 1 > b.len() {
                return None;
            }
            let al = r[v_end + 1 + nl] as usize;
            if p + v_end + 2 + nl + al > b.len() {
                return None;
            }
            rows.push(Row {
                capsule: r[..32].try_into().unwrap(),
                developer: r[32..64].try_into().unwrap(),
                option: r[64..96].try_into().unwrap(),
                size: u64_at(r, 96),
                version,
                name: String::from(core::str::from_utf8(&r[v_end + 1..v_end + 1 + nl]).unwrap_or("")),
                about: String::from(core::str::from_utf8(&r[v_end + 2 + nl..v_end + 2 + nl + al]).unwrap_or("")),
            });
            p += v_end + 2 + nl + al;
        }
        if got == 0 || rows.len() >= all {
            return Some(rows);
        }
    }
}

/// An offer waiting on the person, as the installer's op 6 gives it; None when that capsule is no longer one (answered meanwhile) or the reply does not parse.
fn read_offer(svc: u64, capsule: &[u8; 32]) -> Option<Offer> {
    let mut m = [0u8; 33];
    m[0] = 6;
    m[1..].copy_from_slice(capsule);
    let mut out = [0u8; 4096];
    let n = ferros_sys::call_live(svc, &m, &mut out);
    if n == ERR {
        return None;
    }
    // WHY (each length check here): the bytes are the installer's; a reply cut short is no offer rather than an index past it.
    let b = &out[..(n as usize).min(out.len())];
    if b.len() < 43 || b[0] != 0 {
        return None;
    }
    let size = u64_at(b, 1);
    let native = b[9] & 1 == 1;
    let developer: [u8; 32] = b[10..42].try_into().unwrap();
    let parts = b[42] as usize;
    let mut p = 43 + (parts << 3);
    if p + 1 > b.len() {
        return None;
    }
    let version = ferros_kist::Version((0..parts).map(|k| u64_at(b, 43 + (k << 3))).collect());
    let text = |p: &mut usize| -> Option<String> {
        let l = *b.get(*p)? as usize;
        let t = b.get(*p + 1..*p + 1 + l)?;
        *p += 1 + l;
        Some(String::from(core::str::from_utf8(t).unwrap_or("")))
    };
    let name = text(&mut p)?;
    let developer_name = text(&mut p)?;
    let n = *b.get(p)? as usize;
    p += 1;
    let mut wants = Vec::with_capacity(n);
    for _ in 0..n {
        let right = *b.get(p)?;
        p += 1;
        wants.push((right, text(&mut p)?));
    }
    Some(Offer { capsule: *capsule, size, native, developer, version, name, developer_name, wants })
}

/// The installer's doorbell, held on a thread of its own: answered with its status whenever anything the page shows changes, the directory's rows read again each time it has been read afresh. While the installer is not serving (the kernel starts it once a person has attested) it is asked again after a wait that doubles to a second, as `fgtw_poller` asks fgtw: no doorbell exists for a server coming up.
pub extern "C" fn watch(grants: u64) -> ! {
    let svc = cap(grants, S_INSTALLER);
    let mut count = u32::MAX;
    let mut absent = freq() >> 6;
    let mut snap = Installer::default();
    // The directory generation the rows were last read at.
    let mut rows_gen: Option<u32> = None;
    let mut out = [0u8; 4096];
    loop {
        let mut m = [0u8; 5];
        m[1..].copy_from_slice(&count.to_le_bytes());
        let n = ferros_sys::call_live(svc, &m, &mut out);
        if n == ERR || n < 7 {
            sleep_until(now() + absent);
            if absent < freq() {
                absent <<= 1;
            }
            continue;
        }
        absent = freq() >> 6;
        let was = snap.dir_state;
        // WHY: the length is the installer's, at most what the buffer took.
        if !read_status(&out[..(n as usize).min(out.len())], &mut snap) {
            continue;
        }
        count = u32::from_le_bytes(out[..4].try_into().unwrap());
        snap.here = true;
        // The rows read again when the directory was read afresh: on the way into "read", or when its generation moved (a read again between two doorbells, which once left the page the rows it had), or when the last read of them failed.
        if snap.dir_state == 2 && (was != 2 || rows_gen != Some(snap.dir_gen)) {
            if let Some(rows) = read_directory(svc) {
                snap.dir = rows;
                rows_gen = Some(snap.dir_gen);
            }
        }
        // An offer to stow that has reached the person (checked, waiting on their answer): what it is read once, when it comes; gone from the page as soon as it is answered or ended.
        let asking = snap.jobs.iter().find(|j| j.kind == KIND_OFFER && j.stage == ASKING).map(|j| j.capsule);
        snap.offer = match asking {
            Some(c) if snap.offer.as_ref().is_some_and(|o| o.capsule == c) => snap.offer.take(),
            Some(c) => {
                let o = read_offer(svc, &c);
                // One line as each offer reaches the map (or does not), so a run shows whether the page was ever told.
                let mut l = Line::new();
                let _ = match o.as_ref() {
                    Some(o) => write!(l, "map: a kist offered over USB waits on the person: {}\n", o.name),
                    None => write!(l, "map: the installer has a kist offered over USB waiting, but its offer did not read\n"),
                };
                ferros_sys::console(grants, l.as_str().as_bytes());
                o
            }
            None => None,
        };
        with(|x| *x = Some(snap.clone()));
        SEQ.fetch_add(1, Ordering::AcqRel);
    }
}

/// Why a job failed, in the person's words: ferros_kist's table, which `ferros send` prints too.
fn why(w: u8) -> &'static str {
    ferros_kist::offer::words(w)
}

/// A job's stage in words: what it is doing now, with its bytes while it fetches (in the person's number base).
pub fn words(j: &Job, f: tatau::Fmt) -> Line {
    let mut l = Line::new();
    let size = |l: &mut Line, bytes: u64| {
        let _ = tatau::size_bits(l, f, bytes << 3, 1);
    };
    match j.stage {
        QUEUED => {
            l.s("Waiting its turn");
        }
        FETCHING => {
            let _ = write!(l, "Fetching {}: ", j.name);
            size(&mut l, j.got);
            if j.total != 0 {
                l.s(" of ");
                size(&mut l, j.total);
            }
        }
        SIGNATURE => {
            l.s("Checking its signature");
        }
        CODE => {
            l.s("Checking its code");
        }
        SEALING => {
            l.s("Sealing it to this device");
        }
        STORING => {
            l.s("Storing it");
        }
        ASKING => {
            l.s(tr(Msg::OfferedOverUsb));
        }
        DONE => {
            l.s(match j.kind {
                KIND_INSTALL => "Installed",
                KIND_UNINSTALL => "Uninstalled",
                KIND_OFFER => tr(Msg::Stowed),
                _ => "Restored",
            });
        }
        _ => {
            let not = match j.kind {
                KIND_INSTALL => "Not installed",
                KIND_UNINSTALL => "Not uninstalled",
                KIND_OFFER => tr(Msg::NotStowed),
                _ => "Not restored",
            };
            let _ = if j.why == 17 && !j.who.is_empty() {
                write!(l, "{not}: this kist is signed by a different key than {}'s other kists", j.who)
            } else {
                write!(l, "{not}: {}", why(j.why))
            };
        }
    }
    l
}

/// How far a job under way is, 0 to 1, for its bar: its bytes while it fetches and while the kernel reads it back to check it, whole after.
pub fn share(j: &Job) -> f64 {
    match j.stage {
        FETCHING | CODE if j.total != 0 => j.got as f64 / j.total as f64,
        QUEUED | FETCHING => 0.,
        _ => 1.,
    }
}
