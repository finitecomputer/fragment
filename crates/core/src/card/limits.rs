//! A card's bounds: its image, its timings, and what a page's report keeps.

/// The saved image's size. At 200% zoom the page lays out in half as
/// many CSS pixels, rendered at twice the device scale: legible cards
/// with the same image size and upload contract.
pub const WIDTH: u16 = 1280;
pub const HEIGHT: u16 = 800;
pub const SCALE: u8 = 2;
pub const VIEWPORT_WIDTH: u16 = WIDTH / SCALE as u16;
pub const VIEWPORT_HEIGHT: u16 = HEIGHT / SCALE as u16;
const _: () = assert!(VIEWPORT_WIDTH * SCALE as u16 == WIDTH && VIEWPORT_HEIGHT * SCALE as u16 == HEIGHT);
/// What a card is stored and served as.
pub const MEDIA_TYPE: &str = "image/jpeg";
/// The JPEG qualities a shot tries, in turn: the next when the image is
/// over `BYTES_MAX`.
pub const QUALITIES: [u8; 2] = [70, 40];
/// A card's largest size. Its base64 (4/3 of it) stays under the 1 MiB a
/// Workers WebSocket message has held, so a shot crosses as one message.
pub const BYTES_MAX: usize = 512 * 1024;
/// Tries at one live's card, the first included: four retries, as a job
/// step gets.
pub const ATTEMPTS_MAX: u32 = 5;
/// One shot's deadline, from acquiring the browser to the image.
pub const SHOT_TIMEOUT_MS: u64 = 45_000;
/// A page's load event is waited for at most this; then it is shot as it is.
pub const LOAD_TIMEOUT_MS: u64 = 15_000;
/// After its load, a page's scripts render for this long before the shot
/// (an app reads its data over its socket after the load).
pub const SETTLE_MS: u64 = 1_000;
/// The browser session's inactivity timeout, Browser Rendering's least
/// (`keep_alive`): a session whose close was lost ends this long after its
/// last use, and is billed for it (`billed_ms`).
pub const KEEP_ALIVE_MS: u64 = 10_000;
/// A shot asked for again (`Cards::again`) is asked at most once this
/// often per fragment: each is a browser session from the account's few,
/// which every fragment's deploys share, billed to its owner. A deploy's
/// shot is not counted: it comes with the deploy.
pub const AGAIN_EVERY_MS: i64 = 60_000;

/// A shot keeps the first this many errors the page reports, and counts
/// the rest.
pub const ERRORS_MAX: usize = 10;
/// An error's text, and its source, are each cut to this many bytes.
pub const ERROR_BYTES_MAX: usize = 1024;

const _: () = assert!(BYTES_MAX.div_ceil(3) * 4 < 1024 * 1024, "a card's base64 fits one 1 MiB WebSocket message");
// `cut` leaves no control character but a newline or a tab, so each byte
// is at most two in JSON; the summary is at most an error's text and 100 bytes
const _: () = assert!(
    (ERRORS_MAX + 1) * (2 * 2 * ERROR_BYTES_MAX + 100) < fragment_proto::limits::RECORD_BODY_MAX_BYTES,
    "a report is one `page.errors` event"
);
const _: () = assert!(LOAD_TIMEOUT_MS + SETTLE_MS < SHOT_TIMEOUT_MS, "a slow page is still shot within the deadline");
