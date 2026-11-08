//! `ferros-bridge input …`: drive the device's touch and keys from the host, as `adb shell input` drives Android, in ferros's terms (docs/INPUT-INJECT.md). Each command builds a script (ferros_inject: a complete VSF document, positions as fractions of the panel, time in shown frames), prints it, and sends it: its touch parts through the inject process (payloads/inject, spawned with `use touch` and `use display`), its key presses through the kernel's development `diag key`. Development builds only: a production kernel refuses the spawn and the diag, a production touch process has no injection op.

use ferros_inject::{At, Event, Ev, Part, Script, ONE};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A tap's press, in frames: the map reads touch at a thirtieth of a second at rest (four frames at 120 Hz), so a shorter press could come and go between two of its reads.
const TAP_HOLD: u32 = 1 << 3;
/// A swipe's, a drag's, a pinch's and a turn's frames when none are given.
const MOVE_FRAMES: u32 = 1 << 4;
/// A drag's still frames at each end: the hold that takes a circle at the start, and the stop before the lift that leaves no fling.
const DRAG_HOLD: u32 = 1 << 5;
/// Every moving gesture rests this many frames where it landed before it moves, as a finger does: the map reads touch a thirtieth of a second apart at rest (four frames), and a finger already moving when it is first seen is first seen somewhere else (the home corner's swipe was read inside the app's circle, 2026-11-08).
const LAND: u32 = 1 << 3;

pub fn usage() {
    eprintln!("ferros-bridge input — fingers and keys in shown frames (docs/INPUT-INJECT.md)");
    eprintln!("  A position is two fractions of the panel's width and height (0.5 0.25) or two panel pixels (672px 748px).");
    eprintln!("  tap X Y [HOLD]                     a finger down and up where it landed (HOLD frames, default 8)");
    eprintln!("  hold X Y FRAMES                    a finger held still");
    eprintln!("  swipe X0 Y0 X1 Y1 [FRAMES]         down, a straight line over FRAMES (16), up at once: the fling is the consumer's");
    eprintln!("  drag X0 Y0 X1 Y1 [FRAMES] [HOLD]   down, held HOLD frames (32), the line, held HOLD frames again, up: no fling");
    eprintln!("  pinch CX CY R0 R1 [FRAMES]         two fingers either side of the centre, R0 to R1 apart from it (fractions of the width)");
    eprintln!("  rotate CX CY R DEGREES [FRAMES]    two fingers R from the centre (fraction of the width), turned DEGREES (clockwise positive)");
    eprintln!("  key power                          a press of the power key, through the kernel (the screen off and on; the wake policy applies)");
    eprintln!("  idle                               wait until the display is idle");
    eprintln!("  shot [out.vsf]                     the screen as tools/screencap reads it (a VSF visual)");
    eprintln!("  script FILE.vsf                    send a script document as it is (keys cut out and sent between its parts)");
    eprintln!("  Options after a gesture: --idle (then wait for idle), --shot [out.vsf] (then a screencap, after idle), --save FILE (write the document, send nothing), --dry (print it only)");
}

fn fail(msg: &str) -> ! {
    eprintln!("input: {msg}");
    std::process::exit(1);
}

/// A position from two words: fractions (`0.5`), or pixels (`672px`) when both end in px.
fn at(a: &str, b: &str) -> At {
    match (a.strip_suffix("px"), b.strip_suffix("px")) {
        (Some(x), Some(y)) => match (x.parse::<u16>(), y.parse::<u16>()) {
            (Ok(x), Ok(y)) => At::Px(x, y),
            _ => fail(&format!("pixels {a} {b}: whole numbers")),
        },
        (None, None) => At::Frac(frac(a), frac(b)),
        _ => fail(&format!("{a} {b}: both fractions or both pixels")),
    }
}

/// A fraction of a side in Q16, from a person's decimal.
fn frac(s: &str) -> u16 {
    let v: f64 = s.parse().unwrap_or_else(|_| fail(&format!("{s}: a fraction of the panel (0.5), or pixels (672px)")));
    // WHY: a fraction is on the panel only from 0 up to, not including, its far edge; anything else is a typo, refused rather than pinned to an edge.
    if !(0. ..1.).contains(&v) {
        fail(&format!("{s}: a fraction from 0 up to 1, not including 1"));
    }
    (v * ONE as f64) as u16
}

