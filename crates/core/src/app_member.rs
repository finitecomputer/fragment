//! An app as a member of another fragment (docs/cloudflare-v1.md,
//! decision 72): a fragment, its own key unregistered, that is a member of
//! another, the target, as a viewer or an editor, never its owner. One
//! rule holds it, no laundering: an app shows no one more than they can
//! already open. It is a member of a target only while everyone who can
//! see or edit it is a member there too, and its editors hold at least its
//! role there.
//!
//! - People are counted by name (`audience`): its owner, its people, and
//!   an agent as its person (decision 64). A link or public never counts,
//!   as whoever holds one is no one by name: an app answers a target only
//!   while it is members only (`members_only`), and the target's own
//!   visibility changes nothing, as only its members count.
//! - The target is the authority. For each app member it keeps the
//!   people it approved as the app's audience, each with what they do to
//!   the app (an `Audience`), and holds them to its own members in its own
//!   transaction (`short`), so no check needs two fragments to agree at
//!   one instant:
//!   - the app joins, or its role there changes (`join`): the target asks
//!     the app once, and the audience it answers joins the list, beside
//!     every yes the target gave before (one the app may not have written
//!     yet), all held to the role it is asked at;
//!   - the app's audience would grow (`grow`): the target records the
//!     person as it says yes, before the app writes them, and forgets them
//!     (`shrink`) when the app does not, or drops them later;
//!   - the target loses or lowers a member: never refused; while its list
//!     is `short` of anyone the app is paused, and it resumes once not.
//!
//! Every failure lands on the safe side: a list holding someone the app
//! never took may pause it too early, and shows no one anything.

use fragment_proto::{IdentityKind, Member, Role, Visibility};

/// Who can see or edit an app, by name (an npub, as `Member` shows it),
/// each once, in the order counted, with what they do to it: `Viewer` sees
/// it, `Editor` edits it (its owner does). A list, not a map: it is
/// bounded by an app's members, and a map's code costs the cell's Wasm
/// more than a scan of so few.
pub type Audience = Vec<(String, Role)>;

/// What `person` does to the app, by `audience`, if they are on it.
pub fn power_of(audience: &Audience, person: &str) -> Option<Role> {
    audience.iter().find(|(p, _)| p == person).map(|(_, power)| *power)
}

/// What holding `role` in an app lets a person do to it: see it, edit it
/// (owning it, too), or nothing (the public floor, no one's by name).
fn power(role: Role) -> Option<Role> {
    match role {
        Role::Public => None,
        Role::Viewer => Some(Role::Viewer),
        Role::Editor | Role::Owner => Some(Role::Editor),
    }
}

/// `person` on `people` at the most of what they did and `role` lets them.
fn count(people: &mut Audience, person: &str, role: Role) {
    if let Some(power) = power(role) {
        match people.iter_mut().find(|(p, _)| p == person) {
            Some((_, held)) => *held = (*held).max(power),
            None => people.push((person.to_string(), power)),
        }
    }
}

/// An app's audience: its owner, each person member, and each agent
/// member as its person, at the most any of theirs does to it. An app
/// member of the app adds no one: its own audience is held to these
/// members already. A member of a kind this does not know counts as
/// itself, and as the owner it names too: the safe side.
pub fn audience(owner: &str, members: &[Member]) -> Audience {
    let mut people = Audience::new();
    count(&mut people, owner, Role::Owner);
    for m in members.iter().filter(|m| m.app.is_none()) {
        match (m.kind, &m.owner) {
            (Some(IdentityKind::Agent), Some(person)) => count(&mut people, person, m.role),
            (_, person) => {
                count(&mut people, &m.principal, m.role);
                if let Some(person) = person {
                    count(&mut people, person, m.role);
                }
            }
        }
    }
    people
}

/// The first of an app's audience other than its owner. While an app is
/// a member anywhere and its audience grows by asking no target, its
/// owner is its only person; an agent fragment, which goes wherever its
/// person goes (decision 64), would be held so always.
pub fn others<'a>(owner: &str, audience: &'a Audience) -> Option<&'a str> {
    audience.iter().map(|(p, _)| p.as_str()).find(|p| *p != owner)
}

/// The least role a person must hold in the target, by what they do to
/// the app and the app's role there: a member at all to see it, the app's
/// own role to edit it.
pub fn needs(power: Role, app_role: Role) -> Role {
    power.min(app_role).max(Role::Viewer)
}

/// One of an app's audience a target does not hold as the rule needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Short {
    pub person: String,
    /// What they do to the app: see it (`Viewer`) or edit it (`Editor`).
    pub power: Role,
    /// Their own role in the target, if they are a member there.
    pub holds: Option<Role>,
}

