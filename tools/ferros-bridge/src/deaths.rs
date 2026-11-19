//! `ferros-bridge deaths [n]`: the device's recent deaths, read from the flash log and named (KERNEL.md "A process that dies", LEDGER.md "Deaths").
//!
//! The kernel keeps every process's end as a `kernel.process` ledger entry, and its own panic or early fault, found at the next boot, as `kernel.panic`: the message, the file, line and column, the thread, ESR, FAR, PC and LR, and the call chain as offsets into the process's arc, each a typed field (ferros_ledger death).
//! `diag "log deaths <n> <blocks>"` hands over the newest n of them; this names each frame from the ELF the process was built from (the unstripped build output), function and source line, inlined calls included, with the gimli/addr2line crates.
//! A frame is an arc offset; the image starts at the death's `code` offset (G#1000), and payloads and std programs link at 0, so `frame - code` is the ELF's address. A return address is named at the call before it (`- 4`).
//! A masked capsule is named the same way: its ELF is the masked build's (target/masked/<bin>/<bin>.elf), linked after the masking, so the offsets are that ELF's addresses and its line tables come thru ferros-mask's assembly.
//!
//! The ELF must be the build the device ran: nothing on the device names its build yet, so the ELF used and its time are printed with each death, and an ELF newer than the death is flagged.
//!
//! Usage: deaths [n = 16] [--blocks k = 2^10] [--elf <name>=<path>]... [--dir <dir>]... [--kernel <elf>] [--raw] [--from <file.vsf>]

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use ferros_ledger::death::{self, Death};
use ferros_ledger::view::{self, Dead};

/// Where the ELFs are looked for, under the repository (`FERROS_REPO`, else the working directory): the payloads' build, the masked builds, the std programs'.
fn default_dirs(root: &Path) -> Vec<PathBuf> {
    let mut d = vec![root.join("target/el0/aarch64-unknown-none/release"), root.join("target/masked")];
    if let Ok(rd) = std::fs::read_dir(root.join("std")) {
        let mut std: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("target/aarch64-unknown-ferros/release")).filter(|p| p.is_dir()).collect();
        std.sort();
        d.extend(std);
    }
    d
}

/// The names a process's ELF may have: its own, lowercased, and a puck's (`puck_<name>`, the kist named `Compass` is `puck_compass`).
fn bin_names(process: &str) -> Vec<String> {
    let lower = process.to_ascii_lowercase();
    let mut v = vec![process.to_string()];
    if lower != process {
        v.push(lower.clone());
    }
    v.push(format!("puck_{lower}"));
    v
}

