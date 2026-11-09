//! What a computer's owner is told of it, in words (FIN-458): each notice
//! `notices` works out, as the `Notice` every client draws the same way,
//! with its actions' routes. Short and calm, and a warning only where one
//! is literally true.

use fragment_proto::computer::ComputerNotice;
use fragment_proto::notice::{Notice, NoticeAction, NoticeLevel, Until};
use serde_json::{Map, Value};

use crate::notice::{provider_name, size, span, usd};

/// What the words need of the computer beyond its notices.
pub struct Facts<'a> {
    pub computer: &'a str,
    /// The start its owner sees: what a Restart names (`RestartAsk`).
    pub generation: u64,
    pub always_on: bool,
    /// Where its owner gets help when it won't start (`support_url_ok`).
    pub support: Option<&'a str>,
    pub now_ms: i64,
}

/// `told` (`notices`, in its order), in words.
pub fn said(told: &[ComputerNotice], f: &Facts) -> Vec<Notice> {
    told.iter().map(|n| one(n, f)).collect()
}

/// An action's body: `{key: value}`.
fn body(key: &str, value: impl Into<Value>) -> Value {
    let mut m = Map::new();
    m.insert(key.to_string(), value.into());
    Value::Object(m)
}

fn notice(id: String, kind: &str, level: NoticeLevel, title: String) -> Notice {
    Notice { id, kind: kind.into(), level, title, text: None, detail: None, until: None, actions: vec![] }
}

fn one(n: &ComputerNotice, f: &Facts) -> Notice {
    let at = |path: &str| format!("/api/computers/{}/{path}", f.computer);
    let restart = |label: &str| NoticeAction::post(label, at("restart"), body("generation", f.generation));
    match n {
        ComputerNotice::WontWake { why } => Notice {
            text: Some(format!("{} Your agents can't answer until it does.", wont_start(why))),
            detail: Some(format!("{}: {why}", f.computer)),
            actions: [Some(restart("Restart")), f.support.map(|url| NoticeAction::open("Get help", url))].into_iter().flatten().collect(),
            ..notice("wont_wake".into(), "wont_wake", NoticeLevel::Warn, "Your computer won't start".into())
        },
        ComputerNotice::Unsaved { since, why, stops_at, .. } => Notice {
            text: Some(format!("It hasn't saved for {}, so what its agents did since isn't kept yet. It keeps trying, and Restart tries to save first.", span(f.now_ms - since))),
            detail: Some(why.clone()),
            until: stops_at.map(|at| Until { at, said: "If it still can't save, it stops".into() }),
            actions: vec![restart("Restart now")],
            ..notice("unsaved".into(), "unsaved", NoticeLevel::Warn, "Your computer can't save right now".into())
        },
        ComputerNotice::WentBack { life, pending, save, saved_at, ended_at, .. } => {
            // what went: the span between the save it goes back to and the
            // loss, which stays true however long the notice waits
            let lost = match (save, saved_at, ended_at) {
                (None, _, _) => None,
                (Some(_), Some(saved), Some(ended)) => Some(format!("made in the {} before it stopped", span(ended - saved))),
                (Some(_), _, _) => Some("on it since its last save".to_string()),
            };
            let said = match (lost, pending) {
                (Some(lost), true) => format!("When it starts again, changes {lost} won't be there."),
                (Some(lost), false) => format!("Changes {lost} weren't kept."),
                (None, true) => "Nothing on it was saved yet, so it will start fresh.".into(),
                (None, false) => "Nothing on it was saved yet, so it started fresh.".into(),
            };
            let title = if *pending { "Your computer stopped" } else { "Your computer restarted" };
            Notice {
                text: Some(format!("{said} Your chats are all here.")),
                actions: vec![NoticeAction::post("OK", at("notices/seen"), body("life", *life))],
                ..notice(format!("went_back:{life}"), "went_back", NoticeLevel::Info, title.into())
            }
        }
        ComputerNotice::Update { version } => Notice {
            // never restarted behind their agents' backs; an always-on
            // computer never sleeps, so never updates by itself
            text: Some(format!("Restart to get it; your chats and work are kept.{}", if f.always_on { "" } else { " It also updates by itself the next time your computer sleeps." })),
            actions: vec![restart("Restart"), NoticeAction::post("Later", at("notices/later"), body("version", version.as_str()))],
            ..notice(format!("update:{version}"), "update", NoticeLevel::Info, "An update is ready for your computer".into())
        },
        ComputerNotice::DiskFilling { free, .. } => Notice {
            text: Some("Delete files you don't need, or ask an agent to tidy up.".into()),
            ..notice("disk_filling".into(), "disk_filling", NoticeLevel::Warn, format!("Your computer is almost full ({} left)", size(*free)))
        },
        ComputerNotice::KeyPaused { provider, spent_micros, cap_micros, allow_micros, resumes_at } => Notice {
            text: allow_micros.is_none().then(|| format!("{} an hour is the most allowed.", usd(*cap_micros))),
            until: Some(Until { at: *resumes_at, said: "Calls resume".into() }),
            // one tap lifts its cap a step from the one shown (a stale page changes nothing)
            actions: allow_micros.map(|allow| NoticeAction::post(format!("Allow up to {} an hour", usd(allow)), at(&format!("key-caps/{provider}")), body("from", *cap_micros))).into_iter().collect(),
            ..notice(format!("key_paused:{provider}"), "key_paused", NoticeLevel::Warn, format!("{} calls paused: {} this hour", provider_name(provider), usd(*spent_micros)))
        },
    }
}

