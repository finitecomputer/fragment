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
//!
//! A self-hosted stack's own models (docs/self-host.md, seam 3) come the
//! same way without our Hermes image: `FRAGMENT_MODELS_FILE` names a file
//! holding a deployment's `models` block alone, its slots' URLs on this
//! machine allowed (`http://127.0.0.1:<port>`, as a local fleet's egress
//! reaches it), and `FRAGMENT_MODEL_KEY_FILE` the key they call with
//! (`ModelsFile`).

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
    let (cell, keys) = crate::deploy::models_cell(models, false)?;
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

/// The variable naming a file of a deployment's `models` (seam 3).
pub const MODELS_FILE_ENV: &str = "FRAGMENT_MODELS_FILE";
/// The variable naming the file that holds their one key.
pub const MODEL_KEY_FILE_ENV: &str = "FRAGMENT_MODEL_KEY_FILE";

/// A self-hosted stack's models (`FRAGMENT_MODELS_FILE`): as `RealAgent`'s,
/// without our Hermes image, and on this machine's servers.
pub struct ModelsFile {
    pub models: String,
    pub slots: Vec<String>,
    pub key: String,
    pub from: PathBuf,
}

impl ModelsFile {
    /// The file `FRAGMENT_MODELS_FILE` names and the key in
    /// `FRAGMENT_MODEL_KEY_FILE`, read and checked before anything
    /// builds; `None` when the first is unset.
    pub fn read() -> Result<Option<ModelsFile>> {
        let Some(from) = std::env::var_os(MODELS_FILE_ENV).map(PathBuf::from) else { return Ok(None) };
        let text = std::fs::read_to_string(&from).with_context(|| format!("reading {} ({MODELS_FILE_ENV})", from.display()))?;
        let key = match std::env::var_os(MODEL_KEY_FILE_ENV) {
            Some(path) => Some(std::fs::read_to_string(&path).with_context(|| format!("reading {} ({MODEL_KEY_FILE_ENV})", Path::new(&path).display()))?),
            None => None,
        };
        let (models, slots, key) = models_file_of(&text, key).with_context(|| format!("{MODELS_FILE_ENV}, {}", from.display()))?;
        Ok(Some(ModelsFile { models, slots, key, from }))
    }
}

/// A file of a deployment's `models` block (`text`, comments and all) as
/// dev binds it: its slots' URLs may be this machine's, and the slots on
/// an OpenAI-compatible API name one key, `key`'s.
fn models_file_of(text: &str, key: Option<String>) -> Result<(String, Vec<String>, String)> {
    let models: serde_json::Value = serde_json::from_str(&devstack::strip_comments(text)).context("a deployment's `models` (JSON with comments)")?;
    let (cell, keys) = crate::deploy::models_cell(&models, true)?;
    let mut names: Vec<&str> = keys.iter().map(|(_, k)| k.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    match names.len() {
        0 => bail!("every slot runs on Workers AI, which dev fakes: name slots on an OpenAI-compatible API"),
        1 => {}
        _ => bail!("dev binds one model key, and its slots name {names:?}"),
    }
    let key = key.map(|k| k.trim().to_string()).unwrap_or_default();
    if key.is_empty() {
        bail!("{MODEL_KEY_FILE_ENV} names the file holding the key its slots call with ({}): set it, the file holding only the key (a server that takes none still gets a word)", names[0]);
    }
    Ok((cell.to_string(), keys.into_iter().map(|(slot, _)| slot).collect(), key))
}

#[cfg(test)]
mod tests {
    use super::{models_file_of, models_of};

    /// Goal: a self-hosted stack's models file runs its slots on this
    /// machine's server, priced by the slot, with its one key; a public
    /// server's plain http, and a slot priced by nothing, are refused.
    #[test]
    fn a_models_file_runs_on_this_machines_server() {
        let local = r#"{ // Bonsai on this box
          "cheap": { "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "key": "bonsai-key", "model": "bonsai-2-27b", "price": { "input": 0, "output": 0 } },
          "medium": { "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "key": "bonsai-key", "model": "bonsai-2-27b", "price": { "input": 0, "output": 0 } } }"#;
        let (cell, slots, key) = models_file_of(local, Some("k\n".into())).unwrap();
        assert_eq!((slots, key.as_str()), (vec!["cheap".to_string(), "medium".to_string()], "k"), "read trimmed");
        assert!(cell.contains("127.0.0.1:18087") && !cell.contains("bonsai-key"), "{cell}");
        let refused = |text: &str, key: Option<&str>, says: &str| {
            let e = format!("{:#}", models_file_of(text, key.map(str::to_string)).expect_err(text));
            assert!(e.contains(says), "{text}: {e}");
        };
        refused(local, None, super::MODEL_KEY_FILE_ENV);
        refused(&local.replace("http://127.0.0.1:18087", "http://192.168.50.7:18087"), Some("k"), "base_url");
        refused(&local.replace(r#", "price": { "input": 0, "output": 0 }"#, ""), Some("k"), "price book");
        refused(r#"{ "cheap": { "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3-flash" } }"#, Some("k"), "Workers AI");
    }


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