/// The ELF for `process`: given by `--elf`, else the first found in `dirs` (a masked build's `<bin>/<bin>.elf`, else `<bin>`).
pub fn find_elf(process: &str, given: &[(String, PathBuf)], dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some((_, p)) = given.iter().find(|(n, _)| n == process) {
        return Some(p.clone());
    }
    for name in bin_names(process) {
        for d in dirs {
            for p in [d.join(&name).join(format!("{name}.elf")), d.join(&name)] {
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// One place a frame resolves to: the function and, when the ELF has line tables, the file and line. An address inside inlined calls gives several, innermost first.
#[derive(Debug, PartialEq)]
pub struct Place {
    pub function: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Names addresses in one ELF.
pub struct Names {
    loader: addr2line::Loader,
}

impl Names {
    pub fn open(path: &Path) -> Option<Names> {
        addr2line::Loader::new(path).ok().map(|loader| Names { loader })
    }

    /// The places at `addr`, innermost first: from the line tables when the ELF has them, else the symbol table's function alone.
    pub fn at(&self, addr: u64) -> Vec<Place> {
        let mut out = Vec::new();
        if let Ok(mut frames) = self.loader.find_frames(addr) {
            while let Ok(Some(f)) = frames.next() {
                let function = f.function.as_ref().and_then(|n| n.demangle().ok()).map(|n| n.into_owned()).unwrap_or_else(|| self.symbol(addr));
                let (file, line) = match f.location {
                    Some(l) => (l.file.map(String::from), l.line),
                    None => (None, None),
                };
                out.push(Place { function, file, line });
            }
        }
        if out.is_empty() {
            let function = self.symbol(addr);
            if !function.is_empty() {
                out.push(Place { function, file: None, line: None });
            }
        }
        out
    }

    fn symbol(&self, addr: u64) -> String {
        self.loader.find_symbol(addr).map(|s| addr2line::demangle_auto(Cow::from(s), None).into_owned()).unwrap_or_default()
    }
}

/// A path shown short: from the repository on, else as it is.
fn short(file: &str) -> &str {
    for mark in ["/ferros/", "/library/", "/registry/src/"] {
        if let Some(i) = file.rfind(mark) {
            return &file[i + mark.len()..];
        }
    }
    file
}

/// The ELF address a frame names: its arc offset less where the image starts; a return address (every frame but the first) at its call, the instruction before.
pub fn elf_address(frame: u64, code: u64, innermost: bool) -> u64 {
    let a = frame.wrapping_sub(code);
    if innermost { a } else { a.wrapping_sub(4) }
}

/// A frame's line: its arc offset, then each place it names.
fn frame_lines(k: usize, frame: u64, d: &Death, names: Option<&Names>, out: &mut Vec<String>) {
    let code = d.code.unwrap_or(0);
    let places = names.map(|n| n.at(elf_address(frame, code, k == 0))).unwrap_or_default();
    if places.is_empty() {
        out.push(format!("    #{k} +G#{frame:X}"));
        return;
    }
    for (j, p) in places.iter().enumerate() {
        let at = match (&p.file, p.line) {
            (Some(f), Some(l)) => format!(" ({}:{l})", short(f)),
            (Some(f), None) => format!(" ({})", short(f)),
            _ => String::new(),
        };
        if j == 0 {
            out.push(format!("    #{k} +G#{frame:X}  {}{at}", p.function));
        } else {
            out.push(format!("         (inlined into) {}{at}", p.function));
        }
    }
}

/// Where the ELFs come from, and the kernel's.
pub struct Elves {
    pub given: Vec<(String, PathBuf)>,
    pub dirs: Vec<PathBuf>,
    pub kernel: PathBuf,
}

/// A death as lines a person reads, its frames named from `elves`.
pub fn render(dd: &Dead, elves: &Elves, wall: &dyn Fn(i64) -> String) -> Vec<String> {
    let d = &dd.death;
    let when = dd.time.map(wall).unwrap_or_else(|| "(no clock yet)".into());
    let mut out = vec![format!("b{} {when} {}: {}", dd.boot, dd.writer, death::end_name(d.end))];
    if let Some(m) = &d.message {
        match (&d.file, d.line, d.column) {
            (Some(f), Some(l), Some(c)) => out.push(format!("  at {f}:{l}:{c}: {m}")),
            _ => out.push(format!("  {m}")),
        }
    }
    if let Some(t) = &d.thread {
        out.push(format!("  thread {t}"));
    }
    if let (Some(esr), Some(far)) = (d.esr, d.far) {
        out.push(format!("  ESR G#{esr:X} (EC G#{:X}), FAR G#{far:X}", esr >> 26 & 0x3F));
    }
    if let Some(b) = d.of_boot {
        out.push(format!("  in boot b{b}, core A#{}, last crumb G#{:X}", d.core.unwrap_or(0), d.crumb.unwrap_or(0)));
    }
    if let Some(e) = d.exit {
        out.push(format!("  exit G#{e:X}"));
    }
    if let Some(w) = &d.last_words {
        out.push(format!("  its last line: {w}"));
    }
    if d.frames.is_empty() && d.lr.is_none() {
        return out;
    }
    // The kernel's own end: its frames are the kernel's addresses, named from the kernel's ELF as they are.
    let kernel = matches!(d.end, death::END_KERNEL_PANIC | death::END_KERNEL_FAULT);
    let elf = if kernel { Some(elves.kernel.clone()).filter(|p| p.is_file()) } else { find_elf(&dd.writer, &elves.given, &elves.dirs) };
    let names = elf.as_deref().and_then(Names::open);
    match (&elf, &names) {
        (Some(p), Some(_)) => {
            let made = std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let made_text = made.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
            // An ELF built after the death is not the build that died: say so (the time is the host's file time against the death's Eagle time).
            let newer = match (made, dd.time) {
                (Some(m), Some(t)) => vsf::EagleTime::from_oscillations(t).to_datetime_opt().is_some_and(|dt| chrono::DateTime::<chrono::Utc>::from(m) > dt),
                _ => false,
            };
            out.push(format!("  frames, named from {} (built {made_text}){}:", p.display(), if newer { "; built AFTER this death, so perhaps not the build that died" } else { "" }));
        }
        (Some(p), None) => out.push(format!("  frames ({} would not open as an ELF; offsets only):", p.display())),
        (None, _) => out.push(format!("  frames (no ELF found for `{}`: --elf {}=<path>; offsets only):", dd.writer, dd.writer)),
    }
    let code = if kernel { 0 } else { d.code.unwrap_or(0) };
    let d0 = Death { code: Some(code), ..d.clone() };
    for (k, &f) in d.frames.iter().enumerate() {
        frame_lines(k, f, &d0, names.as_ref(), &mut out);
    }
    if let Some(lr) = d.lr {
        let mut l = Vec::new();
        frame_lines(1, lr, &d0, names.as_ref(), &mut l);
        for s in l {
            out.push(s.replacen("    #1", "    LR", 1));
        }
    }
    out
}

/// An Eagle time as the host's local wall time, for display only.
fn wall(osc: i64) -> String {
    match vsf::EagleTime::from_oscillations(osc).to_datetime_opt() {
        Some(t) => t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("(Eagle time A#{osc} out of range)"),
    }
}

fn usage() -> ! {
    eprintln!("Usage: ferros-bridge deaths [n = 16] [--blocks k = 1024] [--elf <name>=<path>]... [--dir <dir>]... [--kernel <elf>] [--raw] [--from <file.vsf>]");
    eprintln!("  the newest n deaths in the flash log (the newest k blocks of its ring), each frame named from its process's ELF");
    std::process::exit(1);
}

/// `deaths`.
pub async fn cmd_deaths(args: &[String]) {
    let root = std::env::var("FERROS_REPO").map(PathBuf::from).unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let mut n = 16u32;
    let mut blocks = 1u32 << 10;
    let mut elves = Elves { given: Vec::new(), dirs: Vec::new(), kernel: root.join("target/aarch64-unknown-none/release/ferros_kernel") };
    let (mut raw, mut from) = (false, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--blocks" => blocks = it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--elf" => {
                let v = it.next().unwrap_or_else(|| usage());
                let (name, path) = v.split_once('=').unwrap_or_else(|| usage());
                elves.given.push((name.to_string(), PathBuf::from(path)));
            }
            "--dir" => elves.dirs.push(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--kernel" => elves.kernel = PathBuf::from(it.next().unwrap_or_else(|| usage())),
            "--raw" => raw = true,
            "--from" => from = Some(it.next().unwrap_or_else(|| usage()).clone()),
            v => n = v.parse().unwrap_or_else(|_| usage()),
        }
    }
    elves.dirs.extend(default_dirs(&root));
    let run = match from {
        Some(f) => std::fs::read(&f).unwrap_or_else(|e| {
            eprintln!("{f}: {e}");
            std::process::exit(1);
        }),
        None => {
            let link = crate::usb::UsbLink::open().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            crate::ledger::diag(&link, &format!("log deaths {n} {blocks}")).await
        }
    };
    if raw {
        use std::io::Write;
        let _ = std::io::stdout().write_all(&run);
        return;
    }
    let found = view::deaths(&run);
    if found.is_empty() {
        println!("no deaths in the newest A#{blocks} blocks of the flash log (--blocks for more; the ring is A#16384)");
        return;
    }
    for dd in &found {
        for l in render(dd, &elves, &wall) {
            println!("{l}");
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A function this test names from its own executable's line tables.
    #[inline(never)]
    #[unsafe(no_mangle)]
    pub extern "C" fn ferros_deaths_marker() -> u32 {
        std::hint::black_box(7)
    }

    #[test]
    fn names_a_function_from_its_elf() {
        let exe = std::env::current_exe().unwrap();
        let data = std::fs::read(&exe).unwrap();
        use object::{Object, ObjectSymbol};
        let file = object::File::parse(&*data).unwrap();
        let sym = file.symbols().find(|s| s.name() == Ok("ferros_deaths_marker")).expect("the marker's symbol");
        let names = Names::open(&exe).expect("the test's own ELF opens");
        let places = names.at(sym.address() + 4);
        assert!(!places.is_empty());
        assert!(places[0].function.contains("ferros_deaths_marker"), "{places:?}");
        // The test builds with line tables (cargo test's debug profile): the file is this one.
        assert!(places.iter().any(|p| p.file.as_deref().is_some_and(|f| f.ends_with("deaths.rs"))), "{places:?}");
        assert!(ferros_deaths_marker() == 7);
    }

    #[test]
    fn frames_map_to_elf_addresses() {
        // The innermost frame as it is, every return address at its call; the image at G#1000 in the arc.
        assert_eq!(elf_address(0x1_2344, 0x1000, true), 0x1_1344);
        assert_eq!(elf_address(0x1_2348, 0x1000, false), 0x1_1344);
    }

    #[test]
    fn a_puck_is_found_by_its_bin() {
        let dir = std::env::temp_dir().join(format!("ferros-deaths-{}", std::process::id()));
        let masked = dir.join("masked");
        std::fs::create_dir_all(masked.join("puck_compass")).unwrap();
        std::fs::write(masked.join("puck_compass/puck_compass.elf"), b"x").unwrap();
        let el0 = dir.join("el0");
        std::fs::create_dir_all(&el0).unwrap();
        std::fs::write(el0.join("display"), b"x").unwrap();
        let dirs = vec![el0.clone(), masked.clone()];
        assert_eq!(find_elf("Compass", &[], &dirs), Some(masked.join("puck_compass/puck_compass.elf")));
        assert_eq!(find_elf("display", &[], &dirs), Some(el0.join("display")));
        assert_eq!(find_elf("display", &[("display".into(), PathBuf::from("/x/display"))], &dirs), Some(PathBuf::from("/x/display")));
        assert_eq!(find_elf("nothing", &[], &dirs), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_death_renders_whole() {
        let d = Dead {
            seq: 4,
            boot: 932,
            time: None,
            category: "kernel.process".into(),
            writer: "nothing-by-this-name".into(),
            words: "x".into(),
            death: Death { end: death::END_PANIC, exit: Some(0xDEAD), message: Some("range end index 409600 out of range for slice of length 64".into()), file: Some("payloads/panictest/src/main.rs".into()), line: Some(52), column: Some(14), thread: Some("panictest".into()), frames: vec![0x2010, 0x1100], code: Some(0x1000), ..Default::default() },
        };
        let elves = Elves { given: vec![], dirs: vec![], kernel: PathBuf::from("/nonexistent") };
        let l = render(&d, &elves, &wall);
        assert_eq!(l[0], "b932 (no clock yet) nothing-by-this-name: panicked");
        assert_eq!(l[1], "  at payloads/panictest/src/main.rs:52:14: range end index 409600 out of range for slice of length 64");
        assert!(l.iter().any(|s| s == "    #0 +G#2010"));
        assert!(l.iter().any(|s| s == "    #1 +G#1100"));
    }
}
