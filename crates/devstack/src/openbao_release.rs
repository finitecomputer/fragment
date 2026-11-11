//! The secrets service a self-hosted deployment reads its own secrets from
//! (docs/self-host.md, seam 12), pinned: OpenBao, its version and the
//! SHA-256 of each platform's release tarball. Nothing else lives in this
//! file, so a cache of the unpacked binary (`target/tools`) can key on it
//! alone.
//!
//! Moving the pin: the release's `checksums.txt` and its `.gpgsig`, the
//! signature checked against OpenBao's key (openbao.org/docs/install,
//! `openbao-gpg-pub-20240618.asc`, primary 66D1 5FDD 8728 7219 C8E1 5478
//! D200 CD70 2853 E6D0), then each tarball's line below.

/// OpenBao 2.7.2 (2026-10-26): the static seal (2.4) and declarative
/// self-initialization (2.4) are in it; 2.7 removed the `file` storage
/// backend, whose successor for one server is `pebbledb`.
pub const OPENBAO_VERSION: &str = "2.7.2";

/// One platform's tarball: `openbao_<OPENBAO_VERSION>_<platform>.tar.gz` at
/// https://github.com/openbao/openbao/releases/download/v<OPENBAO_VERSION>/.
#[derive(Debug, PartialEq, Eq)]
pub struct Tarball {
    /// OpenBao's name for the platform (`linux_amd64`), as in the file name.
    pub platform: &'static str,
    /// Its line in that release's checksums.txt.
    pub sha256: &'static str,
}

/// From https://github.com/openbao/openbao/releases/download/v2.7.2/checksums.txt,
/// whose signature (checksums.txt.gpgsig, by subkey E617 DCD4 065C 2AFC
/// 0B2C F7A7 BA8B C08C 0F69 1F94 of the primary key above, fetched from
/// openbao.org and identical to the release's copy) checked good on
/// 2026-11-11, when 2.7.2 was the newest 2.x.
pub const TARBALLS: [Tarball; 4] = [
    Tarball { platform: "darwin_amd64", sha256: "ea62ce00b81a2170a5daa8589f8420434e72d4d4ebcd2a4cbfa9add507a2602d" },
    Tarball { platform: "darwin_arm64", sha256: "2b4fc131910f4b075b2e1c9a9de67c461489491a8f03fbdc73387294d67d9547" },
    Tarball { platform: "linux_amd64", sha256: "582b1f3c2fdc5afd443b2d57a225ef654de75adea609fdb66731fd582a9cbc96" },
    Tarball { platform: "linux_arm64", sha256: "5b97bf816fa820e5e521abec1db19c6161fcca2f666cf4a84e335089c0e83edd" },
];
