//! A fragment app's preview card (docs/cloudflare-v1.md, decision 31):
//! after each move of `live`, the platform renders the app's page with
//! Browser Rendering, as a visitor without an account sees it, and keeps
//! the image as one of the fragment's blobs, which `GET
//! /api/f/{name}/card` serves its members (docs/api.md, Cards).
//!
//! This is the card's pure part, run by the cell (cell/src/card.rs):
//!
//! - who is shot, and as whom (`visitor`): an app or a brain, never a
//!   chat or an agent (they are not in the shell's Apps list); a public
//!   fragment as anyone sees it, a link fragment as anyone holding its
//!   share link sees it, and a members-only fragment not at all (an
//!   anonymous visitor gets its refusal, which says nothing of the app);
//! - what a shot must be (`check_image`): a JPEG of `WIDTH` × `HEIGHT`, at
//!   most `BYTES_MAX`;
//! - its meter's reference (`meter_ref`) and billed time (`billed_ms`);
//! - the schedule (`Cards`): the fragment's alarm takes each try itself,
//!   so one is out at a time (a Durable Object runs one alarm at a time);
//!   the newest live wins (a shot of an older one is dropped as it lands);
//!   a try counts as failed from when it begins until it lands, so one
//!   the object lost mid-shot (a restart) is a failed try; a failure is
//!   tried again after a wait that doubles, at most `ATTEMPTS_MAX` tries
//!   in all, and then given up quietly: the card before stays, and
//!   nothing else changes;
//! - its content's first change after a live's shot (`Cards::content_moved`,
//!   FIN-368): the app's files moved (main) while live stayed, as a brain's
//!   pages arrive after its first, empty shot; the same live is shot once
//!   more, once a live, so a busy app is never shot over and over;
//! - an editor's two asks (docs/api.md, Cards): a shot of the live again
//!   (`Cards::again`), at most one ask each `AGAIN_EVERY_MS`, joining a
//!   shot already wanted; and an image of their own as the card
//!   (`Cards::upload`), held to what a shot is held to, which shots leave
//!   (they still load the page, for its report) until a shot is asked for
//!   again;
//! - what the page reported as it loaded (`Heard`): its uncaught
//!   exceptions, console errors, failed loads and security refusals, the
//!   first `ERRORS_MAX` kept (the rest counted), each cut to
//!   `ERROR_BYTES_MAX`; the try that ends its live's tries makes it the
//!   page's report (`Cards::report`), which status shows.

mod heard;
mod image;
mod limits;
mod meter;
mod schedule;
#[cfg(test)]
mod testing;
mod visit;

pub use heard::{cut, summary, Heard, HIDDEN_TOKEN};
pub use image::{check_image, check_upload, ImageFault};
pub use limits::{AGAIN_EVERY_MS, ATTEMPTS_MAX, BYTES_MAX, ERRORS_MAX, ERROR_BYTES_MAX, HEIGHT, KEEP_ALIVE_MS, LOAD_TIMEOUT_MS, MEDIA_TYPE, QUALITIES, SCALE, SETTLE_MS, SHOT_TIMEOUT_MS, VIEWPORT_HEIGHT, VIEWPORT_WIDTH, WIDTH};
pub use meter::{billed_ms, meter_ref};
pub use schedule::{valid_live, Asked, Card, Cards, Fault, Next, Outcome, Pace, Shot, Verdict, Wanted};
pub use visit::{is_app, landed, visit, visitor, Elsewhere, Skip, Visit, Visitor};
