//! The `FRAGMENTS` capability (entry.mjs `Fragments`; docs/api.md, Apps):
//! a fragment's app acting in another fragment as the fragment's own
//! principal. Only an agent's fragment has one: its agent, whose key is
//! the fragment's own (registered at its first assignment: runs_on.rs
//! `assign`). Each act goes to the target as the router hands it a request
//! that key signed, its body as the app sent it, so the target checks who
//! is asking exactly as for the agent's own request (its membership and
//! role, its owner's hold on it, a `relayed` record's door): it opens
//! nothing the agent could not do. The router's own check of an agent is
//! for routes no act names (`fragment_core::access::reserved`).
//!
//! A post says how deep in a chain of triggered runs the app's call is
//! (`depth_now`), as a job's fetch does, so its record's runs are a hop
//! deeper and fragments that trigger one another stop at `HOP_DEPTH_MAX`,
//! as one fragment's chain does.
//!
//! It could later replace the platform's own cases of the same act: a
//! search across fragments as its signer (across.rs), and a computer's
//! bridge posting as its agent through the public API.

use fragment_core::acting::{self, Method as Asked, Refused};
use fragment_proto::{ErrorCode, IdentityKind};
use worker::{Method, Response, Url};

use crate::error::{CellError, CellResult};
use crate::fragment::FragmentCell;
use crate::routed::{Routed, Signed};

/// Where the app facet's acts arrive: `<act>/<fragment>/<channel>`.
pub(crate) const PREFIX: &str = "/call/fragments/";

/// One of the app's calls into its facet while it runs at a hop depth (a
/// job's, a query's): `depth_now` is the deepest of those under way.
pub(crate) struct AtDepth<'a> {
    cell: &'a FragmentCell,
    depth: u32,
}

impl Drop for AtDepth<'_> {
    fn drop(&mut self) {
        let mut depths = self.cell.app_depths.borrow_mut();
        let at = depths.iter().position(|d| *d == self.depth).expect("a depth is pushed before its guard is made");
        depths.swap_remove(at);
    }
}

impl FragmentCell {
    /// An app call at `depth` under way until the guard drops.
    pub(crate) fn at_depth(&self, depth: u32) -> AtDepth<'_> {
        self.app_depths.borrow_mut().push(depth);
        AtDepth { cell: self, depth }
    }

    /// How deep the app's call that acts is: the deepest of its calls under
    /// way (0 for none: a page's, a person's). An act names no call, and
    /// calls at once in one object are few, so the deepest is a stand-in
    /// that never makes a chain look shorter than it is.
    fn depth_now(&self) -> u32 {
        self.app_depths.borrow().iter().copied().max().unwrap_or(0)
    }

    /// An act of the app's in another fragment (`PREFIX`), the target's
    /// answer as it made it.
    pub(crate) async fn act(&self, path: &str, body: &[u8]) -> CellResult<Response> {
        let act = acting::read(path.strip_prefix(PREFIX).unwrap_or_default(), body, self.depth_now()).map_err(|r| match r {
            Refused::Invalid(why) => CellError::invalid(why),
            Refused::Unknown(why) => CellError::new(ErrorCode::NotFound, why),
        })?;
        let url = Url::parse(&format!("{}/api/f/{}{}", self.cfg.platform_url, act.fragment, act.route)).map_err(|e| CellError::host(e.to_string()))?;
        let routed = Routed { name: act.fragment.to_string(), url, signed: Some(self.as_agent().await?), credential: None };
        let method = match act.method {
            Asked::Get => Method::Get,
            Asked::Post => Method::Post,
            Asked::Put => Method::Put,
        };
        let hops = act.hops.map(|d| d.to_string());
        routed.ask(&self.env, method, &format!("/api{}", act.route), act.body.as_deref(), hops.as_deref().map(|h| (crate::jobs::HOPS_HEADER, h))).await
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
