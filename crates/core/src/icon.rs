//! A fragment's icon (docs/api.md, Icons; issue #587): one `SIZE` × `SIZE`
//! PNG of one object on a transparent background, a clean 3D CG render
//! in the look of the bot's own picture (the shell's `agent.png`), which
//! the dock shows and the fragment's pages take as their favicon. It is made or uploaded; one made is drawn, cut out,
//! checked twice, and offered among `CANDIDATES`, its asker picking one.
//!
//! This is its pure part, run by the cell (cell/src/icon.rs), which calls:
//!
//! 1. GLM-5.3 Flash on Workers AI, for its subject (`subject_input`,
//!    `subject_of`): the app's title and description named as one
//!    concrete object. An abstract name drawn as it is comes back a flat
//!    rounded square, the look an icon must not have (the spike,
//!    2026-10-24). Its look is asked in words (`STYLE`), with no
//!    reference image: the bot's picture as one (FLUX.2 takes up to four),
//!    even named "a style reference only", turned a keycap and a desk
//!    calendar into its monitor (the same spike).
//! 2. FLUX.2 [klein] 4B on Workers AI, `CANDIDATES` times, for its
//!    drawings (`image_input`): the only model Cloudflare hosts that draws
//!    a bold object filling its frame (flux-1-schnell's are small and
//!    faint, in the same spike).
//! 3. The Images binding's `segment: foreground` (BiRefNet), for each
//!    drawing's object on a transparent background, which `examine` reads
//!    from its alpha: one piece, inside the frame, no tile, clean edges.
//! 4. Images again, for the object cropped (`crop`) and padded to `SIZE`,
//!    and flattened onto `BACKDROP` for:
//! 5. Clef on Workers AI (`check`, `judged`): no writing, no tile, one
//!    object, the subject. Clef is the only one of the five that reads
//!    an image to answer a question with a probability, and the cheapest.
//!
//! Each call is metered on the fragment's owner's ledger (docs/ledger.md):
//! models in neurons and tokens, Images in transformations.

use serde_json::{json, Value};

use crate::decide;
use crate::price::Usage;

/// The icon's side, in pixels: the dock's largest (a phone's 2×) and well
/// past a favicon's.
pub const SIZE: u32 = 256;
/// How many drawings one making offers, drawn at once.
pub const CANDIDATES: usize = 4;
/// A drawing's side: the smallest the model's price counts in, and four
/// times `SIZE`'s area to crop from.
pub const DRAWN_PX: u32 = 512;
/// The subject's model, by its Workers AI catalog id (in the price book).
pub const SUBJECT_MODEL: &str = "@cf/zai-org/glm-5.3-flash";
/// The drawings' model (its catalog entry, 2026-10-24): JPEGs of 256 to
/// 1920 a side, its input multipart form fields.
pub const IMAGE_MODEL: &str = "@cf/black-forest-labs/flux-2-klein-4b";
/// What a drawing costs when Workers AI does not say (its answer's
/// `cf-ai-neurons`, which `neurons` reads): 104.20 neurons at 512×512, as
/// it said in the spike (its pricing page's 26.05 per output tile, four
/// times), in thousandths of a neuron, the ledger's unit.
pub const DRAWN_MILLI_NEURONS: u64 = 104_200;
/// The checks' model: Clef's cheapest, which separated every drawing of
/// the spike as clef-omni did, at a quarter of its price.
pub const CHECK_MODEL: decide::Model = decide::Model::ClefFlash;
/// The colour an icon is flattened onto for its check: a plain blue, as
/// a desktop's, that a white or grey object stands out against.
pub const BACKDROP: &str = "#3a6ea5";
/// What its asker may add: a few words ("a rocket", "make it purple").
pub const HINT_MAX_CHARS: usize = 200;
/// The longest subject drawn; a longer answer is the model's mistake.
pub const SUBJECT_MAX_CHARS: usize = 120;
/// The subject's answer, its reasoning included (GLM reasons at `low`).
pub const SUBJECT_MAX_TOKENS: u32 = 400;
/// What a title and a description may add to the subject's question.
const ABOUT_MAX_CHARS: usize = 600;
/// An upload: an image of any size up to the router's body, which Images
/// fits to `SIZE` (the router refuses a larger one, 413).
pub const UPLOAD_MAX_BYTES: usize = fragment_proto::limits::BODY_MAX_BYTES;
/// The icon kept, as a profile picture is: a drawn one is 50 to 70 KiB; an
/// upload fitted past it (a photograph's grain) is refused, saying so.
pub const ICON_MAX_BYTES: usize = fragment_proto::limits::PICTURE_MAX_BYTES;
/// The largest image `examine` reads, a side: a drawing is `DRAWN_PX`.
pub const EXAMINE_MAX_PX: u32 = 2048;
/// How many makings a fragment starts in an hour: each is a few cents at
/// most, and a page that loops would otherwise spend its owner's money.
pub const MAKINGS_PER_HOUR: usize = 10;
pub const HOUR_MS: i64 = 3_600_000;
/// The media type kept and served.
pub const MEDIA_TYPE: &str = "image/png";

