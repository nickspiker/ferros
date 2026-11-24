//! kistcat: kists' signed entries and the kist directory (docs/KISTS.md "The directory"; docs/KIST-DISTRIBUTION.md section 1).
//!
//! - `kistcat entry --key <developer key .vsf> --capsule <capsule.vsf> --about TEXT --host <base URL>... --out <entry.vsf>`: the developer's signed entry for a capsule (KIST-DISTRIBUTION.md 1.0; owner, 2026-11-24: "One entry format, yes."), its `web` the capsule's path under each host (`<base>/capsules/<blake3>.vsf`). The capsule is checked first as the device's stower checks it (every egg against the bundle it carries, or Photon's release bundle), the key must be the capsule's own developer's, and the entry is verified and checked against the capsule's declaration before it is written. Prints the wrangler commands that publish both. Uploads nothing.
//! - `kistcat kist ... --entry <out> --about TEXT --host <base URL>...`: the entry written beside the capsule, in the same run, with the same key.
//! - `kistcat build --unsigned --out <dir> [--web <web/index.html>] --entry <entry.vsf>...`: `<dir>/kists.vsf` (each kist's entry carried byte for byte, KIST-DISTRIBUTION.md 1.1), `<dir>/index.html` (the page for kists.fgtw.org) and, with `--web`, the list on ferros.org between its markers, then the wrangler commands that publish them. `--key <secret>` in place of `--unsigned` signs the whole with the project key (open question 14 recommends dropping that signature). An entry that does not verify stops the build.
//! - `kistcat verify <kists.vsf> [--key <secret>]`: every entry verified alone, and with `--key` the directory's own signature.
//! - `kistcat devkey ...`: developer keys and their public records (devkey.rs).
//! - A key file (`--key`) is VSF, section `developer_key` { label a, seed kge }, read thru ferros_kist::secret (the one reader, shared with the bridge and ferros-mkimg). Nothing of a key is printed, copied or written.

mod devkey;
use ferros_kist::Version;
use ferros_kist::directory::{self, Directory, Entry};
use fgtw::pq::SigningBundle;
use std::path::{Path, PathBuf};
use std::process::exit;

/// The markers around the list in ferros.org's page; kistcat writes only between them.
const BEGIN: &str = "<!-- kists: begin";
const END: &str = "<!-- kists: end -->";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("kistcat: {msg}");
    exit(1)
}

fn usage() -> ! {
    eprintln!("usage:\n  kistcat entry --key <developer key .vsf> --capsule <capsule.vsf> --about TEXT --host <http base URL>... --out <entry.vsf>\n  kistcat build (--key <secret .key.vsf> | --unsigned) --out <dir> [--web <web/index.html>] --entry <entry.vsf>...\n  kistcat verify <kists.vsf> [--key <secret .key.vsf>]\n  kistcat kist --key <developer key .vsf> --masked <image.bin> --text-len N --name NAME --option HEX64 --version 0.1.0 [--want RIGHT REASON]... [--record <public record .vsf>] [--icon <icon .vsf>] [--fixed K] [--bti] [--exact] --out <capsule.vsf> [--entry <entry.vsf> --about TEXT --host <http base URL>...]\n  kistcat devkey new --secret <file> --label LABEL --name NAME [--contact C] [--web W] [--kind K] --record <file>\n  kistcat devkey show <record>\n  kistcat devkey convert --raw <old key file> --label LABEL --secret <new .key.vsf>   (the ferros key: --label FERROS_PROJECT)");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("entry") => entry_cmd(&args[1..]),
        Some("build") => build(&args[1..]),
        Some("verify") => verify(&args[1..]),
        Some("devkey") => devkey(&args[1..]),
        Some("kist") => kist(&args[1..]),
        _ => usage(),
    }
}

