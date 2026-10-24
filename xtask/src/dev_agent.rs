//! `cargo xtask dev --real-agent [--models <config>]`: the dev stack's
//! computers on our Hermes image, built from the tree, and its text tiers
//! on real models, so a fresh agent's first turns run as they do on a
//! deployment (AGENTS.md, `cargo xtask dev`). Without it, dev runs the
//! stub image on the Workers AI fake and calls no real model.
//!
//! The models are a deployment config's `models` block (deploy/example.jsonc's
//! by default; a copy of the preview's config runs the preview's), checked
//! as a deploy checks it. Its slots on an OpenAI-compatible API call with
//! the one key in the file `FRAGMENT_DEV_MODEL_KEY_FILE` names: read from
//! a file, as `WORKOS_API_KEY_FILE` is, so no key is on a command line.
//! Its Workers AI slots stay on the fake.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;

/// The variable naming the file that holds the models' key.
pub const KEY_FILE_ENV: &str = "FRAGMENT_DEV_MODEL_KEY_FILE";

/// Where the models come from without `--models`.
const EXAMPLE_CONFIG: &str = "deploy/example.jsonc";

/// What `--real-agent` runs: the models, and the image new computers are
/// pinned to (`hermes`, `devstack::hermes_image`).
pub struct RealAgent {
    /// `FRAGMENT_MODELS`, as a deploy renders it from the config.
    pub models: String,
    /// The slots on an OpenAI-compatible API, which call with `key`.
    pub slots: Vec<String>,
    pub key: String,
    /// The config its models are read from, for the banner.
    pub from: PathBuf,
}

impl RealAgent {
    /// The image new computers run, by its name in the node's config.
    pub const IMAGE: &'static str = "hermes";

    /// `--models <config>` (else deploy/example.jsonc) and the key file
    /// `FRAGMENT_DEV_MODEL_KEY_FILE` names: read and checked before
    /// anything builds.
    pub fn read(models: Option<&Path>) -> Result<RealAgent> {
        let from = models.map(Path::to_path_buf).unwrap_or_else(|| devstack::repo_root().join(EXAMPLE_CONFIG));
        let text = std::fs::read_to_string(&from).with_context(|| format!("reading {}", from.display()))?;
        let key = match std::env::var_os(KEY_FILE_ENV) {
            Some(path) => Some(std::fs::read_to_string(&path).with_context(|| format!("reading {} ({KEY_FILE_ENV})", Path::new(&path).display()))?),
            None => None,
        };
        let (models, slots, key) = models_of(&text, key).with_context(|| format!("--real-agent's models, {}", from.display()))?;
        Ok(RealAgent { models, slots, key, from })
    }
}

/// A deployment config's `models` (`text`, comments and all) as dev binds
/// them: the cell's, the slots on an OpenAI-compatible API, and `key`,
/// which they all call with. Dev binds one model key, so they name one;
/// and `--real-agent` is real models, so there is at least one such slot.
fn models_of(text: &str, key: Option<String>) -> Result<(String, Vec<String>, String)> {
    let config: serde_json::Value = serde_json::from_str(&devstack::strip_comments(text)).context("a deployment's config (JSON with comments)")?;
    let Some(models) = config.get("models") else { bail!("it names no models: a deployment's config names them in `models` (deploy/example.jsonc)") };
    let (cell, keys) = crate::deploy::models_cell(models)?;
    let mut names: Vec<&str> = keys.iter().map(|(_, k)| k.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    match names.len() {
        0 => bail!("every slot runs on Workers AI, which dev fakes: name a config with slots on an OpenAI-compatible API (deploy/example.jsonc's are OpenRouter's)"),
        1 => {}
        _ => bail!("dev binds one model key, and its slots name {names:?}"),
    }
    let key = key.map(|k| k.trim().to_string()).unwrap_or_default();
    if key.is_empty() {
        bail!("{KEY_FILE_ENV} names the file holding the key its slots call with ({}): set it, the file holding only the key", names[0]);
    }
    Ok((cell.to_string(), keys.into_iter().map(|(slot, _)| slot).collect(), key))
}

#[cfg(test)]
mod tests {
    use super::models_of;

    /// Goal: `--real-agent` runs real models as a deploy would, and only
    /// with their key. Method: example.jsonc's models (OpenRouter's), the
    /// preview's shape (comments and all), and each refusal: no models,
    /// Workers AI alone, two keys, no key, a deploy's own refusal.
    #[test]
    fn real_agent_models_are_a_deployments_with_their_one_key() {
        let example = std::fs::read_to_string(fragment_devstack::repo_root().join(super::EXAMPLE_CONFIG)).unwrap();
        let (cell, slots, key) = models_of(&example, Some("sk-or-test\n".into())).unwrap();
        assert_eq!(slots, ["cheap", "medium"]);
        assert_eq!(key, "sk-or-test", "read trimmed");
        assert!(!cell.contains("fragment-openrouter-key"), "the cell's models name no key: {cell}");
        assert!(cell.contains("workers_ai"), "its Workers AI slots stay (on the fake): {cell}");

        let preview = r#"{ "zone": "finite.build", // the preview's, less the rest
          "models": {
            "cheap":  { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "key": "fragment-openrouter-key", "model": "deepseek/deepseek-v4.1-flash",
                        "body": { "provider": { "zdr": true, "max_price": { "prompt": 0.15, "completion": 0.75 } } } },
            "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "key": "fragment-openrouter-key", "model": "deepseek/deepseek-v4.1-flash",
                        "body": { "provider": { "zdr": true, "max_price": { "prompt": 0.15, "completion": 0.75 } } } },
            "fallback": { "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3-flash" } } }"#;
        assert_eq!(models_of(preview, Some("k".into())).unwrap().1, ["cheap", "medium"]);

        let refused = |text: &str, key: Option<&str>, says: &str| {
            let e = models_of(text, key.map(str::to_string)).expect_err(text).to_string();
            assert!(e.contains(says), "{text}: {e}");
        };
        refused(r#"{ "zone": "x" }"#, Some("k"), "names no models");
        refused(r#"{ "models": { "cheap": { "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3-flash" } } }"#, Some("k"), "Workers AI");
        refused(
            r#"{ "models": { "cheap": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "key": "k1", "model": "z-ai/glm-5.3-flash",
                                        "body": { "provider": { "max_price": { "prompt": 0.15, "completion": 0.5 } } } },
                             "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "key": "k2", "model": "deepseek/deepseek-v4.1-flash",
                                         "body": { "provider": { "max_price": { "prompt": 0.15, "completion": 0.75 } } } } } }"#,
            Some("k"),
            "one model key",
        );
        refused(preview, None, super::KEY_FILE_ENV);
        refused(preview, Some("  \n"), super::KEY_FILE_ENV);
        refused(r#"{ "models": { "cheap": { "provider": "openai", "base_url": "https://a.example/v1", "model": "z-ai/glm-5.3-flash" } } }"#, Some("k"), "names its key");
        refused("{ not json", Some("k"), "JSON");
    }
}
