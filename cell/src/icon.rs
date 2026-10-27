//! A fragment's icon (fragment_core::icon, its pure part; docs/api.md,
//! Icons): made from drawings its editors pick among, or uploaded, kept
//! as one of the fragment's blobs (named in its meta, which the blob
//! collection keeps), served to its viewers, and shown in its members'
//! lists as its picture. An agent fragment's picture is its avatar
//! (crate::plane), never an icon.
//!
//! A making runs in the request that asks for it: a subject, then
//! `icon::CANDIDATES` drawings at once, each cut out, examined, fitted and
//! checked (a minute at most, the drawings' wait). Each vendor call that
//! answers is charged to the fragment's owner as it answers, one meter
//! row each, named by its making (`icon::meter_ref`, `icon::operation`):
//! a making that fails halfway has paid for what it bought. Nothing of a
//! making is kept but what it answers: the asker keeps one by sending its
//! bytes back (`PUT`). Keeping and removing one are writes, refused while
//! the owner's fragments are read-only (`writable`).

use base64::Engine;
use fragment_core::icon::{self, Flaw, Refusal};
use fragment_core::ledger::Spend;
use fragment_core::price::Usage;
use fragment_core::{blob, decide, face};
use fragment_proto::{ErrorCode, FragmentKind, IdentityKind, Role};
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::{Caller, FragmentCell, MetaKey};
use crate::js;
use crate::ledger::MaySpend;

/// An icon is fetched again at most this often: its address names its
/// bytes (`icon::icon_path`), so a new one is a new address.
const ICON_CACHE: &str = "private, max-age=86400";

fn refused(r: Refusal) -> CellError {
    let code = match r {
        Refusal::Busy => ErrorCode::RateLimited,
        Refusal::Hint | Refusal::NotAnImage | Refusal::Agent => ErrorCode::InvalidRequest,
    };
    CellError::new(code, r.message())
}

/// Whether the caller spends here as the fragment's owner: they, or an
/// agent of theirs. Anyone else's making is held to the fragment's cap,
/// as a job's AI step is (ai.rs `paying`; docs/ledger.md, Caps).
fn spends_as_owner(caller: &Caller, owner: &str) -> bool {
    caller.principal() == Some(owner) || (caller.kind() == Some(IdentityKind::Agent) && caller.owner() == Some(owner))
}

/// A making's or an upload's own name in its meter rows: when it started
/// and four random bytes, so two in one millisecond are two.
fn operation(started_ms: i64) -> String {
    icon::operation(started_ms, js::random_bytes::<4>())
}

impl FragmentCell {
    /// The icon's SHA-256, when it has one.
    fn icon_sha(&self) -> CellResult<Option<String>> {
        self.meta(MetaKey::Icon)
    }

    /// Where its lists show its icon, when it has one and is no agent.
    pub(crate) fn icon_shown(&self, kind: FragmentKind) -> CellResult<Option<String>> {
        if kind == FragmentKind::Agent {
            return Ok(None);
        }
        let Some(sha) = self.icon_sha()? else { return Ok(None) };
        Ok(Some(icon::icon_path(&self.name()?, &sha)))
    }

    /// Its kind and title as its lists show them now (`MetaKey::Face`):
    /// an app's, untitled, before its first install.
    fn shown_as(&self) -> CellResult<(FragmentKind, Option<String>)> {
        let face: Value = self.meta(MetaKey::Face)?.and_then(|f| serde_json::from_str(&f).ok()).unwrap_or(Value::Null);
        let kind = face["kind"].as_str().and_then(FragmentKind::parse).unwrap_or_default();
        Ok((kind, face["title"].as_str().map(str::to_string)))
    }

    /// Its lists show its icon as it is now, sent in this request (a
    /// row that fails waits for the alarm, as any index change's).
    async fn show_icon(&self) -> CellResult<()> {
        let (kind, title) = self.shown_as()?;
        self.face_is(kind, title.as_deref(), self.icon_shown(kind)?.as_deref())?;
        self.flush_index().await;
        Ok(())
    }