#[derive(Default)]
struct Args {
    key: Option<PathBuf>,
    out: Option<PathBuf>,
    web: Option<PathBuf>,
    /// `--unsigned`: the directory with no signature of its own (each entry is its developer's, signed).
    unsigned: bool,
    doc: Option<PathBuf>,
    entries: Vec<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--key" => a.key = Some(val()?.into()),
            "--out" => a.out = Some(val()?.into()),
            "--web" => a.web = Some(val()?.into()),
            "--unsigned" => a.unsigned = true,
            "--entry" => a.entries.push(val()?.into()),
            "--capsule" | "--manifest" => return Err(format!("{flag}: the directory lists developers' signed entries since 2026-11-24 (one entry form); make one with kistcat entry")),
            s if !s.starts_with("--") && a.doc.is_none() => a.doc = Some(s.into()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// A version as a person types it on the command line: parts of decimal digits, any number of them, separated by dots (`0.1.0`).
fn parse_version(s: &str) -> Result<Version, String> {
    Version::parse(s).ok_or_else(|| format!("{s}: a version is parts of decimal digits separated by dots, as many as you use (0.1.0)"))
}

/// 64 hex digits, as the kist scripts give an option.
fn parse_hex32(s: &str) -> Result<[u8; 32], String> {
    // WHY: the text is a person's; anything but 64 digits is not 32 bytes, and slicing two digits at a time would run past a shorter one.
    if s.len() != 64 || !s.is_ascii() {
        return Err(format!("{s}: an option is 64 hex digits"));
    }
    let mut out = [0u8; 32];
    for (k, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[k << 1..(k << 1) + 2], 16).map_err(|_| format!("{s}: an option is 64 hex digits"))?;
    }
    Ok(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A key as people read it: its first four bytes, G# hex in groups of four digits.
fn short(k: &[u8; 32]) -> String {
    format!("G#{:02X}{:02X}_{:02X}{:02X}", k[0], k[1], k[2], k[3])
}

/// The project key's bundle from its secret file (VSF, read thru ferros_kist::secret): the post-quantum halves derived from its seed under its own label.
fn signing_bundle(path: &Path) -> SigningBundle {
    devkey::read_secret(path).unwrap_or_else(|e| fail(e)).1
}

/// A capsule's URL under a host's base: `<base>/capsules/<blake3>.vsf`. The base is plain HTTP, as the device fetches (it has no TLS).
fn capsule_url(base: &str, capsule: &[u8; 32]) -> Result<String, String> {
    if !base.starts_with("http://") {
        return Err(format!("{base}: a host is plain HTTP (the device has no TLS)"));
    }
    Ok(format!("{}{}", base.trim_end_matches('/'), directory::capsule_path(capsule)))
}

/// The developer's signed entry for a capsule (KIST-DISTRIBUTION.md 1.0). The capsule is checked as the device's stower checks it, with nothing pinned (every egg against the bundle it carries, or Photon's release bundle), the signing key must be its developer's own, and the entry is verified and checked against the capsule's declaration before it is returned. `hosts` are the developer's web hosts' bases.
fn make_entry(key: &SigningBundle, c: &[u8], about: &str, hosts: &[String]) -> Result<(Entry, Vec<u8>), String> {
    let (signer, eggs, file_hash) = vsf::verification::header_eggs(c).map_err(|e| format!("the capsule is not signed with eggs ({e})"))?;
    if fgtw::pq::egg_mask(&eggs) != fgtw::release::RELEASE_MASK {
        return Err("the capsule is signed with only some schemes; a kist carries every egg".into());
    }
    ferros_kist::capsule::admit(c, &signer, &eggs, &file_hash, &ferros_kist::Kists::default()).map_err(|r| format!("the device would refuse the capsule: {}", ferros_kist::capsule::refusal_words(&r)))?;
    let public = key.public();
    let (developer, option) = ferros_kist::identity(c)?;
    if public.ed25519() != developer {
        return Err(format!("the key {} is not the capsule's developer {}: an entry is its developer's own word", short(&public.ed25519()), short(&developer)));
    }
    // The bundle the entry carries is the one the capsule's eggs were checked against: the capsule's own, or (Photon's release key, a capsule from before bundles) the one fgtw pins.
    let bundle = ferros_kist::capsule::bundle_of(c)?.unwrap_or_else(fgtw::release::release_bundle);
    if bundle != public {
        return Err("the key's bundle is not the one the capsule carries".into());
    }
    if about.is_empty() || !about.bytes().all(|b| (0x20..0x7F).contains(&b)) {
        return Err(format!("--about {about:?}: one line of printable ASCII (the device's stower is no_std)"));
    }
    let capsule = *blake3::hash(c).as_bytes();
    let web = hosts.iter().map(|h| capsule_url(h, &capsule)).collect::<Result<Vec<_>, _>>()?;
    let e = Entry {
        developer,
        option,
        name: ferros_kist::declaration(c)?.name,
        developer_name: ferros_kist::developer_name_of(c)?,
        version: ferros_kist::version_of(c)?.ok_or("its declaration has no version; re-sign it with a version")?,
        capsule,
        size: c.len() as u64,
        bundle,
        about: about.into(),
        web,
    };
    let doc = fgtw::release::sign_release(ferros_kist::entry::unsigned(&e, vsf::eagle_time_oscillations())?, key)?;
    // The device's own checks of what was just signed: the entry alone, then the capsule's declaration against it.
    let back = ferros_kist::entry::verify(&doc).map_err(|e| format!("the entry just signed does not verify: {e}"))?;
    if back != e {
        return Err("the entry just signed does not read back as built".into());
    }
    ferros_kist::entry::agrees(&e, c)?;
    Ok((e, doc))
}

/// The entry written, and the wrangler commands that publish it and its capsule printed: on each host's own bucket the capsule and its entry beside it, and the entry on fgtw.org's phonebook (the `kists` bucket) too.
fn write_entry(e: &Entry, doc: &[u8], out: &Path, capsule_file: &Path) {
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).unwrap_or_else(|err| fail(format!("{}: {err}", dir.display())));
    }
    std::fs::write(out, doc).unwrap_or_else(|err| fail(format!("{}: {err}", out.display())));
    println!("entry {} {} by {}: capsule {} (G#{:X} bytes), G#{:X} bytes -> {}", e.name, version_text(&e.version), developer_text(e), hex(&e.capsule), e.size, doc.len(), out.display());
    for w in &e.web {
        println!("  web: {w}");
    }
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    println!("  the capsule and this entry go on each host above (capsules/<blake3>.vsf, capsules/<blake3>.entry.vsf), and the entry on kists.fgtw.org{}:", ferros_kist::entry::path(&e.capsule));
    println!("    npx wrangler r2 object put kists{} --file {} --content-type application/vnd.vsf --cache-control 'public, max-age=31536000, immutable' --remote", ferros_kist::entry::path(&e.capsule), abs(out).display());
    println!("    (capsule file: {})", abs(capsule_file).display());
}

/// `kistcat entry`: the developer's signed entry for a capsule already signed.
fn entry_cmd(args: &[String]) {
    let (mut key, mut capsule, mut about, mut out, mut hosts) = (None::<PathBuf>, None::<PathBuf>, None::<String>, None::<PathBuf>, Vec::new());
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| fail(format!("{flag} needs a value")));
        match flag.as_str() {
            "--key" => key = Some(val().into()),
            "--capsule" => capsule = Some(val().into()),
            "--about" => about = Some(val()),
            "--host" => hosts.push(val()),
            "--out" => out = Some(val().into()),
            other => fail(format!("unknown argument {other}")),
        }
    }
    let (Some(key), Some(capsule), Some(about), Some(out)) = (key, capsule, about, out) else { fail("entry needs --key, --capsule, --about, at least one --host and --out") };
    if hosts.is_empty() {
        fail("entry needs at least one --host (where the developer hosts the capsule)");
    }
    let signer = signing_bundle(&key);
    let c = std::fs::read(&capsule).unwrap_or_else(|e| fail(format!("{}: {e}", capsule.display())));
    let (e, doc) = make_entry(&signer, &c, &about, &hosts).unwrap_or_else(|e| fail(format!("{}: {e}", capsule.display())));
    drop(signer);
    write_entry(&e, &doc, &out, &capsule);
}

