//! A computer image prepared on Cloudflare's network before anything a
//! deploy changes: `deploy` waits for each image the config pins to be
//! ready to run, and refuses, having changed nothing, when one is not.
//!
//! Why ours, beside wrangler's own: wrangler prepares each named image as
//! `wrangler deploy` starts (`waitForImagePreparation` in 4.145.0: the
//! Containers API's `POST /image-preparations`, asked every 2 s), and gives
//! up after 15 minutes. On 2026-10-24 production's copy of round 11's image
//! (made by skopeo at 19:15) was still pending at 20:07, so its promotion
//! failed after 15 minutes, and its re-run, which found it ready, deployed
//! 50 minutes later; round 10's had taken 2 min 18 s, and round 11's own on
//! the preview's account 2 min 19 s. Asked here first, a deploy waits as
//! long as `DEPLOY_LIMIT` before its build, its DNS, its bucket and its
//! queues, saying how long it has waited, and wrangler's own ask then
//! answers at once. An image a deploy builds from the tree is pushed by
//! wrangler, which prepares it before it uploads the Worker: it is not
//! asked here.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const API: &str = "https://api.cloudflare.com/client/v4";

/// How long a deploy waits for its images, all of them: past it, it
/// refuses, having changed nothing. Shorter than its CI job's limit
/// (ci.yml, `deploy`'s 45 minutes) by the build, the deploy and its checks.
pub(crate) const DEPLOY_LIMIT: Duration = Duration::from_secs(20 * 60);
/// The pause between two asks about an image still pending.
const POLL: Duration = Duration::from_secs(5);
/// How often a wait says how long it has waited.
const SAY_EVERY: Duration = Duration::from_secs(60);
/// The most images one wait asks about: a config names a stable and a
/// canary.
pub(crate) const IMAGES_MAX: usize = 16;

/// What Cloudflare answers about an image's preparation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Preparation {
    /// Ready to run: a start pulls it without waiting for it.
    Ready,
    /// Still being prepared: ask again.
    Pending,
    /// It cannot be prepared (it is no image of the account's registry,
    /// or Cloudflare failed it): asking again will not change that.
    Failed(String),
}

/// One ask that got no answer about the image: worth asking again
/// (`Transient`: the API did not answer, or answered 429 or 5xx), or not
/// (`Refused`: the token may not ask, or the API refused the request).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AskError {
    Transient(String),
    Refused(String),
}

/// Asks Cloudflare about one image's preparation, starting it when none
/// has started.
pub(crate) trait Preparer {
    fn ask(&mut self, image: &str) -> Result<Preparation, AskError>;
}

/// The time a wait reads, and how it pauses.
pub(crate) trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&mut self, pause: Duration);
}

/// The wall clock.
pub(crate) struct Wall(Instant);

impl Wall {
    pub(crate) fn new() -> Wall {
        Wall(Instant::now())
    }
}

impl Clock for Wall {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&mut self, pause: Duration) {
        std::thread::sleep(pause);
    }
}

/// The Containers API of one account (`POST /accounts/{account}/containers/
/// image-preparations`, as wrangler asks it), with a token that may use it.
pub(crate) struct Api {
    http: reqwest::blocking::Client,
    account: String,
    token: String,
}

impl Api {
    /// The API for `account`, with wrangler's token (`CLOUDFLARE_API_TOKEN`,
    /// as a deploy's wrangler and the Image workflow's are given it).
    pub(crate) fn from_env(account: &str) -> Result<Api> {
        let token = std::env::var("CLOUDFLARE_API_TOKEN").ok().filter(|t| !t.trim().is_empty()).context(
            "CLOUDFLARE_API_TOKEN is not set: the image's preparation is asked of Cloudflare's API with wrangler's token (Containers on the account), before anything is built or changed",
        )?;
        let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build()?;
        Ok(Api { http, account: account.to_string(), token: token.trim().to_string() })
    }
}

impl Preparer for Api {
    fn ask(&mut self, image: &str) -> Result<Preparation, AskError> {
        let url = format!("{API}/accounts/{}/containers/image-preparations", self.account);
        let resp = self.http.post(url).bearer_auth(&self.token).json(&json!({ "image": image })).send().map_err(|e| AskError::Transient(format!("Cloudflare's API did not answer: {e}")))?;
        let status = resp.status().as_u16();
        let body: Value = resp.json().unwrap_or(Value::Null);
        answered(status, &body)
    }
}

