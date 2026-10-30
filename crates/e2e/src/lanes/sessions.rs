//! A person's session archive (templates/sessions/README.md) through the
//! real CLI against the cell and code.storage: `fragment session import`
//! of the old Finite's export and of Hermes' state.db, both synthetic
//! fixtures made here; the same import again, a newer copy, an import
//! after a crash, what it refuses, and an agent acting for its owner
//! reaching the same archive. Hermes' chains and messages, the export's
//! layout and the merge are host-tested (cli/src/session/,
//! crates/core/src/session.rs).

use std::path::Path;

use anyhow::{Context, Result};
use fragment_core::session::{self as archive, Kind, Message, Session};
use serde_json::{json, Value};

use crate::api::Api;
use crate::Suite;

/// The old Finite's export's chats, as its export.py writes them: a chat,
/// a chat large enough to be a blob, a month's scheduled runs, and a chat
/// past what a conversation may hold.
fn export(root: &Path) -> Result<()> {
    let chats = root.join("chats");
    std::fs::create_dir_all(chats.join("sessions"))?;
    std::fs::create_dir_all(chats.join("runs"))?;
    let format = "finite-chat-archive/1";
    let chat = |id: &str, title: &str, at: &str, words: Vec<Value>| {
        json!({ "format": format, "id": id, "file": format!("chats/sessions/{id}.json"), "title": title, "kind": "chat", "source": "finitechat",
            "with": "Ann", "started_at": at, "ended_at": at, "messages": words, "sessions": [id] })
    };
    let said = |role: &str, at: &str, text: &str| json!({ "role": role, "at": at, "text": text });
    let beans = chat("e2e_beans", "Beans", "2026-10-01T09:00:00Z", vec![said("user", "2026-10-01T09:00:00Z", "plant beans?"), said("assistant", "2026-10-01T09:00:05Z", "in spring")]);
    let big = chat("e2e_big", "A long letter", "2026-10-05T09:00:00Z", vec![said("user", "2026-10-05T09:00:00Z", &"a long letter ".repeat(110_000))]);
    let huge = chat("e2e_huge", "Too long", "2026-10-06T09:00:00Z", vec![said("user", "2026-10-06T09:00:00Z", &"w".repeat(archive::SESSION_MAX_BYTES + 1))]);
    let run = json!({ "id": "e2e_digest", "file": "chats/runs/cron-2026-10.json", "title": "Digest", "kind": "cron", "started_at": "2026-10-02T06:00:00Z",
        "messages": [said("user", "2026-10-02T06:00:00Z", "the morning digest")], "sessions": ["e2e_digest"] });
    let mut listed = vec![];
    for c in [&beans, &big, &huge] {
        let id = c["id"].as_str().unwrap_or_default();
        std::fs::write(chats.join(format!("sessions/{id}.json")), c.to_string())?;
        listed.push(json!({ "id": id, "file": c["file"], "title": c["title"], "kind": "chat" }));
    }
    std::fs::write(chats.join("runs/cron-2026-10.json"), json!({ "format": format, "conversations": { "e2e_digest": run } }).to_string())?;
    listed.push(json!({ "id": "e2e_digest", "file": "chats/runs/cron-2026-10.json", "kind": "cron" }));
    std::fs::write(chats.join("index.json"), json!({ "format": format, "agent": "Juniper", "conversations": listed }).to_string())?;
    Ok(())
}

