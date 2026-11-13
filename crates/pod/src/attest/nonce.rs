//! The verifier's nonce: up to 64 bytes, given as hex, zero-padded into
//! the TD report's 64-byte REPORTDATA.

use std::fmt;

pub const REPORT_DATA_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonceError {
    Empty,
    TooLong { bytes: usize },
    NotHex,
}

impl fmt::Display for NonceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "nonce is empty"),
            Self::TooLong { bytes } => {
                write!(f, "nonce is {bytes} bytes; at most {REPORT_DATA_LEN}")
            }
            Self::NotHex => write!(f, "nonce is not an even-length hex string"),
        }
    }
}

impl std::error::Error for NonceError {}

/// Parse a hex nonce of 1 to 64 bytes.
pub fn parse_hex(text: &str) -> Result<Vec<u8>, NonceError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(NonceError::Empty);
    }
    // Refuse before decoding, so a huge query string costs nothing.
    if text.len() > REPORT_DATA_LEN * 2 {
        return Err(NonceError::TooLong { bytes: text.len() / 2 });
    }
    hex::decode(text).map_err(|_| NonceError::NotHex)
}

/// The REPORTDATA a quote for `nonce` must carry: the nonce, then zeros.
pub fn report_data(nonce: &[u8]) -> Result<[u8; REPORT_DATA_LEN], NonceError> {
    if nonce.is_empty() {
        return Err(NonceError::Empty);
    }
    if nonce.len() > REPORT_DATA_LEN {
        return Err(NonceError::TooLong { bytes: nonce.len() });
    }
    let mut out = [0u8; REPORT_DATA_LEN];
    out[..nonce.len()].copy_from_slice(nonce);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_a_short_nonce_with_zeros() {
        let data = report_data(&[0xab, 0xcd]).unwrap();
        assert_eq!(&data[..2], &[0xab, 0xcd]);
        assert!(data[2..].iter().all(|b| *b == 0));
    }

    #[test]
    fn takes_a_full_64_byte_nonce() {
        let nonce = parse_hex(&"11".repeat(64)).unwrap();
        assert_eq!(report_data(&nonce).unwrap(), [0x11; 64]);
    }

    #[test]
    fn refuses_empty_long_and_bad_hex() {
        assert_eq!(parse_hex(""), Err(NonceError::Empty));
        assert_eq!(parse_hex(&"00".repeat(65)), Err(NonceError::TooLong { bytes: 65 }));
        assert_eq!(parse_hex("abc"), Err(NonceError::NotHex));
        assert_eq!(parse_hex("zz"), Err(NonceError::NotHex));
        assert_eq!(report_data(&[]), Err(NonceError::Empty));
        assert_eq!(report_data(&[0; 65]), Err(NonceError::TooLong { bytes: 65 }));
    }
}