fn num(s: Option<&String>, default: u32, what: &str) -> u32 {
    match s {
        Some(s) => s.parse().unwrap_or_else(|_| fail(&format!("{what} {s}: a whole number of frames"))),
        None => default,
    }
}

/// A movement's frames: at least one, since a line or a turn is drawn a step a frame.
fn moving(s: Option<&String>) -> u32 {
    let n = num(s, MOVE_FRAMES, "frames");
    // WHY: each step is the distance over the frame count; zero frames is no movement to divide.
    if n == 0 {
        fail("a movement takes at least one frame");
    }
    n
}

fn ev(after: u32, ev: Ev) -> Event {
    Event { after, ev }
}

/// A straight line from `a` to `b` for finger `f`: a move each frame for `frames` frames, the last one at `b`.
fn line(f: u8, a: At, b: At, frames: u32, out: &mut Vec<Event>) {
    for k in 1..=frames {
        let t = |p: u16, q: u16| (p as i64 + (q as i64 - p as i64) * k as i64 / frames as i64) as u16;
        let p = match (a, b) {
            (At::Frac(x0, y0), At::Frac(x1, y1)) => At::Frac(t(x0, x1), t(y0, y1)),
            (At::Px(x0, y0), At::Px(x1, y1)) => At::Px(t(x0, x1), t(y0, y1)),
            _ => fail("a line's two ends in the same units"),
        };
        out.push(ev(1, Ev::Move { finger: f, at: p }));
    }
}

/// The panel's width and height from the display server (op 5 through payloads/capture), for the gestures that place two fingers by a radius: a fraction of the width across is that many pixels up and down too.
fn panel(bridge: &Path) -> (f64, f64) {
    let bin = payload(bridge, "capture");
    let answer = |l: &str| !l.is_empty() && l.chars().all(|c| c.is_ascii_hexdigit());
    // Asked again when another reader took the answer off the console (`spawn`), up to three times.
    let mut hex = String::new();
    for _ in 0..3 {
        if let Some(h) = spawn(bridge, &bin, "05", "panel", &["--use", "display"], 8, answer).into_iter().find(|l| answer(l)) {
            hex = h;
            break;
        }
    }
    // [w u16][h u16]… little-endian, as hex.
    let byte = |i: usize| hex.get(i * 2..i * 2 + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    match (byte(0), byte(1), byte(2), byte(3)) {
        (Some(a), Some(b), Some(c), Some(d)) => (u16::from_le_bytes([a, b]) as f64, u16::from_le_bytes([c, d]) as f64),
        _ => fail("the display server did not say the panel's size"),
    }
}

/// Two fingers about a centre for `frames` frames: finger k at angle θ(t) + k·π, radius r(t) (fractions of the width), the y offset scaled by the panel's aspect so the path is round on the glass.
fn two(c: (u16, u16), aspect: f64, r: impl Fn(f64) -> f64, th: impl Fn(f64) -> f64, frames: u32, out: &mut Vec<Event>) {
    let place = |t: f64, k: u8| {
        let a = th(t) + k as f64 * std::f64::consts::PI;
        let x = c.0 as f64 + r(t) * a.cos() * ONE as f64;
        let y = c.1 as f64 + r(t) * a.sin() * aspect * ONE as f64;
        // WHY: a finger the radius puts off the panel is a gesture this panel cannot take; refused, not slid along the edge.
        if !(0. ..ONE as f64).contains(&x) || !(0. ..ONE as f64).contains(&y) {
            fail("a finger would leave the panel: a smaller radius, or a centre further in");
        }
        At::Frac(x as u16, y as u16)
    };
    out.push(ev(0, Ev::Down { finger: 0, at: place(0., 0) }));
    out.push(ev(0, Ev::Down { finger: 1, at: place(0., 1) }));
    for k in 1..=frames {
        let t = k as f64 / frames as f64;
        out.push(ev(if k == 1 { 1 + LAND } else { 1 }, Ev::Move { finger: 0, at: place(t, 0) }));
        out.push(ev(0, Ev::Move { finger: 1, at: place(t, 1) }));
    }
    out.push(ev(1, Ev::Up { finger: 0 }));
    out.push(ev(0, Ev::Up { finger: 1 }));
}

/// A payload built and turned into its flat binary, as tools/screencap builds capture.
fn payload(_bridge: &Path, name: &str) -> PathBuf {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rel = repo.join("target/el0/aarch64-unknown-none/release");
    let ok = Command::new("cargo").current_dir(&repo).args(["el0", "-q", "-p", &format!("ferros-payload-{name}")]).output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        fail(&format!("the {name} payload did not build (cargo el0 -p ferros-payload-{name})"));
    }
    let bin = rel.join(format!("{name}.bin"));
    let _ = Command::new("aarch64-linux-gnu-objcopy").args(["-O", "binary"]).arg(rel.join(name)).arg(&bin).status();
    bin
}

