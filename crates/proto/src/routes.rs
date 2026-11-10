//! The wire's routes, each once: what a client calls, what it sends and
//! what it is answered (docs/wire, which `cargo xtask wire` writes from
//! this table and the types it names). The host's alone (feature
//! `schema`): the cell's own routing is its `match`es, and the e2e holds
//! the two to each other.
//!
//! A route's `operation` is `group.method`: its stable name in the
//! OpenAPI document (`operationId`) and the reference. Renaming one is
//! a change every generated client sees, so it is never renamed.
//! docs/api.md keeps what each route means, who may call it and its
//! limits; the shapes are the types here.

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::live::{LiveIn, LiveOut};
use crate::migrations::{Mailbox, MoveAt, OfferYes, Report, Reported, Roster, RosterTaken, StageComplete, StageCompleted, StageDone, StageOpened, StagePart, StageParts};
use crate::notice::NoticeList;
use crate::vault::{PutSecret, Secret, SecretList, SecretRemoved};
use crate::{
    Archived, ChannelNewest, ChannelPage, Docked, ErrorCode, FragmentList, FragmentStatus, MemberList, Pinned, PostRecord, Posted, PutDraft, SearchAnswer, Seen, SetArchived,
    SetDock, SetPinned,
};
use ErrorCode::{BudgetUsedUp, Conflict, ConflictingBody, Forbidden, InvalidRequest, NotFound, RateLimited, RegistryUnavailable, TooLarge, Unauthenticated};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// A credential a route takes, as the OpenAPI document's security
/// schemes name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    /// A NIP-98 signature (`Authorization: Nostr <event>`): the CLI, the
    /// iOS app, an agent's computer.
    Nip98,
    /// The shell's session cookie, from the platform's own page
    /// (docs/api.md, Sign-in).
    Session,
    /// A fragment's own page's session cookie (its origin's sign-in).
    Site,
    /// The session cookie of a fragment's page framed by another page
    /// (docs/api.md, Frame sessions).
    Frame,
    /// Its share link's token (`?view=`).
    Link,
    /// None: whoever the fragment's visibility lets in (an anonymous
    /// visitor of a public fragment).
    Anyone,
}

/// A type's schema, as the generator of its direction makes it: a
/// request's as the cell reads it, an answer's as the cell writes it.
pub type Shape = fn(&mut SchemaGenerator) -> Schema;

/// `T`'s schema (a reference to its component, or a scalar's own).
pub fn shape<T: JsonSchema>(g: &mut SchemaGenerator) -> Schema {
    g.subschema_for::<T>()
}

/// A type a route takes or answers: its schema, and its round trip
/// through serde (read, then written again), to which `cargo xtask wire`
/// holds each default the schema states: what serde reads of a field left
/// out.
#[derive(Clone, Copy)]
pub struct Ty {
    pub schema: Shape,
    pub round_trip: RoundTrip,
}

/// What serde reads of a JSON value, as it writes it again and as its
/// `Debug` shows it (a value `skip_serializing_if` leaves out of the one
/// is in the other; every wire type's `Debug` is derived), or why it is
/// refused.
pub type RoundTrip = fn(Value) -> Result<(Value, String), String>;

/// `T` as a route's body, answer or frame.
pub const fn ty<T: JsonSchema + Serialize + DeserializeOwned + std::fmt::Debug>() -> Ty {
    Ty { schema: shape::<T>, round_trip: round_trip::<T> }
}

/// `v` read as a `T`, then written again, and its `Debug`.
pub fn round_trip<T: Serialize + DeserializeOwned + std::fmt::Debug>(v: Value) -> Result<(Value, String), String> {
    let t: T = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let shown = format!("{t:?}");
    Ok((serde_json::to_value(t).map_err(|e| e.to_string())?, shown))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamAt {
    Path,
    Query,
}

/// A path or query parameter.
#[derive(Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub at: ParamAt,
    pub required: bool,
    pub schema: Shape,
    pub doc: &'static str,
}

