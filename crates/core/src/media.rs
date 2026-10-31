//! A job's media steps (docs/api.md, AI). An image (`job.ai.image`) is
//! FLUX.1 [schnell] on Workers AI, called through the deployment's AI
//! Gateway as text is (cell/src/models.rs; decisions 17 and 23), and
//! metered on the payer's ledger in the neurons Workers AI prices it at:
//! its 512×512 tiles and its diffusion steps (`price::Usage::Neurons`). A
//! step is charged after its call at the image it got, its tiles read from
//! the JPEG's own header.
//!
//! A video (`job.ai.video`) is HappyHorse 1.0 text-to-video, a third-party
//! model of Cloudflare's catalog called through the same binding and
//! gateway, which pays for it by Unified Billing. Its answer names the
//! video's URL, which the cell reads (`VIDEO_MAX_BYTES` at most, an MP4
//! whose length reads from its header: `mp4_duration_ms`). It is charged
//! as the catalog bills it, its seconds at its resolution's price a second
//! (`VideoCall::usage`), as soon as the model answers with its URL: a read
//! that fails after it never makes the video again.

use base64::Engine;
use serde_json::{json, Value};

use crate::price::Usage;
use crate::steps::{AiImage, AiVideo};

/// The image model, by its Workers AI catalog id (its catalog entry,
/// 2026-10-03: `cloudflare-docs` `workers-ai-models/flux-1-schnell.json`).
pub const IMAGE_MODEL: &str = "@cf/black-forest-labs/flux-1-schnell";
/// What it answers: a JPEG (its catalog's output schema).
pub const IMAGE_MEDIA_TYPE: &str = "image/jpeg";
/// Its input's bounds, from the same schema: a prompt of 1 to 2048
/// characters, and at most 8 diffusion steps (4 unless asked).
pub const PROMPT_MAX_CHARS: usize = 2048;
pub const STEPS_DEFAULT: u32 = 4;
pub const STEPS_MAX: u32 = 8;
/// Workers AI's price for it (its pricing page, 2026-10-03): 4.80 neurons
/// per 512×512 tile and 9.60 per step, which at $0.011 per thousand
/// neurons (`price::DEFAULT_NEURONS`) is its listed $0.0000528 a tile and
/// $0.0001056 a step. Here in thousandths of a neuron, the ledger's unit.
pub const TILE_MILLI_NEURONS: u64 = 4_800;
pub const STEP_MILLI_NEURONS: u64 = 9_600;
pub const TILE_PX: u64 = 512;
/// The largest image kept: a 1024×1024 JPEG is a few hundred KiB, and its
/// base64 stays inside the model route's answer limit.
pub const IMAGE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Why a media step is refused before it calls anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A prompt that is empty or over `PROMPT_MAX_CHARS`.
    Prompt,
    /// Steps outside 1 to `STEPS_MAX`.
    Steps,
    /// A path that does not end in `.jpg` or `.jpeg`: the model draws
    /// JPEGs, and a file is served by its extension (bug 5).
    Path,
    /// A video's prompt that is empty or over `VIDEO_PROMPT_MAX_CHARS`.
    VideoPrompt,
    /// A video's duration outside `DURATION_MIN_S` to `DURATION_MAX_S`.
    Duration,
    /// A video's resolution other than `720p` or `1080p`.
    Resolution,
    /// A video's aspect ratio outside `ASPECT_RATIOS`.
    AspectRatio,
    /// A video's path that does not end in `.mp4`: the model makes MP4s.
    VideoPath,
}

impl Refusal {
    /// The refusal for people (and the job, which may catch it).
    pub fn message(self) -> String {
        match self {
            Refusal::Prompt => format!("ai.image's prompt is 1 to {PROMPT_MAX_CHARS} characters"),
            Refusal::Steps => format!("ai.image's steps is 1 to {STEPS_MAX} ({STEPS_DEFAULT} unless named)"),
            Refusal::Path => "ai.image draws a JPEG: its path ends in .jpg or .jpeg".into(),
            Refusal::VideoPrompt => format!("ai.video's prompt is 1 to {VIDEO_PROMPT_MAX_CHARS} characters"),
            Refusal::Duration => format!("ai.video's duration is {DURATION_MIN_S} to {DURATION_MAX_S} seconds ({DURATION_DEFAULT_S} unless named)"),
            Refusal::Resolution => "ai.video's resolution is 720p or 1080p (720p unless named)".into(),
            Refusal::AspectRatio => format!("ai.video's aspect_ratio is one of {} ({} unless named)", ASPECT_RATIOS.join(", "), ASPECT_RATIOS[0]),
            Refusal::VideoPath => "ai.video makes an MP4: its path ends in .mp4".into(),
        }
    }
}

/// Why the model's answer is no image the platform keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerFault {
    /// Not JSON with a base64 `image`.
    NoImage,
    /// Over `IMAGE_MAX_BYTES`.
    TooLarge,
    /// Not a JPEG whose size reads from its header.
    NotJpeg,
    /// A video's answer that is not a completed video at an http(s) URL.
    NoVideo,
    /// A video over `VIDEO_MAX_BYTES`.
    VideoTooLarge,
    /// Not an MP4 whose length reads from its header (`mp4_duration_ms`).
    NotMp4,
}