/// The script in a person's words, a line an event, with the frame each lands on.
fn print(s: &Script) {
    let mut f: u64 = 0;
    let at = |a: At| match a {
        At::Frac(x, y) => format!("x G#{x:04X} y G#{y:04X} of G#10000"),
        At::Px(x, y) => format!("pixel A#{x}, A#{y}"),
    };
    for e in &s.events {
        f += e.after as u64;
        let what = match e.ev {
            Ev::Down { finger, at: a } => format!("finger A#{finger} down at {}", at(a)),
            Ev::Move { finger, at: a } => format!("finger A#{finger} to {}", at(a)),
            Ev::Up { finger } => format!("finger A#{finger} up"),
            Ev::Key { key } => format!("key {}", ferros_inject::key::NAMES[key as usize]),
            Ev::Wait { idle: true } => String::from("wait until the display is idle"),
            Ev::Wait { idle: false } => String::from("wait"),
        };
        println!("  frame A#{f}: {what}");
    }
}

/// Send a script: its touch parts through the inject process, its keys through the kernel. Returns whether every part was taken.
fn send(bridge: &Path, s: &Script, doc_as_is: Option<&[u8]>) -> bool {
    let parts = s.parts();
    let mut inject = None;
    for p in &parts {
        match p {
            Part::Key(k) => {
                let name = ["power", "volup", "voldown"][*k as usize];
                let out = Command::new(bridge).args(["diag", &format!("key {name}")]).output().unwrap_or_else(|e| fail(&format!("diag: {e}")));
                let text = String::from_utf8_lossy(&out.stdout).into_owned();
                println!("key {name}: {}", text.trim());
                if !text.contains("pressed") {
                    return false;
                }
            }
            Part::Touch(t) => {
                // A document with no key in it goes as it came; a part is a document of its own.
                let doc = match doc_as_is.filter(|_| parts.len() == 1) {
                    Some(d) => d.to_vec(),
                    None => t.build(vsf::eagle_time_oscillations()).unwrap_or_else(|e| fail(&format!("the script: {e}"))),
                };
                let bin = inject.get_or_insert_with(|| payload(bridge, "inject")).clone();
                let hex: String = doc.iter().map(|b| format!("{b:02X}")).collect();
                // A frame budget's worth of seconds, plus the idle waits' longest (1024 frames, under 9 s).
                let idles = t.events.iter().filter(|e| matches!(e.ev, Ev::Wait { idle: true })).count() as u64;
                let secs = t.frames() / 120 + 8 + idles * 9;
                let said = spawn(bridge, &bin, &hex, "inject", &["--use", "touch", "--use", "display"], secs, |l| l.starts_with("done") || l.starts_with("refused"));
                for l in &said {
                    println!("inject: {l}");
                }
                if said.iter().any(|l| l == GONE) {
                    println!("inject: whether every event was taken is not known");
                } else if !said.iter().any(|l| l.starts_with("done")) {
                    if said.is_empty() {
                        println!("inject: no word from the process");
                    }
                    return false;
                }
            }
        }
    }
    true
}