/// What a route takes as its body.
#[derive(Clone, Copy)]
pub enum Body {
    /// JSON of this type.
    Json(Ty),
    /// Bytes of this media type, streamed (`x-fragment-takes`): the signed
    /// URL names their SHA-256 (NIP-98's streamed payload).
    Bytes { media: &'static str },
}

/// What a route answers.
#[derive(Clone, Copy)]
pub enum Answer {
    /// 200, JSON of this type.
    Json(Ty),
    /// A WebSocket (101): the frames a client sends on it, if any, and the
    /// frames it receives.
    Socket { sends: Option<Ty>, receives: Ty },
    /// 200, bytes of this media type; a request with a `Range` of them is
    /// answered 206, that range alone (`x-fragment-bytes`).
    Bytes { media: &'static str },
}

#[derive(Clone, Copy)]
pub struct Route {
    /// `group.method`, never renamed (the module's doc).
    pub operation: &'static str,
    pub method: Method,
    /// OpenAPI's template: `{name}` is a fragment's name in full.
    pub path: &'static str,
    /// What it does, in a line.
    pub summary: &'static str,
    pub auth: &'static [Auth],
    pub params: &'static [Param],
    /// Its body, if it takes one.
    pub request: Option<Body>,
    pub answer: Answer,
    /// The refusals it answers with (`ErrorBody`), beside `ANY_ROUTE`'s.
    pub errors: &'static [ErrorCode],
}

/// What any route may answer (docs/api.md, the contract's preamble).
pub const ANY_ROUTE: [ErrorCode; 2] = [ErrorCode::Deploying, ErrorCode::SchemaRefused];

/// Each group's tag: its name and what its routes are about.
pub const TAGS: [(&str, &str); 8] = [
    ("fragments", "A signer's list of the fragments they hold a role on, and their own view of each."),
    ("notices", "What the platform tells a person, each in its own words, with what they may do about it."),
    ("search", "A signer's search over their own list."),
    ("members", "Who is in a fragment."),
    ("channels", "A fragment's channels: its records, read, posted and drafted."),
    ("live", "A fragment's live socket: records as they are appended, presence, drafts and queries."),
    ("migrations", "The move of finite.computer's agents (docs/migrations.md): the mover's roster, mailbox and reports, a person's answers to the offer to bring an old agent over, and a new agent's stage. Removable, with the move."),
    ("vault", "A person's secrets, which their agents use without holding: set and listed by the person alone, a value never answered."),
];

// Each route's credentials are what its dispatch takes (cell/src/lib.rs):
// every `/api/…` route's `signer` or `signer_for` (a signature, or else
// the shell's session from the platform's own page: `caller`), and a
// fragment's page's routes (`/f/{name}/…`) `site_credential` (a
// signature, its origin's session or frame cookie) beside its share link
// and the anonymous visitor its visibility lets in (`serve`).
const SIGNED: &[Auth] = &[Auth::Nip98, Auth::Session];
// The move's (`/api/migrations/…`): a key's signature alone, the mover's
// (its npub the deployment's `migrations.mover`) or a new agent's; but a
// person's answers to its offer (`/api/migrations/offers/…`), signed.
const SIGNATURE: &[Auth] = &[Auth::Nip98];
const VISITOR: &[Auth] = &[Auth::Nip98, Auth::Site, Auth::Frame, Auth::Link, Auth::Anyone];

const NAME: Param = Param { name: "name", at: ParamAt::Path, required: true, schema: shape::<String>, doc: "The fragment's name, in full." };
const OLD: Param = Param { name: "old", at: ParamAt::Path, required: true, schema: shape::<String>, doc: "The old agent's id (Core's project id)." };
const STAGE: Param = Param { name: "stage", at: ParamAt::Path, required: true, schema: shape::<String>, doc: "The stage's id, `<old>.<phase>.<attempt>`, as its command and its routine name it." };
const PART: Param = Param { name: "n", at: ParamAt::Path, required: true, schema: shape::<u32>, doc: "The part's number, 1 to `PARTS_MAX`." };
const SECRET: Param = Param { name: "secret", at: ParamAt::Path, required: true, schema: shape::<String>, doc: "The secret's name: 1 to 32 of a-z, 0-9 and -." };
const CHANNEL: Param = Param { name: "channel", at: ParamAt::Path, required: true, schema: shape::<String>, doc: "The channel's name." };

pub const ROUTES: &[Route] = &[
    Route {
        operation: "fragments.list",
        method: Method::Get,
        path: "/api/fragments",
        summary: "The signer's fragments, a row each.",
        auth: SIGNED,
        params: &[Param {
            name: "for",
            at: ParamAt::Query,
            required: false,
            schema: shape::<String>,
            doc: "An agent's: the identity it acts for. The rows are the fragments that identity holds a role on where the agent or its owner is a member, each with the role the agent acts with there, `owned` on that identity's own, and no `listed`.",
        }],
        request: None,
        answer: Answer::Json(ty::<FragmentList>()),
        errors: &[Unauthenticated, Forbidden, RegistryUnavailable],
    },
    Route {
        operation: "fragments.watch",
        method: Method::Get,
        path: "/api/fragments/watch",
        summary: "A socket that says when the signer's list, or what they are told, changes.",
        auth: SIGNED,
        params: &[],
        request: None,
        answer: Answer::Socket { sends: None, receives: ty::<ListWatchFrame>() },
        errors: &[Unauthenticated, InvalidRequest, RateLimited],
    },
    Route {
        operation: "notices.list",
        method: Method::Get,
        path: "/api/notices",
        summary: "What the signer is told now, the most pressing first: each owner's notices, in the server's words, with their actions.",
        auth: SIGNED,
        params: &[],
        request: None,
        answer: Answer::Json(ty::<NoticeList>()),
        errors: &[Unauthenticated, Forbidden],
    },
    Route {
        operation: "fragments.archive",
        method: Method::Put,
        path: "/api/fragments/{name}/archived",
        summary: "Archive or unarchive a fragment in the signer's own list.",
        auth: SIGNED,
        params: &[NAME],
        request: Some(Body::Json(ty::<SetArchived>())),
        answer: Answer::Json(ty::<Archived>()),
        errors: &[Unauthenticated, InvalidRequest, NotFound],
    },
    Route {
        operation: "fragments.dock",
        method: Method::Put,
        path: "/api/fragments/{name}/dock",
        summary: "Put a fragment on the signer's dock, or take it off.",
        auth: SIGNED,
        params: &[NAME],
        request: Some(Body::Json(ty::<SetDock>())),
        answer: Answer::Json(ty::<Docked>()),
        errors: &[Unauthenticated, InvalidRequest, NotFound],
    },
    Route {
        operation: "fragments.pin",
        method: Method::Put,
        path: "/api/fragments/{name}/pinned",
        summary: "Pin or unpin a fragment in the signer's sidebar.",
        auth: SIGNED,
        params: &[NAME],
        request: Some(Body::Json(ty::<SetPinned>())),
        answer: Answer::Json(ty::<Pinned>()),
        errors: &[Unauthenticated, InvalidRequest, NotFound],
    },
    Route {
        operation: "fragments.seen",
        method: Method::Put,
        path: "/api/fragments/{name}/seen",
        summary: "The signer has seen a fragment's messages: its row counts none unread.",
        auth: SIGNED,
        params: &[NAME],
        request: None,
        answer: Answer::Json(ty::<Seen>()),
        errors: &[Unauthenticated, InvalidRequest, NotFound],
    },
    Route {
        operation: "search.find",
        method: Method::Get,
        path: "/api/search",
        summary: "Search the signer's own list: titles and names, then messages.",
        auth: SIGNED,
        params: &[Param {
            name: "q",
            at: ParamAt::Query,
            required: true,
            schema: shape::<String>,
            doc: "What to look for, once: words, each in a match as a word or a word's start.",
        }],
        request: None,
        answer: Answer::Json(ty::<SearchAnswer>()),
        errors: &[Unauthenticated, InvalidRequest],
    },
    Route {
        operation: "fragments.status",
        method: Method::Get,
        path: "/api/f/{name}/status",
        summary: "A fragment's status, as the caller's role sees it.",
        auth: SIGNED,
        params: &[NAME],
        request: None,
        answer: Answer::Json(ty::<FragmentStatus>()),
        errors: &[Unauthenticated, Forbidden, NotFound],
    },
    Route {
        operation: "members.list",
        method: Method::Get,
        path: "/api/f/{name}/members",
        summary: "A fragment's members, the first added first.",
        auth: SIGNED,
        params: &[NAME],
        request: None,
        answer: Answer::Json(ty::<MemberList>()),
        errors: &[Unauthenticated, Forbidden, NotFound],
    },
    Route {
        operation: "channels.read",
        method: Method::Get,
        path: "/api/f/{name}/channels/{channel}",
        summary: "A page of a channel's records, always ascending.",
        auth: SIGNED,
        params: &[
            NAME,
            CHANNEL,
            Param { name: "after", at: ParamAt::Query, required: false, schema: shape::<i64>, doc: "Read forward from past this seq (0 when neither cursor is named)." },
            Param { name: "before", at: ParamAt::Query, required: false, schema: shape::<i64>, doc: "Read the records just below this seq." },
            Param { name: "limit", at: ParamAt::Query, required: false, schema: shape::<u64>, doc: "At most `CHANNEL_PAGE`." },
        ],
        request: None,
        answer: Answer::Json(ty::<ChannelPage>()),
        errors: &[Unauthenticated, InvalidRequest, Forbidden, NotFound],
    },
    Route {
        operation: "channels.newest",
        method: Method::Get,
        path: "/api/f/{name}/channels/{channel}/newest",
        summary: "Each poster's newest record of each kind the channel keeps the newest of, the newest first.",
        auth: SIGNED,
        params: &[NAME, CHANNEL],
        request: None,
        answer: Answer::Json(ty::<ChannelNewest>()),
        errors: &[Unauthenticated, InvalidRequest, Forbidden, NotFound],
    },
    Route {
        operation: "channels.post",
        method: Method::Post,
        path: "/api/f/{name}/channels/{channel}",
        summary: "Append a `message` record as the poster.",
        auth: SIGNED,
        params: &[NAME, CHANNEL],
        request: Some(Body::Json(ty::<PostRecord>())),
        answer: Answer::Json(ty::<Posted>()),
        errors: &[Unauthenticated, InvalidRequest, Forbidden, NotFound, ConflictingBody, TooLarge, BudgetUsedUp],
    },
    Route {
        operation: "channels.draft",
        method: Method::Put,
        path: "/api/f/{name}/channels/{channel}/draft",
        summary: "Show a record as it is written to the channel's live readers; never stored.",
        auth: SIGNED,
        params: &[NAME, CHANNEL],
        request: Some(Body::Json(ty::<PutDraft>())),
        answer: Answer::Json(ty::<Done>()),
        errors: &[Unauthenticated, InvalidRequest, Forbidden, NotFound, TooLarge, RateLimited],
    },
    Route {
        operation: "live.open",
        method: Method::Get,
        path: "/f/{name}/__live",
        summary: "A fragment's live socket: subscribe to channels, share presence, run queries.",
        auth: VISITOR,
        params: &[NAME],
        request: None,
        answer: Answer::Socket { sends: Some(ty::<LiveIn>()), receives: ty::<LiveOut>() },
        errors: &[Unauthenticated, Forbidden, NotFound],
    },
    Route {
        operation: "migrations.roster",
        method: Method::Put,
        path: "/api/migrations/mover/roster",
        summary: "The mover's roster, whole: every live old agent and its owner. Rows not yet asked that it leaves out are withdrawn.",
        auth: SIGNATURE,
        params: &[],
        request: Some(Body::Json(ty::<Roster>())),
        answer: Answer::Json(ty::<RosterTaken>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, TooLarge, RegistryUnavailable],
    },
    Route {
        operation: "migrations.mailbox",
        method: Method::Get,
        path: "/api/migrations/mover/mailbox",
        summary: "What the mover is told to do, oldest first: each command stays until a report on it is taken.",
        auth: SIGNATURE,
        params: &[],
        request: None,
        answer: Answer::Json(ty::<Mailbox>()),
        errors: &[Unauthenticated, Forbidden],
    },
    Route {
        operation: "migrations.report",
        method: Method::Post,
        path: "/api/migrations/mover/agents/{old}/report",
        summary: "What happened on an old agent's current command: a stale command's report is 409 and changes nothing.",
        auth: SIGNATURE,
        params: &[OLD],
        request: Some(Body::Json(ty::<Report>())),
        answer: Answer::Json(ty::<Reported>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict],
    },
    Route {
        operation: "migrations.part",
        method: Method::Put,
        path: "/api/migrations/mover/stages/{stage}/parts/{n}",
        summary: "One part of the stage's zip, streamed unread into the platform's bucket: only for the stage of the old agent's current stop_and_stage, while it stages.",
        auth: SIGNATURE,
        params: &[
            STAGE,
            PART,
            Param { name: "sha256", at: ParamAt::Query, required: true, schema: shape::<String>, doc: "The part's SHA-256 (hex), as the mover computed it: kept with the part for its resume." },
        ],
        request: Some(Body::Bytes { media: "application/octet-stream" }),
        answer: Answer::Json(ty::<StagePart>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, Conflict, TooLarge],
    },
    Route {
        operation: "migrations.parts",
        method: Method::Get,
        path: "/api/migrations/mover/stages/{stage}/parts",
        summary: "The stage's parts taken so far, to resume an upload: their numbers, sizes, hashes and etags, never their bytes.",
        auth: SIGNATURE,
        params: &[STAGE],
        request: None,
        answer: Answer::Json(ty::<StageParts>()),
        errors: &[Unauthenticated, Forbidden, Conflict],
    },
    Route {
        operation: "migrations.complete",
        method: Method::Post,
        path: "/api/migrations/mover/stages/{stage}/complete",
        summary: "The stage's zip made whole from its parts, then its manifest written, last.",
        auth: SIGNATURE,
        params: &[STAGE],
        request: Some(Body::Json(ty::<StageComplete>())),
        answer: Answer::Json(ty::<StageCompleted>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, Conflict],
    },
    Route {
        operation: "migrations.open",
        method: Method::Post,
        path: "/api/migrations/stages/{stage}/open",
        summary: "The new agent bound to the move opens its stage: its manifest.",
        auth: SIGNATURE,
        params: &[STAGE],
        request: None,
        answer: Answer::Json(ty::<StageOpened>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict],
    },
    Route {
        operation: "migrations.zip",
        method: Method::Get,
        path: "/api/migrations/stages/{stage}/zip",
        summary: "The stage's zip, for the new agent bound to the move while it imports, by ranges (`Range: bytes=…`).",
        auth: SIGNATURE,
        params: &[STAGE],
        request: None,
        answer: Answer::Bytes { media: "application/zip" },
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict],
    },
    Route {
        operation: "migrations.done",
        method: Method::Post,
        path: "/api/migrations/stages/{stage}/done",
        summary: "The new agent says its import ended, well or not: its stage's reads end, and the old agent is retired once it went well. A phase 1 stage is deleted; a phase 2 stage is kept until it expires.",
        auth: SIGNATURE,
        params: &[STAGE],
        request: Some(Body::Json(ty::<StageDone>())),
        answer: Answer::Json(ty::<MoveAt>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict],
    },
    Route {
        operation: "migrations.later",
        method: Method::Post,
        path: "/api/migrations/offers/{old}/later",
        summary: "The person's Not now: their old agent's offer is asked again after a day.",
        auth: SIGNED,
        params: &[OLD],
        request: None,
        answer: Answer::Json(ty::<MoveAt>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict],
    },
    Route {
        operation: "migrations.never",
        method: Method::Post,
        path: "/api/migrations/offers/{old}/never",
        summary: "The person's Never: their old agent's offer is asked no more, until they ask again.",
        auth: SIGNED,
        params: &[OLD],
        request: None,
        answer: Answer::Json(ty::<MoveAt>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict],
    },
    Route {
        operation: "migrations.again",
        method: Method::Post,
        path: "/api/migrations/offers/{old}/ask-again",
        summary: "The person's Ask again, after a Never: their old agent's offer is asked again now.",
        auth: SIGNED,
        params: &[OLD],
        request: None,
        answer: Answer::Json(ty::<MoveAt>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict],
    },
    Route {
        operation: "migrations.yes",
        method: Method::Post,
        path: "/api/migrations/offers/{old}/yes",
        summary: "The person's Yes, bound to a new agent of their own: the old agent is stopped and its stage made, which the new agent imports.",
        auth: SIGNED,
        params: &[OLD],
        request: Some(Body::Json(ty::<OfferYes>())),
        answer: Answer::Json(ty::<MoveAt>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest, NotFound, Conflict, RegistryUnavailable],
    },
    Route {
        operation: "vault.list",
        method: Method::Get,
        path: "/api/vault",
        summary: "The signer's secrets, by name, each with its hosts, place and agents: never a value.",
        auth: SIGNED,
        params: &[],
        request: None,
        answer: Answer::Json(ty::<SecretList>()),
        errors: &[Unauthenticated, Forbidden],
    },
    Route {
        operation: "vault.put",
        method: Method::Put,
        path: "/api/vault/{secret}",
        summary: "Sets a secret, whole: its value (sealed, never answered), the hosts it may go to, where it goes on a request, and the agents that may use it.",
        auth: SIGNED,
        params: &[SECRET],
        request: Some(Body::Json(ty::<PutSecret>())),
        answer: Answer::Json(ty::<Secret>()),
        errors: &[Unauthenticated, Forbidden, InvalidRequest],
    },
    Route {
        operation: "vault.remove",
        method: Method::Delete,
        path: "/api/vault/{secret}",
        summary: "Removes a secret: its agents' next use of it is refused.",
        auth: SIGNED,
        params: &[SECRET],
        request: None,
        answer: Answer::Json(ty::<SecretRemoved>()),
        errors: &[Unauthenticated, Forbidden],
    },
];

// Shapes the cell writes as text rather than through a type (a frame
// it sends as a constant, an ack built with `json!`), described here
// for the schema alone, so the cell's Wasm stays as it is. One moves
// beside the others in lib.rs once the cell writes it through the type
// (measure the Wasm then).

/// `{ok: true}`: the call did what it asked, and there is nothing more to
/// say (a draft shown, say).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Done {
    pub ok: bool,
}

