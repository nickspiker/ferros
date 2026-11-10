//! kists.fgtw.org's bare address (owner, 2026-11-10). R2 serves an object only by its exact key and has no index page, so `/` comes here and gets `index.html` from the bucket (tools/kistcat writes it). The route is the bare address alone (wrangler.toml); the directory, the capsules and everything else go to R2 directly, never thru this.
//! Plain HTTP as the rest of the host: no redirect, nothing added but the page's type.

use worker::*;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    // The route sends only `/`; anything else reaching here is a misrouted request, and R2 is where it belongs.
    if req.path() != "/" {
        return Response::error("Not found", 404);
    }
    let Some(page) = env.bucket("KISTS")?.get("index.html").execute().await? else {
        return Response::error("The kist list is not published yet", 404);
    };
    let bytes = page.body().ok_or("index.html has no body")?.bytes().await?;
    let mut headers = Headers::new();
    headers.set("Content-Type", "text/html; charset=utf-8")?;
    headers.set("Cache-Control", "no-cache")?;
    Ok(Response::from_bytes(bytes)?.with_headers(headers))
}