/// A Hermes database (the columns of v0.21.7's a read uses): the export's
/// first chat again, a reply longer, and a chat Hermes compressed once.
fn database(path: &Path) -> Result<()> {
    let db = rusqlite::Connection::open(path)?;
    db.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, session_key TEXT, display_name TEXT, model TEXT, model_config TEXT,
            parent_session_id TEXT, started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, title TEXT, last_activity_at REAL);
         CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT,
            tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL NOT NULL, reasoning TEXT, reasoning_content TEXT,
            _compressed_summary INTEGER NOT NULL DEFAULT 0, active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0,
            display_kind TEXT, display_metadata TEXT);
         -- 2026-10-01T09:00:00Z, and a reply the export did not have
         INSERT INTO sessions VALUES ('e2e_beans','finitechat','fc:1','Ann',NULL,NULL,NULL,1790845200,NULL,NULL,'Beans',NULL);
         INSERT INTO messages (session_id,role,content,timestamp) VALUES ('e2e_beans','user','plant beans?',1790845200);
         INSERT INTO messages (session_id,role,content,timestamp) VALUES ('e2e_beans','assistant','in spring',1790845205);
         INSERT INTO messages (session_id,role,content,timestamp) VALUES ('e2e_beans','user','and peas?',1791104400);
         -- a chat compressed into a second session
         INSERT INTO sessions VALUES ('e2e_long','cli','cli:1',NULL,NULL,NULL,NULL,1790931600,1790931700,'compression','Long',NULL);
         INSERT INTO sessions VALUES ('e2e_long_2','cli','cli:1',NULL,NULL,'{}','e2e_long',1790931700,NULL,NULL,NULL,NULL);
         INSERT INTO messages (session_id,role,content,timestamp) VALUES ('e2e_long','user','one',1790931600);
         INSERT INTO messages (session_id,role,content,timestamp) VALUES ('e2e_long_2','user','two',1790931800);",
    )?;
    Ok(())
}

