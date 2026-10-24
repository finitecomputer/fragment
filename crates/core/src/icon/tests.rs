use base64::Engine;

use super::*;

/// Real cutouts from the spike (2026-10-24: FLUX.2 [klein] 4B, then
/// Images' `segment: foreground`), shrunk: two objects drawn as asked,
/// the flat square an abstract name was drawn as, and the loose rays a
/// tiled drawing's cutout left (an indexed PNG, as Images may answer).
const APPLE: &[u8] = include_bytes!("fixtures/apple.png");
const NOTEPAD: &[u8] = include_bytes!("fixtures/notepad.png");
const SQUARE: &[u8] = include_bytes!("fixtures/square.png");
const SCATTERED: &[u8] = include_bytes!("fixtures/scattered.png");

fn examined(png: &[u8]) -> Result<Shape, Flaw> {
    let (w, h, alpha) = alpha_of(png)?;
    examine(w, h, &alpha)
}

/// A `side`² alpha, opaque where `on` says.
fn drawn(side: u32, on: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    (0..side * side).map(|i| on(i % side, i / side)).collect()
}

fn disc(cx: f64, cy: f64, r: f64) -> impl Fn(u32, u32) -> bool {
    move |x, y| (x as f64 - cx).powi(2) + (y as f64 - cy).powi(2) <= r * r
}

fn png_of(width: u32, height: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, width, height);
    e.set_color(color);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(data).unwrap();
    out
}

/// Valid: the spike's objects are offered, each box inside its frame.
#[test]
fn a_drawn_object_is_offered_with_its_box() {
    for (name, png) in [("apple", APPLE), ("notepad", NOTEPAD)] {
        let s = examined(png).unwrap_or_else(|f| panic!("{name}: {f:?}"));
        assert!(s.left > 0 && s.top > 0 && s.right < s.width - 1 && s.bottom < s.height - 1, "{name}: {s:?}");
    }
}

/// Invalid: the flat square is a tile, the loose rays are pieces.
#[test]
fn the_spikes_failures_are_refused_for_what_they_are() {
    assert_eq!(examined(SQUARE), Err(Flaw::Tile));
    assert_eq!(examined(SCATTERED), Err(Flaw::Pieces));
}

/// Each flaw of a cutout, drawn: nothing, a speck, an object off the
/// frame, a square, a rounded square, two objects apart, a haze; and a
/// disc that is none of them, its outline thin enough to test the grid.
#[test]
fn each_flaw_of_a_cutout_is_named() {
    let side = 128;
    let at = |on: &dyn Fn(u32, u32) -> bool| drawn(side, |x, y| if on(x, y) { 255 } else { 0 });
    let ok = disc(64.0, 64.0, 40.0);
    assert!(examine(side, side, &at(&ok)).is_ok());
    let ring = |x, y| disc(64.0, 64.0, 40.0)(x, y) && !disc(64.0, 64.0, 38.0)(x, y);
    assert!(examine(side, side, &at(&ring)).is_err_and(|f| f == Flaw::Empty), "a thin ring alone is too little");
    let ringed = |x, y| ring(x, y) || disc(64.0, 64.0, 20.0)(x, y) || (x == 64 && (24..=44).contains(&y)) || (y == 64 && (24..=44).contains(&x));
    assert!(examine(side, side, &at(&ringed)).is_ok(), "a thin outline joined to its object is one piece");
    assert_eq!(examine(side, side, &vec![0; (side * side) as usize]), Err(Flaw::Empty));
    assert_eq!(examine(side, side, &at(&disc(64.0, 64.0, 4.0))), Err(Flaw::Empty));
    assert_eq!(examine(side, side, &at(&disc(10.0, 64.0, 30.0))), Err(Flaw::CutOff));
    assert_eq!(examine(side, side, &at(&|x, y| (20..108).contains(&x) && (24..104).contains(&y))), Err(Flaw::Tile));
    let rounded = |x: u32, y: u32| {
        let (dx, dy) = ((x as f64 - 64.0).abs() - 34.0, (y as f64 - 64.0).abs() - 34.0);
        dx.max(0.0).hypot(dy.max(0.0)) <= 6.0
    };
    assert_eq!(examine(side, side, &at(&rounded)), Err(Flaw::Tile), "an app tile's rounded corners");
    let tall = |x, y| (44..84).contains(&x) && (8..120).contains(&y);
    assert!(examine(side, side, &at(&tall)).is_ok(), "a full box that is not square is an object (a phone, a door)");
    let two = |x, y| disc(36.0, 64.0, 22.0)(x, y) || disc(94.0, 64.0, 22.0)(x, y);
    assert_eq!(examine(side, side, &at(&two)), Err(Flaw::Pieces));
    let haze = drawn(side, |x, y| if ok(x, y) { if (x + y) % 3 == 0 { 255 } else { 160 } } else { 0 });
    assert_eq!(examine(side, side, &haze), Err(Flaw::Ragged));
}

