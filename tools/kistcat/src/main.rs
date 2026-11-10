//! kistcat: the kist directory (docs/KISTS.md "The directory"; owner, 2026-11-10).
//!
//! - `kistcat build --key <secret> --out <dir> [--web <web/index.html>] [--signer-name NAME] ENTRY...` writes `<dir>/kists.vsf` (signed by the project key with every egg), `<dir>/index.html` (the page for kists.fgtw.org) and, with `--web`, the list on ferros.org between its markers, then prints the wrangler commands that publish them. It uploads nothing.
//! - An ENTRY is a local capsule, `--capsule <file> --version M.m.p --about TEXT [--icon PATH]`: read for its developer key, option, name, blake3 and size, with every egg present and its Ed25519 egg verified (every egg when its developer is Photon's release key, the one bundle pinned here); listed at `/capsules/<blake3>.vsf` on kists.fgtw.org.
//! - Or a developer's manifest, `--manifest <http URL> --name NAME --option HEX --about TEXT [--platform P] [--arch A] [--icon PATH]`: fetched over plain HTTP at build time, every egg verified against the developer's pinned bundle, and its artefact P/A (ferros/arm64 by default) read for the capsule's blake3, size and version. The entry links the manifest rather than copying the artefact's address: the device fetches the manifest and resolves the artefact itself, and the blake3 recorded here is what it must match.
//! - An entry that cannot be listed (a capsule signed before every-egg kists, a manifest with no ferros artefact yet) is skipped with a warning; a capsule file that is not there stops the build.
//! - `kistcat verify <kists.vsf> --key <secret> [--signer-name NAME]`: every egg against the project key's bundle, then every entry's shape (`ferros_kist::directory::verify_directory`, as the device checks it).
//! - The key file's first 32 bytes are the Ed25519 seed (ferros.secret is the seed then the public key); nothing of it is printed, copied or written.

use ferros_kist::directory::{self, Directory, Entry, Metrics, Source, Version};
use fgtw::pq::{FleetSigner, KeyBundle, SigningBundle};
use std::path::{Path, PathBuf};
use std::process::exit;
use vsf::VsfType;

/// The developer name the project key's post-quantum halves are derived under (fgtw `SigningBundle::developer`). It is part of the key: never changed once a directory is published.
const SIGNER_NAME: &str = "FERROS_PROJECT";
/// The artefact a manifest entry names unless told otherwise (Photon's manifest: `platform` and `arch`).
const PLATFORM: &str = "ferros";
const ARCH: &str = "arm64";
/// The markers around the list in ferros.org's page; kistcat writes only between them.
const BEGIN: &str = "<!-- kists: begin";
const END: &str = "<!-- kists: end -->";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("kistcat: {msg}");
    exit(1)
}

fn usage() -> ! {
    eprintln!("usage:\n  kistcat build --key <secret> --out <dir> [--web <web/index.html>] [--signer-name NAME] ENTRY...\n    ENTRY: --capsule <file.vsf> --version M.m.p --about TEXT [--icon PATH]\n         | --manifest <http URL> --name NAME --option HEX64 --about TEXT [--platform P] [--arch A] [--icon PATH]\n  kistcat verify <kists.vsf> --key <secret> [--signer-name NAME]");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") => build(&args[1..]),
        Some("verify") => verify(&args[1..]),
        _ => usage(),
    }
}

/// One entry as the command line gives it.
enum Spec {
    Capsule { file: PathBuf, version: Option<Version>, about: Option<String>, icon: Option<String> },
    Manifest { url: String, name: Option<String>, option: Option<[u8; 32]>, about: Option<String>, icon: Option<String>, platform: String, arch: String },
}

