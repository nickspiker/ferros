//! ferros-flash: the CLI over the library (lib.rs and its modules).
//!   ferros-flash guide                         the walk thru for a person: find the phone, name it, its lock state, the choices, each step said before it is done (docs/FLASHER.md)
//!   ferros-flash devices                       every phone in fastboot on this computer, by product and serial (read only)
//!   ferros-flash getvar <name|all> [serial]    one bootloader variable, or all of them (read only)
//!   ferros-flash board <product> <out.vsf>     the built-in board manifest as its VSF document
//!   ferros-flash log show <run.log.vsf>        a run's records, rendered
//!   ferros-flash log report <run.log.vsf> <out.vsf> [--keep-serial]    the one document a person submits
//!   ferros-flash release verify <release.vsf>  a release checked against the ferros key, its files listed
//!   ferros-flash release make ...              (built with `publish`) a release signed with a key from keys/
//!   ferros-flash show  <head> <tail> <tail-lba> [block]
//!   ferros-flash carve <head> <tail> <tail-lba> <userdata-GiB> <out-dir> [block]
//! `carve` writes <out-dir>/head.bin and tail.bin (the images with the new table), reads them back as a disk would, and prints the block ranges to write as `write <first-lba> <blocks>` lines, the primary's first.
//! Every command that talks to a phone keeps a run log (runlog.rs) from its first step, and a panic is logged before the tool ends.

use ferros_flash::board::Board;
use ferros_flash::release::{Release, FERROS_PUB};
use ferros_flash::runlog::{self, Ev, RunLog};
use ferros_flash::Gpt;
use std::io::{BufRead, Write};
use std::process::exit;
use std::sync::{Arc, Mutex};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn die(msg: impl core::fmt::Display) -> ! {
    eprintln!("ferros-flash: {msg}");
    exit(1)
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}")))
}

fn num(s: &str) -> u64 {
    s.parse()
        .unwrap_or_else(|_| die(format!("not a number: {s}")))
}

/// The run's log, shared with the panic hook so a crash still leaves its last words.
type Log = Arc<Mutex<RunLog>>;

fn open_log(what: &str) -> Log {
    let start = vsf::eagle_time_oscillations();
    let facts = Ev::info("start", "ferros-flash {} on {}: {}")
        .t(VERSION)
        .t(&platform())
        .t(what)
        .id("tool", VERSION)
        .id("platform", &platform());
    let l = RunLog::create(&runlog::default_dir(), start, facts)
        .unwrap_or_else(|e| die(format!("cannot keep a log: {e}")));
    eprintln!("ferros-flash: this run's log is {}", l.path().display());
    let log = Arc::new(Mutex::new(l));
    let hook = log.clone();
    std::panic::set_hook(Box::new(move |p| {
        let at = p
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let msg = p
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        if let Ok(mut l) = hook.lock() {
            let _ = l.append(
                &Ev::error("crash", "the tool crashed at {}: {}")
                    .t(&at)
                    .t(&msg),
            );
            eprintln!(
                "ferros-flash crashed ({msg} at {at}); the log is {}",
                l.path().display()
            );
        }
    }));
    log
}

fn note(log: &Log, ev: Ev) {
    if let Err(e) = log.lock().unwrap().append(&ev) {
        eprintln!("ferros-flash: the log could not be written: {e}");
    }
}

/// Say it, log it.
fn say(log: &Log, ev: Ev, words: &str) {
    println!("{words}");
    note(log, ev);
}

