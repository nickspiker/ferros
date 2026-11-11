//! The installer (docs/KISTS.md "Install"; owner 2026-11-10: "complete the on device compile and seal", "ship the first kist proper"): it installs, updates and uninstalls the person's kists, and brings back the capsules a restored list names. Never the map (KISTS.md: the map holds none of a kist's rights): the map's Kists page asks it, on the `kists` endpoint.
//!
//! An install is SANDBOX.md's lifecycle with nothing skipped, each stage told to the page:
//! 0. **fetching**: the capsule over plain HTTP (net.rs), straight into the vault in pieces of one transfer (`capsules.<blake3>`, the device's), hashed as it passes (ferros_kist::fetch::Intake), never held whole; the last piece is held back, so nothing is kept before its signature is checked;
//! 1. **checking its signature**: its blake3 and size the directory's, its developer and option the entry's, and every egg (Ed25519, Falcon-512, SPHINCS+) against the developer bundle the capsule carries (`kb`), its Ed25519 key the kist's developer (fgtw::pq; owner 2026-11-10: "no single eggs"); that bundle's commitment the one the person's list pins for that developer, or none pinned yet (trust on first install: ferros_kist `capsule::admit`); then the last piece commits it;
//! 2. **checking its code**: the kernel reads it back from the vault, gates it where it lies and runs the sandbox checker over its masked code, once (`ferros_sys::install_kist`); native code is a development build and is refused ("this kist is a development build and can't be installed from the network");
//! 3. **sealing it to this device**: the kernel's keyed seal, kept in the vault's `sealed` list, the only thing a launch checks;
//! 4. **storing**: the kist in the person's list (`kists.list`), with where it came from (`source`) and when (`installed`), the answers it had (or kept from an uninstall) standing, and its developer's bundle pinned there at their first kist (`Kists::trust`; an uninstall keeps the pin);
//! 5. **done**, or the failure and why (the capsule's bytes dropped from the vault on any failure).
//!
//! An uninstall takes the kist out of the person's list, its answers kept (ferros_kist `Kists::remove`: asked once, ever), and drops its capsule; the identity service carries the removal to the person's other devices. A restore (the person's list came back from their fleet after a reset, payloads/fgtw space.rs) fetches each capsule the list names that this device lacks, from its recorded source then the directory, through the same stages; the fleet's own devices first is the owner's order, and a seam (`fleet_fetch`) until that protocol exists.
//!
//! Nothing here asks on a timer (owner, 2026-11-10: "No polling!"): the page holds a doorbell (op 0) answered when anything it shows changes; the worker sleeps on a word until a job comes; a restore follows the identity service's own doorbell (fgtw op 21); the kernel tells each stage of its check on a word (`wait_world`). The one timer is net.rs's: ip has no held receive yet.
//!
//! Started by the kernel once a person has attested, with its own grants (service.rs `kists_grants`; init's page is full): 0 console, 1 use udp, 2 the vault scope `capsules.`, 3 the vault scope `kists.`, 4 serve `kists`, 5 use fgtw, 6 the Install right.
//!
//! `kists` ops (the map's, first byte):
//! - 0 [count u32 LE]: the doorbell. Answered at once with the status when its count is not the caller's, else held until it changes: [count u32 LE][directory: 0 not read, 1 fetching, 2 read, 3 not reachable][1 the directory's signature checked, 0 not (the project key not pinned yet)][jobs n][per job: kind (0 install, 1 uninstall, 2 restore), stage (`STAGE_*`), why (`WHY_*`), the capsule 32, bytes so far u64 LE, bytes in all u64 LE, name length, name], then per job again: [who length, who] (the developer whose other kists a refused key differs from, `WHY_OTHER_KEY`; empty for any other job), then the directory's generation u32 LE (moved each time a directory is read: the page reads the rows again).
//! - 1: fetch the directory again and look for missing capsules again (the page opened) → [0].
//! - 2 [from]: the directory's entries from `from` → [entries in all][n][per entry: capsule 32, developer 32, option 32, size u64 LE, the version's part count u8 then each part u64 LE (as its capsule's signed declaration writes it, leading zeros and all; at most 255 parts), name length, name, about length, about], as many as fit one reply (4000 bytes: an entry is at most A#2275, so one always fits). Until 2026-11-10 the version was three u16 (major, minor, patch).
//! - 3 [capsule 32]: install (or update to) the directory's entry with that capsule → [0 queued, 1 not in the directory, 2 already under way, 3 too many under way].
//! - 4 [developer 32][option 32]: uninstall that kist → [0 queued, 1 not installed, 3 too many under way].

#![no_std]
#![no_main]

extern crate alloc;

mod net;

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use ferros_kist::directory::{Directory, Entry, Source};
use ferros_kist::fetch::{Body, Intake, Taken};
use ferros_sys::{cap, console, freq, now, sleep_until, ERR, NOTHING};

const S_UDP: u64 = 1;
const S_CAPSULES: u64 = 2;
const S_KISTS: u64 = 3;
const S_SERVE: u64 = 4;
const S_FGTW: u64 = 5;
const S_INSTALL: u64 = 6;

