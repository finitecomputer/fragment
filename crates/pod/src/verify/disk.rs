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
    let len = (last + 1 - first) * SECTOR;
    if len > MAX_ESP {
        bail!("the ESP is over 1 GiB");
    }
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(first * SECTOR))?;
    let mut esp = Vec::new();
    f.take(len).read_to_end(&mut esp)?;
    let fs = fatfs::FileSystem::new(Cursor::new(esp), fatfs::FsOptions::new()).context("reading the ESP as FAT")?;
    let mut file = fs.root_dir().open_file("EFI/BOOT/BOOTX64.EFI").context("no \\EFI\\BOOT\\BOOTX64.EFI on the ESP")?;
    let mut efi = Vec::new();
    (&mut file).take(MAX_EFI).read_to_end(&mut efi)?;
    Ok(Disk { head, efi })
}

/// First and last LBA of the one ESP in the GPT.
fn esp_range(head: &[u8]) -> Result<(u64, u64)> {
    let h = head.get(512..604).context("no GPT header")?;
    if &h[0..8] != b"EFI PART" {
        bail!("no GPT at LBA 1");
    }
    let lba = u64::from_le_bytes(h[72..80].try_into()?);
    let count = u32::from_le_bytes(h[80..84].try_into()?) as u64;
    let size = u32::from_le_bytes(h[84..88].try_into()?) as u64;
    let table = head
        .get((lba * SECTOR) as usize..(lba * SECTOR + count * size) as usize)
        .context("GPT entries lie past LBA 33")?;
    let esps: Vec<(u64, u64)> = table
        .chunks_exact(size as usize)
        .filter(|e| e[0..16] == ESP_TYPE)
        .map(|e| (u64::from_le_bytes(e[32..40].try_into().expect("8")), u64::from_le_bytes(e[40..48].try_into().expect("8"))))
        .collect();
    match esps.as_slice() {
        [one] if one.1 >= one.0 => Ok(*one),
        [] => bail!("no EFI system partition"),
        _ => bail!("more than one EFI system partition, or a malformed one"),
    }
}
