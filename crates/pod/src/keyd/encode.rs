//! The bytes a statement is signed over. Fields go in a fixed order:
//! fixed-size ones as they are, numbers as 8 big-endian bytes, variable
//! ones after their length as 8 big-endian bytes, an absent option as a
//! 0 byte and a present one as 1 then the value. JSON only carries the
//! fields; nothing is signed over its text, so key order, whitespace or
//! hex case cannot change what a signature means.

use sha2::{Digest, Sha256};

pub struct Canon(Sha256);

impl Canon {
    pub fn new(domain: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(domain);
        Self(h)
    }

    pub fn fixed(&mut self, b: &[u8]) -> &mut Self {
        self.0.update(b);
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.fixed(&v.to_be_bytes())
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u64(b.len() as u64).fixed(b)
    }

    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    pub fn opt_u64(&mut self, v: Option<u64>) -> &mut Self {
        match v {
            None => self.fixed(&[0]),
            Some(v) => self.fixed(&[1]).u64(v),
        }
    }

    pub fn opt_fixed(&mut self, v: Option<&[u8]>) -> &mut Self {
        match v {
            None => self.fixed(&[0]),
            Some(b) => self.fixed(&[1]).fixed(b),
        }
    }

    pub fn digest(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sha256_of_the_tagged_and_length_prefixed_fields() {
        let mut c = Canon::new(b"d\0");
        c.fixed(&[9; 2]).u64(5).str("ab").opt_u64(None).opt_u64(Some(1)).opt_fixed(Some(&[7]));
        let mut h = Sha256::new();
        h.update(b"d\0");
        h.update([9, 9]);
        h.update(5u64.to_be_bytes());
        h.update(2u64.to_be_bytes());
        h.update(b"ab");
        h.update([0, 1]);
        h.update(1u64.to_be_bytes());
        h.update([1, 7]);
        assert_eq!(c.digest(), <[u8; 32]>::from(h.finalize()));
    }

    #[test]
    fn lengths_keep_neighbouring_strings_apart() {
        let a = { let mut c = Canon::new(b""); c.str("ab").str("c"); c.digest() };
        let b = { let mut c = Canon::new(b""); c.str("a").str("bc"); c.digest() };
        assert_ne!(a, b);
    }
}