/// The heap: the directory, the person's list, a capsule's first bytes (64 KiB), the eggs' checks.
const HEAP_SIZE: usize = 4 << 20;
/// The worker's stack: SPHINCS+ and Falcon verification, the list parsed through vsf.
const WORK_STACK: usize = 1 << 19;
/// The space watcher's stack: two small calls.
const WATCH_STACK: usize = 1 << 14;
/// One vault transfer (ferros_sys::STORE_MAX): a capsule goes in pieces of this.
const PIECE: usize = ferros_sys::STORE_MAX;

#[repr(C, align(64))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_SIZE> = Region(UnsafeCell::new([0; HEAP_SIZE]));
static WORKER_STACK: Region<WORK_STACK> = Region(UnsafeCell::new([0; WORK_STACK]));
static WATCHER_STACK: Region<WATCH_STACK> = Region(UnsafeCell::new([0; WATCH_STACK]));
/// The piece being filled: in the arc (.bss), where a syscall's bytes land (never the heap's start, a DMA region in some processes; here the heap is in the arc too).
static PIECE_BUF: Region<PIECE> = Region(UnsafeCell::new([0; PIECE]));

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

/// The ferros project key's public bundle (`KeyBundle::to_bytes`), every scheme, that signs the directory (ferros_kist::directory::verify_directory). Not pinned yet (owner, 2026-11-10: "the project public bundle ... isn't pinned yet"): until it is, the directory is read unchecked and the page says so. A directory vouches for nothing either way: every capsule's own eggs are checked against the developer bundle it carries and the person's pin for that developer (below), so an unchecked directory can list nothing that installs unchecked. tools/kistcat publishes it UNSIGNED (`build --unsigned`) until the project key is chosen.
const PROJECT_BUNDLE: &[u8] = &[];
// No list of developers is built into the OS (owner, 2026-11-10: trust on first install): a kist carries its developer's whole public bundle (`kb`), its eggs are checked against that, and the person's list pins the bundle's commitment for that developer at their first kist (ferros_kist `capsule::admit`, `Kists::trust`). Photon's release key stays trusted as fgtw pins it.

/// The kinds of job.
const KIND_INSTALL: u8 = 0;
const KIND_UNINSTALL: u8 = 1;
const KIND_RESTORE: u8 = 2;

/// A job's stages, as the page shows them.
const STAGE_QUEUED: u8 = 0;
const STAGE_FETCHING: u8 = 1;
const STAGE_SIGNATURE: u8 = 2;
const STAGE_CODE: u8 = 3;
const STAGE_SEALING: u8 = 4;
const STAGE_STORING: u8 = 5;
const STAGE_DONE: u8 = 6;
const STAGE_FAILED: u8 = 7;

/// Why a job failed (the page words each).
const WHY_NETWORK: u8 = 1;
const WHY_MISSING: u8 = 2;
const WHY_SIZE: u8 = 3;
const WHY_NOT_NAMED: u8 = 4;
const WHY_EGGS: u8 = 5;
/// It carries no bundle of its developer's keys (a capsule from before 2026-11-10).
const WHY_NO_BUNDLE: u8 = 6;
const WHY_NATIVE: u8 = 7;
const WHY_CHECKER: u8 = 8;
const WHY_SEAL: u8 = 9;
const WHY_VAULT: u8 = 10;
const WHY_BROKEN: u8 = 11;
const WHY_MANIFEST: u8 = 12;
const WHY_NOT_LISTED: u8 = 13;
const WHY_NOBODY: u8 = 14;
const WHY_ROOM: u8 = 15;
const WHY_NOT_KIST: u8 = 16;
/// The person trusted another bundle for this developer key at its first kist (the job's `who` names the developer).
const WHY_OTHER_KEY: u8 = 17;
/// The bundle it carries is not its developer's (its Ed25519 key is not the kist's).
const WHY_NOT_ITS_KEYS: u8 = 18;

/// The most jobs the page lists (finished ones give way first): one reply holds them, and the map holds sixteen circles.
const JOBS_MAX: usize = 1 << 4;
/// The most doorbells held (fgtw's op 21 holds eight): one map, with room.
const WATCHERS_MAX: usize = 1 << 3;

#[derive(Clone)]
struct Job {
    kind: u8,
    capsule: [u8; 32],
    developer: [u8; 32],
    option: [u8; 32],
    name: String,
    stage: u8,
    why: u8,
    got: u64,
    total: u64,
    /// For `WHY_OTHER_KEY`: the developer the person trusted, as their pin names them (ferros_kist `capsule::pin_name`); empty otherwise.
    who: String,
}

struct State {
    /// The status's change count: the doorbell's.
    count: u32,
    watchers: Vec<[u8; 32]>,
    /// 0 not read, 1 fetching, 2 read, 3 not reachable.
    dir_state: u8,
    dir_checked: bool,
    dir: Option<Directory>,
    /// Moved on each time a directory is read: the page reads the rows again when it moves (a read that went 2 → 1 → 2 between two of its doorbells no longer leaves it the rows it had).
    dir_gen: u32,
    dir_wanted: bool,
    restore_wanted: bool,
    jobs: Vec<Job>,
}

/// The state the server, the worker and the space watcher share, behind a spin lock (held for a few copies at a time; nothing that waits is done under it).
struct Shared(UnsafeCell<Option<State>>, AtomicBool);
unsafe impl Sync for Shared {}
static STATE: Shared = Shared(UnsafeCell::new(None), AtomicBool::new(false));

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    while STATE.1.swap(true, Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let r = f(unsafe { (*STATE.0.get()).as_mut().unwrap() });
    STATE.1.store(false, Ordering::Release);
    r
}

