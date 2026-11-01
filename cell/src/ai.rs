//! AI steps (a job's `ai.*`; docs/api.md, AI). Text goes through the
//! platform's model route by tier (models.rs), streamed and read to its
//! `[DONE]`: OpenAI's tools pass through, and its answer is one message
//! (its text, its tool calls, why it stopped; never its reasoning) and its
//! timing. With a draft, its text so far is that app channel's draft as it
//! streams, from the run's principal, as `PUT …/draft` puts one. Typed
//! decisions are the Clef family on Workers AI (fragment_core::decide),
//! their media the fragment's own blobs or small and inline, priced in
//! input tokens only. An image is FLUX.1
//! [schnell] on Workers AI, on the model route's transport (the AI binding
//! through the deployment's gateway), priced in neurons by its tiles and
//! steps (fragment_core::media). A generated image is a file written to
//! `main` (a blob when 1 MiB or more), so it syncs to folders like anything
//! else. A video is HappyHorse 1.0 from Cloudflare's model catalog, on the
//! same transport (a third party's, which the gateway pays for by Unified
//! Billing): its answer names the video's URL, which the step reads and
//! writes to `main` as an image is, on any deployment, Private's too (the
//! catalog keeps no data; Paul, 2026-10-31: as its images are). Its call
//! waits while the model makes it (there is no request to poll), and is
//! asked at most once an attempt (`making`): a try that never heard its
//! answer fails for good, and only a replay asks again.
//!
//! Who pays (docs/cloudflare-v1.md, decision 26): every paid step bills the
//! fragment's owner, on their ledger, capped when the run's principal is
//! not that owner nor an agent of theirs (a run does not record whom an
//! agent asked for, so an agent of the owner's counts as the owner).
//!
//! A paid step asks the gate first (ledger.rs `may`: the owner's standing,
//! and the fragment's cap when it is capped, as this isolate heard them
//! within the minute; a ledger that does not answer refuses nothing:
//! decision 62), makes its call, and keeps what it bought beside the step
//! (`paid`, by the step's key `<f>@<life>/run/<run>/step/<i>`, which a
//! retry and a replay's attempt find too) together with its charge, in one
//! transaction: one row in the fragment's meter outbox, `step:<key>`, which
//! the ledger charges once by that reference (meter.rs). A receipt the
//! release before kept is never charged again (`reused`). A step tried again after its
//! call answered reuses what was kept and never calls the vendor again
//! (bug 2): a commit that did not finish runs from the kept bytes. Nothing
//! is held before a call, so a step that fails for good gives nothing back
//! (bug 3 is gone with the holds). A call that reported no usage is
//! charged nothing, said aloud (`ledger.unknown-usage`).

use fragment_core::ledger::Spend;
use fragment_core::decide;
use fragment_core::media;
use fragment_core::models::{self as bounds, Answer, Bounded, Stream};
use fragment_core::price::Usage;
use fragment_core::steps::{AiText, AiVideo, Step};
use fragment_core::{blob, npub};
use fragment_proto::limits;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::files::FileWrite;
use crate::fragment::{FragmentCell, MetaKey};
use crate::jobs::{permanent, RunRow, StepFail};
use crate::ledger::{self, LedgerError, MaySpend};
use crate::ops::JOB_ID_PREFIX;

/// A text step's drafts go at most this often: 4 a second.
const DRAFT_EVERY_MS: i64 = 250;
/// While its model has no words yet, its draft says it is thinking at
/// most this often (a page counts the seconds on its own between them).
const THINKING_EVERY_MS: i64 = 1000;

/// A model's refusal: passing (429, 5xx) or lasting.
fn model_failure(status: u16, body: &[u8]) -> StepFail {
    let message = String::from_utf8_lossy(body).chars().take(500).collect::<String>();
    match status {
        429 | 500..=599 => StepFail::Retry(format!("the model answered {status}: {message}")),
        _ => permanent(format!("the model answered {status}: {message}")),
    }
}

/// The gate's refusal as a step's failure: for good (no credit, a cap, a
/// guest: the run is held, and replays after a top-up or next month). The
/// gate answers nothing else: a ledger that does not answer refuses
/// nothing.
fn ledger_fail(e: LedgerError) -> StepFail {
    permanent(e.message)
}

