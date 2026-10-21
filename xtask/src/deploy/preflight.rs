//! Read-only, offline deployment configuration checks. The existing deploy
//! validators remain authoritative; this command gives their failures safe
//! field-oriented diagnostics rather than printing values from the config.

use super::*;
use fragment_core::catalog::{Catalog, CatalogError, Provider};

const USAGE: &str = "usage: cargo xtask preflight --config <file> (offline; exit 0 valid, 1 invalid)";

pub(crate) fn run(args: &[String]) -> Result<()> {
    let [flag, path] = args else { bail!(USAGE) };
    if flag != "--config" || path.is_empty() || path.starts_with("--") {
        bail!(USAGE);
    }
    // Do not attach the I/O error or path: even a filename may contain a
    // pasted credential. No file referenced by this config is opened.
    let text = fs::read_to_string(path).map_err(|_| anyhow::anyhow!("config: cannot read UTF-8 file; check --config and file permissions"))?;
    let report = inspect(&text)?;
    println!("{report}");
    Ok(())
}

fn inspect(text: &str) -> Result<String> {
    let value: Value = serde_json::from_str(&devstack::strip_comments(text))
        .map_err(|e| anyhow::anyhow!("config: invalid JSONC at line {}, column {}; check JSON syntax", e.line(), e.column()))?;
    // WorkOS is mandatory in Deployment; both fields name store entries,
    // not their values. Deserializer errors can quote arbitrary input.
    let workos: WorkOs = serde_json::from_value(value["workos"].clone())
        .map_err(|_| anyhow::anyhow!("workos: supply client_id and api_key as secret-store names; no other fields (deploy/example.jsonc)"))?;
    for (field, name) in [("client_id", &workos.client_id), ("api_key", &workos.api_key)] {
        secret_name(&format!("workos.{field}"), name)?;
    }
    let mut d: Deployment = serde_json::from_value(value)
        .map_err(|_| anyhow::anyhow!("config: invalid deployment shape; check required fields, types and unknown fields against deploy/example.jsonc"))?;
    let rows = std::mem::take(&mut d.providers);
    let mut providers = vec![];
    for (i, row) in rows.iter().enumerate() {
        let field = format!("providers[{i}]");
        let mut wire = row.clone();
        let key = wire.as_object_mut().and_then(|o| o.remove("key"));
        let provider: Provider = serde_json::from_value(wire).map_err(|_| anyhow::anyhow!(
            "{field}: invalid provider shape; check name, kind, hosts, placements, env and optional oauth/connection_oauth fields against deploy/example.jsonc; omit optional OAuth entirely to disable browser authorization"
        ))?;
        Catalog::of(vec![provider.clone()]).map_err(|e| anyhow::anyhow!("{field}.{}", catalog_hint(&e)))?;
        // Use the deploy's own key/kind rules, including absent OAuth. Only
        // the error text differs: no key, provider name or URL is echoed.
        d.providers = vec![row.clone()];
        catalog_of(&d).map_err(|_| anyhow::anyhow!(
            "{field}.key: operator and connection_oauth rows require a string secret-store name; other rows must omit key (disable connection OAuth by removing both connection_oauth and key)"
        ))?;
        if let Some(Value::String(key)) = key {
            secret_name(&format!("{field}.key"), &key)?;
        }
        providers.push(provider);
    }
    let catalog = Catalog::of(providers).map_err(|e| anyhow::anyhow!("providers: {}", catalog_hint(&e)))?;
    d.providers = rows;
    // Also preserve all existing offline deployment checks. Never forward
    // their error chain: several intentionally quote configuration values.
    checked(d).map_err(|_| anyhow::anyhow!(
        "config: existing deployment checks failed; check secret-store names/rotation, models, computer images, support_url and mail_from against deploy/example.jsonc"
    ))?;
    let browser = catalog.providers().iter().filter(|p| p.oauth.is_some() || p.connection_oauth.is_some()).count();
    let absent = catalog.providers().iter().filter(|p| p.kind != fragment_core::catalog::Kind::Operator && p.oauth.is_none() && p.connection_oauth.is_none()).count();
    Ok(format!(
        "preflight: OK (offline configuration only)\nproviders: {} total, {browser} browser OAuth configured, {absent} intentionally without browser OAuth\nNot checked: secret existence/values, credential files, provider enablement, registered callbacks, grants or refresh. No network calls or writes.",
        catalog.providers().len()
    ))
}

fn secret_name(field: &str, name: &str) -> Result<()> {
    if !devstack::store::valid_name(name) {
        bail!("{field}: use a secret-store name (1-{} ASCII letters, digits, _ or -), not a credential value or file path", devstack::store::NAME_MAX_BYTES);
    }
    Ok(())
}

/// A closed set of messages: even malformed names, unknown fields, URLs,
/// header formats and enum values cannot copy input into the error stream.
fn catalog_hint(error: &CatalogError) -> &'static str {
    match error {
        CatalogError::Shape(_) => "shape: check provider field types against deploy/example.jsonc",
        CatalogError::TooMany(_) => "count: reduce providers to the catalog limit",
        CatalogError::Name(_) => "name: use 1-32 lowercase letters, digits or hyphens, no leading/trailing hyphen",
        CatalogError::Duplicate(_) => "name: each provider must have a unique name",
        CatalogError::Hosts { .. } => "hosts: supply supported lowercase DNS host names, without ports or wildcards",
        CatalogError::Placement { .. } => "placements: supply non-conflicting header, query or basic placements; check reserved headers and credential placeholders",
        CatalogError::Env { .. } => "env: supply bounded uppercase environment names; omit reserved names/prefixes",
        CatalogError::DuplicateEnv(_) => "env: each environment name must belong to only one provider",
        CatalogError::Price { .. } => "price: only operator rows may set a price; check micros/per bounds",
        CatalogError::NoPrice(_) => "price: this operator needs an explicit price because the price book has no default",
        CatalogError::OAuth { .. } => "oauth/connection_oauth: use oauth only for own keys, connection_oauth only for connections; check endpoint URLs (no userinfo or fragment), authorize protocol parameters, query-free exchange, client_id and 1-32 bounded scopes; see docs/pipes-browser-binding.md",
        CatalogError::Models { .. } => "models: only own keys offer models; check base_url against hosts and a bounded, unique model offer",
    }
}