fn build(args: &[String]) {
    let a = parse_args(args).unwrap_or_else(|e| fail(e));
    let out = a.out.clone().unwrap_or_else(|| fail("--out <dir> is needed"));
    let key = match (&a.key, a.unsigned) {
        (Some(_), true) => fail("--key and --unsigned: one or the other"),
        (Some(p), false) => Some(signing_bundle(p)),
        (None, true) => {
            eprintln!("kistcat: UNSIGNED directory: no signature over the whole (each entry is its developer's own, signed and checked alone)");
            None
        }
        (None, false) => fail("--key <the project key's secret file> is needed, or --unsigned"),
    };
    let entries: Vec<Vec<u8>> = a.entries.iter().map(|p| std::fs::read(p).unwrap_or_else(|e| fail(format!("{}: {e}", p.display())))).collect();
    let made = vsf::eagle_time_oscillations();
    let (doc, dir) = match &key {
        Some(key) => {
            let unsigned = directory::build_unsigned(&entries, made, key.public().ed25519()).unwrap_or_else(|e| fail(e));
            let doc = fgtw::release::sign_release(unsigned, key).unwrap_or_else(|e| fail(e));
            let dir = directory::verify_directory(&doc, &key.public()).unwrap_or_else(|e| fail(format!("the directory just built does not verify: {e}")));
            (doc, dir)
        }
        None => {
            let doc = directory::build_open(&entries, made).unwrap_or_else(|e| fail(e));
            let dir = directory::parse_directory(&doc).unwrap_or_else(|e| fail(format!("the directory just built does not parse: {e}")));
            (doc, dir)
        }
    };
    if dir.skipped != 0 || dir.kists.len() != entries.len() {
        fail(format!("the directory just built reads back G#{:X} of G#{:X} entries", dir.kists.len(), entries.len()));
    }
    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    let (vsf_out, html_out) = (out.join("kists.vsf"), out.join("index.html"));
    std::fs::write(&vsf_out, &doc).unwrap_or_else(|e| fail(format!("{}: {e}", vsf_out.display())));
    std::fs::write(&html_out, page(&dir)).unwrap_or_else(|e| fail(format!("{}: {e}", html_out.display())));
    if let Some(web) = &a.web {
        let site = std::fs::read_to_string(web).unwrap_or_else(|e| fail(format!("{}: {e}", web.display())));
        let site = splice(&site, &fragment(&dir)).unwrap_or_else(|e| fail(format!("{}: {e}", web.display())));
        std::fs::write(web, site).unwrap_or_else(|e| fail(format!("{}: {e}", web.display())));
    }
    let by = dir.signer.as_ref().map_or_else(|| String::from("UNSIGNED as a whole"), |s| format!("signed by {}", short(s)));
    println!("kistcat: G#{:X} kists listed; {} (G#{:X} bytes, {by}) and {}", dir.kists.len(), vsf_out.display(), doc.len(), html_out.display());
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    println!("publish (R2 bucket kists, served at http://{}/):", directory::HOST);
    println!("  npx wrangler r2 object put kists{} --file {} --content-type application/vnd.vsf --cache-control no-cache --remote", directory::PATH, abs(&vsf_out).display());
    println!("  npx wrangler r2 object put kists/index.html --file {} --content-type 'text/html; charset=utf-8' --cache-control no-cache --remote", abs(&html_out).display());
}

fn verify(args: &[String]) {
    let a = parse_args(args).unwrap_or_else(|e| fail(e));
    let Some(doc_path) = a.doc else { usage() };
    let doc = std::fs::read(&doc_path).unwrap_or_else(|e| fail(format!("{}: {e}", doc_path.display())));
    let read = match &a.key {
        Some(key_path) => directory::verify_directory(&doc, &signing_bundle(key_path).public()).map(|d| (d, "signed with every egg")),
        None => directory::parse_directory(&doc).map_err(String::from).map(|d| (d, "each entry checked alone")),
    };
    match read {
        Ok((d, how)) => {
            println!("{}: {how}{}, made {}, G#{:X} kists, G#{:X} entries that do not verify left out", doc_path.display(), d.signer.as_ref().map(|s| format!(" (the whole names {})", short(s))).unwrap_or_default(), day(d.made), d.kists.len(), d.skipped);
            for e in &d.kists {
                println!("  {} {} by {}: G#{:X} bytes, blake3 {}, at {}", e.name, version_text(&e.version), developer_text(e), e.size, hex(&e.capsule), e.web.join(" "));
            }
        }
        Err(e) => fail(format!("{}: {e}", doc_path.display())),
    }
}

