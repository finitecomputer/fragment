//! The fragment's live socket (`__live`), defined once: every frame a page
//! sends (`LiveIn`) and every frame the fragment answers (`LiveOut`), as
//! JSON objects tagged by `type`. The cell decodes and encodes through
//! these (cell/src/live.rs), the CLI's `channel --follow` decodes them, and
//! the browser library (cell/client.mjs) speaks the same field names.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;

use crate::{ChannelRecord, ErrorBody, ErrorCode, Role};

/// A frame a page sends. One that does not decode is answered with
/// `LiveOut::Error`, naming what was wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveIn {
    /// One page of a channel's records from a cursor; the page that
    /// reaches the end makes the socket live on the channel.
    Subscribe(Subscribe),
    Unsubscribe { channel: String },
    /// Runs a query as the socket's principal and role; a `result` with the
    /// same id answers it.
    Query(Query),
    /// Shares `data` with everyone on the page (at most
    /// `limits::PRESENCE_MAX_BYTES`); `null`, or no `data`, clears it. At
    /// most `limits::PRESENCE_PER_S` a second, after a burst of
    /// `limits::PRESENCE_BURST`: faster ones are dropped, each with an error.
    Presence {
        #[serde(default)]
        data: Value,
    },
    Ping,
}

/// `{type: "subscribe", channel, after}` or `{type: "subscribe", channel,
/// last}`: exactly one cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "SubscribeFrame", into = "SubscribeFrame")]
pub struct Subscribe {
    pub channel: String,
    pub from: Cursor,
}

/// `{type: "query", id, op, input}`: the page's own `id` for the run
/// (`^[A-Za-z0-9._:-]{1,128}$`, one run at a time each), the query, and
/// its input (`null` when absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub id: String,
    pub op: String,
    #[serde(default)]
    pub input: Value,
}

/// Where a subscribe's page starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// The records with `seq` after this one (0: from the start).
    After(i64),
    /// The channel's last this many records (at most `limits::CHANNEL_PAGE`).
    Last(i64),
}

/// A subscribe as it travels: the cursor is one of two keys.
#[derive(Serialize, Deserialize)]
// its schema is written by hand (`schema`, below), as its rules are
#[serde(deny_unknown_fields)]
struct SubscribeFrame {
    channel: String,
    /// The records with `seq` after this one (0: from the start). Exactly
    /// one of `after` and `last`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<i64>,
    /// The channel's last this many records (at most `limits::CHANNEL_PAGE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<i64>,
}

impl TryFrom<SubscribeFrame> for Subscribe {
    type Error = String;

    fn try_from(f: SubscribeFrame) -> Result<Subscribe, String> {
        let from = match (f.after, f.last) {
            (Some(after), None) => Cursor::After(after),
            (None, Some(last)) => Cursor::Last(last),
            (Some(_), Some(_)) => return Err("a subscribe starts after a seq or at the last records, not both".into()),
            (None, None) => return Err("a subscribe names where it starts: after or last".into()),
        };
        Ok(Subscribe { channel: f.channel, from })
    }
}

impl From<Subscribe> for SubscribeFrame {
    fn from(s: Subscribe) -> SubscribeFrame {
        let (after, last) = match s.from {
            Cursor::After(n) => (Some(n), None),
            Cursor::Last(n) => (None, Some(n)),
        };
        SubscribeFrame { channel: s.channel, after, last }
    }
}

