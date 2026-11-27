//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): the
//! secrets their agents and apps use without holding them. A value goes in
//! and never comes out: every answer here names a secret, never its value.

use serde::{Deserialize, Serialize};

/// `PUT /api/vault/{secret}`: a secret set (or set again), whole. Only its
/// person sets one, so only they say where its value goes; set again with
/// no value, it keeps the one it holds and goes where this says. Its
/// `Debug` shows the value, as the wire's checker needs: never print one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PutSecret {
    /// The value: a printable token, or, for `place.sign`, a Nostr secret
    /// key as 64 hex. None, the value it holds is kept: a secret it does
    /// not hold yet, or one that would go from signing to filling or back,
    /// is refused (400).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub value: Option<String>,
    /// The only hosts it is ever sent to (exact DNS names); none, it is
    /// parked and fills nothing.
    pub hosts: Vec<String>,
    /// Where it goes on a request; none, `Authorization: Bearer {}`.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend("default" = {})))]
    pub place: Place,
    /// The agents that may use it, each by its fragment's name (Paul,
    /// 2026-11-09: per agent). With `apps`, at least one between them.
    pub agents: Vec<String>,
    /// The apps that may use it, each under the name its code fills
    /// (FIN-615, PR 12: per app, as per agent).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub apps: Vec<AppUse>,
}

/// An app of its person's that may use a secret, and the name its code
/// fills it by: `{{KEY}}` in a job's fetch, which an app's own secret of
/// that name fills first (docs/secrets.md, "A person's Vault").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AppUse {
    /// The app's fragment name.
    pub app: String,
    /// The secret's name in its code (`^[A-Z][A-Z0-9_]{0,63}$`).
    pub key: String,
}

/// `PUT /api/vault/{secret}/apps/{app}`: the secret lent to `app` under
/// `key`, the name its code fills, in place of any name it had for that
/// app; nothing else of the secret changes, so a lend never puts back what
/// its person changed meanwhile (FIN-615, PR 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LendSecret {
    /// The secret's name in the app's code (`^[A-Z][A-Z0-9_]{0,63}$`).
    pub key: String,
}

/// Where a secret goes on a request: one of a header in a format
/// (`{"header": "authorization", "format": "Bearer {}"}`), a query
/// parameter (`{"query": "key"}`), a half of basic auth (`{"basic":
/// "password"}`), or a NIP-98 signature over the request (`{"sign":
/// "nip98"}`: the Vault signs, its key never leaves it). None of them,
/// `Authorization: Bearer {}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Place {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub header: Option<String>,
    /// The header's value, `{}` the secret; with `header` alone, `{}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub query: Option<String>,
    /// `user` or `password`: which half of basic auth it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub basic: Option<String>,
    /// `nip98`: a NIP-98 signature over the request (kind 27235, its URL,
    /// method and body), in `Authorization: Nostr …`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub sign: Option<String>,
}

/// A secret as its person reads it: everything but its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Secret {
    pub name: String,
    pub hosts: Vec<String>,
    pub place: Place,
    pub agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub apps: Vec<AppUse>,
    /// When its value was set, in ms since the epoch.
    pub set_at: i64,
}

/// `GET /api/vault`: the person's secrets, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SecretList {
    pub secrets: Vec<Secret>,
}

/// `DELETE /api/vault/{secret}`: whether there was one to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SecretRemoved {
    pub name: String,
    pub removed: bool,
}
