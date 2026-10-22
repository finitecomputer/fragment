//! The browser preview cards are shot with off Cloudflare (docs/self-host.md,
//! seam 7), pinned: chrome-headless-shell from Chrome for Testing, its
//! version and the SHA-256 of each platform's zip. Nothing else lives in
//! this file, so a cache of the unpacked browser (`target/tools`) can key
//! on it alone.
//!
//! Moving the pin: the version from Chrome for Testing's
//! last-known-good-versions-with-downloads.json; each platform's zip
//! fetched, its MD5 checked against the `x-goog-hash` the bucket answers
//! for it, and its SHA-256 recorded below.

/// Chrome for Testing's Stable on 2026-10-22 (its
/// last-known-good-versions-with-downloads.json, revision 1706539).
pub const CHROME_VERSION: &str = "156.0.8078.47";

/// One platform's zip: `chrome-headless-shell-<platform>.zip` at
/// https://storage.googleapis.com/chrome-for-testing-public/<CHROME_VERSION>/<platform>/.
#[derive(Debug, PartialEq, Eq)]
pub struct Zip {
    /// Chrome for Testing's name for the platform (`mac-arm64`).
    pub platform: &'static str,
    pub sha256: &'static str,
}

/// Chrome for Testing publishes no checksums. Each hash is of the zip
/// fetched on 2026-10-22, whose MD5 matched the `x-goog-hash` Google's
/// bucket answers for it.
pub const ZIPS: [Zip; 4] = [
    Zip { platform: "linux64", sha256: "0d5acc62c3e213f8c87d3c298d66ffcf9a236d12ea4b9f85318bf85dbc279daa" },
    Zip { platform: "linux-arm64", sha256: "955059fa7020d7be85eb503e21339fd4d718f51f5996d3b60633fbf908bda132" },
    Zip { platform: "mac-arm64", sha256: "2fd1c09a3e1ebde9e69e6aba92f1dd3f21ad247b55d9bf66e39213ff4205436d" },
    Zip { platform: "mac-x64", sha256: "5e230044c6ac989597f4a4e301a0a710d257e07d6f7754754da2bf62ea3d272c" },
];
