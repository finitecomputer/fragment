//! An agent's home brought in through the real CLI against the cell and
//! code.storage (FIN-463 §6): `fragment home survey` and `fragment home
//! files` of the fixture old home (crates/fakes/fixtures/old-home) as a
//! stage brings it, one zip, with a photo large enough to be a blob; then
//! the same again. The plan's rules, the cap, the layouts and the skills'
//! provenance are host-tested (cli/src/home/).

use anyhow::{Context, Result};
use serde_json::json;

use crate::api::Api;
use crate::Suite;

pub fn home(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("home", &[crate::Need::Fakes]) {
        return Ok(());
    }
    let home = s.dir("home-cli");
    s.login(api, &home);
    let keys = s.cli_keys(&home).context("the CLI logged in")?;
    let zip = s.dir("home-stage").join("stage.zip");
    // a photo of 2 MiB: a blob, among the media brought first
    let photo: Vec<u8> = (0..2 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    fragment_fakes::old_home::stage(&zip, &[("workspace/photos/garden.jpg", photo.clone())])?;
    let zip = zip.to_str().expect("a utf-8 path").to_string();
    let layout = fragment_fakes::old_home::layout().to_str().expect("a utf-8 path").to_string();

    // valid: the survey of the stage's zip
    let survey = s.cli_json(api, &home, &["home", "survey", &zip, "--layout", &layout, "--json"])?;
    let comes: Vec<&str> = survey["skills"]["listed"].as_array().into_iter().flatten().filter(|k| k["comes_over"] == true).filter_map(|k| k["name"].as_str()).collect();
    let routine = &survey["routines"]["jobs"][0];
    s.ok(
        "the survey of a stage's zip: its own skills come over, its job is a routine in UTC, its files the cap's pick",
        comes == ["helper", "recipe-tables"]
            && routine["suggestion"] == "routine"
            && routine["schedule_utc"].as_str().is_some_and(|c| c.ends_with("* * mon-fri"))
            && survey["files"]["brought"]["files"] == 8
            && survey["files"]["left"]["files"] == 3,
        json!({ "skills": comes, "routine": routine, "files": survey["files"] }),
    );

    // valid: its files into a fragment of files, the photo as a blob
    let created = s.cli_json(api, &home, &["create", "old-files", "--visibility", "members", "--json"])?;
    let name = created["name"].as_str().unwrap_or_default().to_string();
    let brought = s.cli_json(api, &home, &["home", "files", &zip, "--layout", &layout, "--into", &name, "--json"])?;
    s.ok("home files brings the plan's files into the fragment", brought["files"] == 8 && brought["already"] == 0 && brought["left"] == 3, &brought);
    let file = |p: &str| api.signed(&keys, "GET", &format!("/api/f/{name}/file?path={p}"), None);
    let listed = api.signed(&keys, "GET", &format!("/api/f/{name}/files"), None)?;
    let row = listed.body["files"].as_array().and_then(|l| l.iter().find(|f| f["path"] == "files/photos/garden.jpg").cloned()).unwrap_or_default();
    let big = file("files/photos/garden.jpg")?;
    s.ok("a file of 1 MiB or more is a blob that reads back whole", big.status == 200 && big.bytes == photo && row["sha256"].is_string(), json!({ "status": big.status, "row": row }));
    let left = file("left-behind.md")?;
    let text = String::from_utf8_lossy(&left.bytes).to_string();
    s.ok(
        "left-behind.md lists what stayed behind and why",
        left.status == 200 && text.contains("`projects/feed/.venv/`") && text.contains("`projects/app/dist/`") && text.contains("`models/tiny.gguf`"),
        &text,
    );
    let manifest = file("fragment.json")?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest.bytes).unwrap_or_default();
    s.ok("the fragment's page is the files view", manifest["files"]["page"] == true, &manifest);

    // replay: the same again sends nothing and lands no commit
    let status = api.status(&keys, &name)?;
    let repo = status.body["repo"].as_str().unwrap_or("").to_string();
    let head = s.fake.branch(&repo, "main");
    let again = s.cli_json(api, &home, &["home", "files", &zip, "--layout", &layout, "--into", &name, "--json"])?;
    s.ok("the same again sends nothing twice", again["files"] == 0 && again["already"] == 8 && head.is_some() && s.fake.branch(&repo, "main") == head, &again);

    // a home holding files that may hold secrets (by the layout's path, by
    // a name) reads whole less those: skipped unread, and named; the
    // fixture's own 7 files brought (no photo in this zip)
    let secret = s.dir("home-secret").join("stage.zip");
    fragment_fakes::old_home::stage(&secret, &[("agent/identity/identity.json", b"{}".to_vec()), ("workspace/projects/app/.npmrc", b"//r/:_authToken=x".to_vec())])?;
    let skipped = s.cli_json(api, &home, &["home", "survey", secret.to_str().expect("a utf-8 path"), "--layout", &layout, "--json"])?;
    s.ok(
        "a zip holding files that may hold secrets surveys the rest, and names them skipped",
        skipped["source"]["skipped"] == json!(["agent/identity/identity.json", "workspace/projects/app/.npmrc"]) && skipped["files"]["brought"]["files"] == 7,
        &skipped["source"],
    );
    Ok(())
}