/// A frame of `GET /api/fragments/watch`, the socket that follows a
/// signer's list. Frames name nothing: on `changed` the page reads its
/// list, and what it is told (`GET /api/notices`), again with its own
/// credential. The cell writes these as constant text
/// (cell/src/principal.rs).
///
/// Open (`x-fragment-open`, docs/wire): a reader ignores a frame it does
/// not know (the shell's list, the iOS app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(extend("x-fragment-open" = true))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ListWatchFrame {
    /// First, as the socket opens.
    Hello,
    /// Their list changed: a row added, gone, or changed.
    Changed,
    /// The invites waiting on their emails changed. Sent no more since
    /// FIN-458 (an invite is a notice: `changed`); kept for the readers
    /// that know it.
    Invited,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: each route's credentials are what its dispatch takes. Method:
    /// an `/api/…` route is a signer's alone (`signer`, `signer_for`), the
    /// move's a signature alone (cell/src/migrations.rs), and a fragment's
    /// page's (`/f/{name}/…`) a visitor's (`site_credential`).
    #[test]
    fn credentials_are_the_dispatchs() {
        for r in ROUTES {
            match r.path {
                p if p.starts_with("/api/migrations/offers/") => assert_eq!(r.auth, SIGNED, "{}", r.operation),
                p if p.starts_with("/api/migrations/") => assert_eq!(r.auth, SIGNATURE, "{}", r.operation),
                p if p.starts_with("/api/") => assert_eq!(r.auth, SIGNED, "{}", r.operation),
                p if p.starts_with("/f/{name}/") => assert_eq!(r.auth, VISITOR, "{}", r.operation),
                p => panic!("{} at {p}: say whose dispatch it is", r.operation),
            }
        }
    }

    /// Goal: an operation names one route, and a route one operation, for
    /// good: OpenAPI's `operationId` is unique, and a generated client's
    /// method is its name. Method: every pair of rows differs in both.
    #[test]
    fn each_route_is_named_once() {
        for (i, a) in ROUTES.iter().enumerate() {
            let (group, method) = a.operation.split_once('.').expect("group.method");
            assert!(!group.is_empty() && !method.is_empty() && !method.contains('.'), "{}", a.operation);
            assert!(TAGS.iter().any(|(t, _)| *t == group), "{} is in no tag", a.operation);
            for b in &ROUTES[i + 1..] {
                assert_ne!(a.operation, b.operation);
                assert!((a.method, a.path) != (b.method, b.path), "{} {} twice", a.method.as_str(), a.path);
            }
        }
    }
}