/// A frame the fragment sends.
///
/// Open (`x-fragment-open`, docs/wire): a reader skips a frame it does not
/// know (the CLI's `channel --follow`, cell/client.mjs, the iOS app).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(extend("x-fragment-open" = true)))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveOut {
    /// First on every socket: who the page is to the fragment (the role is
    /// fixed while the socket stays open), the platform's origin (the one
    /// page that frames it with its person's session, whose messages a
    /// framed page may take: docs/api.md, The shell), and everyone on the
    /// page who shares presence; and what a message of the person's may
    /// carry (add-only: absent from an older platform).
    Hello {
        id: String,
        principal: String,
        role: Role,
        platform: String,
        presence: Vec<Present>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
        limits: Option<crate::UploadLimits>,
    },
    /// A record of a channel: one of a page, or a new one while live.
    Record(#[serde(deserialize_with = "record_through_value")] ChannelRecord),
    /// After each page: the cursor past it, and whether more follow. With
    /// `more`, the socket is not live on the channel yet: subscribe again
    /// from `next`.
    Subscribed { channel: String, next: i64, more: bool },
    /// One socket's presence changed: its data, or `null` once it cleared
    /// it or left.
    Presence(Present),
    /// A channel's draft: a record its poster is writing (a reply as it
    /// streams), its whole text so far, or `null` once they stopped.
    /// Never stored; the record that carries the same `turn` replaces it.
    /// `thinking`: a job's text step whose model reasons with no words yet
    /// (its text empty), for this many ms so far.
    Draft {
        channel: String,
        principal: String,
        turn: String,
        text: Option<String>,
        at: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
        thinking: Option<u64>,
    },
    /// A mutation applied: re-run live queries.
    Changed { op: String },
    /// A query's answer.
    Result(Answer),
    /// A frame the fragment refused, and why; the socket stays open.
    Error { message: String },
    Pong,
}

/// One socket's presence (`data` is never `null` in a `hello`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Present {
    pub id: String,
    pub principal: String,
    pub data: Value,
}

/// A query's answer: `{type: "result", id, result}`, or its refusal as
/// `__op` would answer it, `{type: "result", id, error, message, status}`
/// (the code's HTTP status, for the browser library's `FragmentError`).
/// The result is the app's answer as the cell got it, JSON text passed
/// through, never parsed on its way to the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "AnswerFrame", into = "AnswerFrame")]
pub struct Answer {
    pub id: String,
    pub outcome: Result<Box<RawValue>, ErrorBody>,
}

/// Two answers are equal when their results are equal JSON, whatever the
/// spacing or key order of the text.
impl PartialEq for Answer {
    fn eq(&self, other: &Answer) -> bool {
        let json = |raw: &RawValue| serde_json::from_str::<Value>(raw.get()).expect("a raw value holds JSON");
        self.id == other.id
            && match (&self.outcome, &other.outcome) {
                (Ok(a), Ok(b)) => json(a) == json(b),
                (Err(a), Err(b)) => a == b,
                _ => false,
            }
    }
}

/// An answer as it travels: a result, or an error with its message and status.
#[derive(Serialize, Deserialize)]
// its schema is written by hand (`schema`, below), as its rules are
#[serde(deny_unknown_fields)]
struct AnswerFrame {
    id: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    result: Option<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<ErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
}

/// A key that is present, even as `null`, is `Some`: a query may answer
/// `null`. Read through a `Value`, as a record frame's record is: serde
/// buffers a tagged frame's fields, and raw JSON text cannot pass through
/// that buffer. Only a reader of the frame pays this (the CLI, a test); the
/// cell writes the text it has.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    let value = Value::deserialize(d)?;
    serde_json::value::to_raw_value(&value).map(Some).map_err(serde::de::Error::custom)
}

impl TryFrom<AnswerFrame> for Answer {
    type Error = String;

    fn try_from(f: AnswerFrame) -> Result<Answer, String> {
        let outcome = match (f.result, f.error, f.message, f.status) {
            (Some(result), None, None, None) => Ok(result),
            (None, Some(error), Some(message), Some(status)) if status == error.status() => Err(ErrorBody { error, message, reason: None, retry_after_ms: None }),
            (None, Some(error), Some(_), Some(status)) => return Err(format!("status {status} is not {}'s", error.status())),
            _ => return Err("a result carries a result, or an error with its message and status".into()),
        };
        Ok(Answer { id: f.id, outcome })
    }
}

impl From<Answer> for AnswerFrame {
    fn from(a: Answer) -> AnswerFrame {
        match a.outcome {
            Ok(result) => AnswerFrame { id: a.id, result: Some(result), error: None, message: None, status: None },
            Err(e) => AnswerFrame { id: a.id, result: None, error: Some(e.error), status: Some(e.error.status()), message: Some(e.message) },
        }
    }
}