fn retry(e: CellError) -> StepFail {
    StepFail::Retry(e.message)
}

/// A paid step's place on its payer's ledger.
struct Paying {
    /// The fragment's owner, who pays.
    owner: String,
    fragment: String,
    /// Its charge's reference, `step:<key>`: one charge whatever attempt
    /// makes the call.
    reference: String,
    /// What the step bought is kept by this, the same in every attempt.
    key: String,
    run: i64,
    /// The run's principal, when it is an agent (for the record).
    agent: Option<String>,
    /// The fragment's cap applies (decision 26).
    capped: bool,
}

/// What a paid step bought, kept beside it, and whether it was charged
/// as it was kept (every receipt of this release's).
struct Kept {
    result: Value,
    settled: bool,
}

#[derive(Deserialize)]
struct KeptRow {
    result: String,
    settled: i64,
}

impl FragmentCell {
    /// This step's key: unique to the fragment's life, the run, and the
    /// step's place in it (the same on a retry and a replay).
    pub(crate) fn step_ref(&self, run: &RunRow, index: u32) -> Result<String, StepFail> {
        Ok(format!("{}@{}/run/{}/step/{index}", self.name().map_err(retry)?, self.must(MetaKey::CreatedAt).map_err(retry)?, run.id))
    }

    /// Who pays for step `index` of `run`, and where on their ledger.
    fn paying(&self, run: &RunRow, index: u32) -> Result<Paying, StepFail> {
        let [owner, created] = self.metas([MetaKey::Owner, MetaKey::CreatedAt]).map_err(retry)?;
        let (owner, created) = (owner.ok_or_else(|| retry(crate::fragment::missing(MetaKey::Owner)))?, created.ok_or_else(|| retry(crate::fragment::missing(MetaKey::CreatedAt)))?);
        let fragment = self.name().map_err(retry)?;
        #[derive(Deserialize)]
        struct Agent {
            owner: Option<String>,
        }
        let agents: Vec<Agent> =
            self.typed("SELECT owner FROM members WHERE principal = ? AND kind = 'agent'", vec![run.principal.as_str().into()]).map_err(retry)?;
        let by_owner = run.principal == owner || agents.first().is_some_and(|a| a.owner.as_deref() == Some(owner.as_str()));
        let key = self.step_ref(run, index)?;
        debug_assert!(key.starts_with(&format!("{fragment}@{created}/")), "a step's key is its fragment's life's");
        Ok(Paying {
            reference: format!("step:{key}"),
            key,
            owner,
            fragment,
            run: run.id,
            agent: (!agents.is_empty() && npub::is_identity(&run.principal)).then(|| run.principal.clone()),
            capped: !by_owner,
        })
    }

    fn kept(&self, key: &str) -> Result<Option<Kept>, StepFail> {
        let rows: Vec<KeptRow> = self.typed("SELECT result, settled FROM paid WHERE key = ?", vec![key.into()]).map_err(retry)?;
        let Some(row) = rows.into_iter().next() else { return Ok(None) };
        let corrupt = |e: serde_json::Error| retry(CellError::host(format!("a kept step result: {e}")));
        let result = serde_json::from_str(&row.result).map_err(corrupt)?;
        Ok(Some(Kept { result, settled: row.settled != 0 }))
    }

