//! Files moved off a disk and back through the real CLI, against the cell,
//! its blob store and code.storage (Paul, 2026-11-18: a pressure release
//! for a computer whose disk fills): `fragment offload` of a folder holding
//! a blob into a members-only fragment, read there, and `--back`, byte for
//! byte; a fragment open by link is refused with nothing removed. A failed
//! upload or check, the parts of a file past one upload, and replay and
//! restart are host-tested (cli/src/offload.rs).

use anyhow::{Context, Result};
use serde_json::json;

use crate::api::Api;
use crate::Suite;

pub fn offload(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("offload", &[crate::Need::Fakes]) {
        return Ok(());
    }
    let home = s.dir("offload-cli");
    s.login(api, &home);
    let keys = s.cli_keys(&home).context("the CLI logged in")?;
    let created = s.cli_json(api, &home, &["create", "storage", "--visibility", "members", "--json"])?;
    let name = created["name"].as_str().unwrap_or_default().to_string();

    // a folder of a note and a video of 3 MiB: a blob
    let work = s.dir("offload-work");
    let folder = work.join("downloads");
    std::fs::create_dir_all(folder.join("talks"))?;
    let video: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(folder.join("talks/keynote.mp4"), &video)?;
    std::fs::write(folder.join("notes.md"), b"# notes\n")?;
    let path = folder.to_str().expect("a utf-8 path").to_string();
    let note = work.join("downloads.offloaded.md");

    // valid: moved, read back there, and a note in its place
    let moved = s.cli_json(api, &home, &["offload", &path, "--into", &name, "--json"])?;
    let at = moved["at"].as_str().unwrap_or_default().to_string();
    let file = |p: &str| api.signed(&keys, "GET", &format!("/api/f/{name}/file?path={}", crate::api::url_enc(&format!("{at}/{p}"))), None);
    let big = file("downloads/talks/keynote.mp4")?;
    let small = file("downloads/notes.md")?;
    let text = std::fs::read_to_string(&note).unwrap_or_default();
    s.ok(
        "a folder moves into a members-only fragment, a video as a blob that reads back whole there, and a note in its place says where it went and how to bring it back",
        moved["files"] == 2
            && moved["bytes"] == video.len() as u64 + 8
            && big.status == 200
            && big.bytes == video
            && small.bytes == b"# notes\n"
            && !folder.exists()
            && text.starts_with("# Moved to storage")
            && text.contains(&format!("`{name}`"))
            && text.contains("fragment offload --back"),
        json!({ "moved": moved, "big": big.status, "note": text }),
    );

    // back: byte for byte, the note gone, the fragment's copy kept
    let back = s.cli_json(api, &home, &["offload", "--back", note.to_str().expect("a utf-8 path"), "--json"])?;
    let video_back = std::fs::read(folder.join("talks/keynote.mp4")).unwrap_or_default();
    s.ok(
        "--back brings each file back byte for byte, the note goes, and the fragment keeps its copy",
        back["files"] == 2 && back["already"] == 0 && video_back == video && std::fs::read(folder.join("notes.md")).ok().as_deref() == Some(&b"# notes\n"[..]) && !note.exists() && file("downloads/talks/keynote.mp4")?.status == 200,
        &back,
    );

    // invalid: a fragment open to anyone with its link takes nothing, and nothing here is removed
    let open = s.cli_json(api, &home, &["create", "shared", "--json"])?;
    let open = open["name"].as_str().unwrap_or_default().to_string();
    let refused = s.cli(api, &home, &["offload", &path, "--into", &open, "--json"]);
    let said: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap_or_default();
    s.ok(
        "a fragment that is not members-only is refused, and every file stays",
        !refused.status.success() && said["error"]["message"].as_str().is_some_and(|m| m.contains("members-only")) && std::fs::read(folder.join("talks/keynote.mp4")).ok() == Some(video.clone()) && !note.exists(),
        &said,
    );
    Ok(())
}