/// The worker's doorbell: moved on whenever there is something for it.
static WORK: AtomicU64 = AtomicU64::new(0);
/// The kernel's word for the install it is checking (`ferros_sys::install_kist`).
static KERNEL_WORD: AtomicU64 = AtomicU64::new(u64::MAX);
/// The grants page, for the threads.
static GRANTS: AtomicU64 = AtomicU64::new(0);

fn grants() -> u64 {
    GRANTS.load(Ordering::Relaxed)
}

fn say(t: &[u8]) {
    console(grants(), t);
}

fn kick() {
    WORK.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&WORK, 1);
}

/// The status, as op 0 answers it.
fn status(s: &State) -> Vec<u8> {
    let mut r = Vec::with_capacity(8 + s.jobs.len() * 96);
    r.extend_from_slice(&s.count.to_le_bytes());
    r.push(s.dir_state);
    r.push(s.dir_checked as u8);
    r.push(s.jobs.len() as u8);
    for j in &s.jobs {
        r.extend_from_slice(&[j.kind, j.stage, j.why]);
        r.extend_from_slice(&j.capsule);
        r.extend_from_slice(&j.got.to_le_bytes());
        r.extend_from_slice(&j.total.to_le_bytes());
        // WHY: a name is at most 32 bytes here (the page's row), a length byte.
        let n = &j.name.as_bytes()[..j.name.len().min(32)];
        r.push(n.len() as u8);
        r.extend_from_slice(n);
    }
    // After the jobs (a page from before reads to the last job and stops): per job, whose other kists a refused one's key differs from (`WHY_OTHER_KEY`), its length then its bytes, empty for any other.
    for j in &s.jobs {
        // WHY: a developer's name is at most 32 bytes here (the page's line), a length byte.
        let w = &j.who.as_bytes()[..j.who.len().min(32)];
        r.push(w.len() as u8);
        r.extend_from_slice(w);
    }
    // Last, the directory's generation (u32 LE): the page reads the rows again whenever it moves.
    r.extend_from_slice(&s.dir_gen.to_le_bytes());
    r
}

fn reply(token: &[u8; 32], msg: &[u8]) {
    ferros_sys::reply(token, msg);
}

/// The status changed: its count moves and every held doorbell is answered.
fn ring() {
    let (msg, tokens) = with(|s| {
        s.count = s.count.wrapping_add(1);
        (status(s), core::mem::take(&mut s.watchers))
    });
    for t in &tokens {
        reply(t, &msg);
    }
}

/// A job's stage (and its bytes) changed.
fn stage(job: usize, st: u8, got: u64, total: u64) {
    with(|s| {
        if let Some(j) = s.jobs.get_mut(job) {
            j.stage = st;
            j.got = got;
            j.total = total;
        }
    });
    ring();
}

fn fail(job: usize, why: u8) {
    with(|s| {
        if let Some(j) = s.jobs.get_mut(job) {
            j.stage = STAGE_FAILED;
            j.why = why;
        }
    });
    ring();
}

/// The job's `who`: the developer a refused kist's key differs from.
fn set_who(job: usize, who: String) {
    with(|s| {
        if let Some(j) = s.jobs.get_mut(job) {
            j.who = who;
        }
    });
}

/// A refusal of `ferros_kist::capsule::admit` as the page's reason (and the developer's name for a key other than the pinned one), said on the console in full.
fn refused(job: usize, r: &ferros_kist::capsule::Refused) -> u8 {
    use ferros_kist::capsule::Refused;
    let mut l = Vec::new();
    l.extend_from_slice(b"kists: refused: ");
    l.extend_from_slice(ferros_kist::capsule::refusal_words(r).as_bytes());
    l.push(b'\n');
    say(&l);
    match r {
        Refused::NoBundle => WHY_NO_BUNDLE,
        Refused::NotItsKey => WHY_NOT_ITS_KEYS,
        Refused::Eggs => WHY_EGGS,
        Refused::OtherKey(p) => {
            set_who(job, ferros_kist::capsule::pin_name(p));
            WHY_OTHER_KEY
        }
    }
}

/// Room for one more job: the oldest finished one gives way. False when every slot is under way.
fn make_room(s: &mut State) -> bool {
    if s.jobs.len() < JOBS_MAX {
        return true;
    }
    match s.jobs.iter().position(|j| j.stage >= STAGE_DONE) {
        Some(i) => {
            s.jobs.remove(i);
            true
        }
        None => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The tables of `&str` and the vtables are absolute words: relocated before anything reads them.
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_SIZE) };
    GRANTS.store(grants, Ordering::Relaxed);
    unsafe { *STATE.0.get() = Some(State { count: 0, watchers: Vec::new(), dir_state: 0, dir_checked: false, dir: None, dir_gen: 0, dir_wanted: false, restore_wanted: true, jobs: Vec::new() }) };
    ferros_sys::thread_spawn(worker, unsafe { &mut *WORKER_STACK.0.get() }, grants);
    ferros_sys::thread_spawn(space_watch, unsafe { &mut *WATCHER_STACK.0.get() }, grants);
    say(b"kists: serving the installer (kists); a restore follows the identity service\n");
    serve(grants)
}

