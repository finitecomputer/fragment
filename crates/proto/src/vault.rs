//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): the
//! secrets their agents use without holding them. A value goes in and
//! never comes out: every answer here names a secret, never its value.

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
    /// The agents that may use it, each by its fragment's name: at least
    /// one (Paul, 2026-11-09: per agent).
    pub agents: Vec<String>,
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

/// `POST /api/vault/asks`: an agent asks its person for a secret. The
/// person answers in a card the
/// platform draws (a notice, `GET /api/notices`, also reached directly at
/// `SecretAsk.url`), never in a chat's
/// records, so no agent, model or app ever reads what they paste.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AskSecret {
    /// The secret's name in its person's Vault (1 to 32 of `a-z0-9-`).
    pub name: String,
    /// Why, in the agent's words, shown on the card: 1 to 200 characters.
    pub why: String,
    /// The hosts it would go to (exact DNS names, at most 8); none, kept
    /// and sent nowhere until its person says where.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub hosts: Vec<String>,
}

/// Where an ask stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AskState {
    /// On its person's card, not answered yet.
    Waiting,
    /// Answered: the secret is in their Vault, bound to the agent.
    Saved,
    /// Their Decline.
    Declined,
    /// Not answered in time, or asked again since.
    Expired,
}

impl AskState {
    /// Its name, as the wire and the Vault's rows say it.
    pub fn name(self) -> &'static str {
        match self {
            AskState::Waiting => "waiting",
            AskState::Saved => "saved",
            AskState::Declined => "declined",
            AskState::Expired => "expired",
        }
    }

    /// The state `name` names (`name`'s inverse).
    pub fn named(name: &str) -> Option<AskState> {
        [AskState::Waiting, AskState::Saved, AskState::Declined, AskState::Expired].into_iter().find(|s| s.name() == name)
    }
}

/// An ask, as its agent and its person read it: never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SecretAsk {
    /// 32 hex.
    pub id: String,
    pub name: String,
    pub state: AskState,
    /// When it expires unanswered, in ms since the epoch.
    pub expires_at: i64,
    /// Platform-origin URL the person can open directly (`/vault/ask/{id}`)
    /// to enter or approve this secret on the platform's own page.
    pub url: String,
}

/// `POST /api/vault/asks/{id}`: the person's answer — what they pasted for a
/// new secret, or `allow: true` to share or widen a secret they already keep.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AnswerAsk {
    /// Which secret its card showed held under the ask's name (its
    /// revision, `"none"` for none), as the card's action carries it: the
    /// answer grants what the card said, or nothing (409).
    pub held: String,
    /// What the person pasted (required for a new secret; refused with `allow`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub value: Option<String>,
    /// True on an Allow card for an existing secret: keeps its sealed value
    /// and place, adds the asking agent, and unions any asked hosts.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend("default" = false)))]
    pub allow: bool,
}