impl AnswerFault {
    pub fn message(self) -> String {
        match self {
            AnswerFault::NoImage => "the model's answer has no image".into(),
            AnswerFault::TooLarge => format!("the model's image is over {IMAGE_MAX_BYTES} bytes"),
            AnswerFault::NotJpeg => "the model's image is not a JPEG whose size reads".into(),
            AnswerFault::NoVideo => "the model's answer is no completed video".into(),
            AnswerFault::VideoTooLarge => format!("the model's video is over {VIDEO_MAX_BYTES} bytes"),
            AnswerFault::NotMp4 => "the model's video is not an MP4 whose length reads".into(),
        }
    }
}

/// An image step, checked: the model's input, and its steps (which its
/// price counts).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageCall {
    pub input: Value,
    pub steps: u32,
}

/// The call an image step makes, or why it is refused.
pub fn image_call(step: &AiImage) -> Result<ImageCall, Refusal> {
    let chars = step.prompt.chars().count();
    if chars == 0 || chars > PROMPT_MAX_CHARS {
        return Err(Refusal::Prompt);
    }
    let steps = step.steps.unwrap_or(STEPS_DEFAULT);
    if !(1..=STEPS_MAX).contains(&steps) {
        return Err(Refusal::Steps);
    }
    let lower = step.path.to_ascii_lowercase();
    if !(lower.ends_with(".jpg") || lower.ends_with(".jpeg")) {
        return Err(Refusal::Path);
    }
    Ok(ImageCall { input: json!({ "prompt": step.prompt, "steps": steps }), steps })
}

impl ImageCall {
    /// What an image of `width` × `height` at its steps cost.
    pub fn usage(&self, width: u16, height: u16) -> Usage {
        neurons(tiles(width, height), self.steps)
    }
}

/// The 512×512 tiles an image of `width` × `height` covers, a part tile
/// counted whole.
pub fn tiles(width: u16, height: u16) -> u64 {
    u64::from(width).div_ceil(TILE_PX) * u64::from(height).div_ceil(TILE_PX)
}

fn neurons(tiles: u64, steps: u32) -> Usage {
    // at most 128² tiles (u16 sides) and STEPS_MAX steps: far under QUANTITY_MAX
    Usage::Neurons { milli: tiles * TILE_MILLI_NEURONS + u64::from(steps) * STEP_MILLI_NEURONS }
}

/// An image the model drew: its bytes and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub bytes: Vec<u8>,
    pub width: u16,
    pub height: u16,
}

/// The image in the model's answer: `{"image": "<base64>"}`, its catalog's
/// output schema, as the binding answers it raw, or the same inside
/// `result`, as Workers AI's REST API wraps an answer.
pub fn image_of(answer: &[u8]) -> Result<Image, AnswerFault> {
    let v: Value = serde_json::from_slice(answer).map_err(|_| AnswerFault::NoImage)?;
    let b64 = v["image"].as_str().or_else(|| v["result"]["image"].as_str()).ok_or(AnswerFault::NoImage)?;
    // base64 is 4 characters for 3 bytes: a longer one decodes past the limit
    if b64.len() / 4 * 3 > IMAGE_MAX_BYTES + 3 {
        return Err(AnswerFault::TooLarge);
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| AnswerFault::NoImage)?;
    if bytes.len() > IMAGE_MAX_BYTES {
        return Err(AnswerFault::TooLarge);
    }
    let (width, height) = jpeg_size(&bytes).ok_or(AnswerFault::NotJpeg)?;
    Ok(Image { bytes, width, height })
}

/// A JPEG's width and height, from its first frame header (SOFn); `None`
/// when the bytes are no JPEG, or end before a frame header that reads.
pub fn jpeg_size(b: &[u8]) -> Option<(u16, u16)> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    // bounded: each pass moves `i` forward at least one byte
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        match marker {
            // a fill byte before a marker
            0xFF => i += 1,
            // markers that stand alone, with no length
            0x01 | 0xD0..=0xD7 => i += 2,
            // a scan, or the end, before any frame header
            0xD9 | 0xDA => return None,
            _ => {
                let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
                if len < 2 {
                    return None;
                }
                // SOF0 to SOF15, less DHT (C4), JPG (C8) and DAC (CC)
                if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    // its length, the sample precision, then height and width
                    let f = b.get(i + 4..i + 9)?;
                    let (height, width) = (u16::from_be_bytes([f[1], f[2]]), u16::from_be_bytes([f[3], f[4]]));
                    // a height of 0 is set later by a DNL marker: no size here
                    return (width > 0 && height > 0).then_some((width, height));
                }
                i += 2 + len;
            }
        }
    }
    None
}