fn day(made: i64) -> String {
    vsf::EagleTime::from_oscillations(made).to_datetime().format("%Y-%m-%d").to_string()
}

/// A version on the page: hex, as the page's other numbers (G#), by the device's rule (ferros_kist `write_version`): leading zero parts left out, so 0.1.0 reads G#1.0.
fn version_text(v: &Version) -> String {
    let mut s = String::from("G#");
    let hex = ferros_kist::tatau::Fmt::new(ferros_kist::tatau::Base::Hex, ferros_kist::tatau::Style::Words);
    ferros_kist::write_version(&mut s, hex, &v.0).expect("a String takes any text");
    s
}

fn developer_text(e: &Entry) -> String {
    e.developer_name.clone().unwrap_or_else(|| short(&e.developer))
}

/// Text from a developer, made safe to put in a page: anyone may list a kist.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

/// How many developers list kists: distinct keys.
fn developers(d: &Directory) -> usize {
    let mut keys: Vec<[u8; 32]> = d.kists.iter().map(|e| e.developer).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.len()
}

/// The list as a table, one row per kist, or a line saying it is empty.
fn table(d: &Directory) -> String {
    if d.kists.is_empty() {
        return "<p class=\"audit-note\">None listed yet.</p>\n".into();
    }
    let mut t = String::from("<table class=\"kists\" id=\"list\">\n<thead><tr><th>Name</th><th>Developer</th><th>Version</th><th>What it is</th></tr></thead>\n<tbody>\n");
    for e in &d.kists {
        t.push_str(&format!(
            "<tr><td>{}</td><td class=\"mono\" title=\"G#{}\">{}</td><td class=\"mono\">{}</td><td>{}</td></tr>\n",
            esc(&e.name),
            hex(&e.developer).to_uppercase(),
            esc(&developer_text(e)),
            version_text(&e.version),
            esc(&e.about)
        ));
    }
    t.push_str("</tbody>\n</table>\n");
    t
}

/// The style both pages share: ferros.org's tokens and measures (web/index.html), relative units only.
const STYLE: &str = r#"      :root {
         --ru: 1vw;
      }

      @media (min-aspect-ratio: 1/1) {
         :root {
            --ru: 1vh;
         }
      }

      :root {
         --bg: #0a0e13;
         --ink: #e1e8f0;
         --dim: #8b949e;
         --faint: #55606b;
         --line: #30363d;
         --zero: #32ff00;
         --mono: ui-monospace, 'Cascadia Code', 'JetBrains Mono', Menlo, Consolas, monospace;
      }

      * {
         margin: 0;
         padding: 0;
         box-sizing: border-box;
      }

      body {
         background: var(--bg);
         color: var(--ink);
         font-family: Charter, 'Bitstream Charter', Georgia, serif;
         font-size: calc(2.2 * var(--ru));
         line-height: 1.75;
      }

      main {
         width: min(calc(94 * var(--ru)), 72ch);
         margin: 0 auto;
         padding: calc(8 * var(--ru)) calc(2 * var(--ru)) calc(12 * var(--ru));
         text-align: center;
      }

      nav {
         font-family: var(--mono);
         font-size: calc(1.8 * var(--ru));
         margin-bottom: calc(4 * var(--ru));
      }

      nav a {
         color: var(--dim);
         text-decoration: none;
      }

      nav a:hover {
         color: var(--zero);
      }

      h1 {
         font-family: var(--mono);
         font-weight: 400;
         font-size: calc(5 * var(--ru));
         letter-spacing: 0.02em;
      }

      h1 span {
         color: var(--zero);
         text-shadow: 0 0 calc(2 * var(--ru)) rgba(50, 255, 0, 0.35);
      }

      h2 {
         font-family: var(--mono);
         font-weight: 400;
         font-size: calc(1.9 * var(--ru));
         letter-spacing: 0.14em;
         text-transform: uppercase;
         color: var(--dim);
         margin: calc(5 * var(--ru)) 0 calc(1.5 * var(--ru));
         border-bottom: calc(0.1 * var(--ru)) solid var(--line);
         padding-bottom: calc(0.6 * var(--ru));
      }

      p {
         margin: calc(1.5 * var(--ru)) 0;
      }

      a {
         color: var(--ink);
         text-decoration: underline;
         text-decoration-color: var(--faint);
         text-underline-offset: calc(0.4 * var(--ru));
      }

      a:hover {
         text-decoration-color: var(--zero);
      }

      .thesis {
         font-size: calc(2.8 * var(--ru));
         line-height: 1.6;
         margin: calc(3 * var(--ru)) 0;
      }

      .audit-note {
         font-family: var(--mono);
         font-size: calc(1.7 * var(--ru));
         color: var(--faint);
      }

      .ledger {
         width: 100%;
         border-collapse: collapse;
         margin: calc(4 * var(--ru)) 0 calc(1.5 * var(--ru));
         font-family: var(--mono);
         font-size: calc(1.9 * var(--ru));
      }

      .ledger td {
         padding: calc(0.9 * var(--ru)) 0;
         border-bottom: calc(0.1 * var(--ru)) solid var(--line);
         color: var(--dim);
         text-align: left;
      }

      .ledger td:last-child {
         text-align: right;
         color: var(--zero);
         font-size: calc(2.6 * var(--ru));
         text-shadow: 0 0 calc(1.5 * var(--ru)) rgba(50, 255, 0, 0.4);
      }

      /* The search: one line, the page's own colours. */
      input[type=search] {
         width: 100%;
         margin: calc(2 * var(--ru)) 0 0;
         padding: calc(0.9 * var(--ru)) calc(1.2 * var(--ru));
         background: var(--bg);
         color: var(--ink);
         border: calc(0.1 * var(--ru)) solid var(--line);
         font-family: var(--mono);
         font-size: calc(1.9 * var(--ru));
      }

      input[type=search]:focus {
         outline: none;
         border-color: var(--zero);
      }

