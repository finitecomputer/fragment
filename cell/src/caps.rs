//! The capabilities a live operation's call uses (the Finite Agent spike;
//! entry.mjs `Models`, `Fragments`, `Commands`), each answered only while
//! one of this fragment's live calls runs (jobs.rs `step_live`):
//!
//!   /call/models/chat            the model route, as the fragment: its
//!        owner pays, an agent fragment's calls are its agent's; the answer
//!        streamed back as the model gives it (models.rs `complete`, the
//!        agent route's own path)
//!   /call/fragments/fetch    another fragment's control API, as this
//!        fragment's agent: routed as the router routes a signed request,
//!        its membership and role checked there (routed.rs `Routed::ask`,
//!        the share sheet's and search's path)
//!   /call/commands/exec        a command on the computer the agent uses,
//!        its output streamed (computer.rs `exec_for`)
//!
//! An agent fragment acts as its agent, the identity the registry gave it
//! when it was first assigned to a computer (runs_on.rs `assign`): one
//! never assigned has none, and is refused.

use fragment_proto::{valid_fragment_name, ErrorCode};
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::FragmentsFetch;
use crate::fragment::{missing, FragmentCell, MetaKey};
use crate::models::{InFragment, ModelCall};
use crate::routed::{Routed, Signed};

impl FragmentCell {
    /// Its capabilities answer a live call alone, while it runs.
    fn live_only(&self) -> CellResult<()> {
        if self.live_calls.get() == 0 {
            return Err(CellError::new(ErrorCode::Forbidden, "this capability answers a live operation's call alone, while it runs"));
        }
        Ok(())
    }

    pub(crate) async fn cap_models(&self, v: Value) -> CellResult<Response> {
        self.live_only()?;
        let [owner, name, agent] = self.metas([MetaKey::Owner, MetaKey::Name, MetaKey::AgentIdentity])?;
        let (owner, name) = (owner.ok_or_else(|| missing(MetaKey::Owner))?, name.ok_or_else(|| missing(MetaKey::Name))?);
        let named = self.cfg.models.named(v.get("model").and_then(Value::as_str), None).map_err(|e| CellError::invalid(e.message()))?;
        let stream = v.get("stream") == Some(&Value::Bool(true));
        let call = ModelCall { payer: &owner, agent: agent.as_deref(), fragment: Some(InFragment { name: &name, owner: &owner, by_owner: true }), named, route: None, body: v, stream };
        crate::models::complete(&self.env, call, &self.state).await
    }

    pub(crate) async fn cap_fragments(&self, r: FragmentsFetch) -> CellResult<Response> {
        self.live_only()?;
        if !valid_fragment_name(&r.name) || !r.path.starts_with("/api/") || r.path.split(['/', '?']).any(|s| s == "..") {
            return Err(CellError::invalid("a fragment's name, and an /api/ route there"));
        }
        let method = match r.method.as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            m => return Err(CellError::invalid(format!("method {m:?}"))),
        };
        let who = self.as_agent().await?;
        let url = Url::parse(&format!("{}/api/f/{}{}", self.cfg.platform_url, r.name, &r.path["/api".len()..])).map_err(|e| CellError::invalid(format!("path: {e}")))?;
        let body = r.body.map(|b| b.to_string());
        Routed { name: r.name, url, signed: Some(who), credential: None }.ask(&self.env, method, &r.path, body.as_deref(), None).await
    }

    pub(crate) async fn cap_computer(&self, mut ask: Value) -> CellResult<Response> {
        self.live_only()?;
        let [owner, agent, computer] = self.metas([MetaKey::Owner, MetaKey::AgentIdentity, MetaKey::Computer])?;
        let owner = owner.ok_or_else(|| missing(MetaKey::Owner))?;
        if agent.is_none() {
            return Err(CellError::new(ErrorCode::Forbidden, "only an agent's fragment runs commands on its computer"));
        }
        // the computer it uses; none named, its owner's one (M16 step 5 names it)
        let computer = computer.unwrap_or_else(|| fragment_core::computer::default_computer_of(&owner));
        ask["owner"] = json!(owner);
        let text = ask.to_string();
        crate::routed::send(&self.env, "COMPUTER", &computer, crate::routed::internal_request("computer/exec", &text)?).await
    }

    /// This fragment's agent, as the registry knows it now.
    async fn as_agent(&self) -> CellResult<Signed> {
        let id = self.meta(MetaKey::AgentIdentity)?.ok_or_else(|| CellError::new(ErrorCode::Forbidden, "an agent's fragment acts elsewhere as its agent, and this one has none yet"))?;
        let identity = crate::ask_registry(&self.env, &crate::registry::calls::Lookup { who: id }).await?;
        Ok(Signed::new(identity, None))
    }
}
