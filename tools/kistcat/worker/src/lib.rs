//! kists.fgtw.org's Worker (owner, 2026-11-10): the bare address, and developers appending their own entries (docs/KIST-DISTRIBUTION.md 1.6; owner, 2026-11-24: "fgtw.org is just the bootstrap/phonebook and host for a few things. devs should be able to append to that list at will.").
//! - `/` (GET): R2 serves an object only by its exact key and has no index page, so `/` gets `index.html` from the bucket (tools/kistcat writes it).
//! - `/append` (POST): one complete VSF entry exactly as its developer signed it (ferros_kist `entry`). No gatekeeper, no account, no approval, no fee: the checks are the signature and the key's own history, never the content.
//! The routes are those two paths alone (wrangler.toml); the directory, the capsules, the entries and everything else go to R2 directly, never thru this.
//! Plain HTTP as the rest of the host: no redirect; the record carries its own signatures, so plain HTTP costs nothing.

use ferros_kist::entry::{self, Entry};
use worker::*;

/// An entry is about 10 KB (its SPHINCS+ egg most of it); a body past 64 KiB is no entry and is not read further.
const ENTRY_MOST: usize = 1 << 16;

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    match (req.method(), req.path().as_str()) {
        (Method::Get, "/") => page(&env).await,
        (Method::Post, "/append") => {
            let body = req.bytes().await?;
            let (taken, why) = match append(&env, &body).await {
                Ok(why) => (true, why),
                Err(why) => (false, why),
            };
            reply(taken, &why)
        }
        // Anything else reaching here is a misrouted request, and R2 is where it belongs.
        _ => Response::error("Not found", 404),
    }
}

async fn page(env: &Env) -> Result<Response> {
    let Some(page) = env.bucket("KISTS")?.get("index.html").execute().await? else {
        return Response::error("The kist list is not published yet", 404);
    };
    let bytes = page.body().ok_or("index.html has no body")?.bytes().await?;
    let mut headers = Headers::new();
    headers.set("Content-Type", "text/html; charset=utf-8")?;
    headers.set("Cache-Control", "no-cache")?;
    Ok(Response::from_bytes(bytes)?.with_headers(headers))
}

/// Eagle time now, in hydrogen oscillations: the Worker's only clock is the platform's milliseconds since 1970, converted once here (as fgtw-bootstrap converts it).
fn eagle_now() -> i64 {
    let ms = Date::now().as_millis() as i64;
    // The Eagle epoch, July 20, 1969, 20:17:40 UTC, is 14,182,940 s before the platform's.
    const EAGLE_TO_PLATFORM_S: i64 = 14_182_940;
    let per_s = vsf::OSCILLATIONS_PER_SECOND as i64;
    (ms / 1000 + EAGLE_TO_PLATFORM_S) * per_s + (ms % 1000) * per_s / 1000
}

/// The answer, a small complete VSF document: section `append` { `taken` (u, 1 or 0), `why` (a, the words) }.
fn reply(taken: bool, why: &str) -> Result<Response> {
    let mut s = vsf::file_format::VsfSection::new("append");
    s.add_field("taken", vsf::VsfType::u(taken as usize, false));
    let words = vsf::types::Ascii::new(why).unwrap_or_else(|_| vsf::types::Ascii::new("refused").unwrap());
    s.add_field("why", vsf::VsfType::a(words));
    let doc = vsf::VsfBuilder::new().creation_time_oscillations(eagle_now()).provenance_only().add_section_direct(s).build().map_err(|e| Error::RustError(e))?;
    let mut headers = Headers::new();
    headers.set("Content-Type", "application/vnd.vsf")?;
    headers.set("Cache-Control", "no-store")?;
    Ok(Response::from_bytes(doc)?.with_status(if taken { 200 } else { 422 }).with_headers(headers))
}