/// Whether a JPEG is whole, read by its structure: its start, every
/// segment within the bytes and at its stated length, one frame header
/// (SOFn) whose components it holds, quantization and Huffman tables,
/// at least one scan whose header is whole and which carries data, and
/// its end (EOI) as its last two bytes. Its compressed data is not
/// decoded: that is a decoder's (and the cell's Wasm budget's), and what
/// it would refuse is an image someone made broken on purpose, no worse
/// to its viewers than any other image they chose. What it refuses is a
/// JPEG cut short (a header and no image, a frame header that ends
/// early, a scan with nothing after it), which no browser shows.
pub fn jpeg_whole(b: &[u8]) -> bool {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return false;
    }
    let (mut frame, mut quantized, mut huffman, mut scans) = (false, false, false, 0usize);
    let mut i = 2;
    // bounded: each pass moves `i` forward at least one byte
    while i + 2 <= b.len() {
        if b[i] != 0xFF {
            return false;
        }
        let marker = b[i + 1];
        match marker {
            // a fill byte before a marker
            0xFF => {
                i += 1;
                continue;
            }
            // the end: whole when it is the last of the bytes, after a scan
            0xD9 => return i + 2 == b.len() && frame && quantized && huffman && scans > 0,
            // markers that stand alone, with no length
            0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            // a second start
            0xD8 => return false,
            _ => {}
        }
        let Some(&[hi, lo]) = b.get(i + 2..i + 4) else { return false };
        let len = usize::from(u16::from_be_bytes([hi, lo]));
        let Some(segment) = b.get(i + 4..i + 2 + len.max(2)) else { return false };
        if len < 2 {
            return false;
        }
        match marker {
            // SOF0 to SOF15, less DHT (C4), JPG (C8) and DAC (CC): its
            // precision, height, width, and three bytes a component
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let n = usize::from(segment.get(5).copied().unwrap_or(0));
                let sized = segment.get(1..5).is_some_and(|s| s[0..2] != [0, 0] && s[2..4] != [0, 0]);
                if frame || n == 0 || segment.len() != 6 + 3 * n || !sized {
                    return false;
                }
                frame = true;
            }
            0xDB => quantized = true,
            0xC4 => huffman = true,
            // a scan: its components, two bytes each, three more, then its
            // data up to the next marker (0xFF00 is a data byte; restarts
            // are inside it)
            0xDA => {
                let n = usize::from(segment.first().copied().unwrap_or(0));
                if !frame || n == 0 || segment.len() != 4 + 2 * n {
                    return false;
                }
                let start = i + 2 + len;
                let mut j = start;
                // bounded: `j` moves forward each pass
                while j + 1 < b.len() && !(b[j] == 0xFF && b[j + 1] != 0x00 && !(0xD0..=0xD7).contains(&b[j + 1])) {
                    j += 1;
                }
                if j == start || j + 1 >= b.len() {
                    return false;
                }
                scans += 1;
                i = j;
                continue;
            }
            _ => {}
        }
        i += 2 + len;
    }
    false
}

// ---- videos

/// The video model, by its catalog id: Alibaba's HappyHorse 1.0
/// text-to-video, third-party, through the AI Gateway (Unified Billing),
/// with zero data retention (its catalog entry, 2026-10-31:
/// developers.cloudflare.com/ai/models/alibaba/hh1-t2v; its 1.1 keeps
/// what it is sent, so it is not this one).
pub const VIDEO_MODEL: &str = "alibaba/hh1-t2v";
/// What it makes: an MP4.
pub const VIDEO_MEDIA_TYPE: &str = "video/mp4";
/// Its input's bounds, from its catalog's schema: a prompt of 1 to 2500
/// characters, 3 to 15 seconds, and its ratios.
pub const VIDEO_PROMPT_MAX_CHARS: usize = 2500;
pub const DURATION_MIN_S: u32 = 3;
pub const DURATION_MAX_S: u32 = 15;
pub const DURATION_DEFAULT_S: u32 = 5;
pub const ASPECT_RATIOS: [&str; 5] = ["16:9", "9:16", "1:1", "4:3", "3:4"];
/// The largest video read from its URL: 15 seconds of 1080p is tens of
/// MiB, and the Durable Object holds it whole once.
pub const VIDEO_MAX_BYTES: usize = 64 * 1024 * 1024;
/// The longest a video's header may say it runs: past the model's 15
/// seconds with room for a frame's rounding, a length that is no video's.
pub const VIDEO_LENGTH_MAX_MS: u64 = (DURATION_MAX_S as u64 + 1) * 1000;

/// A video's resolution, as its model names it, and the row its price is
/// kept under in the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    P720,
    P1080,
}

impl Resolution {
    /// As the model's input names it.
    pub fn input(self) -> &'static str {
        match self {
            Resolution::P720 => "720P",
            Resolution::P1080 => "1080P",
        }
    }

    /// The book's row its seconds are priced by (`crate::price`'s
    /// `DEFAULT_MODELS`): a model row, its output a millisecond of video.
    pub fn priced_as(self) -> &'static str {
        match self {
            Resolution::P720 => "alibaba/hh1-t2v@720P",
            Resolution::P1080 => "alibaba/hh1-t2v@1080P",
        }
    }
}

/// A video step, checked: the model's input, and the seconds and the
/// resolution its price counts.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoCall {
    pub input: Value,
    pub seconds: u32,
    pub resolution: Resolution,
}