impl Short {
    /// What a paused app's row says: who it waits on, and why.
    pub fn paused(&self, app: &str, person: &str) -> String {
        match self.holds {
            None => format!("Paused. {person} can see {app} but is no longer here."),
            Some(_) => format!("Paused. {person} can edit {app} but only views here."),
        }
    }
}

/// The first on `approved` the target does not hold at what they need,
/// with the app at `app_role` there; `holds` answers each person's own
/// role in the target (its owner's is `Owner`; no agent of theirs lends
/// them one). `None`: the rule holds.
pub fn short(approved: &Audience, app_role: Role, holds: impl Fn(&str) -> Option<Role>) -> Option<Short> {
    approved.iter().find_map(|(person, power)| {
        let (power, held) = (*power, holds(person));
        (held < Some(needs(power, app_role))).then(|| Short { person: person.clone(), power, holds: held })
    })
}

/// Why an app does not join a target, or its audience does not grow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Its role there would be neither viewer nor editor.
    Role(Role),
    /// It is open by its link, or public: the app's own answer.
    Open(Visibility),
    /// Joining: someone who can see or edit it is not held there as the
    /// rule needs.
    Audience(Short),
    /// Growing: the person would see or edit it, and is not held there as
    /// the rule needs.
    Grows(Short),
}

impl Refusal {
    /// One line naming the fix: `app` and `target` by their titles, and
    /// the person as whoever asked may see them.
    pub fn line(&self, app: &str, target: &str, person: &str) -> String {
        match self {
            Refusal::Role(_) => format!("{app} can view or edit {target}, never own it."),
            Refusal::Open(Visibility::Public) => format!("{app} is public. Make it members only first."),
            Refusal::Open(_) => format!("{app} is shared by link. Make it members only first."),
            Refusal::Audience(Short { holds: None, .. }) => format!("{person} can see {app} but isn't in {target}. Add them here first."),
            Refusal::Audience(_) => format!("{person} can edit {app} but only view {target}."),
            Refusal::Grows(Short { holds: None, .. }) => format!("{app} can see {target}. Add {person} to {target} first."),
            Refusal::Grows(_) => format!("{app} can edit {target}. Make {person} an editor there first."),
        }
    }
}

/// The app's own part as a target asks it in: members only, as it stays
/// while it is a member anywhere.
pub fn members_only(visibility: Visibility) -> Result<(), Refusal> {
    match visibility {
        Visibility::Members => Ok(()),
        open => Err(Refusal::Open(open)),
    }
}

/// A target's answer to an app joining at `app_role`, or its role there
/// changing, given the list it `approved` before (empty at a join) and the
/// audience the app answered: the list to keep beside the app's row, or
/// why not. The answer is added to the list, never put in its place: a
/// yes the app has not written yet stays on it, held to the new role too.
/// Asked again, the same answer.
pub fn join(app_role: Role, approved: &Audience, audience: Audience, holds: impl Fn(&str) -> Option<Role>) -> Result<Audience, Refusal> {
    if !matches!(app_role, Role::Viewer | Role::Editor) {
        return Err(Refusal::Role(app_role));
    }
    let mut list = approved.clone();
    for (person, power) in audience {
        count(&mut list, &person, power);
    }
    match short(&list, app_role, holds) {
        Some(s) => Err(Refusal::Audience(s)),
        None => Ok(list),
    }
}

/// A target's answer to an app's audience growing: `person` would hold
/// `role` in the app (added, invited, made an editor, or its new owner),
/// and holds `holds` in the target. Yes records them on `approved`, before
/// the app writes them; asked again, yes again, changing nothing.
pub fn grow(approved: &mut Audience, app_role: Role, person: &str, role: Role, holds: Option<Role>) -> Result<(), Refusal> {
    let Some(power) = power(role) else { return Ok(()) };
    if holds < Some(needs(power, app_role)) {
        return Err(Refusal::Grows(Short { person: person.to_string(), power, holds }));
    }
    count(approved, person, role);
    Ok(())
}