/// The `kists` endpoint: every op answered at once (the work is the worker's), the doorbell held.
fn serve(grants: u64) -> ! {
    let mut req = [0u8; 128];
    let mut tok = [0u8; 32];
    loop {
        let n = ferros_sys::recv(cap(grants, S_SERVE), &mut req, &mut tok, false);
        if n == ERR || n == NOTHING || n == 0 {
            continue;
        }
        // WHY: `n` is what the caller sent, which may be more than the buffer took.
        let got = (n as usize).min(req.len());
        let m = &req[..got];
        match m[0] {
            0 if got >= 5 => {
                let theirs = u32::from_le_bytes([m[1], m[2], m[3], m[4]]);
                let now_msg = with(|s| {
                    if theirs != s.count || s.watchers.len() >= WATCHERS_MAX {
                        Some(status(s))
                    } else {
                        s.watchers.push(tok);
                        None
                    }
                });
                if let Some(msg) = now_msg {
                    reply(&tok, &msg);
                }
            }
            // The page opened: the directory read again, and a restore that failed (no network then) tried again, on the person's own act.
            1 => {
                with(|s| {
                    s.dir_wanted = true;
                    s.restore_wanted = true;
                });
                kick();
                reply(&tok, &[0]);
            }
            2 if got >= 2 => {
                let r = with(|s| directory_reply(s, m[1]));
                reply(&tok, &r);
            }
            3 if got >= 33 => {
                let capsule: [u8; 32] = m[1..33].try_into().unwrap();
                let r = with(|s| {
                    let Some(e) = s.dir.as_ref().and_then(|d| d.kists.iter().find(|e| e.capsule == capsule)).cloned() else { return 1 };
                    if s.jobs.iter().any(|j| j.capsule == capsule && j.stage < STAGE_DONE) {
                        return 2;
                    }
                    if !make_room(s) {
                        return 3;
                    }
                    s.jobs.push(Job { kind: KIND_INSTALL, capsule, developer: e.developer, option: e.option, name: e.name.clone(), stage: STAGE_QUEUED, why: 0, got: 0, total: e.size, who: String::new() });
                    0
                });
                reply(&tok, &[r]);
                if r == 0 {
                    kick();
                    ring();
                }
            }
            4 if got >= 65 => {
                let (developer, option): ([u8; 32], [u8; 32]) = (m[1..33].try_into().unwrap(), m[33..65].try_into().unwrap());
                // The list read here, not under the lock: a vault call waits.
                let k = ferros_kist::store::read(cap(grants, S_KISTS)).and_then(|l| l.by_identity(&developer, &option).cloned());
                let r = match k {
                    None => 1,
                    Some(k) => with(|s| {
                        if !make_room(s) {
                            return 3;
                        }
                        s.jobs.push(Job { kind: KIND_UNINSTALL, capsule: k.capsule, developer, option, name: k.name.clone(), stage: STAGE_QUEUED, why: 0, got: 0, total: 0, who: String::new() });
                        0
                    }),
                };
                reply(&tok, &[r]);
                if r == 0 {
                    kick();
                    ring();
                }
            }
            _ => reply(&tok, &[]),
        }
    }
}

/// Op 2: the directory's entries from `from`, as many as fit a reply.
fn directory_reply(s: &State, from: u8) -> Vec<u8> {
    let Some(d) = s.dir.as_ref() else { return alloc::vec![0, 0] };
    // WHY: one byte counts the entries a page lists; a directory past 255 is shown to 255 until the page pages further.
    let all = d.kists.len().min(255);
    let mut r = alloc::vec![all as u8, 0];
    let mut n = 0u8;
    for e in d.kists.iter().take(all).skip(from as usize) {
        // WHY: the page's rows; longer text is cut, never sent past one reply (4 KiB).
        let name = &e.name.as_bytes()[..e.name.len().min(32)];
        let about = &e.about.as_bytes()[..e.about.len().min(96)];
        // WHY: the part count is one byte. The version is sent as written (the page drops its leading zero parts itself, ferros_kist::significant); one past 255 parts is sent to its first 255, and an entry is then at most 32 * 3 + 8 + 1 + 255 * 8 + 2 + 32 + 96 = A#2275 bytes, so the first always fits the reply.
        let parts = &e.version.0[..e.version.0.len().min(255)];
        let need = 32 * 3 + 8 + 1 + (parts.len() << 3) + 2 + name.len() + about.len();
        if r.len() + need > 4000 {
            break;
        }
        r.extend_from_slice(&e.capsule);
        r.extend_from_slice(&e.developer);
        r.extend_from_slice(&e.option);
        r.extend_from_slice(&e.size.to_le_bytes());
        r.push(parts.len() as u8);
        for p in parts {
            r.extend_from_slice(&p.to_le_bytes());
        }
        r.push(name.len() as u8);
        r.extend_from_slice(name);
        r.push(about.len() as u8);
        r.extend_from_slice(about);
        n += 1;
    }
    r[1] = n;
    r
}

