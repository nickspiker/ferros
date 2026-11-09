//! flame — where a ferros device's cores spend their time: drives the kernel's sampling profiler (ferros_kernel/src/prof.rs, `diag prof`) over the bridge, reads its rings, names every address from the right ELF, and prints the report: each core's share by process, the hottest functions (self and total), the syscalls that held ticks, and the folded stacks. `--html <file>` also writes a flame graph to look at in a browser (a host-side view: it never goes to the device or into the repo).
//!
//! What the samples mean (prof.rs has the detail): each tick records where its core was. A tick that waited on masked IRQs (every syscall runs masked) is weighed by the periods it waited, and when it waited inside a syscall it is drawn under the caller's own stack, ending in `[syscall NAME]`; time spent in the kernel's masked code is visible as that leaf, not inside the kernel.
//!
//! The ELFs: the kernel's (`--kernel`, default target/aarch64-unknown-none/release/ferros_kernel; it must be the running kernel: the index is found by its `FERROS_PROF_ANCHOR` symbol), and each process's by its name: target/el0/…/<name>, target/masked/<name>/<name>.elf, std/*/target/aarch64-unknown-ferros/release/<name>, or `--elf <name>=<path>`. Each candidate is checked against the code the device runs (the first bytes of the image, read back), and the first that matches is used. Call chains need frame pointers: build with FERROS_PROFILE=1 (tools/lib/profile.sh); without them the report is flat (the sampled function and its caller).
//!
//! Usage:
//!   flame run [seconds = 2] [hz = 4096] [options]   start, wait, stop, report
//!   flame start [hz]                                 start a run (do the thing to measure, then `flame stop`)
//!   flame stop [options]                             stop and report
//!   flame report [options]                           report the last run again (its rings stay on the device until `free`)
//!   flame free                                       give the rings back to the kernel's heap
//! Options: --html <file>, --kernel <elf>, --elf <name>=<elf>, --top <n> (rows per table, 30), --folded (every folded stack, not the top), --core <k> (only that core's samples)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const F_EL2: u64 = 1;
const F_HELD: u64 = 2;
const F_SYSCALL: u64 = 4;
const MAGIC: u64 = u64::from_le_bytes(*b"ferrprof");
/// The kernel's memory read hands back up to 4 MiB a call (service.rs).
const CHUNK: usize = 4 << 20;

// ------------------------------------------------------------------ ELF symbols

/// A function symbol: start, length (0 for an assembly label, which runs to the next symbol), name.
struct Sym {
    addr: u64,
    size: u64,
    name: String,
}
struct Elf {
    path: PathBuf,
    data: Vec<u8>,
    /// (vaddr, file offset, size) of every section with bytes in the file.
    sections: Vec<(u64, u64, u64)>,
    syms: Vec<Sym>,
}

