//! Reading a disk image the way the firmware does for `expect`: its first
//! 34 sectors (the GPT) and the file `\EFI\BOOT\BOOTX64.EFI` on its EFI
//! system partition.

use anyhow::{bail, Context, Result};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

const SECTOR: u64 = 512;
/// The ESP's partition type GUID, C12A7328-F81F-11D2-BA4B-00A0C93EC93B,
/// as stored (mixed-endian).
const ESP_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
const MAX_ESP: u64 = 1 << 30;
const MAX_EFI: u64 = 512 << 20;
/// UEFI requires a GPT entry of 128 bytes or a larger power of two; no
/// partitioning tool writes one over 4 KiB (the same bounds as
/// `attest::gpt`, which reads the same table).
const ENTRY_SIZES: std::ops::RangeInclusive<u64> = 128..=4096;
/// 128 entries is the usual table; 34 sectors could not hold more than
/// 128 of 128 bytes anyway, so this only stops a wild count early.
const MAX_ENTRIES: u64 = 1024;

pub struct Disk {
    /// LBA 0 to 33: the protective MBR, the GPT header and its entries.
    pub head: Vec<u8>,
    pub efi: Vec<u8>,
}

pub fn read_head(path: &Path) -> Result<Vec<u8>> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(SECTOR * 34).read_to_end(&mut head))
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(head)
}

pub fn read(path: &Path) -> Result<Disk> {
    let head = read_head(path)?;
    let (first, last) = esp_range(&head)?;
    let (start, len) = esp_bytes(first, last)?;
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut esp = Vec::new();
    f.take(len).read_to_end(&mut esp)?;
    let fs = fatfs::FileSystem::new(Cursor::new(esp), fatfs::FsOptions::new()).context("reading the ESP as FAT")?;
    let mut file = fs.root_dir().open_file("EFI/BOOT/BOOTX64.EFI").context("no \\EFI\\BOOT\\BOOTX64.EFI on the ESP")?;
    let mut efi = Vec::new();
    (&mut file).take(MAX_EFI).read_to_end(&mut efi)?;
    Ok(Disk { head, efi })
}

/// The ESP's byte offset and length on the disk, from its first and last
/// LBA (inclusive), refusing one past u64 or over `MAX_ESP`.
fn esp_bytes(first: u64, last: u64) -> Result<(u64, u64)> {
    let sectors = last.checked_sub(first).and_then(|n| n.checked_add(1)).context("the ESP ends before it starts")?;
    let len = sectors.checked_mul(SECTOR).filter(|l| *l <= MAX_ESP).context("the ESP is over 1 GiB")?;
    let start = first.checked_mul(SECTOR).context("the ESP starts past the end of any disk")?;
    Ok((start, len))
}

/// First and last LBA of the one ESP in the GPT. The entry size and count
/// are checked, and the table's place computed without overflow, before
/// the table is sliced or chunked: a bad table is an error, never a panic.
fn esp_range(head: &[u8]) -> Result<(u64, u64)> {
    let h = head.get(512..604).context("no GPT header")?;
    if &h[0..8] != b"EFI PART" {
        bail!("no GPT at LBA 1");
    }
    let lba = u64::from_le_bytes(h[72..80].try_into()?);
    let count = u64::from(u32::from_le_bytes(h[80..84].try_into()?));
    let size = u64::from(u32::from_le_bytes(h[84..88].try_into()?));
    if !ENTRY_SIZES.contains(&size) {
        bail!("GPT entry size {size}: expected 128 to 4096 bytes");
    }
    if count > MAX_ENTRIES {
        bail!("GPT entry count {count}: at most {MAX_ENTRIES}");
    }
    let start = lba.checked_mul(SECTOR).context("GPT entry LBA is past the end of any disk")?;
    let end = start.checked_add(count * size).context("GPT entry LBA is past the end of any disk")?;
    let range = usize::try_from(start).ok().zip(usize::try_from(end).ok()).context("GPT entry LBA is past the end of any disk")?;
    let table = head.get(range.0..range.1).context("GPT entries lie past LBA 33")?;
    // `size` is at least 128, so each chunk holds the type GUID and both LBAs
    let entry = usize::try_from(size)?;
    let esps: Vec<(u64, u64)> = table
        .chunks_exact(entry)
        .filter(|e| e[0..16] == ESP_TYPE)
        .map(|e| (u64::from_le_bytes(e[32..40].try_into().expect("8")), u64::from_le_bytes(e[40..48].try_into().expect("8"))))
        .collect();
    match esps.as_slice() {
        [one] if one.1 >= one.0 => Ok(*one),
        [] => bail!("no EFI system partition"),
        _ => bail!("more than one EFI system partition, or a malformed one"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first 34 sectors of a disk: a GPT at LBA 1 whose entries start
    /// at `lba`, `count` of `size` bytes, and one ESP entry from sector
    /// 2048 to `esp_last` in the first entry's place (if it fits there).
    fn head(lba: u64, count: u32, size: u32, esp_last: u64) -> Vec<u8> {
        let mut d = vec![0u8; 512 * 34];
        d[512..520].copy_from_slice(b"EFI PART");
        d[512 + 72..512 + 80].copy_from_slice(&lba.to_le_bytes());
        d[512 + 80..512 + 84].copy_from_slice(&count.to_le_bytes());
        d[512 + 84..512 + 88].copy_from_slice(&size.to_le_bytes());
        let e = 1024;
        d[e..e + 16].copy_from_slice(&ESP_TYPE);
        d[e + 32..e + 40].copy_from_slice(&2048u64.to_le_bytes());
        d[e + 40..e + 48].copy_from_slice(&esp_last.to_le_bytes());
        d
    }

    #[test]
    fn a_well_formed_table_names_its_one_esp() {
        assert_eq!(esp_range(&head(2, 128, 128, 4095)).unwrap(), (2048, 4095));
    }

    #[test]
    fn an_entry_size_of_zero_is_refused_not_a_panic() {
        let err = esp_range(&head(2, 128, 0, 4095)).unwrap_err();
        assert!(err.to_string().contains("entry size"), "{err}");
    }

    #[test]
    fn an_entry_size_of_8_is_refused_not_a_panic() {
        let err = esp_range(&head(2, 128, 8, 4095)).unwrap_err();
        assert!(err.to_string().contains("entry size"), "{err}");
    }

    #[test]
    fn entries_running_past_the_head_are_refused() {
        // 1024 entries of 128 bytes from LBA 2: 128 KiB, past LBA 33
        let err = esp_range(&head(2, 1024, 128, 4095)).unwrap_err();
        assert!(err.to_string().contains("past LBA 33"), "{err}");
    }

    #[test]
    fn an_entry_lba_whose_offset_overflows_is_refused_not_a_panic() {
        let err = esp_range(&head(u64::MAX / 256, 128, 128, 4095)).unwrap_err();
        assert!(err.to_string().contains("entry LBA"), "{err}");
    }

    #[test]
    fn an_esp_ending_at_the_last_lba_is_refused_not_a_panic() {
        let path = std::env::temp_dir().join(format!("fragment-pod-disk-{}.img", std::process::id()));
        std::fs::write(&path, head(2, 128, 128, u64::MAX)).unwrap();
        let r = read(&path);
        let _ = std::fs::remove_file(&path);
        let err = r.err().expect("refused");
        assert!(err.to_string().contains("ESP"), "{err}");
    }
}