/// The identity service's doorbell (fgtw op 21), held on this thread: each time it rings and the person's space generation (op 17) moved, or its first pull has just settled, a restore pass is due. While fgtw is not serving (it starts after attest and may restart) it is asked again after a wait that doubles to a second, as the map's `fgtw_poller` does: no doorbell exists for a server coming up.
extern "C" fn space_watch(grants: u64) -> ! {
    let svc = cap(grants, S_FGTW);
    let mut count = u32::MAX;
    let mut seen: Option<(u32, u8)> = None;
    let mut absent = freq() >> 6;
    loop {
        let mut m = [21u8, 0, 0, 0, 0];
        m[1..].copy_from_slice(&count.to_le_bytes());
        let mut r = [0u8; 4];
        if ferros_sys::call_live(svc, &m, &mut r) != 4 {
            sleep_until(now() + absent);
            if absent < freq() {
                absent <<= 1;
            }
            continue;
        }
        absent = freq() >> 6;
        count = u32::from_le_bytes(r);
        let mut sp = [0u8; 8];
        if ferros_sys::call_live(svc, &[17], &mut sp) < 6 {
            continue;
        }
        // [generation u32][home sync][first pull settled]...
        let now_seen = (u32::from_le_bytes([sp[0], sp[1], sp[2], sp[3]]), sp[5]);
        if now_seen.1 == 1 && seen != Some(now_seen) {
            seen = Some(now_seen);
            with(|s| s.restore_wanted = true);
            kick();
        }
    }
}

/// What the worker does next: the directory when asked, then a queued job, then a restore pass.
enum Task {
    Directory,
    Job(usize),
    Restore,
}

extern "C" fn worker(grants: u64) -> ! {
    loop {
        ferros_sys::pet();
        let w = WORK.load(Ordering::Acquire);
        let task = with(|s| {
            if s.dir_wanted {
                s.dir_wanted = false;
                return Some(Task::Directory);
            }
            if let Some(i) = s.jobs.iter().position(|j| j.stage == STAGE_QUEUED) {
                return Some(Task::Job(i));
            }
            if s.restore_wanted {
                s.restore_wanted = false;
                return Some(Task::Restore);
            }
            None
        });
        match task {
            Some(Task::Directory) => fetch_directory(grants),
            Some(Task::Job(i)) => run(grants, i),
            Some(Task::Restore) => restore_pass(grants),
            None => {
                ferros_sys::wait_world(&WORK, w, 0);
            }
        }
    }
}

/// The directory fetched, checked when the project key is pinned, and kept for the page.
fn fetch_directory(grants: u64) {
    with(|s| s.dir_state = 1);
    ring();
    let url = alloc::format!("http://{}{}", ferros_kist::directory::HOST, ferros_kist::directory::PATH);
    let mut doc: Vec<u8> = Vec::new();
    // WHY the cap: the directory is read whole in the heap; one past 1 MiB is no list of kists this device takes.
    let r = net::get(cap(grants, S_UDP), &url, |_| Ok(()), |b| if doc.len() + b.len() > 1 << 20 { Err("the directory is too large") } else { Ok(doc.extend_from_slice(b)) });
    let read = match r {
        Ok(()) => {
            let project = fgtw::pq::KeyBundle::from_bytes(PROJECT_BUNDLE).ok();
            match &project {
                Some(p) => ferros_kist::directory::verify_directory(&doc, p).map(|d| (d, true)).map_err(|_| ()),
                None => ferros_kist::directory::parse_directory(&doc).map(|d| (d, false)).map_err(|_| ()),
            }
        }
        Err(_) => Err(()),
    };
    match read {
        Ok((d, checked)) => {
            if !checked {
                say(b"kists: the directory's signature is not checked: the project key's bundle is not pinned yet\n");
            }
            // Each entry's version as this process parsed it, parts as written (2026-11-10: the page showed "Clock 0.0.0" for an entry the host parses as 0.1.0; the next run says which side read zeros).
            for e in &d.kists {
                let mut l = alloc::format!("kists: listed {} version", e.name);
                for (i, p) in e.version.0.iter().enumerate() {
                    l.push_str(&alloc::format!("{}A#{p}", if i == 0 { " " } else { "." }));
                }
                l.push('\n');
                say(l.as_bytes());
            }
            with(|s| {
                s.dir = Some(d);
                s.dir_state = 2;
                s.dir_checked = checked;
                // WHY wrapping: the page only asks whether it moved; past 2^32 reads it starts again, which a plain add would panic on in a debug build.
                s.dir_gen = s.dir_gen.wrapping_add(1);
            });
        }
        Err(()) => {
            say(b"kists: the directory could not be read\n");
            with(|s| s.dir_state = 3);
        }
    }
    ring();
}

/// The directory, read first if it has not been (a restore names capsules by blake3 alone).
fn directory(grants: u64) -> Option<Directory> {
    if with(|s| s.dir_state != 2) {
        fetch_directory(grants);
    }
    with(|s| s.dir.clone())
}

fn run(grants: u64, i: usize) {
    let job = with(|s| s.jobs[i].clone());
    let r = match job.kind {
        KIND_INSTALL => install(grants, i, &job),
        KIND_UNINSTALL => uninstall(grants, &job),
        _ => restore(grants, i, &job),
    };
    match r {
        Ok(()) => stage(i, STAGE_DONE, job.total, job.total),
        Err(why) => fail(i, why),
    }
}

/// Trust on first install, kept: the developer's bundle pinned in the person's list when none was (`Kists::trust`); a pin that differs now (the space brought one in since the check) refuses. Ok(true) when the list changed and must be written.
fn pin(i: usize, list: &mut ferros_kist::Kists, developer: &[u8; 32], a: &ferros_kist::capsule::Admitted, now: i64) -> Result<bool, u8> {
    let Some(b) = a.bundle else { return Ok(false) };
    match list.trust(developer, &b, a.name.as_deref(), now) {
        ferros_kist::Trust::First => Ok(true),
        ferros_kist::Trust::Same => Ok(false),
        ferros_kist::Trust::Other(p) => Err(refused(i, &ferros_kist::capsule::Refused::OtherKey(p))),
    }
}

