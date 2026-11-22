//! Who may do what: the one place that decides permission (docs/api.md,
//! Principals and access; FIN-615, "Who acts for whom: One Right Way").
//! It is pure: no storage, no clock, no network. A caller reads what it
//! knows of a request once (`Facts`) and asks `decide`, whose `Verdict`
//! carries its reason; the caller only says it in its own wire's words.
//!
//! `decide`'s rules, in the order they apply:
//!
//! 1. **A caller's own role.** Their membership role if they have one.
//!    Otherwise the fragment's visibility gives it: a public fragment
//!    gives everyone the `public` floor, and a share link (on a `link` or
//!    `public` fragment) counts as a viewer. A `members` fragment gives
//!    non-members nothing.
//! 2. **An agent's owner reads what the agent reads** (FIN-11): a person
//!    who owns an agent member reads the fragment as a viewer, and never
//!    acts through it (no operations, nothing a membership would let them
//!    change).
//! 3. **An agent acts for its owner, capped** (decision R17): a request an
//!    agent signs `for=<its owner>` (no one else: `may_act_for`) acts with
//!    the lower of that identity's role and a cap. The cap is the agent's
//!    own role (its membership, or the visibility floor), or, on a
//!    fragment its owner belongs to, the owner's role; never more than
//!    `editor`. The owner's part is the owner's own role capped at
//!    `editor`, not `editor` outright: an agent never reaches further than
//!    its owner could, so a key that signs `for` someone else gains nothing
//!    its owner does not hold.
//! 4. **A hold** (decision 36): an agent its owner holds below them acts
//!    with at most its hold with its owner's access, whomever it acts for,
//!    and as itself where its person is a guest (decision 64: the whole
//!    entity is secured); never in its own memberships elsewhere (its
//!    chats), where it answers and records its turns. Its other, a share
//!    marked "people only" that lent the member's agents nothing, went on
//!    2026-10-13 (Paul: "Giving a person access should imply giving their
//!    agent access. It's basically 'giving all the pubkeys under the
//!    umbrella of this email' access"): a member's agents reach what the
//!    member does, capped as above.
//! 5. **A connector is its owner's alone to edit**: everyone else, their
//!    own agent acting for them included, is at most a viewer there.
//! 6. **The repo is its members'** (`Action::Repo`): its whole history,
//!    whatever the visibility; no floor, share link or agent of theirs
//!    that is a member lends it.
//! 7. **Short of the role**, an anonymous caller is told to sign in (401)
//!    and a signed one is refused (403).
//!
//! Some control routes need more than a role (`reserved`). Sharing
//! (members, invites, visibility, the links) is the owner's, and, since
//! Paul's "yes, your agent can share on your behalf" (2026-10-04), their
//! own agent's acting for them: an agent that acts for its own owner, held
//! at nothing below them, shares a fragment its owner owns as its owner
//! would (`agent_shares`), under the same role rules (it never makes
//! anyone owner, itself included). For anyone else, as itself, held, or on
//! a fragment its owner only edits or views, it shares nothing. What an
//! agent shares is recorded as the agent's, for its owner, so its owner
//! sees it did.
//!
//! Deleting a fragment and setting its money cap are dangerous
//! (`Reserved::Dangerous`): today an agent is refused them, whomever it
//! acts for, and its owner does them. That is not the design. Paul,
//! 2026-11-21: "I want my agent to be able to delete fragments, it should
//! just pop open an approval card for that. just like it has me confirm
//! when I do it myself in the UI." The target is in `the_rule_table`'s
//! comment.

use fragment_proto::{FragmentKind, IdentityKind, Member, Role, Visibility};

/// What `decide` decides over: the fragment's visibility, what the caller
/// brings to it, and what they would do. Read once per request, by the
/// caller, from rows it already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub visibility: Visibility,
    pub standing: Standing,
    pub action: Action,
}

/// What a caller would do, and the weakest role that may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Read: a page, a file, a query, a channel's records. An agent's
    /// owner reads what the agent reads (rule 2).
    Read(Role),
    /// Act: a mutation, a job, a post. It takes a standing of one's own.
    Act(Role),
    /// Its repo, through a storage token: its whole git history, not only
    /// main's files. It is read first, as `Read`, then decided by
    /// memberships alone (rule 6).
    Repo(Role),
}

impl Action {
    fn needs(self) -> Role {
        match self {
            Action::Read(role) | Action::Act(role) | Action::Repo(role) => role,
        }
    }
}

/// What a caller brings to a fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing {
    /// Their own membership.
    pub member: Option<Role>,
    /// They own an agent that is a member.
    pub owns_member_agent: bool,
    /// They hold the share link.
    pub link: bool,
    /// The request is signed (or has a session).
    pub signed: bool,
    /// An agent acts for the caller this standing describes (its asker):
    /// what caps it. `None`: the caller acts as themselves.
    pub cap: Option<Cap>,
    /// The caller is an agent held below its owner (decision 36): the most
    /// it acts with wherever a hold applies (rule 4).
    pub held: Option<Role>,
    /// The caller is an agent acting as itself, and its person is a member
    /// here who is not the owner (decision 64: the whole entity is
    /// secured), so its hold caps its own membership too.
    pub beside_person: bool,
    /// The fragment is a connector: its owner's alone to edit (rule 5).
    pub connector: bool,
    /// The caller is the fragment's owner, a person signing as themselves.
    pub owner_in_person: bool,
}

impl Standing {
    /// A role decided earlier and kept, as a membership is held: a live
    /// socket's, from when it connected (it reopens when its role changes).
    /// On a `members` fragment nothing else lends it more.
    pub fn kept(role: Role) -> Standing {
        Standing { member: Some(role), signed: true, ..Standing::default() }
    }
}

/// What an agent acting for someone brings to a fragment (decision R17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cap {
    /// The agent's own membership.
    pub agent: Option<Role>,
    /// Its owner's membership.
    pub owner: Option<Role>,
}

/// `decide`'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Verdict {
    /// Allowed, with the role the caller acts with.
    Allow(Role),
    /// Allowed once whom it names says yes.
    Ask(Ask),
    /// Refused, and why.
    Refuse(Refusal),
}

/// Whom a verdict asks, and for what. No rule of today's asks: the grant,
/// lend and device rules will, each with a variant of its own. Until then
/// `Verdict::Ask` cannot be made, and a caller's `match ask {}` proves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {}

/// Why `decide` refused, as one refusal says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    /// The weakest role that may.
    pub needs: Role,
    pub why: Why,
    /// Anonymous and short of the role: signing in could help (401).
    /// Otherwise the caller is signed and still short of it (403).
    pub sign_in: bool,
}

/// What a refused caller lacks, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// An agent acting for someone, short of what its cap allows (rule 3).
    Capped,
    /// They read it through their agent's membership, and acting needs a
    /// standing of their own (rule 2).
    ThroughAgent,
    /// A `link` fragment, opened without its share link.
    NoLink,
    /// A `members` fragment, and they are not one.
    MembersOnly,
    /// Their role is below the one it needs.
    Short,
    /// Its repo is its members' alone, and they read it otherwise (rule 6).
    RepoIsMembers,
}

impl Refusal {
    /// How the caller is told.
    pub fn message(self) -> String {
        let needs = self.needs.as_str();
        let why = match self.why {
            Why::Capped => format!("this needs the {needs} role: an agent acts with the lower of its asker's role and its own (or its owner's), at most editor"),
            Why::ThroughAgent => "you read this fragment through your agent's membership; acting here needs your own".to_string(),
            Why::NoLink => "this fragment is shared by link: open it with its share link (?view=)".to_string(),
            Why::MembersOnly => "this fragment is for its members only".to_string(),
            Why::Short => format!("this needs the {needs} role"),
            Why::RepoIsMembers => "a storage token reads the repo's whole history, so it is its members' alone: reading it through your agent's membership lends none".to_string(),
        };
        match self.sign_in {
            true => format!("{why}; sign in or sign the request"),
            false => why,
        }
    }
}

