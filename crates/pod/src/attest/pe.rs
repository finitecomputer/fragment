//! Just enough of the PE/COFF format to predict two kinds of boot
//! measurement from an EFI file:
//!
//! - the Authenticode hash the firmware extends for an EFI application it
//!   loads (EV_EFI_BOOT_SERVICES_APPLICATION), computed as EDK2's
//!   `Tcg2MeasurePeImage` does (Microsoft's Authenticode PE spec,
//!   "Calculating the PE Image Hash");
//! - the bytes of a named section as they lie in memory once loaded,
//!   which is what systemd-stub measures for each section of a unified
//!   kernel image.

use sha2::{Digest, Sha384};
use std::fmt;

const MAX_SECTIONS: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeError {
    Truncated(&'static str),
    NotPe(&'static str),
    BadSection { name: String },
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(what) => write!(f, "PE file truncated: {what}"),
            Self::NotPe(why) => write!(f, "not a PE image: {why}"),
            Self::BadSection { name } => write!(f, "section {name} lies outside the file"),
        }
    }
}

impl std::error::Error for PeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub raw_size: u32,
    pub raw_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pe<'a> {
    bytes: &'a [u8],
    checksum_at: usize,
    /// Where the certificate-table data directory entry is, if the
    /// optional header has one.
    security_entry_at: Option<usize>,
    cert_size: usize,
    size_of_headers: usize,
    pub sections: Vec<Section>,
}

fn u16_at(b: &[u8], at: usize, what: &'static str) -> Result<u16, PeError> {
    let s = b.get(at..at + 2).ok_or(PeError::Truncated(what))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize, what: &'static str) -> Result<u32, PeError> {
    let s = b.get(at..at + 4).ok_or(PeError::Truncated(what))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn parse(bytes: &[u8]) -> Result<Pe<'_>, PeError> {
    if bytes.get(0..2) != Some(b"MZ") {
        return Err(PeError::NotPe("no MZ header"));
    }
    let pe = u32_at(bytes, 0x3c, "e_lfanew")? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(PeError::NotPe("no PE signature"));
    }
    let coff = pe + 4;
    let count = u16_at(bytes, coff + 2, "section count")? as usize;
    if count > MAX_SECTIONS {
        return Err(PeError::NotPe("too many sections"));
    }
    let opt_size = u16_at(bytes, coff + 16, "optional header size")? as usize;
    let opt = coff + 20;
    let (rva_count_at, dirs_at) = match u16_at(bytes, opt, "optional header magic")? {
        0x10b => (opt + 92, opt + 96),
        0x20b => (opt + 108, opt + 112),
        _ => return Err(PeError::NotPe("unknown optional header magic")),
    };
    let size_of_headers = u32_at(bytes, opt + 60, "SizeOfHeaders")? as usize;
    let rva_count = u32_at(bytes, rva_count_at, "NumberOfRvaAndSizes")? as usize;
    // data directory 4 is the certificate table (a file offset, not an RVA)
    let (security_entry_at, cert_size) = if rva_count > 4 {
        let at = dirs_at + 4 * 8;
        (Some(at), u32_at(bytes, at + 4, "certificate table size")? as usize)
    } else {
        (None, 0)
    };
    let table = opt + opt_size;
    let sections = (0..count)
        .map(|i| {
            let s = table + i * 40;
            let raw_name = bytes.get(s..s + 8).ok_or(PeError::Truncated("section table"))?;
            let end = raw_name.iter().position(|b| *b == 0).unwrap_or(8);
            Ok(Section {
                name: String::from_utf8_lossy(&raw_name[..end]).into_owned(),
                virtual_size: u32_at(bytes, s + 8, "section")?,
                virtual_address: u32_at(bytes, s + 12, "section")?,
                raw_size: u32_at(bytes, s + 16, "section")?,
                raw_offset: u32_at(bytes, s + 20, "section")?,
            })
        })
        .collect::<Result<Vec<_>, PeError>>()?;
    if size_of_headers > bytes.len() || security_entry_at.is_some_and(|at| at + 8 > size_of_headers) {
        return Err(PeError::Truncated("headers"));
    }
    Ok(Pe { bytes, checksum_at: opt + 64, security_entry_at, cert_size, size_of_headers, sections })
}

