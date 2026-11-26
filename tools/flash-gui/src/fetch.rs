//! The signed release, fetched once at the start: checked against the ferros key before anything in it is trusted, and the mark (logo.vsf, role `logo`) taken from it, checked against its BLAKE3.
//! `base` is https://get.ferros.org, or a directory a staged release sits in (tools/flash/publish.sh's output), so the window can be tried against a release before it is published.

use ferros_flash::release::{FERROS_PUB, Release, Role};

/// What the fetch found.
pub struct Fetched {
    pub release: Release,
    /// The mark's VSF document, when the release carries one.
    pub logo: Option<Vec<u8>>,
}

/// One file of the release's host, by name.
fn get(base: &str, name: &str) -> Result<Vec<u8>, String> {
    if base.starts_with("http://") || base.starts_with("https://") {
        let r = ureq::get(&format!("{base}/{name}")).call().map_err(|e| e.to_string())?;
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut r.into_reader(), &mut b).map_err(|e| e.to_string())?;
        Ok(b)
    } else {
        std::fs::read(std::path::Path::new(base).join(name)).map_err(|e| format!("{base}/{name}: {e}"))
    }
}

/// The release and its mark.
pub fn release(base: &str) -> Result<Fetched, String> {
    let doc = get(base, "release.vsf")?;
    let release = Release::verify(&doc, &FERROS_PUB)?;
    let logo = match release.find(Role::Logo, "", "") {
        Some(art) => {
            let bytes = get(base, &art.file_name())?;
            art.check(&bytes)?;
            Some(bytes)
        }
        None => None,
    };
    Ok(Fetched { release, logo })
}

/// The fetch on its own thread; `deliver` gets the result once.
pub fn start(base: String, deliver: impl FnOnce(Result<Fetched, String>) + Send + 'static) {
    std::thread::Builder::new()
        .name("release fetch".into())
        .spawn(move || deliver(release(&base)))
        .expect("the release fetch thread");
}