/// Whether the caller may do `facts.action`, with the role they act with
/// or why not: the rules of the module doc, in their order.
pub fn decide(facts: &Facts) -> Verdict {
    let Facts { visibility, standing, action } = *facts;
    let needs = action.needs();
    let purpose = match action {
        Action::Read(_) | Action::Repo(_) => Purpose::Read,
        Action::Act(_) => Purpose::Act,
    };
    let role = match effective_role(visibility, standing, purpose) {
        Some(role) if role >= needs => role,
        _ => return Verdict::Refuse(Refusal { needs, why: short(facts, purpose), sign_in: !standing.signed }),
    };
    let Action::Repo(_) = action else { return Verdict::Allow(role) };
    // what a visibility floor, a share link or an agent of theirs that is a
    // member gave the read above, the repo never does
    let members = Standing { link: false, owns_member_agent: false, ..standing };
    match effective_role(Visibility::Members, members, Purpose::Read) {
        Some(repo) if repo >= needs => {
            assert!(repo <= role, "memberships alone lend no more than the read");
            Verdict::Allow(repo)
        }
        _ => Verdict::Refuse(Refusal { needs, why: Why::RepoIsMembers, sign_in: false }),
    }
}

/// What a caller short of the role lacks, most specific first.
fn short(facts: &Facts, purpose: Purpose) -> Why {
    let Facts { visibility, standing, action } = *facts;
    if standing.cap.is_some() {
        return Why::Capped;
    }
    if standing.owns_member_agent && purpose == Purpose::Act {
        return Why::ThroughAgent;
    }
    match (action.needs(), visibility) {
        (Role::Public, Visibility::Link) => Why::NoLink,
        (Role::Public, Visibility::Members) => Why::MembersOnly,
        _ => Why::Short,
    }
}

/// Whether the caller reads or acts: an owner's view through their agent
/// counts only for reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Read,
    Act,
}

/// The role a caller acts with, or `None` when they cannot see the fragment.
/// An agent acting for them (`standing.cap`) acts with the lower of that
/// and its cap, and of its hold: a hold limits what an agent does with its
/// owner's access (decision 36; Paul, 2026-10-05), never its own
/// memberships, so an agent held at viewer still answers in the chats it is
/// a member of, recording its turns there.
fn effective_role(visibility: Visibility, standing: Standing, purpose: Purpose) -> Option<Role> {
    // a hold is set below its owner (registry.rs), never at the `public`
    // floor: a held agent's floor is the floor anyone has
    assert!(standing.held.is_none_or(|h| h > Role::Public && h <= AGENT_ROLE_MAX), "a hold is viewer or editor");
    let own = own_role(visibility, standing, purpose);
    // `None` is below every role: nothing on either side is nothing
    let role = match standing.cap {
        None => own,
        Some(cap) => own.min(cap_role(visibility, cap, standing.link)),
    };
    // rule 4: with its owner's access, and as itself where its person is a
    // guest; never its own memberships elsewhere (its chats, its own agent
    // fragment), where it answers and records its turns
    let held = match (standing.cap, standing.beside_person) {
        (Some(_), _) | (None, true) => standing.held,
        (None, false) => None,
    };
    let role = match held {
        Some(held) => role.map(|r| r.min(held)),
        None => role,
    };
    // rule 5: everyone but its owner, in person, is at most a viewer there
    match standing.connector && !standing.owner_in_person {
        true => role.map(|r| r.min(Role::Viewer)),
        false => role,
    }
}

fn own_role(visibility: Visibility, standing: Standing, purpose: Purpose) -> Option<Role> {
    if standing.member.is_some() {
        return standing.member;
    }
    let floor = match visibility {
        Visibility::Members => None,
        Visibility::Link => standing.link.then_some(Role::Viewer),
        Visibility::Public => Some(if standing.link { Role::Viewer } else { Role::Public }),
    };
    if purpose == Purpose::Read && standing.owns_member_agent {
        return floor.max(Some(Role::Viewer));
    }
    floor
}

/// The role a cap allows: the agent's own (its membership, or the floor
/// anyone has) or its owner's membership, whichever is higher, and never
/// above `AGENT_ROLE_MAX`.
fn cap_role(visibility: Visibility, cap: Cap, link: bool) -> Option<Role> {
    let agent = own_role(visibility, Standing { member: cap.agent, link, signed: true, ..Standing::default() }, Purpose::Act);
    agent.max(cap.owner).map(|r| r.min(AGENT_ROLE_MAX))
}

/// The role an agent acting for someone holds in a fragment it lists for
/// them, from the memberships alone (the asker's, its own, and its
/// owner's); `None` leaves the fragment out. A call decides again, with
/// the fragment's visibility.
pub fn listed_role(asker: Option<Role>, cap: Cap) -> Option<Role> {
    let standing = Standing { member: asker, signed: true, cap: Some(cap), ..Standing::default() };
    // every role is at least `public`: what it acts with at all lists it
    match decide(&Facts { visibility: Visibility::Members, standing, action: Action::Act(Role::Public) }) {
        Verdict::Allow(role) => Some(role),
        Verdict::Ask(ask) => match ask {},
        Verdict::Refuse(_) => None,
    }
}

/// The highest role an agent ever acts with: owning is never its. Sharing
/// for its owner (`agent_shares`) is its owner's authority lent on the
/// sharing routes alone, not a role it holds anywhere else.
pub const AGENT_ROLE_MAX: Role = Role::Editor;
// an agent's role never reaches the role sharing needs: only the lent
// authority shares, and only where `agent_shares` lends it
const _: () = assert!((AGENT_ROLE_MAX as u8) < (Role::Owner as u8));

/// What its owner is told as they invite someone to a chat an agent of
/// theirs is in: whoever is in a chat can talk to the agents in it (an
/// agent hears only chats: the bridge follows chats, never an app or a
/// doc), and an agent acts for its owner whoever asks it. Paul, 2026-10-23:
/// "this is the design. The solution is a warning when inviting someone to
/// your chat"; 2026-10-24: only on a chat, and short. The share sheet shows
/// it by the invite, the CLI prints it after one, and the apps skill has an
/// agent pass it on.
pub const TRUST_WARNING: &str = "People in this chat can talk to your agent.";

/// Whether whoever a fragment's owner lets in can talk to an agent of
/// theirs: it is a chat, and one of the owner's agents is in it (a chat's
/// own agent is its owner's). Where it is, inviting someone warns
/// (`TRUST_WARNING`); an app or a doc an agent of theirs is in never does,
/// as no agent hears it.
pub fn talks_to_owners_agent(kind: FragmentKind, members: &[Member]) -> bool {
    if kind != FragmentKind::Chat {
        return false;
    }
    let Some(owner) = members.iter().find(|m| m.role == Role::Owner) else { return false };
    members.iter().any(|m| m.kind == Some(IdentityKind::Agent) && m.owner.as_deref() == Some(owner.principal.as_str()))
}

/// What a control request needs beyond a role (`reserved`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reserved {
    /// Sharing: adding, changing, or removing members (but not leaving),
    /// invites, visibility, rotating links, and claiming an app address. The owner's, and their
    /// own agent's acting for them (`agent_shares`; Paul, 2026-10-04).
    Sharing,
    /// Dangerous: deleting the fragment, and its money cap
    /// (docs/ledger.md). Today the owner's own, refused to any agent
    /// whomever it acts for; the target asks the owner instead, each time
    /// (`the_rule_table`).
    Dangerous,
}

/// What a control API request (`method`, and its path under
/// `/api/f/<name>`) reserves, or `None` when a role alone decides it. A
/// member leaving (`DELETE members/me`) reserves nothing. The router asks
/// this of its undecoded path; the fragment's handlers decide again on
/// their own, so a path spelled otherwise (`%69nvites`) gains nothing.
pub fn reserved(method: &str, rest: &[&str]) -> Option<Reserved> {
    match (method, rest) {
        ("DELETE", [] | [""]) => Some(Reserved::Dangerous),
        // a fragment's cap is its owner's money (docs/ledger.md)
        ("PUT", ["cap"]) => Some(Reserved::Dangerous),
        ("PUT", ["address"]) => Some(Reserved::Sharing),
        ("PUT", ["members", _]) => Some(Reserved::Sharing),
        ("DELETE", ["members", "me"]) => None,
        ("DELETE", ["members", _]) => Some(Reserved::Sharing),
        (_, ["invites", ..]) => Some(Reserved::Sharing),
        ("PUT", ["visibility"]) => Some(Reserved::Sharing),
        ("POST", ["rotate"]) => Some(Reserved::Sharing),
        _ => None,
    }
}