/// How an icon is drawn, before its subject. Every rule a drawing breaks
/// is checked after (`examine`, `check`): the prompt only makes it rarer.
const STYLE: &str = "A clean 3D CG render of one object, like a product shot for a desktop icon: smooth satin plastic and metal, soft diffuse studio lighting, gentle ambient occlusion, crisp clean edges, slight 3/4 view, clear friendly colours. Centered, filling about 70% of the frame, isolated on a plain flat light grey background. No text, letters or numbers on it. No rounded square, no tile, no app frame, no border.";

/// What the subject's question tells GLM, before the app it is asked of.
pub const SUBJECT_SYSTEM: &str = "You name the one physical object that a desktop icon for an app should show. Answer with a short noun phrase only (at most 12 words): one concrete, colourful, everyday object, optionally with one small accessory touching it. Never an abstract shape, a logo, a screen, a person, or anything with writing or numbers on it. Examples: \"a red apple with a green leaf\", \"a blue mail tray holding letters\".";

/// Where the subject's question names its asker's hint, and a drawing's
/// prompt its subject, after the style: what each is told by (the e2e's
/// fake reads them so).
pub const ASKS: &str = "\nIts maker asks for: ";
pub const OBJECT: &str = " The object: ";

/// Why a making or an upload is refused before anything is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A hint over `HINT_MAX_CHARS`, or not text.
    Hint,
    /// `MAKINGS_PER_HOUR` started within the hour.
    Busy,
    /// An upload that is no PNG, JPEG, WebP or GIF by its bytes.
    NotAnImage,
    /// An agent's picture is its `avatar.png` (crate::face).
    Agent,
}

impl Refusal {
    pub fn message(self) -> String {
        match self {
            Refusal::Hint => format!("an icon's hint is text of at most {HINT_MAX_CHARS} characters"),
            Refusal::Busy => format!("{MAKINGS_PER_HOUR} icons were made for this fragment in the last hour: try again later"),
            Refusal::NotAnImage => "an icon is made from a PNG, JPEG, WebP or GIF".into(),
            Refusal::Agent => "an agent's picture is its avatar.png: set it in Settings, Agents".into(),
        }
    }
}

/// A making's hint: none, or trimmed text of at most `HINT_MAX_CHARS`.
pub fn hint(body: &Value) -> Result<Option<String>, Refusal> {
    match body.get("hint") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.chars().count() <= HINT_MAX_CHARS => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
        Some(_) => Err(Refusal::Hint),
    }
}

/// Whether another making may start at `now_ms`, given when the last ones
/// started (kept, oldest first): the starts within the hour, with this
/// one's, or `Busy`.
pub fn admit(started: &[i64], now_ms: i64) -> Result<Vec<i64>, Refusal> {
    let mut within: Vec<i64> = started.iter().copied().filter(|t| now_ms - t < HOUR_MS).collect();
    if within.len() >= MAKINGS_PER_HOUR {
        return Err(Refusal::Busy);
    }
    within.push(now_ms);
    assert!(within.len() <= MAKINGS_PER_HOUR, "a making's starts are bounded");
    Ok(within)
}

/// The subject's question, for GLM on Workers AI: the fragment's title
/// and description, and its asker's hint.
pub fn subject_input(title: &str, description: Option<&str>, hint: Option<&str>) -> Value {
    let mut about = format!("App: {}.", clip(title, 100));
    if let Some(d) = description.filter(|d| !d.trim().is_empty()) {
        about.push(' ');
        about.push_str(d.trim());
    }
    let mut asked = clip(&about, ABOUT_MAX_CHARS).to_string();
    if let Some(h) = hint {
        asked.push_str(ASKS);
        asked.push_str(h);
    }
    json!({
        "messages": [{ "role": "system", "content": SUBJECT_SYSTEM }, { "role": "user", "content": asked }],
        "max_tokens": SUBJECT_MAX_TOKENS,
        "reasoning_effort": "low",
    })
}

