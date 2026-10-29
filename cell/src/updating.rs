//! A deploy refusal opened as a document: a quiet page that asks again.
//! JSON clients keep the original refusal, including its message.

use fragment_proto::{ErrorBody, ErrorCode};
use worker::{Request, Response};

use crate::{config::Config, error::CellResult, shell};

/// Explicit fetch metadata wins over Accept: a fetch asking for HTML is
/// still a fetch. Older clients must explicitly prefer HTML to JSON.
pub(crate) fn navigation(req: &Request) -> CellResult<bool> {
    let mode = req.headers().get("sec-fetch-mode")?;
    let dest = req.headers().get("sec-fetch-dest")?;
    if mode.as_deref() == Some("navigate") || matches!(dest.as_deref(), Some("document" | "iframe" | "frame")) {
        return Ok(true);
    }
    if mode.is_some() || dest.is_some() {
        return Ok(false);
    }
    let accept = req.headers().get("accept")?.unwrap_or_default();
    let quality = |media: &str| -> Option<f32> {
        accept.split(',').filter_map(|part| {
            let mut params = part.split(';');
            if !params.next()?.trim().eq_ignore_ascii_case(media) {
                return None;
            }
            let q = params.find_map(|p| {
                let (key, value) = p.trim().split_once('=')?;
                key.eq_ignore_ascii_case("q").then(|| value.trim().parse::<f32>().unwrap_or(0.0))
            }).unwrap_or(1.0);
            Some(if (0.0..=1.0).contains(&q) { q } else { 0.0 })
        }).reduce(f32::max)
    };
    let json = quality("application/json").or_else(|| quality("application/*")).or_else(|| quality("*/*")).unwrap_or(0.0);
    Ok(quality("text/html").is_some_and(|html| html > json))
}

pub(crate) async fn answer(mut resp: Response, env: &worker::Env, platform: bool) -> CellResult<Response> {
    if resp.status_code() != 503 || !resp.headers().get("content-type")?.is_some_and(|t| t.starts_with("application/json")) {
        return Ok(resp);
    }
    let body = resp.cloned()?.json::<ErrorBody>().await;
    if !matches!(body, Ok(b) if b.error == ErrorCode::Deploying) {
        return Ok(resp);
    }
    let file = fragment_templates::shell("updating.html").expect("the updating page is in the release");
    let bytes = crate::assets::read(env, file).await?;
    let html = std::str::from_utf8(&bytes).expect("the updating page is UTF-8");
    let sheet = if platform { shell::versioned("sheet.css") } else { shell::fragment_sheet() };
    let h = resp.headers().clone();
    h.delete("content-length")?;
    h.delete("content-encoding")?;
    h.delete("etag")?;
    h.set("content-type", "text/html; charset=utf-8")?;
    h.set("cache-control", "no-store")?;
    h.set("retry-after", "3")?;
    // No script or inline style; meta refresh works under this policy.
    // Keep any original policy too. This notice carries no private data.
    let ancestors = if platform { "'none'".into() } else { format!("'self' {}", Config::from_env(env).platform()) };
    h.append("content-security-policy", &format!("default-src 'none'; style-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors {ancestors}"))?;
    Ok(Response::from_html(html.replace("{{sheet}}", &sheet))?.with_status(503).with_headers(h))
}