async fn get(bucket: &Bucket, key: &str) -> Result<Option<Vec<u8>>> {
    match bucket.get(key).execute().await? {
        Some(o) => Ok(Some(o.body().ok_or("an object without a body")?.bytes().await?)),
        None => Ok(None),
    }
}

async fn put(bucket: &Bucket, key: &str, bytes: Vec<u8>, cache: &str) -> Result<()> {
    let meta = HttpMetadata { content_type: Some("application/vnd.vsf".into()), cache_control: Some(cache.into()), ..Default::default() };
    bucket.put(key, bytes).http_metadata(meta).execute().await?;
    Ok(())
}

/// One entry appended (KIST-DISTRIBUTION.md 1.6), its checks cheapest first so junk costs least: the shape, then every egg against the bundle it carries, then the same bundle as this developer key's first entry here, then newest only per (developer, option). Ok(the words) taken, Err(the words) refused. Hand-off and revocation records are not built yet (1.5), so only entries are taken.
async fn append(env: &Env, body: &[u8]) -> std::result::Result<String, String> {
    let fail = |e: Error| format!("the bucket: {e}");
    // WHY the bound: the body is anyone's; past an entry's size it is not one, and nothing more of it is read.
    if body.len() > ENTRY_MOST {
        return Err("longer than an entry".into());
    }
    // 0: the shape.
    entry::read(body).map_err(|w| format!("not an entry: {w}"))?;
    // 1: every egg.
    let e: Entry = entry::verify(body).map_err(|w| format!("its signature does not check out: {w}"))?;
    let bucket = env.bucket("KISTS").map_err(fail)?;
    let dev = entry::hex(&e.developer);
    // 2: the same developer key with the same bundle as its first entry here (trust on first stow, as a person's devices pin it).
    let first_key = format!("developers/{dev}.entry.vsf");
    match get(&bucket, &first_key).await.map_err(fail)? {
        Some(first) => {
            let p = entry::read(&first).map_err(|w| format!("this key's first entry here does not read: {w}"))?;
            if p.bundle != e.bundle {
                return Err("signed by another bundle than this developer key's first entry here".into());
            }
        }
        None => put(&bucket, &first_key, body.to_vec(), "no-cache").await.map_err(fail)?,
    }
    // 4: newest only, per developer and option, by the stage rule.
    let kist_key = format!("entries/{dev}/{}.entry.vsf", entry::hex(&e.option));
    if let Some(held) = get(&bucket, &kist_key).await.map_err(fail)? {
        let h = entry::read(&held).map_err(|w| format!("the entry held does not read: {w}"))?;
        if e.version == h.version {
            return if e.capsule == h.capsule { Ok("taken already: the same entry is held".into()) } else { Err("the same version naming another capsule (the developer's own equivocation)".into()) };
        }
        if e.version < h.version {
            return Err("older than the entry held for this kist".into());
        }
    }
    put(&bucket, &kist_key, body.to_vec(), "no-cache").await.map_err(fail)?;
    put(&bucket, &entry::path(&e.capsule)[1..], body.to_vec(), "public, max-age=31536000, immutable").await.map_err(fail)?;
    // The directory assembled from the entries held, newest per kist, each carried byte for byte (1.1).
    let mut docs = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut l = bucket.list().prefix("entries/");
        if let Some(c) = cursor.take() {
            l = l.cursor(c);
        }
        let page = l.execute().await.map_err(fail)?;
        for o in page.objects() {
            if let Some(d) = get(&bucket, &o.key()).await.map_err(fail)? {
                docs.push(d);
            }
        }
        match page.cursor() {
            Some(c) if page.truncated() => cursor = Some(c),
            _ => break,
        }
    }
    let dir = ferros_kist::directory::build_held(&docs, eagle_now()).map_err(|w| format!("the directory: {w}"))?;
    put(&bucket, "kists.vsf", dir, "no-cache").await.map_err(fail)?;
    Ok(format!("taken: {} version {} is listed", e.name, e.version.0.iter().map(|p| format!("A#{p}")).collect::<Vec<_>>().join(".")))
}
