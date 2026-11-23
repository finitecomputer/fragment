//! The image a computer runs and the one its next start runs (`Images`):
//! their versions, the update its owner is told of, and whether its image
//! does SimpleX.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::computer::lifecycle::{Lifecycle, Phase};
use crate::computer::saves::Saves;

/// Hex digits in an image's version (`version_of`).
pub const VERSION_HEX: usize = 12;

/// An image reference's version, as its owner is shown it and names it back
/// ("Later": `Images::later`): the first `VERSION_HEX` hex digits of its
/// digest when it names one (`…@sha256:<hex>`, as a deploy pins an image to
/// what it pushed), else of the reference's own SHA-256 (a local build's
/// tag, which every image of one build shares). One image is one version
/// wherever it is pulled from; another reference, another.
pub fn version_of(reference: &str) -> String {
    let digest = reference.rsplit_once("@sha256:").map(|(_, hex)| hex).filter(|hex| hex.get(..VERSION_HEX).is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())));
    let version = match digest {
        Some(hex) => hex[..VERSION_HEX].to_ascii_lowercase(),
        None => hex::encode(Sha256::digest(reference.as_bytes()))[..VERSION_HEX].to_string(),
    };
    assert!(valid_version(&version), "a version is {VERSION_HEX} hex digits");
    version
}

/// Whether `v` has a version's shape (`version_of`'s): `VERSION_HEX`
/// lowercase hex digits.
pub fn valid_version(v: &str) -> bool {
    v.len() == VERSION_HEX && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What the Computer DO knows of the image its next start runs, beside the
/// one its running start came up on (`Saves::image_of`), for its owner's
/// `update` notice (`notices`): the reference its pinned image's name stood
/// for as it last looked (the runtime's own, which a deploy that changed the
/// image changes, as does a pin), and the version its owner put off
/// ("Later"). The DO looks at every event and every read of its view, so a
/// deploy is a change of this state at its next touch, and its owner's open
/// pages are told of it as of any other.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Images {
    /// None: the deployment has no image by its pinned name, or it never looked.
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    pub(super) later: Option<String>,
}

impl Images {
    /// It looked: its pinned image's reference now (none: the deployment
    /// has no image by that name). Answers whether that is news.
    pub fn looked(&mut self, next: Option<&str>) -> bool {
        let news = self.next.as_deref() != next;
        if news {
            self.next = next.map(str::to_string);
        }
        news
    }

    /// The reference its next start runs, as it last looked.
    pub fn next(&self) -> Option<&str> {
        self.next.as_deref()
    }

    /// The version its next start runs (its view's `latest`).
    pub fn latest(&self) -> Option<String> {
        self.next().map(version_of)
    }

    /// Its owner put off the update to `version` ("Later", as an `update`
    /// notice named it): it is told no more until its next start runs
    /// another. Only the version its next start runs can be put off: any
    /// other (an older notice's, read before a newer deploy) is refused
    /// (`false`), as is any with no image to run. Again, the same.
    pub fn later(&mut self, version: &str) -> bool {
        let next = self.latest().filter(|v| v == version);
        let put_off = next.is_some();
        if put_off {
            self.later = next;
        }
        put_off
    }
}

/// The reference of the image a computer runs now: its running start's,
/// once it came up (awake, or going to sleep); none while it runs nothing.
pub fn image_running<'a>(life: &Lifecycle, saves: &'a Saves) -> Option<&'a str> {
    match life.phase {
        Phase::Awake { generation, .. } | Phase::Sleeping { generation, .. } => saves.image_of(generation),
        Phase::Asleep { .. } | Phase::Starting { .. } | Phase::Failed { .. } => None,
    }
}

/// The version of the image a computer runs now (`image_running`'s).
pub fn version_running(life: &Lifecycle, saves: &Saves) -> Option<String> {
    image_running(life, saves).map(version_of)
}

/// The version of the image a computer is on: the one it runs now, or,
/// running none (asleep, or starting), the one its next start runs: what
/// its owner is offered by (`does_simplex`).
pub fn version_now(life: &Lifecycle, saves: &Saves, images: &Images) -> Option<String> {
    version_running(life, saves).or_else(|| images.latest())
}