    /// Refuses an agent fragment: its picture is its avatar.
    fn icon_taker(&self) -> CellResult<()> {
        match self.shown_as()?.0 {
            FragmentKind::Agent => Err(refused(Refusal::Agent)),
            _ => Ok(()),
        }
    }

    /// One meter row of a making (`op`, `operation`), on the owner's
    /// ledger; a row not kept is said and the making goes on (it was
    /// bought either way).
    fn icon_charge(&self, key: &str, op: &str, call: &str, usage: Usage) {
        let reference = icon::meter_ref(key, op, call);
        if let Err(e) = self.outbox(&reference, usage, js::now_ms(), None) {
            console_error!("{}", json!({ "event": "ledger.unknown-usage", "ref": reference, "why": e.message }));
        }
    }

    /// `POST /api/f/{name}/icon/make` (editors and up), `{hint?}`: a
    /// subject and up to `icon::CANDIDATES` icons of it, best first, each
    /// a `SIZE` PNG as base64, and how many drawings each flaw cost.
    pub(crate) async fn icon_make(&self, caller: &Caller, body: Value) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        let hint = icon::hint(&body).map_err(refused)?;
        self.icon_taker()?;
        let owner = self.must(MetaKey::Owner)?;
        // asked as a job's AI step is: a guest's fragment makes none, nor
        // one at its cap for anyone but its owner and their agents
        let by_owner = spends_as_owner(caller, &owner);
        let fragment = if by_owner { None } else { Some(self.name()?) };
        crate::ledger::may(&self.env, &owner, &MaySpend { spend: Spend::AiStep, fragment, by_owner }).await?;
        // admitted against the hour's starts as they are once the ledger
        // has answered: read, counted and kept with no wait between, so
        // makings asked at once each count the others'
        let now = js::now_ms();
        let started: Vec<i64> = self.meta(MetaKey::IconMakings)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let within = icon::admit(&started, now).map_err(refused)?;
        self.set_meta(MetaKey::IconMakings, &serde_json::to_string(&within).expect("starts serialize"))?;
        let (key, op) = (self.meter_key()?, operation(now));
        let subject = self.icon_subject(&owner, &key, &op, hint.as_deref()).await?;
        let seeds: Vec<u32> = (0..icon::CANDIDATES).map(|_| u32::from_le_bytes(js::random_bytes::<4>())).collect();
        let drawn = futures_util::future::join_all(seeds.iter().enumerate().map(|(n, seed)| self.icon_candidate(&owner, &key, &op, n, &subject, *seed))).await;
        let (mut candidates, mut flaws) = (Vec::new(), serde_json::Map::new());
        for d in drawn {
            match d {
                Ok(c) => candidates.push(c),
                Err(flaw) => {
                    let n = flaws.get(flaw.name()).and_then(Value::as_u64).unwrap_or(0);
                    flaws.insert(flaw.name().to_string(), json!(n + 1));
                }
            }
        }
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.event("icon.made", &format!("{} icon(s) of {subject:?}", candidates.len()), json!({ "subject": subject, "offered": candidates.len(), "flaws": flaws }));
        let b64 = base64::engine::general_purpose::STANDARD;
        let candidates: Vec<Value> = candidates.into_iter().map(|(png, score)| json!({ "png": b64.encode(png), "score": score })).collect();
        Ok(Response::from_json(&json!({ "subject": subject, "candidates": candidates, "flaws": flaws }))?)
    }

    /// The making's subject: GLM's, or, when it named none, the hint or
    /// the title (the checks then judge what was drawn of it).
    async fn icon_subject(&self, owner: &str, key: &str, op: &str, hint: Option<&str>) -> CellResult<String> {
        let meta: Value = self.meta(MetaKey::MetaLive)?.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null);
        let name = self.name()?;
        let label = fragment_proto::split_fragment_name(&name).map_or(name.as_str(), |(label, _)| label).to_string();
        let title = meta["title"].as_str().map(str::to_string).or(self.shown_as()?.1).unwrap_or(label);
        let input = icon::subject_input(&title, meta["description"].as_str(), hint);
        let named = match crate::models::run(&self.env, icon::SUBJECT_MODEL, &input, owner, None).await {
            Ok((200, bytes, _)) => {
                let answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if let Some(usage) = fragment_core::models::usage_of(icon::SUBJECT_MODEL, &answer["usage"]) {
                    self.icon_charge(key, op, "subject", usage);
                }
                icon::subject_of(&answer)
            }
            Ok((status, bytes, _)) => {
                console_error!("{}", json!({ "event": "icon.subject-failed", "status": status, "why": String::from_utf8_lossy(&bytes[..bytes.len().min(300)]) }));
                None
            }
            Err(e) => {
                console_error!("{}", json!({ "event": "icon.subject-failed", "why": e.message }));
                None
            }
        };
        Ok(named.or(hint.map(str::to_string)).unwrap_or(title))
    }

    /// One candidate (`n` of the making): drawn, cut out, examined, fitted,
    /// flattened and checked; its PNG and score, or its flaw.
    async fn icon_candidate(&self, owner: &str, key: &str, op: &str, n: usize, subject: &str, seed: u32) -> Result<(Vec<u8>, f64), Flaw> {
        let failed = |step: &str, why: String| {
            console_error!("{}", json!({ "event": "icon.step-failed", "step": step, "n": n, "why": why }));
            Flaw::Failed
        };
        let (status, bytes, neurons) = crate::models::run_form(&self.env, icon::IMAGE_MODEL, &icon::image_input(subject, seed), owner).await.map_err(|e| failed("draw", e.message))?;
        if status != 200 {
            return Err(failed("draw", format!("{status}: {}", String::from_utf8_lossy(&bytes[..bytes.len().min(300)]))));
        }
        self.icon_charge(key, op, &format!("draw-{n}"), icon::neurons(neurons.as_deref()));
        let answer: Value = serde_json::from_slice(&bytes).map_err(|e| failed("draw", format!("not JSON: {e}")))?;
        drop(bytes);
        let drawing = answer["image"].as_str().and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).ok_or_else(|| failed("draw", "no image in its answer".into()))?;
        let images = |steps: Vec<Value>, format: &'static str, call: String, from: Vec<u8>| async move {
            let out = js::images(self.env.as_ref(), &from, &steps, format).await.map_err(|e| failed(&call, e.message))?;
            self.icon_charge(key, op, &call, Usage::Images { count: 1 });
            Ok::<Vec<u8>, Flaw>(out)
        };
        let cut = images(vec![json!({ "segment": "foreground" })], "image/png", format!("cut-{n}"), drawing).await?;
        let (width, height, alpha) = icon::alpha_of(&cut)?;
        let shape = icon::examine(width, height, &alpha)?;
        drop(alpha);
        let png = images(icon::fit(Some(icon::crop(&shape))), "image/png", format!("fit-{n}"), cut).await?;
        if png.len() > icon::ICON_MAX_BYTES {
            return Err(failed("fit", format!("{} bytes", png.len())));
        }
        let flat = images(vec![json!({ "background": icon::BACKDROP })], "image/jpeg", format!("flat-{n}"), png.clone()).await?;
        let url = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(&flat));
        let call = decide::bound(&icon::check(subject, &url), decide::Asker::Agent).expect("an icon's check is a decision (icon's tests)");
        let input = call.input(vec![]);
        drop((flat, url));
        let (status, bytes, _) = crate::models::run(&self.env, call.model.id(), &input, owner, None).await.map_err(|e| failed("check", e.message))?;
        if status != 200 {
            return Err(failed("check", format!("{status}: {}", String::from_utf8_lossy(&bytes[..bytes.len().min(300)]))));
        }
        let response: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if let Some(usage) = call.usage(&response) {
            self.icon_charge(key, op, &format!("check-{n}"), usage);
        }
        let answer = call.answer(&response).map_err(|e| failed("check", e.message().to_string()))?;
        Ok((png, icon::judged(&answer)?))
    }

    /// `PUT /api/f/{name}/icon` (editors and up): the image its body is
    /// becomes the icon. A `SIZE` PNG within `ICON_MAX_BYTES` (a making's
    /// candidate) is kept as it is; any other image is fitted inside
    /// `SIZE` by Images, its padding transparent. A write: refused while
    /// the owner's fragments are read-only, before anything is fitted.
    pub(crate) async fn icon_put(&self, caller: &Caller, req: &mut Request) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        self.icon_taker()?;
        self.writable().await?;
        let bytes = crate::read_body(req, icon::UPLOAD_MAX_BYTES).await?;
        face::picture_type(&bytes).ok_or_else(|| refused(Refusal::NotAnImage))?;
        let whole = bytes.len() <= icon::ICON_MAX_BYTES && matches!(icon::alpha_of(&bytes), Ok((w, h, _)) if w == icon::SIZE && h == icon::SIZE);
        let png = if whole {
            bytes
        } else {
            let fitted = js::images(self.env.as_ref(), &bytes, &icon::fit(None), icon::MEDIA_TYPE).await.map_err(|e| CellError::invalid(format!("the image could not be read: {}", e.message)))?;
            self.icon_charge(&self.meter_key()?, &operation(js::now_ms()), "upload", Usage::Images { count: 1 });
            fitted
        };
        if png.len() > icon::ICON_MAX_BYTES {
            return Err(CellError::too_large("an icon, fitted,", png.len(), icon::ICON_MAX_BYTES));
        }
        let (sha, size) = (blob::sha256_hex(&png), png.len());
        self.put_blob_typed(&sha, png, Some(icon::MEDIA_TYPE)).await?;
        self.set_meta(MetaKey::Icon, &sha)?;
        self.show_icon().await?;
        self.event("icon.set", &format!("a new icon ({size} bytes)"), json!({ "sha256": sha, "fitted": !whole }));
        let path = icon::icon_path(&self.name()?, &sha);
        Ok(Response::from_json(&json!({ "icon": path, "sha256": sha, "size": size }))?)
    }

    /// `DELETE /api/f/{name}/icon` (editors and up): no icon; its lists
    /// show the shell's own again. Its blob goes with the collection. A
    /// write: refused while the owner's fragments are read-only.
    pub(crate) async fn icon_delete(&self, caller: &Caller) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        self.writable().await?;
        if self.icon_sha()?.is_some() {
            self.del_meta(MetaKey::Icon)?;
            self.show_icon().await?;
            self.event("icon.removed", "the icon was removed", json!({}));
        }
        Ok(Response::from_json(&json!({ "icon": null }))?)
    }

    /// `favicon.ico` on the fragment's own origin, which a browser asks of
    /// a page that names no icon (serve.rs, once its live has no such
    /// file), to whoever may open its pages: the icon's PNG (browsers read
    /// one there), revalidated each time so a new icon shows; `None` when it
    /// has none.
    pub(crate) async fn favicon(&self, req: &Request, public: bool) -> CellResult<Option<Response>> {
        let Some(sha) = self.icon_sha()? else { return Ok(None) };
        let cache = if public { "public, no-cache" } else { "private, no-cache" };
        let etag = format!("\"{sha}\"");
        if let Some(resp) = crate::serve::not_modified(req, &etag, cache)? {
            return Ok(Some(resp));
        }
        let mut resp = self.stream_blob(&sha, icon::MEDIA_TYPE, None).await?;
        let h = resp.headers_mut();
        h.set("cache-control", cache)?;
        h.set("x-content-type-options", "nosniff")?;
        Ok(Some(resp))
    }

    /// `GET /api/f/{name}/icon` (viewers and up): the icon's PNG, tagged by
    /// its SHA-256, or 404 when it has none.
    pub(crate) async fn icon_get(&self, caller: &Caller, req: &Request) -> CellResult<Response> {
        self.require(caller, false, Role::Viewer)?;
        let Some(sha) = self.icon_sha()? else {
            return Err(CellError::new(ErrorCode::NotFound, "no icon: make or upload one"));
        };
        let etag = format!("\"{sha}\"");
        if let Some(resp) = crate::serve::not_modified(req, &etag, ICON_CACHE)? {
            return Ok(resp);
        }
        let mut resp = self.stream_blob(&sha, icon::MEDIA_TYPE, None).await?;
        let h = resp.headers_mut();
        h.set("cache-control", ICON_CACHE)?;
        h.set("x-content-type-options", "nosniff")?;
        Ok(resp)
    }
}