#[derive(Default)]
struct Args {
    key: Option<PathBuf>,
    out: Option<PathBuf>,
    web: Option<PathBuf>,
    signer: Option<String>,
    doc: Option<PathBuf>,
    specs: Vec<Spec>,
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
            "--signer-name" => a.signer = Some(val()?),
            "--capsule" => a.specs.push(Spec::Capsule { file: val()?.into(), version: None, about: None, icon: None }),
            "--manifest" => {
                let url = val()?;
                if !url.starts_with("http://") {
                    return Err(format!("{url}: a manifest is fetched over plain HTTP, as the device fetches it (it has no TLS)"));
                }
                a.specs.push(Spec::Manifest { url, name: None, option: None, about: None, icon: None, platform: PLATFORM.into(), arch: ARCH.into() });
            }
            "--version" => match a.specs.last_mut() {
                Some(Spec::Capsule { version, .. }) => *version = Some(parse_version(&val()?)?),
                _ => return Err("--version follows a --capsule (a manifest's version is its artefact's)".into()),
            },
            "--about" => match a.specs.last_mut() {
                Some(Spec::Capsule { about, .. } | Spec::Manifest { about, .. }) => *about = Some(val()?),
                None => return Err("--about follows its --capsule or --manifest".into()),
            },
            "--icon" => match a.specs.last_mut() {
                Some(Spec::Capsule { icon, .. } | Spec::Manifest { icon, .. }) => *icon = Some(val()?),
                None => return Err("--icon follows its --capsule or --manifest".into()),
            },
            "--name" | "--option" | "--platform" | "--arch" => {
                let v = val()?;
                let Some(Spec::Manifest { name, option, platform, arch, .. }) = a.specs.last_mut() else {
                    return Err(format!("{flag} follows a --manifest (a capsule names itself)"));
                };
                match flag.as_str() {
                    "--name" => *name = Some(v),
                    "--option" => *option = Some(parse_hex32(&v)?),
                    "--platform" => *platform = v,
                    _ => *arch = v,
                }
            }
            s if !s.starts_with("--") && a.doc.is_none() => a.doc = Some(s.into()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// `M.m.p`, as a person types a version on the command line.
fn parse_version(s: &str) -> Result<Version, String> {
    let n: Vec<u64> = s.split('.').map(|p| p.parse::<u64>()).collect::<Result<_, _>>().map_err(|_| format!("{s}: a version is major.minor.patch"))?;
    match n[..] {
        [major, minor, patch] => Ok(Version { major, minor, patch }),
        _ => Err(format!("{s}: a version is major.minor.patch")),
    }
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

/// The project key's bundle from its secret file: the first 32 bytes are the Ed25519 seed, and the post-quantum halves are derived from it under `name`.
fn signing_bundle(path: &Path, name: &str) -> SigningBundle {
    let key = std::fs::read(path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    // WHY: the file is outside input; a file shorter than a seed has none, and the slice below would panic on it.
    let Some(seed) = key.get(..32) else { fail(format!("{}: shorter than a 32-byte seed", path.display())) };
    let seed: [u8; 32] = seed.try_into().unwrap();
    SigningBundle::developer(&seed, name)
}

/// The directory signed with every egg of `key` (the bridge's machinery for a kist: the release slots, then `sign_release`).
fn sign(kists: &[Entry], made: i64, key: &SigningBundle) -> Result<Vec<u8>, String> {
    let unsigned = directory::build_unsigned(kists, made, key.keypair().public.to_bytes())?;
    fgtw::release::sign_release(unsigned, key)
}

/// A local capsule as an entry: every scheme signed, the Ed25519 egg verified (and every egg, for the one developer whose bundle is pinned here), its identity and name from its own declaration.
fn capsule_entry(c: &[u8], version: Version, about: String, icon: Option<String>) -> Result<Entry, String> {
    let (signer, eggs, _) = vsf::verification::header_eggs(c).map_err(|e| format!("not signed with eggs ({e}); a kist carries every scheme's signature since 2026-11-10, re-sign it"))?;
    if fgtw::pq::egg_mask(&eggs) != fgtw::release::RELEASE_MASK {
        return Err("signed with only some schemes; a kist carries every egg".into());
    }
    if let Some(bundle) = pinned(&signer) {
        fgtw::release::verify_signed_by(c, &signer, &bundle)?;
    }
    let (developer, option) = ferros_kist::identity(c)?;
    let capsule = *blake3::hash(c).as_bytes();
    Ok(Entry {
        name: ferros_kist::declaration(c)?.name,
        developer,
        developer_name: ferros_kist::developer_name_of(c)?,
        option,
        version,
        source: Source::Path(format!("/capsules/{}.vsf", hex(&capsule))),
        capsule,
        size: c.len() as u64,
        about,
        icon,
        metrics: Metrics::default(),
    })
}

/// The bundles pinned here, by Ed25519 key: Photon's release key alone (fgtw::release). A developer of their own has none yet (KISTS.md: fgtw's developer bundles are not pinned anywhere), so only their Ed25519 egg is checked here and every egg at install.
fn pinned(k: &[u8; 32]) -> Option<KeyBundle> {
    (*k == fgtw::release::RELEASE_ED25519).then(fgtw::release::release_bundle)
}

/// A manifest's artefact for one platform and architecture.
struct Artefact {
    capsule: [u8; 32],
    size: u64,
    url: String,
    version: Version,
}

/// The artefact section whose `platform` and `arch` are these, fields by name and type marker (Photon's manifest: `platform`/`arch` text, `major`/`minor`/`patch` `z` with major and patch absent when 0, `size` `z`, `url` `nu`, `hash` `hb`); None when the manifest has no such artefact.
fn artefact(doc: &[u8], platform: &str, arch: &str) -> Result<Option<Artefact>, String> {
    let (h, end) = vsf::file_format::VsfHeader::decode(doc)?;
    for s in h.sections(doc, end)? {
        let text = |n: &str| s.get_field(n).and_then(|f| f.values.iter().find_map(|v| v.as_string()));
        if text("platform") != Some(platform) || text("arch") != Some(arch) {
            continue;
        }
        let num = |n: &str| {
            s.get_field(n).and_then(|f| {
                f.values.iter().find_map(|v| match v {
                    VsfType::z(x) => Some(*x as u64),
                    other => other.as_u64(),
                })
            })
        };
        // WHY: the length is the manifest's; a hash of any other length is not a blake3, and `try_into` refuses it.
        let capsule = s.get_field("hash").and_then(|f| f.values.iter().find_map(|v| if let VsfType::hb(b) = v { b[..].try_into().ok() } else { None })).ok_or("an artefact without its blake3")?;
        let url = s.get_field("url").and_then(|f| f.values.iter().find_map(|v| if let VsfType::nu(u) = v { Some(u.clone()) } else { None })).ok_or("an artefact without its url")?;
        return Ok(Some(Artefact {
            capsule,
            size: num("size").ok_or("an artefact without its size")?,
            url,
            version: Version { major: num("major").unwrap_or(0), minor: num("minor").ok_or("an artefact without its version")?, patch: num("patch").unwrap_or(0) },
        }));
    }
    Ok(None)
}

/// A manifest entry: the manifest verified with every egg against its signer's pinned bundle, then its artefact. The developer listed is the manifest's signer: the key that signs the capsule too (Photon's release key signs both). None when the manifest has no such artefact yet.
#[allow(clippy::too_many_arguments)]
fn manifest_entry(doc: &[u8], url: &str, name: String, option: [u8; 32], about: String, icon: Option<String>, platform: &str, arch: &str, pinned: impl Fn(&[u8; 32]) -> Option<KeyBundle>) -> Result<Option<Entry>, String> {
    let (h, _) = vsf::file_format::VsfHeader::decode(doc)?;
    let signer: [u8; 32] = match &h.signer_pubkey {
        // WHY: the length is the manifest's; anything but 32 bytes is no Ed25519 key.
        Some(VsfType::ke(k)) => k[..].try_into().map_err(|_| "a signer key that is not Ed25519")?,
        _ => return Err("an unsigned manifest".into()),
    };
    let bundle = pinned(&signer).ok_or_else(|| format!("signed by {}, whose bundle is not pinned here", short(&signer)))?;
    fgtw::release::verify_signed_by(doc, &signer, &bundle)?;
    let Some(a) = artefact(doc, platform, arch)? else { return Ok(None) };
    if !a.url.starts_with("http://") {
        return Err(format!("its {platform}/{arch} artefact is at {}, not plain HTTP: the device has no TLS", a.url));
    }
    Ok(Some(Entry {
        name,
        developer: signer,
        developer_name: None,
        option,
        version: a.version,
        source: Source::Manifest { url: url.into(), platform: platform.into(), arch: arch.into() },
        capsule: a.capsule,
        size: a.size,
        about,
        icon,
        metrics: Metrics::default(),
    }))
}

/// A document over plain HTTP (no TLS feature in this build).
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let mut r = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
    r.body_mut().read_to_vec().map_err(|e| format!("{url}: {e}"))
}

fn build(args: &[String]) {
    let a = parse_args(args).unwrap_or_else(|e| fail(e));
    let key_path = a.key.unwrap_or_else(|| fail("--key <the project key's secret file> is needed"));
    let out = a.out.unwrap_or_else(|| fail("--out <dir> is needed"));
    let key = signing_bundle(&key_path, a.signer.as_deref().unwrap_or(SIGNER_NAME));
    let mut kists = Vec::new();
    let mut uploads: Vec<(String, PathBuf)> = Vec::new();
    let mut skipped = 0usize;
    for spec in a.specs {
        match spec {
            Spec::Capsule { file, version, about, icon } => {
                let (Some(version), Some(about)) = (version, about) else { fail(format!("{}: needs its --version and --about", file.display())) };
                let c = std::fs::read(&file).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
                match capsule_entry(&c, version, about, icon) {
                    Ok(e) => {
                        if let Source::Path(p) = &e.source {
                            uploads.push((p.trim_start_matches('/').to_string(), std::fs::canonicalize(&file).unwrap_or(file)));
                        }
                        kists.push(e);
                    }
                    Err(why) => {
                        skipped += 1;
                        eprintln!("kistcat: warning: {} skipped: {why}", file.display());
                    }
                }
            }
            Spec::Manifest { url, name, option, about, icon, platform, arch } => {
                let (Some(name), Some(option), Some(about)) = (name, option, about) else { fail(format!("{url}: needs its --name, --option and --about")) };
                match fetch(&url).and_then(|doc| manifest_entry(&doc, &url, name.clone(), option, about, icon, &platform, &arch, pinned)) {
                    Ok(Some(e)) => kists.push(e),
                    Ok(None) => {
                        skipped += 1;
                        eprintln!("kistcat: warning: {name} skipped: {url} has no {platform}/{arch} artefact yet (not published)");
                    }
                    Err(why) => {
                        skipped += 1;
                        eprintln!("kistcat: warning: {name} skipped: {why}");
                    }
                }
            }
        }
    }
    let doc = sign(&kists, vsf::eagle_time_oscillations(), &key).unwrap_or_else(|e| fail(e));
    // The gate before anything is written: what was just signed passes the device's own check.
    let dir = directory::verify_directory(&doc, &key.public()).unwrap_or_else(|e| fail(format!("the directory just built does not verify: {e}")));
    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    let (vsf_out, html_out) = (out.join("kists.vsf"), out.join("index.html"));
    std::fs::write(&vsf_out, &doc).unwrap_or_else(|e| fail(format!("{}: {e}", vsf_out.display())));
    std::fs::write(&html_out, page(&dir)).unwrap_or_else(|e| fail(format!("{}: {e}", html_out.display())));
    if let Some(web) = &a.web {
        let site = std::fs::read_to_string(web).unwrap_or_else(|e| fail(format!("{}: {e}", web.display())));
        let site = splice(&site, &fragment(&dir)).unwrap_or_else(|e| fail(format!("{}: {e}", web.display())));
        std::fs::write(web, site).unwrap_or_else(|e| fail(format!("{}: {e}", web.display())));
    }
    println!("kistcat: G#{:X} kists listed, G#{:X} skipped; {} (G#{:X} bytes, signed by {}) and {}", dir.kists.len(), skipped, vsf_out.display(), doc.len(), short(&dir.signer), html_out.display());
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    println!("publish (R2 bucket kists, served at http://{}/):", directory::HOST);
    for (k, f) in &uploads {
        println!("  npx wrangler r2 object put kists/{k} --file {} --content-type application/vnd.vsf --cache-control 'public, max-age=31536000, immutable' --remote", f.display());
    }
    println!("  npx wrangler r2 object put kists{} --file {} --content-type application/vnd.vsf --cache-control no-cache --remote", directory::PATH, abs(&vsf_out).display());
    println!("  npx wrangler r2 object put kists/index.html --file {} --content-type 'text/html; charset=utf-8' --cache-control no-cache --remote", abs(&html_out).display());
}

fn verify(args: &[String]) {
    let a = parse_args(args).unwrap_or_else(|e| fail(e));
    let (Some(doc_path), Some(key_path)) = (a.doc, a.key) else { usage() };
    let key = signing_bundle(&key_path, a.signer.as_deref().unwrap_or(SIGNER_NAME));
    let doc = std::fs::read(&doc_path).unwrap_or_else(|e| fail(format!("{}: {e}", doc_path.display())));
    match directory::verify_directory(&doc, &key.public()) {
        Ok(d) => {
            println!("{}: signed by {} with every egg, made {}, G#{:X} kists", doc_path.display(), short(&d.signer), day(d.made), d.kists.len());
            for e in &d.kists {
                let at = match &e.source {
                    Source::Path(p) => format!("http://{}{p}", directory::HOST),
                    Source::Manifest { url, platform, arch } => format!("{platform}/{arch} in {url}"),
                };
                println!("  {} {} by {}: G#{:X} bytes, blake3 {}, {at}", e.name, version_text(&e.version), developer_text(e), e.size, hex(&e.capsule));
            }
        }
        Err(e) => fail(format!("{}: {e}", doc_path.display())),
    }
}

/// The date for people (display only; the directory keeps Eagle time).
fn day(made: i64) -> String {
    vsf::EagleTime::from_oscillations(made).to_datetime().format("%Y-%m-%d").to_string()
}

fn version_text(v: &Version) -> String {
    format!("G#{:X}.{:X}.{:X}", v.major, v.minor, v.patch)
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
   <meta name="description" content="Kists: apps for ferros, listed by anyone, installed with no account. The list vouches for nothing; your device checks every kist itself.">
   <style>
{STYLE}{KISTS_STYLE}   </style>
</head>
<body>
   <main>
      <nav>
         <a href="https://ferros.org/">ferros.org</a> · <a href="{path}">kists.vsf</a> · <a href="https://github.com/nickspiker/ferros/blob/forge/docs/KISTS.md">how kists work</a>
      </nav>

      <h1>kists<span>.fgtw.org</span></h1>
      <p class="thesis">Apps for ferros, listed by anyone,<br>installed with no account.</p>

      <table class="ledger">
         <tr><td>Kists listed</td><td>G#{count:X}</td></tr>
         <tr><td>Developers</td><td>G#{devs:X}</td></tr>
         <tr><td>Last updated</td><td>{day}</td></tr>
      </table>

{search}{table}
      <h2>Getting one</h2>
      <p>Kists run on <a href="https://ferros.org/">ferros</a> today. A kist is packed the same for every platform; Linux, Android and macOS will run them thru a host app once kists carry portable code, and none is out yet. For now, use ferros.</p>

      <h2>What this list is</h2>
      <p>Anyone may list a kist, and nobody needs an account to install one. The list vouches for nothing: your device checks each kist against its hash here and against its developer's signature when it installs it. The list itself is signed, so nobody between can change it unseen.</p>
      <p>No fees, no ads, no stars. Measures will come, many of them, and never a single score.</p>
   </main>
{script}</body>
</html>
"#,
        path = directory::PATH,
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

    /// A capsule signed as the bridge signs a kist: its declaration, its code, every egg.
    fn capsule(key: &SigningBundle, name: &str, eggs: bool) -> Vec<u8> {
        let d = ferros_kist::Decl { name: name.into(), wants: vec![] };
        let (k, _) = ferros_kist::declaration_sections(&d, Some([5; 32]));
        let mut code = vsf::file_format::VsfSection::new("aarch64");
        code.add_field("code", VsfType::v(b'b', vec![7; 64]));
        let pubkey = key.keypair().public.to_bytes();
        let b = vsf::VsfBuilder::new().add_section_direct(k).add_section_direct(code);
        if eggs {
            fgtw::release::sign_release(b.signed_only_eggs(VsfType::ke(pubkey.to_vec()), &fgtw::release::release_slots()).build().unwrap(), key).unwrap()
        } else {
            vsf::verification::sign_file(b.signature_ed25519(pubkey, [0; 64]).build().unwrap(), &[9; 32]).unwrap()
        }
    }

    fn version() -> Version {
        Version { major: 0, minor: 1, patch: 0 }
    }

    #[test]
    fn build_then_verify_round_trips_and_a_changed_byte_fails() {
        let c = capsule(&developer(), "Clock", true);
        let e = capsule_entry(&c, version(), "the time, drawn as rings".into(), None).unwrap();
        assert_eq!(e.name, "Clock");
        assert_eq!(e.option, [5; 32]);
        assert_eq!(e.capsule, *blake3::hash(&c).as_bytes());
        let project = SigningBundle::developer(&[1; 32], SIGNER_NAME);
        let doc = sign(&[e.clone()], 7, &project).unwrap();
        let d = directory::verify_directory(&doc, &project.public()).unwrap();
        assert_eq!(d.kists, vec![e]);
        for at in [0, doc.len() >> 2, doc.len() >> 1, doc.len() - 1] {
            let mut t = doc.clone();
            t[at] ^= 1;
            assert!(directory::verify_directory(&t, &project.public()).is_err(), "a byte changed at {at}");
        }
        // Another key's directory is not the project's.
        let other = SigningBundle::developer(&[2; 32], SIGNER_NAME);
        assert!(directory::verify_directory(&doc, &other.public()).is_err());
    }

    #[test]
    fn a_capsule_with_one_egg_is_not_listed() {
        let c = capsule(&developer(), "Clock", false);
        assert!(capsule_entry(&c, version(), "x".into(), None).is_err());
    }

    /// A manifest as Photon's tool writes one (photon-manifest): a section per artefact, signed with every egg of the release-shaped key.
    fn manifest(key: &SigningBundle, artefacts: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(1).signed_only_eggs(VsfType::ke(key.keypair().public.to_bytes().to_vec()), &fgtw::release::release_slots());
        for (platform, arch, url) in artefacts {
            let mut s = vsf::file_format::VsfSection::new("manifest.photon.release");
            s.add_field("platform", VsfType::x(platform.to_string()));
            s.add_field("arch", VsfType::x(arch.to_string()));
            s.add_field("minor", VsfType::z(150));
            s.add_field("size", VsfType::z(4096));
            s.add_field("url", VsfType::nu(url.to_string()));
            s.add_field("hash", VsfType::hb(vec![3; 32]));
            b = b.add_section_direct(s);
        }
        fgtw::release::sign_release(b.build().unwrap(), key).unwrap()
    }

    #[test]
    fn a_manifest_without_the_artefact_is_skipped() {
        let key = SigningBundle::release(&[7; 32]);
        let pin = |k: &[u8; 32]| (*k == key.keypair().public.to_bytes()).then(|| key.public());
        let url = "http://example.org/manifest-release.vsf";
        let without = manifest(&key, &[("Android", "arm64", "https://example.org/a.apk")]);
        assert!(manifest_entry(&without, url, "Photon".into(), [8; 32], "x".into(), None, PLATFORM, ARCH, pin).unwrap().is_none());
        let with = manifest(&key, &[("Android", "arm64", "https://example.org/a.apk"), ("ferros", "arm64", "http://example.org/photon.vsf")]);
        let e = manifest_entry(&with, url, "Photon".into(), [8; 32], "x".into(), None, PLATFORM, ARCH, pin).unwrap().unwrap();
        assert_eq!((e.capsule, e.size, e.version), ([3; 32], 4096, Version { major: 0, minor: 150, patch: 0 }));
        assert_eq!(e.source, Source::Manifest { url: url.into(), platform: PLATFORM.into(), arch: ARCH.into() });
        // An https-only artefact: the device could not fetch it.
        let tls = manifest(&key, &[("ferros", "arm64", "https://example.org/photon.vsf")]);
        assert!(manifest_entry(&tls, url, "Photon".into(), [8; 32], "x".into(), None, PLATFORM, ARCH, pin).is_err());
        // A manifest whose signer is not pinned is refused, and so is one with a changed byte.
        assert!(manifest_entry(&with, url, "Photon".into(), [8; 32], "x".into(), None, PLATFORM, ARCH, |_| None).is_err());
        let mut t = with.clone();
        let n = t.len();
        t[n - 2] ^= 1;
        assert!(manifest_entry(&t, url, "Photon".into(), [8; 32], "x".into(), None, PLATFORM, ARCH, pin).is_err());
    }

    #[test]
    fn splice_replaces_only_between_the_markers() {
        let site = "a\n<!-- kists: begin (note) -->\nold\n<!-- kists: end -->\nb";
        let out = splice(site, "new\n").unwrap();
        assert_eq!(out, "a\n<!-- kists: begin (note) -->\nnew\n<!-- kists: end -->\nb");
        assert_eq!(splice(&out, "new\n").unwrap(), out);
        assert!(splice("no markers", "x").is_err());
    }

    #[test]
    fn the_page_lists_searches_and_escapes() {
        let e = capsule_entry(&capsule(&developer(), "Clock", true), version(), "rings <script>".into(), None).unwrap();
        let d = Directory { made: 0, signer: [0; 32], kists: vec![e] };
        let p = page(&d);
        assert!(p.contains("<td>Clock</td>") && p.contains("G#0.1.0") && p.contains("rings &lt;script&gt;"));
        assert!(p.contains("<tr><td>Kists listed</td><td>G#1</td></tr>") && p.contains("id=\"q\""));
        assert!(!fragment(&d).contains("<script"), "ferros.org runs no script");
        let empty = page(&Directory { made: 0, signer: [0; 32], kists: vec![] });
        assert!(empty.contains("None listed yet") && !empty.contains("<script"));
    }

    #[test]
    fn developer_text_is_escaped() {
        assert_eq!(esc("<b>\"x\" & 'y'</b>"), "&lt;b&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/b&gt;");
    }
}