impl Pe<'_> {
    /// The Authenticode SHA-384 the firmware measures for this image.
    pub fn authenticode_sha384(&self) -> Result<[u8; 48], PeError> {
        let b = self.bytes;
        let mut h = Sha384::new();
        h.update(&b[..self.checksum_at]);
        let after_checksum = self.checksum_at + 4;
        match self.security_entry_at {
            Some(at) => {
                h.update(&b[after_checksum..at]);
                h.update(&b[at + 8..self.size_of_headers]);
            }
            None => h.update(&b[after_checksum..self.size_of_headers]),
        }
        let mut sections: Vec<&Section> = self.sections.iter().filter(|s| s.raw_size > 0).collect();
        sections.sort_by_key(|s| s.raw_offset);
        let mut hashed = self.size_of_headers;
        for s in sections {
            h.update(self.raw(s)?);
            hashed += s.raw_size as usize;
        }
        // whatever follows the last section, except the certificate table
        if b.len() > hashed + self.cert_size {
            h.update(&b[hashed..b.len() - self.cert_size]);
        }
        Ok(h.finalize().into())
    }

    fn raw(&self, s: &Section) -> Result<&[u8], PeError> {
        let start = s.raw_offset as usize;
        self.bytes
            .get(start..start + s.raw_size as usize)
            .ok_or_else(|| PeError::BadSection { name: s.name.clone() })
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// The section as it lies in memory once loaded: VirtualSize bytes,
    /// the file's bytes first and zeros after them.
    pub fn section_in_memory(&self, name: &str) -> Result<Option<Vec<u8>>, PeError> {
        let Some(s) = self.section(name) else { return Ok(None) };
        let raw = self.raw(s)?;
        let size = s.virtual_size as usize;
        let mut out = raw[..raw.len().min(size)].to_vec();
        out.resize(size, 0);
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PE32+ file: headers (0x200), two sections, a trailer, and a
    /// certificate table of `cert` bytes at the end.
    fn tiny_pe(cert: usize) -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        b[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes());
        b[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes());
        let opt = coff + 20;
        b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        b[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes());
        b[opt + 64..opt + 68].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        b[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        let table = opt + 240;
        for (i, (name, vsize, raw_off)) in [(".text", 0x10u32, 0x200u32), (".data", 0x300, 0x400)].iter().enumerate() {
            let s = table + i * 40;
            b[s..s + name.len()].copy_from_slice(name.as_bytes());
            b[s + 8..s + 12].copy_from_slice(&vsize.to_le_bytes());
            b[s + 12..s + 16].copy_from_slice(&(0x1000 * (i as u32 + 1)).to_le_bytes());
            b[s + 16..s + 20].copy_from_slice(&0x200u32.to_le_bytes());
            b[s + 20..s + 24].copy_from_slice(&raw_off.to_le_bytes());
        }
        b.extend((0..0x400).map(|i| (i % 251) as u8));
        b.extend_from_slice(b"trailer!");
        let cert_at = b.len();
        b.extend(std::iter::repeat_n(0xcc, cert));
        let sec = opt + 112 + 4 * 8;
        b[sec..sec + 4].copy_from_slice(&(cert_at as u32).to_le_bytes());
        b[sec + 4..sec + 8].copy_from_slice(&(cert as u32).to_le_bytes());
        b
    }

    #[test]
    fn authenticode_skips_checksum_certificate_entry_and_table() {
        let unsigned = tiny_pe(0);
        let signed = tiny_pe(64);
        let a = parse(&unsigned).unwrap().authenticode_sha384().unwrap();
        // the checksum and the certificate table do not count
        let mut other_checksum = signed.clone();
        other_checksum[0x84 + 20 + 64] ^= 0xff;
        let b = parse(&other_checksum).unwrap().authenticode_sha384().unwrap();
        assert_eq!(a, b);
        // any byte of a section does
        let mut changed = unsigned.clone();
        changed[0x450] ^= 1;
        assert_ne!(parse(&changed).unwrap().authenticode_sha384().unwrap(), a);
        // and so does the trailer before the certificate table
        let mut trailer = signed.clone();
        trailer[0x600] ^= 1;
        assert_ne!(parse(&trailer).unwrap().authenticode_sha384().unwrap(), a);
    }

    #[test]
    fn a_section_in_memory_is_cut_or_zero_filled_to_its_virtual_size() {
        let b = tiny_pe(0);
        let pe = parse(&b).unwrap();
        let text = pe.section_in_memory(".text").unwrap().unwrap();
        assert_eq!(text, b[0x200..0x210]);
        let data = pe.section_in_memory(".data").unwrap().unwrap();
        assert_eq!(data.len(), 0x300);
        assert_eq!(data[..0x200], b[0x400..0x600]);
        assert!(data[0x200..].iter().all(|x| *x == 0));
        assert_eq!(pe.section_in_memory(".none").unwrap(), None);
    }

    #[test]
    fn refuses_what_is_not_pe() {
        assert!(parse(b"MZ").is_err());
        assert!(parse(&[0u8; 512]).is_err());
        let mut b = tiny_pe(0);
        b[0x80] = b'X';
        assert_eq!(parse(&b), Err(PeError::NotPe("no PE signature")));
    }
}
