//! The deployment's search service in a container of its own (Cloudflare
//! Containers; search/README.md, "On Cloudflare"): one Durable Object for
//! its store (`searchable::Container::object`), whose container runs the
//! search image, a pool of one, its store on R2 (`Config::search` is
//! `searchable::At::Container`). A call reaches it through `routed::send`
//! (searchable.rs) and is refused first if it is another release's, as
//! every object's is. The object starts its container if it is not
//! running, with its store, its R2 key and the token
//! (`searchable::Container::env`, read from Secrets Store as it starts),
//! waits for it to answer, and passes the call on untouched: entry.mjs's
//! `SearchHost`, Cloudflare's own pattern (workers-rs has no Containers).
//!
//! The container stops once its object has been idle ten minutes
//! (entry.mjs, `SEARCH_IDLE_MS`), so a deployment that searches nothing
//! runs none. While the service still has work of its own (`/healthz`'s
//! `busy`: a deletion's maintenance, its sweep after a start), an alarm
//! keeps the object, and so its container, awake: what a write removed
//! leaves the store within minutes, whether another call comes or not.

use fragment_core::searchable::At;
use fragment_proto::ErrorCode;
use serde_json::json;
use wasm_bindgen::{JsCast, JsValue};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::js;

/// How often a busy service's object is woken, under the container's idle
/// time (entry.mjs, `SEARCH_IDLE_MS`), so it never stops while busy.
const KEEP_AWAKE_MS: i64 = 60_000;

#[durable_object]
pub struct SearchCell {
    state: State,
    raw: JsValue,
    env: Env,
    /// The isolate's settings (config.rs: built once per isolate).
    cfg: &'static Config,
}

impl DurableObject for SearchCell {
    fn new(state: State, env: Env) -> Self {
        let raw: JsValue = state._inner().into();
        let state = State::from(raw.clone().unchecked_into::<worker_sys::DurableObjectState>());
        let cfg = Config::from_env(&env);
        SearchCell { state, raw, env, cfg }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        if let Some(refused) = crate::routed::refused(&req, crate::routed::Own::of(self.cfg), &self.raw)? {
            return Ok(refused);
        }
        match self.pass(req).await {
            Ok(resp) => Ok(resp),
            Err(e) => e.response(),
        }
    }

    /// Awake while the service has work of its own left: woken again,
    /// else let sleep (its container stops once the object has been idle).
    async fn alarm(&self) -> Result<Response> {
        let busy = match self.host() {
            Ok(host) => js::invoke(&host, "busy", &[]).await.map(|b| b.as_bool() == Some(true)),
            Err(e) => Err(e),
        };
        match busy {
            Ok(true) => self.wake_in(KEEP_AWAKE_MS).await?,
            Ok(false) => {}
            Err(e) => {
                console_error!("{}", json!({ "event": "search.container-unasked", "error": e.message }));
                self.wake_in(KEEP_AWAKE_MS).await?;
            }
        }
        Response::ok("")
    }
}

impl SearchCell {
    fn host(&self) -> CellResult<JsValue> {
        let host = js::property(&self.raw, "searchHost")?;
        if host.is_undefined() {
            return Err(CellError::host("this Durable Object has no search host (entry.mjs's Search class)"));
        }
        Ok(host)
    }

    /// `req` passed to the service, its container started first if it is
    /// not running, and the object kept awake a while after.
    async fn pass(&self, req: Request) -> CellResult<Response> {
        let Some(At::Container(container)) = &self.cfg.search else {
            return Err(CellError::new(ErrorCode::NotFound, "this deployment runs no search container"));
        };
        let token = crate::keys::search_token(&self.env).await?;
        let key = crate::keys::search_r2_key(&self.env).await?;
        let env = js_sys::Object::new();
        for (name, value) in container.env(&token, key.as_ref().map(|(id, secret)| (id.as_str(), secret.as_str()))) {
            js_sys::Reflect::set(&env, &name.into(), &value.into()).map_err(|_| CellError::host("the container's environment"))?;
        }
        let req: &JsValue = req.inner().as_ref();
        let answer = js::invoke(&self.host()?, "forward", &[req.clone(), env.into()]).await?;
        let answer: web_sys::Response = answer.dyn_into().map_err(|_| CellError::host("the search container answered no response"))?;
        if self.state.storage().get_alarm().await?.is_none() {
            self.wake_in(KEEP_AWAKE_MS).await?;
        }
        Ok(Response::from(answer))
    }

    async fn wake_in(&self, ms: i64) -> Result<()> {
        let at = js::now_ms() + ms;
        self.state.storage().set_alarm(ScheduledTime::new(js_sys::Date::new(&JsValue::from_f64(at as f64)))).await
    }
}