/// The subject GLM named (`{choices: [{message: {content}}]}`): its first
/// line, unquoted, without a last full stop; `None` when it named none
/// (an empty answer, one of its reasoning only, or one too long), and the
/// cell draws the hint, or the title, instead.
pub fn subject_of(response: &Value) -> Option<String> {
    let content = response["choices"][0]["message"]["content"].as_str()?;
    let line = content.trim().lines().next()?.trim();
    let line = line.trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == '*').trim().trim_end_matches('.').trim();
    (!line.is_empty() && line.chars().count() <= SUBJECT_MAX_CHARS).then(|| line.to_string())
}

/// The prompt a drawing of `subject` is made from.
pub fn prompt(subject: &str) -> String {
    format!("{STYLE}{OBJECT}{}.", clip(subject, SUBJECT_MAX_CHARS))
}

/// One drawing's input: its form's fields (the cell sends them as the
/// multipart form the model takes), `seed` making each of a making's
/// different.
pub fn image_input(subject: &str, seed: u32) -> Value {
    json!({ "prompt": prompt(subject), "width": DRAWN_PX, "height": DRAWN_PX, "seed": seed })
}

/// A drawing's cost: what Workers AI said it was (`cf-ai-neurons`, a
/// decimal count of neurons), or, when it said nothing readable,
/// `DRAWN_MILLI_NEURONS`.
pub fn neurons(said: Option<&str>) -> Usage {
    let milli = said.and_then(|s| s.trim().parse::<f64>().ok()).filter(|n| n.is_finite() && *n >= 0.0 && *n < 1e9).map_or(DRAWN_MILLI_NEURONS, |n| (n * 1000.0).ceil() as u64);
    Usage::Neurons { milli }
}

/// Why a drawing is not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flaw {
    /// Not a PNG `examine` reads (Images answered something else), or
    /// larger than `EXAMINE_MAX_PX` a side.
    Unreadable,
    /// Nothing, or almost nothing, was cut out.
    Empty,
    /// The object runs off the frame.
    CutOff,
    /// A filled square: a tile, a badge, a button.
    Tile,
    /// Several pieces: loose parts, scattered marks, a cutout gone wrong.
    Pieces,
    /// Soft, half-transparent edges: a cutout that found no clear object.
    Ragged,
    /// Clef read writing in it.
    Writing,
    /// Clef read a tile, a badge or a frame.
    Framed,
    /// Clef read no one object.
    NoObject,
    /// Clef read another subject.
    OffSubject,
    /// A step's call failed (said in the cell's log).
    Failed,
}

impl Flaw {
    pub const fn name(self) -> &'static str {
        match self {
            Flaw::Unreadable => "unreadable",
            Flaw::Empty => "empty",
            Flaw::CutOff => "cut_off",
            Flaw::Tile => "tile",
            Flaw::Pieces => "pieces",
            Flaw::Ragged => "ragged",
            Flaw::Writing => "writing",
            Flaw::Framed => "framed",
            Flaw::NoObject => "no_object",
            Flaw::OffSubject => "off_subject",
            Flaw::Failed => "failed",
        }
    }
}

/// The object's place in a cutout: its bounding box (opaque pixels), in
/// the cutout's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// A pixel counts as the object's from this alpha up.
const OPAQUE: u8 = 128;
/// Less than this share of the frame is nothing cut out (the spike's
/// objects covered 24 to 41%).
const COVER_MIN: f64 = 0.04;
/// A box this full and this square is a tile: the spike's flat square
/// filled 99% of its box (aspect 0.93); its objects 69 to 90%.
const TILE_FILL: f64 = 0.95;
const TILE_ASPECT: f64 = 0.85;
/// The largest piece's least share of the object, on a grid of
/// `BLOCK`-pixel cells: every object of the spike was one piece; its
/// broken cutout's largest was 43% of 12.
const MAIN_SHARE_MIN: f64 = 0.85;
const BLOCK: u32 = 4;
/// The most of the visible pixels that may be half transparent: the
/// spike's objects 2 to 6%, its broken cutout 18%.
const RAGGED_MAX: f64 = 0.12;