"#;

/// The list's own rules, the same on both pages (web/index.html carries a copy).
const KISTS_STYLE: &str = r#"      /* The kist list (tools/kistcat): name, developer, version, what it is. */
      .kists {
         width: 100%;
         border-collapse: collapse;
         margin: calc(2 * var(--ru)) 0;
         font-size: calc(1.9 * var(--ru));
         text-align: left;
      }

      .kists th {
         font-family: var(--mono);
         font-weight: 400;
         font-size: calc(1.6 * var(--ru));
         letter-spacing: 0.1em;
         text-transform: uppercase;
         color: var(--faint);
         padding: calc(0.7 * var(--ru)) calc(1 * var(--ru)) calc(0.7 * var(--ru)) 0;
         border-bottom: calc(0.1 * var(--ru)) solid var(--line);
      }

      .kists td {
         padding: calc(0.9 * var(--ru)) calc(1 * var(--ru)) calc(0.9 * var(--ru)) 0;
         border-bottom: calc(0.1 * var(--ru)) solid var(--line);
         color: var(--dim);
         vertical-align: top;
      }

      .kists td:first-child {
         color: var(--ink);
      }

      .kists .mono {
         font-family: var(--mono);
         font-size: calc(1.7 * var(--ru));
         white-space: nowrap;
      }
"#;

/// The page at kists.fgtw.org: the list, its numbers, a search box, and where kists run.
fn page(d: &Directory) -> String {
    let search = if d.kists.is_empty() {
        String::new()
    } else {
        "<input id=\"q\" type=\"search\" placeholder=\"search kists\" aria-label=\"Search kists\" autocomplete=\"off\">\n".into()
    };
    // The one script: a few lines that hide the rows the search does not match. No libraries, nothing fetched.
    let script = if d.kists.is_empty() {
        String::new()
    } else {
        "<script>\nconst q = document.getElementById('q'), rows = document.querySelectorAll('#list tbody tr');\nq.addEventListener('input', () => {\n   const t = q.value.trim().toLowerCase();\n   for (const r of rows) r.hidden = t !== '' && !r.textContent.toLowerCase().includes(t);\n});\n</script>\n".into()
    };
    format!(
        r#"<!DOCTYPE html>
<!--
  The kist list, written by tools/kistcat in the ferros repository from kists.vsf (the same signed file the device reads, beside this page).
  No cookies, no trackers, no fonts fetched. One short script: the search.
-->
<html lang="en">
<head>
   <meta charset="utf-8">
   <meta name="viewport" content="width=device-width, initial-scale=1">
   <title>kists</title>
   <meta name="description" content="Kists: apps for ferros, listed by anyone, stowed with no account. The list vouches for nothing; your device checks every kist itself.">
   <style>
{STYLE}{KISTS_STYLE}   </style>
</head>
<body>
   <main>
      <nav>
         <a href="https://ferros.org/">ferros.org</a> · <a href="{path}">kists.vsf</a> · <a href="https://github.com/nickspiker/ferros/blob/forge/docs/KISTS.md">how kists work</a>
      </nav>

      <h1>kists<span>.fgtw.org</span></h1>
      <p class="thesis">Apps for ferros, listed by anyone,<br>stowed with no account.</p>

      <table class="ledger">
         <tr><td>Kists listed</td><td>G#{count:X}</td></tr>
         <tr><td>Developers</td><td>G#{devs:X}</td></tr>
         <tr><td>Last updated</td><td>{day}</td></tr>
      </table>

{search}{table}
      <h2>Getting one</h2>
      <p>Kists run on <a href="https://ferros.org/">ferros</a> today. A kist is packed the same for every platform; Linux, Android and macOS will run them thru a host app once kists carry portable code, and none is out yet. For now, use ferros.</p>

      <h2>What this list is</h2>
      <p>Anyone may list a kist, and nobody needs an account to stow one. The list vouches for nothing: your device checks each kist against its hash here and against its developer's signature when it stows it, and trusts a developer's keys from their first kist on, refusing any kist under their key signed otherwise. {signed}</p>
      <p>No fees, no ads, no stars. Measures will come, many of them, and never a single score.</p>
   </main>
{script}</body>
</html>
"#,
        path = directory::PATH,
        signed = if d.signer.is_some() {
            "The list itself is signed, so nobody between can change it unseen."
        } else {
            "Each kist here is its developer's own signed entry, carried as they signed it, so your device checks every one alone and needs nothing else to trust the list."
        },
        count = d.kists.len(),
        devs = developers(d),
        day = day(d.made),
        table = table(d),
    )
}