    /// Keeps what a paid call bought and its charge in one transaction: the
    /// receipt (`paid`, by the step's key, `settled` 1: charged here), its
    /// usage as a row in the fragment's meter outbox (`step:<key>`, which
    /// the ledger charges once whatever attempt makes the call), and its
    /// price on the run (`charges`, priced with the configured book, as
    /// the ledger prices it). So a receipt of this release's is its
    /// charge: a step tried again finds it, never calls again, and charges
    /// nothing more. One that reported no usage is kept and charged
    /// nothing, said aloud.
    async fn keep(&self, p: &Paying, result: &Value, usage: Option<Usage>, log_id: Option<&str>) -> Result<(), StepFail> {
        let now = crate::js::now_ms();
        let text = usage.as_ref().map(|u| serde_json::to_string(u).expect("a usage serializes"));
        let charge = match usage {
            Some(usage) => {
                let micros = self.cfg.book.price(&usage).map_or(0, |priced| priced.charge);
                Some((self.outbox_row(&p.reference, usage, now, p.agent.clone()).map_err(retry)?, micros))
            }
            None => {
                console_error!("{}", json!({ "event": "ledger.unknown-usage", "ref": p.reference, "logId": log_id, "why": "the step's call reported no usage: charged nothing" }));
                None
            }
        };
        let (sql, key, reference, run, result) = (self.sql(), p.key.clone(), p.reference.clone(), p.run, result.to_string());
        let charged = charge.is_some();
        crate::js::transaction_sync(&self.raw, move || {
            sql.exec(
                "INSERT INTO paid (key, run, result, usage, settled, at) VALUES (?, ?, ?, ?, 1, ?) ON CONFLICT (key) DO NOTHING",
                vec![key.as_str().into(), SqlStorageValue::Integer(run), result.into(), text.map_or(SqlStorageValue::Null, SqlStorageValue::from), SqlStorageValue::Integer(now)],
            )?;
            if let Some((row, micros)) = charge {
                sql.exec(crate::meter::OUTBOX_INSERT, vec![reference.as_str().into(), row.into()])?;
                sql.exec(
                    "INSERT INTO charges (ref, run, micros, held, at) VALUES (?, ?, ?, 0, ?) ON CONFLICT (ref) DO NOTHING",
                    vec![reference.as_str().into(), SqlStorageValue::Integer(run), SqlStorageValue::Integer(micros), SqlStorageValue::Integer(now)],
                )?;
            }
            Ok(())
        })
        .map_err(retry)?;
        // the outbox goes at the alarm's next pass
        if charged {
            if let Err(e) = self.schedule().await {
                console_error!("{}", json!({ "event": "meter.arm-failed", "message": e.message }));
            }
        }
        Ok(())
    }

    /// A receipt reused. One of this release's was charged as it was kept
    /// (`keep`). One the release before kept (`settled` 0) is never charged
    /// here: that release reserved its call under an attempt's reference,
    /// which it settled (perhaps with its answer lost before it marked the
    /// receipt), its sweep charges, or the next release forgives; a charge
    /// under the step's key now could charge the call twice.
    fn reused(&self, p: &Paying, kept: &Kept) {
        if !kept.settled {
            console_log!("{}", json!({ "event": "ledger.receipt-before", "ref": p.reference, "why": "kept by the release before, which reserved its call: not charged again" }));
        }
    }

    /// Asks the gate whether the owner may pay for this step (ledger.rs
    /// `may`): their standing, and the fragment's cap when it is capped.
    async fn gate(&self, p: &Paying) -> Result<(), StepFail> {
        let may = MaySpend { spend: Spend::AiStep, fragment: p.capped.then(|| p.fragment.clone()), by_owner: !p.capped };
        ledger::may(&self.env, &p.owner, &may).await.map_err(ledger_fail)
    }

    /// Writes generated bytes to `path` on `main` once per step: in git, or
    /// as a blob and its pointer when 1 MiB or more. The platform's own
    /// commit, past an app's write limit (256 KiB), so the two limits meet
    /// and an image between them is kept (bug 1).
    async fn store_media(&self, run: &RunRow, index: u32, path: &str, bytes: Vec<u8>) -> Result<Value, StepFail> {
        // refused before any blob is stored for it
        let target = fragment_proto::RepoPath::parse(path).map_err(|not| permanent(not.to_string()))?;
        let sha = blob::sha256_hex(&bytes);
        let size = bytes.len() as u64;
        let content = if bytes.len() >= blob::BLOB_MIN_BYTES {
            self.put_blob_bytes(&sha, bytes).await.map_err(retry)?;
            blob::pointer(&sha, size).into_bytes()
        } else {
            bytes
        };
        assert!(content.len() < blob::BLOB_MIN_BYTES, "what goes to git is under a blob's size");
        let key = format!("{JOB_ID_PREFIX}{}:{index}", run.id);
        let message = format!("{} run {}: generated {path}", run.op, run.id);
        let writes = [FileWrite { path: target, bytes: Some(content) }];
        self.commit(&key, &writes, &Default::default(), &message, &run.principal, run.depth).await.map_err(retry)?;
        Ok(json!({ "path": path, "size": size, "sha256": sha }))
    }