/// Spawn `bin` named `name` with `input` (hex) and `grants`, and collect its console lines until one satisfies `last`. A process that waits on another (the display's frames, its answer) parks, and the spawn comes back before it is done: the rest is read off the console, an eighth of a second apart (the first reader takes a line, so this one asks often), for up to `secs` seconds.
fn spawn(bridge: &Path, bin: &Path, input: &str, name: &str, grants: &[&str], secs: u64, last: impl Fn(&str) -> bool) -> Vec<String> {
    let out = Command::new(bridge).arg("spawn").arg(bin).arg(input).args(["--name", name]).args(grants).output().unwrap_or_else(|e| fail(&format!("spawn: {e}")));
    if !out.status.success() {
        fail(&format!("spawn {name}: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")));
    }
    let mut said = lines(&String::from_utf8_lossy(&out.stdout), name);
    // The console is one queue for every reader: another session's `diag console` takes these lines off it as readily as this one (2026-11-08, three answers lost while the other agents polled). So the process itself is watched too: once it has ended, one more read, and a missing answer is said to be missing rather than waited on.
    let tag = format!(" {name} (");
    for k in 0..secs * 8 {
        if said.iter().any(|l| last(l)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(125));
        if let Ok(o) = Command::new(bridge).args(["diag", "console"]).output() {
            said.extend(lines(&String::from_utf8_lossy(&o.stdout), name));
        }
        if k % 8 == 7 && !said.iter().any(|l| last(l)) {
            let alive = Command::new(bridge).args(["diag", "threads"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains(&tag)).unwrap_or(true);
            if !alive {
                if let Ok(o) = Command::new(bridge).args(["diag", "console"]).output() {
                    said.extend(lines(&String::from_utf8_lossy(&o.stdout), name));
                }
                if !said.iter().any(|l| last(l)) {
                    said.push(String::from(GONE));
                }
                break;
            }
        }
    }
    said
}

/// What `spawn` reports for a process that ended with its answer read off the console by someone else.
const GONE: &str = "ended; its report was taken off the console by another reader";

/// One process's own console lines (the kernel prefixes each with its name), out of everything the console carried. Reading the console takes every process's lines off it, so the others go to stderr rather than nowhere.
fn lines(text: &str, name: &str) -> Vec<String> {
    let mut own = Vec::new();
    let tag = format!("{name}: ");
    for l in text.lines() {
        match l.split(tag.as_str()).nth(1) {
            Some(s) => own.push(String::from(s)),
            None if l.contains(": ") && !l.starts_with(' ') && !l.starts_with("ALIVE") => eprintln!("console: {l}"),
            None => {}
        }
    }
    own
}

/// tools/screencap, built if it is not: the screen as a VSF visual.
fn shot(out: Option<&String>) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = repo.join("tools/screencap/Cargo.toml");
    let mut c = Command::new("cargo");
    c.args(["run", "-q", "--release", "--manifest-path"]).arg(&manifest).arg("--");
    if let Some(o) = out {
        c.arg(o);
    }
    match c.status() {
        Ok(s) if s.success() => {}
        _ => fail("screencap failed"),
    }
}

