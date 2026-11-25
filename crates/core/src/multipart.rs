//! `multipart/form-data` (RFC 7578), read as OpenAI's SDKs write it: the
//! body of the model route's transcriptions (crate::transcribe), a file and
//! a few fields. Pure and bounded: each part is a slice of the body, never
//! a copy, and a body out of shape is refused whole, saying where. A form
//! is written the same way (`form`): a transcription sent on to an
//! OpenAI-compatible API.
//!
//! Ours rather than the runtime's `Request::form_data()`: the route's
//! NIP-98 signature binds the body's hash, so the cell reads the bytes
//! into its Wasm whatever parses them (`read_body`, bounded as it
//! streams). The runtime's parser would need them copied back out to
//! JavaScript and the audio copied in again, up to 10 MiB each way, and
//! would refuse a malformed form without saying how.

/// Parts one body holds at most: OpenAI's transcription form has six.
pub const PARTS_MAX: usize = 16;
/// One part's headers, at most.
pub const HEADERS_MAX_BYTES: usize = 4 * 1024;
/// A boundary is 1 to 70 characters (RFC 2046).
pub const BOUNDARY_MAX: usize = 70;

/// One part: its field's name, its file's name and type when it is a
/// file, and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part<'a> {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: &'a [u8],
}

/// Why a body is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Malformed {
    /// The content type is not `multipart/form-data` with a boundary.
    ContentType,
    /// No opening boundary, or no closing one.
    Boundary,
    /// A part's headers are out of shape (no blank line, too long, not text).
    Headers,
    /// A part names no field (`Content-Disposition: form-data; name=…`).
    Disposition,
    /// More than `PARTS_MAX` parts.
    TooMany,
}

impl Malformed {
    pub fn message(&self) -> &'static str {
        match self {
            Malformed::ContentType => "the body is multipart/form-data, with its boundary",
            Malformed::Boundary => "the multipart body opens and closes with its boundary",
            Malformed::Headers => "a part's headers end with a blank line, in at most 4 KiB of text",
            Malformed::Disposition => "each part is a form field: Content-Disposition: form-data; name=\"…\"",
            Malformed::TooMany => "a multipart body has at most 16 parts",
        }
    }
}

/// The boundary a `multipart/form-data` content type names.
pub fn boundary(content_type: &str) -> Result<String, Malformed> {
    let mut params = content_type.split(';');
    let kind = params.next().unwrap_or_default().trim();
    if !kind.eq_ignore_ascii_case("multipart/form-data") {
        return Err(Malformed::ContentType);
    }
    for p in params {
        let Some((k, v)) = p.split_once('=') else { continue };
        if k.trim().eq_ignore_ascii_case("boundary") {
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
            if v.is_empty() || v.len() > BOUNDARY_MAX || !v.bytes().all(|b| (0x20..0x7f).contains(&b)) {
                return Err(Malformed::ContentType);
            }
            return Ok(v.to_string());
        }
    }
    Err(Malformed::ContentType)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// A `Content-Disposition` parameter's value (`name="file"`), quoted or not.
fn param(value: &str, key: &str) -> Option<String> {
    let mut rest = value;
    // bounded by the value: each pass takes one parameter off it
    while let Some((_, after)) = rest.split_once(';') {
        let after = after.trim_start();
        let (k, v) = after.split_once('=')?;
        let (v, next) = match v.strip_prefix('"') {
            Some(q) => {
                let end = q.find('"')?;
                (q[..end].to_string(), &q[end + 1..])
            }
            None => {
                let end = v.find(';').unwrap_or(v.len());
                (v[..end].trim().to_string(), &v[end..])
            }
        };
        if k.trim().eq_ignore_ascii_case(key) {
            return Some(v);
        }
        rest = next;
    }
    None
}

/// One field of a form `form` writes: its name, its file's name and type
/// when it is a file, and its bytes. Its name, file name and type are
/// written into its headers as they are, so each is `header_safe`: a quote
/// or a line break in a client's file name would write a field of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field<'a> {
    pub name: &'a str,
    pub filename: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub data: &'a [u8],
}

/// A header value's most bytes as `form` writes it.
pub const HEADER_VALUE_MAX_BYTES: usize = 128;

/// Whether `s` may be written into a part's headers as it is: 1 to
/// `HEADER_VALUE_MAX_BYTES` of printable ASCII, no quote, backslash or
/// `;` (none ends its parameter early).
pub fn header_safe(s: &str) -> bool {
    (1..=HEADER_VALUE_MAX_BYTES).contains(&s.len()) && s.bytes().all(|b| (0x20..0x7f).contains(&b) && !matches!(b, b'"' | b'\\' | b';'))
}

