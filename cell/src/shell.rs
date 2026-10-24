//! The shell (docs/cloudflare-v1.md, decisions 6–12): the platform's one
//! page, at `/`, `/settings` and `/welcome`, Finite's about page, privacy
//! policy and terms at `/about`, `/privacy` and `/terms`, and its files at
//! `/__shell/<file>`, the release's (cell/shell/), read from its Static
//! Assets (assets.rs). It holds no key: its script calls the API with
//! the person's platform session (lib.rs `shell_session`) and frames their
//! fragments, each signed in on its own origin by the platform's frame mint.
//!
//! Its policy: scripts and styles from this origin only (and Google
//! Fonts' stylesheet and files); frames of this origin (the share sheet,
//! the frame mint's redirects) and of the fragments' and computers' hosts
//! under the suffix; never framed itself.

use fragment_proto::ErrorCode;
use fragment_templates::File;
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};

/// A file's version: its bytes' hash, its first 20 hex digits (a release
/// changes them).
fn version(f: &File) -> &'static str {
    &f.sha256[..20]
}

/// A file's validator: its version.
fn etag(f: &File) -> String {
    format!("\"s-{}\"", version(f))
}

/// How long a browser keeps a file asked for by its version: for good, as
/// the version names its bytes.
const PINNED: &str = "public, max-age=31536000, immutable";

/// One of the files the shell publishes, by its name: a page links only
/// those.
fn published(name: &str) -> &'static File {
    fragment_templates::shell_published(name).unwrap_or_else(|| panic!("the shell publishes {name}")).0
}

/// `/__shell/<name>?v=<version>`: one of the shell's files as a page links
/// it to be kept (`asset`: a browser asks for it once, not on every page;
/// a release that changes it links another version). The platform's pages
/// link their stylesheets and script so: the share sheet, framed by the
/// shell each time it opens, waited a round trip on its stylesheet every
/// time (share.rs `platform_html`).
pub(crate) fn versioned(name: &str) -> String {
    format!("/__shell/{name}?v={}", version(published(name)))
}

/// The pages' base stylesheet on a fragment's own origin, where the
/// platform's notices there link it (share.rs `sheet_html`): they load
/// nothing from the platform's origin.
const FRAGMENT_SHEET: &str = "__sheet.css";

/// Whether a path on a fragment's origin is the base stylesheet's.
pub(crate) fn is_fragment_sheet(rest: &str) -> bool {
    rest == FRAGMENT_SHEET
}

/// `/__sheet.css?v=<version>`, as a fragment's origin serves `sheet.css`
/// (`fragment_sheet_asset`), linked by its version as `versioned` links.
pub(crate) fn fragment_sheet() -> String {
    format!("/{FRAGMENT_SHEET}?v={}", version(published("sheet.css")))
}

/// `GET /__sheet.css` on a fragment's origin: the shell's `sheet.css`,
/// kept or revalidated as `asset` keeps it.
pub async fn fragment_sheet_asset(req: &Request, env: &Env) -> CellResult<Response> {
    asset(req, env, "sheet.css").await?.ok_or_else(|| CellError::host("the shell publishes sheet.css"))
}

fn not_modified(req: &Request, tag: &str) -> Result<bool> {
    Ok(req.headers().get("if-none-match")?.is_some_and(|v| v.split(',').any(|t| t.trim() == tag)))
}

/// One of the shell's pages, its file in the release.
fn page_file(name: &str) -> &'static File {
    fragment_templates::shell(name).unwrap_or_else(|| panic!("the shell's {name} is in the release's index"))
}

/// Where the shell may frame: this origin, and every fragment's and
/// computer's host under the suffix.
fn frame_src(cfg: &Config, url: &Url) -> String {
    let platform = cfg.platform();
    let scheme = url.scheme();
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    format!("'self' {platform} {scheme}://*.{}{port}", cfg.host_suffix)
}

/// `GET /`, `GET /settings` and `GET /welcome`: the shell's page, for
/// anyone (signed out, it asks them to sign in; the shell opens the view
/// its path names), naming the release that answers it
/// (`fragment_templates::shell_page`): its script reloads at a safe moment
/// once a later answer names another (docs/api.md, The shell). A few KiB,
/// read whole to fill it.
pub async fn page(req: &Request, env: &Env, cfg: &Config, url: &Url) -> CellResult<Response> {
    // its bytes are checked against their hash as they are read: UTF-8, as the release's index.html is
    let bytes = crate::assets::read(env, page_file("index.html")).await?;
    let filled = fragment_templates::shell_page(std::str::from_utf8(&bytes).map_err(|_| CellError::host("the release's shell page is not UTF-8"))?, &crate::levers::answered_release(cfg, req));
    let h = Headers::new();
    h.set("content-type", "text/html; charset=utf-8")?;
    h.set("cache-control", "no-store")?;
    h.set(
        "content-security-policy",
        &format!(
            "default-src 'self'; script-src 'self'; style-src 'self' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; img-src 'self' data: blob:; connect-src 'self'; frame-src {}; manifest-src 'self'; form-action 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'",
            frame_src(cfg, url)
        ),
    )?;
    h.set("x-frame-options", "DENY")?;
    // a page that opens it in a window keeps no hold on it, as on every platform page
    h.set("cross-origin-opener-policy", "same-origin")?;
    h.set("referrer-policy", "strict-origin-when-cross-origin")?;
    Ok(Response::ok(filled)?.with_headers(h))
}