/// Where an entry's capsule is fetched from: a path on the directory's host. A manifest entry (the developer's own signed manifest, Photon's) is not read on the device yet.
fn entry_url(e: &Entry) -> Result<String, u8> {
    match &e.source {
        Source::Path(p) => Ok(alloc::format!("http://{}{}", ferros_kist::directory::HOST, p)),
        Source::Manifest { .. } => Err(WHY_MANIFEST),
    }
}

/// An install (or an update) of the directory's entry: brought in through every stage, then into the person's list.
fn install(grants: u64, i: usize, job: &Job) -> Result<(), u8> {
    let e = with(|s| s.dir.as_ref().and_then(|d| d.kists.iter().find(|e| e.capsule == job.capsule)).cloned()).ok_or(WHY_NOT_LISTED)?;
    let url = entry_url(&e)?;
    let (taken, admitted) = bring(grants, i, &url, &e.capsule, Some(e.size), &e.developer, &e.option)?;
    stage(i, STAGE_STORING, e.size, e.size);
    let decl = ferros_kist::declaration(&taken.first).map_err(|_| WHY_NOT_KIST)?;
    // Its version as its signed declaration gives it (what the page compares a listed one against); a capsule without one (a manifest's artefact) takes the entry's, which its developer's signed manifest gave. A version field that does not read is no kist's.
    let version = ferros_kist::version_of(&taken.first).map_err(|_| WHY_NOT_KIST)?.unwrap_or_else(|| e.version.clone());
    let list_cap = cap(grants, S_KISTS);
    let Some(mut list) = ferros_kist::store::read(list_cap) else {
        drop_capsule(grants, &e.capsule);
        return Err(WHY_NOBODY);
    };
    let at = ferros_detail::eagle_now();
    // The developer's bundle pinned with the kist, in one write of the list: the first kist by a developer trusts it, every later one was checked against it.
    if let Err(why) = pin(i, &mut list, &e.developer, &admitted, at) {
        drop_capsule(grants, &e.capsule);
        return Err(why);
    }
    let old = list.by_identity(&e.developer, &e.option).map(|k| k.capsule);
    let k = list.install(e.capsule, e.developer, e.option, &decl);
    list.kists[k].source = Some(url);
    list.kists[k].installed = at;
    list.kists[k].version = Some(version);
    if !ferros_kist::store::write(list_cap, &list, at) {
        drop_capsule(grants, &e.capsule);
        return Err(WHY_VAULT);
    }
    // An update: the version it replaced is named by no kist of the person's any more.
    if let Some(o) = old.filter(|o| *o != e.capsule && list.by_capsule(o).is_none()) {
        drop_capsule(grants, &o);
    }
    let mut l = Vec::new();
    l.extend_from_slice(b"kists: installed ");
    l.extend_from_slice(e.name.as_bytes());
    l.extend_from_slice(b", checked and sealed\n");
    say(&l);
    Ok(())
}

/// An uninstall: out of the person's list, its answers kept, its capsule dropped when no kist of theirs names it.
fn uninstall(grants: u64, job: &Job) -> Result<(), u8> {
    let list_cap = cap(grants, S_KISTS);
    let mut list = ferros_kist::store::read(list_cap).ok_or(WHY_NOBODY)?;
    let Some(k) = list.remove(&job.developer, &job.option) else { return Ok(()) };
    if !ferros_kist::store::write(list_cap, &list, ferros_detail::eagle_now()) {
        return Err(WHY_VAULT);
    }
    if list.by_capsule(&k.capsule).is_none() {
        drop_capsule(grants, &k.capsule);
    }
    let mut l = Vec::new();
    l.extend_from_slice(b"kists: uninstalled ");
    l.extend_from_slice(k.name.as_bytes());
    l.extend_from_slice(b"; its answers kept\n");
    say(&l);
    Ok(())
}

/// A restore pass: a job for each capsule the person's list names that this device lacks (the list came back from their fleet after a reset, or a kist was installed on another of their devices).
fn restore_pass(grants: u64) {
    let Some(list) = ferros_kist::store::read(cap(grants, S_KISTS)) else { return };
    let Some(have) = capsules_here(grants) else { return };
    let missing = ferros_space_missing(&list, &have);
    if missing.is_empty() {
        return;
    }
    let mut added = false;
    with(|s| {
        for c in &missing {
            if s.jobs.iter().any(|j| j.capsule == *c && j.stage < STAGE_DONE) {
                continue;
            }
            let Some(k) = list.by_capsule(c) else { continue };
            if !make_room(s) {
                break;
            }
            s.jobs.push(Job { kind: KIND_RESTORE, capsule: *c, developer: k.developer, option: k.option, name: k.name.clone(), stage: STAGE_QUEUED, why: 0, got: 0, total: 0, who: String::new() });
            added = true;
        }
    });
    if added {
        say(b"kists: restoring the capsules the person's list names and this device lacks\n");
        kick();
        ring();
    }
}