/// `fields` as a `multipart/form-data` body with `boundary`, 1 to
/// `BOUNDARY_MAX` letters, digits and `-`; `None` when the boundary occurs
/// in a field's bytes (a random one does not: its caller picks another).
pub fn form(boundary: &str, fields: &[Field<'_>]) -> Option<Vec<u8>> {
    assert!((1..=BOUNDARY_MAX).contains(&boundary.len()) && boundary.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "a form's boundary is the writer's own: {boundary:?}");
    let named = |f: &Field<'_>| [Some(f.name), f.filename, f.content_type].into_iter().flatten().all(header_safe);
    assert!(fields.len() <= PARTS_MAX && fields.iter().all(named), "a form's fields are the writer's own, header-safe");
    let open = format!("--{boundary}");
    if fields.iter().any(|f| find(f.data, open.as_bytes(), 0).is_some()) {
        return None;
    }
    let mut out = Vec::with_capacity(fields.iter().map(|f| f.data.len() + 256).sum::<usize>() + open.len() + 4);
    for f in fields {
        out.extend(format!("{open}\r\nContent-Disposition: form-data; name=\"{}\"", f.name).as_bytes());
        if let Some(file) = f.filename {
            out.extend(format!("; filename=\"{file}\"").as_bytes());
        }
        if let Some(kind) = f.content_type {
            out.extend(format!("\r\nContent-Type: {kind}").as_bytes());
        }
        out.extend(b"\r\n\r\n");
        out.extend(f.data);
        out.extend(b"\r\n");
    }
    out.extend(format!("{open}--\r\n").as_bytes());
    Some(out)
}

/// The parts of `body`, a `multipart/form-data` body with `boundary`.
pub fn parts<'a>(body: &'a [u8], boundary: &str) -> Result<Vec<Part<'a>>, Malformed> {
    let open = format!("--{boundary}");
    let delimiter = format!("\r\n--{boundary}");
    let start = find(body, open.as_bytes(), 0).ok_or(Malformed::Boundary)?;
    // a preamble before the first boundary is allowed (and ignored)
    if start != 0 && !body[..start].ends_with(b"\r\n") {
        return Err(Malformed::Boundary);
    }
    let mut at = start + open.len();
    let mut out = Vec::new();
    // bounded by PARTS_MAX
    loop {
        if body[at..].starts_with(b"--") {
            return Ok(out);
        }
        if !body[at..].starts_with(b"\r\n") {
            return Err(Malformed::Boundary);
        }
        at += 2;
        if out.len() == PARTS_MAX {
            return Err(Malformed::TooMany);
        }
        let end_of_headers = find(body, b"\r\n\r\n", at).filter(|e| e - at <= HEADERS_MAX_BYTES).ok_or(Malformed::Headers)?;
        let headers = std::str::from_utf8(&body[at..end_of_headers]).map_err(|_| Malformed::Headers)?;
        let data_start = end_of_headers + 4;
        let data_end = find(body, delimiter.as_bytes(), data_start).ok_or(Malformed::Boundary)?;
        let (mut name, mut filename, mut content_type) = (None, None, None);
        for line in headers.split("\r\n") {
            let Some((k, v)) = line.split_once(':') else { return Err(Malformed::Headers) };
            let (k, v) = (k.trim(), v.trim());
            if k.eq_ignore_ascii_case("content-disposition") {
                if !v.split(';').next().is_some_and(|d| d.trim().eq_ignore_ascii_case("form-data")) {
                    return Err(Malformed::Disposition);
                }
                name = param(v, "name");
                filename = param(v, "filename");
            } else if k.eq_ignore_ascii_case("content-type") {
                content_type = Some(v.to_string());
            }
        }
        let name = name.filter(|n| !n.is_empty()).ok_or(Malformed::Disposition)?;
        out.push(Part { name, filename, content_type, data: &body[data_start..data_end] });
        at = data_end + delimiter.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body as OpenAI's Python SDK (httpx) writes a transcription's form.
    fn form(boundary: &str, audio: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        for (name, value) in [("model", "whisper"), ("response_format", "json")] {
            b.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        }
        b.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.ogg\"\r\nContent-Type: audio/ogg\r\n\r\n").as_bytes());
        b.extend(audio);
        b.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
        b
    }

    /// Valid: an SDK's form reads as its fields and its file, the file's
    /// bytes exactly as sent, a CRLF and a boundary-like run inside them
    /// included.
    #[test]
    fn an_sdks_form_reads_as_its_fields_and_file() {
        let boundary = boundary("multipart/form-data; boundary=8a2bc1f00e").unwrap();
        assert_eq!(boundary, "8a2bc1f00e");
        let audio = b"OggS\x00\x02\r\n--8a2bc1f0\r\nnot a boundary\xff\xfe".to_vec();
        let body = form(&boundary, &audio);
        let p = parts(&body, &boundary).unwrap();
        assert_eq!(p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["model", "response_format", "file"]);
        assert_eq!((p[0].data, p[0].filename.as_deref()), (&b"whisper"[..], None));
        assert_eq!((p[2].data, p[2].filename.as_deref(), p[2].content_type.as_deref()), (&audio[..], Some("note.ogg"), Some("audio/ogg")));
        // quoted, and among other parameters, with headers in any case
        assert_eq!(boundary_of("Multipart/Form-Data; charset=utf-8; boundary=\"a b\""), Ok("a b".into()));
        let lower = b"--x\r\ncontent-disposition: form-data; filename=\"a;b.wav\"; name=file\r\n\r\nRIFF\r\n--x--";
        let p = parts(lower, "x").unwrap();
        assert_eq!((p[0].name.as_str(), p[0].filename.as_deref(), p[0].data), ("file", Some("a;b.wav"), &b"RIFF"[..]));
        // an empty field, and a preamble before the first boundary
        let p = parts(b"preamble\r\n--x\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\n\r\n--x--\r\n", "x").unwrap();
        assert_eq!((p[0].name.as_str(), p[0].data), ("prompt", &b""[..]));
    }

    fn boundary_of(ct: &str) -> Result<String, Malformed> {
        boundary(ct)
    }

    /// Valid: a form written reads back as its fields, a file's bytes as
    /// they were (a CRLF and a boundary-like run among them); a boundary
    /// that occurs in them writes none; a name no header may carry is
    /// none `form` takes.
    #[test]
    fn a_form_written_reads_back() {
        let audio = b"OggS\r\n--fragment-x\r\n\xff".to_vec();
        let fields = [
            Field { name: "model", filename: None, content_type: None, data: b"local-whisper" },
            Field { name: "file", filename: Some("memo.ogg"), content_type: Some("audio/ogg"), data: &audio },
        ];
        let body = super::form("fragment-0a1b", &fields).unwrap();
        let p = parts(&body, "fragment-0a1b").unwrap();
        assert_eq!(p.iter().map(|p| (p.name.as_str(), p.filename.as_deref(), p.content_type.as_deref(), p.data)).collect::<Vec<_>>(), [("model", None, None, &b"local-whisper"[..]), ("file", Some("memo.ogg"), Some("audio/ogg"), &audio[..])]);
        assert_eq!(super::form("fragment-x", &fields), None, "its boundary in the audio: another is picked");
        assert_eq!(parts(&super::form("b", &[]).unwrap(), "b"), Ok(vec![]));
        for bad in ["a\"b.ogg", "a\r\nX: y", "a;b", "a\\b", "", &"a".repeat(HEADER_VALUE_MAX_BYTES + 1), "é.ogg"] {
            assert!(!header_safe(bad), "{bad:?}");
        }
        assert!(header_safe("voice memo (1).m4a") && header_safe("audio/mp4"));
    }

    /// Invalid: what is no multipart form is refused, saying which way.
    #[test]
    fn a_body_out_of_shape_is_refused() {
        for ct in ["application/json", "multipart/form-data", "multipart/form-data; boundary=", "multipart/mixed; boundary=x", &format!("multipart/form-data; boundary={}", "x".repeat(71))] {
            assert_eq!(boundary(ct), Err(Malformed::ContentType), "{ct}");
        }
        assert_eq!(parts(b"no boundary here", "x"), Err(Malformed::Boundary));
        assert_eq!(parts(b"--x\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nunclosed", "x"), Err(Malformed::Boundary));
        assert_eq!(parts(b"--x\r\nContent-Disposition: form-data; name=\"a\"\r\nno blank line", "x"), Err(Malformed::Headers));
        assert_eq!(parts(b"--x\r\nContent-Disposition: attachment; name=\"a\"\r\n\r\nv\r\n--x--", "x"), Err(Malformed::Disposition));
        assert_eq!(parts(b"--x\r\nContent-Type: text/plain\r\n\r\nv\r\n--x--", "x"), Err(Malformed::Disposition));
        assert_eq!(parts(b"--x\r\nnot a header\r\n\r\nv\r\n--x--", "x"), Err(Malformed::Headers));
        let long = format!("--x\r\nX-Pad: {}\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nv\r\n--x--", "p".repeat(HEADERS_MAX_BYTES));
        assert_eq!(parts(long.as_bytes(), "x"), Err(Malformed::Headers));
        let many: String = (0..=PARTS_MAX).map(|i| format!("--x\r\nContent-Disposition: form-data; name=\"f{i}\"\r\n\r\nv\r\n")).collect::<String>() + "--x--";
        assert_eq!(parts(many.as_bytes(), "x"), Err(Malformed::TooMany));
        let mut empty = Vec::new();
        empty.extend(b"--x--");
        assert_eq!(parts(&empty, "x"), Ok(vec![]), "a form with no fields");
    }
}
