//! What the service reads of its environment: each value named directly,
//! or, with `_FILE` after its name, read from that file (a secret mounted
//! in, as Docker's are), never needing a command line.

/// `name`'s value, or the contents of the file `<name>_FILE` names (its
/// trailing newline let go); `None` when neither is set.
pub fn value(name: &str) -> Result<Option<String>, String> {
    let direct = std::env::var(name).ok();
    let file = std::env::var(format!("{name}_FILE")).ok();
    match (direct, file) {
        (Some(_), Some(_)) => Err(format!("{name} is named or read from {name}_FILE, not both")),
        (Some(v), None) => Ok(Some(v)),
        (None, Some(path)) => std::fs::read_to_string(&path).map(|v| Some(v.trim_end_matches(['\n', '\r']).to_string())).map_err(|e| format!("{name}_FILE {path}: {e}")),
        (None, None) => Ok(None),
    }
}

/// The store's options, each `<prefix><OPTION>` (or `<prefix><OPTION>_FILE`)
/// as `option`, lowercased: `SEARCH_STORAGE_AWS_ENDPOINT` is `aws_endpoint`.
pub fn options(prefix: &str) -> Result<Vec<(String, String)>, String> {
    let mut names: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| k.strip_prefix(prefix).map(|o| o.strip_suffix("_FILE").unwrap_or(o).to_string()))
        .filter(|o| !o.is_empty())
        .collect();
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for option in names {
        if let Some(v) = value(&format!("{prefix}{option}"))? {
            out.push((option.to_lowercase(), v));
        }
    }
    Ok(out)
}