/// The capsules the list names that `have` lacks, in list order (ferros_space `missing_capsules`'s rule, without the space's crate in this process).
fn ferros_space_missing(list: &ferros_kist::Kists, have: &[[u8; 32]]) -> Vec<[u8; 32]> {
    list.kists.iter().map(|k| k.capsule).filter(|c| !have.contains(c)).collect()
}

/// The addresses in `capsules.` (a listing read like a large value, 32 bytes each). None when the vault refuses.
fn capsules_here(grants: u64) -> Option<Vec<[u8; 32]>> {
    let c = cap(grants, S_CAPSULES);
    let mut buf = [0u8; 4096];
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let whole = ferros_sys::store_addrs(c, at, &mut buf)?;
        // WHY: the listing's length is the vault's; the last read is shorter than the buffer.
        let n = (whole - at.min(whole)).min(buf.len());
        for a in buf[..n - n % 32].chunks_exact(32) {
            out.push(a.try_into().unwrap());
        }
        at += n;
        if n == 0 || at >= whole {
            return Some(out);
        }
    }
}

/// A restore of one capsule: the fleet's devices first (not built: `fleet_fetch`), then where it was installed from, then the directory by its blake3.
fn restore(grants: u64, i: usize, job: &Job) -> Result<(), u8> {
    if fleet_fetch(&job.capsule) {
        return Ok(());
    }
    let list = ferros_kist::store::read(cap(grants, S_KISTS)).ok_or(WHY_NOBODY)?;
    let k = list.by_capsule(&job.capsule).cloned().ok_or(WHY_NOT_LISTED)?;
    let dir = directory(grants);
    let entry = dir.as_ref().and_then(|d| d.kists.iter().find(|e| e.capsule == job.capsule));
    let mut sources: Vec<String> = Vec::new();
    if let Some(u) = &k.source {
        sources.push(u.clone());
    }
    if let Some(u) = entry.map(entry_url).and_then(|r| r.ok()) {
        if !sources.contains(&u) {
            sources.push(u);
        }
    }
    let size = entry.map(|e| e.size);
    let mut why = WHY_NOT_LISTED;
    for u in &sources {
        match bring(grants, i, u, &job.capsule, size, &k.developer, &k.option) {
            Ok((_, admitted)) => {
                // A list from before pins (or one whose pin has not arrived from the space yet): the restored kist's bundle pinned now, as at a first install.
                let list_cap = cap(grants, S_KISTS);
                if let Some(mut list) = ferros_kist::store::read(list_cap) {
                    let at = ferros_detail::eagle_now();
                    match pin(i, &mut list, &k.developer, &admitted, at) {
                        Ok(true) if !ferros_kist::store::write(list_cap, &list, at) => return Err(WHY_VAULT),
                        Ok(_) => {}
                        Err(w) => {
                            drop_capsule(grants, &job.capsule);
                            return Err(w);
                        }
                    }
                }
                let mut l = Vec::new();
                l.extend_from_slice(b"kists: restored ");
                l.extend_from_slice(k.name.as_bytes());
                l.extend_from_slice(b", checked and sealed\n");
                say(&l);
                return Ok(());
            }
            // The next source only for a capsule this one did not have; a capsule that failed its checks fails the same anywhere.
            Err(w @ (WHY_NETWORK | WHY_MISSING | WHY_BROKEN)) => why = w,
            Err(w) => return Err(w),
        }
    }
    Err(why)
}

/// The fleet's own devices first (owner, 2026-11-10: "BLE, then LAN, then WAN, then hopefully we shouldn't need to but fgtw.org"; docs/IDENTITY-SPACE.md "Capsules", source 2): a `capsule_want` over Photon Transport to the person's devices heard, answered in pieces from their `capsules.`, each piece into this device's vault as the HTTP path does, then the same checks. Not built: it is new protocol (the frame, fgtw's routing to a sibling that holds the capsule, the sibling's answer). TODO(fleet capsules): build it in payloads/fgtw beside `space_want`, and return true here once the capsule is stored and checked.
fn fleet_fetch(_capsule: &[u8; 32]) -> bool {
    false
}

/// The capsule's bytes in the vault dropped (a failed install, an uninstall, an update's old version).
fn drop_capsule(grants: u64, capsule: &[u8; 32]) {
    let _ = ferros_sys::store_delete_addr(cap(grants, S_CAPSULES), capsule);
}

