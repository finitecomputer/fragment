//! Short human descriptions of boot events, and what each event's
//! digest covers. Event types: TCG PC Client Platform Firmware Profile,
//! table 9; structures: UEFI_VARIABLE_DATA, UEFI_IMAGE_LOAD_EVENT and
//! UEFI_PLATFORM_FIRMWARE_BLOB2 from the same spec, and GRUB's and
//! shim's EV_IPL strings.

use super::eventlog::Event;
use sha2::{Digest, Sha384};

pub fn type_name(t: u32) -> &'static str {
    match t {
        0x1 => "EV_POST_CODE",
        0x3 => "EV_NO_ACTION",
        0x4 => "EV_SEPARATOR",
        0x5 => "EV_ACTION",
        0x6 => "EV_EVENT_TAG",
        0x7 => "EV_S_CRTM_CONTENTS",
        0x8 => "EV_S_CRTM_VERSION",
        0x9 => "EV_CPU_MICROCODE",
        0xa => "EV_PLATFORM_CONFIG_FLAGS",
        0xb => "EV_TABLE_OF_DEVICES",
        0xc => "EV_COMPACT_HASH",
        0xd => "EV_IPL",
        0xe => "EV_IPL_PARTITION_DATA",
        0xf => "EV_NONHOST_CODE",
        0x10 => "EV_NONHOST_CONFIG",
        0x11 => "EV_NONHOST_INFO",
        0x12 => "EV_OMIT_BOOT_DEVICE_EVENTS",
        0x8000_0001 => "EV_EFI_VARIABLE_DRIVER_CONFIG",
        0x8000_0002 => "EV_EFI_VARIABLE_BOOT",
        0x8000_0003 => "EV_EFI_BOOT_SERVICES_APPLICATION",
        0x8000_0004 => "EV_EFI_BOOT_SERVICES_DRIVER",
        0x8000_0005 => "EV_EFI_RUNTIME_SERVICES_DRIVER",
        0x8000_0006 => "EV_EFI_GPT_EVENT",
        0x8000_0007 => "EV_EFI_ACTION",
        0x8000_0008 => "EV_EFI_PLATFORM_FIRMWARE_BLOB",
        0x8000_0009 => "EV_EFI_HANDOFF_TABLES",
        0x8000_000a => "EV_EFI_PLATFORM_FIRMWARE_BLOB2",
        0x8000_000b => "EV_EFI_HANDOFF_TABLES2",
        0x8000_000c => "EV_EFI_VARIABLE_BOOT2",
        0x8000_000d => "EV_EFI_GPT_EVENT2",
        0x8000_0010 => "EV_EFI_HCRTM_EVENT",
        0x8000_00e0 => "EV_EFI_VARIABLE_AUTHORITY",
        0x8000_00e1 => "EV_EFI_SPDM_FIRMWARE_BLOB",
        0x8000_00e2 => "EV_EFI_SPDM_FIRMWARE_CONFIG",
        _ => "unknown",
    }
}

/// What an event's digest is a hash of, as far as the log alone shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// SHA-384 of the event data: the description is what was measured.
    Data,
    /// SHA-384 of the event's text without its trailing NUL.
    Text,
    /// SHA-384 of the text after GRUB's "grub_cmd: " or
    /// "kernel_cmdline: " prefix: the command itself was measured.
    GrubText,
    /// SHA-384 of the UEFI variable's value inside the event (boot
    /// variables are measured this way).
    VariableValue,
    /// Something the log only names: a file, an image, a table.
    Elsewhere,
    /// Not extended into any register.
    NotExtended,
}

impl Coverage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Text => "text",
            Self::GrubText => "text",
            Self::VariableValue => "value",
            Self::Elsewhere => "named",
            Self::NotExtended => "-",
        }
    }
}