fn rd16(d: &[u8], o: usize) -> Option<u64> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?) as u64)
}
fn rd32(d: &[u8], o: usize) -> Option<u64> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?) as u64)
}
fn rd64(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

impl Elf {
    /// An ELF64 little-endian file's function symbols (.symtab). Every offset in it is the file's own and checked against its length (`get`), since a stale or foreign file is read here as much as a good one.
    fn load(path: &Path) -> Option<Elf> {
        let data = std::fs::read(path).ok()?;
        if data.get(0..4)? != b"\x7fELF" || data[4] != 2 || data[5] != 1 {
            return None;
        }
        let shoff = rd64(&data, 0x28)? as usize;
        let shentsize = rd16(&data, 0x3A)? as usize;
        let shnum = rd16(&data, 0x3C)? as usize;
        let sh = |i: usize| shoff + i * shentsize;
        let mut sections = Vec::new();
        let mut exec = vec![false; shnum];
        let mut symtab = None;
        for i in 0..shnum {
            let o = sh(i);
            let (ty, flags, addr, off, size, link) = (rd32(&data, o + 4)?, rd64(&data, o + 8)?, rd64(&data, o + 16)?, rd64(&data, o + 24)?, rd64(&data, o + 32)?, rd32(&data, o + 40)?);
            // SHT_PROGBITS with SHF_ALLOC: bytes the image carries.
            if ty == 1 && flags & 2 != 0 {
                sections.push((addr, off, size));
            }
            exec[i] = flags & 4 != 0;
            if ty == 2 {
                symtab = Some((off as usize, size as usize, link as usize));
            }
        }
        let mut syms = Vec::new();
        if let Some((off, size, link)) = symtab {
            let so = sh(link);
            let (str_off, str_size) = (rd64(&data, so + 24)? as usize, rd64(&data, so + 32)? as usize);
            let strs = data.get(str_off..str_off + str_size)?;
            for e in (off..off + size).step_by(24) {
                let (name, info, shndx, value, sz) = (rd32(&data, e)? as usize, data[e + 4], rd16(&data, e + 6)? as usize, rd64(&data, e + 8)?, rd64(&data, e + 16)?);
                let ty = info & 0xF;
                // Functions, and untyped labels in code (the kernel's assembly entry points); not the `$x`/`$d` mapping symbols.
                if !(ty == 2 || ty == 0 && shndx < exec.len() && exec[shndx]) || name == 0 {
                    continue;
                }
                let n = strs.get(name..).and_then(|s| s.split(|&c| c == 0).next()).map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default();
                if n.is_empty() || n.starts_with('$') {
                    continue;
                }
                syms.push(Sym { addr: value, size: sz, name: format!("{:#}", rustc_demangle::demangle(&n)) });
            }
        }
        syms.sort_by_key(|s| (s.addr, std::cmp::Reverse(s.size)));
        syms.dedup_by_key(|s| s.addr);
        Some(Elf { path: path.to_path_buf(), data, sections, syms })
    }
    /// The function holding `a`, if any.
    fn sym(&self, a: u64) -> Option<&str> {
        let i = self.syms.partition_point(|s| s.addr <= a).checked_sub(1)?;
        let s = &self.syms[i];
        let end = if s.size != 0 { s.addr + s.size } else { self.syms.get(i + 1).map_or(u64::MAX, |n| n.addr) };
        (a < end).then_some(s.name.as_str())
    }
    fn addr_of(&self, name: &str) -> Option<u64> {
        self.syms.iter().find(|s| s.name == name).map(|s| s.addr).or_else(|| self.raw_addr_of(name))
    }
    /// Any symbol by its exact (mangled) name, objects included: the anchor is data.
    fn raw_addr_of(&self, want: &str) -> Option<u64> {
        let d = &self.data;
        let shoff = rd64(d, 0x28)? as usize;
        let shentsize = rd16(d, 0x3A)? as usize;
        let shnum = rd16(d, 0x3C)? as usize;
        for i in 0..shnum {
            let o = shoff + i * shentsize;
            if rd32(d, o + 4)? != 2 {
                continue;
            }
            let (off, size, link) = (rd64(d, o + 24)? as usize, rd64(d, o + 32)? as usize, rd32(d, o + 40)? as usize);
            let so = shoff + link * shentsize;
            let strs = d.get(rd64(d, so + 24)? as usize..)?;
            for e in (off..off + size).step_by(24) {
                let name = rd32(d, e)? as usize;
                if strs.get(name..name + want.len() + 1).is_some_and(|s| &s[..want.len()] == want.as_bytes() && s[want.len()] == 0) {
                    return rd64(d, e + 8);
                }
            }
        }
        None
    }
    /// The image's bytes at `vaddr`, from the file.
    fn bytes_at(&self, vaddr: u64, n: usize) -> Option<&[u8]> {
        let &(a, off, _) = self.sections.iter().find(|&&(a, _, size)| vaddr >= a && vaddr + n as u64 <= a + size)?;
        let o = (off + vaddr - a) as usize;
        self.data.get(o..o + n)
    }
}

// ------------------------------------------------------------------ the bridge

struct Bridge {
    exe: PathBuf,
    tmp: PathBuf,
}
impl Bridge {
    fn diag(&self, args: &str) -> String {
        let o = Command::new(&self.exe).args(["diag", args]).output().unwrap_or_else(|e| die(&format!("ferros-bridge: {e}")));
        String::from_utf8_lossy(&o.stdout).into_owned()
    }
    /// `len` bytes of the device's memory at `addr` (both multiples of 8, DRAM), in reads of up to 4 MiB.
    fn read(&self, addr: u64, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let want = (len - out.len()).min(CHUNK);
            let st = Command::new(&self.exe).args(["read", &format!("{:X}", addr + out.len() as u64), &format!("{want:X}"), self.tmp.to_str().unwrap()]).output().unwrap_or_else(|e| die(&format!("ferros-bridge: {e}")));
            let got = std::fs::read(&self.tmp).unwrap_or_default();
            if !st.status.success() || got.is_empty() {
                die(&format!("read G#{:X}: {}", addr + out.len() as u64, String::from_utf8_lossy(&st.stderr).lines().last().unwrap_or("")));
            }
            out.extend_from_slice(&got[..got.len().min(len - out.len())]);
        }
        let _ = std::fs::remove_file(&self.tmp);
        out
    }
}

fn die(m: &str) -> ! {
    eprintln!("flame: {m}");
    std::process::exit(1)
}

// ------------------------------------------------------------------ the index (prof.rs `pull`) and the rings

#[derive(Default)]
struct Index {
    hz: u64,
    cntfrq: u64,
    period: u64,
    started: u64,
    stopped: u64,
    run: u64,
    head: u64,
    cores: Vec<CoreInfo>,
    threads: Vec<ThreadInfo>,
}
#[derive(Default, Clone)]
struct CoreInfo {
    core: u64,
    ring: u64,
    words: u64,
    cap: u64,
    ticks: u64,
    samples: u64,
    held: u64,
    dropped: u64,
    cost: u64,
}
#[derive(Clone)]
struct ThreadInfo {
    thread: u64,
    epoch: u64,
    base: u64,
    process: String,
    name: String,
}

// The schemas as prof.rs builds them.
fn prof_schema() -> vsf::schema::SectionSchema {
    use vsf::schema::TypeConstraint::AnyUnsigned;
    vsf::schema::SectionSchema::new("prof").field("hz", AnyUnsigned).field("cntfrq", AnyUnsigned).field("period", AnyUnsigned).field("started", AnyUnsigned).field("stopped", AnyUnsigned).field("run", AnyUnsigned).field("depth", AnyUnsigned).field("head", AnyUnsigned)
}
fn core_schema() -> vsf::schema::SectionSchema {
    use vsf::schema::TypeConstraint::AnyUnsigned;
    vsf::schema::SectionSchema::new("core").field("core", AnyUnsigned).field("ring", AnyUnsigned).field("words", AnyUnsigned).field("cap", AnyUnsigned).field("ticks", AnyUnsigned).field("samples", AnyUnsigned).field("held", AnyUnsigned).field("dropped", AnyUnsigned).field("cost", AnyUnsigned)
}
fn thread_schema() -> vsf::schema::SectionSchema {
    use vsf::schema::TypeConstraint::{AnyUnsigned, AsciiText};
    vsf::schema::SectionSchema::new("thread").field("thread", AnyUnsigned).field("epoch", AnyUnsigned).field("base", AnyUnsigned).field("process", AsciiText).field("name", AsciiText)
}