/// Stages 0 to 3 for one capsule from one URL: fetched into the vault as it streams, its signature checked (its blake3, its size, its identity, every egg against the developer bundle it carries, and that bundle the one the person pinned for its developer), committed, then checked and sealed by the kernel. On any failure its bytes are dropped. Returns what was taken in (its first bytes carry its declaration) and what to pin.
fn bring(grants: u64, i: usize, url: &str, expect: &[u8; 32], size: Option<u64>, developer: &[u8; 32], option: &[u8; 32]) -> Result<(Taken, ferros_kist::capsule::Admitted), u8> {
    let total = size.unwrap_or(0);
    stage(i, STAGE_FETCHING, 0, total);
    let capsules = cap(grants, S_CAPSULES);
    let piece = unsafe { &mut *PIECE_BUF.0.get() };
    let mut fill = 0usize;
    let mut written = 0usize;
    let mut intake = Intake::new();
    let mut told = 0u64;
    let length = core::cell::Cell::new(total);
    let r = net::get(
        cap(grants, S_UDP),
        url,
        |resp| match (resp.body, size) {
            (Some(Body::Length(n)), Some(s)) if n != s => Err("its length is not the directory's"),
            (Some(Body::Length(n)), None) => {
                length.set(n);
                Ok(())
            }
            _ => Ok(()),
        },
        |mut bytes| {
            intake.update(bytes).map_err(|_| "not a signed capsule")?;
            // WHY: the directory's size, when it gave one, is the most that is taken.
            if size.is_some_and(|s| intake.len() > s) {
                return Err("longer than the directory's size");
            }
            while !bytes.is_empty() {
                // A full piece goes out only once more bytes come, so the last piece is never empty and is held back for the commit.
                if fill == PIECE {
                    if !ferros_sys::store_set_addr_at(capsules, expect, written, piece, true) {
                        return Err("the vault did not take a piece");
                    }
                    written += PIECE;
                    fill = 0;
                }
                // WHY: what fits the piece now.
                let n = (PIECE - fill).min(bytes.len());
                piece[fill..fill + n].copy_from_slice(&bytes[..n]);
                fill += n;
                bytes = &bytes[n..];
            }
            // The page's bar moves at each mebibyte.
            if intake.len() >> 20 != told >> 20 {
                told = intake.len();
                stage(i, STAGE_FETCHING, told, length.get());
            }
            Ok(())
        },
    );
    let failed = |why: u8| {
        drop_capsule(grants, expect);
        Err(why)
    };
    match r {
        Ok(()) => {}
        Err(net::Fail::Network) => return failed(WHY_NETWORK),
        Err(net::Fail::Missing(_)) => return failed(WHY_MISSING),
        Err(net::Fail::Broken("not a signed capsule")) => return failed(WHY_EGGS),
        Err(net::Fail::Broken("the vault did not take a piece")) => return failed(WHY_VAULT),
        Err(net::Fail::Broken("its length is not the directory's" | "longer than the directory's size")) => return failed(WHY_SIZE),
        Err(net::Fail::Broken(_)) => return failed(WHY_BROKEN),
    }
    // 1: its signature, before its last piece commits it.
    stage(i, STAGE_SIGNATURE, intake.len(), intake.len());
    let len = intake.len();
    let Ok(taken) = intake.finish() else { return failed(WHY_EGGS) };
    if taken.address != *expect {
        return failed(WHY_NOT_NAMED);
    }
    if size.is_some_and(|s| s != len) {
        return failed(WHY_SIZE);
    }
    match ferros_kist::identity(&taken.first) {
        Ok((d, o)) if d == *developer && o == *option => {}
        _ => return failed(WHY_NOT_KIST),
    }
    // Every egg against the bundle the capsule carries, its Ed25519 key the kist's developer, and that bundle the one the person pinned for this developer if they pinned one (trust on first install; Photon's release key against fgtw's pinned bundle).
    let Some(list) = ferros_kist::store::read(cap(grants, S_KISTS)) else { return failed(WHY_NOBODY) };
    let admitted = match ferros_kist::capsule::admit(&taken.first, &taken.head.signer, &taken.head.eggs, &taken.file_hash, &list) {
        Ok(a) => a,
        Err(r) => return failed(refused(i, &r)),
    };
    if fill == 0 || !ferros_sys::store_set_addr_at(capsules, expect, written, &piece[..fill], false) {
        return failed(WHY_VAULT);
    }
    // 2 and 3: the kernel reads it back, checks its code and seals it (SANDBOX.md: once, at install).
    stage(i, STAGE_CODE, 0, len);
    if let Err(why) = kernel_check(grants, i, expect, len) {
        return failed(why);
    }
    Ok((taken, admitted))
}

/// The kernel's check and seal of a stored capsule, each stage it tells shown: its reading of the capsule is the code stage's bar, then the checker, then the seal.
fn kernel_check(grants: u64, i: usize, capsule: &[u8; 32], len: u64) -> Result<(), u8> {
    use ferros_sys::install as k;
    KERNEL_WORD.store(u64::MAX, Ordering::Release);
    if ferros_sys::install_kist(grants, S_INSTALL, capsule, &KERNEL_WORD) != 0 {
        return Err(WHY_ROOM);
    }
    let mut last = u64::MAX;
    loop {
        let w = KERNEL_WORD.load(Ordering::Acquire);
        if w != last {
            last = w;
            let (st, v) = k::split(w);
            match st {
                k::READING => stage(i, STAGE_CODE, v, len),
                k::CHECKING => stage(i, STAGE_CODE, len, len),
                k::SEALING => stage(i, STAGE_SEALING, len, len),
                k::DONE => return Ok(()),
                k::REFUSED => {
                    return Err(match v {
                        k::WHY_NATIVE => WHY_NATIVE,
                        k::WHY_CHECKER | k::WHY_MODES => WHY_CHECKER,
                        k::WHY_SIGNATURE | k::WHY_SECTIONS => WHY_EGGS,
                        k::WHY_NOT_NAMED => WHY_NOT_NAMED,
                        k::WHY_SECRET => WHY_SEAL,
                        k::WHY_KEPT | k::WHY_READ => WHY_VAULT,
                        _ => self::WHY_ROOM,
                    })
                }
                // u64::MAX: not told yet.
                _ => {}
            }
        }
        ferros_sys::wait_world(&KERNEL_WORD, w, 0);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