/// The app's audience shrinking, or a growth it never wrote: `person` now
/// holds at most `left` in the app (`None`: nothing). The list never grows
/// this way.
pub fn shrink(approved: &mut Audience, person: &str, left: Option<Role>) {
    match (left.and_then(power), approved.iter().position(|(p, _)| p == person)) {
        (Some(left), Some(at)) => approved[at].1 = approved[at].1.min(left),
        (None, Some(at)) => {
            approved.remove(at);
        }
        (_, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Role::{Editor, Owner, Public, Viewer};

    const OWNER: &str = "npub1owner";
    const ROBIN: &str = "npub1robin";
    const SAM: &str = "npub1sam";

    fn member(principal: &str, role: Role, kind: Option<IdentityKind>, owner: Option<&str>) -> Member {
        Member { principal: principal.into(), role, added_by: OWNER.into(), added_at: 0, kind, owner: owner.map(str::to_string), email: None, app: None }
    }

    fn person(principal: &str, role: Role) -> Member {
        member(principal, role, Some(IdentityKind::Person), None)
    }

    fn agent(principal: &str, role: Role, of: &str) -> Member {
        member(principal, role, Some(IdentityKind::Agent), Some(of))
    }

    fn app(principal: &str, role: Role) -> Member {
        Member { app: Some(fragment_proto::MemberApp { name: "board--x".into(), paused: None }), ..member(principal, role, None, Some(OWNER)) }
    }

    /// An audience, in order of name, as `sorted` puts one.
    fn people(of: &[(&str, Role)]) -> Audience {
        sorted(of.iter().map(|(p, r)| (p.to_string(), *r)).collect())
    }

    /// An audience in order of name, to compare by who is on it.
    fn sorted(mut a: Audience) -> Audience {
        a.sort();
        a
    }

    /// A target's members: each person's own role there.
    fn target<'a>(of: &'a [(&'a str, Role)]) -> impl Fn(&str) -> Option<Role> + 'a {
        move |p| of.iter().find(|(q, _)| *q == p).map(|(_, r)| *r)
    }

    /// Goal: an app's audience is everyone who can see or edit it, by
    /// name, at the most each does. Method: an owner, a viewer, an editor,
    /// agents of both, another app, and a member of no known kind.
    #[test]
    fn the_audience_counts_people_by_name_and_agents_as_their_person() {
        let alone = audience(OWNER, &[person(OWNER, Owner)]);
        assert_eq!(alone, people(&[(OWNER, Editor)]), "its owner edits it");
        assert_eq!(others(OWNER, &alone), None, "its owner's alone");

        let members = [person(OWNER, Owner), agent("npub1ownersagent", Editor, OWNER), person(ROBIN, Viewer), agent("npub1robinsagent", Editor, ROBIN), person(SAM, Viewer)];
        assert_eq!(sorted(audience(OWNER, &members)), people(&[(OWNER, Editor), (ROBIN, Editor), (SAM, Viewer)]), "an agent counts as its person, at the most of theirs: never as itself");

        let with_app = [person(OWNER, Owner), app("npub1board", Editor)];
        assert_eq!(audience(OWNER, &with_app), people(&[(OWNER, Editor)]), "an app member adds no one: its own audience is held to these members");

        let unknown = [member("npub1what", Viewer, None, None), member("npub1owned", Editor, None, Some(ROBIN))];
        assert_eq!(sorted(audience(OWNER, &unknown)), people(&[(OWNER, Editor), ("npub1what", Viewer), ("npub1owned", Editor), (ROBIN, Editor)]), "an unknown kind counts as itself and its owner: the safe side");
        assert_eq!(others(OWNER, &audience(OWNER, &unknown)), Some("npub1what"), "anyone else is another: the first counted");

        assert_eq!(audience(OWNER, &[person(ROBIN, Public)]), people(&[(OWNER, Editor)]), "the public floor is no one by name");
    }

    /// Goal: what a person must hold in the target. Method: each of what
    /// they do to the app, against each role the app may hold there.
    #[test]
    fn seeing_needs_a_membership_and_editing_needs_the_apps_role() {
        assert_eq!(needs(Viewer, Viewer), Viewer);
        assert_eq!(needs(Viewer, Editor), Viewer, "seeing it shows what it reads: a membership is enough");
        assert_eq!(needs(Editor, Viewer), Viewer, "editing an app that only views: what it reads, still");
        assert_eq!(needs(Editor, Editor), Editor, "editing an app that edits steers its writes there");
    }

    /// Goal: a target lets an app in only while it holds everyone the app
    /// shows, and an app answers only members only. Method: valid joins at
    /// each role, then each refusal, then the same join asked again.
    #[test]
    fn an_app_joins_only_while_the_target_holds_its_whole_audience() {
        let audience = people(&[(OWNER, Editor), (ROBIN, Viewer), (SAM, Editor)]);
        let all_edit = [(OWNER, Owner), (ROBIN, Viewer), (SAM, Editor)];
        assert_eq!(join(Editor, &Audience::new(), audience.clone(), target(&all_edit)), Ok(audience.clone()), "everyone in, its editors editors: the list is its audience");
        let sam_views = [(OWNER, Owner), (ROBIN, Viewer), (SAM, Viewer)];
        assert_eq!(join(Viewer, &Audience::new(), audience.clone(), target(&sam_views)), Ok(audience.clone()), "an app that views needs only members");

        let refused = join(Editor, &Audience::new(), audience.clone(), target(&sam_views));
        assert_eq!(refused, Err(Refusal::Audience(Short { person: SAM.into(), power: Editor, holds: Some(Viewer) })), "an editor of it who only views here");
        assert_eq!(refused.unwrap_err().line("Dashboard", "Tracker", "Sam"), "Sam can edit Dashboard but only view Tracker.");

        let robin_out = [(OWNER, Owner), (SAM, Editor)];
        let refused = join(Viewer, &Audience::new(), audience.clone(), target(&robin_out));
        assert_eq!(refused, Err(Refusal::Audience(Short { person: ROBIN.into(), power: Viewer, holds: None })), "someone who sees it is not here");
        assert_eq!(refused.unwrap_err().line("Dashboard", "Tracker", "Robin"), "Robin can see Dashboard but isn't in Tracker. Add them here first.");

        for role in [Owner, Public] {
            let refused = join(role, &Audience::new(), audience.clone(), target(&all_edit));
            assert_eq!(refused, Err(Refusal::Role(role)), "an app views or edits");
            assert_eq!(refused.unwrap_err().line("Dashboard", "Tracker", ""), "Dashboard can view or edit Tracker, never own it.");
        }

        assert_eq!(members_only(Visibility::Members), Ok(()));
        assert_eq!(members_only(Visibility::Link).unwrap_err().line("Dashboard", "Tracker", ""), "Dashboard is shared by link. Make it members only first.");
        assert_eq!(members_only(Visibility::Public).unwrap_err().line("Dashboard", "Tracker", ""), "Dashboard is public. Make it members only first.");

        // a replay: the target asks again (a retry), the app answers the same
        assert_eq!(join(Editor, &Audience::new(), audience.clone(), target(&all_edit)), join(Editor, &Audience::new(), audience, target(&all_edit)));
    }

    /// Goal: an app's audience grows only through every target's yes, and
    /// a yes is recorded before the app writes. Method: valid growths, the
    /// two refusals, a replay, and a growth to an app that views.
    #[test]
    fn the_audience_grows_only_by_the_targets_yes() {
        let mut approved = people(&[(OWNER, Editor)]);
        assert_eq!(grow(&mut approved, Editor, ROBIN, Viewer, Some(Viewer)), Ok(()), "a member here may see it");
        assert_eq!(approved, people(&[(OWNER, Editor), (ROBIN, Viewer)]), "recorded as it says yes");
        let before = approved.clone();
        assert_eq!(grow(&mut approved, Editor, ROBIN, Viewer, Some(Viewer)), Ok(()), "asked again, yes again");
        assert_eq!(approved, before, "a replay changes nothing");

        let refused = grow(&mut approved, Editor, SAM, Viewer, None);
        assert_eq!(refused, Err(Refusal::Grows(Short { person: SAM.into(), power: Viewer, holds: None })));
        assert_eq!(refused.unwrap_err().line("Dashboard", "Tracker", "Sam"), "Dashboard can see Tracker. Add Sam to Tracker first.");
        let refused = grow(&mut approved, Editor, ROBIN, Editor, Some(Viewer));
        assert_eq!(refused, Err(Refusal::Grows(Short { person: ROBIN.into(), power: Editor, holds: Some(Viewer) })), "made an editor of it, a viewer here");
        assert_eq!(refused.unwrap_err().line("Dashboard", "Tracker", "Robin"), "Dashboard can edit Tracker. Make Robin an editor there first.");
        assert_eq!(approved, before, "a refusal records nothing");

        assert_eq!(grow(&mut approved, Viewer, ROBIN, Editor, Some(Viewer)), Ok(()), "an app that views: its editors need only be members");
        assert_eq!(power_of(&approved, ROBIN).expect("on the list"), Editor);
        assert_eq!(grow(&mut approved, Editor, SAM, Owner, Some(Editor)), Ok(()), "a new owner edits it");
        assert_eq!(power_of(&approved, SAM).expect("on the list"), Editor);
        assert_eq!(grow(&mut approved, Editor, "npub1anon", Public, None), Ok(()), "the public floor is no one by name");
        assert_eq!(power_of(&approved, "npub1anon"), None);
    }

    /// Goal: a shrinking audience only ever narrows the list. Method:
    /// lower, remove, a raise that must not, and a replay.
    #[test]
    fn a_shrink_never_grows_the_list() {
        let mut approved = people(&[(OWNER, Editor), (ROBIN, Editor), (SAM, Viewer)]);
        shrink(&mut approved, ROBIN, Some(Viewer));
        assert_eq!(power_of(&approved, ROBIN).expect("on the list"), Viewer, "lowered to what is left");
        shrink(&mut approved, SAM, Some(Owner));
        assert_eq!(power_of(&approved, SAM).expect("on the list"), Viewer, "never raised");
        shrink(&mut approved, SAM, None);
        assert_eq!(power_of(&approved, SAM), None, "gone from it");
        let before = approved.clone();
        shrink(&mut approved, SAM, None);
        shrink(&mut approved, "npub1never", Some(Editor));
        assert_eq!(approved, before, "a replay, or someone never on it, changes nothing");
    }

    /// Goal: the design's races, interleaved over the two fragments' turns,
    /// each land on the safe side. Method: the target's list and members
    /// as each turn leaves them, with the app's write early, late, or never.
    #[test]
    fn every_interleaving_lands_on_the_safe_side() {
        // growth against narrowing: the target says yes to Robin, then
        // loses Robin, before or after the app writes them; the target's
        // list alone decides, so where the app's write fell changes nothing
        let mut approved = people(&[(OWNER, Editor)]);
        let mut members = vec![(OWNER, Owner), (ROBIN, Viewer)];
        grow(&mut approved, Viewer, ROBIN, Viewer, target(&members)(ROBIN)).expect("yes, recorded first");
        members.retain(|(p, _)| *p != ROBIN);
        let paused = short(&approved, Viewer, target(&members)).expect("paused in the target's own turn");
        assert_eq!(paused.person, ROBIN);
        assert_eq!(paused.paused("Dashboard", "Robin"), "Paused. Robin can see Dashboard but is no longer here.");
        // the app takes Robin out, or cancels the growth it never wrote
        shrink(&mut approved, ROBIN, None);
        assert_eq!(short(&approved, Viewer, target(&members)), None, "resumed");

        // a crash between the yes and the write: the list holds one person
        // too many, who can only pause it early, until the app cancels
        let mut approved = people(&[(OWNER, Editor)]);
        let mut members = vec![(OWNER, Owner), (SAM, Editor)];
        grow(&mut approved, Editor, SAM, Editor, target(&members)(SAM)).expect("yes");
        assert_eq!(short(&approved, Editor, target(&members)), None, "nothing shown: the app never took Sam");
        members[1].1 = Viewer;
        let paused = short(&approved, Editor, target(&members)).expect("lowering Sam pauses it, early: never a leak");
        assert_eq!(paused.paused("Dashboard", "Sam"), "Paused. Sam can edit Dashboard but only views here.");
        shrink(&mut approved, SAM, None);
        assert_eq!(short(&approved, Editor, target(&members)), None, "the app's cancel on restart resumes it");

        // narrowing, then the conflict gone the other way: re-added
        let approved = people(&[(OWNER, Editor), (ROBIN, Viewer)]);
        let mut members = vec![(OWNER, Owner)];
        assert!(short(&approved, Editor, target(&members)).is_some(), "Robin removed: paused");
        members.push((ROBIN, Viewer));
        assert_eq!(short(&approved, Editor, target(&members)), None, "Robin back: resumed");

        // its role there raised: the list is held to the new role at once
        let approved = people(&[(OWNER, Editor), (SAM, Editor)]);
        let members = [(OWNER, Owner), (SAM, Viewer)];
        assert_eq!(short(&approved, Viewer, target(&members)), None);
        assert!(join(Editor, &approved, approved.clone(), target(&members)).is_err(), "raised to editor: asked again, and refused");

        // a role change against a growth the app has not written: the yes
        // to Robin stays on the list, so losing Robin still pauses it
        let mut approved = people(&[(OWNER, Editor)]);
        let mut members = vec![(OWNER, Owner), (ROBIN, Viewer)];
        grow(&mut approved, Editor, ROBIN, Viewer, target(&members)(ROBIN)).expect("yes, before the app writes Robin");
        let answered = people(&[(OWNER, Editor)]);
        let approved = join(Viewer, &approved, answered, target(&members)).expect("lowered to viewer: asked again, and yes");
        assert_eq!(approved, people(&[(OWNER, Editor), (ROBIN, Viewer)]), "the answer joins the list, never takes its place");
        members.retain(|(p, _)| *p != ROBIN);
        assert_eq!(short(&approved, Viewer, target(&members)).map(|s| s.person), Some(ROBIN.to_string()), "Robin removed: paused, though the app wrote Robin after the role changed");
    }
}