pub fn coverage(e: &Event) -> Coverage {
    let Some(digest) = e.sha384 else { return Coverage::NotExtended };
    if e.rtmr().is_none() {
        return Coverage::NotExtended;
    }
    let hash = |b: &[u8]| -> [u8; 48] { Sha384::digest(b).into() };
    if hash(&e.data) == digest {
        return Coverage::Data;
    }
    let text = e.data.strip_suffix(b"\0").unwrap_or(&e.data);
    if hash(text) == digest {
        return Coverage::Text;
    }
    if let Some(pos) = text.windows(2).position(|w| w == b": ") {
        if hash(&text[pos + 2..]) == digest {
            return Coverage::GrubText;
        }
    }
    if let Some((_, value)) = variable_parts(&e.data) {
        if hash(value) == digest {
            return Coverage::VariableValue;
        }
    }
    Coverage::Elsewhere
}

pub fn describe(e: &Event) -> String {
    let d = &e.data;
    match e.event_type {
        0x4 => "separator".into(),
        0x3 => format!("no action: {}", printable(d, 48)),
        0xd => ipl(d),
        0x6 => tagged(d),
        0x8000_0001 | 0x8000_0002 | 0x8000_000c | 0x8000_00e0 => variable(e.event_type, d),
        0x8000_0003..=0x8000_0005 => image(d),
        0x8000_0006 | 0x8000_000d => gpt(d),
        0x8000_000a | 0x8000_000b => format!("firmware: {}", counted_text(d)),
        0xa => format!("platform config: {}", printable(d, 64)),
        _ => printable(d, 96),
    }
}

fn ipl(d: &[u8]) -> String {
    // systemd-stub logs its descriptions in UTF-16: ".linux", ".cmdline"
    if d.len() >= 4 && d.len().is_multiple_of(2) && d[1] == 0 && d[0] != 0 {
        let text = utf16(d);
        if text.starts_with('.') {
            return format!("systemd-stub: UKI section {text} (name or contents; digest of either)");
        }
        return format!("systemd-stub: {text}");
    }
    let t = printable(d, 160);
    if let Some(cmd) = t.strip_prefix("grub_cmd: ") {
        format!("GRUB command: {cmd}")
    } else if let Some(cmdline) = t.strip_prefix("kernel_cmdline: ") {
        format!("kernel command line: {cmdline}")
    } else if t.starts_with('(') || t.starts_with('/') {
        format!("file GRUB read: {t} (digest of its contents)")
    } else if t.starts_with("MokList") || t.starts_with("SbatLevel") {
        format!("shim variable: {t}")
    } else {
        t
    }
}

fn tagged(d: &[u8]) -> String {
    if d.len() < 8 {
        return printable(d, 64);
    }
    let id = u32::from_le_bytes(d[0..4].try_into().expect("4"));
    format!("tagged {id:#x}: {}", printable(&d[8..], 64))
}

fn counted_text(d: &[u8]) -> String {
    match d.first() {
        Some(n) if (*n as usize) < d.len() => printable(&d[1..1 + *n as usize], 64),
        _ => printable(d, 64),
    }
}

/// UEFI_VARIABLE_DATA: GUID, name length, data length, UTF-16 name, data.
fn variable(t: u32, d: &[u8]) -> String {
    let kind = match t {
        0x8000_0001 => "Secure Boot variable",
        0x8000_00e0 => "Secure Boot authority (the db entry that verified an image)",
        _ => "boot variable",
    };
    let Some((name, data)) = variable_parts(d) else {
        return format!("{kind}: unparsed, {} bytes", d.len());
    };
    let detail = match name.as_str() {
        "SecureBoot" => format!("= {}", data.first().copied().unwrap_or(0)),
        "BootOrder" => {
            let order: Vec<String> = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| format!("{:04X}", u16::from_le_bytes(*c)))
                .collect();
            format!("= {}", order.join(","))
        }
        n if n.starts_with("Boot") => load_option(data),
        _ => format!("{} bytes", data.len()),
    };
    format!("{kind}: {name} {detail}")
}

pub fn variable_parts(d: &[u8]) -> Option<(String, &[u8])> {
    let name_chars = usize::try_from(u64::from_le_bytes(d.get(16..24)?.try_into().ok()?)).ok()?;
    let data_len = usize::try_from(u64::from_le_bytes(d.get(24..32)?.try_into().ok()?)).ok()?;
    let name_end = 32usize.checked_add(name_chars.checked_mul(2)?)?;
    let name = utf16(d.get(32..name_end)?);
    let data = d.get(name_end..name_end.checked_add(data_len)?)?;
    Some((name, data))
}

