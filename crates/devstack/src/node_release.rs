//! The Node.js release the repo's JavaScript runs on, pinned: its version
//! and the SHA-256 of each platform's official tarball. Nothing else lives
//! in this file, so CI's cache of the unpacked Node (`target/tools`) keys
//! on it alone. Moving the pin: README.md, "The pinned Node".

/// Node 24 ("Krypton"), the Active LTS line since 2025-10-28 and supported
/// to 2028-04-30 (nodejs/Release's schedule): wrangler 4.145 asks for 22
/// or later (its `engines`); 22 has been in maintenance since 2025-10-21
/// and ends 2027-04-30; 26 is no LTS line before 2026-10-28.
pub const NODE_VERSION: &str = "24.22.1";

/// One platform's official tarball: `node-v<NODE_VERSION>-<platform>.tar.gz`
/// at https://nodejs.org/dist/v<NODE_VERSION>/.
#[derive(Debug, PartialEq, Eq)]
pub struct Tarball {
    /// Node's name for the platform (`darwin-arm64`), as in the file name.
    pub platform: &'static str,
    /// Its line in that release's SHASUMS256.txt.
    pub sha256: &'static str,
}

/// From https://nodejs.org/dist/v24.22.1/SHASUMS256.txt (2026-10-21's security
/// release: the Brotli decoder RCE, the TLS hostname bypasses and the rest of
/// nodejs.org's "October 2026 Security Releases"), whose signature
/// (SHASUMS256.txt.sig) checked good against the releaser's key
/// 5BE8A3F6C8A5C01D106C0AD820B1A390B168D356, one nodejs/node's README lists.
pub const TARBALLS: [Tarball; 4] = [
    Tarball { platform: "darwin-arm64", sha256: "89c682593d76cd44233fa53b1563264e7de9cbb189cad169135c5b4fadfbc0a7" },
    Tarball { platform: "darwin-x64", sha256: "d132557e7ee2b85af08b0c8c34338e65c6430f2238d01f727728ef42dc83b035" },
    Tarball { platform: "linux-arm64", sha256: "33c37be234ba8bee041961047b19160f4d7d0f164e8e246f78bdd10feff148b3" },
    Tarball { platform: "linux-x64", sha256: "eb6728e77c576a35162c122c45ccadf224d89ff00abc8dae46cc2897c6002d7c" },
];
