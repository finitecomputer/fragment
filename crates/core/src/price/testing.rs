//! The price tests' shared helpers.

use super::usage::Usage;

pub(super) fn tokens(model: &str, input: u64, cached_input: u64, cache_write: u64, output: u64) -> Usage {
    Usage::Tokens { model: model.into(), input, cached_input, cache_write, output }
}

pub(super) const FLASH: &str = "@cf/zai-org/glm-5.3-flash";