/// An agent on a reserved route, as who it is says: what the router knows
/// of it from the registry, before any fragment is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sharer {
    /// It acts for its own owner (`for=<its owner>`): not as itself, and
    /// not for anyone else.
    pub for_owner: bool,
    /// Its owner holds it below them (decision 36): the most it acts with.
    pub held: Option<Role>,
}

impl Sharer {
    /// An agent as who it is says (`owner`, whom it names in `for`, its
    /// hold): it acts for its own owner when it names them. The router
    /// honors no other `for` (`may_act_for`).
    pub fn of(owner: Option<&str>, named: Option<&str>, held: Option<Role>) -> Sharer {
        Sharer { for_owner: owner.is_some() && named == owner, held }
    }
}

/// Why an agent's request on a reserved route carries no owner's
/// authority (403).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareRefusal {
    /// Deleting the fragment, or setting its cap: dangerous, and today its
    /// owner's own.
    Dangerous,
    /// It acts as itself, or for someone other than its owner.
    NotForOwner,
    /// Its owner holds it below them, and sharing is above every hold.
    Held,
    /// Its owner does not own the fragment: their own role there caps it,
    /// and only an owner shares.
    NotOwner,
}

impl ShareRefusal {
    pub fn message(self) -> &'static str {
        match self {
            ShareRefusal::Dangerous => "an agent never deletes a fragment or sets its cap: its owner does",
            ShareRefusal::NotForOwner => "an agent shares only acting for its own owner (`for=<its owner>`), never as itself or for anyone else",
            ShareRefusal::Held => "an agent its owner holds below them shares nothing",
            ShareRefusal::NotOwner => "an agent shares only what its owner owns",
        }
    }
}

/// The half of `agent_shares` that who the agent is decides (the router
/// asks it): whether its request on a reserved route may go on to the
/// fragment, which decides the rest from its owner's share there.
pub fn agent_may_ask(reserved: Reserved, sharer: Sharer) -> Result<(), ShareRefusal> {
    match reserved {
        Reserved::Dangerous => Err(ShareRefusal::Dangerous),
        Reserved::Sharing => {
            if !sharer.for_owner {
                return Err(ShareRefusal::NotForOwner);
            }
            match sharer.held {
                // a hold is at most `AGENT_ROLE_MAX`, below the owner's role
                // sharing needs: a held agent shares nothing above its hold
                Some(held) if held < Role::Owner => Err(ShareRefusal::Held),
                Some(_) | None => Ok(()),
            }
        }
    }
}

/// Whether an agent shares a fragment with its owner's authority (Paul,
/// 2026-10-04: "your agent can share on your behalf"), and the role it
/// shares with: `owner`, when it acts for its own owner, its owner holds it
/// at nothing below them, and its owner (whose membership is `owner`) owns
/// the fragment. The role rules still apply to what it does
/// (`refuse_set_role`, `refuse_remove`): it never makes anyone owner.
/// Anywhere else its owner's own role caps it, as `decide` has it.
pub fn agent_shares(sharer: Sharer, owner: Option<Role>) -> Result<Role, ShareRefusal> {
    agent_may_ask(Reserved::Sharing, sharer)?;
    if owner != Some(Role::Owner) {
        return Err(ShareRefusal::NotOwner);
    }
    Ok(Role::Owner)
}

/// Why a request may not name `for` whom it names (403).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForRefusal {
    /// A person, or an agent with no owner: it acts as itself.
    NotAnAgent,
    /// An agent naming someone other than its own owner.
    NotItsOwner,
}

impl ForRefusal {
    pub fn message(self) -> &'static str {
        match self {
            ForRefusal::NotAnAgent => "only an agent acts for someone (`for`); a person acts as themselves",
            ForRefusal::NotItsOwner => "an agent acts only for its own owner (`for=<its owner>`)",
        }
    }
}

/// Whom a request signed by an identity of `kind`, owned by `owner`, may
/// name in `for`: an agent its own owner, and no one else. An app is told
/// the named identity as `call.principal` and keys "who did it" on it
/// (cli/GUIDE.md, "Who did it"), and nothing binds a request to a person
/// who asked but its owner's computer setting `FRAGMENT_FOR` to them, so
/// naming anyone else would let any agent's key speak as that person
/// wherever the role floor reaches (a public doc's notes). Every caller
/// the platform ships names the agent's owner (the bridge, Hermes, the
/// CLI's `FRAGMENT_FOR`).
pub fn may_act_for(kind: IdentityKind, owner: Option<&str>, named: &str) -> Result<(), ForRefusal> {
    match (kind, owner) {
        (IdentityKind::Agent, Some(owner)) if owner == named => Ok(()),
        (IdentityKind::Agent, Some(_)) => Err(ForRefusal::NotItsOwner),
        _ => Err(ForRefusal::NotAnAgent),
    }
}

/// Why a membership change is refused, or `None` when it is allowed. Only
/// the owner manages members (their own agent sharing for them acts with
/// their role: `agent_shares`); the owner's own row is never changed here
/// (there is no owner transfer yet); `public` is a floor, not a grant.
pub fn refuse_set_role(actor: Option<Role>, target_current: Option<Role>, new_role: Role) -> Option<&'static str> {
    if actor != Some(Role::Owner) {
        return Some("only the owner manages members");
    }
    if target_current == Some(Role::Owner) {
        return Some("the owner's role cannot be changed");
    }
    match new_role {
        Role::Viewer | Role::Editor => None,
        Role::Owner => Some("a fragment has one owner; ownership transfer is not supported"),
        Role::Public => Some("`public` is what everyone gets on a public fragment; it cannot be granted"),
    }
}

/// Why removing `target` is refused, or `None`. The owner removes anyone
/// but themselves; any other member may leave.
pub fn refuse_remove(actor: Option<Role>, actor_is_target: bool, target_current: Option<Role>) -> Option<&'static str> {
    match target_current {
        None => Some("not a member"),
        Some(Role::Owner) => Some("the owner cannot be removed"),
        Some(_) if actor_is_target || actor == Some(Role::Owner) => None,
        Some(_) => Some("only the owner removes other members"),
    }
}