    /// A kept image's bytes, from the blob store where it was put before
    /// it was kept.
    async fn kept_bytes(&self, sha: &str) -> Result<Vec<u8>, StepFail> {
        let found = self.blob(sha).await?.ok_or_else(|| permanent(format!("the image this step bought ({sha}) is gone from the blob store: replay the run to make it again")))?;
        read_blob(found).await
    }

    /// `job.ai.*` steps (the module's doc); `tried`: the Workflow's try of
    /// the step, from 1, when it says.
    pub(crate) async fn step_ai(&self, run: &RunRow, index: u32, step: &Step, tried: Option<u32>) -> Result<Value, StepFail> {
        match step {
            Step::AiText(t) => self.step_text(run, index, t, tried).await,
            Step::AiDecide(d) => self.step_decide(run, index, d).await,
            Step::AiImage(image) => {
                let call = media::image_call(image).map_err(|why| permanent(why.message()))?;
                let p = self.paying(run, index)?;
                if let Some(kept) = self.kept(&p.key)? {
                    self.reused(&p, &kept);
                    let sha = kept.result["sha256"].as_str().ok_or_else(|| retry(CellError::host("a kept image names its bytes")))?.to_string();
                    let mut out = self.store_media(run, index, &image.path, self.kept_bytes(&sha).await?).await?;
                    out["mediaType"] = kept.result["mediaType"].clone();
                    return Ok(out);
                }
                self.gate(&p).await?;
                let (status, bytes, log_id) = crate::models::run(&self.env, media::IMAGE_MODEL, &call.input, &p.owner, p.agent.as_deref()).await.map_err(retry)?;
                if status != 200 {
                    return Err(model_failure(status, &bytes));
                }
                let drawn = match media::image_of(&bytes) {
                    Ok(drawn) => drawn,
                    Err(fault) => {
                        // it answered, and what it drew is unknown: charged nothing, said aloud
                        self.event("ai.image-refused", &format!("{}: {}; charged nothing", p.reference, fault.message()), json!({ "ref": p.reference, "logId": log_id }));
                        console_error!("{}", json!({ "event": "ledger.unknown-usage", "ref": p.reference, "logId": log_id, "why": "the image's answer was no image: charged nothing" }));
                        return Err(permanent(fault.message()));
                    }
                };
                let usage = call.usage(drawn.width, drawn.height);
                let sha = blob::sha256_hex(&drawn.bytes);
                // the bytes first, then the note of them: a step tried again finds both
                self.put_blob_bytes(&sha, drawn.bytes.clone()).await.map_err(retry)?;
                let kept = json!({ "sha256": sha, "size": drawn.bytes.len(), "mediaType": media::IMAGE_MEDIA_TYPE, "width": drawn.width, "height": drawn.height });
                self.keep(&p, &kept, Some(usage), log_id.as_deref()).await?;
                // test fleets: a failure after the paid call (bug 2's), which the retry survives
                self.test_countdown(MetaKey::TestFailAfterPaid, "the step failed after its paid call").map_err(retry)?;
                let mut out = self.store_media(run, index, &image.path, drawn.bytes).await?;
                out["mediaType"] = kept["mediaType"].clone();
                Ok(out)
            }
            Step::AiVideo(video) => self.step_video(run, index, video).await,
            other => unreachable!("only AI steps are performed here, not {}", other.kind()),
        }
    }