/// Why a computer won't start, in plain words; the platform's own stay in
/// the notice's detail, for whoever helps. No time it may not keep: a big
/// restore takes a start up to 15 minutes (#1001).
fn wont_start(why: &str) -> &'static str {
    let lower = why.to_ascii_lowercase();
    if why.contains("not ready within") {
        "It didn't finish starting in time."
    } else if why.contains("kept stopping") {
        "It kept stopping as soon as it started."
    } else if why.contains("check") {
        "Its saved files didn't pass their check."
    } else if ["restore", "archive", "backup"].iter().any(|w| lower.contains(w)) {
        "Its save couldn't be restored."
    } else {
        "It couldn't start."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::computer::LossCause;
    use serde_json::json;

    const MIN: i64 = 60_000;

    fn facts() -> Facts<'static> {
        Facts { computer: "computer:0123456789abcdef01234567", generation: 7, always_on: false, support: Some("mailto:help@example.com"), now_ms: 100 * MIN }
    }

    fn post(n: &Notice) -> Vec<(String, String, serde_json::Value)> {
        n.actions.iter().map(|a| (a.label.clone(), a.post.clone().unwrap_or_default(), a.body.clone().unwrap_or_default())).collect()
    }

    /// Goal: each kind says what is so, calmly, and carries the route that
    /// answers it, so a client draws it with no code of its own.
    #[test]
    fn each_kind_in_words_with_its_routes() {
        let f = facts();
        let at = "/api/computers/computer:0123456789abcdef01234567";
        let wont = &said(&[ComputerNotice::WontWake { why: "the container was not ready within 600 s".into() }], &f)[0];
        assert_eq!((wont.level, wont.title.as_str()), (NoticeLevel::Warn, "Your computer won't start"));
        assert_eq!(wont.text.as_deref(), Some("It didn't finish starting in time. Your agents can't answer until it does."));
        assert_eq!(wont.detail.as_deref(), Some("computer:0123456789abcdef01234567: the container was not ready within 600 s"));
        assert_eq!(post(wont)[0], ("Restart".into(), format!("{at}/restart"), json!({ "generation": 7 })));
        assert_eq!(wont.actions[1].open.as_deref(), Some("mailto:help@example.com"));
        let no_help = said(&[ComputerNotice::WontWake { why: "x".into() }], &Facts { support: None, ..facts() });
        assert_eq!(no_help[0].actions.len(), 1, "no support link, no Get help");

        let unsaved = &said(&[ComputerNotice::Unsaved { since: 88 * MIN, save: Some(3), failing_since: 90 * MIN, failures: 3, why: "R2 is down".into(), stops_at: Some(130 * MIN) }], &f)[0];
        assert_eq!(unsaved.text.as_deref(), Some("It hasn't saved for 12 minutes, so what its agents did since isn't kept yet. It keeps trying, and Restart tries to save first."));
        assert_eq!(unsaved.until, Some(Until { at: 130 * MIN, said: "If it still can't save, it stops".into() }));
        assert_eq!(post(unsaved), vec![("Restart now".into(), format!("{at}/restart"), json!({ "generation": 7 }))]);

        let back = |pending, save: Option<u64>, ended_at: Option<i64>| {
            said(&[ComputerNotice::WentBack { life: 6, cause: LossCause::Crash, ended_at, pending, save, saved_at: save.map(|_| 20 * MIN), at: None }], &f).remove(0)
        };
        let told = back(false, Some(2), Some(60 * MIN));
        assert_eq!((told.level, told.title.as_str(), told.id.as_str()), (NoticeLevel::Info, "Your computer restarted", "went_back:6"));
        assert_eq!(told.text.as_deref(), Some("Changes made in the 40 minutes before it stopped weren't kept. Your chats are all here."));
        assert_eq!(post(&told), vec![("OK".into(), format!("{at}/notices/seen"), json!({ "life": 6 }))]);
        assert_eq!(back(true, Some(2), Some(60 * MIN)).text.as_deref(), Some("When it starts again, changes made in the 40 minutes before it stopped won't be there. Your chats are all here."));
        assert_eq!(back(true, Some(2), None).text.as_deref(), Some("When it starts again, changes on it since its last save won't be there. Your chats are all here."));
        assert_eq!(back(false, None, None).text.as_deref(), Some("Nothing on it was saved yet, so it started fresh. Your chats are all here."));
        assert_eq!(back(true, None, Some(60 * MIN)).title, "Your computer stopped");

        let update = &said(&[ComputerNotice::Update { version: "0123456789ab".into() }], &f)[0];
        assert_eq!(update.text.as_deref(), Some("Restart to get it; your chats and work are kept. It also updates by itself the next time your computer sleeps."));
        assert_eq!(post(update)[1], ("Later".into(), format!("{at}/notices/later"), json!({ "version": "0123456789ab" })));
        let always = said(&[ComputerNotice::Update { version: "0123456789ab".into() }], &Facts { always_on: true, ..facts() });
        assert_eq!(always[0].text.as_deref(), Some("Restart to get it; your chats and work are kept."), "an always-on computer never sleeps");

        let disk = &said(&[ComputerNotice::DiskFilling { total: 20_000_002_048, free: 3_240_662_016 }], &f)[0];
        assert_eq!(disk.title, "Your computer is almost full (3.2 GB left)");
        assert!(disk.actions.is_empty());

        let paused = |allow_micros| said(&[ComputerNotice::KeyPaused { provider: "elevenlabs".into(), spent_micros: 5_030_000, cap_micros: 5_000_000, allow_micros, resumes_at: 123 * MIN }], &f).remove(0);
        let one = paused(Some(20_000_000));
        assert_eq!((one.title.as_str(), one.text.as_deref()), ("ElevenLabs calls paused: $5.03 this hour", None));
        assert_eq!(one.until, Some(Until { at: 123 * MIN, said: "Calls resume".into() }));
        assert_eq!(post(&one), vec![("Allow up to $20 an hour".into(), format!("{at}/key-caps/elevenlabs"), json!({ "from": 5_000_000 }))]);
        let most = paused(None);
        assert_eq!((most.text.as_deref(), most.actions.len()), (Some("$5 an hour is the most allowed."), 0));
    }

    /// Goal: a client keys its rows by id: unique among one computer's
    /// notices, the same across reads while each tells the same thing.
    #[test]
    fn ids_are_unique_and_stable() {
        let told = vec![
            ComputerNotice::WontWake { why: "x".into() },
            ComputerNotice::Update { version: "0123456789ab".into() },
            ComputerNotice::DiskFilling { total: 2, free: 1 },
            ComputerNotice::KeyPaused { provider: "xai".into(), spent_micros: 1, cap_micros: 1, allow_micros: None, resumes_at: 1 },
            ComputerNotice::KeyPaused { provider: "elevenlabs".into(), spent_micros: 1, cap_micros: 1, allow_micros: None, resumes_at: 1 },
        ];
        let ids: Vec<String> = said(&told, &facts()).into_iter().map(|n| n.id).collect();
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");
        let later = said(&told, &Facts { now_ms: facts().now_ms + 10 * MIN, ..facts() });
        assert_eq!(later.into_iter().map(|n| n.id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn why_a_start_failed_in_plain_words() {
        assert_eq!(wont_start("the guest kept stopping"), "It kept stopping as soon as it started.");
        assert_eq!(wont_start("the image's check of the save failed"), "Its saved files didn't pass their check.");
        assert_eq!(wont_start("Restore of archive 4 failed"), "Its save couldn't be restored.");
        assert_eq!(wont_start("the image would not pull"), "It couldn't start.");
    }
}