/// The call a video step makes, or why it is refused.
pub fn video_call(step: &AiVideo) -> Result<VideoCall, Refusal> {
    let chars = step.prompt.chars().count();
    if chars == 0 || chars > VIDEO_PROMPT_MAX_CHARS {
        return Err(Refusal::VideoPrompt);
    }
    let duration = step.duration.unwrap_or(DURATION_DEFAULT_S);
    if !(DURATION_MIN_S..=DURATION_MAX_S).contains(&duration) {
        return Err(Refusal::Duration);
    }
    let resolution = match step.resolution.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None | Some("720p") => Resolution::P720,
        Some("1080p") => Resolution::P1080,
        Some(_) => return Err(Refusal::Resolution),
    };
    let ratio = step.aspect_ratio.as_deref().unwrap_or(ASPECT_RATIOS[0]);
    if !ASPECT_RATIOS.contains(&ratio) {
        return Err(Refusal::AspectRatio);
    }
    if !step.path.to_ascii_lowercase().ends_with(".mp4") {
        return Err(Refusal::VideoPath);
    }
    let input = json!({ "prompt": step.prompt, "duration": duration, "resolution": resolution.input(), "ratio": ratio, "watermark": false });
    Ok(VideoCall { input, seconds: duration, resolution })
}

impl VideoCall {
    /// What the video costs: its seconds at its resolution, as the catalog
    /// bills a second of it, in its model row's output, one millisecond one
    /// unit. No usage kind of its own: every release's ledger and outbox
    /// store and price a model's tokens already, and one without this row
    /// charges it nothing (decision 62).
    pub fn usage(&self) -> Usage {
        Usage::Tokens { model: self.resolution.priced_as().into(), input: 0, cached_input: 0, cache_write: 0, output: u64::from(self.seconds) * 1000 }
    }
}

/// The video's URL in the model's answer: `{state: "Completed", result:
/// {video}}`, as the binding answers a third-party model raw (its catalog's
/// output), or `{video}` alone. A state other than completed, or a URL that
/// is not http(s), is no video.
pub fn video_url(answer: &[u8]) -> Result<String, AnswerFault> {
    let v: Value = serde_json::from_slice(answer).map_err(|_| AnswerFault::NoVideo)?;
    if v.get("state").is_some_and(|state| state != "Completed") {
        return Err(AnswerFault::NoVideo);
    }
    let url = v["result"]["video"].as_str().or_else(|| v["video"].as_str()).ok_or(AnswerFault::NoVideo)?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AnswerFault::NoVideo);
    }
    Ok(url.to_string())
}

/// A video the model made: its bytes and its length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Video {
    pub bytes: Vec<u8>,
    pub length_ms: u64,
}

/// The video read from its URL, if it is one the platform keeps: at most
/// `VIDEO_MAX_BYTES`, an MP4 whose length reads from its header.
pub fn video_of(bytes: Vec<u8>) -> Result<Video, AnswerFault> {
    if bytes.len() > VIDEO_MAX_BYTES {
        return Err(AnswerFault::VideoTooLarge);
    }
    let length_ms = mp4_duration_ms(&bytes).ok_or(AnswerFault::NotMp4)?;
    Ok(Video { bytes, length_ms })
}

/// One ISO media box at `at` in `b`: its type and its contents' range, or
/// `None` when its header does not read or it runs past `end`.
fn mp4_box(b: &[u8], at: usize, end: usize) -> Option<([u8; 4], std::ops::Range<usize>)> {
    let head = b.get(at..at.checked_add(8)?)?;
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
    let kind = [head[4], head[5], head[6], head[7]];
    let (header, size) = match size {
        // to the end of its container
        0 => (8, (end - at) as u64),
        // a 64-bit size follows the type
        1 => (16, u64::from_be_bytes(b.get(at + 8..at + 16)?.try_into().ok()?)),
        n => (8, n),
    };
    let stop = at.checked_add(usize::try_from(size).ok()?)?;
    (size >= header as u64 && stop <= end).then_some((kind, at + header..stop))
}