    /// `job.ai.video`: checked (a deployment whose tiers are private makes
    /// none), gated, called, and kept with its charge as soon as the model
    /// answers with its video's URL: its seconds at its resolution, as the
    /// catalog bills it. Then the video is read from that URL and written
    /// as an image is. A step tried again after its call answered reads
    /// the URL it kept and never makes the video again, nor charges it
    /// again; an answer that is no completed video is charged nothing,
    /// said aloud.
    async fn step_video(&self, run: &RunRow, index: u32, video: &AiVideo) -> Result<Value, StepFail> {
        let call = media::video_call(video).map_err(|why| permanent(why.message()))?;
        let cfg = crate::config::Config::from_env(&self.env);
        let p = self.paying(run, index)?;
        let url = match self.kept(&p.key)? {
            Some(kept) => {
                self.reused(&p, &kept);
                kept.result["url"].as_str().ok_or_else(|| retry(CellError::host("a kept video names its URL")))?.to_string()
            }
            None => {
                // asked at most once an attempt: a try that finds its own
                // attempt's call unanswered never heard the answer (a
                // restart, a broken try), which may still be made and billed
                if media::lost(self.making(&p.key)?, run.attempt) {
                    self.event("ai.video-lost", &format!("{}: its answer never came back; charged nothing", p.reference), json!({ "ref": p.reference }));
                    console_error!("{}", json!({ "event": "ledger.unknown-usage", "ref": p.reference, "why": "the video's answer never came back, and the catalog may have billed it: charged nothing" }));
                    return Err(permanent("this step's video was asked for and its answer never came back: replay the run to make it again"));
                }
                self.gate(&p).await?;
                self.asking(&p, run).map_err(retry)?;
                let answered = crate::models::run(&self.env, media::VIDEO_MODEL, &call.input, &p.owner, p.agent.as_deref()).await;
                // test fleets: the answer lost before this try hears it, as a restart's is
                self.test_countdown(MetaKey::TestLoseVideoAnswers, "the video's answer was lost").map_err(retry)?;
                // no answer: it may still be making the video, so the row stays
                let (status, bytes, log_id) = answered.map_err(retry)?;
                // answered (a refusal, a 503 too): a try may ask again
                self.answered(&p.key, run.attempt).map_err(retry)?;
                if status != 200 {
                    return Err(model_failure(status, &bytes));
                }
                let url = match media::video_url(&bytes) {
                    Ok(url) => url,
                    Err(fault) => {
                        // it answered with no video: what it made is unknown, charged nothing, said aloud
                        self.event("ai.video-refused", &format!("{}: {}; charged nothing", p.reference, fault.message()), json!({ "ref": p.reference, "logId": log_id }));
                        console_error!("{}", json!({ "event": "ledger.unknown-usage", "ref": p.reference, "logId": log_id, "why": "the video's answer was no completed video: charged nothing" }));
                        return Err(permanent(fault.message()));
                    }
                };
                let kept = json!({ "url": url, "seconds": call.seconds, "resolution": call.resolution.input() });
                self.keep(&p, &kept, Some(call.usage()), log_id.as_deref()).await?;
                // test fleets: a failure after the paid call (bug 2's), which the retry survives
                self.test_countdown(MetaKey::TestFailAfterPaid, "the step failed after its paid call").map_err(retry)?;
                url
            }
        };
        let made = media::video_of(self.read_video(&url, cfg.ai_url.is_some()).await?).map_err(|fault| permanent(fault.message()))?;
        let length_ms = made.length_ms;
        let mut out = self.store_media(run, index, &video.path, made.bytes).await?;
        out["mediaType"] = json!(media::VIDEO_MEDIA_TYPE);
        out["lengthMs"] = json!(length_ms);
        Ok(out)
    }

    /// The attempt whose call for this step's video waits on its model,
    /// if one does (`making`).
    fn making(&self, key: &str) -> Result<Option<u32>, StepFail> {
        #[derive(Deserialize)]
        struct Making {
            attempt: u32,
        }
        let rows: Vec<Making> = self.typed(media::MAKING, vec![key.into()]).map_err(retry)?;
        Ok(rows.into_iter().next().map(|m| m.attempt))
    }

    /// This attempt asks the model for the step's video: a replay's takes
    /// the row from the attempt before.
    fn asking(&self, p: &Paying, run: &RunRow) -> CellResult<()> {
        self.exec(
            media::ASKING,
            vec![p.key.as_str().into(), SqlStorageValue::Integer(run.id), SqlStorageValue::Integer(run.attempt.into()), SqlStorageValue::Integer(crate::js::now_ms())],
        )
    }

    /// The model answered `attempt`'s call for the step's video: its row
    /// goes, and a newer attempt's stays (`media::ANSWERED`).
    fn answered(&self, key: &str, attempt: u32) -> CellResult<()> {
        self.exec(media::ANSWERED, vec![key.into(), SqlStorageValue::Integer(attempt.into())])
    }

