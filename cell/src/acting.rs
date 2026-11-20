//! An agent's fragment acting in another fragment as its agent (docs/api.md,
//! "Acting in another fragment"): its app's `FRAGMENTS` capability
//! (entry.mjs `Fragments`: a draft, a read) and its jobs' `fragments` steps
//! (a post, a read). Only an agent's fragment acts: as its agent, whose key
//! is the fragment's own (registered at its first assignment: runs_on.rs
//! `assign`). Each act goes to the target as the router hands it a request
//! that key signed, its body as the app sent it, so the target checks who
//! is asking exactly as for the agent's own request (its membership and
//! role, its owner's hold on it, a `relayed` record's door): it opens
//! nothing the agent could not do. The router's own check of an agent is
//! for routes no act names (`fragment_core::access::reserved`).
//!
//! The code that acts is its owner's and its agent's alone: it acts only
//! while no one else may edit the fragment, and only code its owner or its
//! agent put live themselves (`vouch`, at their deploy or refresh) or a
//! blessed template's release (`MetaKey::ActsAs`, which each install
//! clears or sets: plane.rs), so an editor's code acts nowhere, even once
//! they are gone, nor a deploy of theirs that lands after.
//!
//! A post is a job's step: it carries its run's hop depth, as a job's
//! fetch and publish do, so fragments triggering one another stop at
//! `HOP_DEPTH_MAX`, and it is made once however often the job's body runs.
//!
//! It could later replace the platform's own cases of the same act: a
//! search across fragments as its signer (across.rs), and a computer's
//! bridge posting as its agent through the public API.

use fragment_core::acting::{self, Method as Asked, Refused};
use fragment_proto::{ErrorCode, IdentityKind, Role};
use worker::{Method, Response, SqlStorageValue, Url};

use crate::error::{CellError, CellResult};
use crate::fragment::{FragmentCell, MetaKey};
use crate::routed::{Routed, Signed};

/// Where the app facet's acts arrive: `<act>/<fragment>/<channel>`.
pub(crate) const PREFIX: &str = "/call/fragments/";

impl FragmentCell {
    /// An act of the app's capability (`PREFIX`), the target's answer as it
    /// made it.
    pub(crate) async fn act(&self, path: &str, body: &[u8]) -> CellResult<Response> {
        self.act_as_agent(path.strip_prefix(PREFIX).unwrap_or_default(), body, None).await
    }

    /// An act (`<act>/<fragment>/<channel>`, `fragment_core::acting`), from
    /// a job's step at its run's `depth`, or from the capability (`None`).
    pub(crate) async fn act_as_agent(&self, at: &str, body: &[u8], depth: Option<u32>) -> CellResult<Response> {
        let act = acting::read(at, body, depth).map_err(|r| match r {
            Refused::Invalid(why) => CellError::invalid(why),
            Refused::Unknown(why) => CellError::new(ErrorCode::NotFound, why),
        })?;
        self.vouched()?;
        let url = Url::parse(&format!("{}/api/f/{}{}", self.cfg.platform_url, act.fragment, act.route)).map_err(|e| CellError::host(e.to_string()))?;
        let routed = Routed { name: act.fragment.to_string(), url, signed: Some(self.as_agent().await?), credential: None };
        let method = match act.method {
            Asked::Get => Method::Get,
            Asked::Post => Method::Post,
            Asked::Put => Method::Put,
        };
        let hops = act.hops.map(|d| d.to_string());
        routed.ask(&self.env, method, &format!("/api{}", act.route), act.body, hops.as_deref().map(|h| (crate::jobs::HOPS_HEADER, h))).await
    }

    /// Whether anyone but its owner and its agent may edit it.
    pub(crate) fn others_edit(&self) -> CellResult<bool> {
        let [owner, agent] = self.metas([MetaKey::Owner, MetaKey::AgentIdentity])?;
        let rows = self.rows(
            "SELECT 1 AS x FROM members WHERE role IN (?, ?) AND principal NOT IN (?, ?) LIMIT 1",
            vec![Role::Editor.as_str().into(), Role::Owner.as_str().into(), owner.unwrap_or_default().into(), SqlStorageValue::from(agent.unwrap_or_default())],
        )?;
        Ok(!rows.is_empty())
    }

    /// Its owner or its agent (`who`) put live, or confirmed, the code
    /// installed now, while they alone may edit it: that code acts as its
    /// agent (`vouched`). Anyone else's deploy or refresh vouches for none.
    pub(crate) fn vouch(&self, who: &str) -> CellResult<()> {
        let [owner, agent, live] = self.metas([MetaKey::Owner, MetaKey::AgentIdentity, MetaKey::PinLive])?;
        if (Some(who) != owner.as_deref() && Some(who) != agent.as_deref()) || self.others_edit()? {
            return Ok(());
        }
        #[derive(serde::Deserialize)]
        struct Installed {
            sha: String,
            loader_id: String,
        }
        let code: Vec<Installed> = self.typed("SELECT sha, loader_id FROM code WHERE id = 1", vec![])?;
        if let Some(code) = code.into_iter().find(|c| Some(&c.sha) == live.as_ref()) {
            self.set_meta(MetaKey::ActsAs, &code.loader_id)?;
        }
        Ok(())
    }

    /// Its code acts as its agent only while its owner and its agent alone
    /// may edit it, and only the code they vouched for (`vouch`).
    fn vouched(&self) -> CellResult<()> {
        let refused = |why: &str| CellError::new(ErrorCode::Forbidden, format!("its code acts as its agent elsewhere only {why}"));
        if self.others_edit()? {
            return Err(refused("while its owner and its agent alone may edit it"));
        }
        if self.meta(MetaKey::ActsAs)?.is_none_or(|code| self.installed_code().ok().flatten() != Some(code)) {
            return Err(refused("from code its owner or its agent put live themselves: deploy it again"));
        }
        Ok(())
    }

    /// Its agent, as the router resolves a request its key signs.
    async fn as_agent(&self) -> CellResult<Signed> {
        let none = || CellError::new(ErrorCode::Forbidden, "only an agent's fragment acts in another fragment, as its agent: this one is no agent's");
        let key = self.own_key()?;
        let identity = match crate::ask_registry(&self.env, &crate::registry::calls::Resolve { key: key.clone() }).await {
            Err(e) if e.code == ErrorCode::Unauthenticated => return Err(none()),
            other => other?,
        };
        if identity.kind != IdentityKind::Agent {
            return Err(none());
        }
        Ok(Signed::new(identity, Some(key)))
    }
}