/// An answer of `POST /image-preparations` (its status and JSON), read:
/// its `result.status` (`ready`, `pending`, `error` with a `reason`) on a
/// success; a 400 is an image the account cannot prepare (not in its
/// registry: `{"error":"image does not exist in this account"}`); 401 and
/// 403 a token that may not ask; 429 and 5xx (or no JSON) worth asking again.
pub(crate) fn answered(status: u16, body: &Value) -> Result<Preparation, AskError> {
    let errors = || {
        let listed: Vec<String> = body["errors"].as_array().into_iter().flatten().map(|e| e["message"].as_str().unwrap_or("?").to_string()).collect();
        if listed.is_empty() { format!("{status}") } else { format!("{status}: {}", listed.join("; ")) }
    };
    match status {
        200..=299 if body["success"] == true => match body["result"]["status"].as_str() {
            Some("ready") => Ok(Preparation::Ready),
            Some("pending") => Ok(Preparation::Pending),
            Some("error") => Ok(Preparation::Failed(body["result"]["reason"].as_str().unwrap_or("Cloudflare failed its preparation").to_string())),
            other => Err(AskError::Refused(format!("Cloudflare answered a preparation status this deploy does not know: {other:?}"))),
        },
        400 | 404 | 422 => Ok(Preparation::Failed(errors())),
        401 | 403 => Err(AskError::Refused(format!("the token may not ask (it needs Containers on the account): {}", errors()))),
        429 | 500..=599 => Err(AskError::Transient(errors())),
        _ => Err(AskError::Transient(errors())),
    }
}

/// Waits until every image in `images` is ready on Cloudflare's network,
/// for `limit` at most (all of them), asking about each in turn every
/// `POLL` while it is pending, and saying every `SAY_EVERY` how long it
/// has waited. Refused, naming the image and why, when one cannot be
/// prepared, the token may not ask, or the limit passes: its caller has
/// changed nothing yet, and says so.
pub(crate) fn until_ready(api: &mut impl Preparer, clock: &mut impl Clock, images: &[&str], limit: Duration) -> Result<()> {
    assert!(images.len() <= IMAGES_MAX, "at most {IMAGES_MAX} images, not {}", images.len());
    assert!(limit > Duration::ZERO, "a wait has a limit");
    let start = clock.now();
    let mut done: Vec<&str> = Vec::with_capacity(images.len());
    for &image in images {
        if done.contains(&image) {
            continue;
        }
        let mut said = clock.now();
        // bounded: each pass sleeps POLL, and the limit ends the loop
        loop {
            let waited = clock.now() - start;
            // why the API answered nothing about it, or `None`: pending
            let unanswered = match api.ask(image) {
                Ok(Preparation::Ready) => {
                    println!("image: {image} is ready on Cloudflare's network (asked for {})", minutes(waited));
                    break;
                }
                Ok(Preparation::Failed(why)) => bail!("image: Cloudflare cannot prepare {image}: {why}"),
                Ok(Preparation::Pending) => None,
                Err(AskError::Refused(why)) => bail!("image: could not ask Cloudflare about {image}: {why}"),
                Err(AskError::Transient(why)) => Some(why),
            };
            if waited >= limit {
                match unanswered {
                    Some(why) => bail!("image: Cloudflare's API answered nothing about {image} for {} (the limit), the last ask: {why}", minutes(waited)),
                    None => bail!("image: {image} is still being prepared on Cloudflare's network after {} (the limit)", minutes(waited)),
                }
            }
            if clock.now() - said >= SAY_EVERY {
                said = clock.now();
                match &unanswered {
                    Some(why) => println!("image: {image}: Cloudflare's API did not answer ({why}); asking again (waited {} of {})", minutes(waited), minutes(limit)),
                    None => println!("image: {image} is being prepared on Cloudflare's network (waited {} of {})", minutes(waited), minutes(limit)),
                }
            }
            clock.sleep(POLL);
        }
        done.push(image);
    }
    Ok(())
}