/// EFI_LOAD_OPTION: attributes, path list length, UTF-16 description.
fn load_option(d: &[u8]) -> String {
    let desc = d.get(6..).map(|rest| {
        let end = rest.as_chunks::<2>().0.iter().position(|c| *c == [0, 0]).unwrap_or(rest.len() / 2);
        utf16(&rest[..end * 2])
    });
    format!("\"{}\"", desc.unwrap_or_default())
}

/// UEFI_IMAGE_LOAD_EVENT: the digest is the image's Authenticode hash.
fn image(d: &[u8]) -> String {
    let len = d.get(8..16).map(|b| u64::from_le_bytes(b.try_into().expect("8")));
    let path = d.get(32..).map(file_path_nodes).unwrap_or_default();
    let path = if path.is_empty() { "path not in log".to_owned() } else { path };
    format!(
        "EFI application or driver, {} bytes in memory, {path} (digest: Authenticode hash of the image)",
        len.unwrap_or(0)
    )
}

/// The file-path nodes (type 4, subtype 4) of a UEFI device path.
fn file_path_nodes(mut d: &[u8]) -> String {
    let mut out = Vec::new();
    while d.len() >= 4 {
        let (ty, sub) = (d[0], d[1]);
        let len = u16::from_le_bytes([d[2], d[3]]) as usize;
        if len < 4 || len > d.len() || ty == 0x7f {
            break;
        }
        if ty == 4 && sub == 4 {
            out.push(utf16(&d[4..len]).trim_end_matches('\0').to_owned());
        }
        d = &d[len..];
    }
    out.join("")
}

/// UEFI_GPT_DATA: the GPT header (92 bytes), then a count of partitions.
fn gpt(d: &[u8]) -> String {
    let count = d.get(92..100).map(|b| u64::from_le_bytes(b.try_into().expect("8")));
    format!("GPT partition table, {} partitions (digest: header and entries)", count.unwrap_or(0))
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units).trim_end_matches('\0').to_owned()
}

fn printable(d: &[u8], max: usize) -> String {
    let text: String = d
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| if (0x20..0x7f).contains(b) { *b as char } else { ' ' })
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > max {
        format!("{}...", text.chars().take(max).collect::<String>())
    } else if text.is_empty() {
        format!("{} bytes", d.len())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(event_type: u32, data: &[u8], digest_of: &[u8]) -> Event {
        Event {
            index: 0,
            mr_index: 3,
            event_type,
            sha384: Some(Sha384::digest(digest_of).into()),
            data: data.to_vec(),
        }
    }

    #[test]
    fn grub_commands_cover_their_text() {
        let e = event(0xd, b"kernel_cmdline: /vmlinuz ro\0", b"/vmlinuz ro");
        assert_eq!(coverage(&e), Coverage::GrubText);
        assert_eq!(describe(&e), "kernel command line: /vmlinuz ro");
        let file = event(0xd, b"/vmlinuz\0", b"kernel bytes");
        assert_eq!(coverage(&file), Coverage::Elsewhere);
        assert!(describe(&file).starts_with("file GRUB read: /vmlinuz"));
    }

    #[test]
    fn reads_systemd_stubs_utf16_descriptions() {
        let d: Vec<u8> = ".cmdline\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let e = event(0xd, &d, b"roothash=...");
        assert_eq!(describe(&e), "systemd-stub: UKI section .cmdline (name or contents; digest of either)");
    }

    #[test]
    fn reads_a_secure_boot_variable() {
        let mut d = vec![0u8; 16];
        d.extend_from_slice(&10u64.to_le_bytes());
        d.extend_from_slice(&1u64.to_le_bytes());
        for c in "SecureBoot".encode_utf16() {
            d.extend_from_slice(&c.to_le_bytes());
        }
        d.push(1);
        let e = event(0x8000_0001, &d, &d);
        assert_eq!(coverage(&e), Coverage::Data);
        assert_eq!(describe(&e), "Secure Boot variable: SecureBoot = 1");
    }
}
