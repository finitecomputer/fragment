//! What a vendor's answer says a call used (FIN-616): the whole number a
//! catalog row's charge names (`catalog::Charge`), read from a header, from
//! a JSON body read whole, or from an event stream's last event naming it.
//! Nothing here guesses from the request: a number the answer does not
//! say is none.

use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

/// Segments a meter's JSON pointer may have: a usage field is never
/// deeper than a few objects (`/usage/cost_in_usd_ticks`,
/// `/data/metadata/creditsUsed`), and a pointer's walk goes one level a
/// segment.
pub const POINTER_SEGMENTS_MAX: usize = 8;
/// The longest event-stream line read for its number: a usage event is a
/// few hundred bytes; a longer line (a chunk of the answer's own text) is
/// skipped, never held.
pub const EVENT_LINE_MAX_BYTES: usize = 64 * 1024;

/// The segments of JSON pointer `pointer` (RFC 6901), `~1` and `~0`
/// unescaped, when it is one a meter may name: `/` first, 1 to
/// `POINTER_SEGMENTS_MAX` segments, none empty, a `~` only as `~0` or `~1`.
pub fn pointer_segments(pointer: &str) -> Option<Vec<String>> {
    let rest = pointer.strip_prefix('/')?;
    if rest.split('~').skip(1).any(|after| !after.starts_with(['0', '1'])) {
        return None;
    }
    let segments: Vec<String> = rest.split('/').map(|s| s.replace("~1", "/").replace("~0", "~")).collect();
    match segments.len() <= POINTER_SEGMENTS_MAX && segments.iter().all(|s| !s.is_empty()) {
        true => Some(segments),
        false => None,
    }
}

/// The whole number `value` is (a header's), when it is one: ASCII digits,
/// no sign, no fraction.
pub fn number(value: &str) -> Option<u64> {
    let digits = value.trim();
    match !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        true => digits.parse().ok(),
        false => None,
    }
}

/// The whole number at `path` in JSON `text`, when there is one: walked
/// to, every other value skipped unread (`IgnoredAny`), so a large answer
/// is never built in memory. A value there that is no whole number (a
/// float, a string, a negative) is none.
pub fn number_at(text: &[u8], path: &[String]) -> Option<u64> {
    assert!(!path.is_empty() && path.len() <= POINTER_SEGMENTS_MAX, "a charge's pointer is checked when its row is");
    let mut d = serde_json::Deserializer::from_slice(text);
    let found = At(path).deserialize(&mut d).ok()?;
    // a body with more after its value is no JSON answer
    d.end().ok()?;
    found
}

/// The walk to a pointer's value: one level a segment.
struct At<'p>(&'p [String]);

