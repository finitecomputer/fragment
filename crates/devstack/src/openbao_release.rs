//! The secrets service a self-hosted deployment reads its own secrets from
//! (docs/self-host.md, seam 12), pinned: OpenBao, its version and the
//! SHA-256 of each platform's release tarball. Nothing else lives in this
//! file, so a cache of the unpacked binary (`target/tools`) can key on it
//! alone.
//!
//! Moving the pin: the release's `checksums.txt` and its `.gpgsig`, the
//! signature checked against OpenBao's key (openbao.org/docs/install,
//! `openbao-gpg-pub-20240618.asc`, primary 66D1 5FDD 8728 7219 C8E1 5478
//! D200 CD70 2853 E6D0), then each tarball's line below; then each
//! tarball fetched, checked against its line, and the SHA-256 of the `bao`
//! it holds (`openbao::binary_from`'s bytes) pinned beside it. The cache's
//! `bao` is checked against that before anything runs it.

/// OpenBao 2.7.3 (2026-11-19), a security release (GHSA-fgmw-h9wp-wc7g,
/// GHSA-h8fh-pmch-cjww): the static seal (2.4) and declarative
/// self-initialization (2.4) are in it; 2.7 removed the `file` storage
/// backend, whose successor for one server is `pebbledb`.
pub const OPENBAO_VERSION: &str = "2.7.3";

/// One platform's tarball: `openbao_<OPENBAO_VERSION>_<platform>.tar.gz` at
/// https://github.com/openbao/openbao/releases/download/v<OPENBAO_VERSION>/.
#[derive(Debug, PartialEq, Eq)]
pub struct Tarball {
    /// OpenBao's name for the platform (`linux_amd64`), as in the file name.
    pub platform: &'static str,
    /// Its line in that release's checksums.txt.
    pub sha256: &'static str,
    /// The SHA-256 of the `bao` it holds, from the verified tarball: what
    /// the cached binary must be before it is run.
    pub bao_sha256: &'static str,
}

/// From https://github.com/openbao/openbao/releases/download/v2.7.3/checksums.txt,
/// whose signature (checksums.txt.gpgsig, by subkey E617 DCD4 065C 2AFC
/// 0B2C F7A7 BA8B C08C 0F69 1F94 of the primary key above, fetched from
/// openbao.org and identical to the release's copy) checked good on
/// 2026-11-24, when 2.7.3 was the newest 2.x. Each `bao_sha256` from that
/// tarball, fetched and checked against its line, the same day.
pub const TARBALLS: [Tarball; 4] = [
    Tarball {
        platform: "darwin_amd64",
        sha256: "fff61d45dd5ca7bbae8668fc07e9d6c7a6662e94e5cdf729b798a8cfcb3a8196",
        bao_sha256: "785f501279c33968b1ee6b7237c2014ee0a015d95cd8ff98bbabbd7b36abfd17",
    },
    Tarball {
        platform: "darwin_arm64",
        sha256: "1189f85499b36c0134e8e6bf0bffe71dfe1b085ab5d223324a5bccc2a59d62f7",
        bao_sha256: "41af7f5962d454fc61d0967e6f2b77de46a579a3d07d71b6d657027b0e8d1745",
    },
    Tarball {
        platform: "linux_amd64",
        sha256: "131df9a21cf3dd2dff47107110b369c9974cbdaa482bcbc723b81b3403c68ecd",
        bao_sha256: "3bc86bbae4adaa1c79bb84670a09d4add3a4a4533d8f7eae95e23127a64a6ff8",
    },
    Tarball {
        platform: "linux_arm64",
        sha256: "b505b07af4206fdf0e81b747fcb7e9e63f2c2b05e26ead973cce62d0ed07d0e0",
        bao_sha256: "9b57f890442e5703965585ab8eff2d5d42448af468a5fcc16510f464e3a51871",
    },
];