/// Reads a PNG's alpha: its width, height, and one alpha byte a pixel (255
/// for an image without alpha).
pub fn alpha_of(png_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), Flaw> {
    let mut decoder = png::Decoder::new_with_limits(std::io::Cursor::new(png_bytes), png::Limits { bytes: 4 * (EXAMINE_MAX_PX as usize).pow(2) });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| Flaw::Unreadable)?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0 || height == 0 || width > EXAMINE_MAX_PX || height > EXAMINE_MAX_PX {
        return Err(Flaw::Unreadable);
    }
    let mut buf = vec![0; reader.output_buffer_size().ok_or(Flaw::Unreadable)?];
    let frame = reader.next_frame(&mut buf).map_err(|_| Flaw::Unreadable)?;
    let (channels, alpha) = match frame.color_type {
        png::ColorType::Rgba => (4, Some(3)),
        png::ColorType::GrayscaleAlpha => (2, Some(1)),
        png::ColorType::Rgb => (3, None),
        png::ColorType::Grayscale => (1, None),
        // EXPAND makes an indexed image RGB or RGBA
        png::ColorType::Indexed => return Err(Flaw::Unreadable),
    };
    let line = frame.line_size;
    assert!(line >= width as usize * channels, "a row holds its pixels");
    let mut out = Vec::with_capacity(width as usize * height as usize);
    // bounded: width × height ≤ EXAMINE_MAX_PX²
    for y in 0..height as usize {
        let row = &buf[y * line..y * line + width as usize * channels];
        match alpha {
            Some(a) => out.extend(row.chunks_exact(channels).map(|px| px[a])),
            None => out.extend(std::iter::repeat_n(255, width as usize)),
        }
    }
    Ok((width, height, out))
}

/// Reads a cutout's alpha (`width` × `height`, one byte a pixel): where its
/// object is, or why it is not one an icon can be cropped from.
pub fn examine(width: u32, height: u32, alpha: &[u8]) -> Result<Shape, Flaw> {
    assert_eq!(alpha.len(), width as usize * height as usize, "one alpha byte a pixel");
    let (w, h) = (width as usize, height as usize);
    let (mut left, mut top, mut right, mut bottom) = (w, h, 0, 0);
    let (mut on, mut visible, mut soft) = (0u64, 0u64, 0u64);
    for (i, &a) in alpha.iter().enumerate() {
        if a > 16 {
            visible += 1;
            if a < 240 {
                soft += 1;
            }
        }
        if a >= OPAQUE {
            on += 1;
            let (x, y) = (i % w, i / w);
            (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x), bottom.max(y));
        }
    }
    let area = (w * h) as f64;
    if on == 0 || (on as f64) < COVER_MIN * area {
        return Err(Flaw::Empty);
    }
    if left == 0 || top == 0 || right == w - 1 || bottom == h - 1 {
        return Err(Flaw::CutOff);
    }
    let (bw, bh) = ((right - left + 1) as f64, (bottom - top + 1) as f64);
    if on as f64 / (bw * bh) >= TILE_FILL && bw.min(bh) / bw.max(bh) >= TILE_ASPECT {
        return Err(Flaw::Tile);
    }
    if main_share(width, height, alpha) < MAIN_SHARE_MIN {
        return Err(Flaw::Pieces);
    }
    if soft as f64 / visible as f64 > RAGGED_MAX {
        return Err(Flaw::Ragged);
    }
    let px = |n: usize| u32::try_from(n).expect("a side is at most EXAMINE_MAX_PX");
    Ok(Shape { width, height, left: px(left), top: px(top), right: px(right), bottom: px(bottom) })
}

