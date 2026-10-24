//! Listed records (docs/api.md, "Listed records"): a channel that declares
//! `listed` puts each member's newest record of the kinds it names on its
//! members' rows in their lists, so a list says what the people and agents
//! in each fragment are doing (a chat's agents: working, waiting) without
//! anyone reading the channel. The fragment keeps each member's newest; the
//! index outbox that carries a row's face carries them, each row only the
//! records of the channels its role may read.
//!
//! The platform never reads what a kind means. As search reads a message
//! (`search::record_text`), it reads a record's body by the chat's
//! convention (docs/chat-records.md): its kind is the body's `kind`, a
//! string, and what a list carries of it is the body's fields the channel
//! names that are strings, each cut short.

use std::collections::BTreeMap;

use fragment_proto::{limits, ListedDecl, ListedRecord, Role};
use serde_json::Value;

/// What a list carries of a record: its kind and its listed fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub kind: String,
    pub fields: BTreeMap<String, String>,
}

/// What `decl` lists of a record's body: `None` when the body names no kind
/// it lists. A listed field that is absent or no string is left out.
pub fn shown(decl: &ListedDecl, body: &Value) -> Option<Shown> {
    assert!(decl.fields.len() <= limits::LISTED_FIELDS_MAX, "a checked manifest's listed fields are bounded");
    let kind = body.as_object()?.get("kind")?.as_str()?;
    if !decl.kinds.iter().any(|k| k == kind) {
        return None;
    }
    let mut fields = BTreeMap::new();
    for field in &decl.fields {
        if let Some(text) = body.get(field).and_then(Value::as_str) {
            fields.insert(field.clone(), cut(text));
        }
    }
    assert!(fields.values().all(|v| v.chars().count() <= limits::LISTED_VALUE_MAX_CHARS), "a listed value is cut to its bound");
    Some(Shown { kind: kind.to_string(), fields })
}

/// At most `LISTED_VALUE_MAX_CHARS` characters of `text`.
fn cut(text: &str) -> String {
    text.chars().take(limits::LISTED_VALUE_MAX_CHARS).collect()
}

/// A member's newest listed record as the fragment keeps it, and sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub shown: Shown,
    pub seq: i64,
    pub at: i64,
}

/// Whether a member's new record of a listed kind is sent to the lists:
/// when it shows something other than the one sent, or the same once it is
/// `LISTED_FRESH_MS` newer, so a list tells a poster still at it from one
/// gone quiet. Back-to-back records that change nothing send nothing, and
/// a record no newer than the one kept (a rebuild ahead of it) sends
/// nothing either: a late record never undoes a newer one.
pub fn moves(kept: Option<&Kept>, shown: &Shown, seq: i64, at: i64) -> bool {
    let Some(kept) = kept else { return true };
    if seq <= kept.seq {
        return false;
    }
    kept.shown != *shown || at - kept.at >= limits::LISTED_FRESH_MS
}

/// What a kept record shows under `decl` as installed now: nothing when
/// its kind is listed no more, and only the fields listed still. The copy
/// may be older than the install (its rebuild failed, or has not run), so
/// every row is made through this.
pub fn still(decl: &ListedDecl, kept: Shown) -> Option<Shown> {
    if !decl.kinds.contains(&kept.kind) {
        return None;
    }
    let Shown { kind, mut fields } = kept;
    fields.retain(|field, _| decl.fields.contains(field));
    assert!(fields.len() <= decl.fields.len(), "a record shows the fields listed, at most");
    Some(Shown { kind, fields })
}

/// The listed records a row at `role` carries: of `all` (each with the
/// role that may read its channel, as the channel's one read check
/// answered, and the newest first), those its role may read, at most
/// `LISTED_POSTERS_MAX`.
pub fn for_role(all: &[(Role, ListedRecord)], role: Role) -> Vec<ListedRecord> {
    let rows: Vec<ListedRecord> = all.iter().filter(|(read, _)| role >= *read).take(limits::LISTED_POSTERS_MAX).map(|(_, r)| r.clone()).collect();
    assert!(rows.len() <= limits::LISTED_POSTERS_MAX, "a row carries a bounded number of listed records");
    rows
}

