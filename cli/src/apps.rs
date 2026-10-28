//! App presentation commands. These are independent of publishing: an
//! agent can reveal its app, then run icon generation as a background
//! terminal task. No create or deploy waits for an image model.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use base64::Engine;
use clap::Subcommand;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{Client, Code, CodedError};

#[derive(Subcommand)]
pub enum IconCmd {
    /// Generate and set the best candidate; slow, so run as a background terminal task after publishing
    Generate {
        name: String,
        /// One concrete subject (at most 200 characters)
        #[arg(long)]
        hint: Option<String>,
    },
    /// Use an image file as the icon (the platform fits and cuts it out)
    Set { name: String, file: PathBuf },
    /// Remove the icon and restore the default glyph
    Remove { name: String },
}

pub fn checked_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > fragment_proto::limits::TITLE_MAX_CHARS {
        return Err(crate::usage(format!("a name is 1 to {} characters", fragment_proto::limits::TITLE_MAX_CHARS)));
    }
    Ok(title.to_string())
}

pub fn default_title(name: &str) -> String {
    let label = name.split_once("--").map_or(name, |(label, _)| label);
    let words = label.replace(['-', '_'], " ");
    let mut chars = words.chars();
    let title: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => words,
    };
    title.chars().take(fragment_proto::limits::TITLE_MAX_CHARS).collect()
}

/// Change just the title, retaining the manifest's operations, metadata
/// and unknown fields. The platform's manifest is main's, not live's.
pub fn set_title(manifest: &mut Value, title: &str) -> Result<()> {
    ensure!(manifest.is_object(), "fragment.json is not an object");
    if manifest["meta"].is_null() { manifest["meta"] = json!({}); }
    ensure!(manifest["meta"].is_object(), "fragment.json's meta is not an object");
    manifest["meta"]["title"] = json!(title);
    Ok(())
}

pub fn rename(c: &Client, name: &str, title: &str) -> Result<Value> {
    let title = checked_title(title)?;
    let name = c.fragment(name)?;
    let path = format!("/api/f/{name}");
    let mut manifest: Value = c.call_as(c.get(&format!("{path}/manifest"))?)?;
    set_title(&mut manifest, &title)?;
    let files = json!([{ "path": "fragment.json", "text": format!("{}\n", serde_json::to_string_pretty(&manifest)?) }]);
    let key = format!("rename-{}", &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(serde_json::to_vec(&files)?))[..16]);
    c.call(c.post_json(&format!("{path}/files"), &json!({ "files": files, "message": "rename", "key": key }))?)?;
    let crate::Deployed::Live { live_tip, .. } = crate::deploy(c, &name, None, None)? else { unreachable!("no folder is synced") };
    crate::refused_live(c, &name, &live_tip)?;
    Ok(json!({ "name": name, "title": title, "live": live_tip }))
}

#[derive(Deserialize)]
struct MadeIcon {
    candidates: Vec<Candidate>,
    flaws: Value,
}

#[derive(Deserialize)]
struct Candidate { png: String }

pub fn icon(c: &Client, command: IconCmd, progress: bool) -> Result<Value> {
    match command {
        IconCmd::Generate { name, hint } => {
            if hint.as_ref().is_some_and(|h| h.chars().count() > 200) { return Err(crate::usage("an icon hint is at most 200 characters")); }
            let name = c.fragment(&name)?;
            let path = format!("/api/f/{name}/icon");
            if progress { eprintln!("Generating an icon for {name}… (up to a minute)"); }
            // The server bounds generation at a minute. Ordinary CLI calls
            // time out at 30 s, which is too short for this one operation.
            let made: MadeIcon = c.call_as(c.post_json_with_timeout(&format!("{path}/make"), &json!({ "hint": hint }), Duration::from_secs(90))?)?;
            let first = made.candidates.first().ok_or_else(|| anyhow::Error::new(CodedError {
                code: Code::AppFailed,
                msg: format!("no usable icon was generated; the existing icon is unchanged. Try another --hint (rejections: {})", made.flaws),
            }))?;
            // The API orders candidates best first. Only the chosen PNG
            // crosses back; base64 is never printed into the transcript.
            let bytes = base64::engine::general_purpose::STANDARD.decode(&first.png).context("the host's icon is not base64")?;
            let result = c.call(c.put_bytes(&path, bytes)?)?;
            Ok(json!({ "name": name, "icon": result["icon"], "sha256": result["sha256"], "size": result["size"] }))
        }
        IconCmd::Set { name, file } => {
            let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            c.call(c.put_bytes(&format!("/api/f/{name}/icon"), bytes)?)
        }
        IconCmd::Remove { name } => c.call(c.delete(&format!("/api/f/{name}/icon"))?),
    }
}

pub fn preview(c: &Client, name: &str) -> Result<Value> {
    c.call(c.post_json(&format!("/api/f/{name}/card"), &json!({}))?)
}

pub fn address(c: &Client, name: &str, label: Option<&str>) -> Result<Value> {
    let path = format!("/api/f/{name}/address");
    match label {
        Some(label) => c.call(c.put_json(&path, &json!({ "label": label }))?),
        None => c.call(c.get(&path)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_useful_and_renaming_keeps_the_rest_of_the_manifest() {
        assert_eq!(default_title("meal-planner--k3x9"), "Meal planner");
        assert_eq!(checked_title("  My app  ").unwrap(), "My app");
        for title in ["", "  ", &"a".repeat(121)] { assert!(checked_title(title).is_err()); }
        let mut manifest = json!({ "meta": { "title": "Blank", "description": "Keep me", "custom": 7 }, "operations": { "list": {} }, "future": true });
        set_title(&mut manifest, "Meal planner").unwrap();
        assert_eq!(manifest, json!({ "meta": { "title": "Meal planner", "description": "Keep me", "custom": 7 }, "operations": { "list": {} }, "future": true }));
        assert!(set_title(&mut json!([]), "Name").is_err());
        assert!(set_title(&mut json!({ "meta": 4 }), "Name").is_err());
    }

    #[test]
    fn scaffolding_names_the_app_and_never_overwrites_an_existing_name() {
        let dir = std::env::temp_dir().join(format!("fragment-naming-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::scaffold(&dir, "blank").unwrap();
        let path = dir.join("fragment.json");
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_ne!(manifest["meta"]["title"], "Blank");
        set_title(&mut manifest, "My chosen title").unwrap();
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        crate::scaffold(&dir, "blank").unwrap();
        let again: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(again["meta"]["title"], "My chosen title");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
