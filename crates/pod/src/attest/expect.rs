//! What RTMR1 and RTMR2 must hold after a correct boot of the pod image,
//! computed from the image's own bytes: its partition table and its one
//! EFI file, a unified kernel image (UKI) the firmware starts as
//! `\EFI\BOOT\BOOTX64.EFI` with no shim or boot loader in between.
//!
//! Each event is the firmware's, systemd-stub's or the Linux EFI stub's,
//! in the order they extend. The register is the fold of the events'
//! digests from zero (`replay::extend`). `Origin` says where an event's
//! bytes come from, so a client can see what the image fixes and what it
//! takes on trust.

use super::pe::{self, PeError};
use super::replay::extend;
use super::report::Mr;
use super::gpt::{self, GptError};
use sha2::{Digest, Sha384};
use std::fmt;

/// Where an expected event's bytes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A fixed string or value in the firmware or stub's code.
    Constant,
    /// The disk image's partition table.
    PartitionTable,
    /// The UKI file, or a section of it.
    Uki,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Constant => "constant",
            Self::PartitionTable => "partition table",
            Self::Uki => "UKI",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    pub rtmr: usize,
    pub origin: Origin,
    pub what: String,
    pub digest: [u8; 48],
}

#[derive(Debug)]
pub enum ExpectError {
    Pe(PeError),
    Gpt(GptError),
    MissingSection(&'static str),
    UnmodelledSection(&'static str),
}

impl fmt::Display for ExpectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pe(e) => write!(f, "{e}"),
            Self::Gpt(e) => write!(f, "{e}"),
            Self::MissingSection(s) => write!(f, "the UKI has no {s} section"),
            Self::UnmodelledSection(s) => write!(f, "the UKI has a {s} section, which this prediction does not model"),
        }
    }
}

impl std::error::Error for ExpectError {}

impl From<PeError> for ExpectError {
    fn from(e: PeError) -> Self {
        Self::Pe(e)
    }
}

impl From<GptError> for ExpectError {
    fn from(e: GptError) -> Self {
        Self::Gpt(e)
    }
}

fn sha384(b: &[u8]) -> [u8; 48] {
    Sha384::digest(b).into()
}

fn constant(rtmr: usize, what: &str, bytes: &[u8]) -> Expected {
    Expected { rtmr, origin: Origin::Constant, what: what.to_owned(), digest: sha384(bytes) }
}

/// The UKI sections systemd-stub measures into PCR 11 (RTMR2), in its
/// order (`unified_sections[]` without `.pcrsig`, which it skips): first
/// the section's name with its NUL, then the section as loaded. Absent
/// ones are skipped. Checked against image D's boot (stub 257): it
/// measured .linux, .osrel, .cmdline, .initrd, .uname and .sbat. Stub 257
/// also knows .ucode, .profile, .dtbauto, .hwids and .efifw, which this
/// image does not have; `expect` refuses a UKI that has any of them
/// rather than guess where they go.
pub const STUB_SECTIONS: [&str; 9] =
    [".linux", ".osrel", ".cmdline", ".initrd", ".splash", ".dtb", ".uname", ".sbat", ".pcrpkey"];

/// RTMR1 (PCRs 2 to 6): the firmware's boot of the UKI.
pub fn rtmr1(uki: &[u8], disk_head: &[u8]) -> Result<Vec<Expected>, ExpectError> {
    let pe = pe::parse(uki)?;
    let linux = pe.section_in_memory(".linux")?.ok_or(ExpectError::MissingSection(".linux"))?;
    let kernel = pe::parse(&linux)?;
    Ok(vec![
        constant(1, "EV_EFI_ACTION Calling EFI Application from Boot Option", b"Calling EFI Application from Boot Option"),
        constant(1, "EV_SEPARATOR", &[0; 4]),
        Expected {
            rtmr: 1,
            origin: Origin::PartitionTable,
            what: "EV_EFI_GPT_EVENT: the GPT header and its used entries".to_owned(),
            digest: sha384(&gpt::event_data(disk_head, 512)?),
        },
        Expected {
            rtmr: 1,
            origin: Origin::Uki,
            what: "EV_EFI_BOOT_SERVICES_APPLICATION: Authenticode hash of BOOTX64.EFI (the UKI)".to_owned(),
            digest: pe.authenticode_sha384()?,
        },
        Expected {
            rtmr: 1,
            origin: Origin::Uki,
            what: "EV_EFI_BOOT_SERVICES_APPLICATION: Authenticode hash of the kernel in .linux, which the stub loads"
                .to_owned(),
            digest: kernel.authenticode_sha384()?,
        },
        constant(1, "EV_EFI_ACTION Exit Boot Services Invocation", b"Exit Boot Services Invocation"),
        constant(
            1,
            "EV_EFI_ACTION Exit Boot Services Returned with Success",
            b"Exit Boot Services Returned with Success",
        ),
    ])
}

/// systemd-stub's major version, from the `#### LoaderInfo: systemd-stub
/// <version> ####` string in its `.sdmagic` section.
pub fn stub_version(uki: &[u8]) -> Option<u32> {
    let pe = pe::parse(uki).ok()?;
    let magic = pe.section_in_memory(".sdmagic").ok()??;
    let text = String::from_utf8_lossy(&magic);
    let rest = text.split("systemd-stub ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// RTMR2 (PCRs 8 to 15): what systemd-stub and the kernel's EFI stub
/// measure. systemd-stub before 257 measures into a TPM if there is one
/// and never into the TDX RTMRs; a GCE TDX guest has Google's vTPM (the
/// kernel finds an MSFT0101 TPM 2.0), which the host runs, outside the
/// TD. So with a 255 stub nothing extends RTMR2: the UKI's sections, and
/// any command line handed in by the boot entry, go only to the vTPM. The
/// Linux EFI stub also prefers the TPM, so it adds nothing either. Our
/// boots' logs show RTMR2 empty. systemd-stub 257 measures into both.
pub fn rtmr2(uki: &[u8]) -> Result<Vec<Expected>, ExpectError> {
    let pe = pe::parse(uki)?;
    let mut out = Vec::new();
    if stub_version(uki).is_none_or(|v| v < 257) {
        return Ok(out);
    }
    for unknown in UNMODELLED_SECTIONS {
        if pe.section(unknown).is_some() {
            return Err(ExpectError::UnmodelledSection(unknown));
        }
    }
    for name in STUB_SECTIONS {
        let Some(bytes) = pe.section_in_memory(name)? else { continue };
        let mut with_nul = name.as_bytes().to_vec();
        with_nul.push(0);
        out.push(Expected { rtmr: 2, origin: Origin::Constant, what: format!("section name {name}"), digest: sha384(&with_nul) });
        out.push(Expected { rtmr: 2, origin: Origin::Uki, what: format!("section {name}"), digest: sha384(&bytes) });
    }
    Ok(out)
}

/// Sections a 257 stub would also measure, whose place in the order this
/// code has not been checked against a real boot.
const UNMODELLED_SECTIONS: [&str; 5] = [".ucode", ".profile", ".dtbauto", ".hwids", ".efifw"];

pub fn fold(events: &[Expected], rtmr: usize) -> Mr {
    events.iter().filter(|e| e.rtmr == rtmr).fold([0u8; 48], |mr, e| extend(&mr, &e.digest))
}