fn parse_index(doc: &[u8]) -> Result<Index, String> {
    use vsf::schema::SectionBuilder;
    let (header, _) = vsf::verification::read_verified(doc, None)?;
    let mut ix = Index::default();
    // WHY checked: the offset and size are the document's own (read from the device); their sum may wrap to an end that passes `get`.
    let body = |f: &vsf::file_format::HeaderField| doc.get(f.offset_bytes..f.offset_bytes.checked_add(f.size_bytes).ok_or(())?).ok_or(());
    for f in &header.fields {
        let Ok(b) = body(f) else { return Err(format!("section {} outside the document", f.name)) };
        match f.name.as_str() {
            "prof" => {
                let s = SectionBuilder::parse(prof_schema(), b).map_err(|e| format!("{e:?}"))?;
                let g = |n: &str| s.get_value::<u64>(n).unwrap_or(0);
                (ix.hz, ix.cntfrq, ix.period, ix.started, ix.stopped, ix.run, ix.head) = (g("hz"), g("cntfrq"), g("period"), g("started"), g("stopped"), g("run"), g("head"));
            }
            "core" => {
                let s = SectionBuilder::parse(core_schema(), b).map_err(|e| format!("{e:?}"))?;
                let g = |n: &str| s.get_value::<u64>(n).unwrap_or(0);
                ix.cores.push(CoreInfo { core: g("core"), ring: g("ring"), words: g("words"), cap: g("cap"), ticks: g("ticks"), samples: g("samples"), held: g("held"), dropped: g("dropped"), cost: g("cost") });
            }
            "thread" => {
                let s = SectionBuilder::parse(thread_schema(), b).map_err(|e| format!("{e:?}"))?;
                let g = |n: &str| s.get_value::<u64>(n).unwrap_or(0);
                let t = |n: &str| s.get_value::<String>(n).unwrap_or_default();
                ix.threads.push(ThreadInfo { thread: g("thread"), epoch: g("epoch"), base: g("base"), process: t("process"), name: t("name") });
            }
            _ => {}
        }
    }
    if ix.cntfrq == 0 || ix.period == 0 || ix.head == 0 {
        return Err("the index has no prof section".into());
    }
    Ok(ix)
}

/// One record of a ring (prof.rs, "A ring").
#[derive(Clone, Debug, PartialEq)]
struct Sample {
    core: u64,
    t: u64,
    flags: u64,
    nr: u64,
    weight: u64,
    thread: u64,
    epoch32: u64,
    addrs: Vec<u64>,
}

fn decode(core: u64, words: &[u64], head: usize) -> (Vec<Sample>, bool) {
    let mut out = Vec::new();
    let mut i = 0;
    while i + head <= words.len() {
        let meta = words[i + 1];
        let n = (meta & 0xFF) as usize;
        if i + head + n > words.len() {
            return (out, false);
        }
        let w2 = words[i + 2];
        out.push(Sample { core, t: words[i], flags: meta >> 8 & 0xFF, nr: meta >> 16 & 0xFFFF, weight: meta >> 32, thread: w2 & 0xFFFF_FFFF, epoch32: w2 >> 32, addrs: words[i + head..i + head + n].to_vec() });
        i += head + n;
    }
    (out, i == words.len())
}

// ------------------------------------------------------------------ names

/// The syscall numbers (ferros_kernel/src/process.rs SYS_*).
fn syscall_name(nr: u64) -> String {
    let n = match nr {
        0 => "exit",
        1 => "call",
        2 => "recv",
        3 => "reply",
        4 => "mmio_read",
        5 => "mmio_write",
        6 => "entropy",
        7 => "dma",
        8 => "input",
        9 => "sleep",
        10 => "spawn",
        11 => "set_pref",
        12 => "facts",
        13 => "store",
        14 => "thread_spawn",
        15 => "thread_exit",
        16 => "wait",
        17 => "wake",
        18 => "irq_wait",
        19 => "irq_ack",
        20 => "pin",
        21 => "light",
        22 => "power",
        23 => "ira",
        24 => "unlock",
        25 => "pt",
        26 => "sensors",
        27 => "status",
        28 => "pet",
        29 => "block",
        30 => "vault",
        31 => "tracks",
        32 => "control",
        33 => "controls",
        34 => "detail",
        0xFFFF => return "[trap: not a syscall]".into(),
        _ => return format!("[syscall A#{nr}]"),
    };
    format!("[syscall {n}]")
}

struct Names<'a> {
    kernel: &'a Elf,
    procs: BTreeMap<String, Option<Elf>>,
}
impl Names<'_> {
    fn elf(&self, process: &str) -> Option<&Elf> {
        self.procs.get(process).and_then(|e| e.as_ref())
    }
}

/// A function name for an address in `elf` (`ret`: a return address, named by the call before it).
fn name(elf: Option<&Elf>, a: u64, ret: bool, tag: &str) -> String {
    let look = if ret { a.wrapping_sub(4) } else { a };
    match elf.and_then(|e| e.sym(look)) {
        Some(n) => n.replace(';', ":"),
        None => format!("{tag}+G#{a:X}"),
    }
}