/// The list for ferros.org's page: no script (that page runs none), the same table and its numbers.
fn fragment(d: &Directory) -> String {
    if d.kists.is_empty() {
        return format!("<p class=\"audit-note\">None listed yet ({}).</p>\n", day(d.made));
    }
    format!("<p class=\"audit-note\">G#{:X} kists from G#{:X} developers, listed {}.</p>\n{}", d.kists.len(), developers(d), day(d.made), table(d))
}

/// The page with the text between its kist markers replaced by `list`; the markers stay.
fn splice(site: &str, list: &str) -> Result<String, String> {
    let b = site.find(BEGIN).ok_or("no kist begin marker")?;
    let open_end = b + site[b..].find("-->").ok_or("an unclosed kist begin marker")? + 3;
    let e = open_end + site[open_end..].find(END).ok_or("no kist end marker after the begin marker")?;
    Ok(format!("{}\n{}{}", &site[..open_end], list, &site[e..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn developer() -> SigningBundle {
        SigningBundle::developer(&[9; 32], "TEST_DEVELOPER")
    }

    /// A capsule signed as `kistcat kist` and the bridge sign a kist: its declaration, its developer's bundle, its code, every egg.
    fn capsule(key: &SigningBundle, version: Option<Version>) -> Vec<u8> {
        let d = ferros_kist::Decl { name: "Clock".into(), wants: vec![] };
        let image = [7u8; 64];
        let code = ferros_kist::capsule::Code { image: &image, masked: true, text: Some(64), fixed: None, bti: false, exact: false };
        fgtw::release::sign_release(ferros_kist::capsule::unsigned(&d, [5; 32], version.as_ref(), Some("Test Developer"), None, &key.public(), &code, 3).unwrap(), key).unwrap()
    }

    #[test]
    fn an_entry_is_made_by_the_capsules_own_developer() {
        let key = developer();
        let c = capsule(&key, Some(Version(vec![0, 1, 0])));
        let (e, doc) = make_entry(&key, &c, "the time, drawn as rings", &["http://example.org/kists/".into()]).unwrap();
        assert_eq!(e.capsule, *blake3::hash(&c).as_bytes());
        assert_eq!(e.web, vec![format!("http://example.org/kists/capsules/{}.vsf", hex(&e.capsule))]);
        assert_eq!(e.developer_name.as_deref(), Some("Test Developer"));
        assert_eq!(ferros_kist::entry::verify(&doc).unwrap(), e);
        // Another developer's key cannot speak for this capsule.
        let other = SigningBundle::developer(&[8; 32], "TEST_DEVELOPER");
        assert!(make_entry(&other, &c, "x", &["http://example.org".into()]).unwrap_err().contains("not the capsule's developer"));
        // No version, an https host, or text a no_std stower cannot read: refused.
        assert!(make_entry(&key, &capsule(&key, None), "x", &["http://example.org".into()]).unwrap_err().contains("no version"));
        assert!(make_entry(&key, &c, "x", &["https://example.org".into()]).is_err());
        assert!(make_entry(&key, &c, "the time \u{2192} rings", &["http://example.org".into()]).is_err());
    }

    #[test]
    fn the_directory_carries_entries_byte_for_byte() {
        let key = developer();
        let c = capsule(&key, Some(Version(vec![2])));
        let (e, doc) = make_entry(&key, &c, "the time", &["http://example.org".into()]).unwrap();
        let dir = directory::parse_directory(&directory::build_open(&[doc], 7).unwrap()).unwrap();
        assert_eq!(dir.kists, vec![e]);
        assert!(table(&dir).contains("Clock"));
        // The old shapes are gone, with no reader kept.
        assert!(parse_args(&["--capsule".into(), "c.vsf".into()]).is_err());
        assert!(parse_args(&["--manifest".into(), "http://x".into()]).is_err());
        assert_eq!(parse_version("0.1.0.7").unwrap().0, vec![0, 1, 0, 7]);
    }
}


/// `kistcat kist --key <developer key .vsf> --masked <image.bin> --text-len N --name NAME --option HEX64 --version 0.1.0 [--want RIGHT REASON]... [--record <public record .vsf>] [--icon <icon .vsf>] [--fixed K] [--bti] [--exact] --out <capsule.vsf>`: a kist signed by its developer's own key (owner, 2026-11-10: trust on first stow). `--want` is for the rights the person is asked for; the implicit set (ferros_kist::IMPLICIT, KISTS.md "The implicit set") every kist is handed without one.
/// - The key is the developer's secret (`devkey new`: section `developer_key` { label a, seed kge }); the bundle is derived from it under its label, used here and dropped, and nothing of it is printed or copied.
/// - The code is a sandboxed image from tools/ferros-masked-build.sh (its flat .bin, and its text length: the .textlen file beside it), as the bridge's `capsule --masked --text-len` takes them; the network stower refuses native code.
/// - `--record`: the developer's public record (`devkey new --record`), checked with every egg and that it names this key; its name goes into the declaration as `developer`, which the directory lists and the person's pin keeps.
/// - The capsule is built by ferros_kist::capsule (the bridge's builder too): the declaration, the developer's whole public bundle (`kb`), the code, an egg per scheme; then checked as the device's stower checks it before it is written.
/// - `--entry <out> --about TEXT --host <base URL>...`: the developer's signed entry for this capsule (KIST-DISTRIBUTION.md 1.0), written beside it with the same key (`make_entry`).
fn kist(args: &[String]) {
    let mut key: Option<PathBuf> = None;
    let mut image: Option<PathBuf> = None;
    let mut text: Option<usize> = None;
    let mut name: Option<String> = None;
    let mut option: Option<[u8; 32]> = None;
    let mut version: Option<Version> = None;
    let mut record: Option<PathBuf> = None;
    let mut icon_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut fixed: Option<u32> = None;
    let (mut bti, mut exact) = (false, false);
    let mut wants: Vec<(u8, String)> = Vec::new();
    // `--entry <out> --about TEXT --host <base>...`: the developer's signed entry written beside the capsule, with the same key, in the same run (KIST-DISTRIBUTION.md 1.0).
    let (mut entry_out, mut about, mut hosts) = (None::<PathBuf>, None::<String>, Vec::<String>::new());
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| fail(format!("{flag} needs a value")));
        match flag.as_str() {
            "--key" => key = Some(val().into()),
            "--masked" => image = Some(val().into()),
            "--text-len" => text = Some(val().trim().parse().unwrap_or_else(|_| fail("--text-len takes the image's instruction bytes"))),
            "--name" => name = Some(val()),
            "--option" => option = Some(parse_hex32(&val()).unwrap_or_else(|e| fail(e))),
            "--version" => version = Some(parse_version(&val()).unwrap_or_else(|e| fail(e))),
            "--record" => record = Some(val().into()),
            "--icon" => icon_path = Some(val().into()),
            "--out" => out = Some(val().into()),
            "--fixed" => fixed = Some(val().parse().unwrap_or_else(|_| fail("--fixed takes K, the arc's size 2^K"))),
            "--bti" => bti = true,
            "--exact" => exact = true,
            "--want" => {
                let right = val();
                let reason = val();
                let c = ferros_kist::code(&right).unwrap_or_else(|| fail(format!("{right} is not a right ({})", ferros_kist::RIGHTS.join(", "))));
                if wants.iter().any(|w| w.0 == c) {
                    fail(format!("--want {right} twice"));
                }
                wants.push((c, reason));
            }
            "--entry" => entry_out = Some(val().into()),
            "--about" => about = Some(val()),
            "--host" => hosts.push(val()),
            other => fail(format!("unknown argument {other}")),
        }
    }
    let (Some(key_path), Some(image_path), Some(text), Some(name), Some(option), Some(version), Some(out)) = (key, image, text, name, option, version, out) else {
        fail("kist needs --key, --masked, --text-len, --name, --option, --version and --out")
    };
    // The text the no_std stower reads is ASCII (`a`): a name or reason it could not read stops here.
    let ascii = |what: &str, s: &str| {
        if s.is_empty() || !s.bytes().all(|b| (0x20..0x7F).contains(&b)) {
            fail(format!("{what} {s:?}: printable ASCII (the device's stower is no_std)"));
        }
    };
    ascii("the name", &name);
    for (_, r) in &wants {
        ascii("a reason", r);
    }
    let (label, signer) = devkey::read_secret(&key_path).unwrap_or_else(|e| fail(e));
    let public = signer.public();
    let developer = match &record {
        Some(p) => {
            let doc = std::fs::read(p).unwrap_or_else(|e| fail(format!("{}: {e}", p.display())));
            let r = devkey::read_record(&doc).unwrap_or_else(|e| fail(format!("{}: {e}", p.display())));
            if r.ed25519 != public.ed25519() || r.label != label {
                fail(format!("{}: the record of {} ({}), not of this key", p.display(), short(&r.ed25519), r.label));
            }
            ascii("the developer's name", &r.name);
            Some(r.name)
        }
        None => None,
    };
    let code = std::fs::read(&image_path).unwrap_or_else(|e| fail(format!("{}: {e}", image_path.display())));
    // WHY: the length is the person's; text past the image's end is not instructions the checker could cover.
    if text == 0 || text > code.len() {
        fail(format!("--text-len {text}: the image is {} bytes", code.len()));
    }
    // `--icon`: the kist's icon document (tools/app-icons `icon`), carried in the signed declaration (ferros_kist::Icon).
    let icon = icon_path.map(|p| {
        let doc = std::fs::read(&p).unwrap_or_else(|e| fail(format!("{}: {e}", p.display())));
        ferros_kist::icon_document(&doc).unwrap_or_else(|e| fail(format!("{}: {e}", p.display())))
    });
    let decl = ferros_kist::Decl { name, wants };
    let c = ferros_kist::capsule::Code { image: &code, masked: true, text: Some(text), fixed, bti, exact };
    let unsigned = ferros_kist::capsule::unsigned(&decl, option, Some(&version), developer.as_deref(), icon.as_ref(), &public, &c, vsf::eagle_time_oscillations()).unwrap_or_else(|e| fail(e));
    let capsule = fgtw::release::sign_release(unsigned, &signer).unwrap_or_else(|e| fail(e));
    // The entry with the same key, before it is dropped; checked against the capsule just built.
    let entry = entry_out.as_ref().map(|_| {
        let Some(about) = about.as_deref() else { fail("--entry needs its --about") };
        if hosts.is_empty() {
            fail("--entry needs at least one --host (where the developer hosts the capsule)");
        }
        make_entry(&signer, &capsule, about, &hosts).unwrap_or_else(|e| fail(format!("the entry: {e}")))
    });
    drop(signer);
    // Checked as the device checks it, before it is written: every egg against the bundle it carries, with nothing pinned yet.
    let (s, eggs, file_hash) = vsf::verification::header_eggs(&capsule).unwrap_or_else(|e| fail(format!("the capsule just signed does not read: {e}")));
    let admitted = ferros_kist::capsule::admit(&capsule, &s, &eggs, &file_hash, &ferros_kist::Kists::default()).unwrap_or_else(|r| fail(format!("the capsule just signed is refused: {}", ferros_kist::capsule::refusal_words(&r))));
    if admitted.bundle != Some(public.commit()) || ferros_kist::declaration(&capsule).map(|d| d != decl).unwrap_or(true) || ferros_kist::version_of(&capsule).ok().flatten().map(|v| v.0) != Some(version.0.clone()) {
        fail("the capsule just signed does not read back as built");
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(format!("{}: {e}", dir.display())));
    }
    std::fs::write(&out, &capsule).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    println!(
        "kist {} {} by {} (key {}, bundle {}): G#{:X} bytes, blake3 {} -> {}",
        decl.name,
        version_text(&version),
        developer.as_deref().unwrap_or("its key"),
        short(&public.ed25519()),
        short(&public.commit()),
        capsule.len(),
        hex(blake3::hash(&capsule).as_bytes()),
        out.display()
    );
    if let (Some((e, doc)), Some(p)) = (entry, entry_out) {
        write_entry(&e, &doc, &p, &out);
    }
}