    /// The video at the URL the model answered, at most
    /// `media::VIDEO_MAX_BYTES` (read no further): https alone, or http to
    /// the dev stack's and the e2e's fake (`plain`). Gone (404, 410) is for
    /// good; a refusal that passes (429, 5xx) or a read that breaks is the
    /// step's to try again.
    async fn read_video(&self, url: &str, plain: bool) -> Result<Vec<u8>, StepFail> {
        if !(url.starts_with("https://") || (plain && url.starts_with("http://"))) {
            return Err(permanent(media::AnswerFault::NoVideo.message()));
        }
        let mut resp = Fetch::Url(Url::parse(url).map_err(|_| permanent(media::AnswerFault::NoVideo.message()))?).send().await.map_err(|e| StepFail::Retry(format!("the video's URL did not answer: {e}")))?;
        match resp.status_code() {
            200 => {}
            404 | 410 => return Err(permanent("the video this step made is gone from its URL (it is kept for a while): replay the run to make it again")),
            status @ (429 | 500..=599) => return Err(StepFail::Retry(format!("the video's URL answered {status}"))),
            status => return Err(permanent(format!("the video's URL answered {status}"))),
        }
        match crate::cs::read_answer(&mut resp, media::VIDEO_MAX_BYTES).await.map_err(retry)? {
            (_, true) => Err(permanent(media::AnswerFault::VideoTooLarge.message())),
            (bytes, false) => Ok(bytes),
        }
    }

    /// `job.ai.decide`: checked, its blobs sized then read, gated, called,
    /// and its paid answer (a malformed one too, as a rejection) kept with
    /// its charge: a retry or a replay never buys it again, nor reads its
    /// media again.
    async fn step_decide(&self, run: &RunRow, index: u32, d: &Value) -> Result<Value, StepFail> {
        let p = self.paying(run, index)?;
        if let Some(kept) = self.kept(&p.key)? {
            self.reused(&p, &kept);
            return decision_result(kept.result);
        }
        let call = decide::bound(d, decide::Asker::Job).map_err(|e| permanent(e.message()))?;
        let blobs = self.decision_blobs(&call).await?;
        self.gate(&p).await?;
        let input = call.input(blobs);
        let answered = crate::models::run(&self.env, call.model.id(), &input, &p.owner, p.agent.as_deref()).await;
        drop(input);
        let (status, bytes, log_id) = answered.map_err(retry)?;
        if status != 200 {
            return Err(model_failure(status, &bytes));
        }
        let response: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let usage = call.usage(&response);
        if usage.is_none() {
            self.event("ai.cost-missing", &format!("{}: the decision model reported no valid input usage; charged nothing", p.reference), json!({ "ref": p.reference, "logId": log_id }));
        }
        let result = match call.answer(&response) {
            Ok(answer) => answer,
            Err(e) => {
                let message = e.message();
                self.event("ai.decide-refused", &format!("{}: {message}; its paid answer is kept", p.reference), json!({ "ref": p.reference, "logId": log_id }));
                json!({ "error": message })
            }
        };
        self.keep(&p, &result, usage, log_id.as_deref()).await?;
        self.test_countdown(MetaKey::TestFailAfterPaid, "the step failed after its paid call").map_err(retry)?;
        decision_result(result)
    }

    /// The bytes of the blobs a decision names, in its order: each one this
    /// fragment holds (its blobs are under its own key: another's hash is
    /// not found), each sized, with those before it, before its bytes are
    /// read, so media past its limits is refused unread.
    async fn decision_blobs(&self, call: &decide::DecisionCall) -> Result<Vec<Vec<u8>>, StepFail> {
        let shas = call.blobs();
        let (mut sizes, mut blobs) = (Vec::with_capacity(shas.len()), Vec::with_capacity(shas.len()));
        // bounded: at most decide::MEDIA_MAX items
        for sha in shas {
            let found = self.blob(sha).await?.ok_or_else(|| permanent(decide::Refusal::BlobMissing.message()))?;
            sizes.push(found.size);
            call.sized(&sizes).map_err(|e| permanent(e.message()))?;
            blobs.push(read_blob(found).await?);
        }
        Ok(blobs)
    }