/// The agents an owner's access view names on one of their fragments
/// (docs/cloudflare-v1.md, decision 64: a person's agents in with them are
/// never named to the owner). Its members read (`entity`: the agents in
/// with a person there), its listed agents less those; its members unread
/// (past the view's bound, or not answering) while someone else may be in
/// it (`others`), none, since one of theirs may be among them; with no one
/// else in it, its listed agents, the owner's own.
pub fn access_agents(listed: Vec<String>, entity: Option<&[String]>, others: bool) -> Vec<String> {
    match (entity, others) {
        (Some(entity), _) => listed.into_iter().filter(|a| !entity.contains(a)).collect(),
        (None, true) => vec![],
        (None, false) => listed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Role::*;
    use Visibility as V;

    fn st(member: Option<Role>, link: bool, signed: bool) -> Standing {
        Standing { member, link, signed, ..Standing::default() }
    }

    fn owner_of_agent() -> Standing {
        Standing { owns_member_agent: true, signed: true, ..Standing::default() }
    }

    /// An agent acting for someone whose membership is `asker`, beside its
    /// own membership and its owner's.
    fn acting(asker: Option<Role>, agent: Option<Role>, owner: Option<Role>) -> Standing {
        Standing { member: asker, signed: true, cap: Some(Cap { agent, owner }), ..Standing::default() }
    }

    fn verdict(visibility: Visibility, standing: Standing, action: Action) -> Verdict {
        decide(&Facts { visibility, standing, action })
    }

    /// `decide`'s role, or `None` when it refuses.
    fn allowed(visibility: Visibility, standing: Standing, action: Action) -> Option<Role> {
        match verdict(visibility, standing, action) {
            Verdict::Allow(role) => Some(role),
            Verdict::Ask(ask) => match ask {},
            Verdict::Refuse(_) => None,
        }
    }

    fn refused(needs: Role, why: Why, sign_in: bool) -> Verdict {
        Verdict::Refuse(Refusal { needs, why, sign_in })
    }

    /// Goal: today's rules as one table, each row the facts a request
    /// reads and the verdict, reason included, that the cell answered
    /// before the rules moved here (no behavior change). Method: every
    /// kind of caller (anonymous, a link holder, a member at each role, an
    /// agent's owner, an agent acting for its owner, held or not) on every
    /// visibility, reading, acting and asking for the repo; each row is
    /// decided twice, as a retried request would be, and answers the same.
    ///
    /// The target (FIN-615, "Who acts for whom: One Right Way", with
    /// Paul's answers of 2026-11-21), so that no later reader takes
    /// today's refusals for the design:
    ///
    /// - **Refuse** only an unsigned or unproven ask; an answer to a
    ///   question that did not ask whoever answers (an agent never answers
    ///   its own card); and a page touching a secret.
    /// - **Dangerous** (deleting, a cap, a transfer, managing keys, an
    ///   agent changing its own grants) asks for a confirm card instead,
    ///   of whoever would click Confirm in the UI: the thing's owner. It is
    ///   asked every time, like the UI's own confirm, and never remembered
    ///   as "always". Today it refuses an agent (`Reserved::Dangerous`), as
    ///   master does; the flip lands with the one ask (PR 10).
    /// - **What should obviously work, works** (Paul: "let's make sure
    ///   things that should obviously work do work"): `the_obvious_cases`.
    /// - **A secret lives with whoever it belongs to** (Paul, 2026-11-21:
    ///   "yes I agree B"). A person's (their API keys, bot tokens, Nostr
    ///   key) stays in their Vault, and their agents and their own apps use
    ///   it in place. An app's own (a webhook's signing key, a team app's
    ///   shared key) stays with the app, set by its editors. Both stores
    ///   stay; only the move between them goes.
    #[test]
    fn the_rule_table() {
        use Action::{Act, Read, Repo};
        let anon = st(None, false, false);
        let link = st(None, true, false);
        let signed = st(None, false, true);
        let member = |role| st(Some(role), false, true);
        let held = |held, s: Standing| Standing { held: Some(held), ..s };
        let beside = |s: Standing| Standing { beside_person: true, ..s };
        let connector = |s: Standing| Standing { connector: true, ..s };
        // an agent for its owner where it alone is in (its owner reads through it)
        let alone = |agent| Standing { owns_member_agent: true, ..acting(None, Some(agent), None) };
        let rows: &[(&str, Visibility, Standing, Action, Verdict)] = &[
            // rule 1: a caller's own role
            ("anonymous reads a public page", V::Public, anon, Read(Public), Verdict::Allow(Public)),
            ("anonymous, short of viewer on a public fragment", V::Public, anon, Read(Viewer), refused(Viewer, Why::Short, true)),
            ("signed, short of viewer on a public fragment", V::Public, signed, Read(Viewer), refused(Viewer, Why::Short, false)),
            ("a link holder reads a link fragment as a viewer", V::Link, link, Read(Viewer), Verdict::Allow(Viewer)),
            ("a link holder on a public fragment is a viewer", V::Public, link, Read(Public), Verdict::Allow(Viewer)),
            ("a link holder is no editor", V::Link, link, Act(Editor), refused(Editor, Why::Short, true)),
            ("a link fragment without its link", V::Link, anon, Read(Public), refused(Public, Why::NoLink, true)),
            ("a link fragment without its link, signed", V::Link, signed, Read(Public), refused(Public, Why::NoLink, false)),
            ("a members fragment, to a non-member", V::Members, signed, Read(Public), refused(Public, Why::MembersOnly, false)),
            ("a members fragment, its link no help", V::Members, link, Read(Public), refused(Public, Why::MembersOnly, true)),
            ("a members fragment, anonymous and short of viewer", V::Members, anon, Read(Viewer), refused(Viewer, Why::Short, true)),
            ("a viewer reads", V::Members, member(Viewer), Read(Viewer), Verdict::Allow(Viewer)),
            ("a viewer does not edit", V::Members, member(Viewer), Act(Editor), refused(Editor, Why::Short, false)),
            ("an editor edits", V::Members, member(Editor), Act(Editor), Verdict::Allow(Editor)),
            ("an editor's membership beats the floor", V::Public, member(Editor), Read(Public), Verdict::Allow(Editor)),
            ("the owner deletes", V::Members, member(Owner), Act(Owner), Verdict::Allow(Owner)),
            // rule 2: an agent's owner reads what the agent reads, and never acts
            ("an agent's owner reads as a viewer", V::Members, owner_of_agent(), Read(Viewer), Verdict::Allow(Viewer)),
            ("an agent's owner reads no more", V::Members, owner_of_agent(), Read(Editor), refused(Editor, Why::Short, false)),
            ("an agent's owner acts with nothing it lends", V::Members, owner_of_agent(), Act(Viewer), refused(Viewer, Why::ThroughAgent, false)),
            ("an agent's owner acts with the public floor", V::Public, owner_of_agent(), Act(Public), Verdict::Allow(Public)),
            // rule 3: an agent acting for its owner, capped
            ("for its owner, who owns it: editor", V::Members, acting(Some(Owner), None, Some(Owner)), Act(Editor), Verdict::Allow(Editor)),
            ("for its owner, never owner", V::Members, acting(Some(Owner), Some(Owner), Some(Owner)), Act(Owner), refused(Owner, Why::Capped, false)),
            ("for an asker neither it nor its owner is in with", V::Members, acting(Some(Owner), None, None), Read(Viewer), refused(Viewer, Why::Capped, false)),
            ("its own viewer membership caps it", V::Members, acting(Some(Editor), Some(Viewer), None), Act(Editor), refused(Editor, Why::Capped, false)),
            ("its owner's viewer share caps it", V::Members, acting(Some(Editor), None, Some(Viewer)), Read(Editor), refused(Editor, Why::Capped, false)),
            ("the public floor, for anyone", V::Public, acting(None, None, None), Read(Public), Verdict::Allow(Public)),
            ("the link is not the agent's to hold", V::Link, acting(None, None, None), Read(Public), refused(Public, Why::Capped, false)),
            // rule 3's floor: for its owner, never less than its own membership
            ("for its owner, where it alone is in: its own membership", V::Members, alone(Editor), Act(Editor), Verdict::Allow(Editor)),
            ("for its owner, its own membership short of the role", V::Members, alone(Viewer), Act(Editor), refused(Editor, Why::Capped, false)),
            ("for its owner, its own membership never owner", V::Members, alone(Owner), Act(Owner), refused(Owner, Why::Capped, false)),
            ("for its owner, its own membership at most editor", V::Members, alone(Owner), Act(Editor), Verdict::Allow(Editor)),
            ("for its owner on a link fragment, its own membership with no link", V::Link, alone(Editor), Act(Editor), Verdict::Allow(Editor)),
            ("for its owner, held: its own membership elsewhere stands", V::Members, held(Viewer, alone(Editor)), Act(Editor), Verdict::Allow(Editor)),
            ("for its owner beside its guest person, held: its own membership held", V::Members, beside(held(Viewer, acting(Some(Viewer), Some(Editor), Some(Viewer)))), Act(Editor), refused(Editor, Why::Capped, false)),
            ("for its owner beside its guest person, held: it reads", V::Members, beside(held(Viewer, acting(Some(Viewer), Some(Editor), Some(Viewer)))), Read(Viewer), Verdict::Allow(Viewer)),
            ("for its owner on a connector: its own membership a viewer's", V::Members, connector(alone(Editor)), Act(Editor), refused(Editor, Why::Capped, false)),
            ("for its owner, its own membership lends the repo", V::Members, alone(Editor), Repo(Editor), Verdict::Allow(Editor)),
            // rule 4: a hold
            ("held at viewer, it reads", V::Members, held(Viewer, acting(Some(Editor), None, Some(Editor))), Read(Viewer), Verdict::Allow(Viewer)),
            ("held at viewer, it does not edit", V::Members, held(Viewer, acting(Some(Editor), None, Some(Editor))), Act(Editor), refused(Editor, Why::Capped, false)),
            ("a hold never limits its own memberships", V::Members, held(Viewer, member(Editor)), Act(Editor), Verdict::Allow(Editor)),
            ("a hold never raises", V::Members, held(Editor, acting(Some(Viewer), None, Some(Editor))), Act(Editor), refused(Editor, Why::Capped, false)),
            ("as itself beside its guest person, its hold caps it", V::Members, beside(held(Viewer, member(Editor))), Act(Editor), refused(Editor, Why::Short, false)),
            ("as itself beside its guest person, it reads", V::Members, beside(held(Viewer, member(Editor))), Read(Viewer), Verdict::Allow(Viewer)),
            ("beside its person, unheld, its membership stands", V::Members, beside(member(Editor)), Act(Editor), Verdict::Allow(Editor)),
            ("beside its person, the floor stays the floor", V::Public, beside(held(Viewer, signed)), Read(Public), Verdict::Allow(Public)),
            // rule 5: a connector is its owner's alone to edit
            ("a connector's editor is a viewer there", V::Members, connector(member(Editor)), Act(Editor), refused(Editor, Why::Short, false)),
            ("a connector's editor reads", V::Members, connector(member(Editor)), Read(Viewer), Verdict::Allow(Viewer)),
            ("a connector's owner, in person, deletes", V::Members, Standing { owner_in_person: true, ..connector(member(Owner)) }, Act(Owner), Verdict::Allow(Owner)),
            ("a connector's owner as their own agent is a viewer", V::Members, connector(acting(Some(Owner), None, Some(Owner))), Act(Editor), refused(Editor, Why::Capped, false)),
            ("a connector's owner's agent reads", V::Members, connector(acting(Some(Owner), None, Some(Owner))), Read(Viewer), Verdict::Allow(Viewer)),
            ("a connector, by its link", V::Link, connector(link), Read(Viewer), Verdict::Allow(Viewer)),
            // a role kept from earlier (a socket's)
            ("a kept role reaches what it holds", V::Members, Standing::kept(Editor), Read(Editor), Verdict::Allow(Editor)),
            ("a kept role is short of more", V::Members, Standing::kept(Viewer), Read(Editor), refused(Editor, Why::Short, false)),
            // rule 6: the repo is its members'
            ("a member reads the repo", V::Members, member(Viewer), Repo(Viewer), Verdict::Allow(Viewer)),
            ("an editor writes the repo", V::Members, member(Editor), Repo(Editor), Verdict::Allow(Editor)),
            ("a viewer does not write the repo", V::Members, member(Viewer), Repo(Editor), refused(Editor, Why::Short, false)),
            ("a link holder reads a link fragment, not its repo", V::Link, st(None, true, true), Repo(Viewer), refused(Viewer, Why::RepoIsMembers, false)),
            ("a link holder not signed in is told to sign in", V::Link, link, Repo(Editor), refused(Editor, Why::Short, true)),
            ("a public fragment's floor is below the repo's", V::Public, signed, Repo(Viewer), refused(Viewer, Why::Short, false)),
            ("an agent's owner reads, but not its repo", V::Members, owner_of_agent(), Repo(Viewer), refused(Viewer, Why::RepoIsMembers, false)),
            ("for its owner who owns it: the repo at most editor", V::Members, acting(Some(Owner), None, Some(Owner)), Repo(Editor), Verdict::Allow(Editor)),
            ("held at viewer: the repo's viewer", V::Members, held(Viewer, acting(Some(Editor), None, Some(Editor))), Repo(Viewer), Verdict::Allow(Viewer)),
        ];
        for &(row, visibility, standing, action, want) in rows {
            let facts = Facts { visibility, standing, action };
            assert_eq!(decide(&facts), want, "{row}");
            assert_eq!(decide(&facts), decide(&facts), "{row}: decided again, the same");
        }
    }

    /// Goal: the two rules the cell applied itself, by narrowing the
    /// standing it read before deciding (an agent's hold beside its guest
    /// person, decision 64; a connector's viewer ceiling), moved here
    /// unchanged. Method: every standing on every visibility for every
    /// action, decided once as the cell did (`as_the_cell_did`, the old
    /// code's narrowing, then `decide` over the narrowed standing with no
    /// facts) and once from the facts; the two agree, verdict and reason.
    #[test]
    fn the_cells_own_rules_moved_in_unchanged() {
        /// fragment.rs `standing`'s hold and doors.rs `only_owner_edits`, as
        /// they read before the rules moved here.
        fn as_the_cell_did(s: Standing) -> Standing {
            let mut t = Standing { beside_person: false, connector: false, owner_in_person: false, ..s };
            if s.cap.is_none() {
                // as itself the hold was never passed on: it narrowed the membership, beside its person
                t.held = None;
                if let (true, Some(held)) = (s.beside_person, s.held) {
                    t.member = t.member.map(|m| m.min(held));
                }
            }
            if s.connector && !s.owner_in_person {
                t.member = t.member.map(|m| m.min(Viewer));
                if t.cap.is_some() {
                    t.held = Some(t.held.map_or(Viewer, |h| h.min(Viewer)));
                }
            }
            t
        }
        let roles = [None, Some(Public), Some(Viewer), Some(Editor), Some(Owner)];
        let mut caps = vec![None];
        for agent in roles {
            for owner in roles {
                caps.push(Some(Cap { agent, owner }));
            }
        }
        let mut actions = vec![];
        for role in [Public, Viewer, Editor, Owner] {
            actions.extend([Action::Read(role), Action::Act(role), Action::Repo(role)]);
        }
        let mut n: u64 = 0;
        for visibility in [V::Members, V::Link, V::Public] {
            for member in roles {
                for cap in &caps {
                    for held in [None, Some(Viewer), Some(Editor)] {
                        for bits in 0u8..32 {
                            let flag = |i: u8| bits & (1 << i) != 0;
                            let standing = Standing { member, owns_member_agent: flag(0), link: flag(1), signed: flag(2), cap: *cap, held, beside_person: flag(3), connector: flag(4), owner_in_person: false };
                            for standing in [standing, Standing { owner_in_person: true, ..standing }] {
                                for &action in &actions {
                                    let new = decide(&Facts { visibility, standing, action });
                                    let old = decide(&Facts { visibility, standing: as_the_cell_did(standing), action });
                                    assert_eq!(new, old, "{visibility:?} {standing:?} {action:?}");
                                    n += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(n, 3 * 5 * 26 * 3 * 32 * 2 * 12, "every combination, once");
    }

    /// Goal: what should obviously work works, and stays working (Paul,
    /// 2026-11-21: "something that should OBVIOUSLY work is if ben is
    /// talking to my agent and he asks it to do something with a fragment
    /// he's shared as an editor on"). Every later change keeps this table
    /// green. Method: each case as the facts its request reads today (the
    /// agent's terminal signs `for` its owner, Ana, whoever asked), its
    /// verdict today, and the one it must have. A row whose two differ is
    /// a known gap, named in `GAPS`: fixing it moves it out of there, and
    /// a change that breaks any other row fails here. Then whom else rule
    /// 3's floor reaches (an agent for its own owner never acts with less
    /// than its own membership), each with its verdict: nobody past what
    /// the same agent holds signing as itself.
    #[test]
    fn the_obvious_cases() {
        use Action::{Act, Read};
        // Juniper is Ana's agent, Fred her second; Ben is in Ana's shared
        // chat; the facts as the cell reads them (fragment.rs `standing`)
        let for_ana = |ana: Option<Role>, juniper: Option<Role>| Standing {
            member: ana,
            // her agent is in where she is not: she reads through it
            owns_member_agent: ana.is_none() && juniper.is_some(),
            link: false,
            signed: true,
            cap: Some(Cap { agent: juniper, owner: ana }),
            // she is in here, and not its owner
            beside_person: ana.is_some_and(|r| r != Owner),
            ..Standing::default()
        };
        let held = |s: Standing| Standing { held: Some(Viewer), ..s };
        // Ben's fragment, Juniper an editor of its own and Ana not in it: a
        // membership from before decision 64 (today an agent comes in only
        // with its person)
        let bens = Facts { visibility: V::Members, standing: for_ana(None, Some(Editor)), action: Act(Editor) };
        const GAPS: &[&str] = &[];
        let rows: &[(&str, Facts, Verdict, Verdict)] = &[
            ("Ben asks Juniper to edit the fragment he shared with it as an editor", bens, Verdict::Allow(Editor), Verdict::Allow(Editor)),
            (
                "Ben asks Juniper to edit a fragment he invited Ana to as an editor",
                // her yes brought her agents in beside her, at her role (decisions 63, 64)
                Facts { visibility: V::Members, standing: for_ana(Some(Editor), Some(Editor)), action: Act(Editor) },
                Verdict::Allow(Editor),
                Verdict::Allow(Editor),
            ),
            (
                "Ana asks Juniper to edit an app she can edit",
                Facts { visibility: V::Members, standing: for_ana(Some(Editor), None), action: Act(Editor) },
                Verdict::Allow(Editor),
                Verdict::Allow(Editor),
            ),
            (
                "Ana asks Juniper to edit an app she owns",
                Facts { visibility: V::Members, standing: for_ana(Some(Owner), None), action: Act(Editor) },
                Verdict::Allow(Editor),
                Verdict::Allow(Editor),
            ),
            (
                "Ben asks Juniper about what everyone in the chat can see",
                // Ana's chat: she owns it, Juniper is in it with her
                Facts { visibility: V::Members, standing: for_ana(Some(Owner), Some(Editor)), action: Read(Viewer) },
                Verdict::Allow(Editor),
                Verdict::Allow(Editor),
            ),
            (
                "Juniper asks Fred (fragment ask) to edit an app Ana can edit",
                Facts { visibility: V::Members, standing: for_ana(Some(Editor), None), action: Act(Editor) },
                Verdict::Allow(Editor),
                Verdict::Allow(Editor),
            ),
            (
                "Juniper asks Fred (fragment ask) to read a doc Ana views",
                Facts { visibility: V::Members, standing: for_ana(Some(Viewer), None), action: Read(Viewer) },
                Verdict::Allow(Viewer),
                Verdict::Allow(Viewer),
            ),
            // whom else the floor reaches
            (
                "Carol, in Ana's chat and not in Ben's fragment, asks Juniper to edit it",
                // the same request as Ben's: until a mandate says who asked
                // (PR 6), whoever talks to Juniper reaches what it does, as
                // with all of Ana's own access today (`TRUST_WARNING`); then
                // the place's lend asks Ana (PR 9)
                bens,
                Verdict::Allow(Editor),
                Verdict::Allow(Editor),
            ),
            (
                "Ana holds Juniper at viewer, and Ben asks it to edit the fragment he shared with it",
                // a hold limits what it does with Ana's access, never its own
                // memberships (decision 36), as signing as itself
                Facts { standing: held(bens.standing), ..bens },
                Verdict::Allow(Editor),
                Verdict::Allow(Editor),
            ),
            (
                "Juniper is in beside Ana, a viewer, and held: for her it never edits",
                // its guest person's hold secures the whole entity (decision 64)
                Facts { visibility: V::Members, standing: held(for_ana(Some(Viewer), Some(Editor))), action: Act(Editor) },
                refused(Editor, Why::Capped, false),
                refused(Editor, Why::Capped, false),
            ),
            (
                "Ana, signed in herself, still only reads Ben's fragment through Juniper",
                Facts { visibility: V::Members, standing: Standing { owns_member_agent: true, signed: true, ..Standing::default() }, action: Act(Editor) },
                refused(Editor, Why::ThroughAgent, false),
                refused(Editor, Why::ThroughAgent, false),
            ),
            ("Juniper, for Ana, never deletes Ben's fragment", Facts { action: Act(Owner), ..bens }, refused(Owner, Why::Capped, false), refused(Owner, Why::Capped, false)),
        ];
        for &(case, facts, today, target) in rows {
            assert_eq!(decide(&facts), today, "{case}: today");
            assert_eq!(today == target, !GAPS.contains(&case), "{case}: a gap is named in GAPS, and only a gap");
        }
        assert!(GAPS.iter().all(|gap| rows.iter().any(|(case, ..)| case == gap)), "every gap is a row");
        // and what Ana could do herself through Fred includes sharing what she owns
        let for_ana = Sharer { for_owner: true, held: None };
        assert_eq!(agent_shares(for_ana, Some(Owner)), Ok(Owner), "Fred shares Ana's app for her");
        // and neither shares Ben's fragment, Ana's role there being none, nor deletes it or sets its cap
        assert_eq!(agent_shares(for_ana, None), Err(ShareRefusal::NotOwner), "Juniper shares nothing of Ben's for Ana");
        assert_eq!(agent_may_ask(Reserved::Dangerous, for_ana), Err(ShareRefusal::Dangerous), "nor deletes it or sets its cap");
    }

    /// Goal: each reason says what the cell said before the rules moved
    /// here, word for word (the e2e lanes and the CLI read these). Method:
    /// one refusal of each kind, signed and not.
    #[test]
    fn refusals_say_what_the_cell_said() {
        let say = |needs, why, sign_in| Refusal { needs, why, sign_in }.message();
        assert_eq!(say(Editor, Why::Capped, false), "this needs the editor role: an agent acts with the lower of its asker's role and its own (or its owner's), at most editor");
        assert_eq!(say(Viewer, Why::ThroughAgent, false), "you read this fragment through your agent's membership; acting here needs your own");
        assert_eq!(say(Public, Why::NoLink, true), "this fragment is shared by link: open it with its share link (?view=); sign in or sign the request");
        assert_eq!(say(Public, Why::MembersOnly, false), "this fragment is for its members only");
        assert_eq!(say(Viewer, Why::Short, true), "this needs the viewer role; sign in or sign the request");
        assert_eq!(say(Editor, Why::Short, false), "this needs the editor role");
        assert!(say(Viewer, Why::RepoIsMembers, false).starts_with("a storage token reads the repo's whole history"));
    }

    /// Goal: the access view never names a guest's agent, read or not
    /// (decision 64). Method: each way a row is drawn: its members read; its
    /// members unread with others possible (the view's 65th fragment, a
    /// fragment that did not answer); and no one else in it.
    #[test]
    fn the_access_view_names_no_agent_in_with_its_person() {
        let listed = || vec!["npub1mine".to_string(), "npub1guests".to_string()];
        let entity = ["npub1guests".to_string()];
        assert_eq!(access_agents(listed(), Some(&entity), true), vec!["npub1mine".to_string()], "read: the guest's agent goes, the owner's stays");
        assert_eq!(access_agents(listed(), None, true), Vec::<String>::new(), "unread, past the bound or failed, with others possible: none named");
        assert_eq!(access_agents(vec!["npub1mine".to_string()], None, false), vec!["npub1mine".to_string()], "no one else in it: its agents are its owner's");
    }

    /// Goal: inviting warns where the invitee can talk to the owner's
    /// agent, and only there: a chat it is in (Paul, 2026-10-23 and 10-24).
    /// Method: a chat of a person's alone; with someone else's agent in it;
    /// with the owner's agent in it (a chat's lead is one); an app and a doc
    /// (an app too) with the owner's agent in it.
    #[test]
    fn inviting_warns_where_the_owners_agent_is_in_a_chat() {
        use FragmentKind::{App, Chat};
        let m = |principal: &str, role: Role, owner: Option<&str>| Member {
            principal: principal.into(),
            role,
            added_by: "npub1paul".into(),
            added_at: 0,
            kind: Some(if owner.is_some() { IdentityKind::Agent } else { IdentityKind::Person }),
            owner: owner.map(str::to_string),
            email: None,
            door: None,
        };
        let alone = vec![m("npub1paul", Owner, None), m("npub1bea", Editor, None)];
        assert!(!talks_to_owners_agent(Chat, &alone), "no agent in it: nothing to warn of");
        let theirs = [alone.clone(), vec![m("npub1beasagent", Editor, Some("npub1bea"))]].concat();
        assert!(!talks_to_owners_agent(Chat, &theirs), "someone else's agent is not the owner's");
        let mine = [alone, vec![m("npub1paulsagent", Editor, Some("npub1paul"))]].concat();
        assert!(talks_to_owners_agent(Chat, &mine), "the owner's agent is in the chat: whoever comes in can talk to it");
        assert!(!talks_to_owners_agent(App, &mine), "an app or a doc the owner's agent is in: no agent hears it, nothing to warn of");
        assert!(!talks_to_owners_agent(Chat, &[]), "no members read: no owner to warn");
    }

    /// Goal: `for` names only the agent's own owner, so no key speaks as
    /// someone who never asked it (an app trusts `call.principal`).
    /// Method: valid, an agent naming its owner; invalid, an agent naming
    /// another person, another agent, or its own id; a person naming
    /// anyone, themselves too; an agent with no owner.
    #[test]
    fn an_agent_names_only_its_own_owner_in_for() {
        use IdentityKind::{Agent, Person};
        assert_eq!(may_act_for(Agent, Some("npub1paul"), "npub1paul"), Ok(()), "its owner");
        for named in ["npub1skyler", "npub1juniper", "npub1hand"] {
            assert_eq!(may_act_for(Agent, Some("npub1paul"), named), Err(ForRefusal::NotItsOwner), "{named}: not its owner");
        }
        assert_eq!(may_act_for(Person, None, "npub1paul"), Err(ForRefusal::NotAnAgent), "a person acts as themselves");
        assert_eq!(may_act_for(Person, Some("npub1paul"), "npub1paul"), Err(ForRefusal::NotAnAgent), "even with an owner field, a person names no one");
        assert_eq!(may_act_for(Agent, None, "npub1paul"), Err(ForRefusal::NotAnAgent), "an agent with no owner acts as itself");
        assert!(ForRefusal::NotItsOwner.message().contains("its own owner"), "the sharing lanes read this phrase");
    }

    /// Goal: decision 36's example and its limit. Skyler's agent edits
    /// what Paul shared with Skyler as an editor, acting for Skyler (a
    /// share is Skyler's and their agents': Paul, 2026-10-13); held at
    /// viewer it only reads with whatever it reaches for whomever it acts,
    /// and keeps its own memberships.
    #[test]
    fn delegation_and_its_limits() {
        let d = |s, needs| allowed(V::Members, s, Action::Act(needs));
        let skylers_agent = acting(Some(Editor), None, Some(Editor));
        assert_eq!(d(skylers_agent, Editor), Some(Editor));
        // Skyler's viewer share lends it viewer, for Skyler, and no more
        let skylers_viewer = acting(Some(Viewer), None, Some(Viewer));
        assert_eq!(d(skylers_viewer, Viewer), Some(Viewer));
        assert_eq!(d(skylers_viewer, Editor), None);
        let held = Standing { held: Some(Viewer), ..skylers_agent };
        assert_eq!(d(held, Editor), None);
        assert_eq!(d(held, Viewer), Some(Viewer));
        // a hold never limits what it is a member of itself (its chats)
        let as_itself = Standing { member: Some(Editor), held: Some(Viewer), ..st(None, false, true) };
        assert_eq!(d(as_itself, Editor), Some(Editor));
        // a hold never raises anything
        let raised = Standing { held: Some(Editor), ..acting(Some(Viewer), None, Some(Editor)) };
        assert_eq!(d(raised, Editor), None);
    }

    #[test]
    fn roles_from_visibility() {
        let r = |v, s| allowed(v, s, Action::Read(Public));
        assert_eq!(r(V::Members, st(None, true, false)), None);
        assert_eq!(r(V::Members, st(Some(Viewer), false, false)), Some(Viewer));
        assert_eq!(r(V::Link, st(None, false, false)), None);
        assert_eq!(r(V::Link, st(None, true, false)), Some(Viewer));
        assert_eq!(r(V::Public, st(None, false, false)), Some(Public));
        assert_eq!(r(V::Public, st(None, true, false)), Some(Viewer));
        assert_eq!(r(V::Public, st(Some(Editor), false, false)), Some(Editor));
    }

    /// Goal: a fragment's repo (its history, through a storage token) is
    /// its members' alone, on every visibility. Method: each standing that
    /// reads a fragment without being in it (a signed-in visitor of a
    /// public one, a link holder, a person whose agent is in, an agent
    /// acting for any of them) holds no repo role where it holds a viewer's
    /// or a public role; a member holds theirs; an agent acting for a
    /// member holds the lower of theirs and its cap, at most its hold.
    #[test]
    fn the_repo_is_its_members_whatever_its_visibility() {
        let repo = |v, s| allowed(v, s, Action::Repo(Viewer));
        // what a visibility floor gives (a link holder reads as a viewer, a
        // visitor of a public one as public), the repo never does
        assert_eq!(allowed(V::Link, st(None, true, true), Action::Read(Viewer)), Some(Viewer));
        assert_eq!(allowed(V::Public, st(None, false, true), Action::Read(Public)), Some(Public));
        for v in [V::Members, V::Link, V::Public] {
            assert_eq!(repo(v, st(None, false, true)), None, "a signed-in non-member, on {v:?}");
            assert_eq!(repo(v, st(None, true, true)), None, "a link holder, on {v:?}");
            assert_eq!(repo(v, st(None, true, false)), None, "a link holder not signed in, on {v:?}");
            assert_eq!(repo(v, owner_of_agent()), None, "a person whose agent is in, on {v:?}");
            assert_eq!(repo(v, acting(None, Some(Viewer), Some(Editor))), None, "an agent acting for a non-member, its own and its owner's roles notwithstanding");
            assert_eq!(repo(v, Standing { link: true, ..acting(None, Some(Viewer), Some(Editor)) }), None, "nor with a link");
            assert_eq!(repo(v, Standing { owns_member_agent: true, ..acting(None, None, Some(Viewer)) }), None, "nor for a person whose agent is in");
            for role in [Viewer, Editor, Owner] {
                assert_eq!(repo(v, st(Some(role), false, true)), Some(role), "a member holds their own role");
            }
            assert_eq!(repo(v, acting(Some(Viewer), None, Some(Viewer))), Some(Viewer), "an agent acting for its viewer owner: viewer");
            assert_eq!(repo(v, acting(Some(Owner), None, Some(Owner))), Some(Editor), "for its owner who owns it: at most editor");
            assert_eq!(repo(v, acting(Some(Editor), None, None)), None, "for an editor, with neither it nor its owner in: nothing (R17's cap)");
            assert_eq!(repo(v, Standing { held: Some(Viewer), ..acting(Some(Editor), None, Some(Editor)) }), Some(Viewer), "held at viewer: viewer");
        }
    }

    #[test]
    fn an_agent_acts_for_its_asker_capped() {
        let d = |v, s, needs| allowed(v, s, Action::Act(needs));
        // the owner's turn on an app the owner made, which the agent is not in
        assert_eq!(d(V::Members, acting(Some(Owner), None, Some(Owner)), Editor), Some(Editor));
        // a guest who is in the owner's chat only: the owner's app is out of reach
        assert_eq!(d(V::Members, acting(None, None, Some(Owner)), Viewer), None);
        assert_eq!(d(V::Link, acting(None, Some(Editor), Some(Owner)), Public), None);
        // what the owner shared with the guest: the guest's own role
        assert_eq!(d(V::Members, acting(Some(Editor), None, Some(Owner)), Editor), Some(Editor));
        assert_eq!(d(V::Members, acting(Some(Viewer), None, Some(Owner)), Editor), None);
        // what neither the agent nor its owner is in: nothing, whoever asks
        assert_eq!(d(V::Members, acting(Some(Owner), None, None), Viewer), None);
        // the agent's own membership caps it
        assert_eq!(d(V::Members, acting(Some(Editor), Some(Viewer), None), Editor), None);
        assert_eq!(d(V::Members, acting(Some(Editor), Some(Viewer), None), Viewer), Some(Viewer));
        // never above editor: owning is never an agent's (sharing for its owner is `agent_shares`)
        assert_eq!(d(V::Members, acting(Some(Owner), Some(Owner), Some(Owner)), Owner), None);
        assert_eq!(d(V::Members, acting(Some(Owner), Some(Owner), Some(Owner)), Public), Some(Editor));
        // never further than its owner: an owner who only views caps a guest who edits
        assert_eq!(d(V::Members, acting(Some(Editor), None, Some(Viewer)), Public), Some(Viewer));
        // the floor anyone has: a public fragment, for anyone
        assert_eq!(d(V::Public, acting(None, None, None), Public), Some(Public));
        assert_eq!(d(V::Public, acting(None, None, None), Viewer), None);
        // the link is not the agent's to hold for anyone
        assert_eq!(allowed(V::Link, acting(None, None, None), Action::Read(Public)), None);
        // an asker who reads through their own agent's membership reads, and never acts
        let owner_reads = Standing { owns_member_agent: true, ..acting(None, Some(Editor), None) };
        assert_eq!(allowed(V::Members, owner_reads, Action::Read(Viewer)), Some(Viewer));
        assert_eq!(allowed(V::Members, owner_reads, Action::Act(Viewer)), None);
    }

    #[test]
    fn listing_for_an_asker() {
        let cap = |agent, owner| Cap { agent, owner };
        assert_eq!(listed_role(Some(Owner), cap(None, Some(Owner))), Some(Editor));
        assert_eq!(listed_role(Some(Viewer), cap(Some(Editor), None)), Some(Viewer));
        assert_eq!(listed_role(Some(Editor), cap(None, None)), None);
        assert_eq!(listed_role(None, cap(Some(Editor), Some(Owner))), None);
    }

    #[test]
    fn reserved_routes() {
        for (method, rest) in [("DELETE", &[][..]), ("DELETE", &[""][..]), ("PUT", &["cap"][..])] {
            assert_eq!(reserved(method, rest), Some(Reserved::Dangerous), "{method} {rest:?}");
        }
        for (method, rest) in [
            ("PUT", &["address"][..]),
            ("PUT", &["members", "id:00"][..]),
            ("DELETE", &["members", "npub1x"][..]),
            ("POST", &["invites"][..]),
            ("GET", &["invites"][..]),
            ("DELETE", &["invites", "ann@example.com"][..]),
            ("PUT", &["visibility"][..]),
            ("POST", &["rotate"][..]),
        ] {
            assert_eq!(reserved(method, rest), Some(Reserved::Sharing), "{method} {rest:?}");
        }
        for (method, rest) in [
            ("DELETE", &["members", "me"][..]),
            ("GET", &["members"][..]),
            ("GET", &["status"][..]),
            ("POST", &["ops", "add"][..]),
            ("POST", &["files"][..]),
            ("POST", &["deploy"][..]),
            ("PUT", &["secrets", "K"][..]),
            ("GET", &["cap"][..]),
            ("GET", &["visibility"][..]),
        ] {
            assert_eq!(reserved(method, rest), None, "{method} {rest:?}");
        }
    }

    /// Goal: Paul's decision of 2026-10-04 ("your agent can share on your
    /// behalf") and its limits. Method: the rule's two halves over each
    /// case: the router's (`agent_may_ask`, from who the agent is) and the
    /// fragment's (`agent_shares`, from its owner's share there), then the
    /// role rules over what the lent authority does.
    #[test]
    fn an_agent_shares_for_its_owner() {
        let for_owner = Sharer { for_owner: true, held: None };
        let owns = Some(Owner);
        // valid: the owner's own agent shares the owner's fragment, as its owner
        assert_eq!(agent_may_ask(Reserved::Sharing, for_owner), Ok(()));
        assert_eq!(agent_shares(for_owner, owns), Ok(Owner));
        // ...under the role rules: it adds, changes, and removes members
        assert_eq!(refuse_set_role(Some(Owner), None, Viewer), None);
        assert_eq!(refuse_set_role(Some(Owner), Some(Viewer), Editor), None);
        assert_eq!(refuse_remove(Some(Owner), false, Some(Editor)), None);
        // ...and never makes anyone owner, itself included, nor changes or removes the owner
        assert!(refuse_set_role(Some(Owner), Some(Editor), Owner).is_some());
        assert!(refuse_set_role(Some(Owner), None, Public).is_some());
        assert!(refuse_set_role(Some(Owner), Some(Owner), Viewer).is_some());
        assert!(refuse_remove(Some(Owner), false, Some(Owner)).is_some());

        // invalid: as itself, or for anyone but its owner (another
        // person's agent naming you, or your agent naming someone else)
        let not_for_owner = Sharer { for_owner: false, held: None };
        assert_eq!(agent_may_ask(Reserved::Sharing, not_for_owner), Err(ShareRefusal::NotForOwner));
        assert_eq!(agent_shares(not_for_owner, owns), Err(ShareRefusal::NotForOwner));
        // another person's agent for its own owner, on what that person only edits
        let editor = Some(Editor);
        assert_eq!(agent_shares(for_owner, editor), Err(ShareRefusal::NotOwner));
        // a fragment its owner merely edits, views, or is not in
        for role in [Some(Editor), Some(Viewer), Some(Public), None] {
            assert_eq!(agent_shares(for_owner, role), Err(ShareRefusal::NotOwner), "{role:?}");
        }
        // held below its owner, at any hold: it shares nothing above its hold
        for held in [Viewer, Editor] {
            let held = Sharer { for_owner: true, held: Some(held) };
            assert_eq!(agent_may_ask(Reserved::Sharing, held), Err(ShareRefusal::Held));
            assert_eq!(agent_shares(held, owns), Err(ShareRefusal::Held));
        }
        // deleting and the cap are dangerous: refused to an agent today, for whomever it acts
        for sharer in [for_owner, not_for_owner, Sharer { for_owner: true, held: Some(Viewer) }] {
            for (method, rest) in [("DELETE", &[][..]), ("PUT", &["cap"][..])] {
                let route = reserved(method, rest).expect("a reserved route");
                assert_eq!(agent_may_ask(route, sharer), Err(ShareRefusal::Dangerous), "{method} {rest:?}");
            }
        }
        // and the lent authority is no role: acting for its owner it is
        // still an editor, so what needs owner (deleting) is refused there too
        let acting_for_owner = acting(Some(Owner), None, Some(Owner));
        assert_eq!(allowed(V::Members, acting_for_owner, Action::Act(Owner)), None);

        // replay: the same member PUT twice decides the same, the second
        // finding the role the first granted
        assert_eq!(agent_shares(for_owner, owns), agent_shares(for_owner, owns));
        let first = refuse_set_role(Some(Owner), None, Viewer);
        let again = refuse_set_role(Some(Owner), Some(Viewer), Viewer);
        assert_eq!((first, again), (None, None));
    }

    #[test]
    fn membership_changes() {
        assert!(refuse_set_role(Some(Owner), None, Editor).is_none());
        assert!(refuse_set_role(Some(Owner), Some(Viewer), Editor).is_none());
        assert!(refuse_set_role(Some(Editor), None, Viewer).is_some());
        assert!(refuse_set_role(Some(Owner), Some(Owner), Viewer).is_some());
        assert!(refuse_set_role(Some(Owner), None, Owner).is_some());
        assert!(refuse_set_role(Some(Owner), None, Public).is_some());
        assert!(refuse_remove(Some(Owner), false, Some(Editor)).is_none());
        assert!(refuse_remove(Some(Viewer), true, Some(Viewer)).is_none());
        assert!(refuse_remove(Some(Editor), false, Some(Viewer)).is_some());
        assert!(refuse_remove(Some(Owner), true, Some(Owner)).is_some());
        assert!(refuse_remove(Some(Owner), false, None).is_some());
    }
}