/// `GET /admin`: the operators' admin page (decision 59), to the platform
/// session of one of the deployment's operators alone, by the rule the
/// admin's API holds the shell's session to (orgs.rs `session_operator`).
/// To anyone else, signed out, signed in, or with a session that ended,
/// it is the router's 404 for a path it lacks (lib.rs `no_route`), word
/// for word: no one else learns it is here. Its API decides again. It
/// frames nothing and is never framed.
pub async fn admin_page(req: &Request, env: &Env, cfg: &Config, url: &Url) -> CellResult<Response> {
    let missing = || crate::no_route(url.path());
    let Some(token) = crate::auth::platform_session_token(req, url)? else { return Err(missing()) };
    match crate::orgs::session_operator(env, cfg, token).await {
        Ok(Some(_)) => {}
        Ok(None) => return Err(missing()),
        // a session not live (401), or a person being wiped (403), is no operator's
        Err(e) if matches!(e.code, ErrorCode::Unauthenticated | ErrorCode::Forbidden) => return Err(missing()),
        Err(e) => return Err(e),
    }
    let h = Headers::new();
    h.set("content-type", "text/html; charset=utf-8")?;
    h.set("cache-control", "no-store")?;
    h.set(
        "content-security-policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-src 'none'; form-action 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'",
    )?;
    h.set("x-frame-options", "DENY")?;
    h.set("cross-origin-opener-policy", "same-origin")?;
    h.set("referrer-policy", "same-origin")?;
    crate::assets::body(env, page_file("admin.html"), h).await
}

/// `GET /about`, `GET /privacy` and `GET /terms`: Finite's about page,
/// privacy policy and terms (cell/shell/about.html, privacy.html,
/// terms.html, with `platform-pages.css`, its fonts, and `legal.css`), for
/// anyone, signed in or not: the pages the first run's footer and Settings
/// link, and the deployment names to WorkOS and Stripe. Static: no script,
/// no form, never framed. Revalidated by their hash (`no-cache`), a 304
/// answered before any read.
pub async fn legal_page(req: &Request, env: &Env, name: &str) -> CellResult<Response> {
    let h = Headers::new();
    h.set("content-type", "text/html; charset=utf-8")?;
    h.set("content-security-policy", "default-src 'none'; style-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")?;
    h.set("x-frame-options", "DENY")?;
    h.set("cross-origin-opener-policy", "same-origin")?;
    h.set("referrer-policy", "strict-origin-when-cross-origin")?;
    revalidated(req, env, page_file(name), h, "no-cache").await
}

/// `GET /__shell/<file>`: one of the files it publishes
/// (`fragment_templates::shell_published`), or `None`. Asked for by its
/// version (`?v=`, `versioned`), it is kept for good; by its name alone, or
/// by a version this release does not hold (a page of the last one's), it
/// is revalidated each time.
pub async fn asset(req: &Request, env: &Env, name: &str) -> CellResult<Option<Response>> {
    let Some((f, mime)) = fragment_templates::shell_published(name) else { return Ok(None) };
    let h = Headers::new();
    h.set("content-type", mime)?;
    h.set("x-content-type-options", "nosniff")?;
    let pinned = req.url()?.query_pairs().any(|(k, v)| k == "v" && v == version(f));
    Ok(Some(revalidated(req, env, f, h, if pinned { PINNED } else { "no-cache" }).await?))
}

/// A file of the release, answered with `h` and `cache`: revalidated by
/// its hash (a release changes it under the same name) unless `cache` keeps
/// it, a 304 and a HEAD answered from the index, before any read.
async fn revalidated(req: &Request, env: &Env, f: &File, h: Headers, cache: &str) -> CellResult<Response> {
    let tag = etag(f);
    h.set("cache-control", cache)?;
    h.set("etag", &tag)?;
    if not_modified(req, &tag)? {
        return Ok(Response::empty()?.with_status(304).with_headers(h));
    }
    if req.method() == Method::Head {
        h.set("content-length", &f.size.to_string())?;
        return Ok(Response::empty()?.with_headers(h));
    }
    crate::assets::body(env, f, h).await
}
