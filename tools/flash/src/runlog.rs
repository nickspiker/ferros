//! The run log (the owner, 2026-11-24: "making sure the install process, OS and all of that is logged in proper VSF so that at any point the user can submit a log. I suspect this to be a bumpy road.").
//! One file a run on the person's computer, a stream of complete VSF records in the stack's log shape (vsf::logging: section `log` with `lvl`, a `msg` template and typed `val`s), so `vsflog` reads it as it reads Photon's.
//! Each record also carries the step it belongs to and typed fields of its own (a partition, a size, a hash, a time taken): numbers and hashes stay binary, words stay words.
//! Every record reaches the disk before the step it describes goes on (`sync_data` after each append), so a crash, a pulled cable or a power cut mid-run leaves every record before it; a torn last record is skipped by the reader.
//! What it never holds: a handle, a key, a location, or the contents of a calibration partition (only their names, sizes and BLAKE3).
//! `report` wraps a run's records into ONE VSF document for submitting: the bytes the person's own disk holds, unchanged, with the report's facts beside them.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use vsf::{VsfBuilder, VsfSection, VsfType};

/// Severity, the same numbers as vsf::logging::LogLevel (the on-disk `lvl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info = 2,
    Warn = 3,
    Error = 4,
}

/// One record being built: a step, a template in words, typed values for its `{}` slots, and named typed fields.
pub struct Ev {
    level: Level,
    step: String,
    msg: String,
    vals: Vec<VsfType>,
    fields: Vec<(String, VsfType)>,
}

fn ascii_or_text(s: &str) -> VsfType {
    match vsf::types::Ascii::new(s) {
        Ok(a) => VsfType::a(a),
        Err(_) => VsfType::x(s.to_string()),
    }
}

impl Ev {
    pub fn new(level: Level, step: &str, msg: &str) -> Ev {
        Ev {
            level,
            step: step.into(),
            msg: msg.into(),
            vals: Vec::new(),
            fields: Vec::new(),
        }
    }
    pub fn info(step: &str, msg: &str) -> Ev {
        Ev::new(Level::Info, step, msg)
    }
    pub fn warn(step: &str, msg: &str) -> Ev {
        Ev::new(Level::Warn, step, msg)
    }
    pub fn error(step: &str, msg: &str) -> Ev {
        Ev::new(Level::Error, step, msg)
    }
    /// A number for the template's next `{}`.
    pub fn n(mut self, v: u64) -> Ev {
        self.vals.push(VsfType::u(v as usize, false));
        self
    }
    /// Words for the template's next `{}` (an error's own text, a bootloader's answer).
    pub fn t(mut self, v: &str) -> Ev {
        self.vals.push(VsfType::x(v.to_string()));
        self
    }
    /// A named unsigned field.
    pub fn u(mut self, name: &str, v: u64) -> Ev {
        self.fields
            .push((name.into(), VsfType::u(v as usize, false)));
        self
    }
    /// A named BLAKE3 hash.
    pub fn hash(mut self, name: &str, h: &[u8; 32]) -> Ev {
        self.fields.push((name.into(), VsfType::hb(h.to_vec())));
        self
    }
    /// A named identifier (a partition, a variable, a product): ASCII as `a`, anything else as text.
    pub fn id(mut self, name: &str, v: &str) -> Ev {
        self.fields.push((name.into(), ascii_or_text(v)));
        self
    }
    /// Named words (a bootloader's message, an error's context).
    pub fn words(mut self, name: &str, v: &str) -> Ev {
        self.fields.push((name.into(), VsfType::x(v.to_string())));
        self
    }
    /// A named flag.
    pub fn flag(mut self, name: &str, v: bool) -> Ev {
        self.fields.push((name.into(), VsfType::u0(v)));
        self
    }