/// An MP4's length in milliseconds, rounded up: its movie header's
/// (`moov`/`mvhd`) duration over its timescale. `None` when the bytes are
/// no MP4 (an `ftyp` box first), its boxes do not read, or its length is
/// zero, unknown or past `VIDEO_LENGTH_MAX_MS`.
pub fn mp4_duration_ms(b: &[u8]) -> Option<u64> {
    let (first, _) = mp4_box(b, 0, b.len())?;
    if &first != b"ftyp" {
        return None;
    }
    let find = |kind: &[u8; 4], within: std::ops::Range<usize>| -> Option<std::ops::Range<usize>> {
        let mut at = within.start;
        // bounded: each box read moves `at` forward at least its 8-byte header
        while at < within.end {
            let (k, body) = mp4_box(b, at, within.end)?;
            if &k == kind {
                return Some(body);
            }
            at = body.end;
        }
        None
    };
    let mvhd = find(b"mvhd", find(b"moov", 0..b.len())?)?;
    let m = b.get(mvhd)?;
    // version 0: 32-bit times; 1: 64-bit; after its version and flags
    let (timescale, duration) = match *m.first()? {
        0 => (u32::from_be_bytes(m.get(12..16)?.try_into().ok()?), u64::from(u32::from_be_bytes(m.get(16..20)?.try_into().ok()?))),
        1 => (u32::from_be_bytes(m.get(20..24)?.try_into().ok()?), u64::from_be_bytes(m.get(24..32)?.try_into().ok()?)),
        _ => return None,
    };
    if timescale == 0 || duration == 0 || duration == u64::from(u32::MAX) || duration == u64::MAX {
        return None;
    }
    let ms = (u128::from(duration) * 1000).div_ceil(u128::from(timescale));
    u64::try_from(ms).ok().filter(|ms| *ms <= VIDEO_LENGTH_MAX_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::PriceBook;
    use crate::steps::Step;

    /// The head of a baseline JPEG of `width` × `height`: SOI, an APP0
    /// (JFIF) segment, a fill byte, SOF0, and the bytes a scan starts with.
    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8];
        b.extend([0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
        b.extend([0xFF, 0xFF, 0xC0, 0x00, 0x11, 8]);
        b.extend(height.to_be_bytes());
        b.extend(width.to_be_bytes());
        b.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        b.extend([0xFF, 0xDA, 0x00, 0x0C]);
        b
    }

    /// A whole baseline JPEG's structure, of `width` × `height`: SOI, DQT,
    /// SOF0 (three components), DHT, SOS, data (a stuffed 0xFF and a
    /// restart among it), EOI.
    fn whole(width: u16, height: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8];
        b.extend([0xFF, 0xDB, 0x00, 0x43, 0x00]);
        b.extend([1u8; 64]);
        b.extend([0xFF, 0xC0, 0x00, 0x11, 8]);
        b.extend(height.to_be_bytes());
        b.extend(width.to_be_bytes());
        b.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        b.extend([0xFF, 0xC4, 0x00, 0x13, 0x00]);
        b.extend([0u8; 16]);
        b.extend([0xFF, 0xDA, 0x00, 0x0C, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0x00, 0x3F, 0x00]);
        b.extend([0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
        b.extend([0xFF, 0xD9]);
        b
    }

    /// A real encoder's JPEG (40×24, one colour; the e2e's icon upload).
    const PHOTO: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSj/2wBDAQcHBwoIChMKChMoGhYaKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCj/wAARCAAYACgDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAb/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFgEBAQEAAAAAAAAAAAAAAAAAAAYH/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AngEi1YAAAAAAAAAB/9k=";

    /// Goal: a JPEG is whole only as a whole one is laid out; one cut
    /// short anywhere, or with a part missing or past its end, is not.
    #[test]
    fn a_jpeg_is_whole_or_cut_short() {
        let w = whole(1280, 800);
        assert!(jpeg_whole(&w));
        assert_eq!(jpeg_size(&w), Some((1280, 800)));
        let photo = base64::engine::general_purpose::STANDARD.decode(PHOTO).unwrap();
        assert!(jpeg_whole(&photo), "a real encoder's");
        // cut short anywhere: Alex's frame header that ends early among them
        for n in 0..w.len() {
            assert!(!jpeg_whole(&w[..n]), "cut at {n}");
        }
        assert!(!jpeg_whole(&[0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x03, 0x20, 0x05, 0x00]));
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x03, 0x20, 0x05, 0x00]), Some((1280, 800)), "which its size reads");
        // a header and its end, no image
        let mut header = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x03, 0x20, 0x05, 0x00];
        header.extend([0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xFF, 0xD9]);
        assert!(!jpeg_whole(&header));
        let mut after = w.clone();
        after.push(0);
        assert!(!jpeg_whole(&after), "bytes after its end");
        // a part missing: each table, the frame, the scan's data
        let without = |from: usize, len: usize| [&w[..from], &w[from + len..]].concat();
        let dqt = 2;
        let sof = dqt + 2 + 0x43;
        let dht = sof + 2 + 0x11;
        let sos = dht + 2 + 0x13;
        let data = sos + 2 + 0x0C;
        assert!(!jpeg_whole(&without(dqt, 2 + 0x43)), "no quantization table");
        assert!(!jpeg_whole(&without(sof, 2 + 0x11)), "no frame");
        assert!(!jpeg_whole(&without(dht, 2 + 0x13)), "no Huffman table");
        assert!(!jpeg_whole(&without(data, 7)), "a scan with no data");
        // a part past its end, or not its length
        let mut long = w.clone();
        long[dht + 3] = 0xFF;
        assert!(!jpeg_whole(&long), "a segment longer than the bytes");
        let mut frame = w.clone();
        frame[sof + 9] = 4;
        assert!(!jpeg_whole(&frame), "a frame naming more components than it holds");
        let mut twice = w[..data + 7].to_vec();
        twice.extend(&w[sof..sof + 2 + 0x11]);
        twice.extend(&w[data + 7..]);
        assert!(!jpeg_whole(&twice), "two frames");
        let mut zero = w.clone();
        zero[sof + 5] = 0;
        zero[sof + 6] = 0;
        assert!(!jpeg_whole(&zero), "a frame of no height");
        assert!(!jpeg_whole(b"\x89PNG\r\n\x1a\n"));
    }

    fn video(args: Value) -> AiVideo {
        match Step::from_parts("ai.video", args) {
            Ok(Step::AiVideo(v)) => v,
            other => panic!("{other:?}"),
        }
    }

    /// An MP4 whose movie header says `duration` at `timescale` (version
    /// 0's 32-bit fields, or version 1's 64-bit), `pad` bytes of media data
    /// before it, as a muxer that writes `moov` last does.
    pub(crate) fn mp4(timescale: u32, duration: u64, version: u8, pad: usize) -> Vec<u8> {
        let boxed = |kind: &[u8; 4], body: &[u8]| [&((body.len() + 8) as u32).to_be_bytes()[..], kind, body].concat();
        let mut mvhd = vec![version, 0, 0, 0];
        if version == 0 {
            mvhd.extend([0; 8]);
            mvhd.extend(timescale.to_be_bytes());
            mvhd.extend((duration as u32).to_be_bytes());
        } else {
            mvhd.extend([0; 16]);
            mvhd.extend(timescale.to_be_bytes());
            mvhd.extend(duration.to_be_bytes());
        }
        mvhd.extend([0; 80]);
        let moov = boxed(b"moov", &boxed(b"mvhd", &mvhd));
        [boxed(b"ftyp", b"isom\0\0\0\0isomiso2mp41"), boxed(b"mdat", &vec![7; pad]), moov].concat()
    }

    /// Goal: a video step's input is the catalog's, in meatproxy's shape,
    /// its length, resolution and ratio defaulted and bounded, its path an
    /// MP4's. Method: valid and invalid steps, each bound at and past its
    /// edge.
    #[test]
    fn a_video_step_is_checked_before_it_calls() {
        let call = video_call(&video(json!({ "prompt": "waves", "path": "video/waves.mp4" }))).unwrap();
        assert_eq!(call.input, json!({ "prompt": "waves", "duration": 5, "resolution": "720P", "ratio": "16:9", "watermark": false }));
        assert_eq!(call.resolution, Resolution::P720);
        let call = video_call(&video(json!({ "prompt": "waves", "path": "V.MP4", "duration": 15, "resolution": "1080P", "aspect_ratio": "9:16" }))).unwrap();
        assert_eq!((call.input["duration"].clone(), call.input["ratio"].clone(), call.resolution), (json!(15), json!("9:16"), Resolution::P1080));
        assert!(video_call(&video(json!({ "prompt": "é".repeat(VIDEO_PROMPT_MAX_CHARS), "path": "v.mp4", "duration": 3 }))).is_ok(), "characters, not bytes");
        let refused = |args: Value| video_call(&video(args)).unwrap_err();
        assert_eq!(refused(json!({ "prompt": "", "path": "v.mp4" })), Refusal::VideoPrompt);
        assert_eq!(refused(json!({ "prompt": "é".repeat(VIDEO_PROMPT_MAX_CHARS + 1), "path": "v.mp4" })), Refusal::VideoPrompt);
        assert_eq!(refused(json!({ "prompt": "p", "path": "v.mp4", "duration": 2 })), Refusal::Duration);
        assert_eq!(refused(json!({ "prompt": "p", "path": "v.mp4", "duration": 16 })), Refusal::Duration);
        assert_eq!(refused(json!({ "prompt": "p", "path": "v.mp4", "resolution": "768p" })), Refusal::Resolution, "the old model's resolution is no longer one");
        assert_eq!(refused(json!({ "prompt": "p", "path": "v.mp4", "aspect_ratio": "21:9" })), Refusal::AspectRatio);
        assert_eq!(refused(json!({ "prompt": "p", "path": "v.mov" })), Refusal::VideoPath);
    }

    /// Goal: a video costs its seconds at its resolution's price a second,
    /// so the book charges the catalog's list price. Method: five seconds
    /// (unnamed) at 720P lists at $0.70, at 1080P at $1.40, and fifteen at
    /// 1080P at $4.20.
    #[test]
    fn a_video_costs_its_seconds_at_its_resolution() {
        let at = |args: Value| video_call(&video(args)).unwrap();
        let book = PriceBook::defaults();
        let five = at(json!({ "prompt": "p", "path": "v.mp4" }));
        assert_eq!(five.usage(), Usage::Tokens { model: "alibaba/hh1-t2v@720P".into(), input: 0, cached_input: 0, cache_write: 0, output: 5_000 });
        assert_eq!(book.price(&five.usage()).unwrap().list, 700_000, "$0.14 a second");
        assert_eq!(book.price(&at(json!({ "prompt": "p", "path": "v.mp4", "resolution": "1080p" })).usage()).unwrap().list, 1_400_000, "$0.28 a second");
        let longest = at(json!({ "prompt": "p", "path": "v.mp4", "resolution": "1080p", "duration": DURATION_MAX_S }));
        assert_eq!(book.price(&longest.usage()).unwrap().list, 4_200_000);
        assert!(longest.usage().validate().is_ok());
    }

    /// Goal: the video's URL reads from the binding's raw answer, and only
    /// a completed one's. Method: the catalog's shape, the bare one, and
    /// each fault.
    #[test]
    fn the_videos_url_reads_from_the_models_answer() {
        let url = "https://examples.aig.cloudflare.com/alibaba/hh1-t2v/v.mp4";
        let raw = json!({ "state": "Completed", "result": { "video": url }, "gatewayMetadata": { "keySource": "Unified" } }).to_string();
        assert_eq!(video_url(raw.as_bytes()), Ok(url.to_string()));
        assert_eq!(video_url(json!({ "video": url }).to_string().as_bytes()), Ok(url.to_string()));
        for bad in [json!({ "state": "Failed", "result": { "video": url } }), json!({ "state": "Running" }), json!({ "result": {} }), json!({ "video": "ftp://x/v.mp4" }), json!({ "video": 7 })] {
            assert_eq!(video_url(bad.to_string().as_bytes()), Err(AnswerFault::NoVideo), "{bad}");
        }
        assert_eq!(video_url(b"not json"), Err(AnswerFault::NoVideo));
    }

    /// Goal: an MP4's length reads from its movie header wherever its
    /// boxes put it, rounded up to the millisecond; anything else is no
    /// length. Method: both header versions, `moov` after `mdat`, a 64-bit
    /// box size, and truncated, foreign, empty and absurd ones.
    #[test]
    fn an_mp4s_length_reads_from_its_header() {
        assert_eq!(mp4_duration_ms(&mp4(1000, 5_000, 0, 64)), Some(5_000));
        assert_eq!(mp4_duration_ms(&mp4(90_000, 450_000, 1, 0)), Some(5_000), "version 1");
        assert_eq!(mp4_duration_ms(&mp4(600, 3_001, 0, 0)), Some(5_002), "a part millisecond rounds up");
        let whole = mp4(1000, 6_000, 0, 1024);
        // the same MP4, its mdat box's size in 64 bits
        let (ftyp, rest) = whole.split_at(whole.windows(4).position(|w| w == b"mdat").unwrap() - 4);
        let (mdat, moov) = rest.split_at(8 + 1024);
        let wide = [ftyp, &1u32.to_be_bytes(), b"mdat", &((mdat.len() + 8) as u64).to_be_bytes(), &mdat[8..], moov].concat();
        assert_eq!(mp4_duration_ms(&wide), Some(6_000), "a 64-bit box size");
        for n in [0, 4, 12, whole.len() - 1] {
            assert_eq!(mp4_duration_ms(&whole[..n]), None, "cut at {n}");
        }
        assert_eq!(mp4_duration_ms(&mp4(0, 5_000, 0, 0)), None, "no timescale");
        assert_eq!(mp4_duration_ms(&mp4(1000, 0, 0, 0)), None, "no length");
        assert_eq!(mp4_duration_ms(&mp4(1000, u64::from(u32::MAX), 0, 0)), None, "an unknown length");
        assert_eq!(mp4_duration_ms(&mp4(1000, VIDEO_LENGTH_MAX_MS + 1, 1, 0)), None, "past any video's");
        assert_eq!(mp4_duration_ms(&mp4(1000, 5_000, 2, 0)), None, "an unknown header version");
        let mut no_ftyp = mp4(1000, 5_000, 0, 0);
        no_ftyp[4..8].copy_from_slice(b"free");
        assert_eq!(mp4_duration_ms(&no_ftyp), None, "no ftyp first");
        assert_eq!(mp4_duration_ms(&jpeg(16, 16)), None);
        assert_eq!(video_of(mp4(1000, 5_000, 0, 0)).map(|v| v.length_ms), Ok(5_000));
        assert_eq!(video_of(b"\x00\x00\x00\x08free".to_vec()), Err(AnswerFault::NotMp4));
        assert_eq!(video_of(vec![0; VIDEO_MAX_BYTES + 1]), Err(AnswerFault::VideoTooLarge));
    }

    fn image(args: Value) -> AiImage {
        match Step::from_parts("ai.image", args) {
            Ok(Step::AiImage(i)) => i,
            other => panic!("{other:?}"),
        }
    }

    /// Goal: a step's input is the catalog's, its steps defaulted and
    /// bounded, its path a JPEG's. Method: valid and invalid steps, each
    /// bound at and past its edge.
    #[test]
    fn an_image_step_is_checked_before_it_calls() {
        let call = image_call(&image(json!({ "prompt": "a lighthouse", "path": "art/a.jpg" }))).unwrap();
        assert_eq!(call, ImageCall { input: json!({ "prompt": "a lighthouse", "steps": 4 }), steps: 4 });
        let call = image_call(&image(json!({ "prompt": "p", "path": "A.JPEG", "steps": 8 }))).unwrap();
        assert_eq!(call.steps, 8);
        let refused = |args: Value| image_call(&image(args)).unwrap_err();
        assert_eq!(refused(json!({ "prompt": "", "path": "a.jpg" })), Refusal::Prompt);
        assert_eq!(refused(json!({ "prompt": "é".repeat(PROMPT_MAX_CHARS + 1), "path": "a.jpg" })), Refusal::Prompt);
        assert!(image_call(&image(json!({ "prompt": "é".repeat(PROMPT_MAX_CHARS), "path": "a.jpg" }))).is_ok(), "characters, not bytes");
        assert_eq!(refused(json!({ "prompt": "p", "path": "a.jpg", "steps": 0 })), Refusal::Steps);
        assert_eq!(refused(json!({ "prompt": "p", "path": "a.jpg", "steps": STEPS_MAX + 1 })), Refusal::Steps);
        assert_eq!(refused(json!({ "prompt": "p", "path": "a.png" })), Refusal::Path, "a JPEG at .png is bug 5");
        assert_eq!(refused(json!({ "prompt": "p", "path": "jpg" })), Refusal::Path);
    }

    /// Goal: an image costs its tiles and steps at Workers AI's neurons,
    /// so the book charges its listed price. Method: the default 1024×1024
    /// at 4 steps lists at 57.6 neurons ($0.0006336), and part tiles count
    /// whole.
    #[test]
    fn an_image_costs_its_tiles_and_steps() {
        let call = image_call(&image(json!({ "prompt": "p", "path": "a.jpg" }))).unwrap();
        assert_eq!(call.usage(1024, 1024), Usage::Neurons { milli: 57_600 });
        let list = PriceBook::defaults().price(&call.usage(1024, 1024)).unwrap().list;
        assert_eq!(list, 634, "57.6 neurons × $0.011 a thousand: $0.0006336, rounded up");
        assert_eq!(PriceBook::defaults().price(&neurons(1, 0)).unwrap().list, 53, "a tile lists at $0.0000528, rounded up");
        assert_eq!(PriceBook::defaults().price(&neurons(0, 1)).unwrap().list, 106, "a step at $0.0001056, rounded up");
        assert_eq!((tiles(512, 512), tiles(513, 512), tiles(1024, 768), tiles(1, 1)), (1, 2, 4, 1));
        assert_eq!(tiles(u16::MAX, u16::MAX), 128 * 128);
        let eight = image_call(&image(json!({ "prompt": "p", "path": "a.jpg", "steps": 8 }))).unwrap();
        assert_eq!(eight.usage(1024, 1024), Usage::Neurons { milli: 4 * 4_800 + 8 * 9_600 });
        assert!(neurons(tiles(u16::MAX, u16::MAX), STEPS_MAX).validate().is_ok(), "the largest image's usage prices");
    }

    /// Goal: a JPEG's size reads from its frame header, past the segments
    /// before it; anything else is no size. Method: a valid header,
    /// progressive (SOF2), and broken, truncated or foreign bytes.
    #[test]
    fn a_jpegs_size_reads_from_its_header() {
        assert_eq!(jpeg_size(&jpeg(1024, 768)), Some((1024, 768)));
        let mut progressive = jpeg(640, 480);
        let at = progressive.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        progressive[at + 1] = 0xC2;
        assert_eq!(jpeg_size(&progressive), Some((640, 480)));
        let mut dht_first = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x03, 0x00];
        dht_first.extend(&jpeg(16, 16)[2..]);
        assert_eq!(jpeg_size(&dht_first), Some((16, 16)), "a DHT segment is no frame header");
        assert_eq!(jpeg_size(b"\x89PNG\r\n\x1a\n"), None);
        assert_eq!(jpeg_size(&[0xFF, 0xD8]), None);
        let whole = jpeg(1024, 1024);
        // the frame header's width ends 9 bytes past its marker
        let end = whole.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap() + 9;
        for n in 0..end {
            assert_eq!(jpeg_size(&whole[..n]), None, "cut at {n}");
        }
        assert_eq!(jpeg_size(&whole[..end]), Some((1024, 1024)));
        assert_eq!(jpeg_size(&jpeg(1024, 0)), None, "a height set later by DNL");
        assert_eq!(jpeg_size(&jpeg(0, 1024)), None, "no width");
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x01, 0, 0]), None, "a length under 2");
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0x00, 0x00, 0x00, 0x00]), None, "no marker");
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0, 0]), None, "a scan before a frame");
    }

    /// Goal: the image in the binding's raw answer (and the REST API's
    /// wrapped one) is decoded and sized; any other answer is refused,
    /// typed. Method: each shape, and each fault.
    #[test]
    fn the_image_reads_from_the_models_answer() {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let drawn = jpeg(1024, 1024);
        let raw = json!({ "image": b64(&drawn) }).to_string();
        assert_eq!(image_of(raw.as_bytes()), Ok(Image { bytes: drawn.clone(), width: 1024, height: 1024 }));
        let wrapped = json!({ "result": { "image": b64(&drawn) }, "success": true }).to_string();
        assert_eq!(image_of(wrapped.as_bytes()).map(|i| i.width), Ok(1024));
        assert_eq!(image_of(b"not json"), Err(AnswerFault::NoImage));
        assert_eq!(image_of(br#"{"response": "a lighthouse"}"#), Err(AnswerFault::NoImage));
        assert_eq!(image_of(br#"{"image": "not base64!"}"#), Err(AnswerFault::NoImage));
        let png = json!({ "image": b64(b"\x89PNG\r\n\x1a\nrest") }).to_string();
        assert_eq!(image_of(png.as_bytes()), Err(AnswerFault::NotJpeg));
        let mut big = jpeg(1024, 1024);
        big.resize(IMAGE_MAX_BYTES + 1, 0);
        assert_eq!(image_of(json!({ "image": b64(&big) }).to_string().as_bytes()), Err(AnswerFault::TooLarge));
        let mut at_limit = jpeg(1024, 1024);
        at_limit.resize(IMAGE_MAX_BYTES, 0);
        assert_eq!(image_of(json!({ "image": b64(&at_limit) }).to_string().as_bytes()).map(|i| i.bytes.len()), Ok(IMAGE_MAX_BYTES));
    }

    /// Every refusal says why, the video's in the words the platform owes.
    #[test]
    fn refusals_say_why() {
        assert!(Refusal::Path.message().contains(".jpg"));
        assert!(Refusal::VideoPath.message().contains(".mp4"));
        assert!(Refusal::Duration.message().contains("3 to 15 seconds"));
        assert!(Refusal::AspectRatio.message().contains("16:9, 9:16, 1:1, 4:3, 3:4"));
        assert!(AnswerFault::VideoTooLarge.message().contains(&VIDEO_MAX_BYTES.to_string()));
        assert!(AnswerFault::TooLarge.message().contains(&IMAGE_MAX_BYTES.to_string()));
    }
}