/// The largest piece's share of the object, its pieces found on a grid of
/// `BLOCK`-pixel cells (a cell is the object's when any pixel of it is),
/// four-connected: a thin outline stays joined, a stray mark does not.
fn main_share(width: u32, height: u32, alpha: &[u8]) -> f64 {
    let (gw, gh) = (width.div_ceil(BLOCK) as usize, height.div_ceil(BLOCK) as usize);
    let mut grid = vec![false; gw * gh];
    for (i, &a) in alpha.iter().enumerate() {
        if a >= OPAQUE {
            let (x, y) = (i % width as usize / BLOCK as usize, i / width as usize / BLOCK as usize);
            grid[y * gw + x] = true;
        }
    }
    let total = grid.iter().filter(|c| **c).count();
    let (mut seen, mut largest, mut stack) = (vec![false; grid.len()], 0, Vec::new());
    // bounded: each cell is pushed at most once
    for start in 0..grid.len() {
        if !grid[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut size = 0;
        while let Some(c) = stack.pop() {
            size += 1;
            let (x, y) = (c % gw, c / gw);
            let near = [(x > 0).then(|| c - 1), (x + 1 < gw).then(|| c + 1), (y > 0).then(|| c - gw), (y + 1 < gh).then(|| c + gw)];
            for n in near.into_iter().flatten() {
                if grid[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        largest = largest.max(size);
    }
    largest as f64 / total.max(1) as f64
}

/// The square an icon is cut from, around its object (Images' `trim`):
/// `MARGIN` of its longer side clear on every side, held to the cutout;
/// Images pads it square, centred, when it fits it to `SIZE`.
pub fn crop(s: &Shape) -> Value {
    let side = (s.right - s.left + 1).max(s.bottom - s.top + 1);
    let margin = side * MARGIN_PCT / 100;
    let (left, top) = (s.left.saturating_sub(margin), s.top.saturating_sub(margin));
    let (right, bottom) = ((s.right + margin).min(s.width - 1), (s.bottom + margin).min(s.height - 1));
    json!({ "left": left, "top": top, "width": right - left + 1, "height": bottom - top + 1 })
}

/// The share of the object's longer side left clear around it.
const MARGIN_PCT: u32 = 6;

/// The transformations an icon is made with, after its cutout (and an
/// upload with, `trim` none): cropped, then fitted inside `SIZE`, padded
/// with transparency.
pub fn fit(trim: Option<Value>) -> Vec<Value> {
    let mut steps = Vec::with_capacity(2);
    if let Some(t) = trim {
        steps.push(json!({ "trim": t }));
    }
    steps.push(json!({ "width": SIZE, "height": SIZE, "fit": "pad", "background": "rgba(0,0,0,0)" }));
    steps
}

/// The questions Clef is asked of one icon (`decide::bound`'s request,
/// the icon flattened onto `BACKDROP` as a JPEG `data:` URL): a probability
/// each.
pub fn check(subject: &str, jpeg_data_url: &str) -> Value {
    json!({
        "model": CHECK_MODEL.name(),
        "input": "A candidate desktop icon for an app, shown on a plain blue background.",
        "questions": {
            "writing": { "type": "predicate", "instructions": "Does the icon contain any letters, words or numbers?" },
            "tile": { "type": "predicate", "instructions": "Is the icon a square or rounded-square tile, badge or button, or drawn inside one?" },
            "object": { "type": "predicate", "instructions": "Is the icon one recognisable physical object (or a tight group of objects), not a flat shape, a scene or a pattern?" },
            "subject": { "type": "predicate", "instructions": format!("Does the icon show {}?", clip(subject, SUBJECT_MAX_CHARS)) },
        },
        "media": [{ "data": jpeg_data_url }],
    })
}

/// Clef's answer to `check` (as `DecisionCall::answer` gives it): the
/// icon's score, higher better (how surely it shows one object of its
/// subject), or the flaw it read. Each question is a refusal past even
/// odds: the spike's good drawings were at most 0.27 for a tile and 0.02
/// for writing, at least 0.74 for an object and 0.88 for the subject.
pub fn judged(answer: &Value) -> Result<f64, Flaw> {
    let p = |q: &str| answer["answers"][q]["probability"].as_f64().filter(|p| (0.0..=1.0).contains(p)).ok_or(Flaw::Failed);
    let (writing, tile, object, subject) = (p("writing")?, p("tile")?, p("object")?, p("subject")?);
    if writing > 0.5 {
        return Err(Flaw::Writing);
    }
    if tile > 0.5 {
        return Err(Flaw::Framed);
    }
    if object < 0.5 {
        return Err(Flaw::NoObject);
    }
    if subject < 0.5 {
        return Err(Flaw::OffSubject);
    }
    Ok(object * subject * (1.0 - writing) * (1.0 - tile))
}

/// A making's or an upload's own name, `<started>.<8 hex>`: when it
/// started and a nonce of its own, so two that start in one millisecond
/// are two (the outbox keeps a reference once: a second row of the same
/// one would be dropped, its usage unbilled).
pub fn operation(started_ms: i64, nonce: [u8; 4]) -> String {
    format!("{started_ms}.{:08x}", u32::from_be_bytes(nonce))
}

/// The meter row's reference for one call of a making or an upload: the
/// fragment's meter key, the operation's name (`operation`), and the call
/// (`subject`, `draw-<n>`, `cut-<n>`, `fit-<n>`, `flat-<n>`, `check-<n>`,
/// `upload`).
pub fn meter_ref(key: &str, op: &str, call: &str) -> String {
    format!("icon:{key}:{op}:{call}")
}

/// Where a fragment's icon is served on the platform's origin: its
/// address names the PNG's SHA-256 (its first 12 digits), so a new icon
/// is a new address (as `face::picture_path`).
pub fn icon_path(name: &str, sha256: &str) -> String {
    assert!(sha256.len() >= 12 && sha256.is_char_boundary(12), "an icon is named by its SHA-256");
    format!("/api/f/{name}/icon?v={}", &sha256[..12])
}

/// At most `n` characters of `s`.
fn clip(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

#[cfg(test)]
mod tests;