/// A sample's stack, outermost first: its root (the process, `kernel` or `idle`), the thread when it is not the process's first (or `[for …]`, the thread the kernel ran for), the functions, and a marker leaf for held time. With the number of leading entries that are not functions.
fn stack(s: &Sample, ix: &Index, names: &Names) -> (Vec<String>, usize) {
    let ti = ix.threads.iter().find(|t| t.thread + 1 == s.thread && t.epoch & 0xFFFF_FFFF == s.epoch32);
    let process = ti.map(|t| t.process.clone()).unwrap_or_else(|| if s.thread != 0 { format!("thread A#{}", s.thread - 1) } else { String::new() });
    let user = s.flags & F_SYSCALL != 0 || s.flags & F_EL2 == 0;
    let (elf, tag) = if user { (names.elf(&process), process.as_str()) } else { (Some(names.kernel), "kernel") };
    let a = &s.addrs;
    let mut frames: Vec<String> = Vec::new();
    // Innermost first while building: the sampled function, its caller from the link register when the frame records do not already name it (a leaf, or a prologue), then each frame's.
    let pc = a.first().copied().unwrap_or(0);
    let leaf = name(elf, pc, false, tag);
    frames.push(leaf.clone());
    if let Some(&lr) = a.get(1).filter(|&&lr| lr != 0) {
        let lr_name = name(elf, lr, true, tag);
        if lr_name != leaf && a.get(2) != Some(&lr) {
            frames.push(lr_name);
        }
    }
    for &r in a.iter().skip(2) {
        frames.push(name(elf, r, true, tag));
    }
    frames.reverse();
    let mut out = Vec::new();
    if user {
        out.push(if process.is_empty() { "?".into() } else { process.clone() });
        if let Some(t) = ti.filter(|t| t.name != t.process) {
            out.push(t.name.clone());
        }
    } else {
        let idle = leaf == "ferros_core_main" && s.thread == 0;
        out.push(if idle { "idle".into() } else { "kernel".into() });
        if let Some(t) = ti {
            out.push(format!("[for {}]", t.name));
        }
    }
    let prefix = out.len();
    out.extend(frames);
    if s.flags & F_SYSCALL != 0 {
        out.push(syscall_name(s.nr));
    } else if s.flags & F_HELD != 0 {
        out.push("[irqs masked]".into());
    }
    (out, prefix)
}

// ------------------------------------------------------------------ the report

struct Report {
    total: u64,
    /// Folded stack → weight.
    folded: BTreeMap<String, u64>,
    by_core: BTreeMap<u64, BTreeMap<String, u64>>,
    selfw: BTreeMap<String, u64>,
    totalw: BTreeMap<String, u64>,
    syscalls: BTreeMap<String, u64>,
    trees: Node,
}
#[derive(Default)]
struct Node {
    w: u64,
    kids: BTreeMap<String, Node>,
}

fn pct(w: u64, total: u64) -> String {
    let p = if total != 0 { w * 1000 / total } else { 0 };
    format!("A#{}.{} %", p / 10, p % 10)
}
fn top(m: &BTreeMap<String, u64>, n: usize) -> Vec<(&String, u64)> {
    let mut v: Vec<(&String, u64)> = m.iter().map(|(k, &w)| (k, w)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.truncate(n);
    v
}

fn build(samples: &[Sample], ix: &Index, names: &Names) -> Report {
    let mut r = Report { total: 0, folded: BTreeMap::new(), by_core: BTreeMap::new(), selfw: BTreeMap::new(), totalw: BTreeMap::new(), syscalls: BTreeMap::new(), trees: Node::default() };
    for s in samples {
        let (st, prefix) = stack(s, ix, names);
        let w = s.weight;
        r.total += w;
        *r.folded.entry(st.join(";")).or_default() += w;
        *r.by_core.entry(s.core).or_default().entry(st[0].clone()).or_default() += w;
        // Self: the innermost real function (a marker leaf names the one before it). Total: every function on the stack, once.
        let fns: Vec<String> = st.iter().skip(prefix).filter(|f| !f.starts_with('[')).map(|f| format!("{}`{f}", st[0])).collect();
        if let Some(l) = fns.last() {
            *r.selfw.entry(l.clone()).or_default() += w;
        }
        let mut once = std::collections::BTreeSet::new();
        for f in fns {
            if once.insert(f.clone()) {
                *r.totalw.entry(f).or_default() += w;
            }
        }
        if s.flags & F_SYSCALL != 0 {
            *r.syscalls.entry(format!("{} {}", st[0], syscall_name(s.nr))).or_default() += w;
        }
        let mut n = &mut r.trees;
        n.w += w;
        for f in &st {
            n = n.kids.entry(f.clone()).or_default();
            n.w += w;
        }
    }
    r
}

fn print_report(r: &Report, ix: &Index, samples_read: usize, top_n: usize, all_folded: bool) {
    let f = ix.cntfrq;
    // A run is pulled only once stopped (prof.rs refuses while it runs), and the counter only climbs: stopped >= started.
    let span = ix.stopped - ix.started;
    println!("== run A#{}: A#{} Hz, A#{} ms, A#{samples_read} records, A#{} periods in all (A#{} ms of core time)", ix.run, ix.hz, span * 1000 / f, r.total, r.total * ix.period * 1000 / f);
    println!("\n-- cores (handler cost: its own measure, from its first statement to its last)");
    for c in &ix.cores {
        let ns = if c.ticks != 0 { c.cost * 1_000_000_000 / f / c.ticks } else { 0 };
        let share = if span != 0 { c.cost * 10_000 / span } else { 0 };
        println!("core A#{}: A#{} records, A#{} held, A#{} periods dropped (ring full), ring A#{}/A#{} words, handler A#{ns} ns a tick = A#{}.{:02} % of the core", c.core, c.samples, c.held, c.dropped, c.words, c.cap, share / 100, share % 100);
        if let Some(m) = r.by_core.get(&c.core) {
            let t: u64 = m.values().sum();
            let line: Vec<String> = top(m, 6).iter().map(|(k, w)| format!("{k} {}", pct(*w, t))).collect();
            println!("    {}", line.join(", "));
        }
    }
    let mut roots: BTreeMap<String, u64> = BTreeMap::new();
    for (k, &w) in &r.folded {
        *roots.entry(k.split(';').next().unwrap_or("").to_string()).or_default() += w;
    }
    println!("\n-- by process (all cores; a core's whole time is one core's worth)");
    for (k, w) in top(&roots, top_n) {
        println!("{:>10}  {:>9} ms  {k}", pct(w, r.total), format!("A#{}", w * ix.period * 1000 / f));
    }
    println!("\n-- functions, self (the innermost function of each sample)");
    for (k, w) in top(&r.selfw, top_n) {
        println!("{:>10}  {k}", pct(w, r.total));
    }
    println!("\n-- functions, total (on the stack at all)");
    for (k, w) in top(&r.totalw, top_n) {
        println!("{:>10}  {k}", pct(w, r.total));
    }
    if !r.syscalls.is_empty() {
        println!("\n-- time inside syscalls (ticks held by masked IRQs, weighed by the periods they waited)");
        for (k, w) in top(&r.syscalls, top_n) {
            println!("{:>10}  {:>9} ms  {k}", pct(w, r.total), format!("A#{}", w * ix.period * 1000 / f));
        }
    }
    println!("\n-- folded stacks{} (weight in periods)", if all_folded { "" } else { ", the heaviest" });
    let v = top(&r.folded, if all_folded { usize::MAX } else { top_n });
    for (k, w) in v {
        println!("{k} {w}");
    }
}

// ------------------------------------------------------------------ the flame graph

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '<' => o.push_str("\\u003c"),
            c if (c as u32) < 32 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
fn node_json(name: &str, n: &Node, out: &mut String) {
    out.push_str(&format!("[{},{},[", json_str(name), n.w));
    let mut kids: Vec<(&String, &Node)> = n.kids.iter().collect();
    kids.sort_by(|a, b| b.1.w.cmp(&a.1.w).then(a.0.cmp(b.0)));
    for (i, (k, c)) in kids.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        node_json(k, c, out);
    }
    out.push_str("]]");
}