/// Whether a listed record is one a fragment sends: a list refuses any
/// other (principal.rs).
pub fn valid(r: &ListedRecord) -> bool {
    let fields_valid = r.fields.len() <= limits::LISTED_FIELDS_MAX
        && r.fields.iter().all(|(k, v)| fragment_proto::valid_listed_field(k) && v.chars().count() <= limits::LISTED_VALUE_MAX_CHARS);
    fragment_proto::valid_channel_name(&r.channel) && crate::npub::is_identity(&r.principal) && fragment_proto::valid_listed_kind(&r.kind) && fields_valid && r.at >= 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decl() -> ListedDecl {
        ListedDecl { kinds: vec!["turn.start".into(), "turn.asked".into(), "turn.end".into()], fields: vec!["asks".into(), "turn".into()] }
    }

    fn shown_of(kind: &str, fields: &[(&str, &str)]) -> Shown {
        Shown { kind: kind.into(), fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    /// Goal: only a body naming a listed kind counts, and a list carries its
    /// listed fields that are strings, cut short, and nothing else of it.
    /// Method: bodies of every shape against one declaration.
    #[test]
    fn a_record_shows_its_kind_and_listed_fields() {
        let d = decl();
        let asked = json!({ "kind": "turn.asked", "turn": "t1", "asks": "npub1paul", "text": "never carried" });
        assert_eq!(shown(&d, &asked), Some(shown_of("turn.asked", &[("asks", "npub1paul"), ("turn", "t1")])));
        assert_eq!(shown(&d, &json!({ "kind": "turn.end", "turn": 7, "asks": null })), Some(shown_of("turn.end", &[])), "a field that is no string is left out");
        for none in [json!({ "kind": "turn.step" }), json!({ "text": "a message" }), json!({ "kind": 1 }), json!("turn.start"), json!(["turn.start"]), json!(null)] {
            assert_eq!(shown(&d, &none), None, "{none} lists nothing");
        }
        let long = "x".repeat(limits::LISTED_VALUE_MAX_CHARS * 2);
        let s = shown(&d, &json!({ "kind": "turn.start", "turn": long })).unwrap();
        assert_eq!(s.fields["turn"].chars().count(), limits::LISTED_VALUE_MAX_CHARS, "a long value is cut");
        let wide = "é".repeat(limits::LISTED_VALUE_MAX_CHARS + 1);
        let s = shown(&d, &json!({ "kind": "turn.start", "turn": wide })).unwrap();
        assert_eq!(s.fields["turn"], "é".repeat(limits::LISTED_VALUE_MAX_CHARS), "cut on a character, never inside one");
    }

    /// Goal: a member's records send a change only when it shows something
    /// new, or the same once it has gone a minute; a replay or a record
    /// older than the one kept sends nothing. Method: `moves` over a kept
    /// record and its successors.
    #[test]
    fn a_record_moves_the_list_only_when_it_says_more() {
        let working = shown_of("turn.start", &[("turn", "t1")]);
        assert!(moves(None, &working, 1, 1000), "a member's first listed record is sent");
        let kept = Kept { shown: working.clone(), seq: 4, at: 1000 };
        assert!(!moves(Some(&kept), &working, 5, 1000 + limits::LISTED_FRESH_MS - 1), "the same again within a minute sends nothing");
        assert!(moves(Some(&kept), &working, 5, 1000 + limits::LISTED_FRESH_MS), "the same a minute on is sent, so its time moves");
        assert!(moves(Some(&kept), &shown_of("turn.end", &[("turn", "t1")]), 5, 1001), "another kind is sent at once");
        assert!(moves(Some(&kept), &shown_of("turn.start", &[("turn", "t2")]), 5, 1001), "another field is sent at once");
        assert!(!moves(Some(&kept), &shown_of("turn.end", &[]), 4, 9_999_999), "the record kept, again, sends nothing");
        assert!(!moves(Some(&kept), &shown_of("turn.end", &[]), 3, 9_999_999), "an older record never undoes a newer one");
    }

    /// Goal: a kept record older than the install shows only what is
    /// listed now. Method: one kept record under declarations that drop its
    /// kind, a field, or neither.
    #[test]
    fn a_kept_record_shows_only_what_is_listed_still() {
        let kept = shown_of("turn.asked", &[("asks", "npub1p"), ("turn", "t1")]);
        assert_eq!(still(&decl(), kept.clone()), Some(kept.clone()), "listed as it was: the same");
        let fewer = ListedDecl { kinds: vec!["turn.asked".into()], fields: vec!["asks".into()] };
        assert_eq!(still(&fewer, kept.clone()), Some(shown_of("turn.asked", &[("asks", "npub1p")])), "a field listed no more is dropped");
        let other = ListedDecl { kinds: vec!["turn.end".into()], fields: vec!["asks".into(), "turn".into()] };
        assert_eq!(still(&other, kept), None, "a kind listed no more shows nothing");
    }

    /// Goal: a row carries only the records of channels its role may read,
    /// the newest first and bounded. Method: records of a viewers' and an
    /// editors' channel, read at each role.
    #[test]
    fn a_row_carries_what_its_role_reads() {
        let record = |channel: &str, at: i64| ListedRecord { channel: channel.into(), principal: "npub1x".into(), kind: "k".into(), fields: BTreeMap::new(), at };
        let all = vec![(Role::Editor, record("secret", 3)), (Role::Viewer, record("work", 2)), (Role::Public, record("wall", 1))];
        let channels = |role| for_role(&all, role).into_iter().map(|r| r.channel).collect::<Vec<_>>();
        assert_eq!(channels(Role::Owner), ["secret", "work", "wall"]);
        assert_eq!(channels(Role::Editor), ["secret", "work", "wall"]);
        assert_eq!(channels(Role::Viewer), ["work", "wall"], "a viewer's row never carries an editors' channel");
        assert_eq!(channels(Role::Public), ["wall"]);
        let many: Vec<(Role, ListedRecord)> = (0..limits::LISTED_POSTERS_MAX as i64 + 5).map(|i| (Role::Viewer, record("work", 100 - i))).collect();
        let rows = for_role(&many, Role::Viewer);
        assert_eq!(rows.len(), limits::LISTED_POSTERS_MAX, "at most the bound");
        assert_eq!(rows[0].at, 100, "the newest first, as given");
    }

    /// Goal: a list takes only what a fragment sends. Method: one valid
    /// record, then each of its parts made invalid.
    #[test]
    fn a_list_takes_only_valid_records() {
        let npub = crate::npub::encode(&"ab".repeat(32));
        let good = ListedRecord { channel: "work".into(), principal: npub, kind: "turn.asked".into(), fields: [("asks".to_string(), "npub1p".to_string())].into(), at: 1 };
        assert!(valid(&good));
        let bad: Vec<ListedRecord> = vec![
            ListedRecord { channel: "Work".into(), ..good.clone() },
            ListedRecord { principal: "anon:abc".into(), ..good.clone() },
            ListedRecord { kind: "turn asked".into(), ..good.clone() },
            ListedRecord { fields: [("kind".to_string(), "x".to_string())].into(), ..good.clone() },
            ListedRecord { fields: [("asks".to_string(), "x".repeat(limits::LISTED_VALUE_MAX_CHARS + 1))].into(), ..good.clone() },
            ListedRecord { fields: (0..=limits::LISTED_FIELDS_MAX).map(|i| (format!("f{i}"), String::new())).collect(), ..good.clone() },
            ListedRecord { at: -1, ..good.clone() },
        ];
        for r in bad {
            assert!(!valid(&r), "{r:?} is refused");
        }
    }
}
