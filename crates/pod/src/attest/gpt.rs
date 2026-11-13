//! The firmware's EV_EFI_GPT_EVENT, predicted from the disk's first
//! sectors. EDK2's `Tcg2MeasureGptTable` measures UEFI_GPT_DATA (TCG PC
//! Client Platform Firmware Profile, 10.4.3): the 92-byte GPT header as
//! read from LBA 1, the number of partitions as a UINTN (8 bytes on
//! x86-64), then each partition entry whose type GUID is not zero, in
//! table order. The digest is SHA-384 of those bytes.

use std::fmt;

const HEADER_LEN: usize = 92;
const MAX_ENTRIES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GptError {
    Truncated,
    NoSignature,
    BadHeader(&'static str),
}

impl fmt::Display for GptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "disk image too short for its GPT"),
            Self::NoSignature => write!(f, "no 'EFI PART' header at LBA 1"),
            Self::BadHeader(why) => write!(f, "GPT header: {why}"),
        }
    }
}

impl std::error::Error for GptError {}

/// UEFI_GPT_DATA for a disk whose first sectors are `disk`, with
/// `sector` bytes per logical block.
pub fn event_data(disk: &[u8], sector: usize) -> Result<Vec<u8>, GptError> {
    let header = disk.get(sector..sector + HEADER_LEN).ok_or(GptError::Truncated)?;
    if &header[0..8] != b"EFI PART" {
        return Err(GptError::NoSignature);
    }
    let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4")) as usize;
    let u64_at = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().expect("8"));
    let entries_lba = usize::try_from(u64_at(72)).map_err(|_| GptError::BadHeader("entry LBA"))?;
    let count = u32_at(80);
    let entry_size = u32_at(84);
    if count > MAX_ENTRIES || !(128..=4096).contains(&entry_size) {
        return Err(GptError::BadHeader("entry count or size out of range"));
    }
    let start = entries_lba.checked_mul(sector).ok_or(GptError::BadHeader("entry LBA"))?;
    let table = disk.get(start..start + count * entry_size).ok_or(GptError::Truncated)?;
    let used: Vec<&[u8]> = table.chunks_exact(entry_size).filter(|e| e[0..16].iter().any(|b| *b != 0)).collect();
    let mut out = header.to_vec();
    out.extend_from_slice(&(used.len() as u64).to_le_bytes());
    for e in used {
        out.extend_from_slice(e);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A disk with a GPT at LBA 1, its 128 entries at LBA 2, and
    /// `types[i]` (zero meaning unused) as entry i's first type byte.
    pub fn disk(types: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 512 * 34];
        d[512..520].copy_from_slice(b"EFI PART");
        d[512 + 12..512 + 16].copy_from_slice(&92u32.to_le_bytes());
        d[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
        d[512 + 80..512 + 84].copy_from_slice(&128u32.to_le_bytes());
        d[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
        for (i, t) in types.iter().enumerate() {
            let e = 1024 + i * 128;
            d[e] = *t;
            d[e + 56] = b'a' + i as u8;
        }
        d
    }

    #[test]
    fn keeps_used_entries_in_order_after_the_header_and_count() {
        let d = disk(&[1, 0, 2]);
        let data = event_data(&d, 512).unwrap();
        assert_eq!(data[..92], d[512..604]);
        assert_eq!(u64::from_le_bytes(data[92..100].try_into().unwrap()), 2);
        assert_eq!(data.len(), 100 + 2 * 128);
        assert_eq!(data[100 + 56], b'a');
        assert_eq!(data[228 + 56], b'c');
    }

    #[test]
    fn refuses_a_disk_without_a_gpt() {
        assert_eq!(event_data(&[0u8; 2048], 512), Err(GptError::NoSignature));
        assert_eq!(event_data(&disk(&[1])[..1024], 512), Err(GptError::Truncated));
    }
}