fn write_html(path: &Path, r: &Report, ix: &Index) {
    let mut data = String::new();
    node_json("all", &r.trees, &mut data);
    let ms_per = ix.period as f64 * 1000. / ix.cntfrq as f64;
    let title = format!("ferros flame: run A#{}, A#{} Hz", ix.run, ix.hz);
    let html = HTML.replace("@TITLE@", &title).replace("@DATA@", &data).replace("@MSPER@", &format!("{ms_per}"));
    std::fs::write(path, html).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    eprintln!("flame graph: {}", path.display());
}

const HTML: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>@TITLE@</title>
<style>
:root { --bg: #f7f5f2; --fg: #1d1b19; --mute: #6b655e; --line: #d9d3cb; --hi: #1d1b19; }
@media (prefers-color-scheme: dark) { :root { --bg: #141312; --fg: #ece8e2; --mute: #9a938a; --line: #2e2b28; --hi: #ffffff; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 13px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; }
header { padding: 12px 16px; display: flex; gap: 12px; align-items: center; flex-wrap: wrap; border-bottom: 1px solid var(--line); }
h1 { font-size: 14px; margin: 0; font-weight: 600; }
input { font: inherit; padding: 4px 8px; border: 1px solid var(--line); background: transparent; color: var(--fg); border-radius: 4px; min-width: 16em; }
button { font: inherit; padding: 4px 10px; border: 1px solid var(--line); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
#info { color: var(--mute); padding: 6px 16px; min-height: 2.2em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
#graph { position: relative; margin: 0 16px 16px; overflow: hidden; }
.f { position: absolute; height: 17px; overflow: hidden; white-space: nowrap; font-size: 11px; line-height: 17px; padding: 0 3px; border-right: 1px solid var(--bg); border-bottom: 1px solid var(--bg); cursor: pointer; color: #1a1209; }
.f.dim { opacity: 0.35; }
.f.match { outline: 2px solid var(--hi); outline-offset: -2px; }
</style></head><body>
<header><h1>@TITLE@</h1><input id="q" placeholder="search (regex)"><button id="reset">whole run</button></header>
<div id="info">click a frame to zoom into it; the root is every sampled period</div>
<div id="graph"></div>
<script>
const MSPER = @MSPER@;
const DATA = @DATA@;
const H = 18;
let root = DATA, q = null;
function hue(name, top) {
  if (top === "all") return "hsl(0,0%,78%)";
  if (top === "idle") return "hsl(0,0%,72%)";
  if (name.startsWith("[syscall") || name.startsWith("[irqs")) return "hsl(200,55%,68%)";
  if (top === "kernel") return "hsl(28,85%,64%)";
  let h = 0; for (const c of top) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return "hsl(" + (40 + h % 80) + ",70%," + (58 + (name.length % 7)) + "%)";
}
function ann(n, top) { n[3] = top; for (const k of n[2]) ann(k, top === undefined ? k[0] : top); }
ann(DATA, undefined);
function depth(n) { let d = 0; for (const k of n[2]) d = Math.max(d, depth(k)); return d + 1; }
function draw() {
  const g = document.getElementById("graph");
  g.innerHTML = "";
  const W = g.clientWidth, D = depth(root);
  g.style.height = (D * H) + "px";
  const total = root[1];
  const re = q ? new RegExp(q) : null;
  function place(n, x, y) {
    const w = n[1] / total * W;
    if (w < 0.5) return;
    const d = document.createElement("div");
    d.className = "f" + (re ? (re.test(n[0]) ? " match" : " dim") : "");
    d.style.left = x + "px"; d.style.width = w + "px"; d.style.bottom = y + "px";
    d.style.background = hue(n[0], n[3] || n[0]);
    d.textContent = w > 24 ? n[0] : "";
    const info = n[0] + "  " + (n[1] * MSPER).toFixed(2) + " ms, " + (n[1] * 100 / DATA[1]).toFixed(1) + " % of the run";
    d.title = info;
    d.onmouseenter = () => document.getElementById("info").textContent = info;
    d.onclick = () => { root = n; draw(); };
    g.appendChild(d);
    let cx = x;
    for (const k of n[2]) { place(k, cx, y + H); cx += k[1] / total * W; }
  }
  place(root, 0, 0);
}
document.getElementById("q").oninput = e => { q = e.target.value || null; try { draw(); } catch (_) {} };
document.getElementById("reset").onclick = () => { root = DATA; draw(); };
window.onresize = draw;
draw();
</script></body></html>
"##;

// ------------------------------------------------------------------ main

struct Opts {
    html: Option<PathBuf>,
    kernel: PathBuf,
    elfs: Vec<(String, PathBuf)>,
    top: usize,
    folded: bool,
    core: Option<u64>,
    rest: Vec<String>,
}

fn opts(repo: &Path, a: &[String]) -> Opts {
    let mut o = Opts { html: None, kernel: repo.join("target/aarch64-unknown-none/release/ferros_kernel"), elfs: Vec::new(), top: 30, folded: false, core: None, rest: Vec::new() };
    let mut i = 0;
    while i < a.len() {
        let v = a.get(i + 1).cloned();
        match a[i].as_str() {
            "--html" => o.html = v.map(PathBuf::from),
            "--kernel" => o.kernel = v.map(PathBuf::from).unwrap_or(o.kernel),
            "--elf" => {
                if let Some((n, p)) = v.as_deref().and_then(|s| s.split_once('=')) {
                    o.elfs.push((n.to_string(), PathBuf::from(p)));
                }
            }
            "--top" => o.top = v.and_then(|s| s.parse().ok()).unwrap_or(o.top),
            "--core" => o.core = v.and_then(|s| s.trim_start_matches("A#").parse().ok()),
            "--folded" => {
                o.folded = true;
                i += 1;
                continue;
            }
            s => {
                o.rest.push(s.to_string());
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    o
}

/// Where a process's ELF may be, in the order tried.
fn candidates(repo: &Path, name: &str, o: &Opts) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = o.elfs.iter().filter(|(n, _)| n == name).map(|(_, p)| p.clone()).collect();
    v.push(repo.join("target/el0/aarch64-unknown-none/release").join(name));
    v.push(repo.join("target/masked").join(name).join(format!("{name}.elf")));
    if let Ok(d) = std::fs::read_dir(repo.join("std")) {
        for e in d.flatten() {
            v.push(e.path().join("target/aarch64-unknown-ferros/release").join(name));
        }
    }
    v.into_iter().filter(|p| p.is_file()).collect()
}

/// The ELF whose first bytes are the code the device runs at `base` (read back), or the first candidate, unverified.
fn pick(bridge: &Bridge, base: u64, cands: Vec<PathBuf>) -> (Option<Elf>, &'static str) {
    let n = 256;
    let dev = if base >= 0x8000_0000 && base % 8 == 0 { Some(bridge.read(base, n)) } else { None };
    let mut first = None;
    for p in cands {
        let Some(e) = Elf::load(&p) else { continue };
        if let Some(d) = &dev {
            if e.bytes_at(0, n).is_some_and(|b| b == &d[..]) {
                return (Some(e), "matches the device");
            }
        }
        if first.is_none() {
            first = Some(e);
        }
    }
    (first, "NOT the code on the device")
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bridge = Bridge { exe: repo.join("target/debug/ferros-bridge"), tmp: std::env::temp_dir().join(format!("flame-{}.part", std::process::id())) };
    let o = opts(&repo, &a);
    let cmd = o.rest.first().map(String::as_str).unwrap_or("");
    match cmd {
        "start" => {
            print!("{}", bridge.diag(&format!("prof start {}", o.rest.get(1).map(String::as_str).unwrap_or(""))));
            return;
        }
        "free" => {
            print!("{}", bridge.diag("prof free"));
            return;
        }
        "run" => {
            let secs: f64 = o.rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.);
            eprint!("{}", bridge.diag(&format!("prof start {}", o.rest.get(2).map(String::as_str).unwrap_or(""))));
            std::thread::sleep(std::time::Duration::from_secs_f64(secs));
            eprint!("{}", bridge.diag("prof stop"));
        }
        "stop" => eprint!("{}", bridge.diag("prof stop")),
        "report" => eprint!("{}", bridge.diag("prof pull")),
        _ => {
            eprintln!("{}", include_str!("main.rs").lines().take_while(|l| l.starts_with("//!")).map(|l| l.trim_start_matches("//!").trim_start_matches(' ')).collect::<Vec<_>>().join("\n"));
            std::process::exit(2);
        }
    }
    let kernel = Elf::load(&o.kernel).unwrap_or_else(|| die(&format!("no kernel ELF at {}", o.kernel.display())));
    let anchor_at = kernel.addr_of("FERROS_PROF_ANCHOR").unwrap_or_else(|| die("the kernel ELF has no FERROS_PROF_ANCHOR: not a profiling kernel"));
    // The kernel's first code bytes against the device's: the anchor's address is only right for the running kernel.
    if let Some(lo) = kernel.addr_of("_start") {
        let n = 256;
        let ok = kernel.bytes_at(lo, n).is_some_and(|b| bridge.read(lo, n) == b);
        eprintln!("kernel {}: {}", kernel.path.display(), if ok { "matches the device" } else { "does NOT match the device's code; the report may be wrong" });
    }
    let anchor = bridge.read(anchor_at, 32);
    let w = |i: usize| u64::from_le_bytes(anchor[i * 8..i * 8 + 8].try_into().unwrap());
    if w(0) != MAGIC {
        die("no index on the device (prof stop or prof pull first)");
    }
    let (at, len) = (w(1), w(2) as usize);
    let doc = bridge.read(at, (len + 7) & !7);
    let ix = parse_index(&doc[..len]).unwrap_or_else(|e| die(&format!("index: {e}")));
    let mut samples = Vec::new();
    for c in &ix.cores {
        if o.core.is_some_and(|k| k != c.core) || c.words == 0 {
            continue;
        }
        let raw = bridge.read(c.ring, c.words as usize * 8);
        let words: Vec<u64> = raw.chunks_exact(8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).collect();
        let (s, whole) = decode(c.core, &words, ix.head as usize);
        if !whole {
            eprintln!("core A#{}: the ring ends inside a record (kept A#{} records)", c.core, s.len());
        }
        samples.extend(s);
    }
    // Each process's ELF, chosen by the code at its image base.
    let mut procs: BTreeMap<String, Option<Elf>> = BTreeMap::new();
    let mut bases: BTreeMap<String, u64> = BTreeMap::new();
    // The latest spawn of each name (the highest epoch): a process restarted during the run has its old arc gone.
    let mut by_epoch: Vec<&ThreadInfo> = ix.threads.iter().collect();
    by_epoch.sort_by_key(|t| t.epoch);
    for t in by_epoch {
        bases.insert(t.process.clone(), t.base);
    }
    let used: std::collections::BTreeSet<(u64, u64)> = samples.iter().map(|s| (s.thread, s.epoch32)).collect();
    for (p, &base) in &bases {
        let live = ix.threads.iter().any(|t| &t.process == p && used.contains(&(t.thread + 1, t.epoch & 0xFFFF_FFFF)));
        if !live {
            continue;
        }
        let c = candidates(&repo, p, &o);
        if c.is_empty() {
            eprintln!("{p}: no ELF found (--elf {p}=<path>)");
            procs.insert(p.clone(), None);
            continue;
        }
        let (e, how) = pick(&bridge, base, c);
        if let Some(e) = &e {
            eprintln!("{p}: {} ({how})", e.path.display());
        }
        procs.insert(p.clone(), e);
    }
    let names = Names { kernel: &kernel, procs };
    let r = build(&samples, &ix, &names);
    print_report(&r, &ix, samples.len(), o.top, o.folded);
    if let Some(h) = &o.html {
        write_html(h, &r, &ix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index as prof.rs builds it (the same schemas and builder calls), parsed back.
    #[test]
    fn index_round_trip() {
        let u = |v: u64| vsf::VsfType::u(v as usize, false);
        let head = prof_schema().build().set("hz", u(4096)).unwrap().set("cntfrq", u(24_576_000)).unwrap().set("period", u(6000)).unwrap().set("started", u(1000)).unwrap().set("stopped", u(2000)).unwrap().set("run", u(3)).unwrap().set("depth", u(32)).unwrap().set("head", u(3)).unwrap().encode().unwrap();
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(1).provenance_only().add_unboxed("prof", head);
        for c in [4u64, 8] {
            let s = core_schema().build().set("core", u(c)).unwrap().set("ring", u(0x8260_0000 + c)).unwrap().set("words", u(77)).unwrap().set("cap", u(1 << 17)).unwrap().set("ticks", u(9)).unwrap().set("samples", u(8)).unwrap().set("held", u(1)).unwrap().set("dropped", u(0)).unwrap().set("cost", u(123)).unwrap().encode().unwrap();
            b = b.add_unboxed("core", s);
        }
        for (t, n) in [(5u64, "display"), (9, "display.3")] {
            let s = thread_schema().build().set("thread", u(t)).unwrap().set("epoch", u(0x1_0000_0007)).unwrap().set("base", u(0x8301_1000)).unwrap().set("process", vsf::VsfType::a("display".into())).unwrap().set("name", vsf::VsfType::a(n.into())).unwrap().encode().unwrap();
            b = b.add_unboxed("thread", s);
        }
        let doc = b.build().unwrap();
        let ix = parse_index(&doc).unwrap();
        assert_eq!((ix.hz, ix.cntfrq, ix.period, ix.run, ix.head), (4096, 24_576_000, 6000, 3, 3));
        assert_eq!(ix.cores.iter().map(|c| c.core).collect::<Vec<_>>(), vec![4, 8]);
        assert_eq!(ix.cores[1].ring, 0x8260_0008);
        assert_eq!(ix.threads[1].name, "display.3");
        assert_eq!(ix.threads[0].epoch, 0x1_0000_0007);
    }

    /// Records as prof.rs writes them, decoded; a ring cut inside a record keeps the whole ones.
    #[test]
    fn ring_decode() {
        let rec = |t: u64, flags: u64, nr: u64, weight: u64, thread: u64, epoch: u64, addrs: &[u64]| {
            let mut v = vec![t, addrs.len() as u64 | flags << 8 | nr << 16 | weight << 32, thread & 0xFFFF_FFFF | epoch << 32];
            v.extend_from_slice(addrs);
            v
        };
        let mut words = rec(10, F_EL2, 0, 1, 0, 0, &[0x8010_0000, 0x8010_0100, 0x8020_0000]);
        words.extend(rec(11, F_HELD | F_SYSCALL, 29, 37, 6, 7, &[0x40, 0x80]));
        let (s, whole) = decode(8, &words, 3);
        assert!(whole);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1], Sample { core: 8, t: 11, flags: F_HELD | F_SYSCALL, nr: 29, weight: 37, thread: 6, epoch32: 7, addrs: vec![0x40, 0x80] });
        let (s, whole) = decode(8, &words[..words.len() - 1], 3);
        assert!(!whole);
        assert_eq!(s.len(), 1);
    }

    /// Symbols from a real ELF: the kernel's, when it is built.
    #[test]
    fn kernel_symbols() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/aarch64-unknown-none/release/ferros_kernel");
        let Some(e) = Elf::load(&p) else { return };
        let Some(a) = e.addr_of("kernel_main") else { panic!("no kernel_main") };
        assert_eq!(e.sym(a + 4), Some("kernel_main"));
        assert!(e.bytes_at(a, 16).is_some());
    }
}

#[cfg(test)]
mod synthetic {
    use super::*;

    /// A report and a flame graph from made-up records over the real kernel and display ELFs (when built): every root, the link-register rule, a syscall leaf and the idle root come out as prof.rs's records mean them. FLAME_HTML=<file> keeps the graph to look at.
    #[test]
    fn report_from_records() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (Some(k), Some(d)) = (Elf::load(&repo.join("target/aarch64-unknown-none/release/ferros_kernel")), Elf::load(&repo.join("target/el0/aarch64-unknown-none/release/display"))) else { return };
        let ka = |n: &str| k.addr_of(n).unwrap_or_else(|| panic!("kernel has no {n}"));
        // Two display functions and their callers, by the ELF's own addresses (offsets: the image links at 0).
        let mut fs: Vec<&Sym> = d.syms.iter().filter(|s| s.size > 64).collect();
        fs.sort_by_key(|s| std::cmp::Reverse(s.size));
        let (f0, f1, f2) = ((fs[0].addr, fs[0].name.clone()), (fs[1].addr, fs[1].name.clone()), (fs[2].addr, fs[2].name.clone()));
        drop(fs);
        let ix = Index { hz: 4096, cntfrq: 24_576_000, period: 6000, started: 0, stopped: 24_576_000, run: 1, head: 3, cores: vec![CoreInfo { core: 8, ..Default::default() }, CoreInfo { core: 4, ..Default::default() }], threads: vec![ThreadInfo { thread: 5, epoch: 0x2_0000_0009, base: 0x8301_1000, process: "display".into(), name: "display".into() }, ThreadInfo { thread: 7, epoch: 0x2_0000_0009, base: 0x8301_1000, process: "display".into(), name: "display.2".into() }] };
        let s = |core, flags, nr, weight, thread, addrs: Vec<u64>| Sample { core, t: 0, flags, nr, weight, thread, epoch32: 9, addrs };
        let samples = vec![
            // A leaf at EL0: pc in f0, lr into f1 (not in the frame records), then f2's frame.
            s(8, 0, 0, 1, 6, vec![f0.0 + 8, f1.0 + 12, f2.0 + 16]),
            // A non-leaf: the link register names f0 itself (a call it made), dropped.
            s(8, 0, 0, 1, 8, vec![f0.0 + 8, f0.0 + 40, f1.0 + 12]),
            // Held in a syscall (block), weighed 37 periods.
            s(4, F_HELD | F_SYSCALL, 29, 37, 8, vec![f1.0 + 20, f2.0 + 20]),
            // The kernel, and a worker idle in its loop.
            s(8, F_EL2, 0, 1, 0, vec![ka("kernel_main") + 4, ka("kernel_main") + 8]),
            s(4, F_EL2, 0, 2, 0, vec![ka("ferros_core_main") + 4, 0]),
        ];
        let mut procs = BTreeMap::new();
        procs.insert("display".to_string(), Some(d));
        let names = Names { kernel: &k, procs };
        let st: Vec<Vec<String>> = samples.iter().map(|x| stack(x, &ix, &names).0).collect();
        assert_eq!(st[0], vec!["display".to_string(), f2.1.clone(), f1.1.clone(), f0.1.clone()]);
        assert_eq!(st[1], vec!["display".to_string(), "display.2".into(), f1.1.clone(), f0.1.clone()]);
        assert_eq!(st[2].last().unwrap(), "[syscall block]");
        assert_eq!(st[3][0], "kernel");
        assert_eq!(st[4][0], "idle");
        let r = build(&samples, &ix, &names);
        assert_eq!(r.total, 42);
        assert_eq!(r.syscalls.values().sum::<u64>(), 37);
        print_report(&r, &ix, samples.len(), 10, true);
        if let Ok(p) = std::env::var("FLAME_HTML") {
            write_html(Path::new(&p), &r, &ix);
        }
    }
}