    /// One of this fragment's blobs, its size known and its bytes unread,
    /// or `None` when it is gone.
    async fn blob(&self, sha: &str) -> Result<Option<crate::js::BlobBody>, StepFail> {
        let key = self.blob_key(sha).map_err(retry)?;
        crate::js::blob_get(&self.env, &key, None).await.map_err(retry)
    }

    /// `job.ai.text`: the model route's call, streamed (`read_text`), kept
    /// with its charge. Its answer is the model's text, its message (`{role,
    /// content, tool_calls?}`, never its reasoning), why it stopped, and its
    /// timing; a step tried again after it was kept answers what was kept.
    async fn step_text(&self, run: &RunRow, index: u32, t: &AiText, tried: Option<u32>) -> Result<Value, StepFail> {
        let p = self.paying(run, index)?;
        if let Some(kept) = self.kept(&p.key)? {
            self.reused(&p, &kept);
            return Ok(kept.result);
        }
        let tier = bounds::tier_named(t.model.as_deref()).map_err(|why| permanent(why.message()))?;
        let body = bounds::text_body(t).map_err(|why| permanent(why.message()))?;
        let drafting = bounds::draft_of(t).map(|d| self.drafting(run, d)).transpose()?;
        // where the deployment runs the tier (Workers AI, or an
        // OpenAI-compatible API: Finite Private on Finite's own), streamed
        let models = &crate::config::Config::from_env(&self.env).models;
        let bounded = models.capped(bounds::Named::Tier(tier)).and_then(|m| bounds::bound(m, body, true)).map_err(|why| permanent(why.message()))?;
        self.gate(&p).await?;
        let read = match self.read_text(&p, &bounded, drafting.as_ref()).await {
            Ok(read) => read,
            Err(failed) => {
                // its draft stops; a try again starts it afresh
                if let Some(d) = &drafting {
                    self.put_draft(d, None, None);
                }
                return Err(failed);
            }
        };
        let usage = bounds::usage_of(read.model, &read.used);
        if usage.is_none() {
            self.event("ai.cost-missing", &format!("{}: the model reported no usage; the step is charged nothing", p.reference), json!({ "ref": p.reference, "logId": read.log_id }));
        }
        // how long it took (docs/api.md, AI): kept with what it bought
        let timing = json!({ "first_ms": read.first_ms, "ms": read.ms, "model": read.model, "calls": read.calls, "tries": tried });
        let a = &read.answer;
        let result = json!({ "text": a.content, "message": a.message(), "finish_reason": a.finish_reason, "model": read.model, "tier": tier, "usage": read.used, "timing": timing });
        self.keep(&p, &result, usage, read.log_id.as_deref()).await?;
        self.test_countdown(MetaKey::TestFailAfterPaid, "the step failed after its paid call").map_err(retry)?;
        Ok(result)
    }

    /// A text step's draft: a channel the app declares (whoever may read
    /// it sees it), drafted as the run's principal, whose record with the
    /// same turn then replaces it on a page.
    fn drafting(&self, run: &RunRow, (channel, turn): (&str, &str)) -> Result<Drafting, StepFail> {
        if self.declared_channel(channel).map_err(retry)?.is_none() {
            return Err(permanent(format!("ai.text drafts to a channel fragment.json declares, and it declares no {channel:?}")));
        }
        Ok(Drafting { channel: channel.to_string(), turn: turn.to_string(), principal: npub::display(&run.principal) })
    }