    /// The record: one complete VSF document, its creation time the moment it was made.
    pub fn record(&self, at: i64) -> Result<Vec<u8>, String> {
        let mut s = VsfSection::new("log");
        s.set_uint("lvl", self.level as u64);
        s.set_text("msg", &self.msg);
        for v in &self.vals {
            s.add_field("val", v.clone());
        }
        s.add_field("step", ascii_or_text(&self.step));
        for (n, v) in &self.fields {
            s.add_field(n.as_str(), v.clone());
        }
        VsfBuilder::new()
            .creation_time_oscillations(at)
            .provenance_only()
            .add_section_direct(s)
            .build()
    }
}

/// The log of one run, open for appending.
pub struct RunLog {
    path: PathBuf,
    file: File,
}

/// Where this computer keeps the runs: under the person's own application data, never beside the executable.
pub fn default_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library/Application Support"))
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("ferros-flash").join("runs")
}

impl RunLog {
    /// A new run's file in `dir`, named by the run's start in Eagle time (hex) and opened with the run's facts as its first record.
    pub fn create(dir: &Path, start: i64, facts: Ev) -> Result<RunLog, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{start:016x}.log.vsf"));
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut l = RunLog { path, file };
        l.append_at(&facts, start)?;
        Ok(l)
    }

    /// An existing run's file, opened to append (a send records its consent and its answer in the run it sent).
    pub fn append_to(path: &Path) -> Result<RunLog, String> {
        let file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(RunLog {
            path: path.to_path_buf(),
            file,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appended and on the disk before this returns.
    pub fn append(&mut self, ev: &Ev) -> Result<(), String> {
        self.append_at(ev, vsf::eagle_time_oscillations())
    }

    fn append_at(&mut self, ev: &Ev, at: i64) -> Result<(), String> {
        let r = ev.record(at)?;
        self.file
            .write_all(&r)
            .and_then(|_| self.file.sync_data())
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// One record read back: when, how severe, its step, its rendered words, and the record itself.
#[derive(Clone, Debug)]
pub struct Read {
    pub osc: i64,
    pub level: u64,
    pub step: String,
    pub msg: String,
    pub raw: Vec<u8>,
}

/// Every whole record of a run's bytes; a torn last record (a crash mid-append) is left out, and the bytes it starts at are returned.
pub fn read(bytes: &[u8]) -> (Vec<Read>, usize) {
    let (recs, whole) = vsf::logging::parse_log_records(bytes);
    let out = recs
        .into_iter()
        .map(|r| {
            let step = vsf::VsfHeader::decode(&r.raw).ok().and_then(|(_, end)| {
                let mut p = end;
                VsfSection::parse(&r.raw, &mut p)
                    .ok()
                    .and_then(|s| s.text("step").map(str::to_string))
            });
            Read {
                osc: r.osc,
                level: r.level,
                step: step.unwrap_or_default(),
                msg: r.msg,
                raw: r.raw,
            }
        })
        .collect();
    (out, whole)
}

/// The report a person submits: one VSF document holding the run's whole records exactly as their disk holds them, with the report's own facts.
/// `keep_serial`: the phone's serial is the only identifier a run holds; unless the person keeps it in, the report carries its keyed BLAKE3 instead.
pub fn report(run: &[u8], tool: &str, made: i64, keep_serial: bool) -> Result<Vec<u8>, String> {
    let (recs, whole) = read(run);
    let mut s = VsfSection::new("report");
    s.add_field("tool", ascii_or_text(tool));
    s.set_uint("records", recs.len() as u64);
    s.add_field("torn_tail", VsfType::u0(whole < run.len()));
    s.add_field("serial_kept", VsfType::u0(keep_serial));
    // Each record whole, as raw bytes (`v` b): it is itself a complete VSF document a reader opens on its own.
    for r in &recs {
        s.add_field(
            "record",
            VsfType::v(
                b'b',
                if keep_serial {
                    r.raw.clone()
                } else {
                    without_serial(&r.raw)?
                },
            ),
        );
    }
    VsfBuilder::new()
        .creation_time_oscillations(made)
        .provenance_only()
        .add_section_direct(s)
        .build()
}

/// Where a person's report goes (docs/FLASHER.md "Submitting"): fgtw.org's `/report`, which keeps it 2^5 days and never the sender's address.
pub const REPORT_URL: &str = "https://fgtw.org/report";

/// The newest run on this computer, the one a person most likely means.
pub fn newest(dir: &Path) -> Option<PathBuf> {
    let mut runs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".log.vsf"))
        .collect();
    // The names are the runs' start times in hex, all the same width: their order is the runs' order.
    runs.sort();
    runs.pop()
}

/// A report's records back as a run log (the bytes the sender's disk held, serial blinding aside), for `log show` and `vsflog`.
pub fn unpack(report: &[u8]) -> Result<Vec<u8>, String> {
    vsf::verification::is_original(report)?;
    let (h, end) = vsf::VsfHeader::decode_strict(report)?;
    let secs = h.sections(report, end)?;
    let r = secs
        .iter()
        .find(|s| s.name == "report")
        .ok_or("not a report")?;
    let mut out = Vec::new();
    for f in r.get_fields("record") {
        match f.values.first() {
            Some(VsfType::v(b'b', rec)) => out.extend_from_slice(rec),
            _ => return Err("a record that is not raw bytes".into()),
        }
    }
    Ok(out)
}

/// fgtw.org's answer to a report: its BLAKE3 when taken, else the refusal's reason and words.
pub fn reply(bytes: &[u8]) -> Result<[u8; 32], String> {
    let (h, end) =
        vsf::VsfHeader::decode(bytes).map_err(|e| format!("the answer is not VSF: {e}"))?;
    let secs = h.sections(bytes, end)?;
    let s = secs.first().ok_or("an empty answer")?;
    match s.name.as_str() {
        "report_taken" => s
            .bytes32("hash")
            .ok_or_else(|| "the answer names no hash".to_string()),
        "error" => Err(format!(
            "refused ({}): {}",
            s.text("reason").unwrap_or("?"),
            s.text("detail").unwrap_or("")
        )),
        n => Err(format!("an answer named `{n}`")),
    }
}

/// A record with its `serial` field replaced by the serial's keyed BLAKE3 (the same phone still reads as the same phone within a report, and no further).
fn without_serial(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (h, end) = vsf::VsfHeader::decode(raw)?;
    // By the TOC, so the section keeps its name (a body alone may be written nameless, its name in the TOC).
    let mut s = h
        .sections(raw, end)?
        .into_iter()
        .next()
        .ok_or("a record with no section")?;
    if s.get_field("serial").is_none() {
        return Ok(raw.to_vec());
    }
    for f in s.fields.iter_mut().filter(|f| f.name == "serial") {
        let v = f
            .values
            .first()
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();
        let k = blake3::derive_key("ferros-flash report serial", v.as_bytes());
        f.name = "serial_blind".into();
        f.values = vec![VsfType::hb(k.to_vec())];
    }
    let at = h
        .creation_time
        .as_ref()
        .and_then(|v| v.as_eagle())
        .unwrap_or(0);
    VsfBuilder::new()
        .creation_time_oscillations(at)
        .provenance_only()
        .add_section_direct(s)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ferros-flash-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn records_survive_a_torn_tail() {
        let d = scratch("torn");
        let mut l = RunLog::create(
            &d,
            1 << 40,
            Ev::info("start", "ferros-flash {} on {}")
                .t("0.1.0")
                .t("windows-x86_64"),
        )
        .unwrap();
        l.append(
            &Ev::info("device", "found {}")
                .t("Pixel 8a")
                .id("product", "akita")
                .id("serial", "TESTSERIAL0"),
        )
        .unwrap();
        l.append(
            &Ev::info("flash", "sent {} bytes to {}")
                .n(1 << 26)
                .t("boot_b")
                .id("part", "boot_b")
                .u("bytes", 1 << 26)
                .hash("blake3", &[3; 32])
                .u("ms", 812),
        )
        .unwrap();
        l.append(
            &Ev::error("flash", "the bootloader answered FAIL: {}").t("Write to device failed"),
        )
        .unwrap();
        let path = l.path().to_path_buf();
        drop(l);
        let bytes = std::fs::read(&path).unwrap();
        let (all, whole) = read(&bytes);
        assert_eq!(whole, bytes.len());
        assert_eq!(all.len(), 4);
        assert_eq!(all[2].msg, "sent 67108864 bytes to boot_b");
        assert_eq!(all[2].step, "flash");
        assert_eq!(all[3].level, Level::Error as u64);
        // A crash mid-append: the last record cut short.
        let cut = &bytes[..bytes.len() - 9];
        let (some, whole) = read(cut);
        assert_eq!(some.len(), 3);
        assert!(whole < cut.len());
        // What fgtw.org's /report checks of every record: a whole document by its provenance hash.
        for r in &all {
            vsf::verification::is_original(&r.raw).unwrap();
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn replies() {
        let taken = VsfBuilder::new()
            .creation_time_oscillations(1)
            .add_section(
                "report_taken",
                vec![
                    ("hash".into(), VsfType::hb(vec![5; 32])),
                    ("keep_days".into(), VsfType::u(32, false)),
                ],
            )
            .build()
            .unwrap();
        assert_eq!(reply(&taken).unwrap(), [5; 32]);
        let refused = VsfBuilder::new()
            .add_section(
                "error",
                vec![
                    (
                        "reason".into(),
                        VsfType::a(vsf::types::Ascii::new("bad_report").unwrap()),
                    ),
                    (
                        "detail".into(),
                        VsfType::a(vsf::types::Ascii::new("no").unwrap()),
                    ),
                ],
            )
            .build()
            .unwrap();
        assert!(reply(&refused).unwrap_err().contains("bad_report"));
    }

    #[test]
    fn report_is_one_document_and_blinds_the_serial() {
        let d = scratch("report");
        let mut l = RunLog::create(&d, 1 << 41, Ev::info("start", "run")).unwrap();
        l.append(&Ev::info("device", "found").id("serial", "TESTSERIAL0"))
            .unwrap();
        let run = std::fs::read(l.path()).unwrap();
        let doc = report(&run, "ferros-flash 0.1.0", 1 << 42, false).unwrap();
        let (h, end) = vsf::VsfHeader::decode_strict(&doc).unwrap();
        let secs = h.sections(&doc, end).unwrap();
        assert_eq!(secs[0].uint("records"), Some(2));
        assert!(!doc.windows(11).any(|w| w == b"TESTSERIAL0"));
        // What fgtw.org's /report checks: the report and each record whole by their provenance hashes, the count right, no `serial` unless kept.
        vsf::verification::is_original(&doc).unwrap();
        let recs: Vec<_> = secs[0].get_fields("record");
        assert_eq!(recs.len(), 2);
        for f in recs {
            let Some(VsfType::v(b'b', rec)) = f.values.first() else {
                panic!("a record is raw bytes")
            };
            vsf::verification::is_original(rec).unwrap();
            let (rh, rend) = vsf::VsfHeader::decode_strict(rec).unwrap();
            let rs = rh.sections(rec, rend).unwrap();
            assert_eq!(rs.len(), 1);
            assert!(rs[0].get_field("serial").is_none());
        }
        assert_eq!(read(&unpack(&doc).unwrap()).0.len(), 2);
        let kept = report(&run, "ferros-flash 0.1.0", 1 << 42, true).unwrap();
        assert_eq!(unpack(&kept).unwrap(), run);
        assert!(kept.windows(11).any(|w| w == b"TESTSERIAL0"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