fn ask(question: &str) -> bool {
    print!("{question} Type yes to go on: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim().eq_ignore_ascii_case("yes")
}

#[cfg(feature = "usb")]
mod phone {
    use super::*;
    use ferros_flash::fastboot::Fastboot;
    use ferros_flash::usb::{self, Usb};

    /// The phones in fastboot with the product each answers; one a line.
    pub fn devices() {
        let found = usb::list().unwrap_or_else(|e| die(e));
        if found.is_empty() {
            println!("no phone in fastboot on this computer");
        }
        for f in found {
            let product = Usb::open(&f)
                .ok()
                .and_then(|u| Fastboot::new(u, Board::akita()).getvar("product").ok())
                .unwrap_or_else(|| "?".into());
            println!(
                "{product}\t{}\tG#{:04X}:{:04X}\tbus A#{}",
                f.serial, f.vid, f.pid, f.bus
            );
        }
    }

    /// The one phone to talk to: the serial asked for, else the only one there is.
    fn one(serial: Option<&str>) -> Result<usb::Found, String> {
        let found = usb::list()?;
        let mut m: Vec<_> = found
            .into_iter()
            .filter(|f| serial.is_none_or(|s| f.serial == s))
            .collect();
        match m.len() {
            0 => Err("no phone in fastboot (plugged in, and in its bootloader: hold power and volume down)".into()),
            1 => Ok(m.remove(0)),
            _ => Err("more than one phone in fastboot: name one by its serial".into()),
        }
    }

    pub fn getvar(name: &str, serial: Option<&str>) {
        let log = open_log("getvar");
        let f = one(serial).unwrap_or_else(|e| {
            note(&log, Ev::error("find", "{}").t(&e));
            die(e)
        });
        let mut fb = Fastboot::new(Usb::open(&f).unwrap_or_else(|e| die(e)), Board::akita());
        let lines = if name == "all" {
            fb.getvar_all()
        } else {
            fb.getvar(name).map(|v| vec![format!("{name}: {v}")])
        };
        match lines {
            Ok(ls) => {
                for l in &ls {
                    println!("{l}");
                }
                note(
                    &log,
                    Ev::info("getvar", "{} answered {} lines")
                        .t(name)
                        .n(ls.len() as u64)
                        .id("serial", &f.serial),
                );
            }
            Err(e) => {
                note(
                    &log,
                    Ev::error("getvar", "{}")
                        .t(&e.to_string())
                        .id("serial", &f.serial),
                );
                die(e)
            }
        }
    }

    /// The walk thru, as far as it is built: every step says what it will do, and stops before anything it cannot yet do.
    pub fn guide() {
        let log = open_log("guide");
        println!("ferros-flash {VERSION}: put ferros on a Pixel.");
        println!("Each step says what it will do before it does it. Nothing is written to the phone without your yes.");
        println!("Everything this run does is kept in the log named above; you can send it to us at any point (ferros-flash log report).");
        println!();
        say(&log, Ev::info("find", "looking for a phone in fastboot"), "Step 0: find the phone. Put it in its bootloader (power it off, then hold power and volume down) and plug it in.");
        let f = loop {
            match one(None) {
                Ok(f) => break f,
                Err(e) => {
                    note(&log, Ev::warn("find", "{}").t(&e));
                    if !ask(&format!("{e}. Try again?")) {
                        die("stopped: no phone");
                    }
                }
            }
        };
        let mut fb = match Usb::open(&f) {
            Ok(u) => Fastboot::new(u, Board::akita()),
            Err(e) => {
                // On Windows this is where a missing WinUSB driver shows.
                note(
                    &log,
                    Ev::error("find", "the phone is there but cannot be opened: {}")
                        .t(&e)
                        .u("vid", f.vid as u64)
                        .u("pid", f.pid as u64),
                );
                die(format!("{e}\nOn Windows, the phone's bootloader needs the WinUSB driver: see the guide's notes."));
            }
        };
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        fb.on_line = Some(Box::new(move |s| l2.lock().unwrap().push(s.to_string())));
        let product = fb.getvar("product").unwrap_or_else(|e| die(e));
        let Some(board) = Board::for_product(product.trim()) else {
            note(
                &log,
                Ev::error("name", "{} is not a phone ferros runs on")
                    .t(&product)
                    .id("product", &product),
            );
            die(format!("this phone answers `{product}`: ferros runs on the Pixel 8a (akita) and the Pixel 8 Pro (husky) only; nothing was written"));
        };
        fb.board = board.clone();
        let mut ev = Ev::info("name", "found a {}")
            .t(&board.model)
            .id("product", &board.product)
            .id("serial", &f.serial);
        for v in [
            "version-bootloader",
            "version-baseband",
            "current-slot",
            "secure",
            "unlocked",
        ] {
            if let Ok(x) = fb.getvar(v) {
                ev = ev.id(&v.replace('-', "_"), x.trim());
            }
        }
        say(
            &log,
            ev,
            &format!(
                "Step 1: this is a {} ({}), serial {}.",
                board.model, board.product, f.serial
            ),
        );
        for t in &board.traps {
            println!("  Note: {t}");
        }

        println!();
        println!("Step 2: the bootloader's lock.");
        let unlocked = fb.unlocked().unwrap_or_else(|e| die(e));
        note(
            &log,
            Ev::info("lock", "unlocked: {}")
                .t(if unlocked { "yes" } else { "no" })
                .flag("unlocked", unlocked),
        );
        if !unlocked {
            let able = fb.unlock_ability().unwrap_or(false);
            note(
                &log,
                Ev::info("lock", "unlock allowed: {}")
                    .t(if able { "yes" } else { "no" })
                    .flag("unlock_ability", able),
            );
            if !able {
                say(
                    &log,
                    Ev::warn("lock", "unlocking is not allowed on this phone yet"),
                    "Unlocking is not allowed yet. In Android: Settings, About phone, tap Build number seven times; then Settings, System, Developer options, turn on OEM unlocking. (A carrier-locked phone may not offer it.) Then come back to the bootloader and run the guide again.",
                );
                return;
            }
            println!("Unlocking WIPES THE PHONE: every app, photo and file on it is erased. Back up what you want to keep first.");
            println!("The phone will ask you itself: choose with the volume keys and press power. The tool only sends the request.");
            if !ask("Send the unlock request to the phone?") {
                say(
                    &log,
                    Ev::info("lock", "the person chose not to unlock"),
                    "Stopped. Nothing was written.",
                );
                return;
            }
            note(&log, Ev::info("lock", "sent flashing unlock"));
            match fb.unlock() {
                Ok(_) => say(&log, Ev::info("lock", "the phone answered the unlock request"), "The phone answered. If you chose to unlock, it is wiping itself and will restart; put it back in the bootloader and run the guide again."),
                Err(e) => {
                    note(&log, Ev::error("lock", "{}").t(&e.to_string()));
                    die(e)
                }
            }
            return;
        }

        println!();
        println!("Step 3: what will be done. ferros replaces Android entirely (the only option: ferros does not run beside Android).");
        println!("  The phone's own calibration (persist, efs and the rest) is copied to this computer first, and the tool stops if any copy fails.");
        println!("  userdata shrinks to A#{} MiB and a new partition, ferros, takes the rest of the flash.", board.userdata_floor >> 20);
        println!("  ferros's signed boot image goes on both boot slots, each checked by reading it back.");
        println!("  Then ferros starts, and the tool checks it answers.");
        println!("  Last, the re-lock to the ferros key, which you do on the phone yourself.");
        note(
            &log,
            Ev::info("plan", "the plan was shown").u("userdata_floor", board.userdata_floor),
        );
        println!();
        say(
            &log,
            Ev::warn("stop", "the steps from the backup on are not built yet"),
            "This build of the tool stops here: the backup, the carve and the flash run thru ferros on the phone, and that part is not built yet (docs/FLASHER.md). Nothing was written. Thank you: the log of this run is worth sending.",
        );
        let words = lines.lock().unwrap().clone();
        for w in words {
            note(&log, Ev::info("bootloader", "{}").t(&w));
        }
    }
}

fn log_cmd(a: &[String]) {
    match (a.get(2).map(String::as_str), a.get(3)) {
        (Some("show"), Some(p)) => {
            let bytes = read(p);
            let (recs, whole) = runlog::read(&bytes);
            for r in &recs {
                let lvl = match r.level {
                    4 => "error",
                    3 => "warn",
                    _ => "info",
                };
                println!("{:016x} {lvl:<5} {:<10} {}", r.osc, r.step, r.msg);
            }
            if whole < bytes.len() {
                println!(
                    "(the last A#{} bytes are a record cut short: the run ended mid-write)",
                    bytes.len() - whole
                );
            }
        }
        (Some("report"), Some(p)) => {
            let out = a
                .get(4)
                .unwrap_or_else(|| die("log report <run.log.vsf> <out.vsf> [--keep-serial]"));
            let keep = a.iter().any(|x| x == "--keep-serial");
            let doc = runlog::report(
                &read(p),
                &format!("ferros-flash {VERSION} {}", platform()),
                vsf::eagle_time_oscillations(),
                keep,
            )
            .unwrap_or_else(|e| die(e));
            std::fs::write(out, &doc).unwrap_or_else(|e| die(format!("{out}: {e}")));
            println!(
                "{out}: A#{} bytes, the phone's serial {}",
                doc.len(),
                if keep {
                    "kept"
                } else {
                    "replaced by its keyed hash"
                }
            );
        }
        _ => die("log show <run.log.vsf> | log report <run.log.vsf> <out.vsf> [--keep-serial]"),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn release_cmd(a: &[String]) {
    match a.get(2).map(String::as_str) {
        Some("verify") if a.len() >= 4 => {
            let r = Release::verify(&read(&a[3]), &FERROS_PUB).unwrap_or_else(|e| die(e));
            println!("channel {}, commit {}", r.channel, hex(&r.commit));
            for f in &r.artefacts {
                println!(
                    "  {:?}\t{}\t{}\t{}\tA#{}\t{}",
                    f.role,
                    f.name,
                    f.board,
                    f.platform,
                    f.size,
                    f.file_name()
                );
            }
        }
        #[cfg(feature = "publish")]
        Some("make") => publish::make(&a[3..]),
        _ => die("release verify <release.vsf> | release make (built with --features publish)"),
    }
}

#[cfg(feature = "publish")]
mod publish {
    use super::*;
    use ferros_flash::release::{Artefact, Role};

    /// `release make --channel <c> --commit <hex40> --key <key.vsf> --out <release.vsf> --file <role> <board|-> <platform|-> <name> <path> ...`
    /// Each file is hashed here and copied to `<out dir>/<blake3 hex>`, the name the host serves it under.
    pub fn make(a: &[String]) {
        let (mut channel, mut commit, mut key, mut out) = (None, None, None, None);
        let mut files = Vec::new();
        let mut i = 0;
        while i < a.len() {
            match a[i].as_str() {
                "--channel" => channel = a.get(i + 1).cloned(),
                "--commit" => commit = a.get(i + 1).cloned(),
                "--key" => key = a.get(i + 1).cloned(),
                "--out" => out = a.get(i + 1).cloned(),
                "--file" if i + 5 < a.len() => {
                    files.push(a[i + 1..i + 6].to_vec());
                    i += 6;
                    continue;
                }
                x => die(format!("release make: `{x}`?")),
            }
            i += 2;
        }
        let (Some(channel), Some(commit), Some(key), Some(out)) = (channel, commit, key, out)
        else {
            die("release make needs --channel --commit --key --out")
        };
        let commit: [u8; 20] = (0..20)
            .map(|k| {
                u8::from_str_radix(commit.get(2 * k..2 * k + 2).unwrap_or("zz"), 16)
                    .unwrap_or_else(|_| die("--commit is the full 40 hex git SHA-1"))
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let dir = std::path::Path::new(&out)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let mut artefacts = Vec::new();
        for f in files {
            let role = match f[0].as_str() {
                "tool" => Role::Tool,
                "board" => Role::Board,
                "boot" => Role::Boot,
                "capsule" => Role::Capsule,
                "avbkey" => Role::AvbKey,
                r => die(format!("role `{r}`?")),
            };
            let bytes = read(&f[4]);
            let dash = |s: &str| {
                if s == "-" {
                    String::new()
                } else {
                    s.to_string()
                }
            };
            let art = Artefact {
                role,
                board: dash(&f[1]),
                platform: dash(&f[2]),
                name: f[3].clone(),
                size: bytes.len() as u64,
                blake3: *blake3::hash(&bytes).as_bytes(),
            };
            let to = dir.join(art.file_name());
            std::fs::write(&to, &bytes).unwrap_or_else(|e| die(format!("{}: {e}", to.display())));
            println!("{}\t{}", art.file_name(), art.name);
            artefacts.push(art);
        }
        let (_label, mut seed) =
            ferros_kist::secret::read(&read(&key)).unwrap_or_else(|e| die(format!("{key}: {e}")));
        let r = Release {
            channel,
            made: vsf::eagle_time_oscillations(),
            commit,
            artefacts,
        };
        let doc = r.sign(&seed);
        seed.fill(0);
        let doc = doc.unwrap_or_else(|e| die(e));
        std::fs::write(&out, &doc).unwrap_or_else(|e| die(format!("{out}: {e}")));
        println!("{out}: signed, A#{} files", r.artefacts.len());
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        #[cfg(feature = "usb")]
        Some("guide") | None => phone::guide(),
        #[cfg(feature = "usb")]
        Some("devices") => phone::devices(),
        #[cfg(feature = "usb")]
        Some("getvar") if a.len() >= 3 => phone::getvar(&a[2], a.get(3).map(String::as_str)),
        Some("board") if a.len() >= 4 => {
            let b = Board::for_product(&a[2]).unwrap_or_else(|| die(format!("no built-in board `{}`", a[2])));
            let doc = b.to_vsf(vsf::eagle_time_oscillations()).unwrap_or_else(|e| die(e));
            std::fs::write(&a[3], &doc).unwrap_or_else(|e| die(format!("{}: {e}", a[3])));
            println!("{}: {} ({}), A#{} bytes", a[3], b.model, b.product, doc.len());
        }
        Some("log") => log_cmd(&a),
        Some("release") => release_cmd(&a),
        Some("show") if a.len() >= 5 => {
            let block = a.get(5).map_or(4096, |s| num(s) as usize);
            let g = Gpt::read(block, &read(&a[2]), &read(&a[3]), num(&a[4])).unwrap_or_else(|e| die(e));
            print!("{}", g.table());
        }
        Some("carve") if a.len() >= 7 => {
            let block = a.get(7).map_or(4096, |s| num(s) as usize);
            let (mut head, mut tail, tail_lba) = (read(&a[2]), read(&a[3]), num(&a[4]));
            let mut g = Gpt::read(block, &head, &tail, tail_lba).unwrap_or_else(|e| die(e));
            g.carve(num(&a[5]) << 30).unwrap_or_else(|e| die(e));
            g.apply(&mut head, &mut tail, tail_lba);
            // Read back from the images as the bootloader will: both CRCs, both copies the same.
            let back = Gpt::read(block, &head, &tail, tail_lba).unwrap_or_else(|e| die(format!("the new table does not read back: {e}")));
            let out = &a[6];
            std::fs::create_dir_all(out).unwrap_or_else(|e| die(e));
            std::fs::write(format!("{out}/head.bin"), &head).unwrap_or_else(|e| die(e));
            std::fs::write(format!("{out}/tail.bin"), &tail).unwrap_or_else(|e| die(e));
            eprint!("{}", back.table());
            for w in g.writes() {
                println!("write {} {}", w.lba, w.bytes.len() / block);
            }
        }
        _ => die("usage: guide | devices | getvar <name|all> [serial] | board <product> <out.vsf> | log show|report ... | release verify|make ... | show ... | carve ..."),
    }
}