/// A record frame's record, read through a `Value`: serde buffers a tagged
/// frame's fields, and a body kept as raw JSON text cannot pass through
/// that buffer, while a `Value` can hand one over.
fn record_through_value<'de, D: Deserializer<'de>>(d: D) -> Result<ChannelRecord, D::Error> {
    ChannelRecord::deserialize(Value::deserialize(d)?).map_err(serde::de::Error::custom)
}

impl LiveIn {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("a live frame serializes")
    }
}

impl LiveOut {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("a live frame serializes")
    }

    /// A record's frame, the text `LiveOut::Record(record).encode()` makes,
    /// from a borrowed record: an append broadcasts its record uncopied.
    pub fn record_frame(record: &ChannelRecord) -> String {
        #[derive(Serialize)]
        #[serde(tag = "type", rename = "record")]
        struct Frame<'a> {
            #[serde(flatten)]
            record: &'a ChannelRecord,
        }
        serde_json::to_string(&Frame { record }).expect("a record frame serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(text: &str) -> Box<RawValue> {
        RawValue::from_string(text.to_string()).unwrap()
    }

    fn decode_in(v: Value) -> Result<LiveIn, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    #[test]
    fn a_page_sends_these_frames() {
        assert_eq!(
            decode_in(json!({ "type": "subscribe", "channel": "chat", "after": 7 })),
            Ok(LiveIn::Subscribe(Subscribe { channel: "chat".into(), from: Cursor::After(7) }))
        );
        assert_eq!(
            decode_in(json!({ "type": "subscribe", "channel": "chat", "last": 100 })),
            Ok(LiveIn::Subscribe(Subscribe { channel: "chat".into(), from: Cursor::Last(100) }))
        );
        assert_eq!(decode_in(json!({ "type": "unsubscribe", "channel": "chat" })), Ok(LiveIn::Unsubscribe { channel: "chat".into() }));
        assert_eq!(decode_in(json!({ "type": "presence", "data": { "name": "paul" } })), Ok(LiveIn::Presence { data: json!({ "name": "paul" }) }));
        assert_eq!(decode_in(json!({ "type": "presence", "data": null })), Ok(LiveIn::Presence { data: Value::Null }));
        assert_eq!(decode_in(json!({ "type": "presence" })), Ok(LiveIn::Presence { data: Value::Null }), "no data clears, as null does");
        assert_eq!(decode_in(json!({ "type": "ping" })), Ok(LiveIn::Ping));
        assert_eq!(
            decode_in(json!({ "type": "query", "id": "live-1", "op": "list", "input": { "n": 1 } })),
            Ok(LiveIn::Query(Query { id: "live-1".into(), op: "list".into(), input: json!({ "n": 1 }) }))
        );
        assert_eq!(
            decode_in(json!({ "type": "query", "id": "live-1", "op": "list" })),
            Ok(LiveIn::Query(Query { id: "live-1".into(), op: "list".into(), input: Value::Null })),
            "no input is null, as over __op"
        );
    }

    #[test]
    fn a_malformed_frame_is_an_error_not_a_default() {
        let refused = |v: Value, says: &str| {
            let e = decode_in(v.clone()).expect_err(&v.to_string());
            assert!(e.contains(says), "{v}: {e}");
        };
        refused(json!({ "type": "subscribe", "after": 0 }), "channel");
        refused(json!({ "type": "subscribe", "channel": "chat" }), "after or last");
        refused(json!({ "type": "subscribe", "channel": "chat", "after": 0, "last": 5 }), "not both");
        refused(json!({ "type": "subscribe", "channel": "chat", "after": "0" }), "invalid type");
        refused(json!({ "type": "subscribe", "channel": "chat", "after": 0, "extra": 1 }), "unknown field");
        refused(json!({ "type": "unsubscribe" }), "channel");
        refused(json!({ "type": "query", "op": "list" }), "missing field `id`");
        refused(json!({ "type": "query", "id": "q", "input": {} }), "missing field `op`");
        refused(json!({ "type": "query", "id": "q", "op": "list", "extra": 1 }), "unknown field");
        refused(json!({ "type": "shout" }), "unknown variant");
        refused(json!({ "channel": "chat" }), "type");
        assert!(serde_json::from_str::<LiveIn>("not json").is_err());
    }

    /// The JSON the browser library reads: the field names are the contract.
    #[test]
    fn the_fragment_sends_these_frames() {
        let record = ChannelRecord { channel: "chat".into(), seq: 3, at: 9, principal: "platform".into(), kind: "said".into(), body: serde_json::value::to_raw_value(&json!({ "text": "hi" })).unwrap() };
        let frames = [
            (
                LiveOut::Hello {
                    id: "ab".into(),
                    principal: "anon:x".into(),
                    role: Role::Public,
                    platform: "https://fragment.club".into(),
                    presence: vec![Present { id: "cd".into(), principal: "id:y".into(), data: json!({ "name": "paul" }) }],
                    limits: Some(crate::UploadLimits { upload_max_bytes: 100_000, files_per_message: 8 }),
                },
                json!({ "type": "hello", "id": "ab", "principal": "anon:x", "role": "public", "platform": "https://fragment.club", "presence": [{ "id": "cd", "principal": "id:y", "data": { "name": "paul" } }], "limits": { "uploadMaxBytes": 100_000, "filesPerMessage": 8 } }),
            ),
            (
                LiveOut::Record(record),
                json!({ "type": "record", "channel": "chat", "seq": 3, "at": 9, "principal": "platform", "kind": "said", "body": { "text": "hi" } }),
            ),
            (LiveOut::Subscribed { channel: "chat".into(), next: 3, more: true }, json!({ "type": "subscribed", "channel": "chat", "next": 3, "more": true })),
            (
                LiveOut::Presence(Present { id: "ab".into(), principal: "anon:x".into(), data: json!({ "name": "paul" }) }),
                json!({ "type": "presence", "id": "ab", "principal": "anon:x", "data": { "name": "paul" } }),
            ),
            (
                LiveOut::Presence(Present { id: "ab".into(), principal: "anon:x".into(), data: Value::Null }),
                json!({ "type": "presence", "id": "ab", "principal": "anon:x", "data": null }),
            ),
            (LiveOut::Changed { op: "say".into() }, json!({ "type": "changed", "op": "say" })),
            (LiveOut::Result(Answer { id: "live-1".into(), outcome: Ok(raw(r#"{ "n": 2 }"#)) }), json!({ "type": "result", "id": "live-1", "result": { "n": 2 } })),
            (LiveOut::Result(Answer { id: "live-2".into(), outcome: Ok(raw("null")) }), json!({ "type": "result", "id": "live-2", "result": null })),
            (
                LiveOut::Result(Answer { id: "live-3".into(), outcome: Err(ErrorBody { error: ErrorCode::Forbidden, message: "this needs the editor role".into(), reason: None, retry_after_ms: None }) }),
                json!({ "type": "result", "id": "live-3", "error": "forbidden", "message": "this needs the editor role", "status": 403 }),
            ),
            (LiveOut::Error { message: "no".into() }, json!({ "type": "error", "message": "no" })),
            (LiveOut::Pong, json!({ "type": "pong" })),
        ];
        for (frame, wire) in frames {
            assert_eq!(serde_json::from_str::<Value>(&frame.encode()).unwrap(), wire);
            // the CLI decodes what the cell encodes, from its text
            let decoded: LiveOut = serde_json::from_str(&wire.to_string()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
        }
    }

    /// An answer is a result or an error, never both or neither, and its
    /// status is its code's.
    #[test]
    fn an_answer_is_a_result_or_an_error() {
        let refused = |v: Value, says: &str| {
            let e = serde_json::from_value::<LiveOut>(v.clone()).expect_err(&v.to_string()).to_string();
            assert!(e.contains(says), "{v}: {e}");
        };
        refused(json!({ "type": "result", "id": "q" }), "a result, or an error");
        refused(json!({ "type": "result", "id": "q", "result": 1, "error": "forbidden", "message": "no", "status": 403 }), "a result, or an error");
        refused(json!({ "type": "result", "id": "q", "error": "forbidden", "status": 403 }), "a result, or an error");
        refused(json!({ "type": "result", "id": "q", "error": "forbidden", "message": "no", "status": 404 }), "is not 403's");
        refused(json!({ "type": "result", "id": "q", "error": "shrug", "message": "no", "status": 400 }), "unknown variant");
    }

    #[test]
    fn a_record_frame_from_a_borrowed_record_is_the_same_text() {
        let body = serde_json::value::to_raw_value(&json!({ "text": "hi", "n": [1, 2] })).unwrap();
        let record = ChannelRecord { channel: "chat".into(), seq: 3, at: 9, principal: "id:ab".into(), kind: "said".into(), body };
        let frame = LiveOut::record_frame(&record);
        assert_eq!(frame, LiveOut::Record(record).encode());
        assert!(frame.starts_with(r#"{"type":"record","channel":"chat""#), "{frame}");
    }

    #[test]
    fn a_subscribe_encodes_one_cursor() {
        let after = LiveIn::Subscribe(Subscribe { channel: "chat".into(), from: Cursor::After(0) });
        assert_eq!(serde_json::from_str::<Value>(&after.encode()).unwrap(), json!({ "type": "subscribe", "channel": "chat", "after": 0 }));
        let last = LiveIn::Subscribe(Subscribe { channel: "chat".into(), from: Cursor::Last(5) });
        assert_eq!(serde_json::from_str::<Value>(&last.encode()).unwrap(), json!({ "type": "subscribe", "channel": "chat", "last": 5 }));
        assert_eq!(serde_json::from_str::<Value>(&LiveIn::Ping.encode()).unwrap(), json!({ "type": "ping" }));
    }
}

/// The two frames serde reads through `TryFrom`, their schemas written by
/// hand as those rules are: the derive would say each optional field
/// stands alone, while a subscribe names exactly one cursor, and a result
/// exactly a result or a refusal whose status is its code's. A null field
/// is none, as the decoder reads it. The tests hold each schema to its
/// decoder, frame by frame.
#[cfg(feature = "schema")]
mod schema {
    use std::borrow::Cow;
    use std::collections::BTreeMap;

    use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
    use serde_json::{json, Value};

    use super::{AnswerFrame, SubscribeFrame};
    use crate::ErrorCode;

    impl JsonSchema for SubscribeFrame {
        fn inline_schema() -> bool {
            true
        }

        fn schema_name() -> Cow<'static, str> {
            "Subscribe".into()
        }

        fn json_schema(_: &mut SchemaGenerator) -> Schema {
            // its bounds besides its format: a validator need not assert a format
            // left out where the other cursor is named, as null is: none
            let cursor = |doc: &str| json!({ "type": ["integer", "null"], "format": "int64", "minimum": i64::MIN, "maximum": i64::MAX, "default": null, "description": doc });
            json_schema!({
                "type": "object",
                "properties": {
                    "channel": { "type": "string" },
                    "after": cursor("The records with `seq` after this one (0: from the start)."),
                    "last": cursor("The channel's last this many records (at most `limits::CHANNEL_PAGE`)."),
                },
                "required": ["channel"],
                "additionalProperties": false,
                "oneOf": [
                    { "required": ["after"], "properties": { "after": { "type": "integer" }, "last": { "type": "null" } } },
                    { "required": ["last"], "properties": { "last": { "type": "integer" }, "after": { "type": "null" } } },
                ],
            })
        }
    }

    /// An enum's names, as its derived schema lists them.
    fn names(schema: &Schema) -> Vec<String> {
        let value = schema.as_value();
        let listed = |v: &Value| v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        let mut out = listed(&value["enum"]);
        for branch in value["oneOf"].as_array().into_iter().flatten() {
            out.extend(branch["const"].as_str().map(str::to_string));
            out.extend(listed(&branch["enum"]));
        }
        out
    }

    impl JsonSchema for AnswerFrame {
        fn inline_schema() -> bool {
            true
        }

        fn schema_name() -> Cow<'static, str> {
            "Answer".into()
        }

        fn json_schema(g: &mut SchemaGenerator) -> Schema {
            // a refusal's status is its code's (`ErrorCode::status`)
            let mut by_status: BTreeMap<u16, Vec<String>> = BTreeMap::new();
            for name in names(&ErrorCode::json_schema(g)) {
                let code: ErrorCode = serde_json::from_value(Value::String(name.clone())).expect("a code's schema names its codes");
                by_status.entry(code.status()).or_default().push(name);
            }
            // open, as the codes are (`ErrorCode`): a reader reads an unknown one by its status
            let statuses: Vec<Value> = by_status
                .into_iter()
                .map(|(status, codes)| json!({ "properties": { "status": { "const": status }, "error": { "enum": codes, "x-fragment-open": true } } }))
                .collect();
            json_schema!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "result": { "description": "The query's answer, any JSON (`null` included), as the app gave it." },
                    // a result's refusal fields are left out, as null is: none
                    "error": { "anyOf": [g.subschema_for::<ErrorCode>(), { "type": "null" }], "default": null },
                    "message": { "type": ["string", "null"], "default": null },
                    "status": { "type": ["integer", "null"], "format": "uint16", "minimum": 0, "maximum": 65535, "default": null },
                },
                "required": ["id"],
                "additionalProperties": false,
                "oneOf": [
                    { "required": ["result"], "properties": { "error": { "type": "null" }, "message": { "type": "null" }, "status": { "type": "null" } } },
                    { "required": ["error", "message", "status"], "properties": { "result": false, "message": { "type": "string" } }, "anyOf": statuses, "x-fragment-open": true },
                ],
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use schemars::generate::SchemaSettings;
        use serde_json::{json, Map, Value};

        use crate::live::{LiveIn, LiveOut};

        /// Whether `v` holds to `s`, as JSON Schema says, for the keywords
        /// these frames' schemas use: `$ref`, `type`, `const`, `enum`,
        /// `properties`, `required`, `additionalProperties`, `items`,
        /// `oneOf`, `anyOf` and the `true`/`false` schemas. Bounds (`format`,
        /// `minimum`, `maximum`) are not held here.
        fn holds(s: &Value, v: &Value, defs: &Map<String, Value>) -> bool {
            let s = match s {
                Value::Bool(b) => return *b,
                Value::Object(m) => m,
                other => panic!("{other} is no schema"),
            };
            if let Some(r) = s.get("$ref").and_then(Value::as_str) {
                if !holds(&defs[r.rsplit('/').next().expect("a ref names its schema")], v, defs) {
                    return false;
                }
            }
            if let Some(t) = s.get("type") {
                let types: Vec<&str> = match t {
                    Value::String(t) => vec![t.as_str()],
                    Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                    _ => vec![],
                };
                let is = |t: &str| match t {
                    "null" => v.is_null(),
                    "string" => v.is_string(),
                    "integer" => v.is_i64() || v.is_u64(),
                    "number" => v.is_number(),
                    "boolean" => v.is_boolean(),
                    "object" => v.is_object(),
                    "array" => v.is_array(),
                    _ => false,
                };
                if !types.into_iter().any(is) {
                    return false;
                }
            }
            if s.get("const").is_some_and(|c| c != v) || s.get("enum").and_then(Value::as_array).is_some_and(|e| !e.contains(v)) {
                return false;
            }
            if let Some(obj) = v.as_object() {
                let props = s.get("properties").and_then(Value::as_object);
                if s.get("required").and_then(Value::as_array).into_iter().flatten().any(|r| !obj.contains_key(r.as_str().expect("a name"))) {
                    return false;
                }
                for (k, x) in obj {
                    let held = match (props.and_then(|p| p.get(k)), s.get("additionalProperties")) {
                        (Some(p), _) => holds(p, x, defs),
                        (None, Some(extra)) => holds(extra, x, defs),
                        (None, None) => true,
                    };
                    if !held {
                        return false;
                    }
                }
            }
            if let (Some(items), Some(a)) = (s.get("items"), v.as_array()) {
                if !a.iter().all(|x| holds(items, x, defs)) {
                    return false;
                }
            }
            if s.get("oneOf").and_then(Value::as_array).is_some_and(|one| one.iter().filter(|b| holds(b, v, defs)).count() != 1) {
                return false;
            }
            !s.get("anyOf").and_then(Value::as_array).is_some_and(|any| !any.iter().any(|b| holds(b, v, defs)))
        }

        /// `T`'s root schema as the wire's generator makes it, and its
        /// definitions.
        fn schema_of<T: schemars::JsonSchema>(answers: bool) -> (Value, Map<String, Value>) {
            let settings = SchemaSettings::draft2020_12();
            let settings = if answers { settings.for_serialize() } else { settings.for_deserialize() };
            let root = settings.into_generator().into_root_schema_for::<T>().to_value();
            let defs = root["$defs"].as_object().cloned().unwrap_or_default();
            (root, defs)
        }

        /// Each frame holds to the schema exactly when its decoder takes it;
        /// both kinds are among them.
        fn agree<T: serde::de::DeserializeOwned + schemars::JsonSchema>(answers: bool, frames: &[Value]) {
            let (schema, defs) = schema_of::<T>(answers);
            let mut taken = 0;
            for frame in frames {
                let decoded = serde_json::from_value::<T>(frame.clone()).is_ok();
                assert_eq!(holds(&schema, frame, &defs), decoded, "{frame}: its decoder {} it", if decoded { "takes" } else { "refuses" });
                taken += usize::from(decoded);
            }
            assert!(taken > 0 && taken < frames.len(), "taken {taken} of {}", frames.len());
        }

        #[test]
        fn a_subscribe_names_exactly_one_cursor_as_its_decoder_reads_it() {
            let s = |rest: Value| {
                let mut frame = json!({ "type": "subscribe", "channel": "chat" });
                frame.as_object_mut().unwrap().extend(rest.as_object().unwrap().clone());
                frame
            };
            agree::<LiveIn>(
                false,
                &[
                    s(json!({ "after": 0 })),
                    s(json!({ "last": 5 })),
                    s(json!({ "after": 7, "last": null })),
                    s(json!({ "after": null, "last": 5 })),
                    s(json!({})),
                    s(json!({ "after": 7, "last": 5 })),
                    s(json!({ "after": null })),
                    s(json!({ "after": null, "last": null })),
                    s(json!({ "after": 7, "more": true })),
                    json!({ "type": "subscribe", "after": 7 }),
                ],
            );
        }

        #[test]
        fn a_result_is_a_result_or_a_refusal_as_its_decoder_reads_it() {
            let r = |rest: Value| {
                let mut frame = json!({ "type": "result", "id": "q1" });
                frame.as_object_mut().unwrap().extend(rest.as_object().unwrap().clone());
                frame
            };
            agree::<LiveOut>(
                true,
                &[
                    r(json!({ "result": { "n": 1 } })),
                    r(json!({ "result": null })),
                    r(json!({ "result": 1, "error": null, "status": null })),
                    r(json!({ "error": "not_found", "message": "none", "status": 404 })),
                    r(json!({ "error": "deploying", "message": "soon", "status": 503 })),
                    r(json!({})),
                    r(json!({ "result": 1, "error": "not_found", "message": "none", "status": 404 })),
                    r(json!({ "error": "not_found", "message": "none" })),
                    r(json!({ "error": "not_found", "message": "none", "status": 500 })),
                    r(json!({ "error": "not_found", "message": "none", "status": 404, "result": null })),
                    r(json!({ "error": "not_found", "message": null, "status": 404 })),
                    r(json!({ "result": 1, "more": true })),
                ],
            );
        }
    }
}