    /// A text step's call (models.rs `streamed`: its tier's model, or the
    /// fallback's when that one failed before answering), its answer and
    /// usage read as they come, to its `[DONE]`, where the connection is
    /// dropped (the gateway can hold it open for minutes after). With a
    /// draft: thinking from the call's start, its text so far at most every
    /// `DRAFT_EVERY_MS` (none past a record's size), and its whole text once
    /// more at its end. A refusal is the step's failure; a stream that
    /// breaks, or ends before its answer says it is whole, is the step's to
    /// try again.
    async fn read_text(&self, p: &Paying, bounded: &Bounded<'static>, d: Option<&Drafting>) -> Result<Read, StepFail> {
        let t0 = crate::js::now_ms();
        if let Some(d) = d {
            self.put_draft(d, Some(""), Some(0));
        }
        let crate::models::Streamed { mut answer, log_id, model, upstream, calls } = crate::models::streamed(&self.env, bounded, &p.owner, p.agent.as_deref()).await.map_err(retry)?;
        let status = answer.status_code();
        if status != 200 {
            let body = crate::models::read_whole(&mut answer).await.unwrap_or_default();
            return Err(model_failure(status, &body));
        }
        let mut bytes = answer.stream().map_err(|e| retry(e.into()))?;
        let mut stream = Stream::answering().on(upstream);
        let mut first_ms = None;
        // the text drafted so far, and when the last draft of any kind went
        let (mut sent, mut drafted_at) = (0usize, t0);
        // bounded by the model's answer: at most the tier's max_tokens, to its [DONE]
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.map_err(|e| StepFail::Retry(format!("the model's stream broke: {e}")))?;
            stream.push(&chunk, None);
            let now = crate::js::now_ms();
            if first_ms.is_none() && stream.began() {
                first_ms = Some(now - t0);
            }
            if let Some(d) = d {
                let text = &stream.answer().expect("an answering stream keeps its answer").content;
                if text.len() > sent && now - drafted_at >= DRAFT_EVERY_MS {
                    if self.put_draft(d, Some(text), None) {
                        (sent, drafted_at) = (text.len(), now);
                    }
                } else if text.is_empty() && now - drafted_at >= THINKING_EVERY_MS {
                    // no words yet: still thinking, this long
                    drafted_at = now;
                    self.put_draft(d, Some(""), Some(u64::try_from(now - t0).unwrap_or(0)));
                }
            }
            if stream.done() {
                break;
            }
        }
        drop(bytes);
        stream.finish(None);
        let answer = stream.answer().cloned().expect("an answering stream keeps its answer");
        if !stream.done() && answer.finish_reason.is_none() {
            return Err(StepFail::Retry("the model's stream ended before its answer did".into()));
        }
        let ms = crate::js::now_ms() - t0;
        if let Some(d) = d.filter(|_| answer.content.len() > sent) {
            // its whole text once more, at the drafts' pace
            let wait = DRAFT_EVERY_MS - (crate::js::now_ms() - drafted_at);
            if wait > 0 {
                Delay::from(std::time::Duration::from_millis(wait.unsigned_abs())).await;
            }
            self.put_draft(d, Some(&answer.content), None);
        }
        let used = stream.usage().cloned().unwrap_or(Value::Null);
        Ok(Read { answer, used, log_id, first_ms, ms, calls, model })
    }

    /// One draft to the channel's live readers: whether it went (none past
    /// a record's size, or past the fragment's pace of drafts).
    fn put_draft(&self, d: &Drafting, text: Option<&str>, thinking: Option<u64>) -> bool {
        text.is_none_or(|t| t.len() <= limits::RECORD_BODY_MAX_BYTES) && self.broadcast_draft(&d.channel, &d.principal, &d.turn, text, thinking)
    }
}

/// A text step's call, read: its answer, the usage it reported, its
/// gateway log id, and its timing.
struct Read {
    answer: Answer,
    used: Value,
    log_id: Option<String>,
    /// When its first data line came, ms after the call began (one did:
    /// an answer whole has its `[DONE]` or its finish reason).
    first_ms: Option<i64>,
    /// The whole call, to its `[DONE]`.
    ms: i64,
    /// Its tries (a second when the first fell back).
    calls: u32,
    /// The model that answered: its tier's, or the fallback.
    model: &'static str,
}

/// Where a text step's drafts go, and who drafts them.
struct Drafting {
    channel: String,
    turn: String,
    principal: String,
}

/// A blob's bytes, read whole (its caller bounds it).
async fn read_blob(found: crate::js::BlobBody) -> Result<Vec<u8>, StepFail> {
    let mut resp = Response::from_body(ResponseBody::Stream(found.body)).map_err(|e| retry(e.into()))?;
    resp.bytes().await.map_err(|e| retry(e.into()))
}

fn decision_result(result: Value) -> Result<Value, StepFail> {
    match result["error"].as_str() {
        Some(message) => Err(permanent(message)),
        None => Ok(result),
    }
}