/// Whether a computer's image does SimpleX (docs/bridge.md, "SimpleX"),
/// so its owner's Settings offer it: `said` is the version whose bridge
/// last said it does (`GET /api/computer?simplex=1`, taken only of the image
/// a running start came up on: `version_running`), and it must be the
/// version the computer is on now (`version_now`). An image without it
/// (production's before SimpleX, or one a rollback goes back to) never
/// said so; one deployed since is offered once its own bridge has.
pub fn does_simplex(said: Option<&str>, life: &Lifecycle, saves: &Saves, images: &Images) -> bool {
    said.is_some_and(|said| version_now(life, saves, images).as_deref() == Some(said))
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::ComputerNotice;

    use super::*;
    use crate::computer::testing::{rec, save_asked, Computer, IMAGE};
    use crate::computer::*;

    /// Goal: an image's version is short, the same for one image wherever
    /// it is pulled from (its digest), and another for another reference,
    /// local builds' tags included (stub and stub-next share one build's
    /// tag). Method: references as a deploy and `wrangler dev` pin them.
    #[test]
    fn an_image_has_one_short_version() {
        let deployed = "registry.cloudflare.com/acct/fragment-computer-hermes@sha256:9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08";
        assert_eq!(version_of(deployed), "9f86d081884c", "its digest's first twelve, lowercase");
        assert_eq!(version_of(&deployed.replace("acct", "other")), version_of(deployed), "one image, wherever it is pulled from");
        let (stub, next) = ("cloudflare-dev/computer-stub-0a1b2c3d4e5f:8c1d", "cloudflare-dev/computer-stub-next-6a7b8c9d0e1f:8c1d");
        assert_ne!(version_of(stub), version_of(next), "two images of one build's tag are two versions");
        assert_eq!(version_of(stub), version_of(stub));
        assert_ne!(version_of("x@sha256:abc"), "abc", "a digest too short is no digest: the reference's own hash");
        for v in [version_of(deployed), version_of(stub), version_of("")] {
            assert!(valid_version(&v), "{v}");
        }
        for bad in ["", "9f86d081884", "9f86d081884c0", "9F86D081884C", "9f86d081884g", " 9f86d08188c"] {
            assert!(!valid_version(bad), "{bad:?}");
        }
    }

    /// Goal: SimpleX is offered only on a computer whose image does it. An
    /// image whose bridge never said so (production's before SimpleX) is
    /// offered nothing, asleep or awake. One whose bridge said so is
    /// offered while the computer runs it, sleeps, and wakes on it again,
    /// and no longer once a deploy or a rollback moves the computer to
    /// another, until that image's own bridge says so. A word said while
    /// it starts is not taken (its next start's image may already be
    /// another: Alex's review), so a pin made during a start, then a crash
    /// before its bridge says so again, offers nothing on the pinned image.
    /// Method: one computer's history, a deploy or a pin being a look that
    /// finds another reference, each word of its bridge's taken as the DO
    /// takes it (`version_running`).
    #[test]
    fn simplex_is_offered_only_on_an_image_whose_bridge_said_so() {
        let does = |c: &Computer, said: &Option<String>| does_simplex(said.as_deref(), &c.life, &c.saves, &c.images);
        let said_now = |c: &Computer| version_running(&c.life, &c.saves);
        let next = format!("registry/hermes@sha256:{}", "2".repeat(64));
        let mut c = Computer::new();
        assert!(!does(&c, &None), "never said: nothing, before its first start");
        c.wake();
        assert!(!does(&c, &None), "awake on an image whose bridge says nothing");
        let said = said_now(&c);
        assert_eq!(said, Some(version_of(IMAGE)), "its bridge's word is of the image it runs");
        assert!(does(&c, &said));
        c.sleep(Some("b1"));
        assert!(does(&c, &said), "asleep: its next start runs the same image");
        // a deploy of another image: still offered while it runs this one
        c.wake();
        assert!(c.images.looked(Some(&next)));
        assert!(does(&c, &said), "awake on the image that said so, another deployed");
        c.sleep(Some("b2"));
        assert!(!does(&c, &said), "asleep: its next start runs one that never said so");
        c.wake();
        assert!(!does(&c, &said), "awake on the new image, until its bridge says so");
        let said = said_now(&c);
        assert_eq!(said, Some(version_of(&next)));
        assert!(does(&c, &said));
        // a rollback to the first image: what the new one said is not its
        assert!(c.images.looked(Some(IMAGE)));
        c.sleep(Some("b3"));
        assert!(!does(&c, &said));
        c.wake();
        assert!(!does(&c, &said), "rolled back, until its own bridge says so");
        assert!(!does(&c, &Some("not a version".into())));

        // a start on IMAGE (which does SimpleX), its owner pinning `next`
        // (which does not) as it starts: its bridge's word then is not taken
        c.sleep(Some("b4"));
        let s = c.apply(Event::Wake { why: Wake::Owner });
        let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("{s:?}") };
        assert!(c.images.looked(Some(&next)), "pinned while it starts");
        assert_eq!(said_now(&c), None, "starting: no word is taken");
        // it comes up on the image its start planned, and says so
        let plan = c.saves.plan(Some(IMAGE));
        let save = c.saves.current().map(|r| r.id.clone()).filter(|_| plan != Plan::Nothing);
        c.saves.starting(generation, plan.source());
        c.saves.came_up(generation, plan.source(), save.as_deref(), Some(IMAGE), c.now).expect("the start under way");
        c.apply(Event::Ready { generation });
        let said = said_now(&c);
        assert_eq!(said, Some(version_of(IMAGE)), "up: its word is of the image it came up on, not the pinned one");
        assert!(does(&c, &said));
        // it crashes before its bridge says anything more: its next start
        // runs the pinned image, which never says so
        let s = c.apply(Event::Exited { generation });
        assert!(!does(&c, &said), "its next start runs the pinned image: nothing offered");
        match s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() {
            Some(Action::Start { generation }) => {
                c.come_up(generation);
            }
            _ => {
                c.wake();
            }
        }
        assert_eq!(version_running(&c.life, &c.saves), Some(version_of(&next)));
        assert!(!does(&c, &said), "awake on the pinned image, which never said so");
    }

    /// Goal (Paul, 2026-10-09: "a notification in the shell: new update,
    /// click to restart", not a restart behind their back): its owner is
    /// told an update is ready while it is awake on another version than
    /// its next start runs, never in the middle of a turn, never while it
    /// sleeps or starts (its next start takes the new one anyway), never
    /// for the version it runs. A restart takes it and says no more; "Later"
    /// puts that version off, and a version other than it is told again.
    /// Valid, invalid and replayed looks and Laters; the same across a
    /// restart of the Durable Object. Method: one computer's history, a
    /// deploy being a look that finds another reference.
    #[test]
    fn its_owner_is_told_an_update_is_ready_while_idle() {
        let rules = Rules::default();
        let told = |c: &Computer| notices(&c.life, &c.saves, &c.images, &rules);
        let update = |c: &Computer| told(c).into_iter().find(|n| matches!(n, ComputerNotice::Update { .. }));
        let ready = |reference: &str| Some(ComputerNotice::Update { version: version_of(reference), room_first: false });
        // deploys' references, each a digest
        let at = |n: u8| format!("registry/stub@sha256:{}", format!("{n:x}").repeat(64));
        let (next, newer, newest, fifth) = (at(2), at(3), at(4), at(5));
        let (next, newer, newest, fifth) = (next.as_str(), newer.as_str(), newest.as_str(), fifth.as_str());
        let mut c = Computer::new();
        assert!(!c.images.looked(Some(IMAGE)), "the same image again is no news");
        c.wake();
        assert!(told(&c).is_empty(), "awake on the version its next start runs");
        assert_eq!(version_running(&c.life, &c.saves), c.images.latest());
        // a deploy: its next look finds another image, once
        assert!(c.images.looked(Some(next)) && !c.images.looked(Some(next)), "news once, then the same");
        assert_eq!(update(&c), ready(next));
        assert_eq!(version_running(&c.life, &c.saves), Some(version_of(IMAGE)), "it runs the one it started on");
        // never in the middle of its agents' work: a turn holds the keepalive
        c.apply(Event::Opened { socket: Socket::Keepalive });
        assert_eq!(update(&c), None, "not during a turn");
        c.apply(Event::Closed { socket: Socket::Keepalive });
        assert_eq!(update(&c), ready(next), "told again once the turn ends");
        // a port tab open is no turn: still told
        c.apply(Event::Opened { socket: Socket::Tab });
        assert_eq!(update(&c), ready(next));
        c.apply(Event::Closed { socket: Socket::Tab });
        // its sleep: not told while it sleeps or is asleep, and its wake takes it
        c.sleep(Some("b1"));
        assert!(told(&c).is_empty() && version_running(&c.life, &c.saves).is_none(), "asleep: its next start takes it anyway");
        let r = c.wake();
        assert!(told(&c).is_empty() && version_running(&c.life, &c.saves) == Some(version_of(next)), "woken on it: {r:?}");

        // a restart takes it: told no more while it sleeps, starts, or after
        assert!(c.images.looked(Some(newer)));
        assert_eq!(update(&c), ready(newer));
        let g = c.life.generation();
        c.saves.restart_fresh();
        assert_eq!(c.apply(Event::Restart { generation: g }).actions, vec![Action::Sleep { generation: g }]);
        assert_eq!(update(&c), None, "its restart's sleep is under way");
        let (_, held, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        c.saves.saved(g, rec("b2"), 0, c.now, held);
        c.apply(Event::Saved { generation: g, seq });
        let s = c.apply(Event::Asleep { generation: g });
        let s = c.owed(&s);
        let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("{s:?}") };
        assert_eq!(update(&c), None, "starting: the start under way runs the newest");
        c.come_up(generation);
        assert!(told(&c).is_empty() && version_running(&c.life, &c.saves) == Some(version_of(newer)), "restarted on it: nothing to tell");

        // Later: that version is told no more; another is
        assert!(c.images.looked(Some(newest)));
        assert_eq!(update(&c), ready(newest));
        assert!(!c.images.later("000000000000") && !c.images.later(&version_of(newer)), "only the version its next start runs is put off");
        assert_eq!(update(&c), ready(newest), "a refused Later changes nothing");
        assert!(c.images.later(&version_of(newest)) && c.images.later(&version_of(newest)), "put off, and again the same");
        assert_eq!(update(&c), None, "put off");
        c.apply(Event::Opened { socket: Socket::Keepalive });
        c.apply(Event::Closed { socket: Socket::Keepalive });
        assert_eq!(update(&c), None, "a turn's end tells it no more either");
        assert!(c.images.looked(Some(fifth)));
        assert_eq!(update(&c), ready(fifth), "a version other than the one put off is told");
        assert!(c.images.looked(Some(newest)));
        assert_eq!(update(&c), None, "the one put off, back again: still put off");
        // the same image pulled from elsewhere is no update; no image is none
        assert!(c.images.looked(Some(&newer.replace("registry/", "elsewhere/"))));
        assert_eq!(update(&c), None, "the version it runs, from another registry");
        assert!(c.images.looked(None));
        assert!(update(&c).is_none() && c.images.latest().is_none() && !c.images.later(&version_of(newer)), "the deployment has no image by its name: nothing to update to, nothing to put off");

        // always on, it is told too (it never sleeps, so it never updates by itself)
        assert!(c.images.looked(Some(next)));
        c.apply(Event::AlwaysOn { on: true });
        assert_eq!(update(&c), ready(next));
        // what it knows survives a restart of its Durable Object
        let back: Images = serde_json::from_str(&serde_json::to_string(&c.images).unwrap()).unwrap();
        assert_eq!(back, c.images);
        assert_eq!(notices(&c.life, &c.saves, &back, &rules), told(&c));
        // a record stored before it looked reads as never looked: nothing told
        let never: Images = serde_json::from_str("{}").unwrap();
        assert!(never.next().is_none() && notices(&c.life, &c.saves, &never, &rules).is_empty());
    }
}