/// A wait's length as people read it: `2m05s`, `40s`.
fn minutes(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { format!("{s}s") } else { format!("{}m{:02}s", s / 60, s % 60) }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};

    /// A clock that moves only when it sleeps.
    #[derive(Default)]
    pub(crate) struct Fake(pub Duration);

    impl Clock for Fake {
        fn now(&self) -> Duration {
            self.0
        }
        fn sleep(&mut self, pause: Duration) {
            self.0 += pause;
        }
    }

    /// Cloudflare, scripted: each image's answers in turn, its last kept
    /// once the script runs out; and every ask made.
    #[derive(Default)]
    pub(crate) struct Scripted {
        pub answers: BTreeMap<String, VecDeque<Result<Preparation, AskError>>>,
        pub asked: Vec<String>,
    }

    impl Scripted {
        pub(crate) fn with(image: &str, answers: Vec<Result<Preparation, AskError>>) -> Scripted {
            let mut s = Scripted::default();
            s.answers.insert(image.to_string(), answers.into());
            s
        }
    }

    impl Preparer for Scripted {
        fn ask(&mut self, image: &str) -> Result<Preparation, AskError> {
            self.asked.push(image.to_string());
            let q = self.answers.get_mut(image).unwrap_or_else(|| panic!("no answers scripted for {image}"));
            if q.len() > 1 { q.pop_front().expect("one at least") } else { q.front().cloned().expect("one at least") }
        }
    }

    const STABLE: &str = "registry.cloudflare.com/0123456789abcdef0123456789abcdef/fragment-computer-hermes@sha256:5b8481f0bb8913e37832d0d6544a28eafdbbded347b38d7a3774fcf3444524b6";
    const CANARY: &str = "registry.cloudflare.com/0123456789abcdef0123456789abcdef/fragment-computer-hermes@sha256:0a022c96d88a451a3243f04eb53448e6f656cd4d218a76453ac4ebfdbca64f84";

    #[test]
    fn an_image_ready_at_once_is_asked_once() {
        let (mut api, mut clock) = (Scripted::with(STABLE, vec![Ok(Preparation::Ready)]), Fake::default());
        until_ready(&mut api, &mut clock, &[STABLE], DEPLOY_LIMIT).unwrap();
        assert_eq!((api.asked.len(), clock.0), (1, Duration::ZERO));
    }

    #[test]
    fn a_pending_image_is_asked_again_until_it_is_ready() {
        let mut api = Scripted::with(CANARY, vec![Ok(Preparation::Pending), Ok(Preparation::Pending), Ok(Preparation::Pending), Ok(Preparation::Ready)]);
        let mut clock = Fake::default();
        until_ready(&mut api, &mut clock, &[CANARY], DEPLOY_LIMIT).unwrap();
        assert_eq!((api.asked.len(), clock.0), (4, 3 * POLL));
    }

    #[test]
    fn an_image_still_pending_at_the_limit_is_refused_and_asked_no_more() {
        // 2026-10-24: round 11's copy, pending for wrangler's whole 15 minutes
        let mut api = Scripted::with(CANARY, vec![Ok(Preparation::Pending)]);
        let mut clock = Fake::default();
        let e = until_ready(&mut api, &mut clock, &[CANARY], DEPLOY_LIMIT).unwrap_err().to_string();
        assert!(e.contains("still being prepared") && e.contains(CANARY) && e.contains("20m00s"), "{e}");
        assert_eq!(clock.0, DEPLOY_LIMIT, "it waits the limit and no longer");
        assert_eq!(api.asked.len() as u128, DEPLOY_LIMIT.as_millis() / POLL.as_millis() + 1, "an ask each POLL, bounded");
    }

    #[test]
    fn an_image_cloudflare_cannot_prepare_is_refused_at_once() {
        for (failed, said) in [
            (Preparation::Failed("400: {\"error\":\"image does not exist in this account\"}".into()), "does not exist"),
            (Preparation::Failed("the image is not linux/amd64".into()), "linux/amd64"),
        ] {
            let mut api = Scripted::with(STABLE, vec![Ok(Preparation::Pending), Ok(failed)]);
            let e = until_ready(&mut api, &mut Fake::default(), &[STABLE], DEPLOY_LIMIT).unwrap_err().to_string();
            assert!(e.contains("cannot prepare") && e.contains(said), "{e}");
            assert_eq!(api.asked.len(), 2, "not asked again");
        }
    }

    #[test]
    fn an_api_that_does_not_answer_is_asked_again_and_one_that_refuses_is_not() {
        let mut api = Scripted::with(STABLE, vec![Err(AskError::Transient("503".into())), Err(AskError::Transient("timed out".into())), Ok(Preparation::Ready)]);
        until_ready(&mut api, &mut Fake::default(), &[STABLE], DEPLOY_LIMIT).unwrap();
        assert_eq!(api.asked.len(), 3);

        let mut api = Scripted::with(STABLE, vec![Err(AskError::Transient("503".into()))]);
        let e = until_ready(&mut api, &mut Fake::default(), &[STABLE], DEPLOY_LIMIT).unwrap_err().to_string();
        assert!(e.contains("answered nothing") && e.contains("503"), "{e}");

        let mut api = Scripted::with(STABLE, vec![Err(AskError::Refused("403: Authentication error".into())), Ok(Preparation::Ready)]);
        let e = until_ready(&mut api, &mut Fake::default(), &[STABLE], DEPLOY_LIMIT).unwrap_err().to_string();
        assert!(e.contains("could not ask") && e.contains("403"), "{e}");
        assert_eq!(api.asked.len(), 1);
    }

    #[test]
    fn every_image_is_ready_within_one_limit_and_each_is_asked_once_ready() {
        let mut api = Scripted::with(STABLE, vec![Ok(Preparation::Ready)]);
        api.answers.insert(CANARY.into(), vec![Ok(Preparation::Pending); 10].into_iter().chain([Ok(Preparation::Ready)]).collect());
        let mut clock = Fake::default();
        // a stable and a canary at one digest (between rounds) are one image
        until_ready(&mut api, &mut clock, &[STABLE, CANARY, STABLE], DEPLOY_LIMIT).unwrap();
        assert_eq!(api.asked.iter().filter(|i| *i == STABLE).count(), 1);
        assert_eq!(api.asked.iter().filter(|i| *i == CANARY).count(), 11);

        // the limit is the whole wait's: a second image pending past it is refused
        let mut api = Scripted::with(STABLE, vec![Ok(Preparation::Pending); 200].into_iter().chain([Ok(Preparation::Ready)]).collect());
        api.answers.insert(CANARY.into(), vec![Ok(Preparation::Pending)].into());
        let mut clock = Fake::default();
        let e = until_ready(&mut api, &mut clock, &[STABLE, CANARY], DEPLOY_LIMIT).unwrap_err().to_string();
        assert!(e.contains(CANARY), "{e}");
        assert_eq!(clock.0, DEPLOY_LIMIT);
    }

    #[test]
    fn cloudflare_s_answers_read_as_it_gives_them() {
        // as the dev account answered on 2026-10-24
        let ready = json!({"success": true, "result": {"image": STABLE, "status": "ready", "artifact_digest": "sha256:b2e8"}, "messages": [], "errors": []});
        assert_eq!(answered(200, &ready), Ok(Preparation::Ready));
        let pending = json!({"success": true, "result": {"image": STABLE, "status": "pending"}, "errors": []});
        assert_eq!(answered(202, &pending), Ok(Preparation::Pending));
        let error = json!({"success": true, "result": {"image": STABLE, "status": "error", "reason": "unsupported platform"}});
        assert_eq!(answered(200, &error), Ok(Preparation::Failed("unsupported platform".into())));
        let missing = json!({"success": false, "result": null, "errors": [{"code": 1000, "message": "{\"error\":\"image does not exist in this account\"}"}]});
        assert!(matches!(answered(400, &missing), Ok(Preparation::Failed(why)) if why.contains("does not exist in this account")));
        assert!(matches!(answered(403, &json!({"success": false, "errors": [{"code": 10000, "message": "Authentication error"}]})), Err(AskError::Refused(why)) if why.contains("Authentication error")));
        assert!(matches!(answered(503, &Value::Null), Err(AskError::Transient(_))));
        assert!(matches!(answered(429, &Value::Null), Err(AskError::Transient(_))));
        assert!(matches!(answered(200, &json!({"success": true, "result": {"status": "warming"}})), Err(AskError::Refused(_))), "a status it does not know is no ready");
    }

    #[test]
    #[should_panic(expected = "at most")]
    fn a_wait_asks_about_a_bounded_number_of_images() {
        let images = vec![STABLE; IMAGES_MAX + 1];
        let _ = until_ready(&mut Scripted::with(STABLE, vec![Ok(Preparation::Ready)]), &mut Fake::default(), &images, DEPLOY_LIMIT);
    }
}