/// A PNG's alpha is read from each of its colour types; one without alpha
/// is opaque everywhere (an upload of a photograph: no object to crop);
/// what is no PNG, or too large a side, is unreadable.
#[test]
fn alpha_is_read_from_each_png_and_nothing_else() {
    let (w, h) = (3, 2);
    let rgba: Vec<u8> = (0..6u8).flat_map(|i| [9, 9, 9, i * 40]).collect();
    assert_eq!(alpha_of(&png_of(w, h, png::ColorType::Rgba, &rgba)), Ok((3, 2, vec![0, 40, 80, 120, 160, 200])));
    let ga: Vec<u8> = (0..6u8).flat_map(|i| [7, 255 - i]).collect();
    assert_eq!(alpha_of(&png_of(w, h, png::ColorType::GrayscaleAlpha, &ga)).unwrap().2, vec![255, 254, 253, 252, 251, 250]);
    assert_eq!(alpha_of(&png_of(w, h, png::ColorType::Rgb, &[1; 18])).unwrap().2, vec![255; 6]);
    assert_eq!(alpha_of(&png_of(w, h, png::ColorType::Grayscale, &[1; 6])).unwrap().2, vec![255; 6]);
    let photo = png_of(64, 64, png::ColorType::Rgb, &[200; 64 * 64 * 3]);
    assert_eq!(examined(&photo), Err(Flaw::CutOff));
    assert_eq!(alpha_of(b"\xFF\xD8\xFF\xE0 a jpeg"), Err(Flaw::Unreadable));
    assert_eq!(alpha_of(&APPLE[..APPLE.len() / 2]), Err(Flaw::Unreadable), "a PNG cut short");
    let wide = png_of(EXAMINE_MAX_PX + 1, 1, png::ColorType::Grayscale, &vec![0; EXAMINE_MAX_PX as usize + 1]);
    assert_eq!(alpha_of(&wide), Err(Flaw::Unreadable));
}

/// The crop keeps a margin around the object, held to the cutout.
#[test]
fn the_crop_keeps_a_margin_inside_the_cutout() {
    let s = Shape { width: 512, height: 512, left: 100, top: 150, right: 299, bottom: 349 };
    assert_eq!(crop(&s), json!({ "left": 88, "top": 138, "width": 224, "height": 224 }));
    let near = Shape { width: 512, height: 512, left: 3, top: 5, right: 505, bottom: 300 };
    assert_eq!(crop(&near), json!({ "left": 0, "top": 0, "width": 512, "height": 331 }));
    assert_eq!(fit(Some(crop(&s)))[0]["trim"]["width"], 224);
    assert_eq!(fit(None), vec![json!({ "width": 256, "height": 256, "fit": "pad", "background": "rgba(0,0,0,0)" })]);
}

/// Clef's answers as the spike got them: the four good drawings are kept
/// in order of how surely they show their subject; the square, the loose
/// rays and the newspaper are refused for what Clef read; an answer
/// missing a question is a failure, never a pass.
#[test]
fn clefs_answers_keep_the_good_and_name_the_rest() {
    let answer = |w: f64, t: f64, o: f64, s: f64| {
        let p = |p: f64| json!({ "type": "predicate", "probability": p });
        json!({ "answers": { "writing": p(w), "tile": p(t), "object": p(o), "subject": p(s) } })
    };
    let todo = judged(&answer(0.0133, 0.1006, 0.8307, 0.9376)).unwrap();
    let inbox = judged(&answer(0.0057, 0.169, 0.8269, 0.8805)).unwrap();
    assert!(todo > inbox && inbox > 0.5);
    assert_eq!(judged(&answer(0.0056, 0.8501, 0.0577, 0.025)), Err(Flaw::Framed));
    assert_eq!(judged(&answer(0.0062, 0.2388, 0.307, 0.2395)), Err(Flaw::NoObject));
    assert_eq!(judged(&answer(0.9525, 0.1575, 0.6066, 0.9283)), Err(Flaw::Writing));
    assert_eq!(judged(&answer(0.01, 0.1, 0.9, 0.2)), Err(Flaw::OffSubject));
    let mut short = answer(0.0, 0.0, 1.0, 1.0);
    short["answers"].as_object_mut().unwrap().remove("subject");
    assert_eq!(judged(&short), Err(Flaw::Failed));
    assert_eq!(judged(&answer(0.0, 0.0, 1.0, 1.5)), Err(Flaw::Failed));
}