pub async fn cmd(args: &[String]) {
    let bridge = std::env::current_exe().unwrap_or_else(|e| fail(&format!("own path: {e}")));
    let Some(verb) = args.first() else {
        usage();
        std::process::exit(1);
    };
    // The options, then the gesture's own words.
    let mut words: Vec<String> = Vec::new();
    let (mut idle, mut take_shot, mut shot_out, mut save, mut dry) = (false, false, None, None, false);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--idle" => idle = true,
            "--dry" => dry = true,
            "--save" => {
                save = args.get(i + 1).cloned();
                i += 1;
            }
            "--shot" => {
                take_shot = true;
                if let Some(o) = args.get(i + 1).filter(|o| !o.starts_with("--")) {
                    shot_out = Some(o.clone());
                    i += 1;
                }
            }
            w => words.push(w.to_string()),
        }
        i += 1;
    }
    let w = |k: usize| words.get(k).map(String::as_str).unwrap_or_else(|| fail(&format!("{verb}: too few words (ferros-bridge input, for the list)")));
    let mut ev_list: Vec<Event> = Vec::new();
    let mut as_is: Option<Vec<u8>> = None;
    match verb.as_str() {
        "tap" => {
            ev_list.push(ev(0, Ev::Down { finger: 0, at: at(w(0), w(1)) }));
            ev_list.push(ev(num(words.get(2), TAP_HOLD, "hold"), Ev::Up { finger: 0 }));
        }
        "hold" => {
            ev_list.push(ev(0, Ev::Down { finger: 0, at: at(w(0), w(1)) }));
            ev_list.push(ev(num(Some(&w(2).to_string()), 0, "frames"), Ev::Up { finger: 0 }));
        }
        "swipe" | "drag" => {
            let (a, b) = (at(w(0), w(1)), at(w(2), w(3)));
            let frames = moving(words.get(4));
            let hold = if verb == "drag" { num(words.get(5), DRAG_HOLD, "hold") } else { 0 };
            // A drag's hold is its landing; a swipe lands for LAND frames.
            let land = if verb == "drag" { hold } else { LAND };
            ev_list.push(ev(0, Ev::Down { finger: 0, at: a }));
            let first = ev_list.len();
            line(0, a, b, frames, &mut ev_list);
            ev_list[first].after += land;
            ev_list.push(ev(if hold > 0 { hold } else { 1 }, Ev::Up { finger: 0 }));
        }
        "pinch" | "rotate" => {
            let At::Frac(cx, cy) = at(w(0), w(1)) else { fail("a pinch's or a turn's centre in fractions") };
            let (pw, ph) = panel(&bridge);
            let aspect = pw / ph;
            let fr = |s: &str| frac(s) as f64 / ONE as f64;
            if verb == "pinch" {
                let (r0, r1) = (fr(w(2)), fr(w(3)));
                let frames = moving(words.get(4));
                two((cx, cy), aspect, |t| r0 + (r1 - r0) * t, |_| 0., frames, &mut ev_list);
            } else {
                let r = fr(w(2));
                let deg: f64 = w(3).parse().unwrap_or_else(|_| fail("degrees: a number"));
                let frames = moving(words.get(4));
                two((cx, cy), aspect, |_| r, |t| deg.to_radians() * t, frames, &mut ev_list);
            }
        }
        "key" => {
            let k = match w(0) {
                "power" => ferros_inject::key::POWER,
                "volup" => ferros_inject::key::VOL_UP,
                "voldown" => ferros_inject::key::VOL_DOWN,
                other => fail(&format!("key {other}: power, volup or voldown")),
            };
            ev_list.push(ev(0, Ev::Key { key: k as u8 }));
        }
        "idle" => ev_list.push(ev(0, Ev::Wait { idle: true })),
        "shot" => {
            shot(words.first());
            return;
        }
        "script" => {
            let doc = std::fs::read(w(0)).unwrap_or_else(|e| fail(&format!("{}: {e}", w(0))));
            let s = Script::parse(&doc).unwrap_or_else(|e| fail(&format!("{}: {e}", w(0))));
            ev_list = s.events;
            as_is = Some(doc);
        }
        _ => {
            usage();
            std::process::exit(1);
        }
    }
    if idle || take_shot {
        ev_list.push(ev(0, Ev::Wait { idle: true }));
        // The document is no longer the one given.
        as_is = None;
    }
    let script = Script { text: std::iter::once(verb.as_str()).chain(words.iter().map(String::as_str)).collect::<Vec<_>>().join(" "), events: ev_list };
    if let Err(e) = script.check() {
        fail(&e);
    }
    println!("input: {} (A#{} events, A#{} frames)", script.text, script.events.len(), script.frames());
    print(&script);
    if let Some(path) = save {
        let doc = match &as_is {
            Some(d) => d.clone(),
            None => script.build(vsf::eagle_time_oscillations()).unwrap_or_else(|e| fail(&e)),
        };
        std::fs::write(&path, &doc).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
        println!("saved: {path} (A#{} bytes)", doc.len());
        return;
    }
    if dry {
        return;
    }
    if !send(&bridge, &script, as_is.as_deref()) {
        std::process::exit(1);
    }
    if take_shot {
        shot(shot_out.as_ref());
    }
}