/// `kistcat devkey new --secret <file> --label LABEL --name NAME [--contact C] [--web W] [--kind K] --record <file>`: a new developer key, its secret into the keys directory, its signed public record beside it; `kistcat devkey show <record>`: the record checked and printed.
fn devkey(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("new") => {
            let get = |k: &str| args.windows(2).find(|w| w[0] == k).map(|w| w[1].clone());
            let secret = PathBuf::from(get("--secret").unwrap_or_else(|| fail("--secret <file in the keys directory>")));
            let label = get("--label").unwrap_or_else(|| fail("--label LABEL (part of the key, never changed: e.g. FERROS_DEVELOPER_NICK)"));
            let name = get("--name").unwrap_or_else(|| fail("--name NAME"));
            let record_path = PathBuf::from(get("--record").unwrap_or_else(|| fail("--record <file for the public record>")));
            let made = vsf::eagle_time_oscillations();
            let key = devkey::new_secret(&secret, &label, made).unwrap_or_else(|e| fail(e));
            let r = devkey::Record { label, name, contact: get("--contact"), web: get("--web"), kind: get("--kind").unwrap_or_else(|| "person".into()), created: made, revision: 1, ed25519: key.public().ed25519() };
            let doc = devkey::record(&r, &key).unwrap_or_else(|e| fail(e));
            std::fs::write(&record_path, &doc).unwrap_or_else(|e| fail(format!("{}: {e}", record_path.display())));
            let back = devkey::read_record(&doc).unwrap_or_else(|e| fail(format!("the record does not verify: {e}")));
            println!("developer key {} ({}): secret {} (owner-only), public record {} ({} bytes, every egg verified)", short(&back.ed25519), back.name, secret.display(), record_path.display(), doc.len());
        }
        // A raw key file written as VSF to a new file (owner, 2026-11-10: secrets move to VSF). Says only the public key's first bytes; the raw file is left for the owner to retire.
        Some("convert") => {
            let get = |k: &str| args.windows(2).find(|w| w[0] == k).map(|w| w[1].clone());
            let raw = PathBuf::from(get("--raw").unwrap_or_else(|| fail("--raw <the old key file>")));
            let label = get("--label").unwrap_or_else(|| fail("--label LABEL (part of the key, never changed; the ferros key: FERROS_PROJECT)"));
            let secret = PathBuf::from(get("--secret").unwrap_or_else(|| fail("--secret <a new file for the VSF key, e.g. keys/ferros.key.vsf>")));
            let public = devkey::convert(&raw, &label, &secret, vsf::eagle_time_oscillations()).unwrap_or_else(|e| fail(e));
            println!("key {} (label {label}): {} written owner-only and read back; {} is unchanged: retire it once nothing reads it", short(&public), secret.display(), raw.display());
        }
        Some("show") => {
            let p = args.get(1).unwrap_or_else(|| fail("kistcat devkey show <record>"));
            let doc = std::fs::read(p).unwrap_or_else(|e| fail(format!("{p}: {e}")));
            let r = devkey::read_record(&doc).unwrap_or_else(|e| fail(e));
            println!("{} ({}), {} key {}, label {}, revision {}, made {}{}{}, every egg verified", r.name, r.kind, "developer", short(&r.ed25519), r.label, r.revision, day(r.created), r.contact.map(|c| format!(", contact {c}")).unwrap_or_default(), r.web.map(|w| format!(", web {w}")).unwrap_or_default());
        }
        _ => usage(),
    }
}