/// The checks' request is one `decide` takes from an agent (inline media),
/// on the cheapest model, asking the four questions `judged` reads.
#[test]
fn the_check_is_a_decision_decide_takes() {
    let url = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0]));
    let call = decide::bound(&check("a red apple", &url), decide::Asker::Agent).expect("a decision");
    assert_eq!(call.model, decide::Model::ClefFlash);
    let wire = call.input(vec![]);
    assert_eq!(wire["images"].as_array().map(Vec::len), Some(1));
    for q in ["writing", "tile", "object", "subject"] {
        assert_eq!(wire["questions"][q]["type"], "noul", "{q}");
    }
    assert!(wire["questions"]["subject"]["instructions"].as_str().unwrap().contains("a red apple"));
}

/// The subject: GLM's first line, unquoted and unstopped; nothing named
/// (an empty answer, as GLM gave once while reasoning past its tokens, or
/// a paragraph) is `None`.
#[test]
fn a_subject_is_one_short_line() {
    let said = |s: &str| json!({ "choices": [{ "message": { "content": s, "reasoning_content": "…" } }] });
    assert_eq!(subject_of(&said("A pair of red dice beside a small blue cup")).as_deref(), Some("A pair of red dice beside a small blue cup"));
    assert_eq!(subject_of(&said("\"a keyboard keycap.\"\n\nIt suits a hackathon.")).as_deref(), Some("a keyboard keycap"));
    assert_eq!(subject_of(&said("**a brass key**")).as_deref(), Some("a brass key"));
    assert_eq!(subject_of(&said("")), None);
    assert_eq!(subject_of(&said(&"word ".repeat(40))), None);
    assert_eq!(subject_of(&json!({ "choices": [{ "message": { "content": null } }] })), None);
    assert_eq!(subject_of(&json!({ "error": "x" })), None);
}

/// The subject's question names the app, its description, and the hint;
/// a long description is clipped, never refused.
#[test]
fn the_subject_question_carries_the_app_and_the_hint() {
    let q = subject_input("dice", Some("Roll dice with friends."), Some("make it purple"));
    let asked = q["messages"][1]["content"].as_str().unwrap();
    assert!(asked.starts_with("App: dice. Roll dice with friends.") && asked.ends_with("Its maker asks for: make it purple"), "{asked}");
    assert_eq!(q["reasoning_effort"], "low");
    let long = subject_input("x", Some(&"é".repeat(5000)), None);
    assert!(long["messages"][1]["content"].as_str().unwrap().chars().count() <= ABOUT_MAX_CHARS);
    assert!(image_input("a brass key", 7)["prompt"].as_str().unwrap().ends_with("The object: a brass key."));
}

/// Valid and invalid hints, and the hour's makings: the tenth in an hour
/// starts, the eleventh waits until the first is an hour old.
#[test]
fn hints_and_the_hours_makings() {
    assert_eq!(hint(&json!({})), Ok(None));
    assert_eq!(hint(&json!({ "hint": "  " })), Ok(None));
    assert_eq!(hint(&json!({ "hint": " a rocket " })), Ok(Some("a rocket".into())));
    assert_eq!(hint(&json!({ "hint": "x".repeat(HINT_MAX_CHARS + 1) })), Err(Refusal::Hint));
    assert_eq!(hint(&json!({ "hint": 3 })), Err(Refusal::Hint));
    let mut started = vec![];
    for i in 0..MAKINGS_PER_HOUR as i64 {
        started = admit(&started, 1_000 + i).unwrap();
    }
    assert_eq!(admit(&started, 2_000), Err(Refusal::Busy));
    let later = admit(&started, 1_000 + HOUR_MS).unwrap();
    assert_eq!(later.len(), MAKINGS_PER_HOUR, "the first hour-old start dropped, this one kept");
}

/// A drawing is charged what Workers AI said, or the measured price.
#[test]
fn a_drawing_is_charged_what_workers_ai_said() {
    assert_eq!(neurons(Some("104.20")), Usage::Neurons { milli: 104_200 });
    assert_eq!(neurons(Some(" 26.0501 ")), Usage::Neurons { milli: 26_051 });
    for unknown in [None, Some(""), Some("lots"), Some("-1"), Some("NaN"), Some("1e12")] {
        assert_eq!(neurons(unknown), Usage::Neurons { milli: DRAWN_MILLI_NEURONS }, "{unknown:?}");
    }
    assert!(neurons(None).validate().is_ok());
    let r = meter_ref("app--ab12@1729000000000", 1_729_000_001_000, "draw-3");
    assert!(crate::price::printable(&r, crate::ledger::ID_MAX_BYTES), "{r}");
    assert_eq!(icon_path("app--ab12", &"ab".repeat(32)), "/api/f/app--ab12/icon?v=abababababab");
}