fn code(out: &std::process::Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

pub fn sessions(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("sessions", &[crate::Need::Fakes, crate::Need::Levers]) {
        return Ok(());
    }
    let home = s.dir("sessions-home");
    s.login(api, &home);
    let keys = s.cli_keys(&home).context("the CLI logged in")?;
    let me = api.identity(&keys)?;
    let root = s.dir("sessions-export");
    export(&root)?;
    let path = |p: &Path| p.to_str().expect("utf-8 path").to_string();

    // valid: the first import makes the person's archive, private, and
    // files each conversation, the one too large refused alone
    let first = s.cli_json(api, &home, &["session", "import", &path(&root), "--json"])?;
    let name = first["archive"].as_str().unwrap_or("").to_string();
    let refused: Vec<&str> = first["refused"].as_array().into_iter().flatten().filter_map(|r| r["id"].as_str()).collect();
    s.ok(
        "an export's chats are imported into an archive made for its person, the one past the limit refused alone",
        first["made"] == true && first["added"] == 3 && refused == ["e2e_huge"] && first["holds"] == json!({ "chat": 2, "cron": 1, "subagent": 0 }),
        &first,
    );
    let status = api.status(&keys, &name)?;
    let listed = api.signed(&keys, "GET", "/api/fragments", None)?;
    let row = listed.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == name.as_str()).cloned()).unwrap_or_default();
    s.ok(
        "the archive is the person's own, of kind sessions, members only, on the blessed template",
        row["kind"] == "sessions" && row["role"] == "owner" && status.body["visibility"] == "members" && status.body["title"] == archive::TITLE,
        json!({ "row": row, "status": status.body }),
    );
    let file = |k: &fragment_nip98::Keys, p: &str| api.signed(k, "GET", &format!("/api/f/{name}/file?path={p}"), None);
    let index = file(&keys, "index/chat.json")?;
    let keys_of = |r: &crate::api::Reply| -> Vec<String> { r.body["sessions"].as_array().into_iter().flatten().filter_map(|e| e["key"].as_str().map(str::to_string)).collect() };
    let files = api.signed(&keys, "GET", &format!("/api/f/{name}/files"), None)?;
    let big_row = files.body["files"].as_array().and_then(|l| l.iter().find(|f| f["path"] == "sessions/hermes-e2e_big.json").cloned()).unwrap_or_default();
    let big = file(&keys, "sessions/hermes-e2e_big.json")?;
    let big_read = serde_json::from_slice::<Session>(&big.bytes).ok();
    s.ok(
        "its chats' index lists them newest first, and a conversation of 1 MiB or more is a blob that reads back whole",
        keys_of(&index) == ["hermes-e2e_big", "hermes-e2e_beans"] && big_row["sha256"].is_string() && big_read.is_some_and(|b| b.messages[0].text.as_ref().is_some_and(|t| t.len() > 1_500_000)),
        json!({ "index": index.body, "big": big_row }),
    );

    // replay: the same import again adds nothing and commits nothing
    let repo = status.body["repo"].as_str().unwrap_or("").to_string();
    let packs = s.fake.requests(&repo, "POST commit-pack");
    let again = s.cli_json(api, &home, &["session", "import", &path(&root.join("chats")), "--json"])?;
    s.ok(
        "the same import again adds nothing twice and commits nothing",
        again["archive"] == name.as_str() && again["made"] == false && again["added"] == 0 && again["unchanged"] == 3 && s.fake.requests(&repo, "POST commit-pack") == packs,
        &again,
    );

    // a newer copy: Hermes' own database holds the first chat with a reply
    // more, and a chat it compressed, which is one conversation
    let db_dir = s.dir("sessions-hermes");
    database(&db_dir.join("state.db"))?;
    let newer = s.cli_json(api, &home, &["session", "import", &path(&db_dir), "--json"])?;
    let beans = serde_json::from_slice::<Session>(&file(&keys, "sessions/hermes-e2e_beans.json")?.bytes).ok();
    let long = serde_json::from_slice::<Session>(&file(&keys, "sessions/hermes-e2e_long.json")?.bytes).ok();
    s.ok(
        "Hermes' database updates a chat it holds a newer copy of and adds one it compressed, as one conversation",
        newer["updated"] == 1 && newer["added"] == 1 && beans.is_some_and(|b| b.messages.len() == 3) && long.is_some_and(|l| l.parts == ["e2e_long", "e2e_long_2"] && l.messages.len() == 2),
        &newer,
    );

    // restart: an import that stopped after its conversations, before its
    // indexes, is put right by the next
    let orphan = Session {
        format: String::new(),
        id: "e2e_orphan".into(),
        origin: "hermes".into(),
        kind: Kind::Subagent,
        title: "Left behind".into(),
        source: None,
        with: None,
        model: None,
        started_at: Some("2026-10-04T09:00:00Z".into()),
        ended_at: None,
        parts: vec![],
        messages: vec![Message { role: "user".into(), at: Some("2026-10-04T09:00:00Z".into()), text: Some("a side task".into()), ..Default::default() }],
    }
    .normalized();
    let orphan_path = archive::session_path("hermes-e2e_orphan");
    let made = json!({ "repo": repo, "name": name });
    s.owned(&made, &keys);
    s.commit(&made, &[(orphan_path.as_str(), Some(&orphan.file()))]);
    let repaired = s.cli_json(api, &home, &["session", "import", &path(&root), "--json"])?;
    let side = file(&keys, "index/subagent.json")?;
    s.ok("an import after one that stopped before its indexes lists what that one wrote", repaired["added"] == 0 && keys_of(&side) == ["hermes-e2e_orphan"], json!({ "import": repaired, "index": side.body }));

    // invalid: a path that holds nothing an import reads
    let empty = s.dir("sessions-empty");
    let out = s.cli(api, &home, &["session", "import", &path(&empty), "--json"]);
    let said: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    s.ok("a folder holding no store is refused, saying what to name", code(&out) == 2 && said["error"]["code"] == "invalid_usage", String::from_utf8_lossy(&out.stdout));

    // the person's agent, acting for them, reaches the same archive as an
    // editor (the CLI on its computer acts so); someone else, none of it
    let (agent, _) = super::delegation::agent_of(api, &keys)?;
    let for_me = |p: &str| api.signed(&agent, "GET", &format!("{p}{}for={me}", if p.contains('?') { "&" } else { "?" }), None);
    let theirs = for_me("/api/fragments")?;
    let found = theirs.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["kind"] == "sessions").cloned()).unwrap_or_default();
    let token = for_me(&format!("/api/f/{name}/storage-token"))?;
    let read = for_me(&format!("/api/f/{name}/file?path=index/chat.json"))?;
    s.ok(
        "an agent acting for its owner finds their archive, owned by them, and may read and write it",
        found["name"] == name.as_str() && found["owned"] == true && token.status == 200 && read.status == 200,
        json!({ "found": found, "token": token.status, "read": read.status }),
    );
    let stranger = api.person()?;
    let r = file(&stranger, "index/chat.json")?;
    s.ok("anyone else reads none of it", matches!(r.status, 403 | 404), r.status);
    Ok(())
}
