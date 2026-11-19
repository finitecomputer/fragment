//! A read-only protobuf wire decoder, enough for Google's launch
//! endorsement (gce-tcb-verifier proto/endorsement.proto): varints and
//! length-delimited fields. Hand-written because the four messages are
//! read once and prost would add a code generator for them.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoError(pub &'static str);

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "protobuf: {}", self.0)
    }
}

fn varint(b: &[u8], pos: &mut usize) -> Result<u64, ProtoError> {
    let mut out = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*pos).ok_or(ProtoError("truncated varint"))?;
        *pos += 1;
        out |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(out);
        }
    }
    Err(ProtoError("varint over 64 bits"))
}

/// Every (field number, value) of one message, in order.
pub fn fields(b: &[u8]) -> Result<Vec<(u64, Value<'_>)>, ProtoError> {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < b.len() {
        let key = varint(b, &mut pos)?;
        let value = match key & 7 {
            0 => Value::Varint(varint(b, &mut pos)?),
            2 => {
                let len = usize::try_from(varint(b, &mut pos)?).map_err(|_| ProtoError("length"))?;
                let end = pos.checked_add(len).filter(|e| *e <= b.len()).ok_or(ProtoError("truncated bytes"))?;
                let v = &b[pos..end];
                pos = end;
                Value::Bytes(v)
            }
            _ => return Err(ProtoError("unsupported wire type")),
        };
        out.push((key >> 3, value));
    }
    Ok(out)
}

pub fn bytes<'a>(f: &[(u64, Value<'a>)], n: u64) -> Option<&'a [u8]> {
    f.iter().rev().find_map(|(k, v)| match v {
        Value::Bytes(b) if *k == n => Some(*b),
        _ => None,
    })
}

pub fn all_bytes<'a>(f: &[(u64, Value<'a>)], n: u64) -> Vec<&'a [u8]> {
    f.iter()
        .filter_map(|(k, v)| match v {
            Value::Bytes(b) if *k == n => Some(*b),
            _ => None,
        })
        .collect()
}

pub fn uint(f: &[(u64, Value<'_>)], n: u64) -> u64 {
    f.iter()
        .rev()
        .find_map(|(k, v)| match v {
            Value::Varint(x) if *k == n => Some(*x),
            _ => None,
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_varints_and_bytes() {
        // field 1 varint 300, field 2 bytes "hi"
        let msg = [0x08, 0xac, 0x02, 0x12, 0x02, b'h', b'i'];
        let f = fields(&msg).unwrap();
        assert_eq!(uint(&f, 1), 300);
        assert_eq!(bytes(&f, 2), Some(&b"hi"[..]));
        assert!(fields(&[0x12, 0x05, b'h']).is_err());
        assert!(fields(&[0x0d, 0, 0, 0, 0]).is_err());
    }
}