impl<'de> DeserializeSeed<'de> for At<'_> {
    type Value = Option<u64>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Option<u64>, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for At<'_> {
    type Value = Option<u64>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_u64<E>(self, v: u64) -> Result<Option<u64>, E> {
        Ok(self.0.is_empty().then_some(v))
    }

    fn visit_i64<E>(self, _: i64) -> Result<Option<u64>, E> {
        Ok(None)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Option<u64>, E> {
        Ok(None)
    }

    fn visit_str<E>(self, _: &str) -> Result<Option<u64>, E> {
        Ok(None)
    }

    fn visit_bool<E>(self, _: bool) -> Result<Option<u64>, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Option<u64>, E> {
        Ok(None)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Option<u64>, A::Error> {
        let mut found = None;
        // bounded by the answer, which serde_json reads once
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            match self.0.split_first() {
                Some((first, rest)) if found.is_none() && key == first.as_str() => found = map.next_value_seed(At(rest))?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(found)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Option<u64>, A::Error> {
        let index = self.0.split_first().and_then(|(first, rest)| Some((first.parse::<usize>().ok()?, rest)));
        let (mut found, mut n) = (None, 0usize);
        // bounded by the answer, which serde_json reads once
        loop {
            let read = match index {
                Some((i, rest)) if i == n => seq.next_element_seed(At(rest))?.map(|v| found = v),
                _ => seq.next_element::<IgnoredAny>()?.map(|_| ()),
            };
            if read.is_none() {
                return Ok(found);
            }
            n += 1;
        }
    }
}

/// An event stream's events read as they stream, each `data:` line's JSON
/// for the number at a pointer: the last event naming it is the call's
/// (a stream's usage is on its last chunk, cumulative). A line past
/// `EVENT_LINE_MAX_BYTES` is skipped, never held.
pub struct Events {
    path: Vec<String>,
    line: Vec<u8>,
    /// The line being read is past its limit: skipped to its end.
    skipping: bool,
    last: Option<u64>,
}

impl Events {
    pub fn new(path: Vec<String>) -> Events {
        Events { path, line: vec![], skipping: false, last: None }
    }

    /// The next piece of the stream, as it came.
    pub fn push(&mut self, piece: &[u8]) {
        // bounded by the piece
        for &b in piece {
            match b {
                b'\n' => self.end_line(),
                _ if self.skipping => {}
                _ if self.line.len() < EVENT_LINE_MAX_BYTES => self.line.push(b),
                _ => {
                    self.line.clear();
                    self.skipping = true;
                }
            }
        }
    }

    /// The stream has ended: the last number an event named.
    pub fn finish(mut self) -> Option<u64> {
        self.end_line();
        self.last
    }

    fn end_line(&mut self) {
        if !self.skipping {
            let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
            if let Some(data) = line.strip_prefix(b"data:") {
                if let Some(n) = number_at(data.trim_ascii(), &self.path) {
                    self.last = Some(n);
                }
            }
        }
        self.line.clear();
        self.skipping = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid and invalid: a pointer a meter may name is short and whole.
    #[test]
    fn a_pointer_is_short_and_whole() {
        assert_eq!(pointer_segments("/usage/cost_in_usd_ticks"), Some(vec!["usage".to_string(), "cost_in_usd_ticks".to_string()]));
        assert_eq!(pointer_segments("/a~0b/c~1d"), Some(vec!["a~b".to_string(), "c/d".to_string()]));
        for bad in ["", "usage", "/", "/a//b", "/a/b/c/d/e/f/g/h/i", "/usage/cost~2", "/a~"] {
            assert_eq!(pointer_segments(bad), None, "{bad:?}");
        }
    }

    fn at(text: &str, pointer: &str) -> Option<u64> {
        number_at(text.as_bytes(), &pointer_segments(pointer).expect("a pointer"))
    }

    /// Valid and invalid: a number is read where the pointer names it, in
    /// an object or a list, past values skipped; anything else there, or
    /// nothing, is none, as is a body that is no JSON.
    #[test]
    fn a_number_is_read_where_its_pointer_names_it() {
        let xai = r#"{"id":"r1","choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":9,"cost_in_usd_ticks":1234500}}"#;
        assert_eq!(at(xai, "/usage/cost_in_usd_ticks"), Some(1_234_500));
        assert_eq!(at(r#"{"data":{"metadata":{"creditsUsed":5}}}"#, "/data/metadata/creditsUsed"), Some(5));
        assert_eq!(at(r#"{"data":[{"n":1},{"n":7}]}"#, "/data/1/n"), Some(7));
        assert_eq!(at(r#"{"a~b":{"c/d":3}}"#, "/a~0b/c~1d"), Some(3));
        for (body, pointer) in [
            (r#"{"usage":{"cost":0.25}}"#, "/usage/cost"),
            (r#"{"usage":{"cost":"12"}}"#, "/usage/cost"),
            (r#"{"usage":{"cost":-3}}"#, "/usage/cost"),
            (r#"{"usage":{}}"#, "/usage/cost"),
            (r#"{"usage":7}"#, "/usage/cost"),
            (r#"[1,2]"#, "/5"),
            (r#"{"usage":{"cost":3}} trailing"#, "/usage/cost"),
            ("not json", "/usage/cost"),
            ("", "/usage/cost"),
        ] {
            assert_eq!(at(body, pointer), None, "{body} at {pointer}");
        }
    }

    /// A header is a whole number or none.
    #[test]
    fn a_header_is_a_whole_number() {
        assert_eq!(number(" 120 "), Some(120));
        for bad in ["", "1.5", "-2", "12a", "99999999999999999999999"] {
            assert_eq!(number(bad), None, "{bad:?}");
        }
    }

    /// An event stream's last event naming the number is the call's,
    /// however the stream is cut; a line past its limit is skipped.
    #[test]
    fn an_event_streams_last_number_is_the_calls() {
        let path = pointer_segments("/usage/cost_in_usd_ticks").unwrap();
        let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: {\"usage\":{\"cost_in_usd_ticks\":10}}\r\n\ndata: {\"usage\":{\"cost_in_usd_ticks\":42}}\n\ndata: [DONE]\n\n";
        for cut in 0..stream.len() {
            let mut e = Events::new(path.clone());
            e.push(&stream.as_bytes()[..cut]);
            e.push(&stream.as_bytes()[cut..]);
            assert_eq!(e.finish(), Some(42), "cut at {cut}");
        }
        let mut long = Events::new(path.clone());
        long.push(format!("data: {{\"usage\":{{\"cost_in_usd_ticks\":{}}}}}", "9".repeat(EVENT_LINE_MAX_BYTES)).as_bytes());
        long.push(b"\ndata: {\"usage\":{\"cost_in_usd_ticks\":5}}");
        assert_eq!(long.finish(), Some(5), "the long line skipped, the last line read at the end");
        assert_eq!(Events::new(path).finish(), None);
    }
}
