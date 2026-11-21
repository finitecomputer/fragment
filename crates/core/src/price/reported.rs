//! What a vendor's answer says a call used (FIN-616): where a catalog
//! row's meter reads it (`catalog::Meter`).

/// Segments a meter's JSON pointer may have: a usage field is never
/// deeper than a few objects (`/usage/cost_in_usd_ticks`,
/// `/data/metadata/creditsUsed`), and a pointer's walk goes one level a
/// segment.
pub const POINTER_SEGMENTS_MAX: usize = 8;

/// The segments of JSON pointer `pointer` (RFC 6901), `~1` and `~0`
/// unescaped, when it is one a meter may name: `/` first, 1 to
/// `POINTER_SEGMENTS_MAX` segments, none empty, a `~` only as `~0` or `~1`.
pub fn pointer_segments(pointer: &str) -> Option<Vec<String>> {
    let rest = pointer.strip_prefix('/')?;
    if rest.split('~').skip(1).any(|after| !after.starts_with(['0', '1'])) {
        return None;
    }
    let segments: Vec<String> = rest.split('/').map(|s| s.replace("~1", "/").replace("~0", "~")).collect();
    match segments.len() <= POINTER_SEGMENTS_MAX && segments.iter().all(|s| !s.is_empty()) {
        true => Some(segments),
        false => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid and invalid: a pointer a meter may name is short and whole.
    #[test]
    fn a_pointer_is_short_and_whole() {
        assert_eq!(pointer_segments("/usage/cost_in_usd_ticks"), Some(vec!["usage".to_string(), "cost_in_usd_ticks".to_string()]));
        assert_eq!(pointer_segments("/a~0b/c~1d"), Some(vec!["a~b".to_string(), "c/d".to_string()]));
        for bad in ["", "usage", "/", "/a//b", "/a/b/c/d/e/f/g/h/i", "/usage/cost~2", "/a~"] {
            assert_eq!(pointer_segments(bad), None, "{bad:?}");
        }
    }
}
