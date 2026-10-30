//! The images, built and run in Docker (linux/amd64) against the fake
//! fragment API and a scripted model on this host, reached from the
//! container as `api.fragment.internal` and `model.fragment.internal`
//! (`--add-host …:host-gateway`), as the Computer DO's intercepts would be.
//!
//! Lower rung, labeled so (docs/cloudflare-v1.md, Evaluation): the real
//! images and the real Hermes, but local Docker in place of Containers, and
//! fakes at the platform's boundary.
//!
//! Run: `cargo test -p fragment-bridge --test docker -- --ignored --nocapture`
//! (`FRAGMENT_DOCKER_SKIP_BUILD=1` reuses images already built;
//! `FRAGMENT_DOCKER_HERMES_TAG` names the Hermes image's tag). CI runs it
//! (.github/workflows/images.yml, `docker`) on pull requests and master's
//! pushes that touch images/hermes, images/bridge or images/stub: the images
//! built first, then this with `FRAGMENT_DOCKER_SKIP_BUILD=1`.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;

use support::fake::{person, Fake};
use support::model::{Model, CHEAP_ID, FALLBACK_ID, ROUTE_FALLBACK_ID};

/// An install for the session, offline (as the e2e's hermes lane has it):
/// a package Hermes builds and installs through apt as root, and a program
/// it puts in /usr/local/bin as root; then both run, on one line.
const INSTALL: &str = r#"d=/tmp/fragment-hello && mkdir -p $d/DEBIAN $d/usr/bin && printf 'Package: fragment-hello\nVersion: 1.0\nArchitecture: all\nMaintainer: e2e <e2e@e2e.test>\nDescription: a package an agent installs\n' > $d/DEBIAN/control && printf '#!/bin/sh\necho hello-from-apt\n' > $d/usr/bin/fragment-hello && chmod 0755 $d $d/DEBIAN $d/usr/bin/fragment-hello && dpkg-deb --build --root-owner-group $d /tmp/fragment-hello.deb > /dev/null && sudo apt-get install -y /tmp/fragment-hello.deb > /dev/null 2>&1 && printf '#!/bin/sh\necho hello-from-usr-local\n' > /tmp/fragment-hi && sudo install -m 0755 /tmp/fragment-hi /usr/local/bin/fragment-hi && echo "$(fragment-hello) $(fragment-hi)""#;

fn docker() -> String {
    std::env::var("DOCKER").unwrap_or_else(|_| if std::path::Path::new("/usr/local/bin/docker").exists() { "/usr/local/bin/docker".into() } else { "docker".into() })
}

fn images_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("images/").to_path_buf()
}

/// The repo's root: the Hermes image's build context (it carries the
/// fragment CLI, of the root workspace).
fn repo_dir() -> PathBuf {
    images_dir().parent().expect("the repo").to_path_buf()
}

/// Builds `dockerfile` (relative to `context`) as `tag`.
fn build(context: &std::path::Path, dockerfile: &str, tag: &str) -> Duration {
    let t = Instant::now();
    if std::env::var("FRAGMENT_DOCKER_SKIP_BUILD").is_ok() {
        return Duration::ZERO;
    }
    let status = Command::new(docker()).args(["build", "--platform", "linux/amd64", "-f", dockerfile, "-t", tag, "."]).current_dir(context).status().expect("docker runs");
    assert!(status.success(), "docker build {dockerfile}");
    t.elapsed()
}

/// The Hermes image's tag (`FRAGMENT_DOCKER_HERMES_TAG`, so two checkouts
/// on one Docker never build over each other's).
fn hermes_tag() -> String {
    std::env::var("FRAGMENT_DOCKER_HERMES_TAG").unwrap_or_else(|_| "fragment-hermes:test".into())
}

fn size(tag: &str) -> u64 {
    let out = Command::new(docker()).args(["image", "inspect", tag, "--format", "{{.Size}}"]).output().expect("docker runs");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

struct Container {
    id: String,
}

impl Drop for Container {
    fn drop(&mut self) {
        // Its log, kept for reading after the test (target/tmp/<id>.log).
        let _ = std::fs::write(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("container-{}.log", &self.id[..12.min(self.id.len())])), self.logs());
        let _ = Command::new(docker()).args(["rm", "-f", &self.id]).output();
    }
}

/// Where the container runs: Docker as it is, or as Cloudflare Containers
/// runs an image, where Docker's defaults differ.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Runtime {
    Docker,
    /// No `/dev/shm` (`--ipc=none`; Docker mounts a 64 MB one in every
    /// container, Containers none), and no `/.dockerenv`, Docker's marker,
    /// which Hermes reads as "in a container": to start Chromium with
    /// `--no-sandbox --disable-dev-shm-usage` (p5, 2026-10-05: the agent's
    /// browser died for want of `/dev/shm`), and for the rest of its
    /// container guesses (docs/computers.md). Without it Hermes takes this
    /// for a host.
    Hosted,
}

impl Container {
    fn run(tag: &str, api: u16, model: u16, extra: &[(&str, &str)]) -> Container {
        Container::run_on(Runtime::Docker, tag, api, model, extra)
    }

    fn run_on(runtime: Runtime, tag: &str, api: u16, model: u16, extra: &[(&str, &str)]) -> Container {
        Container::run_with(runtime, tag, api, model, extra, &[])
    }

    /// As `run_on`, with more of `docker run`'s own arguments (`--network`).
    fn run_with(runtime: Runtime, tag: &str, api: u16, model: u16, extra: &[(&str, &str)], docker_args: &[&str]) -> Container {
        let mut args: Vec<String> = ["run", "-d", "--platform", "linux/amd64", "--add-host", "api.fragment.internal:host-gateway", "--add-host", "model.fragment.internal:host-gateway"].iter().map(|s| s.to_string()).collect();
        args.extend(docker_args.iter().map(|s| s.to_string()));
        if runtime == Runtime::Hosted {
            // the marker goes before the image's own entrypoint runs, as PID 1
            args.extend(["--ipc=none", "--entrypoint", "/bin/sh"].map(String::from));
        }
        let given = [
            ("FRAGMENT_API", format!("http://api.fragment.internal:{api}")),
            ("FRAGMENT_MODEL", format!("http://model.fragment.internal:{model}")),
            ("FRAGMENT_COMPUTER", "computer:00aa".into()),
            ("FRAGMENT_IMAGE", tag.into()),
        ];
        for (k, v) in given {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        for (k, v) in extra {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        // its screen's port, on a port of this host's loopback (`port`)
        args.extend(["-p".into(), "127.0.0.1::6080".into()]);
        args.push(tag.into());
        if runtime == Runtime::Hosted {
            args.extend(["-c", "rm -f /.dockerenv && exec /opt/fragment/bin/hermes-boot pre-init"].map(String::from));
        }
        let out = Command::new(docker()).args(&args).output().expect("docker runs");
        assert!(out.status.success(), "docker run {tag}: {}", String::from_utf8_lossy(&out.stderr));
        Container { id: String::from_utf8_lossy(&out.stdout).trim().to_string() }
    }

    /// The host's port its `port` is published on.
    fn port(&self, port: u16) -> u16 {
        let out = Command::new(docker()).args(["port", &self.id, &format!("{port}/tcp")]).output().expect("docker runs");
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().find_map(|l| l.rsplit_once(':').and_then(|(_, p)| p.trim().parse().ok())).unwrap_or_else(|| panic!("{port} is not published: {text}"))
    }

    fn exec(&self, cmd: &[&str]) -> bool {
        Command::new(docker()).arg("exec").arg(&self.id).args(cmd).output().is_ok_and(|o| o.status.success())
    }

    fn exec_out(&self, cmd: &[&str]) -> String {
        let out = Command::new(docker()).arg("exec").arg(&self.id).args(cmd).output().expect("docker runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `cmd`'s exit code and what it printed.
    fn exec_code(&self, cmd: &[&str]) -> (i32, String) {
        let out = Command::new(docker()).arg("exec").arg(&self.id).args(cmd).output().expect("docker runs");
        (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    /// A container of `tag` that runs nothing but a sleep: a disk to restore
    /// into and check, as a fresh start's is before its gate opens.
    fn idle(tag: &str) -> Container {
        let out = Command::new(docker()).args(["run", "-d", "--platform", "linux/amd64", "--entrypoint", "/bin/sleep", tag, "infinity"]).output().expect("docker runs");
        assert!(out.status.success(), "docker run {tag}: {}", String::from_utf8_lossy(&out.stderr));
        Container { id: String::from_utf8_lossy(&out.stdout).trim().to_string() }
    }

    fn logs(&self) -> String {
        let out = Command::new(docker()).args(["logs", &self.id]).output().expect("docker runs");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    /// Restarted, its disk kept: as a computer's next start after a deploy
    /// or a sleep finds its `/data`.
    fn restart(&self) {
        let out = Command::new(docker()).args(["restart", "-t", "20", &self.id]).output().expect("docker runs");
        assert!(out.status.success(), "docker restart: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// SIGTERM, as the Computer DO's sleep sends it: how long until it is
    /// gone, and its exit code.
    fn sigterm(&self) -> (Duration, i64) {
        let t = Instant::now();
        assert!(Command::new(docker()).args(["kill", "--signal", "TERM", &self.id]).output().expect("docker runs").status.success());
        let out = Command::new(docker()).args(["wait", &self.id]).output().expect("docker runs");
        (t.elapsed(), String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(-1))
    }
}

/// Goal: the stub image starts in well under a second, answers a chat
/// message through its bridge, holds at its restore gate until the marker,
/// and exits within 5 s of SIGTERM.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn the_stub_image() {
    let built = build(&images_dir(), "stub/Dockerfile", "fragment-stub:test");
    eprintln!("stub: built in {:.1} s, {} MB", built.as_secs_f64(), size("fragment-stub:test") / 1_000_000);
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);

    let t = Instant::now();
    let c = Container::run("fragment-stub:test", fake.addr.port(), model.addr.port(), &[]);
    fake.until(30_000, "the stub's bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let ready = t.elapsed();
    eprintln!("stub: docker run to following its chat: {} ms", ready.as_millis());
    // no SimpleX core in it: its reads never say SimpleX
    assert!(fake.with(|w| w.simplex_said == 0 && w.simplex_unsaid >= 1), "the stub's reads of its computer say no SimpleX");
    fake.say(&chat, &person("paul"), json!({ "text": "hello stub" }));
    let asked = Instant::now();
    fake.until(20_000, "the stub's reply", |w| w.bodies(&chat, "chat", "reply").len() == 1).await;
    eprintln!("stub: message to reply: {} ms", asked.elapsed().as_millis());
    fake.with(|w| assert_eq!(w.bodies(&chat, "chat", "reply")[0]["text"], "echo: [paul] hello stub"));
    // Its turn's last posts (its end, then its draft stopped) answered, so
    // none is still in flight when the gate's count below begins (the
    // blocking `docker` calls hold this runtime, and the fake with it).
    fake.until(20_000, "the stub's turn to end and its draft to stop", |w| {
        let end = w.records(&chat, "work").iter().find(|r| r["body"]["kind"] == "turn.end").and_then(|r| r["seq"].as_u64());
        let ended_at = end.and_then(|seq| w.log.iter().position(|l| *l == format!("record {chat} work {seq}")));
        ended_at.is_some_and(|i| w.log[i + 1..].iter().any(|l| l.starts_with(&format!("draft {chat} ")) && l.ends_with(" null")))
    })
    .await;
    // Its screen port serves its page.
    assert!(c.exec_out(&["wget", "-qO-", "http://127.0.0.1:6080/"]).contains("This computer has no screen"));
    let (took, code) = c.sigterm();
    eprintln!("stub: SIGTERM to exit: {} ms (code {code})", took.as_millis());
    assert!(took < Duration::from_secs(5), "{took:?}\n{}", c.logs());
    assert_eq!(code, 0);

    // The restore gate: nothing is asked of the platform until the marker.
    let calls_before = fake.with(|w| w.calls.len());
    let gated = Container::run("fragment-stub:test", fake.addr.port(), model.addr.port(), &[("RESTORE_PENDING", "1")]);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(fake.with(|w| w.calls.len()), calls_before, "the gate holds: {:?}\n{}", fake.with(|w| w.calls[calls_before.min(w.calls.len())..].to_vec()), gated.logs());
    let opened = Instant::now();
    assert!(gated.exec(&["touch", "/run/computer/restored"]), "the marker, as the DO touches it");
    fake.until(30_000, "the gated stub to follow", |w| w.live_sockets() >= 2).await;
    eprintln!("stub: marker to following: {} ms", opened.elapsed().as_millis());
}

/// Goal: the real Hermes image answers a chat message through its bridge
/// with the scripted model, as the agent's profile, with every model call
/// naming its agent; it holds at its restore gate; and SIGTERM ends it in
/// under 5 s.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn the_hermes_image() {
    let tag = hermes_tag();
    let built = build(&repo_dir(), "images/hermes/Dockerfile", &tag);
    eprintln!("hermes: built in {:.1} s, {} MB", built.as_secs_f64(), size(&tag) / 1_000_000);
    // A changed Hermes pin must update the boot's sandbox policy too.
    let policy = Command::new(docker()).args(["run", "--rm", "--entrypoint", "/opt/hermes/.venv/bin/python", &tag, "-c", "from tools.environments.local_env_policy import _HERMES_PROVIDER_ENV_BLOCKLIST; print('\\n'.join(sorted(_HERMES_PROVIDER_ENV_BLOCKLIST)))"]).output().expect("read Hermes' policy");
    assert!(policy.status.success(), "{}", String::from_utf8_lossy(&policy.stderr));
    assert_eq!(String::from_utf8(policy.stdout).unwrap(), include_str!("../../hermes/boot/src/provider-env-blocklist.txt"), "the boot's policy is the pinned Hermes' own");
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    fake.with(|w| {
        let f = w.fragments.get_mut("juniper--k3x9").unwrap();
        f.files.insert("SOUL.md".into(), bytes::Bytes::from_static(b"You are Juniper, a careful gardener.\n"));
        f.files.insert("memories/MEMORY.md".into(), bytes::Bytes::from_static(b"Paul likes tomatoes.\n"));
        f.files.insert("agent.json".into(), bytes::Bytes::from_static(br#"{"tier":"cheap"}"#));
        // its own skills: one of its own, and one a managed skill also names
        f.files.insert("skills/garden-notes/SKILL.md".into(), bytes::Bytes::from_static(b"---\nname: garden-notes\ndescription: Juniper's own notes on the garden.\n---\n# Garden notes\n"));
        f.files.insert("skills/systematic-debugging/SKILL.md".into(), bytes::Bytes::from_static(b"---\nname: systematic-debugging\ndescription: Juniper's own debugging, which wins.\n---\n"));
        f.files.insert("skills/grill-me/SKILL.md".into(), bytes::Bytes::from_static(b"---\nname: grill-me\ndescription: Juniper's own grilling, which wins.\n---\n# Grill\n"));
    });
    // paul's skills fragment: the managed set (the blessed template's release, served as its files)
    let skills = fake.skills(
        "skills",
        &[
            ("fragment.json", r#"{"template":"skills"}"#),
            ("skills/research/model-council-finite/SKILL.md", include_str!("../../../templates/skills/skills/research/model-council-finite/SKILL.md")),
            ("skills/research/model-council-finite/scripts/model_council.py", include_str!("../../../templates/skills/skills/research/model-council-finite/scripts/model_council.py")),
            ("skills/grill-me/SKILL.md", "---\nname: grill-me\ndescription: The managed grilling.\n---\n# Grill\n"),
            ("skills/software-development/apps-finite/SKILL.md", include_str!("../../../templates/skills/skills/software-development/apps-finite/SKILL.md")),
            ("skills/software-development/git-finite/SKILL.md", include_str!("../../../templates/skills/skills/software-development/git-finite/SKILL.md")),
            ("skills/research/brain-finite/SKILL.md", include_str!("../../../templates/skills/skills/research/brain-finite/SKILL.md")),
            ("skills/productivity/google-workspace-finite/SKILL.md", include_str!("../../../templates/skills/skills/productivity/google-workspace-finite/SKILL.md")),
            ("skills/image-editing/image-generation-finite/SKILL.md", include_str!("../../../templates/skills/skills/image-editing/image-generation-finite/SKILL.md")),
            ("skills/finance/cocod-finite/SKILL.md", include_str!("../../../templates/skills/skills/finance/cocod-finite/SKILL.md")),
            ("skills/finance/trading-agent-finite/SKILL.md", include_str!("../../../templates/skills/skills/finance/trading-agent-finite/SKILL.md")),
            ("skills/nostr/nostr-agent-interface-cli-finite/SKILL.md", include_str!("../../../templates/skills/skills/nostr/nostr-agent-interface-cli-finite/SKILL.md")),
            ("skills/nostr/nostr-agent-interface-cli-finite/agents/openai.yaml", include_str!("../../../templates/skills/skills/nostr/nostr-agent-interface-cli-finite/agents/openai.yaml")),
            ("skills/nostr/nostr-agent-interface-cli-finite/references/cli-essentials.md", include_str!("../../../templates/skills/skills/nostr/nostr-agent-interface-cli-finite/references/cli-essentials.md")),
            ("skills/nostr/nostr-agent-interface-cli-finite/references/tool-groups.md", include_str!("../../../templates/skills/skills/nostr/nostr-agent-interface-cli-finite/references/tool-groups.md")),
            ("skills/nostr/nostr-agent-interface-cli-finite/references/workflows.md", include_str!("../../../templates/skills/skills/nostr/nostr-agent-interface-cli-finite/references/workflows.md")),
            ("skills/music-generation-finite/SKILL.md", include_str!("../../../templates/skills/skills/music-generation-finite/SKILL.md")),
            ("skills/research/polymarket-finite/SKILL.md", include_str!("../../../templates/skills/skills/research/polymarket-finite/SKILL.md")),
            ("skills/research/polymarket-finite/references/api-endpoints.md", include_str!("../../../templates/skills/skills/research/polymarket-finite/references/api-endpoints.md")),
            ("skills/research/polymarket-finite/scripts/polymarket.py", include_str!("../../../templates/skills/skills/research/polymarket-finite/scripts/polymarket.py")),
            ("skills/social-media/x-api-finite/SKILL.md", include_str!("../../../templates/skills/skills/social-media/x-api-finite/SKILL.md")),
            ("skills/social-media/x-api-finite/x-api.py", include_str!("../../../templates/skills/skills/social-media/x-api-finite/x-api.py")),
        ],
    );
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);

    let t = Instant::now();
    let c = Container::run(&tag, fake.addr.port(), model.addr.port(), &[("RESTORE_PENDING", "1"), ("HERMES_BOOT_SYNC_MS", "2000"), ("HERMES_BOOT_SKILLS_MS", "2000")]);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(fake.with(|w| w.calls.is_empty()), "the gate holds: {:?}", fake.with(|w| w.calls.clone()));
    let gate = Instant::now();
    assert!(c.exec(&["touch", "/run/computer/restored"]));
    fake.until(120_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    eprintln!("hermes: marker to the bridge following: {} ms ({} ms since docker run)", gate.elapsed().as_millis(), t.elapsed().as_millis());

    // Ready means Hermes answers: the first message waits for its gateway.
    let first = fake.say(&chat, &person("paul"), json!({ "text": "hello hermes" }));
    let t1 = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", first["seq"].as_u64().unwrap());
    let asked = Instant::now();
    let answered = |w: &support::fake::World, turn: &str| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn);
    let replied = tokio::time::timeout(Duration::from_secs(180), fake.until(600_000, "Hermes' reply", |w| answered(w, &t1).is_some()));
    if replied.await.is_err() {
        let logs = c.logs();
        let _ = std::fs::write(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hermes-container.log"), &logs);
        let records = fake.with(|w| format!("chat: {:?}\nwork: {:?}", w.bodies(&chat, "chat", "reply"), w.records(&chat, "work").iter().map(|r| r["body"].clone()).collect::<Vec<_>>()));
        panic!("no reply; the fake holds\n{records}\nthe container said:\n{}", logs.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    eprintln!("hermes: first message to reply: {} ms ({} ms since the marker)", asked.elapsed().as_millis(), gate.elapsed().as_millis());
    fake.until(30_000, "the turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t1)).await;
    fake.with(|w| {
        let reply = answered(w, &t1).unwrap();
        assert!(reply["text"].as_str().unwrap().contains("hello hermes"), "the scripted model's answer: {reply}");
        let by = w.records(&chat, "chat").into_iter().find(|r| r["body"]["turn"] == t1).unwrap()["principal"].clone();
        assert_eq!(by, "npub1juniper");
        assert_eq!(w.bodies(&chat, "work", "turn.end").into_iter().find(|e| e["turn"] == t1).unwrap()["outcome"], "idle");
        let all: Vec<String> = w.bodies(&chat, "chat", "reply").iter().map(|r| r["text"].as_str().unwrap_or("").to_string()).collect();
        assert!(all.iter().all(|t| !t.contains("home channel")), "no notice from Hermes about its home: {all:?}");
    });
    {
        // Hermes also probes `/api/show` (Ollama's model metadata) with no
        // agent; the model intercept answers only `/v1/models` and the
        // completions routes, and Hermes keeps its configured context length.
        let calls: Vec<_> = model.calls.lock().unwrap().iter().filter(|c| c.path.ends_with("/chat/completions")).cloned().collect();
        assert!(!calls.is_empty());
        assert!(calls.iter().all(|c| c.agent.as_deref() == Some("juniper--k3x9")), "every model call names its agent: {calls:?}");
        let tiers: Vec<_> = calls.iter().filter(|c| c.model == CHEAP_ID).collect();
        assert!(!tiers.is_empty() && tiers.iter().all(|c| c.tier.as_deref() == Some("cheap")), "the agent's tier from agent.json, its model by its id, its tier said: {calls:?}");
        assert!(calls.iter().all(|c| c.model != "cheap"), "no call names the tier's name: {calls:?}");
        // the config Hermes reads (Paul, 2026-10-16: "see the real model"):
        // its tier's model and its fallback by their ids, as it was given them
        let config = c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/config.yaml"]);
        for want in [
            format!("  default: \"{CHEAP_ID}\"\n"),
            "    x-fragment-tier: \"cheap\"\n".to_string(),
            format!("      \"{CHEAP_ID}\":\n        stale_timeout_seconds: 20\n"),
            format!("fallback_providers:\n  - provider: \"custom\"\n    base_url: \"http://model.fragment.internal:{}/v1\"\n    model: \"{ROUTE_FALLBACK_ID}\"\n", model.addr.port()),
            "    model: \"vision\"\n".to_string(),
            "\n  tool_use_enforcement: false\n  execution_guidance: false\n  api_max_retries: 1\n".to_string(),
        ] {
            assert!(config.contains(&want), "its config has {want:?}:\n{config}");
        }
        assert_eq!(config.matches("\nagent:\n").count(), 1, "one `agent` mapping, its name, its guidance and its retry in it:\n{config}");
        // and the prompt Hermes gives that model names it by its id, a
        // GLM's, with none of the guidance Hermes gives its family (its
        // `_guidance_parts`: tool-use enforcement, execution discipline),
        // as when it was named `cheap`; every model's own blocks stand
        // (hermes.rs, `profile_config`)
        let prompts: Vec<String> = calls
            .iter()
            .flat_map(|c| c.body["messages"].as_array().into_iter().flatten().filter(|m| m["role"] == "system").map(content_text))
            .filter(|p| p.contains(&format!("\nModel: {CHEAP_ID}")))
            .collect();
        assert!(prompts.iter().any(|p| p.contains("# Finishing the job") && p.contains("# Parallel tool calls")), "a turn's prompt names its model by its id, with every model's guidance: {prompts:?}");
        assert!(prompts.iter().all(|p| !p.contains("# Tool-use enforcement") && !p.contains("# Execution discipline")), "none of its family's: {prompts:?}");
        assert!(c.logs().lines().any(|l| l.contains("\"boot.model_ids\"") && l.contains(CHEAP_ID)), "the boot says the ids its view named");
        // A fresh profile's first message carries no first-contact note of
        // Hermes' (v0.21.5 appended "This is the user's very first message
        // ever…", and the model introduced itself and named /help; since
        // v0.21.6 Hermes adds it only in a direct message, and the bridge's
        // chats are groups): the model is asked exactly what was said.
        let first: Vec<String> = calls.iter().map(|c| c.body["messages"].to_string()).filter(|m| m.contains("hello hermes")).collect();
        assert!(!first.is_empty() && first.iter().all(|m| !m.contains("very first message")), "no first-contact note in the first message's request: {first:?}");
    }
    // The screen: its viewer page, noVNC, and the socket's refusal of a
    // viewer that names no id and no agent (a display starts at a viewer).
    assert!(c.exec_out(&["curl", "-sf", "http://127.0.0.1:6080/"]).contains("Take over"));
    assert!(c.exec(&["curl", "-sf", "-o", "/dev/null", "http://127.0.0.1:6080/novnc/core/rfb.js"]));
    assert!(!c.exec(&["curl", "-sf", "-o", "/dev/null", "http://127.0.0.1:6080/websockify"]));
    assert!(c.exec(&["test", "-f", "/data/hermes/profiles/juniper--k3x9/SOUL.md"]), "the agent's repo is in its profile");
    // the seam (step 2 of docs/durable-computers.md): its work is its own
    assert!(c.exec(&["test", "-d", "/data/work/juniper--k3x9/browser-profile"]), "its work directory, with its browser's profile");
    assert_eq!(c.exec_out(&["readlink", "/data/hermes/profiles/juniper--k3x9/bot-desktop/browser-profile"]).trim(), "/data/work/juniper--k3x9/browser-profile", "Hermes' browser profile is a link into its work");
    assert!(c.exec(&["grep", "-q", "Juniper", "/data/hermes/profiles/juniper--k3x9/SOUL.md"]));
    // What Hermes writes in its profile is committed back to the agent's
    // fragment, as the agent.
    assert!(c.exec(&["sh", "-c", "printf 'Paul likes tomatoes.\\nAnd basil.\\n' > /data/hermes/profiles/juniper--k3x9/memories/MEMORY.md"]));
    fake.until(30_000, "the memory committed back", |w| w.fragments["juniper--k3x9"].files.get("memories/MEMORY.md").is_some_and(|b| b.as_ref() == b"Paul likes tomatoes.\nAnd basil.\n")).await;

    // The managed skills (decision 17): paul's skills fragment's `skills/`,
    // installed read-only where every profile looks after its own, read as
    // the agent acting for paul; its own skills win on a name.
    let (managed, view) = ("/data/hermes/managed-skills", "/var/lib/fragment-run/platform-skills");
    let installed = |path: &str| c.exec(&["test", "-f", &format!("{managed}/{path}")]);
    let t_skills = Instant::now();
    while !(installed("research/model-council-finite/SKILL.md") && installed("grill-me/SKILL.md")) {
        assert!(t_skills.elapsed() < Duration::from_secs(60), "the managed skills never installed; the container said:\n{}", c.logs());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(installed("research/model-council-finite/scripts/model_council.py") && !installed("fragment.json"), "the managed set is the fragment's skills/, nothing else of it");
    assert!(c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", "true"]), "a command runs as Hermes' user");
    assert!(!c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", &format!("echo x > {managed}/research/model-council-finite/SKILL.md")]), "the managed set is read-only to the agents");
    fake.with(|w| {
        let reads: Vec<_> = w.requests.iter().filter(|r| r.0.starts_with("GET /api/f/skills--k3x9/") || r.0 == "GET /api/fragments" && r.1.contains("for=")).collect();
        assert!(!reads.is_empty() && reads.iter().all(|r| r.1.contains("for=npub1paul") && r.2.as_deref() == Some("juniper--k3x9") && !r.3), "read as the agent acting for its owner, unsigned: {reads:?}");
    });
    assert!(
        c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/config.yaml"]).contains(&format!("external_dirs: [\"{managed}\", \"{view}\"]")),
        "its profile names the managed skills and the platform skill's view"
    );
    // what Hermes itself finds for the profile: its own first, then the
    // managed set and the platform skill (the image's: its CLI's own)
    let hermes_finds = || {
        let found = c.exec_out(&[
            "/command/s6-setuidgid", "hermes", "env", "HERMES_HOME=/data/hermes/profiles/juniper--k3x9", "HOME=/data/hermes/profiles/juniper--k3x9/home",
            "/opt/hermes/.venv/bin/python", "-c", "import json, os; os.chdir('/opt/hermes'); from tools.skills_tool import _find_all_skills; print(json.dumps({s['name']: s['description'] for s in _find_all_skills(skip_disabled=True)}))",
        ]);
        serde_json::from_str::<serde_json::Value>(found.trim().lines().last().unwrap_or("{}")).unwrap_or_else(|e| panic!("Hermes' skills: {e}: {found}"))
    };
    let found = hermes_finds();
    assert!(c.exec(&[
        "/command/s6-setuidgid", "hermes", "env", "HERMES_HOME=/data/hermes/profiles/juniper--k3x9", "HOME=/data/hermes/profiles/juniper--k3x9/home",
        "/opt/hermes/.venv/bin/python", "-c", include_str!("skill_fork.py"),
    ]), "automatic memory/skill review stays on; background skill writes create complete local forks");
    fake.until(30_000, "the local skill fork and its helper committed to the agent fragment", |w| {
        let files = &w.fragments["juniper--k3x9"].files;
        files.get("skills/research/model-council-finite/SKILL.md").is_some_and(|b| String::from_utf8_lossy(b).contains("# My Revised Council"))
            && files.contains_key("skills/research/model-council-finite/scripts/model_council.py")
    }).await;
    eprintln!("hermes: {} skills for juniper's profile", found.as_object().map_or(0, |o| o.len()));
    assert!(found["model-council-finite"].as_str().is_some_and(|d| d.contains("Fragment's model tiers")), "a managed skill: {found}");
    assert!(found["garden-notes"].as_str().is_some_and(|d| d.contains("Juniper's own")), "its own skill: {found}");
    assert!(found["grill-me"].as_str().is_some_and(|d| d.contains("which wins")), "its own wins on a name: {found}");
    assert!(found["fragment"].as_str().is_some_and(|d| d.starts_with("You are an agent on a Fragment computer")), "the platform skill: {found}");
    for name in ["apps-finite", "git-finite", "brain-finite", "google-workspace-finite", "image-generation-finite", "model-council-finite", "cocod-finite", "nostr-agent-interface-cli-finite", "x-api-finite", "music-generation-finite", "trading-agent-finite", "polymarket-finite"] {
        assert!(found[name].is_string(), "the release's managed {name} is visible: {found}");
    }
    for name in ["pdf", "docx", "powerpoint", "xlsx", "systematic-debugging", "test-driven-development", "arxiv"] {
        assert!(found[name].is_string(), "Hermes' bundled {name} is visible to this agent profile: {found}");
    }
    assert!(found["systematic-debugging"].as_str().is_some_and(|d| d.contains("Juniper's own")), "agent-owned skills win over bundled names: {found}");
    assert!(c.exec(&[
        "/command/s6-setuidgid", "hermes", "env", "HERMES_HOME=/data/hermes/profiles/juniper--k3x9", "HOME=/data/hermes/profiles/juniper--k3x9/home",
        "/opt/hermes/.venv/bin/python", "-c", "import json, pathlib; from tools.skills_tool import skill_view; s = json.loads(skill_view('pdf')); assert s['success'], s; assert pathlib.Path(s['skill_dir']).resolve().is_relative_to('/var/lib/fragment-run/platform-skills'); helper = json.loads(skill_view('pdf', file_path='scripts/pdf_create.py')); assert helper['success'], helper",
    ]), "Hermes can load the native PDF and helper inside its trusted view");
    assert!(found["google-workspace"].is_null(), "local OAuth must not compete with the platform's Google connection: {found}");
    assert!(!c.exec(&["test", "-d", "/opt/hermes/skills"]), "no upstream sync source to copy into versioned profiles");
    assert!(!c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", "echo x > /var/lib/fragment-run/platform-skills/productivity/pdf/SKILL.md"]), "bundled skills are read-only");
    assert!(c.exec(&["/opt/hermes/.venv/bin/python", "-c", "import reportlab, pypdf, pdfplumber, pypdfium2, pymupdf, docx, pptx, openpyxl; from reportlab.pdfgen import canvas; c = canvas.Canvas('/tmp/document-check.pdf'); c.drawString(72, 720, 'Hermes PDF check'); c.save(); assert 'Hermes PDF check' in pypdf.PdfReader('/tmp/document-check.pdf').pages[0].extract_text()"]), "the installed PDF libraries create and read real content");
    let platform = c.exec_out(&["cat", "/opt/fragment/skills/platform/fragment/SKILL.md"]);
    assert_eq!(c.exec_out(&["cat", &format!("{view}/platform/fragment/SKILL.md")]), platform, "the view shows the image's platform skill");
    let cli = c.exec_out(&["fragment", "skill"]);
    let cli_body = cli.split_once("\n---\n").map_or("", |(_, body)| body.trim());
    assert!(platform.contains("# Your computer") && !cli_body.is_empty() && platform.contains(cli_body), "the platform skill is the image's CLI's own, after the computer's page:\n{platform}");
    assert!(!c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", "echo x > /opt/fragment/skills/platform/fragment/SKILL.md"]), "the platform skill is read-only to the agents");
    assert!(!c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", &format!("echo x > {view}/platform/fragment/SKILL.md")]), "and so is its view");

    let login = c.exec_out(&["sh", "-c", "env FRAGMENT_AS_AGENT=juniper--k3x9 FRAGMENT_FOR=npub1paul fragment login 2>&1; echo exit=$?"]);
    assert!(login.contains("needs no login") && login.contains("exit=2"), "an agent logs in to nothing: {login}");

    // a change to the managed set is followed while it runs: one skill
    // changed, a file gone, and a managed `fragment`, which shadows the
    // platform's (it leaves the view: Hermes finds neither of two of one
    // name in its external dirs)
    fake.with(|w| {
        let f = w.fragments.get_mut(&skills).unwrap();
        f.files.insert("skills/research/model-council-finite/SKILL.md".into(), bytes::Bytes::from_static(b"---\nname: model-council-finite\ndescription: Consult model tiers, again.\n---\n"));
        f.files.remove("skills/research/model-council-finite/scripts/model_council.py");
        f.files.insert("skills/fragment/SKILL.md".into(), bytes::Bytes::from_static(b"---\nname: fragment\ndescription: The managed fragment, which wins.\n---\n"));
        f.files.insert("skills/pdf/SKILL.md".into(), bytes::Bytes::from_static(b"---\nname: pdf\ndescription: The managed PDF, which wins.\n---\n"));
    });
    let t_follow = Instant::now();
    let shown = || c.exec(&["test", "-e", &format!("{view}/platform/fragment/SKILL.md")]);
    while !(c.exec_out(&["cat", &format!("{managed}/research/model-council-finite/SKILL.md")]).contains("again") && !installed("research/model-council-finite/scripts/model_council.py") && installed("fragment/SKILL.md") && !shown()) {
        assert!(t_follow.elapsed() < Duration::from_secs(60), "the managed skills never followed the change; the container said:\n{}", c.logs());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let found = hermes_finds();
    assert!(found["fragment"].as_str().is_some_and(|d| d.contains("which wins")), "a managed `fragment` shadows the platform skill: {found}");

    let pdf_override = Instant::now();
    while !hermes_finds()["pdf"].as_str().is_some_and(|d| d.contains("managed PDF")) {
        assert!(pdf_override.elapsed() < Duration::from_secs(60), "the managed PDF never won over the bundled skill");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    fake.with(|w| { w.fragments.get_mut(&skills).unwrap().files.remove("skills/pdf/SKILL.md"); });
    let pdf_restore = Instant::now();
    while !hermes_finds()["pdf"].as_str().is_some_and(|d| d.contains("create, read")) {
        assert!(pdf_restore.elapsed() < Duration::from_secs(60), "the bundled PDF never returned");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // A second message, warm.
    let second = fake.say(&chat, &person("paul"), json!({ "text": "again" }));
    let t2 = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", second["seq"].as_u64().unwrap());
    let warm = Instant::now();
    fake.until(120_000, "the second reply", |w| answered(w, &t2).is_some()).await;
    eprintln!("hermes: warm message to reply: {} ms", warm.elapsed().as_millis());

    // A tool call: its end is a step, then its answer.
    let third = fake.say(&chat, &person("paul"), json!({ "text": "please use the terminal" }));
    let t3 = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", third["seq"].as_u64().unwrap());
    fake.until(120_000, "the tool turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t3)).await;
    // what the model was given last: each message's role and the start of its text
    let model_saw = || {
        let calls = model.calls.lock().unwrap();
        calls.iter().rev().take(2).map(|c| c.body["messages"].as_array().map(|m| m.iter().rev().take(6).map(|m| format!("{}: {}", m["role"], m["content"].to_string().chars().take(160).collect::<String>())).collect::<Vec<_>>()).unwrap_or_default()).collect::<Vec<_>>()
    };
    fake.with(|w| {
        let steps: Vec<_> = w.bodies(&chat, "work", "turn.step").into_iter().filter(|s| s["turn"] == t3).collect();
        assert!(steps.iter().any(|s| s["tool"] == "terminal"), "a terminal step: {steps:?}; the reply {:?}; the model saw (newest first) {:#?}", answered(w, &t3), model_saw());
        assert!(answered(w, &t3).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("the tool ran")), "{:?}", answered(w, &t3));
    });

    // Two tool calls back to back, the second quick after the first (a real
    // model's quick call after a skill read, seen on a preview): each is a
    // step, in order (Hermes' preview of a command of two: its first, and
    // how many more). Hermes' progress sender (off now) lost the second.
    let twice = fake.say(&chat, &person("paul"), json!({ "text": "please use the terminal twice" }));
    let t_twice = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", twice["seq"].as_u64().unwrap());
    fake.until(120_000, "the two-tool turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t_twice)).await;
    fake.with(|w| {
        let steps: Vec<_> = w.bodies(&chat, "work", "turn.step").into_iter().filter(|s| s["turn"] == t_twice).collect();
        let args: Vec<&str> = steps.iter().filter(|s| s["tool"] == "terminal" && s["status"] == "completed").filter_map(|s| s["args"].as_str()).collect();
        assert_eq!(args, ["echo first-ran", "sleep 2 + 1 command"], "a step for each terminal call, in order: {steps:?}; the reply {:?}", answered(w, &t_twice));
        assert!(answered(w, &t_twice).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("the tool ran")), "{:?}", answered(w, &t_twice));
    });

    // A command Hermes flags (`rm -rf`) runs at once: no card asks the
    // owner (Paul, 2026-10-14), and no guardian is asked either.
    let fourth = fake.say(&chat, &person("paul"), json!({ "text": "do the risky thing" }));
    let t4 = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", fourth["seq"].as_u64().unwrap());
    fake.until(120_000, "the flagged command's turn to end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t4)).await;
    fake.with(|w| {
        assert!(answered(w, &t4).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("the tool ran")), "it ran: {:?}", answered(w, &t4));
        assert!(!w.bodies(&chat, "work", "turn.prompt").iter().any(|p| p["turn"] == t4), "no card");
    });
    assert!(!model.calls.lock().unwrap().iter().any(|c| c.body["messages"].to_string().contains("APPROVE, DENY, or ESCALATE")), "no guardian");

    // An agent assigned while it runs (docs/computers.md: a computer's
    // agents may change while it runs): hermes-boot writes its profile,
    // the gateway serves it, and it answers in its own chat as itself,
    // within seconds, while the other agent's slow turn runs on, whole.
    let fifth = fake.say(&chat, &person("paul"), json!({ "text": "please use the terminal slowly" }));
    let t5 = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", fifth["seq"].as_u64().unwrap());
    fake.until(60_000, "juniper's slow turn running", |w| w.keepalive_open == 1 && c.exec(&["pgrep", "-f", "^sleep 8$"])).await;
    let calls_before = model.calls.lock().unwrap().len();
    let assigned = Instant::now();
    fake.add_agent("maple");
    fake.with(|w| {
        w.fragments.get_mut("maple--k3x9").unwrap().files.insert("SOUL.md".into(), bytes::Bytes::from_static(b"You are Maple, who tends the trees.\n"));
    });
    let grove = fake.chat("grove", &["maple"]);
    let hello = fake.say(&grove, &person("paul"), json!({ "text": "hello maple" }));
    let tm = fragment_bridge::records::turn_id("maple--k3x9", &grove, "chat", hello["seq"].as_u64().unwrap());
    let maple_replied = |w: &support::fake::World| w.bodies(&grove, "chat", "reply").into_iter().find(|r| r["turn"] == tm);
    if tokio::time::timeout(Duration::from_secs(60), fake.until(60_000, "maple's reply", |w| maple_replied(w).is_some())).await.is_err() {
        panic!("maple never answered; the container said:\n{}", c.logs().lines().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    eprintln!("hermes: an agent assigned while it runs to its first reply: {} ms", assigned.elapsed().as_millis());
    fake.until(60_000, "juniper's slow turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t5)).await;
    fake.with(|w| {
        let reply = maple_replied(w).unwrap();
        assert!(reply["text"].as_str().unwrap_or("").contains("hello maple"), "the scripted model's answer, as Maple's profile: {reply}");
        assert_eq!(w.records(&grove, "chat").into_iter().find(|r| r["body"]["turn"] == tm).unwrap()["principal"], "npub1maple");
        assert_eq!(w.bodies(&chat, "work", "turn.end").into_iter().find(|e| e["turn"] == t5).unwrap()["outcome"], "idle", "juniper's turn ran on through the change");
        assert!(answered(w, &t5).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("the tool ran")), "{:?}", answered(w, &t5));
    });
    {
        let calls: Vec<_> = model.calls.lock().unwrap()[calls_before..].iter().filter(|c| c.path.ends_with("/chat/completions")).cloned().collect();
        assert!(calls.iter().any(|c| c.agent.as_deref() == Some("maple--k3x9")), "Maple's calls name Maple: {calls:?}");
        assert!(calls.iter().all(|c| c.agent.is_some()), "no call without its agent (the 401 of a profile that does not exist): {calls:?}");
    }
    assert!(c.exec(&["grep", "-q", "Maple", "/data/hermes/profiles/maple--k3x9/SOUL.md"]), "its repo in its own profile");
    assert!(c.exec(&["grep", "-q", "maple--k3x9", "/var/lib/fragment-run/agents.json"]), "the bridge's ready file names it");

    // Its terminal runs the fragment CLI as itself, acting for paul: the
    // computer's API (here the fake) sees the agent named, no signature,
    // and `for` its owner; the answer is paul's fragments.
    let requests_before = fake.with(|w| w.requests.len());
    let listed = fake.say(&chat, &person("paul"), json!({ "text": "run: fragment list --json" }));
    let tl = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", listed["seq"].as_u64().unwrap());
    fake.until(120_000, "the fragment CLI's answer", |w| answered(w, &tl).is_some()).await;
    fake.with(|w| {
        let reply = answered(w, &tl).unwrap();
        let text = reply["text"].as_str().unwrap_or("");
        assert!(text.contains("the tool said") && text.contains("skills--k3x9") && text.contains("talk--k3x9"), "paul's fragments, from the CLI: {reply}");
        let cli: Vec<_> = w.requests[requests_before..].iter().filter(|r| r.0 == "GET /api/fragments").collect();
        assert!(cli.iter().any(|r| r.2.as_deref() == Some("juniper--k3x9") && r.1 == "for=npub1paul" && !r.3), "as juniper, for paul, unsigned: {cli:?}");
    });

    // Root for the session (docs/computers.md, "Root in our Hermes image"):
    // its terminal installs, offline, a package it builds through apt and a
    // program into /usr/local/bin, both with passwordless sudo, and runs
    // them.
    let install = fake.say(&chat, &person("paul"), json!({ "text": format!("run: {INSTALL}") }));
    let ti = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", install["seq"].as_u64().unwrap());
    fake.until(120_000, "the install's answer", |w| answered(w, &ti).is_some()).await;
    fake.with(|w| {
        let reply = answered(w, &ti).unwrap();
        assert!(reply["text"].as_str().unwrap_or("").contains("hello-from-apt hello-from-usr-local"), "installed as root, and run: {reply}");
    });
    assert!(c.exec(&["dpkg", "-s", "fragment-hello"]), "installed as a package, through apt");
    assert!(!c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", "echo x > /usr/local/bin/fragment-hi"]), "the system's directories stay root's: an install goes through sudo");
    // and `sudo npm install -g` installs where its terminal runs programs
    // from, not into Hermes' tool store (npm's own prefix since Hermes' PM)
    let npm = fake.say(&chat, &person("paul"), json!({ "text": "run: echo npm-global=$(sudo npm prefix -g)" }));
    let tn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", npm["seq"].as_u64().unwrap());
    fake.until(120_000, "npm's answer", |w| answered(w, &tn).is_some()).await;
    fake.with(|w| {
        let reply = answered(w, &tn).unwrap();
        assert!(reply["text"].as_str().unwrap_or("").contains("npm-global=/usr/local\""), "npm's global prefix, as root: {reply}");
    });
    // Hermes' file tools (write_file, patch) may write where its terminal
    // works, its home and /tmp, and nowhere else (HERMES_WRITE_SAFE_ROOT),
    // as Hermes' own check decides
    let denied = |path: &str| {
        let check = format!("from agent.file_safety import is_write_denied; import sys; sys.exit(1 if is_write_denied({path:?}) else 0)");
        !c.exec(&["/command/s6-setuidgid", "hermes", "/opt/hermes/.venv/bin/python", "-c", &check])
    };
    assert!(!denied("/data/work/juniper--k3x9/notes.txt"), "its file tools write its work directory");
    assert!(!denied("/data/hermes/profiles/juniper--k3x9/notes.txt") && !denied("/tmp/notes.txt"), "and its home and /tmp");
    assert!(denied("/usr/local/bin/notes"), "and not the system's directories");

    let (took, code) = c.sigterm();
    eprintln!("hermes: SIGTERM to exit: {} ms (code {code})", took.as_millis());
    if took >= Duration::from_secs(5) {
        panic!("SIGTERM took {took:?}; the container said:\n{}", c.logs());
    }
}

// ---- saves under the hold (docs/computers.md, "The hold";
// docs/durable-computers.md) ----

/// The platform's hold of `/data`, as the Computer DO makes it, in a
/// running container: its answer (`held`, within 20 s) and what it names
/// as left out.
fn hold(c: &Container) -> Option<Vec<String>> {
    assert!(c.exec(&["sh", "-c", "rm -f /run/computer/held && mkdir -p /run/computer && touch /run/computer/hold"]), "the hold");
    let t = Instant::now();
    // bounded by the DO's 20 s
    while t.elapsed() < Duration::from_secs(20) {
        if c.exec(&["test", "-e", "/run/computer/held"]) {
            return Some(c.exec_out(&["cat", "/run/computer/held"]).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn unhold(c: &Container) {
    assert!(c.exec(&["rm", "-f", "/run/computer/hold", "/run/computer/held"]), "the hold let go");
}

/// One save, as the Computer DO takes it: `/data` read out whole as a tar
/// (the DirectoryBackup's archive, on this rung), leaving out what `left_out`
/// names (anchored patterns, `/` being `/data`, as gitignore reads them).
/// Its bytes.
fn save_data(c: &Container, left_out: &[String], to: &std::path::Path) -> u64 {
    let mut args = vec!["exec".to_string(), c.id.clone(), "tar".into(), "-C".into(), "/data".into(), "-cf".into(), "-".into(), "--anchored".into()];
    for p in left_out {
        assert!(p.starts_with('/'), "our image names exactly what it copied: {p}");
        args.push(format!("--exclude=.{p}"));
    }
    args.push(".".into());
    let out = Command::new(docker()).args(&args).output().expect("docker runs");
    // 1: a file changed as it was read, which a hot save is
    assert!(out.status.success() || out.status.code() == Some(1), "tar: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::write(to, &out.stdout).expect("the save written");
    out.stdout.len() as u64
}

/// A save restored into a fresh container of `tag`, as a wake from the
/// image and the save does, before its gate: the image's check (its exit
/// code and what it said), then every SQLite file under `/data` with its
/// `quick_check`, as the hermes user.
fn restore_and_check(tag: &str, save: &std::path::Path) -> (i32, String, Vec<serde_json::Value>) {
    let c = Container::idle(tag);
    assert!(c.exec(&["mkdir", "/data"]), "the restore's target");
    let tar = std::fs::File::open(save).expect("the save");
    let cp = Command::new(docker()).args(["cp", "-a", "-", &format!("{}:/data", c.id)]).stdin(tar).output().expect("docker runs");
    assert!(cp.status.success(), "the restore: {}", String::from_utf8_lossy(&cp.stderr));
    let (code, said) = c.exec_code(&["/usr/local/bin/computer-check"]);
    let report = c.exec_out(&["/command/s6-setuidgid", "hermes", "/opt/fragment/bin/hermes-boot", "sqlite-report", "/data"]);
    let files = report.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    (code, said, files)
}

/// The Hermes image, running against the fake API and the scripted model,
/// its agent answering in its chat.
async fn hermes_running() -> (Fake, Model, String, Container) {
    hermes_running_with_modes(None).await
}

/// Real Hermes, its actual plugin loader/middleware/resize helper, and
/// attachments plus desktop captures through the bridge. Capability
/// revocation restores the separate describer without a restart.
#[tokio::test]
#[ignore = "needs Docker"]
async fn native_images_are_bounded_and_unknown_models_keep_the_describer() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);
    let fallback_chat = fake.chat("text-only", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[]);
    within(&fake,&chat,&c,120_000,"the bridge follows both image chats",|w|w.live_sockets()>=3).await;
    asked(&fake,&chat,&c,"hello",|w,t|w.bodies(&chat,"work","turn.end").iter().any(|r|r["turn"]==t)).await;
    let config = || c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/config.yaml"]);
    fake.with(|w| w.computer["models"]["images"] = json!([CHEAP_ID, FALLBACK_ID]));
    let t = Instant::now();
    while !config().contains("supports_vision: true") {
        assert!(t.elapsed() < Duration::from_secs(40), "capabilities reached the profile: {}", config());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let checked = hermes_python(&c, &["HERMES_HOME=/data/hermes/profiles/juniper--k3x9"], r#"
import base64, copy, io, json
from pathlib import Path
from PIL import Image
from hermes_cli.plugins import discover_plugins, has_middleware
discover_plugins()
assert has_middleware('llm_request')
from agent import image_routing
from tools.vision_tools import _native_tool_result_images
from hermes_cli.middleware import apply_llm_request_middleware
cfg = {'auxiliary': {'vision': {'provider': 'custom', 'model': 'vision'}},
       'providers': {'custom': {'models': {'sees': {'supports_vision': True}, 'blind': {'supports_vision': False}}}}}
assert image_routing.decide_image_input_mode('custom', 'sees', cfg) == 'native'
assert _native_tool_result_images('custom', 'sees', cfg)
assert image_routing.decide_image_input_mode('custom', 'blind', cfg) == 'text'
assert not _native_tool_result_images('custom', 'blind', cfg)
assert cfg['auxiliary']['vision']['model'] == 'vision', 'backend remains configured'
from agent.models_dev import _models_dev_id
assert _models_dev_id('custom', config={'providers': {'custom': {'catalog_provider': 'openrouter'}}}) == 'openrouter'
im = Image.effect_noise((2400, 1800), 100).convert('RGB')
buf = io.BytesIO()
im.save(buf, format='PNG')
original = buf.getvalue()
url = 'data:image/png;base64,' + base64.b64encode(original).decode()
request = {'messages': [{'role': 'user' if i % 2 else 'tool', 'content': [{'type': 'image_url', 'image_url': {'url': url}}]} for i in range(7)]}
before = copy.deepcopy(request)
result = apply_llm_request_middleware(request).payload
assert request == before, 'canonical input is untouched'
parts = [p for m in result['messages'] for p in m['content']]
images = [p for p in parts if p.get('type') == 'image_url']
assert len(images) == 4
assert all(len(p['image_url']['url']) <= 256*1024 for p in images)
for part in images:
    resized = Image.open(io.BytesIO(base64.b64decode(part['image_url']['url'].split(',',1)[1])))
    assert max(resized.size) <= 1568
    factor = 2400 / resized.width
    assert any(f'multiply any coordinates you report by {factor:.2f}' in p.get('text','') for p in parts), 'resized pixels retain their coordinate mapping'
assert all(m['content'][0]['type'] == 'text' for m in result['messages'][:3]), 'oldest images retire first'
assert apply_llm_request_middleware(request).payload == result, 'retry starts from the same history'
class NoCopy:
    def __deepcopy__(self, memo):
        raise TypeError('SDK shallow-copy fallback')
request['test_handle'] = NoCopy()
shallow = apply_llm_request_middleware(request).payload
assert request['messages'] == before['messages'], 'shallow-copy fallback also preserves canonical history'
assert sum(p.get('type') == 'image_url' for m in shallow['messages'] for p in m['content']) == 4
request.pop('test_handle')
transparent = Image.new('RGBA', (2400,1800), (0,0,0,0))
transparent.paste((0,0,0,255), (100,100,1400,900))
buf = io.BytesIO()
transparent.save(buf, format='PNG')
alpha_url = 'data:image/png;base64,' + base64.b64encode(buf.getvalue()).decode()
alpha_request = {'messages':[{'role':'user','content':[{'type':'image_url','image_url':{'url':alpha_url}}]}]}
alpha_parts = apply_llm_request_middleware(alpha_request).payload['messages'][0]['content']
alpha_image = next(p for p in alpha_parts if p.get('type') == 'image_url')['image_url']['url']
alpha_pixels = Image.open(io.BytesIO(base64.b64decode(alpha_image.split(',',1)[1])))
assert alpha_pixels.mode == 'RGBA' and alpha_pixels.getchannel('A').getextrema() == (0,255), 'transparent PNG pixels remain distinguishable'
bad = {'messages': [{'role':'user','content':[{'type':'image_url','image_url':{'url':'data:image/png;base64,YmFk'}}]}]}
assert apply_llm_request_middleware(bad).payload['messages'][0]['content'][0]['type'] == 'text'
print(json.dumps({'images':len(images),'largest':max(len(p['image_url']['url']) for p in images),'original':len(original)}))
"#);
    assert_eq!(checked["images"], 4);
    assert!(checked["largest"].as_u64().unwrap() <= 256 * 1024);
    let before = model.calls.lock().unwrap().len();
    let bytes = bytes::Bytes::from_static(support::model::IMAGE);
    let sha = fragment_bridge::records::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
    fake.with(|w| w.fragments.get_mut(&chat).unwrap().blobs.insert(sha.clone(), ("image/png".into(), bytes.clone())));
    let attachments: Vec<_> = (0..6).map(|i| json!({"name":format!("picture{i}.png"),"type":"image/png","size":bytes.len(),"sha256":sha})).collect();
    let said = fake.say(&chat, &person("paul"), json!({"text":"inspect these attachments","attachments":attachments}));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9",&chat,"chat",said["seq"].as_u64().unwrap());
    within(&fake,&chat,&c,90_000,"the native attachment reply",|w|w.bodies(&chat,"work","turn.end").iter().any(|r|r["turn"]==turn)).await;
    asked(&fake,&chat,&c,"look at your screen",|w,t|w.bodies(&chat,"work","turn.end").iter().any(|r|r["turn"]==t)).await;
    let calls = model.calls.lock().unwrap()[before..].to_vec();
    let images = |body: &serde_json::Value, role: &str| body["messages"].as_array().into_iter().flatten().filter(|m|m["role"]==role).flat_map(|m|m["content"].as_array().into_iter().flatten()).filter(|p|p["type"]=="image_url").count();
    assert!(!calls.iter().any(|c|c.model=="vision"), "native models never ask the describer");
    assert!(calls.iter().any(|c|images(&c.body,"user")>0), "the actual main model sees the attachments");
    assert!(calls.iter().any(|c|images(&c.body,"tool")>0), "the actual main model sees its screenshot");
    assert!(calls.iter().all(|c|images(&c.body,"user")+images(&c.body,"tool")<=4));
    fake.with(|w| w.computer["models"]["images"] = json!([]));
    let t = Instant::now();
    while !config().contains("supports_vision: false") {
        assert!(t.elapsed()<Duration::from_secs(40), "native capability revoked");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let before = model.calls.lock().unwrap().len();
    // A fresh chat forces a fresh capture: Hermes deliberately deduplicates
    // an unchanged screen within a session, including its auxiliary call.
    asked(&fake,&fallback_chat,&c,"look at your screen",|w,t|w.bodies(&fallback_chat,"work","turn.end").iter().any(|r|r["turn"]==t)).await;
    let after = model.calls.lock().unwrap()[before..].to_vec();
    let diagnostic: Vec<_> = after.iter().map(|c|json!({"model":c.model,"userImages":images(&c.body,"user"),"toolImages":images(&c.body,"tool"),"recent":c.body["messages"].as_array().map(|m|m.iter().rev().take(3).map(|m|content_text(m).chars().take(1800).collect::<String>()).collect::<Vec<_>>())})).collect();
    assert!(after.iter().any(|c|c.model=="vision"), "a text-only model uses its configured describer: {diagnostic:?}; config: {}; log: {}", config(), c.logs().lines().rev().take(25).collect::<Vec<_>>().join("\n"));
    assert_eq!(c.logs().matches("\"boot.init\"").count(),1);
}

async fn hermes_running_with_modes(tier: Option<&str>) -> (Fake, Model, String, Container) {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    if let Some(tier) = tier {
        fake.with(|w| {
            let models = &mut w.computer["models"];
            models["images"] = json!([CHEAP_ID, ROUTE_FALLBACK_ID, FALLBACK_ID, "deepseek/deepseek-v4.1-flash"]);
            models["routes"]["cheap"]["private"] = json!(true);
            for (tier, deep, fallback) in [("cheap", ROUTE_FALLBACK_ID, CHEAP_ID), ("medium", "deepseek/deepseek-v4.1-flash", FALLBACK_ID)] {
                models["routes"][tier]["reasoning"] = json!("high");
                models["routes"][tier]["deep"] = json!({ "id": deep, "reasoning": "max", "fallback": fallback });
            }
            w.fragments.get_mut("juniper--k3x9").unwrap().files.insert("agent.json".into(), bytes::Bytes::from(json!({ "tier": tier, "reasoning": "low" }).to_string()));
        });
    }
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[]);
    fake.until(120_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let first = fake.say(&chat, &person("paul"), json!({ "text": "hello" }));
    let t = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", first["seq"].as_u64().unwrap());
    fake.until(180_000, "Hermes' first reply", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t)).await;
    (fake, model, chat, c)
}

/// Decision 65's image gate: a real Hermes background delegate finishes
/// after its parent's reply, its result comes back through the bridge
/// once, and the computer remains held while the child is still working.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_background_delegate_delivers_and_keeps_the_computer_awake() {
    background_delegate_gate(false, None, false).await;
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_background_delegate_delivers_a_file() {
    let (fake, _model, chat, c) = hermes_running().await;
    asked(&fake, &chat, &c, "delegate the gate work", |w, t| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t)).await;
    within(&fake, &chat, &c, 90_000, "the checked background result", |w| w.bodies(&chat, "chat", "reply").iter().any(|r| r["text"].as_str().is_some_and(|t| t.contains("checked the delegated result")))).await;
    within(&fake, &chat, &c, 10_000, "the background result's explicit MEDIA file", |w| w.bodies(&chat, "chat", "reply").iter().any(|r| r["attachments"].as_array().is_some_and(|a| a.iter().any(|a| a["name"] == "delegate.txt")))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    fake.with(|w| {
        let files: Vec<_> = w.bodies(&chat, "chat", "reply").into_iter().flat_map(|r| r["attachments"].as_array().cloned().unwrap_or_default()).filter(|a| a["name"] == "delegate.txt").collect();
        assert_eq!(files.len(), 1);
        let sha = files[0]["sha256"].as_str().unwrap();
        assert_eq!(w.fragments[&chat].blobs[sha].1.as_ref(), b"fragment-delegate-complete\n");
    });
}

/// The child is still working while its next model call waits through a
/// deployment longer than Fast's two stale windows (#674 + #654).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_background_delegate_waits_out_deploying_and_keeps_the_computer_awake() {
    background_delegate_gate(true, None, false).await;
}

/// The same gate on both platform tiers: Fast handles chat, Deep the child;
/// the latter keeps its tier identity, Max effort and larger output cap.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn private_fast_delegates_to_private_deep() {
    background_delegate_gate(false, Some("cheap"), false).await;
}

#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn standard_fast_delegates_to_standard_deep() {
    background_delegate_gate(false, Some("medium"), false).await;
}

#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn private_deep_falls_back_to_private_fast() {
    background_delegate_gate(false, Some("cheap"), true).await;
}

#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn standard_deep_falls_back_to_cloudflare() {
    background_delegate_gate(false, Some("medium"), true).await;
}

async fn background_delegate_gate(deploying: bool, tier: Option<&str>, fallback: bool) {
    let (fake, model, chat, c) = hermes_running_with_modes(tier).await;
    if fallback {
        model.fail_model(if tier == Some("cheap") { ROUTE_FALLBACK_ID } else { "deepseek/deepseek-v4.1-flash" });
    }
    let parent = asked(&fake, &chat, &c, "delegate the gate work", |w, t| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t)).await;
    let started = "/data/work/juniper--k3x9/delegate-started";
    let finished = "/data/work/juniper--k3x9/delegate-finished";
    let mut active = false;
    let mut held = true;
    let mut delivered = vec![];
    let mut deployment = None;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(120) {
        if c.exec(&["test", "-f", started]) && !c.exec(&["test", "-f", finished]) {
            active = true;
            held &= fake.with(|w| w.keepalive_open > 0);
            if deploying && deployment.is_none() {
                let calls = model.calls.lock().unwrap().len();
                model.deploying_for(Duration::from_secs(90));
                deployment = Some((Instant::now(), calls));
            }
        }
        // The terminal tool finishes first; the child then waits for its
        // model's signing preflight. Leave a margin before admission so
        // its eventual stop hook can legitimately release the hold.
        if deployment.as_ref().is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(85)) {
            held &= fake.with(|w| w.keepalive_open > 0);
        }
        delivered = fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["text"].as_str().is_some_and(|s| s.contains("checked the delegated result: fragment-delegate-complete"))).collect::<Vec<_>>());
        if !delivered.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    eprintln!("delegation gate: active after parent ended={active}, held={held}, replies={delivered:?}; {}", told(&fake, &chat, &c));
    assert!(active, "the delegate runs after the parent's turn ends");
    // its child's calls are no steps of its parent's turn, nor of any: they
    // ran after it, in a run of their own
    let tools: Vec<String> = fake.with(|w| w.bodies(&chat, "work", "turn.step").iter().filter(|s| s["turn"] == parent).map(|s| s["tool"].as_str().unwrap_or("").to_string()).collect());
    assert_eq!(tools, ["delegate_task"], "the parent's own call alone");
    assert!(fake.with(|w| !w.bodies(&chat, "work", "turn.step").iter().any(|s| s["args"].as_str().is_some_and(|a| a.contains("delegate-started")))), "no child's call is a step");
    assert_eq!(delivered.len(), 1, "the background result arrives in the chat once");
    assert_ne!(delivered[0]["turn"], parent, "its completion has a turn of its own");
    assert!(held, "the computer stays awake throughout the background delegate");
    if let Some(tier) = tier {
        let deep = if tier == "cheap" { ROUTE_FALLBACK_ID } else { "deepseek/deepseek-v4.1-flash" };
        let backup = if tier == "cheap" { CHEAP_ID } else { FALLBACK_ID };
        let calls = model.calls.lock().unwrap().clone();
        let child: Vec<_> = calls.iter().filter(|c| support::model::delegate_goal(&c.body)).collect();
        assert!(!child.is_empty());
        assert!(child.iter().all(|c| (c.model == deep || (fallback && c.model == backup)) && c.tier.as_deref() == Some(tier) && c.agent.as_deref() == Some("juniper--k3x9") && c.body["reasoning_effort"] == (if c.model == deep { "max" } else { "high" }) && c.body["max_tokens"] == 32768), "Deep calls retain their route and limits (Fast fallback restores its High effort): {child:?}");
        assert_eq!(child.iter().any(|c| c.model == backup), fallback, "only its own chain is used: {child:?}");
        assert!(!c.logs().contains("Stream stale for"), "Deep waits past Fast's stale window");
        let main: Vec<_> = calls.iter().filter(|c| c.body.get("tools").is_some() && !support::model::delegate_goal(&c.body)).collect();
        assert!(!main.is_empty() && main.iter().all(|c| c.model == CHEAP_ID && c.tier.as_deref() == Some(tier) && c.body["reasoning_effort"] == "high"), "Fast answers chats at High despite the old Low pick: {main:?}");
        let settings = hermes_python(&c, &[], "import json\nfrom pathlib import Path\nimport hermes_yaml as yaml\ncfg=yaml.safe_load(Path('/data/hermes/profiles/juniper--k3x9/config.yaml').read_text())\nprint(json.dumps({'delegation':cfg['delegation'],'models':cfg['providers']['custom']['models']}))");
        assert_eq!(settings["models"][CHEAP_ID]["stale_timeout_seconds"], 20);
        assert_eq!(settings["models"][deep]["stale_timeout_seconds"], 180);
        assert_eq!(settings["delegation"]["max_concurrent_children"], 3);
        assert_eq!(settings["delegation"]["fallback_providers"][0]["model"], backup);
    }
    if deploying {
        let (at, before) = deployment.expect("the child ran through the deployment window");
        assert!(at.elapsed() >= Duration::from_secs(90));
        assert!(model.refused.load(std::sync::atomic::Ordering::SeqCst) > 2);
        let calls = model.calls.lock().unwrap().clone();
        let dispatched: Vec<_> = calls[before..].iter().filter(|c| support::model::delegate_goal(&c.body)).collect();
        assert_eq!(dispatched.len(), 1, "one admitted child model call: {dispatched:?}");
        assert_eq!(dispatched[0].model, CHEAP_ID, "the deployment caused no fallback");
        assert!(!c.logs().contains("Stream stale for"), "marked preflight kept its own patience budget");
        assert!(!fake.with(|w| w.records(&chat, "chat").iter().any(|r| r.to_string().contains("Provider temporarily unavailable"))));
    }
    within(&fake, &chat, &c, 10_000, "the child's hold to end", |w| w.keepalive_open == 0).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let replies = fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["text"].as_str().is_some_and(|s| s.contains("checked the delegated result: fragment-delegate-complete"))).count());
    assert_eq!(replies, 1, "no duplicate arrives after the completion settles");
}

/// A deployment is platform preflight, not provider silence: real Hermes
/// waits beyond Fast's two stale windows without falling back or posting a
/// provider error. The fake refuses before counting a provider dispatch.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn hermes_waits_out_model_preflight_deploying() {
    use std::sync::atomic::Ordering;
    let (fake, model, chat, c) = hermes_running().await;
    let before = model.calls.lock().unwrap().len();
    model.deploying_for(Duration::from_secs(90));
    let started = Instant::now();
    let said = fake.say(&chat, &person("paul"), json!({ "text": "hello after the deployment" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    fake.until(180_000, "one reply after a 90s deployment", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn)).await;
    let calls = model.calls.lock().unwrap().clone();
    let dispatched: Vec<_> = calls[before..].iter().filter(|c| c.body["messages"].to_string().contains("hello after the deployment")).collect();
    assert!(started.elapsed() >= Duration::from_secs(90), "the deploy window really elapsed");
    assert!(model.refused.load(Ordering::SeqCst) > 2, "multiple marked preflight refusals");
    assert_eq!(dispatched.len(), 1, "one provider dispatch, no stale retry/fallback: {dispatched:?}");
    assert_eq!(dispatched[0].model, CHEAP_ID, "same model throughout");
    fake.with(|w| {
        let replies: Vec<_> = w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == turn).collect();
        assert_eq!(replies.len(), 1, "one reply: {replies:?}");
        assert!(replies[0]["text"].as_str().unwrap_or("").contains("hello after the deployment"), "{replies:?}");
        assert!(!w.records(&chat, "chat").iter().any(|r| r.to_string().contains("Provider temporarily unavailable")), "no provider recovery message");
    });
    assert!(!c.logs().contains("Stream stale for"), "the deployment never tripped the provider watchdog");

    // Stop while the preflight is refusing. After the gate opens, the
    // abandoned turn must never dispatch a provider request.
    let refused_before = model.refused.load(Ordering::SeqCst);
    model.deploying_for(Duration::from_secs(6));
    let said = fake.say(&chat, &person("paul"), json!({ "text": "cancel this deployment wait" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    fake.until(20_000, "the cancelled turn's preflight refusal", |_| model.refused.load(Ordering::SeqCst) > refused_before).await;
    fake.say(&chat, &person("paul"), json!({ "kind": "stop", "turn": turn }));
    fake.until(30_000, "the cancelled turn ends", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn)).await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert!(!model.calls.lock().unwrap()[before..].iter().any(|c| c.body["messages"].to_string().contains("cancel this deployment wait")), "no provider dispatch after cancellation");
}

#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn hermes_deployment_adapter_contract() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let c = Container::idle(&hermes_tag());
    let (code, report) = c.exec_code(&["/opt/hermes/.venv/bin/python", "-c", include_str!("deploy_wait.py")]);
    assert_eq!(code, 0, "{report}");
}

/// Goal (Paul, 2026-10-09: a time-based fallback "only if time-based
/// fallback is built into hermes"): a tier agent whose model stalls, its
/// call streaming nothing, switches to the route's `fallback` by Hermes'
/// own mechanism, as the image configures it (hermes-boot's
/// `STALE_TIMEOUT_S`, `STREAM_RETRIES`, `api_max_retries` and
/// `fallback_providers`): its stale detector gives up on the call after
/// 20 s, tries it once more, then switches; the fallback's call names the
/// agent; and its next turn is on its tier again. Method: the scripted
/// model holds every tier call about `stall` for `STALL_S` (past the stale
/// timeout) and answers `fallback`'s at once.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_stalled_model_falls_back_to_the_routes_fallback() {
    let (fake, model, chat, c) = hermes_running().await;
    let calls = || model.calls.lock().unwrap().clone();
    let mentions = |call: &support::model::Call, words: &str| call.body["messages"].as_array().and_then(|m| m.iter().rev().find(|m| m["role"] == "user")).is_some_and(|m| m["content"].to_string().contains(words));
    let answered = |w: &support::fake::World, turn: &str| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn);
    let before = calls().len();
    let asked = Instant::now();
    let said = fake.say(&chat, &person("paul"), json!({ "text": "stall, then answer" }));
    let t = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    let replied = tokio::time::timeout(Duration::from_secs(240), fake.until(600_000, "the stalled turn's reply", |w| answered(w, &t).is_some()));
    if replied.await.is_err() {
        let tried: Vec<String> = calls()[before..].iter().map(|c| format!("{} stream={}", c.model, c.stream)).collect();
        panic!("no reply to a stalled turn; the model was asked {tried:?}; the container said:\n{}", c.logs().lines().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    let took = asked.elapsed();
    let made: Vec<support::model::Call> = calls()[before..].iter().filter(|c| c.path.ends_with("/chat/completions") && mentions(c, "stall, then answer")).cloned().collect();
    // its route's fallback (Paul, 2026-10-24: a route falls back within itself), by its id
    let fell = made.iter().position(|c| c.model == ROUTE_FALLBACK_ID);
    eprintln!("hermes: a stalled turn's reply in {} ms; its calls: {:?}", took.as_millis(), made.iter().map(|c| format!("{} stream={}", c.model, c.stream)).collect::<Vec<_>>());
    let fell = fell.unwrap_or_else(|| panic!("the turn never asked the route's fallback: {:?}", made.iter().map(|c| &c.model).collect::<Vec<_>>()));
    let stalled = made[..fell].iter().filter(|c| c.model == CHEAP_ID).count();
    assert!(stalled >= 2, "its tier was asked, and asked once more, before the fallback: {stalled} tries");
    let fallback = &made[fell];
    assert!(
        fallback.agent.as_deref() == Some("juniper--k3x9") || fallback.authorization.as_deref() == Some("Bearer agent:juniper--k3x9"),
        "the fallback's call names its agent, by header or key: {:?} {:?}",
        fallback.agent,
        fallback.authorization
    );
    assert!(took >= Duration::from_secs(38) && took < Duration::from_secs(150), "about two stale timeouts of 20 s, then the fallback's answer: {took:?}");
    fake.with(|w| {
        let reply = answered(w, &t).unwrap();
        let text = reply["text"].as_str().unwrap_or("");
        assert!(text.starts_with("scripted:") && text.contains("stall, then answer"), "the fallback's answer is the reply: {reply}");
    });
    fake.until(30_000, "the stalled turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t)).await;

    // the next turn starts on its tier again
    let before = calls().len();
    let said = fake.say(&chat, &person("paul"), json!({ "text": "and now?" }));
    let t2 = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    fake.until(120_000, "the next turn's reply", |w| answered(w, &t2).is_some()).await;
    let next: Vec<String> = calls()[before..].iter().filter(|c| c.path.ends_with("/chat/completions") && mentions(c, "and now?")).map(|c| c.model.clone()).collect();
    assert_eq!(next.first().map(String::as_str), Some(CHEAP_ID), "the next turn is on its tier: {next:?}");
    assert!(!next.iter().any(|m| m == ROUTE_FALLBACK_ID), "and needs no fallback: {next:?}");
}

/// A turn whose tool writes a SQLite database of its own for about six
/// seconds (a commit every few ms), while Hermes writes its own.
const WRITING: &str = "run: python3 -c \"import sqlite3,time;c=sqlite3.connect('tool.db');c.execute('pragma journal_mode=wal');c.execute('create table if not exists t(x)');[(c.execute('insert into t values(randomblob(8000))'),c.commit(),time.sleep(0.01)) for _ in range(500)];print('wrote')\"";

/// Whether `WRITING`'s tool is running in `c` (a process whose command line
/// names its `randomblob`).
fn writing(c: &Container) -> bool {
    c.exec(&["pgrep", "-f", "randomblob"])
}

/// Whether a reported file is one of Hermes' own databases (its home's
/// `*.db`), or another SQLite file.
fn hermes_db(file: &serde_json::Value) -> bool {
    file["path"].as_str().is_some_and(|p| p.starts_with("/data/hermes/") && p.ends_with(".db"))
}

/// Goal : a save taken under the hold while turns write opens:
/// every one of Hermes' databases in it passes `quick_check`, whatever was
/// writing as it was taken. Method: saves taken many times during turns
/// whose tool writes a database of its own (and Hermes its own), half under
/// the hold (the databases copied by SQLite's online backup, their live
/// files left out) and half hot (no hold: `/data` as it is, the platform's
/// save before the hold), each restored into a fresh container, checked by
/// the image's own check as the platform runs it, then every SQLite file
/// found checked. Asserts the held saves are whole; reports the hot ones'
/// tear rate, and any other SQLite file's (a measure, not an assertion).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_save_taken_while_it_writes_opens() {
    const ROUNDS: u32 = 12;
    let (fake, _model, chat, c) = hermes_running().await;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("saves");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut saves = vec![];
    for round in 0..ROUNDS {
        let said = fake.say(&chat, &person("paul"), json!({ "text": WRITING }));
        let t = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
        // A script run from `-c`, which Hermes flags ("script execution via
        // -e/-c flag"), runs unasked: no card.
        let asked = Instant::now();
        // bounded: a minute
        while !writing(&c) {
            assert!(fake.with(|w| !w.bodies(&chat, "work", "turn.prompt").iter().any(|p| p["turn"] == t)), "round {round}: no card asks its owner");
            assert!(asked.elapsed() < Duration::from_secs(60), "round {round}: its tool never wrote; the container said:\n{}", c.logs());
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        // the tool writing for about six seconds: the save is taken in its midst
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        let held = round % 2 == 0;
        let t0 = Instant::now();
        let left_out = if held { hold(&c).unwrap_or_else(|| panic!("no answer to the hold; the container said:\n{}", c.logs())) } else { vec![] };
        let answered_ms = t0.elapsed().as_millis();
        let path = dir.join(format!("save-{round}.tar"));
        let bytes = save_data(&c, &left_out, &path);
        let saved_ms = t0.elapsed().as_millis();
        if held {
            unhold(&c);
        }
        eprintln!("save {round}: {} {bytes} bytes, answered in {answered_ms} ms, saved in {saved_ms} ms", if held { "held" } else { "hot" });
        saves.push((held, path));
        fake.until(120_000, "the writing turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t)).await;
    }
    // the seam: the tool wrote in its agent's work directory, not in Hermes' home
    assert!(c.exec(&["test", "-f", "/data/work/juniper--k3x9/tool.db"]), "the tool's database is the agent's work: {}", c.exec_out(&["sh", "-c", "find /data -name tool.db"]));
    let (mut held_torn, mut held_dbs, mut hot_torn, mut hot_dbs, mut other_torn, mut others) = (0, 0, 0, 0, 0, 0);
    for (held, path) in &saves {
        let t = Instant::now();
        let (code, said, files) = restore_and_check(&hermes_tag(), path);
        let torn: Vec<&serde_json::Value> = files.iter().filter(|f| f["ok"] != true).collect();
        eprintln!("{}: check {code} in {} ms ({}); {} SQLite files, torn: {torn:?}", path.display(), t.elapsed().as_millis(), said.trim(), files.len());
        let ours = files.iter().filter(|f| hermes_db(f)).count();
        let ours_torn = torn.iter().filter(|f| hermes_db(f)).count();
        assert!(ours > 0, "a save holds Hermes' databases: {files:?}");
        if *held {
            assert_eq!(code, 0, "the image's check of a held save passes: {said}");
            (held_dbs, held_torn) = (held_dbs + ours, held_torn + ours_torn);
        } else {
            (hot_dbs, hot_torn) = (hot_dbs + ours, hot_torn + ours_torn);
        }
        others += files.len() - ours;
        other_torn += torn.len() - ours_torn;
    }
    eprintln!("tear rate: Hermes' databases held {held_torn}/{held_dbs}, hot {hot_torn}/{hot_dbs}; other SQLite files {other_torn}/{others}");
    assert_eq!(held_torn, 0, "a held save's databases are whole");
}

/// The files under `/data`, each path with its size and time, but those
/// `leave` names (the hold's answer: anchored paths, `/` being `/data`).
fn files(c: &Container, leave: &[String]) -> BTreeMap<String, String> {
    let listed = c.exec_out(&["sh", "-c", "find /data -type f -printf '%s %T@ %p\\n'"]);
    let mut all = BTreeMap::new();
    for line in listed.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(size), Some(time), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if !leave.iter().any(|p| path.strip_prefix("/data") == Some(p.as_str())) {
            all.insert(path.to_string(), format!("{size} {time}"));
        }
    }
    all
}

/// The paths added, removed or changed from `before` to `after`.
fn changed(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    let paths: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    paths.into_iter().filter(|p| before.get(*p) != after.get(*p)).cloned().collect()
}

/// What Hermes' gateway writes on its own timers, which the hold cannot
/// quiet (its gateway is not paused: P2), and which the save keeps as it
/// reads it (docs/durable-computers.md, "What changes under the hold"): each
/// file replaced whole (a temp file beside it, fsynced, renamed over it, so
/// a save has the old or the new, never a mix), appended (a log), an empty
/// lock, or a copy only a broken config would read; and those temp files as
/// they are written. Read from Hermes v0.21.5, measured idle:
/// - its home's loop heartbeat (`state/gateway.heartbeat`, every 30 s), its
///   runtime status (`gateway_state.json`, every 60 s) and its channel
///   directory (`channel_directory.json`, every 5 minutes);
/// - in its home and each profile's, its cron ticker's stamps and lock
///   (`cron/ticker_*`, `cron/.tick.lock`, every 60 s, though its cron tool is
///   off), its logs, and its last-known-good copy of the profile's config
///   (`backups/config/config.yaml.good.<time>`): made once, in place, by the
///   first read of a `config.yaml` whose bytes its newest copy lacks (on a
///   first start, the gateway's catalog watcher's tick 30 s after it starts,
///   which reads the config with the catalog off), and read only when that
///   `config.yaml` will not parse, which the boot's never fail to do (each
///   written whole at every start).
fn kept_hot(path: &str) -> bool {
    let Some(rel) = path.strip_prefix("/data/hermes/") else { return false };
    let (home, rel) = match rel.strip_prefix("profiles/") {
        Some(p) => (false, p.split_once('/').map_or("", |(_, r)| r)),
        None => (true, rel),
    };
    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let temp = name.starts_with('.') && name.ends_with(".tmp");
    let home_only = home && (matches!(rel, "state/gateway.heartbeat" | "gateway_state.json" | "channel_directory.json") || (temp && matches!(dir, "" | "state")));
    let each_home = matches!(rel, "cron/ticker_heartbeat" | "cron/ticker_last_success" | "cron/ticker_last_error" | "cron/.tick.lock")
        || (temp && dir == "cron")
        || (dir == "logs" && name.contains(".log"))
        || (dir == "backups/config" && name.starts_with("config.yaml.good."));
    home_only || each_home
}

/// What changed under `/data` while held that `kept_hot` does not explain.
fn unexplained(changed: &[String]) -> Vec<&String> {
    changed.iter().filter(|p| !kept_hot(p)).collect()
}

/// Each of `paths` Hermes replaces whole, a state or a stamp, read now:
/// whole, it parses as JSON; a stamp is its epoch, a number, and the
/// cron ticker's heartbeat its writer's pid after it (`<epoch> <pid>`, since
/// Hermes v0.21.6).
fn whole(c: &Container, paths: &[String]) {
    let replaced = |p: &&String| p.ends_with(".json") || ["/gateway.heartbeat", "/ticker_heartbeat", "/ticker_last_success", "/ticker_last_error"].iter().any(|s| p.ends_with(s));
    for p in paths.iter().filter(replaced) {
        let (code, text) = c.exec_code(&["cat", p]);
        if code != 0 {
            continue;
        }
        let stamp = p.ends_with("/ticker_heartbeat") && text.split_once(' ').is_some_and(|(epoch, pid)| epoch.parse::<f64>().is_ok() && pid.trim().parse::<u32>().is_ok());
        assert!(stamp || serde_json::from_str::<serde_json::Value>(&text).is_ok(), "{p} is whole: {text:?}");
    }
}

/// The caches Hermes' model-catalog refresh writes into the gateway's home.
const CATALOG_CACHES: [&str; 4] = ["model_catalog.json", "openrouter_curated_catalog.json", "nous_recommended_cache.json", "reasoning_caps.json"];

/// A hold kept this long, and until Hermes' heartbeat (every 30 s) and its
/// cron ticker (every 60 s, as its status is) have both run inside it, sees
/// each of Hermes' own idle timers but its 5-minute one, and the image's own
/// repo sync (every 60 s) were the hold not to quiet it: every run sees the
/// same. Past `HELD_MAX_MS` without them, the test fails.
const HELD_MS: u64 = 65_000;
const HELD_MAX_MS: u64 = 150_000;
/// Two of Hermes' timers, each seen rewritten inside the hold.
const TIMERS: [&str; 2] = ["/data/hermes/state/gateway.heartbeat", "/data/hermes/cron/ticker_heartbeat"];

/// Goal: once the guest says `held`, no file its save keeps changes
/// but what Hermes' gateway rewrites on its own timers, each whole
/// (`kept_hot`): the bridge claims nothing, the sync, the skills and the
/// agents' reads start no round, and the copy is done before the answer.
/// Hermes' gateway is not paused (P2: it cannot be asked), so the hold comes
/// between turns; what Hermes writes on its own while idle in a database
/// (its kanban dispatcher opens its board's on a timer, making its `-wal`
/// and `-shm`) is in the files its answer names, which the save leaves out
/// (their copies kept). Its model-catalog refresh, which on 2026-10-07
/// rewrote four caches inside this test's hold (then 5 s, landing in it
/// only in a slow run), is off, and Node's compile cache is out of `/data`.
/// Method: an idle Hermes held, a listing of the files under `/data` (each
/// path, size and time) but those the answer names, then another at least
/// `HELD_MS` later, once Hermes' heartbeat and cron ticker have both run:
/// past every one of those timers, so every run sees them inside the hold,
/// whenever it began. Then a message said while it is held is not
/// claimed until the hold goes (its bridge records it, unclaimed: its state
/// file is rewritten, whole).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn held_nothing_under_data_changes() {
    let (fake, _model, chat, c) = hermes_running().await;
    // between turns: once Hermes' start-up writes are done (its lazy
    // packages; its kanban dispatcher makes its board's database some
    // seconds after the gateway starts), two windows of three seconds with
    // no file changed but a database's journal and what Hermes' timers keep
    let journals = |p: &String| [".db-wal", ".db-shm", ".db-journal"].iter().any(|s| p.ends_with(s));
    let settled = |all: BTreeMap<String, String>| all.into_iter().filter(|(p, _)| !journals(p) && !kept_hot(p)).collect::<BTreeMap<_, _>>();
    let quiet = Instant::now();
    // bounded: two minutes
    while !c.exec(&["test", "-f", "/data/hermes/kanban.db"]) {
        assert!(quiet.elapsed() < Duration::from_secs(120), "Hermes never made its kanban board's database");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let mut last = settled(files(&c, &[]));
    let mut still = 0;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let now = settled(files(&c, &[]));
        still = if now == last { still + 1 } else { 0 };
        if still >= 2 {
            break;
        }
        last = now;
    }
    eprintln!("hermes: /data quiet {} ms after its first reply", quiet.elapsed().as_millis());
    let left_out = hold(&c).unwrap_or_else(|| panic!("no answer to the hold; the container said:\n{}", c.logs()));
    assert!(left_out.iter().any(|l| l == "/hermes/profiles/juniper--k3x9/state.db") && left_out.iter().all(|l| l.starts_with("/hermes/") && l.contains(".db")), "it names exactly the databases it copied: {left_out:?}");
    assert!(c.exec(&["test", "-f", "/data/held-copies/manifest.json"]), "its copies are made before it answers");
    let before = files(&c, &left_out);
    let held = Instant::now();
    // bounded by HELD_MAX_MS
    let seen = loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let seen = changed(&before, &files(&c, &left_out));
        let fired = TIMERS.iter().all(|t| seen.iter().any(|p| p == t));
        if fired && held.elapsed() >= Duration::from_millis(HELD_MS) {
            break seen;
        }
        assert!(held.elapsed() < Duration::from_millis(HELD_MAX_MS), "Hermes' timers ran inside the hold ({TIMERS:?}): {seen:?}");
    };
    eprintln!("hermes: changed in {} ms held: {seen:?}", held.elapsed().as_millis());
    assert!(unexplained(&seen).is_empty(), "a file the save keeps changed while held, not one Hermes' timers rewrite whole: {:?} (all changed: {seen:?})", unexplained(&seen));
    whole(&c, &seen);
    // its catalogs, refreshed 30 s after the gateway started had they been
    // on, are not; Node's compile cache (its `npx --version` probes ran with
    // the first turn) is under /tmp
    for cache in CATALOG_CACHES {
        assert!(!c.exec(&["test", "-e", &format!("/data/hermes/cache/{cache}")]), "no catalog refresh wrote {cache}");
    }
    assert_eq!(c.exec_out(&["find", "/data", "-name", "node-compile-cache"]).trim(), "", "Node's compile cache is not under /data");
    assert!(!c.exec_out(&["find", "/tmp/node-compile-cache", "-type", "f"]).trim().is_empty(), "it is under /tmp, where npm's start put it");
    // a message as it is held: recorded, never claimed while held
    let said = fake.say(&chat, &person("paul"), json!({ "text": "said while held" }));
    let t = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    tokio::time::sleep(Duration::from_secs(5)).await;
    fake.with(|w| assert!(w.bodies(&chat, "work", "turn.start").iter().all(|s| s["turn"] != t), "held, the message is not claimed"));
    unhold(&c);
    fake.until(120_000, "the message answered once the hold goes", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t)).await;
    // the boot sees the hold go on its own poll (every 100 ms), apart from
    // the bridge's: waited for, bounded, not a fixed time
    let gone = Instant::now();
    while c.exec(&["test", "-e", "/data/held-copies"]) {
        assert!(gone.elapsed() < Duration::from_secs(10), "the copies go with the hold");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// What the agent's desktop looks like from inside the container, as the
/// hermes user: the pointer's position and the windows' titles, read from
/// the X server the profile publishes (`<profile>/bot-desktop/env`).
fn desk(c: &Container, profile: &str) -> serde_json::Value {
    const SCRIPT: &str = r#"import ctypes, json, os, re, subprocess, sys
env = dict(l.split("=", 1) for l in open(sys.argv[1]).read().splitlines() if "=" in l)
os.environ.update(env)
x = ctypes.cdll.LoadLibrary("libX11.so.6")
x.XOpenDisplay.restype, x.XOpenDisplay.argtypes = ctypes.c_void_p, [ctypes.c_char_p]
x.XDefaultRootWindow.restype, x.XDefaultRootWindow.argtypes = ctypes.c_ulong, [ctypes.c_void_p]
x.XQueryPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_void_p] * 7
d = x.XOpenDisplay(env["DISPLAY"].encode())
root = x.XDefaultRootWindow(d)
v = [ctypes.c_ulong(), ctypes.c_ulong(), ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_uint()]
x.XQueryPointer(d, root, *[ctypes.byref(a) for a in v])
ids = re.findall(r"0x[0-9a-f]+", subprocess.run(["xprop", "-root", "_NET_CLIENT_LIST"], capture_output=True, text=True).stdout)
names = [subprocess.run(["xprop", "-id", w, "_NET_WM_NAME"], capture_output=True, text=True).stdout.split("=", 1)[-1].strip().strip('"') for w in ids]
print(json.dumps({"pointer": [v[2].value, v[3].value], "windows": names}))
"#;
    let env = format!("/data/hermes/profiles/{profile}/bot-desktop/env");
    let out = c.exec_out(&["/command/s6-setuidgid", "hermes", "/opt/hermes/.venv/bin/python", "-c", SCRIPT, &env]);
    serde_json::from_str(out.trim().lines().last().unwrap_or("null")).unwrap_or(serde_json::Value::Null)
}

/// Waits until the pointer on `profile`'s desktop is at `at` (looked at every
/// 200 ms, for at most 15 s), as a viewer's move puts it there: through the
/// screen's socket, the bridge and Xvnc, which a loaded runner slows. How
/// long it took.
async fn pointer_at(c: &Container, profile: &str, at: [i64; 2], what: &str) -> Duration {
    let t = Instant::now();
    // bounded: 15 s
    loop {
        let seen = desk(c, profile)["pointer"].clone();
        if seen == json!(at) {
            return t.elapsed();
        }
        assert!(t.elapsed() < Duration::from_secs(15), "{what}: the pointer is at {seen}, not {at:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// How long a move that must not land is given to land before its absence
/// counts: three times what a move that did land took, and at least a
/// second, so a slower runner gives it longer.
fn settle(took: Duration) -> Duration {
    (took * 3).max(Duration::from_secs(1))
}

/// Where the window titled `title` is on the agent's desktop, `[x, y, w,
/// h]`, as its X server has it (`xwininfo`, as the hermes user).
fn window(c: &Container, profile: &str, title: &str) -> Option<[usize; 4]> {
    let env = format!("/data/hermes/profiles/{profile}/bot-desktop/env");
    let script = format!("set -a; . {env}; xwininfo -root -tree");
    let tree = c.exec_out(&["/command/s6-setuidgid", "hermes", "bash", "-c", &script]);
    // `0x200003 "Example Domain - …": ("chromium-browser" "Chromium-browser")  1004x748+10+10  +10+10`
    let line = tree.lines().find(|l| l.contains(&format!("\"{title}")))?;
    let mut fields = line.split_whitespace().rev();
    let at = fields.next()?.trim_start_matches('+');
    let size = fields.next()?;
    let (x, y) = at.split_once('+')?;
    let (w, rest) = size.split_once('x')?;
    let h = rest.split('+').next()?;
    Some([x.parse().ok()?, y.parse().ok()?, w.parse().ok()?, h.parse().ok()?])
}

/// The share of `rect`'s pixels in `frame` (`width` wide) that are
/// example.com's background (`#eee`, or `#222` in a dark scheme): the page
/// drawn where its window is, as a viewer sees it.
fn page_shown(frame: &[u32], width: usize, rect: [usize; 4]) -> f64 {
    let [x, y, w, h] = rect;
    let (mut page, mut all) = (0usize, 0usize);
    for row in y..y + h {
        for col in x..(x + w).min(width) {
            if let Some(&p) = frame.get(row * width + col) {
                all += 1;
                page += usize::from(p == 0x00ee_eeee || p == 0x0022_2222);
            }
        }
    }
    page as f64 / all.max(1) as f64
}

/// The command line of the browser's Chromium (its first process).
fn chromium(c: &Container) -> String {
    c.exec_out(&["sh", "-c", "for p in $(pgrep -f -- '/(chrome(-headless-shell)?|chromium) ' | head -1); do tr '\\0' ' ' < /proc/$p/cmdline; done"])
}

/// Inspect the real desktop browser through the adapter's ownership check.
fn desktop_browser(c: &Container, profile: &str) -> serde_json::Value {
    const SCRIPT: &str = r#"import json, sys
sys.path.insert(0, '/opt/hermes')
from tools.bot_desktop import browser
from tools.bot_desktop.fragment_browser import _owned_process, _endpoint
p = _owned_process(browser.profile_dir())
print(json.dumps({'pid': p.pid, 'port': _endpoint(p), 'argv': p.cmdline()} if p else None))
"#;
    let (code, out) = c.exec_code(&["/command/s6-setuidgid", "hermes", "env", &format!("HERMES_HOME=/data/hermes/profiles/{profile}"), "/opt/hermes/.venv/bin/python", "-c", SCRIPT]);
    assert_eq!(code, 0, "the desktop browser's identity: {out}");
    serde_json::from_str(out.trim().lines().last().unwrap_or("null")).expect("the browser identity is JSON")
}

/// Run the Browser icon through the desktop's own launcher and environment.
fn open_browser_icon(c: &Container, profile: &str) {
    const SCRIPT: &str = r#"import ctypes, os, sys, time
sys.path.insert(0, '/opt/hermes')
from tools.bot_desktop import browser, runtime
from tools.bot_desktop.fragment_browser import _owned_process, _endpoint
panel = runtime.state_dir() / 'xdg/xfce4/panel'
number = (panel / '.hermes-browser-launcher').read_text().strip()
os.environ.update(runtime.published_env())
# Xfce has GIO's launcher library, without the optional `gio` CLI.
gio = ctypes.CDLL('libgio-2.0.so.0')
gio.g_desktop_app_info_new_from_filename.argtypes = [ctypes.c_char_p]
gio.g_desktop_app_info_new_from_filename.restype = ctypes.c_void_p
gio.g_app_info_launch.argtypes = [ctypes.c_void_p] * 4
gio.g_app_info_launch.restype = ctypes.c_int
icon = gio.g_desktop_app_info_new_from_filename(str(panel / f'launcher-{number}/hermes.desktop').encode())
assert icon and gio.g_app_info_launch(icon, None, None, None), 'the desktop entry launches'
for _ in range(200):
    p = _owned_process(browser.profile_dir())
    if p is not None:
        try:
            _endpoint(p)
            break
        except (OSError, ValueError):
            pass
    time.sleep(.1)
else:
    raise AssertionError('the Browser icon did not start Chromium within 20s')
"#;
    let (code, out) = c.exec_code(&["/command/s6-setuidgid", "hermes", "env", &format!("HERMES_HOME=/data/hermes/profiles/{profile}"), "/opt/hermes/.venv/bin/python", "-c", SCRIPT]);
    assert_eq!(code, 0, "the desktop's Browser icon: {out}");
}

/// Waits until `viewer`'s frame shows the browser's page where its window
/// is (within 30 s): where it is, `[x, y, w, h]`.
async fn browser_shown(c: &Container, viewer: &mut support::rfb::Viewer, what: &str) -> [usize; 4] {
    let t = Instant::now();
    loop {
        let at = window(c, "juniper--k3x9", "Example Domain");
        let frame = viewer.frame().await.unwrap_or_else(|e| panic!("{what}: the viewer's frame: {e}"));
        let page = at.map(|r| page_shown(&frame, viewer.width as usize, r));
        if let (Some(rect), Some(p)) = (at, page) {
            if p > 0.4 {
                return rect;
            }
        }
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "{what}: the browser never showed in the viewer's frame: its window at {at:?}, the page {page:?} of it; the desktop {}; chromium {}\n{}",
            desk(c, "juniper--k3x9"),
            chromium(c),
            c.logs().lines().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// An HTTP server that answers every request `{}` and keeps each one's
/// request line and headers: what a vendor's API would be sent.
struct Recorder {
    port: u16,
    seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Recorder {
    async fn start() -> Recorder {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.expect("the recorder listens");
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = seen.clone();
        tokio::spawn(async move {
            // bounded by the test: it ends with the runtime
            while let Ok((mut s, _)) = listener.accept().await {
                let kept = kept.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut b = [0u8; 4096];
                    // bounded: a request's head is at most 64 KiB
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 65_536 {
                        match s.read(&mut b).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&b[..n]),
                        }
                    }
                    kept.lock().unwrap().push(String::from_utf8_lossy(&head).into_owned());
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}").await;
                });
            }
        });
        Recorder { port, seen }
    }
}

/// The container's memory now, in MiB: its cgroup's anonymous memory (what
/// its processes hold, without the page cache).
fn anon_mib(c: &Container) -> u64 {
    let stat = c.exec_out(&["cat", "/sys/fs/cgroup/memory.stat"]);
    stat.lines().find_map(|l| l.strip_prefix("anon ")).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0) / 1_048_576
}

/// Goal: a person can watch and take over the agent's desktop, and the
/// agent operates it (docs/computers.md, Ports; Paul, 2026-10-05): before
/// any agent's call the screen's page, its control socket and its RFB
/// stream answer, the first agent's desktop started for its first viewer;
/// a viewer who takes over moves the pointer, and one who has not cannot;
/// the agent's `computer_use` is among its tools and captures the screen;
/// its browser opens on that desktop, in the frame of the viewer that has
/// watched since before it started; and the desktop restarted under its
/// viewers, a viewer that opens the screen again (the page does, on its
/// own) sees the browser on the new one. All run as Containers runs the
/// image (`Runtime::Hosted`: no `/dev/shm`, no Docker marker; p5,
/// 2026-10-05).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn the_hermes_desktop() {
    use fragment_bridge::net::Base;
    use support::rfb::{colours, Control, Viewer};
    let tag = hermes_tag();
    build(&repo_dir(), "images/hermes/Dockerfile", &tag);
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    // its owner connected Google: the agent's placeholder, as the platform makes one
    let google = "fcx_google_0123456789abcdef0123456789abcdef";
    fake.with(|w| {
        w.computer["agents"][0]["credentials"] = json!([{ "provider": "google", "kind": "connection", "env": ["GOOGLE_OAUTH_ACCESS_TOKEN"], "placeholder": google, "hosts": ["www.googleapis.com"] }]);
        w.computer["credentialEnv"] = json!(["GOOGLE_OAUTH_ACCESS_TOKEN"]);
    });
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("desk", &["juniper"]);
    let c = Container::run_on(Runtime::Hosted, &tag, fake.addr.port(), model.addr.port(), &[]);
    assert!(!c.exec(&["test", "-e", "/dev/shm"]) && !c.exec(&["test", "-e", "/.dockerenv"]), "run as Containers runs it: no /dev/shm, no Docker marker");
    fake.until(180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let base = Base::parse(&format!("http://127.0.0.1:{}", c.port(6080))).unwrap();
    let before = anon_mib(&c);
    eprintln!("desktop: the container holds {before} MiB before any screen");

    // before any agent's call: the page, the control socket, the RFB stream
    assert!(c.exec_out(&["curl", "-sf", "http://127.0.0.1:6080/"]).contains("Take over"));
    let mut watching = Control::open(&base, "watcher", "juniper--k3x9").await.unwrap_or_else(|e| panic!("the control socket: {e}"));
    assert_eq!(watching.next().await.unwrap(), json!({ "type": "control", "agent": "juniper--k3x9", "holder": null }), "a viewer hears whose screen it is, and who holds control");
    let t = Instant::now();
    let opened = Viewer::open(&base, "watcher", "juniper--k3x9", Duration::from_secs(60)).await;
    let mut watcher = opened.unwrap_or_else(|e| panic!("the screen's RFB stream: {e}\n{}\nlauncher: {}", c.logs(), c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/bot-desktop/launcher.log"])));
    eprintln!("desktop: first viewer to the RFB greeting {} ms; {}x{} {:?}", t.elapsed().as_millis(), watcher.width, watcher.height, watcher.name);
    // the desktop drawn: its wallpaper and panel, not one colour
    let t = Instant::now();
    let mut frame = watcher.frame().await.unwrap();
    while colours(&frame) <= 16 && t.elapsed() < Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_millis(500)).await;
        frame = watcher.frame().await.unwrap();
    }
    let after = anon_mib(&c);
    eprintln!("desktop: a frame of {} colours {} ms on; the container holds {after} MiB with the desktop (+{})", colours(&frame), t.elapsed().as_millis(), after.saturating_sub(before));
    assert!(colours(&frame) > 16, "the desktop shows something: {} colours\n{}", colours(&frame), c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/bot-desktop/launcher.log"]));

    // Take over: the holder's pointer moves the screen's; a watcher's does not
    let mut driving = Control::open(&base, "driver", "juniper--k3x9").await.unwrap();
    assert_eq!(driving.next().await.unwrap()["holder"], json!(null));
    let mut driver = Viewer::open(&base, "driver", "juniper--k3x9", Duration::from_secs(30)).await.unwrap();
    assert!(driver.clipboard_caps, "Xvnc offers its extended clipboard, and the viewer answers it as noVNC does (a ClientCutText of negative length)");
    driving.say("take").await.unwrap();
    assert_eq!(driving.next().await.unwrap()["holder"], "driver");
    assert_eq!(watching.next().await.unwrap()["holder"], "driver", "every viewer hears who took over");
    driver.pointer(101, 57, 0).await.unwrap();
    let took = pointer_at(&c, "juniper--k3x9", [101, 57], "the holder moves the pointer").await;
    watcher.pointer(301, 257, 0).await.unwrap();
    tokio::time::sleep(settle(took)).await;
    assert_eq!(desk(&c, "juniper--k3x9")["pointer"], json!([101, 57]), "a watcher does not (given {:?}, the holder's move having taken {took:?})", settle(took));
    driving.say("give").await.unwrap();
    assert_eq!(driving.next().await.unwrap()["holder"], json!(null));

    // the agent's computer_use: among its tools, and it captures the screen
    let reply = |w: &support::fake::World, turn: &str| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn).and_then(|r| r["text"].as_str().map(str::to_string));
    let asked = fake.say(&chat, &person("paul"), json!({ "text": "look at your screen" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", asked["seq"].as_u64().unwrap());
    fake.until(240_000, "the capture's reply", |w| reply(w, &turn).is_some()).await;
    let offered = model.calls.lock().unwrap().iter().find(|c| c.path.ends_with("/chat/completions") && c.body["messages"].to_string().contains("look at your screen")).map(|c| c.body["tools"].clone()).unwrap_or_default();
    let tools: Vec<&str> = offered.as_array().into_iter().flatten().filter_map(|t| t["function"]["name"].as_str()).collect();
    let search = offered.as_array().into_iter().flatten().find(|t| t["function"]["name"] == "tool_search").and_then(|t| t["function"]["description"].as_str()).unwrap_or("");
    let listed = search.find("computer_use").map(|i| search[i..].chars().take(160).collect::<String>());
    eprintln!("desktop: the agent's tools: {tools:?}; computer_use in tool_search's listing: {listed:?}");
    let captured = fake.with(|w| reply(w, &turn)).unwrap_or_default();
    eprintln!("desktop: the capture's reply: {}", captured.chars().take(600).collect::<String>());
    assert!(tools.contains(&"computer_use") || listed.is_some(), "computer_use is the agent's, directly or through tool_search: {tools:?}");
    // the screenshot is described by the route's vision model (the profile's
    // `auxiliary.vision`), as the agent, its image in the call
    let looked: Vec<(String, Option<String>, bool)> = model
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.body["messages"].to_string().contains("\"image_url\""))
        .map(|c| (c.model.clone(), c.agent.clone(), c.path.ends_with("/v1/chat/completions") && c.body["messages"].to_string().contains("data:image/")))
        .collect();
    eprintln!("desktop: the calls shown an image (model, agent, a data: image on /v1/chat/completions): {looked:?}");
    assert!(
        !looked.is_empty() && looked.iter().all(|(m, a, ok)| m == "vision" && a.as_deref() == Some("juniper--k3x9") && *ok),
        "the capture's screenshot goes to the route's vision model, as the agent: {looked:?}"
    );
    assert!(tools.contains(&"browser_navigate") && !tools.contains(&"browser_exec"), "Hermes' built-in browser tools: {tools:?}");

    // its browser, on its desktop, seen by the viewer that has watched
    // since before the desktop started
    let browse = |fake: &Fake| {
        let asked = fake.say(&chat, &person("paul"), json!({ "text": "browse: https://example.com" }));
        fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", asked["seq"].as_u64().unwrap())
    };
    let turn = browse(&fake);
    fake.until(240_000, "the browser's reply", |w| reply(w, &turn).is_some()).await;
    eprintln!("desktop: the browser's reply: {:?}", fake.with(|w| reply(w, &turn)));
    let rect = browser_shown(&c, &mut watcher, "the first browse").await;
    let started = chromium(&c);
    eprintln!("desktop: the browser's window at {rect:?} in the watcher's frame; chromium {started}; the container holds {} MiB", anon_mib(&c));
    assert!(started.contains(" --no-sandbox --disable-dev-shm-usage "), "the image's Chromium, with the flags a container needs: {started}");
    // and the desktop's Browser icon, a person's after Take over, starts the same
    let panel = "/data/hermes/profiles/juniper--k3x9/bot-desktop/xdg/xfce4/panel";
    let icon = c.exec_out(&["sh", "-c", &format!("cat {panel}/launcher-$(cat {panel}/.hermes-browser-launcher)/hermes.desktop")]);
    assert!(icon.lines().any(|l| l.starts_with("Exec=\"/opt/hermes/.venv/bin/python\" \"/opt/hermes/tools/bot_desktop/fragment_browser.py\" ")), "the Browser icon starts the shared desktop Chromium: {icon}");

    // The desktop restarts under its viewers (stopped as `hermes
    // computer-use screen stop` stops it): each viewer's stream ends with
    // its display, so a viewer knows to open it again (the screen's page
    // does, on its own); the one it opens starts the display at once, and
    // shows the browser on it at the agent's next call.
    let stopped = c.exec_code(&["env", "HOME=/data/hermes", "HERMES_HOME=/data/hermes", "/command/s6-setuidgid", "hermes", "/opt/hermes/.venv/bin/hermes", "-p", "juniper--k3x9", "computer-use", "screen", "stop"]);
    eprintln!("desktop: stopped: {stopped:?}");
    let ended = tokio::time::timeout(Duration::from_secs(15), watcher.frame()).await.map(|r| r.map(|f| f.len()));
    assert!(matches!(ended, Ok(Err(_))), "the watcher's stream ends with its display: {ended:?}");
    let t = Instant::now();
    let mut again = Viewer::open(&base, "watcher", "juniper--k3x9", Duration::from_secs(30)).await.unwrap_or_else(|e| panic!("the watcher, back: {e}\n{}", c.logs()));
    eprintln!("desktop: the watcher back on a desktop started for it in {} ms", t.elapsed().as_millis());
    let turn = browse(&fake);
    fake.until(240_000, "the browser's reply after the restart", |w| reply(w, &turn).is_some()).await;
    eprintln!("desktop: the browser's reply after the restart: {:?}", fake.with(|w| reply(w, &turn)));
    let rect = browser_shown(&c, &mut again, "after the desktop's restart").await;
    eprintln!("desktop: the browser's window at {rect:?} after the restart; chromium {}", chromium(&c));

    // gws, Google's Workspace CLI: in the image, and run in the agent's
    // terminal it sends the agent's Google placeholder as its bearer token
    // (the computer's egress swaps it on Google's hosts). Its Drive API is
    // pointed at a recorder (a discovery document in its cache) to see it.
    let version = c.exec_out(&["gws", "--version"]);
    assert!(version.contains("0.22.5"), "gws is pinned: {version}");
    let api = Recorder::start().await;
    let doc = json!({
        "name": "drive", "version": "v3", "rootUrl": format!("http://api.fragment.internal:{}/", api.port), "servicePath": "drive/v3/",
        "resources": { "files": { "methods": { "list": { "id": "drive.files.list", "httpMethod": "GET", "path": "files", "scopes": ["https://www.googleapis.com/auth/drive"] } } } },
    });
    let planted = format!("mkdir -p /tmp/gws/cache && printf '%s' '{doc}' > /tmp/gws/cache/drive_v3.json");
    assert!(c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", &planted]));
    let asked = fake.say(&chat, &person("paul"), json!({ "text": "run: GOOGLE_WORKSPACE_CLI_CONFIG_DIR=/tmp/gws gws drive files list" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", asked["seq"].as_u64().unwrap());
    fake.until(240_000, "gws's reply", |w| reply(w, &turn).is_some()).await;
    let seen = api.seen.lock().unwrap().clone();
    eprintln!("desktop: gws in the agent's terminal said {:?}; its API saw {seen:?}", fake.with(|w| reply(w, &turn)));
    assert!(
        seen.iter().any(|r| r.starts_with("GET /drive/v3/files") && r.to_ascii_lowercase().contains(&format!("authorization: bearer {google}"))),
        "gws sends the agent's placeholder as its bearer token: {seen:?}; it said {:?}",
        fake.with(|w| reply(w, &turn))
    );
}

/// Goal: the first native browser tool starts regular Chromium on its own
/// screen, with webdriver off; the Browser icon and later tool connections
/// share that process and cookies. Closing a tool connection leaves it up;
/// stopping its desktop closes it and keeps its cookies. A human-first
/// restart, a browser crash and a whole computer restart reuse the same jar.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_regular_browser_is_shared_by_the_desktop_and_tools() {
    use support::rfb::Viewer;
    let (fake, _model, chat, c) = hermes_running().await;
    let profile = "juniper--k3x9";
    assert!(!desktop_up(&c, profile), "no desktop or browser starts at boot");
    let navigate = tool_said(&fake, &chat, &c, "browse: https://example.com").await;
    let first = desktop_browser(&c, profile);
    assert!(first.is_object(), "the native browse starts its desktop browser: {navigate}");
    let argv: Vec<&str> = first["argv"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(argv[0].ends_with("/chromium"), "regular Chromium, not the testing bundle: {argv:?}");
    assert!(argv.contains(&"--test-type") && argv.contains(&"--no-sandbox") && argv.contains(&"--disable-dev-shm-usage"), "the container and desktop UI flags: {argv:?}");
    assert!(first["port"].as_u64().is_some_and(|p| p > 0), "a nonzero CDP port: {first}");
    for unwanted in ["--enable-automation", "--remote-debugging-port=0", "--remote-debugging-pipe", "--headless", "--disable-blink-features"] {
        assert!(!argv.iter().any(|arg| arg.starts_with(unwanted)), "no {unwanted}: {argv:?}");
    }
    let ordinary = tool_said(&fake, &chat, &c, "inspect browser: navigator.webdriver === false ? 'ordinary-browser' : 'automation-enabled'").await;
    assert!(ordinary.contains("ordinary-browser"), "the native tool sees webdriver false: {ordinary}");
    let cookie = tool_said(&fake, &chat, &c, "inspect browser: document.cookie = 'fragment_browser_probe=kept; Max-Age=3600; SameSite=Lax'; document.cookie").await;
    assert!(cookie.contains("fragment_browser_probe=kept"), "a real persistent cookie: {cookie}");

    let base = fragment_bridge::net::Base::parse(&format!("http://127.0.0.1:{}", c.port(6080))).unwrap();
    let _viewer = Viewer::open(&base, "watcher", profile, Duration::from_secs(30)).await.unwrap();
    open_browser_icon(&c, profile);
    assert_eq!(desktop_browser(&c, profile)["pid"], first["pid"], "the icon reuses the tool's browser");
    let port = first["port"].to_string();
    assert!(c.exec(&["/command/s6-setuidgid", "hermes", "agent-browser", "--session", "close-probe", "--cdp", &port, "get", "title"]));
    assert!(c.exec(&["/command/s6-setuidgid", "hermes", "agent-browser", "--session", "close-probe", "close"]));
    assert_eq!(desktop_browser(&c, profile)["pid"], first["pid"], "tool cleanup only disconnects");

    let stopped = c.exec_code(&["env", "HERMES_HOME=/data/hermes", "/command/s6-setuidgid", "hermes", "hermes", "-p", profile, "computer-use", "screen", "stop"]);
    assert_eq!(stopped.0, 0, "desktop stop: {stopped:?}");
    assert!(desktop_browser(&c, profile).is_null(), "desktop stop closes its Chromium");
    let _back = Viewer::open(&base, "watcher", profile, Duration::from_secs(30)).await.unwrap();
    open_browser_icon(&c, profile); // Human starts this browser before any tool.
    let human_first = desktop_browser(&c, profile);
    assert_ne!(human_first["pid"], first["pid"], "a fresh browser after the stop");
    let read_cookie = "inspect browser: document.cookie.includes('fragment_browser_probe=kept') ? 'cookie-kept' : 'cookie-missing'";
    tool_said(&fake, &chat, &c, "browse: https://example.com").await;
    let cookie = tool_said(&fake, &chat, &c, read_cookie).await;
    assert!(cookie.contains("cookie-kept"), "the tools attach to the human's browser with its cookie: {cookie}");
    assert_eq!(desktop_browser(&c, profile)["pid"], human_first["pid"]);

    // The old port file is deliberately bogus. Discovery uses the live
    // profile owner and verifies CDP's PID, never this stale metadata.
    assert!(c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", &format!("printf '1\\n/devtools/browser/stale\\n' > /data/work/{profile}/browser-profile/DevToolsActivePort")]));
    assert_eq!(desktop_browser(&c, profile)["pid"], human_first["pid"]);
    assert!(c.exec(&["kill", "-KILL", &human_first["pid"].to_string()]));
    tool_said(&fake, &chat, &c, "browse: https://example.com").await;
    assert_ne!(desktop_browser(&c, profile)["pid"], human_first["pid"], "a crash starts a new browser, not the stale endpoint");
    c.restart();
    let navigated = tool_said(&fake, &chat, &c, "browse: https://example.com").await;
    let cookie = tool_said(&fake, &chat, &c, read_cookie).await;
    assert!(cookie.contains("cookie-kept"), "the cookie survives a computer restart: {cookie}; {navigated}");
}

/// Hermes' own reading of `profile`'s Bot Desktop lease, as its
/// computer_use reads it before every action: `{holder, viewer_id, epoch,
/// refused}`, `refused` being what `assert_agent_may_act` raised (its
/// `human_has_control`), or null.
fn hermes_lease(c: &Container, profile: &str) -> serde_json::Value {
    const SCRIPT: &str = r#"import json, os, sys
os.chdir('/opt/hermes'); sys.path.insert(0, '/opt/hermes')
from tools.bot_desktop import lease
home = sys.argv[1]
l = lease.get(home)
try:
    lease.assert_agent_may_act(home); refused = None
except lease.HumanHasControl as e:
    refused = str(e)
print(json.dumps({"holder": l.holder, "viewer_id": l.viewer_id, "epoch": l.epoch, "refused": refused}))
"#;
    let home = format!("/data/hermes/profiles/{profile}");
    let out = c.exec_out(&["env", "HERMES_HOME=/data/hermes", "/command/s6-setuidgid", "hermes", "/opt/hermes/.venv/bin/python", "-c", SCRIPT, &home]);
    serde_json::from_str(out.trim().lines().last().unwrap_or("null")).unwrap_or(serde_json::Value::Null)
}

/// Whether `profile`'s desktop is up: its launcher's published environment.
fn desktop_up(c: &Container, profile: &str) -> bool {
    c.exec(&["test", "-e", &format!("/data/hermes/profiles/{profile}/bot-desktop/env")])
}

/// Goal: two agents on one computer, two desktops (Hermes gives each
/// profile its own), each its own screen: a socket naming juniper is
/// juniper's desktop and one naming fred is fred's, each started for its
/// own first viewer; an agent not on the computer, or no name, refused.
/// Take over of juniper's screen is juniper's Bot Desktop lease, as Hermes
/// reads it (`human`, the viewer, `human_has_control`), and juniper's
/// computer_use refuses while it holds, fred's working on its own desktop;
/// Give back is juniper's again, and its computer_use works. A desktop no
/// one watches or uses stops after the idle bound (here 30 s), one watched
/// does not, and the next viewer starts it again, the browser's profile
/// (in the agent's work) kept. As Containers runs the image.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn two_agents_two_desktops() {
    use fragment_bridge::net::Base;
    use support::rfb::{Control, Viewer};
    let tag = hermes_tag();
    build(&repo_dir(), "images/hermes/Dockerfile", &tag);
    let fake = Fake::start("0.0.0.0:0", &["juniper", "fred"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let (jchat, fchat) = (fake.chat("juniper-desk", &["juniper"]), fake.chat("fred-desk", &["fred"]));
    let c = Container::run_on(Runtime::Hosted, &tag, fake.addr.port(), model.addr.port(), &[("HERMES_BOOT_SCREEN_IDLE_MS", "30000")]);
    fake.until(180_000, "Hermes' bridge to follow both agents' chats", |w| w.live_sockets() >= 4).await;
    let base = Base::parse(&format!("http://127.0.0.1:{}", c.port(6080))).unwrap();
    let screens: serde_json::Value = serde_json::from_str(&c.exec_out(&["cat", "/var/lib/fragment-run/screens.json"])).unwrap_or_default();
    eprintln!("two desktops: the screens file {screens}");
    assert!(!c.exec(&["test", "-e", "/var/lib/fragment-run/screen.sock"]), "no one screen for every agent");

    // each agent's own screen, refusals included
    let mut jc = Control::open(&base, "watcher", "juniper--k3x9").await.unwrap();
    assert_eq!(jc.next().await.unwrap(), json!({ "type": "control", "agent": "juniper--k3x9", "holder": null }));
    let mut fc = Control::open(&base, "watcher", "fred--k3x9").await.unwrap();
    assert_eq!(fc.next().await.unwrap(), json!({ "type": "control", "agent": "fred--k3x9", "holder": null }));
    let nobody = fragment_bridge::net::connect_ws(&base, "/websockify?viewer=w&agent=nobody--k3x9", &[]).await.err().unwrap_or_default();
    assert!(nobody.contains("404"), "an agent not on this computer: {nobody}");
    let unnamed = fragment_bridge::net::connect_ws(&base, "/websockify?viewer=w&agent=../fred--k3x9", &[]).await.err().unwrap_or_default();
    assert!(unnamed.contains("400"), "no agent's name: {unnamed}");
    let t = Instant::now();
    let jv = Viewer::open(&base, "watcher", "juniper--k3x9", Duration::from_secs(60)).await.unwrap_or_else(|e| panic!("juniper's screen: {e}\n{}", c.logs()));
    let mut fv = Viewer::open(&base, "watcher", "fred--k3x9", Duration::from_secs(60)).await.unwrap_or_else(|e| panic!("fred's screen: {e}\n{}", c.logs()));
    eprintln!("two desktops: both up for their viewers in {} ms; the container holds {} MiB", t.elapsed().as_millis(), anon_mib(&c));
    assert_eq!((jv.name.as_str(), fv.name.as_str()), ("hermes:juniper--k3x9", "hermes:fred--k3x9"), "each socket shows its own agent's desktop");
    let xvnc = c.exec_out(&["sh", "-c", "pgrep -x Xvnc | wc -l"]);
    assert_eq!(xvnc.trim(), "2", "two desktops, two X servers");
    // Both icons start a browser; the agents attach to their own profile's
    // live PID and port even if another profile's old port file is present.
    open_browser_icon(&c, "juniper--k3x9");
    open_browser_icon(&c, "fred--k3x9");
    let (jb, fb) = (desktop_browser(&c, "juniper--k3x9"), desktop_browser(&c, "fred--k3x9"));
    assert_ne!(jb["port"], fb["port"], "two agents never share a CDP port");
    assert_ne!(jb["pid"], fb["pid"], "two browser processes");
    const GUARDS: &str = r#"import os, sys, tempfile
from pathlib import Path
sys.path.insert(0, '/opt/hermes')
from tools.bot_desktop.fragment_browser import _owned_process, _check_version
with tempfile.TemporaryDirectory(dir='/tmp') as directory:
    profile = Path(directory)
    # A saved SingletonLock whose PID now belongs to another agent is stale.
    os.symlink('previous-host-' + sys.argv[1], profile / 'SingletonLock')
    assert _owned_process(profile) is None, 'a foreign PID is never adopted'
    version = profile / 'Last Version'
    version.write_text('999.0.0.0')
    try:
        _check_version(profile)
    except RuntimeError as error:
        assert 'newer Chromium image' in str(error)
    else:
        raise AssertionError('a newer profile must refuse before Chromium starts')
    assert version.read_text() == '999.0.0.0', 'refusal never rewrites the profile'
"#;
    let (code, out) = c.exec_code(&["/command/s6-setuidgid", "hermes", "/opt/hermes/.venv/bin/python", "-c", GUARDS, &fb["pid"].to_string()]);
    assert_eq!(code, 0, "browser ownership and profile-version guards: {out}");
    let stale = format!("printf '{}\\n/devtools/browser/stale\\n' > /data/work/juniper--k3x9/browser-profile/DevToolsActivePort", fb["port"]);
    assert!(c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", &stale]));
    assert_eq!(desktop_browser(&c, "juniper--k3x9")["pid"], jb["pid"], "another agent's stale port cannot redirect attachment");

    // Take over of juniper's screen is juniper's lease
    let mut driving = Control::open(&base, "driver", "juniper--k3x9").await.unwrap();
    assert_eq!(driving.next().await.unwrap()["holder"], json!(null));
    let mut driver = Viewer::open(&base, "driver", "juniper--k3x9", Duration::from_secs(30)).await.unwrap();
    driving.say("take").await.unwrap();
    assert_eq!(driving.next().await.unwrap()["holder"], "driver");
    assert_eq!(jc.next().await.unwrap()["holder"], "driver", "juniper's watcher hears it");
    let (jl, fl) = (hermes_lease(&c, "juniper--k3x9"), hermes_lease(&c, "fred--k3x9"));
    eprintln!("two desktops: juniper's lease as Hermes reads it {jl}; fred's {fl}");
    assert!(jl["holder"] == "human" && jl["viewer_id"] == "driver" && jl["refused"].is_string(), "juniper's lease is the person's, and Hermes refuses juniper's screen actions: {jl}");
    assert!(fl["holder"] == "agent" && fl["refused"].is_null(), "fred's is fred's: {fl}");
    driver.pointer(101, 57, 0).await.unwrap();
    pointer_at(&c, "juniper--k3x9", [101, 57], "the person drives juniper's desktop").await;
    assert_ne!(desk(&c, "fred--k3x9")["pointer"], json!([101, 57]), "and not fred's");

    // each agent's computer_use, as the lease has it
    let look = |chat: &str, agent: &str| {
        let said = fake.say(chat, &person("paul"), json!({ "text": "look at your screen" }));
        fragment_bridge::records::turn_id(agent, chat, "chat", said["seq"].as_u64().unwrap())
    };
    let reply = |w: &support::fake::World, chat: &str, turn: &str| w.bodies(chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn).and_then(|r| r["text"].as_str().map(str::to_string));
    let jturn = look(&jchat, "juniper--k3x9");
    within(&fake, &jchat, &c, 240_000, "juniper's look while a person holds its screen", |w| reply(w, &jchat, &jturn).is_some()).await;
    let jsaid = fake.with(|w| reply(w, &jchat, &jturn)).unwrap_or_default();
    eprintln!("two desktops: juniper, held: {}", jsaid.chars().take(400).collect::<String>());
    assert!(jsaid.contains("human_has_control"), "juniper's computer_use refuses while a person holds its screen: {jsaid}");
    let refused = tool_said(&fake, &jchat, &c, "inspect browser: document.title").await;
    assert!(refused.to_lowercase().contains("human has taken over"), "the native browser tool retains the desktop's lease fence: {refused}");
    let fturn = look(&fchat, "fred--k3x9");
    within(&fake, &fchat, &c, 240_000, "fred's look", |w| reply(w, &fchat, &fturn).is_some()).await;
    let fsaid = fake.with(|w| reply(w, &fchat, &fturn)).unwrap_or_default();
    eprintln!("two desktops: fred, meanwhile: {}", fsaid.chars().take(400).collect::<String>());
    assert!(!fsaid.contains("human_has_control") && !fsaid.contains("no computer_use"), "fred's computer_use works on its own desktop: {fsaid}");
    let fred_looked = model.calls.lock().unwrap().iter().any(|m| m.agent.as_deref() == Some("fred--k3x9") && m.body["messages"].to_string().contains("\"image_url\""));
    assert!(fred_looked, "fred's capture went to the vision model, as fred");

    // Give back: juniper's lease again, and its computer_use works
    driving.say("give").await.unwrap();
    assert_eq!(driving.next().await.unwrap()["holder"], json!(null));
    let jl = hermes_lease(&c, "juniper--k3x9");
    assert!(jl["holder"] == "agent" && jl["refused"].is_null() && jl["epoch"].as_u64() >= Some(2), "given back: {jl}");
    let jturn = look(&jchat, "juniper--k3x9");
    within(&fake, &jchat, &c, 240_000, "juniper's look, given back", |w| reply(w, &jchat, &jturn).is_some()).await;
    let jsaid = fake.with(|w| reply(w, &jchat, &jturn)).unwrap_or_default();
    assert!(!jsaid.contains("human_has_control"), "given back, juniper's computer_use works: {jsaid}");
    let unheld = tool_said(&fake, &jchat, &c, "browse: https://example.com").await;
    assert_eq!(desktop_browser(&c, "juniper--k3x9")["pid"], jb["pid"], "the native tool attaches after Give back: {unheld}");

    // idle: juniper's desktop, unwatched and unused, stops; fred's, watched, does not
    let marker = "/data/work/juniper--k3x9/browser-profile/fragment-marker";
    assert!(c.exec(&["/command/s6-setuidgid", "hermes", "sh", "-c", &format!("echo kept > {marker}")]));
    drop((jc, driving, jv, driver));
    let t = Instant::now();
    // bounded: the 30 s bound, a look every 7.5 s, and Hermes' own stop
    while desktop_up(&c, "juniper--k3x9") && t.elapsed() < Duration::from_secs(120) {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = fv.frame().await;
    }
    eprintln!("two desktops: juniper's stopped {} ms after its last viewer left", t.elapsed().as_millis());
    assert!(!desktop_up(&c, "juniper--k3x9"), "an unwatched, unused desktop stops: {}", c.logs().lines().filter(|l| l.contains("screen.")).collect::<Vec<_>>().join("\n"));
    assert!(desktop_browser(&c, "juniper--k3x9").is_null(), "idle stop closes the browser too");
    assert_eq!(desktop_browser(&c, "fred--k3x9")["pid"], fb["pid"], "stopping one agent spares the other's browser");
    assert!(desktop_up(&c, "fred--k3x9"), "one watched does not");
    let stopped = c.logs().lines().filter(|l| l.contains("\"screen.idle_stopped\"") && l.contains("juniper--k3x9")).map(str::to_string).collect::<Vec<_>>();
    assert!(!stopped.is_empty() && stopped.iter().all(|l| l.contains("\"code\":0")), "Hermes' own stop: {stopped:?}");
    assert_eq!(c.exec_out(&["sh", "-c", "pgrep -x Xvnc | wc -l"]).trim(), "1", "its X server gone");
    drop((fc, fv));
    let t = Instant::now();
    while desktop_up(&c, "fred--k3x9") && t.elapsed() < Duration::from_secs(120) {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(!desktop_up(&c, "fred--k3x9"), "fred's too, once no one watches it");

    // and starts again on demand, its browser's profile kept
    let back = Viewer::open(&base, "watcher", "juniper--k3x9", Duration::from_secs(60)).await.unwrap_or_else(|e| panic!("juniper's screen again: {e}\n{}", c.logs()));
    assert_eq!(back.name, "hermes:juniper--k3x9");
    assert_eq!(c.exec_out(&["cat", marker]).trim(), "kept", "its browser's profile, in its work, kept");
    assert_eq!(c.exec_out(&["readlink", "/data/hermes/profiles/juniper--k3x9/bot-desktop/browser-profile"]).trim(), "/data/work/juniper--k3x9/browser-profile");
}

// ---- what Hermes would decide by guessing it runs in a container (its
// `is_container()`: Docker's marker `/.dockerenv`, which Containers has
// not; images/hermes/boot/src/hermes.rs, `RUNTIME_ENV`) ----

/// What one runtime shows of an agent's home.
#[derive(Debug)]
struct Homes {
    /// Whether Hermes takes the runtime for a container.
    container: bool,
    /// The agent's terminal's `HOME`, and where it leads.
    terminal: String,
    terminal_real: String,
    /// Where Hermes' write_file put `~/fragment-home.txt`, as it said, and
    /// where that leads.
    written: String,
    written_real: String,
    /// The modes of Hermes' home and its directories (`<mode> <path>`).
    modes: String,
}

/// The value after `name=` in what a tool said, up to a space or a quote.
fn said(text: &str, name: &str) -> String {
    let pat = format!("{name}=");
    text.split_once(&pat).map(|(_, rest)| rest.chars().take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\\' | ',')).collect()).unwrap_or_default()
}

/// The Hermes image on `runtime`: juniper's terminal says its `HOME`, and
/// its write_file writes under `~`.
async fn homes(runtime: Runtime) -> Homes {
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("home", &["juniper"]);
    let c = Container::run_on(runtime, &hermes_tag(), fake.addr.port(), model.addr.port(), &[]);
    within(&fake, &chat, &c, 180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let guess = c.exec_out(&["/command/s6-setuidgid", "hermes", "/opt/hermes/.venv/bin/python", "-c", "import os; os.chdir('/opt/hermes'); from hermes_platform.host.runtime import is_container; print(is_container())"]);
    let ask = |text: &str| {
        let said = fake.say(&chat, &person("paul"), json!({ "text": text }));
        fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap())
    };
    let reply = |turn: &str| fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn)).map(|r| r["text"].as_str().unwrap_or("").to_string());
    let turn = ask("run: echo home=$HOME real=$HERMES_REAL_HOME path=$PATH");
    within(&fake, &chat, &c, 240_000, "the terminal's reply", |w| w.bodies(&chat, "chat", "reply").iter().any(|r| r["turn"] == turn)).await;
    let terminal = reply(&turn).unwrap_or_default();
    eprintln!("home: on {runtime:?} the terminal said {terminal:?}");
    let turn = ask("write: ~/fragment-home.txt");
    within(&fake, &chat, &c, 240_000, "write_file's reply", |w| w.bodies(&chat, "chat", "reply").iter().any(|r| r["turn"] == turn)).await;
    let wrote = reply(&turn).unwrap_or_default();
    // the absolute path it wrote, of those it names
    let written = wrote.match_indices("fragment-home.txt").filter_map(|(at, _)| wrote[..at].rsplit(['"', ' ', '\'']).next()).find(|dir| dir.starts_with('/')).map(|dir| format!("{dir}fragment-home.txt")).unwrap_or_default();
    eprintln!("home: on {runtime:?} write_file said {wrote:?}");
    if !written.is_empty() {
        assert_eq!(c.exec_out(&["cat", &written]), support::model::WRITTEN, "write_file wrote where it said");
    }
    let dirs = ["/data/hermes", "/data/hermes/sessions", "/data/hermes/logs", "/data/hermes/memories", "/data/hermes/cron", "/data/hermes/cache/scratch", "/data/hermes/profiles/juniper--k3x9", "/data/hermes/profiles/juniper--k3x9/sessions", "/data/work/juniper--k3x9/tmp", "/data/work/juniper--k3x9/home"];
    let modes = c.exec_out(&["sh", "-c", &format!("stat -c '%a %n' {} 2>/dev/null", dirs.join(" "))]);
    let real = |p: &str| if p.is_empty() { String::new() } else { c.exec_out(&["realpath", p]).trim().to_string() };
    let terminal = said(&terminal, "home");
    Homes { container: guess.trim() == "True", terminal_real: real(&terminal), terminal, written_real: real(&written), written, modes }
}

/// Goal: an agent's home is the same on Docker and on Containers, though
/// Hermes takes only Docker for a container: its terminal's `HOME` and its
/// file tools' `~` are its profile's own `home` (each agent its own, as
/// Hermes gives a container), a link to its home in its work, and Hermes
/// leaves its home's modes as the image makes them. Pinned by the image
/// (`RUNTIME_ENV`), not guessed.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn an_agents_home_is_the_same_on_either_runtime() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let (docker, hosted) = tokio::join!(homes(Runtime::Docker), homes(Runtime::Hosted));
    eprintln!("home: Docker {docker:#?}\nhome: Containers' way {hosted:#?}");
    assert!(docker.container && !hosted.container, "Hermes takes Docker for a container and the hosted rung for none (else this proves nothing): {docker:?} {hosted:?}");
    let (profile_home, home) = ("/data/hermes/profiles/juniper--k3x9/home", "/data/work/juniper--k3x9/home");
    for (on, h) in [("Docker", &docker), ("Containers' way", &hosted)] {
        assert_eq!(h.terminal, profile_home, "on {on}, the agent's terminal's HOME is its profile's: {h:#?}");
        assert_eq!(h.terminal_real, home, "on {on}, which is its home in its work: {h:#?}");
        assert!(h.written.ends_with("/fragment-home.txt"), "on {on}, write_file said where it wrote: {h:#?}");
        assert_eq!(h.written_real, format!("{home}/fragment-home.txt"), "on {on}, its file tools' `~` is the same home: {h:#?}");
    }
    assert_eq!(docker.modes, hosted.modes, "Hermes leaves its home's modes alike on both");
}

/// The image's last `held` event: what its last hold copied, and kept hot.
fn held_event(c: &Container) -> Option<serde_json::Value> {
    c.logs().lines().rev().filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok()).find(|v| v["event"] == "held")
}

/// Goal (Paul, 2026-10-07: an agent's `~` is its work): a Chromium the
/// agent runs from its terminal, headless with no profile named, the full
/// one as its desktop runs it, keeps its databases out of Hermes' home, so
/// a hold while it runs keeps none of them hot (`locked` empty), and none
/// is under Hermes' home for a restore's check to find. Before, its profile
/// was under Hermes' home (on Containers `/data/hermes/.config/…`; with the
/// home pinned, the profile's `home/.config/…`), and its
/// `declarative_performance_observer.db` was held locked. Chrome 153 (Hermes
/// v0.21.5's image) kept that temporary profile under `~`, in its work;
/// Chrome 145 (Hermes v0.21.6) keeps it in `TMPDIR`, which Hermes points at
/// its profile's scratch (a link into its work, saved with it), so the
/// image's Chromium gives it the container's `/tmp` (hermes.rs,
/// `CHROMIUM_TMP`).
/// The agent starts it as Hermes' background process (`start:`).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_browser_the_agent_runs_keeps_its_databases_in_its_work() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("browse", &["juniper"]);
    let c = Container::run_on(Runtime::Hosted, &hermes_tag(), fake.addr.port(), model.addr.port(), &[]);
    within(&fake, &chat, &c, 180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let said = fake.say(&chat, &person("paul"), json!({ "text": "start: DISPLAY=:99 /opt/fragment/bin/chromium --headless=new about:blank" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    within(&fake, &chat, &c, 240_000, "the terminal's reply", |w| w.bodies(&chat, "chat", "reply").iter().any(|r| r["turn"] == turn)).await;
    let dbs = |under: &str| c.exec_out(&["sh", "-c", &format!("find {under} \\( -name '*.db' -o -name History -o -name Cookies \\) -path '*chrom*' 2>/dev/null")]);
    let t = Instant::now();
    while dbs("/data/ /tmp/").trim().is_empty() {
        assert!(t.elapsed() < Duration::from_secs(60), "the agent's Chromium made no databases; the terminal said {:?}", fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn)));
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(c.exec(&["pgrep", "-f", "/usr/lib/chromium/chromium"]), "the agent's regular Chromium runs into the hold");
    let left_out = hold(&c).unwrap_or_else(|| panic!("no answer to the hold; the container said:\n{}", c.logs()));
    let held = held_event(&c).unwrap_or_else(|| panic!("no held event; the container said:\n{}", c.logs()));
    eprintln!("browse: held {held}; left out {left_out:?}\nbrowse: its Chromium's databases:\n{}", dbs("/data/ /tmp/"));
    assert!(c.exec(&["pgrep", "-f", "/usr/lib/chromium/chromium"]), "and through it");
    assert_eq!(held["locked"], json!([]), "no database kept hot for being locked: {held}");
    assert_eq!(dbs("/data/hermes/").trim(), "", "none of its databases is under Hermes' home");
    assert!(!dbs("/tmp/").trim().is_empty(), "the regular browser's headless throwaway profile stays in the container's /tmp");
    assert!(dbs("/data/work/juniper--k3x9/home/").trim().is_empty(), "a headless throwaway profile is not saved as the agent's work");
    unhold(&c);
}

/// Says `text` and waits until `done` holds of its turn, which asks its
/// owner nothing: the turn.
async fn asked(fake: &Fake, chat: &str, c: &Container, text: &str, done: impl Fn(&support::fake::World, &str) -> bool) -> String {
    let said = fake.say(chat, &person("paul"), json!({ "text": text }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", chat, "chat", said["seq"].as_u64().unwrap());
    let t = Instant::now();
    // bounded: four minutes
    loop {
        if fake.with(|w| done(w, &turn)) {
            return turn;
        }
        assert!(fake.with(|w| !w.bodies(chat, "work", "turn.prompt").iter().any(|p| p["turn"] == turn)), "{text:?} asks its owner nothing; {}", told(fake, chat, c));
        assert!(t.elapsed() < Duration::from_secs(240), "{text:?} not done in 240 s; {}", told(fake, chat, c));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// What the tool of `text`'s turn said, as the scripted model quotes it
/// (`scripted: the tool said: …`).
async fn tool_said(fake: &Fake, chat: &str, c: &Container, text: &str) -> String {
    let reply = |w: &support::fake::World, turn: &str| w.bodies(chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn);
    let turn = asked(fake, chat, c, text, |w, turn| reply(w, turn).is_some()).await;
    fake.with(|w| reply(w, &turn)).and_then(|r| r["text"].as_str().map(str::to_string)).unwrap_or_default()
}

/// Goal (the seam, step 2 of docs/durable-computers.md): a temp file an
/// agent's tools make is its work (`/data/work/<profile>/tmp`), saved with
/// it, never Hermes' home. Hermes points every child's `TMPDIR` at its
/// home's `cache/scratch` (`hermes_constants.apply_scratch_tmp_env`), for an
/// agent's the profile's, which until 2026-10-07 was a directory under
/// Hermes' home (`/data/hermes/profiles/juniper--k3x9/cache/scratch`); the
/// image makes it a link into the work (`hermes::tmp_dir`). Each way a tool
/// runs is given its environment its own way, so each is asked: its
/// terminal (`mktemp`), a background process it starts (`mktemp`, its path
/// written to a file), and execute_code (Python's `tempfile`), whose
/// scripts Hermes gives the gateway's own temp directory instead
/// (docs/technical-debt-ledger.md, "An agent's execute_code makes its temp
/// files in Hermes' home"): pinned here, so the day Hermes fixes it this
/// fails and the entry goes. Then Hermes' own use of the scratch: a file
/// made there and named in the reply's `MEDIA:` tag, as Hermes' prompts
/// suggest, is still sent, read through the link.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_tools_temp_files_are_its_work() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("temp", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[]);
    within(&fake, &chat, &c, 180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let (work, real) = ("/data/work/juniper--k3x9/tmp", |p: &str| c.exec_out(&["realpath", "-e", p]).trim().to_string());
    let terminal = tool_said(&fake, &chat, &c, "run: echo tmpdir=$TMPDIR made=$(mktemp)").await;
    let started = "/data/work/juniper--k3x9/started-temp.txt";
    let background = tool_said(&fake, &chat, &c, &format!("start: echo made=$(mktemp) > {started}")).await;
    let t = Instant::now();
    // bounded: a minute
    while !c.exec(&["test", "-s", started]) {
        assert!(t.elapsed() < Duration::from_secs(60), "the background process wrote nothing; the terminal said {background:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let background = c.exec_out(&["cat", started]);
    let code = tool_said(&fake, &chat, &c, "code: import tempfile; print('made=' + tempfile.mkstemp()[1])").await;
    let made: Vec<(&str, String, String)> = [("its terminal", &terminal), ("a background process", &background), ("execute_code", &code)].into_iter().map(|(tool, text)| (tool, said(text, "made"), text.clone())).collect();
    for (tool, path, _) in &made {
        eprintln!("temp: {tool} made {path:?}, in {:?}", real(path));
    }
    eprintln!("temp: the gateway's own scratch holds {:?}", c.exec_out(&["ls", "-A", "/data/hermes/cache/scratch"]));
    let tmpdir = said(&terminal, "tmpdir");
    assert_eq!(real(&tmpdir), work, "its terminal's TMPDIR ({tmpdir:?}) is its work's: {terminal}");
    for (tool, path, text) in &made {
        assert!(path.starts_with('/'), "{tool} said where it made its temp file: {text:?}");
        let in_fact = real(path);
        if *tool == "execute_code" {
            assert!(
                in_fact.starts_with("/data/hermes/cache/scratch/"),
                "execute_code's temp file is no longer the gateway's own but {in_fact:?}: Hermes now keeps its scratch marker for execute_code's scripts, so drop the ledger's entry (\"An agent's execute_code makes its temp files in Hermes' home\") and hold it to its work with the rest"
            );
        } else {
            assert!(in_fact.starts_with(&format!("{work}/")), "{tool}'s temp file {path:?} is in its work, not {in_fact:?}");
        }
    }
    let attached = |w: &support::fake::World, turn: &str| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn && r.get("attachments").is_some());
    let turn = asked(&fake, &chat, &c, "send: f=$(mktemp --suffix=.txt) && echo temp-sent > $f && echo made=$f", |w, turn| attached(w, turn).is_some()).await;
    fake.with(|w| {
        let reply = attached(w, &turn).unwrap();
        let sha = reply["attachments"][0]["sha256"].as_str().unwrap_or("");
        let sent = w.fragments[&chat].blobs.get(sha).map(|(_, b)| b.clone());
        assert_eq!(sent.as_deref(), Some(&b"temp-sent\n"[..]), "the temp file Hermes sent is the one in its work: {reply}");
    });
}

/// Goal (Paul on preview, 2026-10-16: asked its version, an agent read
/// "0.0.0" from Hermes' package metadata, and found `hermes` only by its
/// full path): an agent runs Hermes' own CLI by name, each way its tools
/// run a command. Its terminal and a background process it starts run in
/// a login shell (`bash -l`, `bash -lic`), where Debian's `/etc/profile`
/// sets PATH anew, without the image's `/opt/hermes/bin`; the image puts
/// `hermes` in `/usr/local/bin` (images/hermes/Dockerfile). Pinned that
/// it is the image's that the terminal finds, so the day Hermes' login
/// shell keeps the image's PATH this fails and that file goes. Invalid
/// (Paul, 2026-10-20: an agent asked to check something hourly had no
/// scheduler that lasted): `hermes cron` in its terminal is refused (exit
/// 2), saying to add a schedule (`fragment schedule add`) instead.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn an_agent_runs_hermes_by_name() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("by-name", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[]);
    within(&fake, &chat, &c, 180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    // what Hermes says it is, by its full path: `Hermes Agent v0.21.7`
    let full = c.exec_out(&["/command/s6-setuidgid", "hermes", "/opt/hermes/bin/hermes", "--version"]);
    let version = full.lines().next().and_then(|l| l.split(" (").next()).unwrap_or("").to_string();
    assert!(version.starts_with("Hermes Agent v"), "Hermes says its version by its full path: {full:?}");
    let terminal = tool_said(&fake, &chat, &c, "run: echo found=$(command -v hermes) && hermes --version").await;
    let started = "/data/work/juniper--k3x9/started-hermes.txt";
    let background = tool_said(&fake, &chat, &c, &format!("start: hermes --version > {started} 2>&1")).await;
    let t = Instant::now();
    // bounded: a minute
    while !c.exec(&["test", "-s", started]) {
        assert!(t.elapsed() < Duration::from_secs(60), "the background process wrote nothing; the terminal said {background:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let background = c.exec_out(&["cat", started]);
    let code = tool_said(&fake, &chat, &c, "code: import subprocess; print(subprocess.run(['hermes', '--version'], capture_output=True, text=True).stdout)").await;
    let ran = [("its terminal", &terminal), ("a background process", &background), ("execute_code", &code)];
    for (tool, text) in ran {
        eprintln!("by name: {tool} said {text:?}");
    }
    for (tool, text) in ran {
        assert!(text.contains(&version), "{tool} runs `hermes` by name, as {version:?}: {text:?}");
    }
    let found = said(&terminal, "found");
    assert_eq!(found, "/usr/local/bin/hermes", "its terminal found Hermes' CLI at {found:?}: at /opt/hermes/bin/hermes, Hermes' login shell keeps the image's PATH now, and /usr/local/bin/hermes (images/hermes/Dockerfile) can go");
    // Hermes' own scheduler is off (decision 38): in its terminal, `hermes
    // cron` is refused, pointing at the platform's routines; `--version`
    // passed through above
    let cron = tool_said(&fake, &chat, &c, "run: hermes cron list 2>&1; echo cron-exit=$?").await;
    eprintln!("by name: hermes cron said {cron:?}");
    assert_eq!(said(&cron, "cron-exit"), "2", "the image's hermes refuses `hermes cron`: {cron:?}");
    assert!(cron.contains("fragment schedule add --routine"), "and says to add a schedule instead: {cron:?}");
}

/// Goal (Paul on p5, 2026-10-09: "'other' type your answer for questions
/// doesn't work very well"): real Hermes' clarify with choices is a card
/// showing its question, its "Other" answered in words; answered so on the
/// card, the words reach the clarify as its answer (the bridge sends them
/// right after the answer, and Hermes waits on words as it takes it), and
/// the turn's one reply quotes them: nothing asks "Type your answer:".
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn other_answered_in_words_on_its_card() {
    let (fake, _model, chat, c) = hermes_running().await;
    let said = fake.say(&chat, &person("paul"), json!({ "text": "clarify: Which color? | red | blue" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    let card = |w: &support::fake::World| w.bodies(&chat, "work", "turn.prompt").into_iter().find(|p| p["turn"] == turn);
    within(&fake, &chat, &c, 120_000, "the clarify's card", |w| card(w).is_some()).await;
    let p = fake.with(|w| card(w).unwrap());
    let words: Vec<(String, bool)> = p["options"].as_array().unwrap().iter().map(|o| (o["id"].as_str().unwrap_or("").to_string(), o["words"] == true)).collect();
    // its question without the glyph (v0.21.6 adds a line on "skip")
    assert!(p["text"].as_str().is_some_and(|t| t.starts_with("Which color?")), "{p}");
    assert_eq!(words, vec![("c0".to_string(), false), ("c1".to_string(), false), ("other".to_string(), true)], "{p}");
    fake.say(&chat, &person("paul"), json!({ "kind": "prompt_response", "prompt": p["prompt"], "option": "other", "text": "purple" }));
    within(&fake, &chat, &c, 120_000, "the turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn)).await;
    fake.with(|w| {
        let replies: Vec<String> = w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == turn).map(|r| r["text"].as_str().unwrap_or("").to_string()).collect();
        assert!(replies.len() == 1 && replies[0].contains("purple"), "its one reply quotes the words, nothing asks for them: {replies:?}");
        let closed = w.bodies(&chat, "work", "turn.prompt.closed").into_iter().find(|x| x["turn"] == turn).unwrap();
        assert_eq!((&closed["option"], &closed["text"]), (&json!("other"), &json!("purple")), "{closed}");
        let end = w.bodies(&chat, "work", "turn.end").into_iter().find(|e| e["turn"] == turn).unwrap();
        assert_eq!(end["outcome"], "idle", "{end}");
    });
}

/// Goal (the owner's commands, docs/chat-records.md; Hermes v0.21.6's
/// manifest, relay/menu.rs): real Hermes takes a `{kind: "command"}` the
/// bridge says as it is, as its own command, never as words for its model:
/// `/usage` is answered with the session's usage, no model call; `/steer`
/// lands in the running turn (its words in the model's next request,
/// after the tool), and Hermes' word on it is a message of its own; a
/// message quoting the agent's reply reaches the model as Hermes' quoted
/// reply (`[Replying to your previous message: "…"]`); `/new` starts a new
/// session for the chat, so the next message's request holds none of the
/// chat before; `/model … --global` is refused, its end saying why, and
/// Hermes never hears it.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn commands_and_quotes_reach_hermes() {
    let (fake, model, chat, c) = hermes_running().await;
    let calls = || model.calls.lock().unwrap().clone();
    let turn = |said: &serde_json::Value| fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    let reply =|t: &str| fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["turn"] == t)).map(|r| r["text"].as_str().unwrap_or("").to_string());
    let command = |name: &str, args: &str| fake.say(&chat, &person("paul"), json!({ "kind": "command", "command": name, "args": args }));

    // /usage: Hermes' own answer, no model call
    let before = calls().len();
    let usage = turn(&command("usage", ""));
    within(&fake, &chat, &c, 120_000, "/usage's end", ended_turn(&chat, &usage)).await;
    let said = reply(&usage).unwrap_or_default();
    eprintln!("commands: /usage said {said:?}");
    assert!(said.to_lowercase().contains("token") || said.to_lowercase().contains("usage"), "Hermes' usage, not the model's words: {said:?}; {}", told(&fake, &chat, &c));
    assert!(!said.starts_with("scripted:"), "{said:?}");
    assert!(calls()[before..].iter().all(|x| !x.body["messages"].to_string().contains("/usage")), "no model call for it");

    // a quote of the agent's first reply
    let first = fake.with(|w| w.records(&chat, "chat").into_iter().find(|r| r["body"]["turn"].is_string()).unwrap());
    let quoted = first["body"]["text"].as_str().unwrap_or("").to_string();
    let q = turn(&fake.say(&chat, &person("paul"), json!({ "text": "what did you mean", "reply_to": first["seq"] })));
    within(&fake, &chat, &c, 120_000, "the quoting message's end", ended_turn(&chat, &q)).await;
    let (asked, _) = asked_with(&calls(), "what did you mean").unwrap_or_else(|| panic!("a request for the quoting message; {}", told(&fake, &chat, &c)));
    eprintln!("commands: the quote as the model saw it: {asked:?}");
    assert!(asked.contains(&format!("[Replying to your previous message: \"{quoted}\"]")), "Hermes' quoted reply, its own: {asked:?}");

    // /steer while a turn runs its tool: into that turn
    let slow = turn(&fake.say(&chat, &person("paul"), json!({ "text": "use the terminal slowly" })));
    within(&fake, &chat, &c, 120_000, "the slow tool is running", |_| c.exec(&["pgrep", "-f", "^sleep 8$"])).await;
    let steer = command("steer", "and say blue");
    within(&fake, &chat, &c, 120_000, "the slow turn's end", ended_turn(&chat, &slow)).await;
    let steered = calls().iter().any(|x| x.body["messages"].to_string().contains("and say blue"));
    let word = fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().find(|r| r["text"].as_str().is_some_and(|t| t.starts_with('⏩'))));
    eprintln!("commands: steer's word {word:?}");
    assert!(steered, "the steer is in a request of the running turn's; {}", told(&fake, &chat, &c));
    fake.with(|w| {
        assert!(w.bodies(&chat, "work", "turn.start").iter().all(|s| s["cause"]["seq"] != steer["seq"]), "the steer is no turn of its own");
        let word = word.as_ref().expect("Hermes' word on the steer");
        assert_ne!(word["turn"], slow.as_str(), "a message of its own, not the running turn's: {word}");
    });

    // /new: a new session for the chat
    let fresh = turn(&command("new", ""));
    within(&fake, &chat, &c, 120_000, "/new's end", ended_turn(&chat, &fresh)).await;
    eprintln!("commands: /new said {:?}", reply(&fresh));
    let after = turn(&fake.say(&chat, &person("paul"), json!({ "text": "a fresh start" })));
    within(&fake, &chat, &c, 120_000, "the next message's end", ended_turn(&chat, &after)).await;
    let request = calls().into_iter().find(|x| x.path.ends_with("/chat/completions") && asked_with(std::slice::from_ref(x), "a fresh start").is_some()).expect("a request for the next message");
    let history = request.body["messages"].to_string();
    for earlier in ["what did you mean", "use the terminal slowly", "[paul] hello"] {
        assert!(!history.contains(earlier), "the new session holds none of the chat before ({earlier:?}): {history}");
    }

    // /model: the model its tier runs, by its id, and the ids the route takes
    let before = calls().len();
    let shown = model_shown(&fake, &chat, &c).await;
    eprintln!("commands: /model said {shown:?}");
    assert!(shown.contains(CHEAP_ID) && !shown.starts_with("scripted:"), "Hermes' /model names the real model: {shown:?}; {}", told(&fake, &chat, &c));
    assert!(calls()[before..].iter().all(|x| !x.body["messages"].to_string().contains("/model")), "no model call for it");

    // refused: never said to Hermes
    let global = turn(&command("model", "glm --global"));
    within(&fake, &chat, &c, 30_000, "the refused command's end", ended_turn(&chat, &global)).await;
    fake.with(|w| {
        let end = w.bodies(&chat, "work", "turn.end").into_iter().find(|e| e["turn"] == global.as_str()).unwrap();
        assert_eq!(end["outcome"], "error", "{end}");
        assert!(end["error"].as_str().is_some_and(|e| e.contains("this chat's session alone")), "{end}");
    });
    assert!(!c.logs().lines().any(|l| l.contains("\"relay.inbound\"") && l.contains(&global)), "never handed to Hermes");
}

/// Whether the turn `t` of `chat` has ended.
fn ended_turn<'a>(chat: &'a str, t: &'a str) -> impl Fn(&support::fake::World) -> bool + 'a {
    move |w| w.bodies(chat, "work", "turn.end").iter().any(|e| e["turn"] == t)
}

/// Match the full lane's short narration and instant command: one final
/// reply, with pre-tool words retaining their non-final semantics.
#[tokio::test]
#[ignore = "needs Docker"]
async fn short_narration_keeps_its_interim_semantics() {
    let (fake, _model, chat, c) = hermes_running().await;
    let ended = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn);
    for _ in 0..6 {
        let turn = asked(&fake, &chat, &c, "brief narration\nnarrate: echo narrated-ran", ended).await;
        let replies = fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == turn).collect::<Vec<_>>());
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert_eq!(replies[0], json!({"turn":turn,"text":"Let me check that.","interim":true}));
        assert!(replies[1].get("interim").is_none(), "{replies:?}");
        assert!(replies[1]["text"].as_str().unwrap().contains("narrated-ran"));
    }
}

/// Goal (the finite-mono parity audit, 2026-10-09: an image an agent made
/// with FAL's image generation showed as a bare link): real Hermes sends a
/// reply's image link by its URL (`send_media`), and the bridge fetches it:
/// the reply carries the image as the chat's blob, its bytes the
/// provider's, and no link as text. Method: the scripted model answers
/// `image: <url>` with that image as markdown, as a model whose tool made
/// one does, and its own server is the provider's CDN; it is on this host,
/// a local address, so the bridge is told it may fetch from one
/// (`BRIDGE_MEDIA_LOCAL=allow`, a test's: refused otherwise, tests/relay.rs).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn an_image_sent_by_url_is_the_replys_file() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[("BRIDGE_MEDIA_LOCAL", "allow")]);
    within(&fake, &chat, &c, 180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let url = format!("http://model.fragment.internal:{}/generated/cat.png", model.addr.port());
    let ended = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn);
    let turn = asked(&fake, &chat, &c, &format!("image: {url}"), ended).await;
    fake.with(|w| {
        let said: Vec<serde_json::Value> = w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == turn).collect();
        eprintln!("image: the turn said {said:?}");
        let file = said.iter().find(|r| r.get("attachments").is_some()).unwrap_or_else(|| panic!("a reply with the image: {said:?}"));
        assert_eq!(file["attachments"][0]["type"], "image/png", "{file}");
        let sha = file["attachments"][0]["sha256"].as_str().unwrap_or("");
        assert_eq!(w.fragments[&chat].blobs.get(sha).map(|(_, b)| b.to_vec()), Some(support::model::IMAGE.to_vec()), "the chat's blob is the provider's image");
        assert!(!said.iter().any(|r| r["text"].as_str().is_some_and(|t| t.contains(&url))), "no link as text: {said:?}");
    });
}

/// Goal (Skyler's agent on production, 2026-10-26: its routine every 15
/// minutes, which had nothing to tell, posted "⚠️ The model returned only a
/// silence marker for a message that needed a reply" at each fire): on the
/// real Hermes, a routine whose model answers exactly `[SILENT]` says
/// nothing in its chat (no reply, no notice) and ends idle, as Hermes' own
/// cron delivers nothing for it. A person's message answered so gets
/// Hermes' warning, its reply, in the words the bridge knows it by
/// (`wire::UNEXPECTED_SILENCE`: a Hermes that says it otherwise fails here);
/// a routine with something to say says it. Method: the scripted model
/// answers `[SILENT]` to a message telling it to end its turn with exactly
/// that.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_routine_may_say_nothing() {
    let (fake, _model, chat, c) = hermes_running().await;
    let routine = |text: &str| {
        let fired = fake.with(|w| w.append("juniper--k3x9", "tasks", "npub1juniper", json!({ "kind": "routine", "text": text, "chat": chat })));
        fragment_bridge::records::turn_id("juniper--k3x9", "juniper--k3x9", "tasks", fired["seq"].as_u64().unwrap())
    };
    let said = |w: &support::fake::World, turn: &str| w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == turn).filter_map(|r| r["text"].as_str().map(str::to_string)).collect::<Vec<_>>();
    let end = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.end").into_iter().find(|e| e["turn"] == turn);

    let t0 = Instant::now();
    let quiet = routine("check the shelf; if it has not changed, end this turn with exactly [SILENT]");
    within(&fake, &chat, &c, 180_000, "the silent routine's end", |w| end(w, &quiet).is_some()).await;
    eprintln!("silent: a routine with nothing to tell ended in {} ms", t0.elapsed().as_millis());
    let theirs = asked(&fake, &chat, &c, "end this turn with exactly [SILENT]", |w, turn| end(w, turn).is_some()).await;
    let tells = routine("water the plants");
    within(&fake, &chat, &c, 180_000, "the routine's reply", |w| end(w, &tells).is_some()).await;
    // what Hermes would say after a turn's end (a notice, a late reply), said by now
    tokio::time::sleep(Duration::from_secs(5)).await;
    let detail = told(&fake, &chat, &c);
    fake.with(|w| {
        assert_eq!(end(w, &quiet).unwrap()["outcome"], "idle", "{detail}");
        assert!(said(w, &quiet).is_empty(), "the silent routine says nothing: {detail}");
        assert!(!w.bodies(&chat, "work", "turn.notice").iter().any(|n| n["turn"] == quiet.as_str()), "nor a notice: {detail}");
        assert_eq!(said(w, &theirs), vec![fragment_bridge::runtime::relay::wire::UNEXPECTED_SILENCE.to_string()], "a person's message answered with silence gets Hermes' warning, in these words: {detail}");
        assert_eq!(end(w, &theirs).unwrap()["outcome"], "idle");
        let reply = said(w, &tells);
        assert!(reply.len() == 1 && reply[0].starts_with("scripted:") && reply[0].contains("water the plants"), "the routine with something to tell says it: {detail}");
    });
    let logs = c.logs();
    let ops: Vec<&str> = logs.lines().filter(|l| l.contains(&quiet) && (l.contains("\"relay.op\"") || l.contains("\"relay.silent\""))).collect();
    eprintln!("silent: what Hermes sent for the routine {ops:#?}");
    assert!(ops.iter().any(|l| l.contains("\"relay.silent\"")), "the bridge said why it said nothing: {ops:?}");
}

/// Goal (the finite-mono parity audit, 2026-10-09: a question in words left
/// the page saying "Juniper is working"): real Hermes' open clarify (no
/// choices: `❓ <question>`) is the turn's reply, then a `turn.asked` naming
/// its asker, once; the asker's next message is its answer, and the turn's
/// reply after quotes it.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_question_in_words_says_whose_answer_it_waits_for() {
    let (fake, _model, chat, c) = hermes_running().await;
    let said = fake.say(&chat, &person("paul"), json!({ "text": "clarify: What should I call the plant?" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    let asked = |w: &support::fake::World| w.bodies(&chat, "work", "turn.asked").into_iter().filter(|a| a["turn"] == turn).collect::<Vec<_>>();
    within(&fake, &chat, &c, 120_000, "the question, and whose answer it waits for", |w| !asked(w).is_empty()).await;
    fake.with(|w| {
        assert_eq!(asked(w), vec![json!({ "kind": "turn.asked", "turn": turn, "asks": person("paul") })]);
        let replies: Vec<String> = w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == turn).map(|r| r["text"].as_str().unwrap_or("").to_string()).collect();
        assert!(replies.len() == 1 && replies[0].starts_with("What should I call the plant?"), "the question is its reply, before: {replies:?}");
    });
    fake.say(&chat, &person("paul"), json!({ "text": "Fernando" }));
    within(&fake, &chat, &c, 120_000, "the turn's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn)).await;
    fake.with(|w| {
        let replies: Vec<String> = w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == turn).map(|r| r["text"].as_str().unwrap_or("").to_string()).collect();
        assert!(replies.last().is_some_and(|r| r.contains("Fernando")), "the answer reached the clarify: {replies:?}");
        assert_eq!(asked(w).len(), 1, "asked once");
        let started = w.bodies(&chat, "work", "turn.start").len();
        assert_eq!(started, 2, "the answer started no turn of its own (hello, then this one)");
    });
}

/// Goal (the finite-mono parity audit, 2026-10-09: a step read as its raw
/// tool name and its arguments cut at 40 characters): real Hermes' steps
/// say what they do. Its terminal, write_file and execute_code calls are
/// each their tool, their category, and Hermes' own preview of the call
/// whole up to the bridge's 140 (the image's plugin asks for that), not
/// Hermes' 40.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn steps_say_what_they_do() {
    let (fake, _model, chat, c) = hermes_running().await;
    let steps = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.step").into_iter().filter(|s| s["turn"] == turn).map(|s| (s["tool"].as_str().unwrap_or("").to_string(), s["category"].as_str().unwrap_or("").to_string(), s["args"].as_str().unwrap_or("").to_string())).collect::<Vec<_>>();
    let ended = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn);
    // each past Hermes' default preview of 40
    let command = "echo a-command-that-runs-well-past-forty-characters-0123456789";
    let path = "/data/work/juniper--k3x9/a-file-whose-path-runs-past-forty-characters.txt";
    let code = "print('a script whose one line runs well past forty characters')";
    for (said, want) in [(format!("run: {command}"), ("terminal", "shell", command)), (format!("write: {path}"), ("write_file", "write", path)), (format!("code: {code}"), ("execute_code", "shell", code))] {
        let turn = asked(&fake, &chat, &c, &said, ended).await;
        let got = fake.with(|w| steps(w, &turn));
        eprintln!("steps: {said:?} is {got:?}");
        assert!(got.iter().any(|(tool, category, args)| (tool.as_str(), category.as_str(), args.as_str()) == want), "{said:?}: a step {want:?}, its preview whole; got {got:?}");
    }
}

/// Goal (Paul on production, 2026-10-24: an agent's words mid-turn were
/// cut to a step's 300 characters and folded into its steps, "I can't read
/// it all"; everything the agent says is a reply in the chat): real
/// Hermes' words before a tool call, said in the response that calls it,
/// are a reply of their own, whole, posted before the step and marked
/// `interim`; the step carries no words; the turn's answer is a reply after
/// it. Two full replies, one step. Method: the scripted model says a long
/// answer before its instant terminal call (`narrate:`). The companion
/// short-narration test also covers a result overtaking the first Relay send.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn words_before_a_tool_call_are_a_reply() {
    let (fake, _model, chat, c) = hermes_running().await;
    let ended = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn);
    let turn = asked(&fake, &chat, &c, "narrate: echo narrated-ran", ended).await;
    fake.with(|w| {
        let replies: Vec<serde_json::Value> = w.records(&chat, "chat").into_iter().filter(|r| r["body"]["turn"] == turn).collect();
        let steps: Vec<serde_json::Value> = w.records(&chat, "work").into_iter().filter(|r| r["body"]["turn"] == turn && r["body"]["kind"] == "turn.step").collect();
        let said: Vec<serde_json::Value> = replies.iter().map(|r| r["body"].clone()).collect();
        eprintln!("words: replies {said:?}; steps {:?}", steps.iter().map(|s| s["body"].clone()).collect::<Vec<_>>());
        assert_eq!(said.len(), 2, "its words and its answer: {said:?}");
        let words = support::model::narrated();
        assert!(words.chars().count() > 1000);
        assert_eq!(said[0], json!({ "text": words, "turn": turn, "interim": true }), "the words before the call, whole (never cut to 300), interim");
        assert!(said[1]["text"].as_str().is_some_and(|t| t.contains("narrated-ran")) && said[1].get("interim").is_none(), "the answer after: {said:?}");
        assert_eq!(steps.len(), 1, "one step: {steps:?}");
        assert_eq!((&steps[0]["body"]["tool"], &steps[0]["body"]["category"]), (&json!("terminal"), &json!("shell")));
        assert!(steps[0]["body"].get("text").is_none(), "the step carries no words: {steps:?}");
        let at = |r: &serde_json::Value| r["at"].as_i64().expect("a record's time");
        assert!(at(&replies[0]) < at(&steps[0]) && at(&steps[0]) < at(&replies[1]), "the words, the step, then the answer");
    });
}

/// Goal (the finite-mono parity audit, 2026-10-09: Hermes' notices read as
/// steps, `💾 Self-improvement review: Memory updated` one named
/// "Self-improvement"; Paul keeps these): real Hermes' review of its memory,
/// which it runs after every tenth turn of a session (its
/// `memory.nudge_interval`) and announces once the turn is done, is a
/// `turn.notice` of the turn it reviewed (`memory`), never a step nor a
/// reply. Nor is its review's memory call a step (the review of #664: it
/// was, when it ended before its turn did): Hermes runs the review as a
/// fork sharing the turn's session and context, whose calls the image's
/// plugin tells from the turn's own. Method: short turns until one is
/// reviewed, the scripted model saving one thing when asked to review; each
/// answer shows an image its provider serves slowly, so its turn ends only
/// after the review saved (its notice comes first).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_memory_review_is_a_notice() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[("BRIDGE_MEDIA_LOCAL", "allow")]);
    within(&fake, &chat, &c, 180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let ended = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn);
    let reviewed = |w: &support::fake::World| w.bodies(&chat, "work", "turn.notice").into_iter().find(|n| n["category"] == "memory");
    let reviews = || model.calls.lock().unwrap().iter().filter(|call| content_text(call.body["messages"].as_array().and_then(|m| m.last()).unwrap_or(&json!(null))).starts_with("Review the conversation")).count();
    let mut turns = Vec::new();
    // bounded: the nudge's ten turns, and a few more. Once the model is
    // asked to review, nothing more is said: Hermes cancels a review a live
    // turn of its session meets (`cancel_background_review_for_live_turn`)
    for n in 0..14 {
        let image = format!("http://model.fragment.internal:{}/generated/slow-{n}.png", model.addr.port());
        turns.push(asked(&fake, &chat, &c, &format!("remember this: fact number {n}\nimage: {image}"), ended).await);
        let t = Instant::now();
        // bounded: four seconds
        while reviews() == 0 && t.elapsed() < Duration::from_secs(4) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if reviews() > 0 {
            break;
        }
    }
    if tokio::time::timeout(Duration::from_secs(60), fake.until(120_000, "the review's notice", |w| reviewed(w).is_some())).await.is_err() {
        let logs = c.exec_out(&["sh", "-c", "grep -rhi 'review' /data/hermes/logs /data/hermes/profiles/juniper--k3x9/logs 2>/dev/null | tail -30"]);
        panic!("no review's notice after {} turns; the model was asked to review {} times; Hermes logged:\n{logs}\n{}", turns.len(), reviews(), told(&fake, &chat, &c));
    }
    fake.with(|w| {
        let n = reviewed(w).unwrap();
        eprintln!("review: {n} after {} turns; work: {:?}", turns.len(), w.records(&chat, "work").iter().rev().take(6).map(|r| r["body"]["kind"].clone()).collect::<Vec<_>>());
        assert_eq!(n["text"], "Self-improvement review: Memory updated", "{n}");
        assert!(turns.iter().any(|t| n["turn"] == t.as_str()), "the notice is a turn's it reviewed: {n}");
        let work: Vec<serde_json::Value> = w.records(&chat, "work").into_iter().map(|r| r["body"].clone()).collect();
        let at = |kind: &str| work.iter().position(|b| b["kind"] == kind && b["turn"] == n["turn"]);
        assert!(matches!((at("turn.notice"), at("turn.end")), (Some(notice), Some(end)) if notice < end), "the review saved while its turn was still running: {work:?}");
        let steps = w.bodies(&chat, "work", "turn.step");
        assert!(steps.is_empty(), "no step: its turns call nothing, and the review's call is none of theirs: {steps:?}");
        let said = w.bodies(&chat, "chat", "reply");
        assert!(!said.iter().any(|r| r["text"].as_str().is_some_and(|t| t.contains("Self-improvement"))), "no reply: {said:?}");
    });
}

// ---- a card nobody answers (Paul on p5, 2026-10-05: "I missed the 1hr
// window and now it's not responding to chats") ----

/// How long Hermes waits on a card in the expiry tests, and with it the
/// card's life (`HERMES_BOOT_CARD_TIMEOUT_S`: the image gives the bridge the
/// same).
const CARD_S: u64 = 20;

/// A question Hermes asks as a card, with choices (its `clarify`): what a
/// card is now that nothing technical asks.
const CARD_ASKED: &str = "clarify: Which pot should the basil go in? | clay | plastic";

/// What the chat and its `work` hold, and the container's last lines: a
/// failure's detail.
fn told(fake: &Fake, chat: &str, c: &Container) -> String {
    let records = fake.with(|w| format!("chat: {:?}\nwork: {:?}", w.records(chat, "chat").iter().map(|r| r["body"].clone()).collect::<Vec<_>>(), w.records(chat, "work").iter().map(|r| r["body"].clone()).collect::<Vec<_>>()));
    let logs = c.logs();
    let _ = std::fs::write(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("expired-{}.log", &c.id[..12.min(c.id.len())])), &logs);
    format!("the fake holds\n{records}\nthe container said:\n{}", logs.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"))
}

/// Waits up to `ms` for `cond`, failing with what the chat and the
/// container say.
async fn within(fake: &Fake, chat: &str, c: &Container, ms: u64, what: &str, cond: impl Fn(&support::fake::World) -> bool) {
    if tokio::time::timeout(Duration::from_millis(ms), fake.until(ms + 60_000, what, &cond)).await.is_err() {
        panic!("waited {ms} ms for {what}; {}", told(fake, chat, c));
    }
}

/// A chat with Hermes on the line: the image with a short card, its
/// agent's first reply given.
struct Expiry {
    fake: Fake,
    model: Model,
    chat: String,
}

impl Expiry {
    async fn start() -> (Expiry, Container) {
        build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
        let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
        let model = Model::start("0.0.0.0:0").await;
        let chat = fake.chat("talk", &["juniper"]);
        let x = Expiry { fake, model, chat };
        let c = x.container(&[]);
        within(&x.fake, &x.chat, &c, 120_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
        let first = x.say("hello");
        within(&x.fake, &x.chat, &c, 180_000, "Hermes' first reply", |w| !x.ends(w, &first).is_empty()).await;
        (x, c)
    }

    fn container(&self, extra: &[(&str, &str)]) -> Container {
        let card = CARD_S.to_string();
        let mut env = vec![("HERMES_BOOT_CARD_TIMEOUT_S", card.as_str())];
        env.extend_from_slice(extra);
        Container::run(&hermes_tag(), self.fake.addr.port(), self.model.addr.port(), &env)
    }

    /// Paul says `text`: its turn.
    fn say(&self, text: &str) -> String {
        let said = self.fake.say(&self.chat, &person("paul"), json!({ "text": text }));
        fragment_bridge::records::turn_id("juniper--k3x9", &self.chat, "chat", said["seq"].as_u64().unwrap())
    }

    fn ends(&self, w: &support::fake::World, turn: &str) -> Vec<serde_json::Value> {
        w.bodies(&self.chat, "work", "turn.end").into_iter().filter(|e| e["turn"] == turn).collect()
    }

    fn cards(&self, w: &support::fake::World, turn: &str) -> Vec<serde_json::Value> {
        w.bodies(&self.chat, "work", "turn.prompt").into_iter().filter(|p| p["turn"] == turn).collect()
    }

    fn closed(&self, w: &support::fake::World, turn: &str) -> Vec<serde_json::Value> {
        w.bodies(&self.chat, "work", "turn.prompt.closed").into_iter().filter(|p| p["turn"] == turn).collect()
    }

    fn reply(&self, w: &support::fake::World, turn: &str) -> Option<serde_json::Value> {
        w.bodies(&self.chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn)
    }

    /// The model's calls since `from`: each one's last few messages (role,
    /// and the start of its text or its tool calls).
    fn model_saw(&self, from: usize) -> Vec<Vec<String>> {
        let calls = self.model.calls.lock().unwrap();
        calls[from.min(calls.len())..]
            .iter()
            .filter(|c| c.path.ends_with("/chat/completions"))
            .map(|c| {
                let messages = c.body["messages"].as_array().cloned().unwrap_or_default();
                messages[messages.len().saturating_sub(5)..].iter().map(|m| format!("{}: {} {}", m["role"], m["content"].to_string().chars().take(140).collect::<String>(), m["tool_calls"].to_string().chars().take(140).collect::<String>())).collect()
            })
            .collect()
    }
}


/// Goal (Paul on p5, 2026-10-05: "the agent asked me for permission for
/// something but I missed the 1hr window and now it's not responding to
/// chats"), with real Hermes: a card nobody answers is closed `expired` once
/// its `expiresAt` passes, its turn ends once, as Hermes ends it (its wait
/// on the card times out with it), and the chat's next message is claimed
/// and answered, not met by the card asked again; and its owner's message
/// said while a card that takes words is open answers it, its turn going on
/// with the words (Paul on production, 2026-10-22).
///
/// Method: the image with a 20 s card (`HERMES_BOOT_CARD_TIMEOUT_S`: Hermes'
/// clarify waits as long as the bridge's card lives), the scripted model
/// asking a question with choices (`CARD_ASKED`, a card), and the Computer
/// DO's idle rule in a test's time (crates/core/src/computer.rs, `IDLE_MS`:
/// a computer with no keepalive open sleeps 20 minutes after its last
/// record; here, at once): if the bridge lets its keepalive go under the
/// card, the computer is put to sleep as the DO does it (the hold, a save of
/// `/data`, SIGTERM) and woken from that save by the next message, after
/// the card's `expiresAt`; otherwise it stays awake through the expiry.
/// Before the keepalive was held under a card it was let go (decision 42),
/// and the woken Hermes folded the next message into the cut request (its
/// model was given `[paul] do the risky thing\n\n[paul] good morning`), so
/// the next message asked the cut card again.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn an_expired_card_ends_its_turn() {
    let (x, c) = Expiry::start().await;
    let (fake, chat) = (&x.fake, x.chat.as_str());

    // 1. a card nobody answers; then the next message
    let risky = x.say(CARD_ASKED);
    within(fake, chat, &c, 120_000, "the card", |w| !x.cards(w, &risky).is_empty()).await;
    let asked = fake.with(|w| x.cards(w, &risky)[0].clone());
    let expires_at = asked["expiresAt"].as_u64().unwrap();
    assert!(expires_at.saturating_sub(fragment_bridge::log::now_ms()) <= CARD_S * 1000 + 2_000, "the card lives as long as Hermes waits: {asked}");
    // the DO's idle rule: what the keepalive does while the card waits
    tokio::time::sleep(Duration::from_secs(1)).await;
    let let_go = fake.with(|w| w.keepalive_open == 0);
    let calls_before = x.model.calls.lock().unwrap().len();
    let (c, next) = if let_go {
        let left_out = hold(&c).unwrap_or_else(|| panic!("no answer to the hold; {}", told(fake, chat, &c)));
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("expired");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.join("asleep.tar");
        save_data(&c, &left_out, &save);
        let (took, _) = c.sigterm();
        eprintln!("expired: the keepalive let go under the card; held, saved and stopped ({} ms to exit)", took.as_millis());
        let _ = told(fake, chat, &c);
        drop(c);
        fake.until(30_000, "its sockets closed", |w| w.live_sockets() == 0).await;
        // bounded: the card's life
        while fragment_bridge::log::now_ms() < expires_at + 3_000 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // the next message wakes it from the save
        let next = x.say("good morning");
        let c = x.container(&[("RESTORE_PENDING", "1")]);
        let tar = std::fs::File::open(&save).expect("the save");
        let cp = Command::new(docker()).args(["cp", "-a", "-", &format!("{}:/data", c.id)]).stdin(tar).output().expect("docker runs");
        assert!(cp.status.success(), "the restore: {}", String::from_utf8_lossy(&cp.stderr));
        let (code, said) = c.exec_code(&["/usr/local/bin/computer-check"]);
        assert_eq!(code, 0, "the save checks: {said}");
        assert!(c.exec(&["touch", "/run/computer/restored"]));
        (c, next)
    } else {
        within(fake, chat, &c, CARD_S * 1000 + 60_000, "the card expired, awake, and its turn's end", |w| x.closed(w, &risky).iter().any(|p| p["outcome"] == "expired") && !x.ends(w, &risky).is_empty()).await;
        fake.with(|w| eprintln!("expired, awake: its end {:?}, its reply {:?}", x.ends(w, &risky), x.reply(w, &risky)));
        (c, x.say("good morning"))
    };
    within(fake, chat, &c, 240_000, "the next message's turn to end", |w| !x.ends(w, &next).is_empty()).await;
    let saw = x.model_saw(calls_before);
    fake.with(|w| {
        let asked_again = x.cards(w, &next);
        assert!(asked_again.is_empty(), "the next message asks nothing again (the keepalive let go under the card: {let_go}); the model saw {saw:#?}; it asked {asked_again:?}");
        assert!(x.reply(w, &next).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("good morning")), "the next message is answered: {:?}; the model saw {saw:#?}", x.reply(w, &next));
        assert_eq!((x.ends(w, &risky).len(), x.closed(w, &risky).len()), (1, 1), "the card's turn ends once, its card closed once");
        assert_eq!(x.closed(w, &risky)[0]["outcome"], "expired");
        assert_eq!(x.ends(w, &risky)[0]["outcome"], "idle", "Hermes ended it, its wait timed out with the card");
    });
    assert!(!let_go, "the card holds the computer awake");

    // 2. its owner's message while the card (a clarify's, which takes words)
    // is open answers it (Paul on production, 2026-10-22: his typed answer
    // stayed queued, the card open): the card closes with their words,
    // Hermes' clarify takes them (as "Other" with those words), its turn
    // replies once with them, and the message starts no turn of its own
    let risky2 = x.say(&format!("{CARD_ASKED} | none"));
    within(fake, chat, &c, 120_000, "the second card", |w| !x.cards(w, &risky2).is_empty()).await;
    let meanwhile = x.say("a terracotta one");
    within(fake, chat, &c, 120_000, "the card answered by the message, and its turn's end", |w| x.closed(w, &risky2).iter().any(|p| p["outcome"] == "answered") && !x.ends(w, &risky2).is_empty()).await;
    fake.with(|w| {
        let closed = x.closed(w, &risky2);
        assert_eq!((closed.len(), closed[0]["option"].as_str(), closed[0]["text"].as_str(), closed[0]["by"].as_str()), (1, Some("other"), Some("a terracotta one"), Some("npub1paul")), "closed once, with their words: {closed:?}");
        assert_eq!(x.ends(w, &risky2).len(), 1, "its turn ends once");
        assert!(x.reply(w, &risky2).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("a terracotta one")), "Hermes' clarify had their words: {:?}", x.reply(w, &risky2));
        assert!(x.ends(w, &meanwhile).is_empty() && x.reply(w, &meanwhile).is_none(), "the message started no turn of its own");
    });
}

/// Goal (Paul, 2026-10-14: "hermes asking for approvals for stuff like
/// 'can I make this site public?' would make sense, but for technical
/// command line things it can't be asking"; on p5, 2026-10-09: "asking me
/// for permission for silly things like changing focus in computer use and
/// scrolling"): nothing technical asks the agent's owner, with real Hermes,
/// and no guardian model is called in a person's place; the agent is told,
/// in its system prompt, to ask only about outcomes, in plain words.
///
/// Method: the image with a 20 s card, so a step that asked would end its
/// turn `BLOCKED` within it. Each a turn of its own, none a card nor
/// refused: on its own desktop (after a capture that starts it), a focus
/// change raising the window (its `bring_to_front` scope too), then a
/// scroll and a click in the foreground (on X11 Hermes' driver delivers a
/// scroll only so); in its terminal a recursive delete Hermes flags
/// (`risky`: `rm -rf`); an execute_code script; and Hermes' file tools'
/// writes to a project's `AGENTS.md` (a protected instruction file, which
/// Hermes asks about even under `/yolo` unless told otherwise) and to its
/// SSH config.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn nothing_technical_asks_its_owner() {
    let (x, c) = Expiry::start().await;
    let (fake, chat) = (&x.fake, x.chat.as_str());
    let look = x.say("look at your screen");
    within(fake, chat, &c, 240_000, "its look", |w| x.reply(w, &look).is_some()).await;
    let agents_md = "/data/work/juniper--k3x9/garden/AGENTS.md";
    let desk = |args: serde_json::Value| format!("desk: {args}");
    for said in [
        desk(json!({ "action": "focus_app", "app": "xfce4-panel", "raise_window": true })),
        desk(json!({ "action": "scroll", "direction": "down", "amount": 3, "coordinate": [400, 300], "delivery_mode": "foreground" })),
        desk(json!({ "action": "click", "coordinate": [400, 300], "delivery_mode": "foreground" })),
        "do the risky thing".to_string(),
        "code: import shutil; shutil.rmtree('/tmp/fragment-gone', ignore_errors=True); print('code-ran')".to_string(),
        format!("write: {agents_md}"),
        "write: ~/.ssh/config".to_string(),
    ] {
        let turn = x.say(&said);
        within(fake, chat, &c, CARD_S * 1000 + 60_000, &format!("{said} to end"), |w| x.reply(w, &turn).is_some() && !x.ends(w, &turn).is_empty()).await;
        fake.with(|w| {
            let reply = x.reply(w, &turn).unwrap()["text"].as_str().unwrap_or("").to_string();
            eprintln!("nothing technical asks: {said} said {}", reply.chars().take(300).collect::<String>());
            assert!(x.cards(w, &turn).is_empty(), "{said} asks no one: {:?}", x.cards(w, &turn));
            assert!(reply.starts_with("scripted: the tool said:") || reply.contains("the tool ran"), "{said}: the tool ran: {reply}");
            assert!(!reply.contains("BLOCKED") && !reply.contains("requires approval") && !reply.contains("denied"), "{said} is not refused: {reply}");
        });
    }
    assert_eq!(c.exec_out(&["cat", agents_md]), support::model::WRITTEN, "the protected file was written");
    let calls = x.model.calls.lock().unwrap();
    assert!(!calls.iter().any(|m| m.body["messages"].to_string().contains("APPROVE, DENY, or ESCALATE")), "no guardian");
    let told = calls.iter().filter(|m| m.path.ends_with("/chat/completions")).filter(|m| m.body["messages"][0]["role"] == "system").map(|m| m.body["messages"][0]["content"].to_string()).find(|s| s.contains("Never ask them to approve a command, a file, a path or code"));
    assert!(told.as_ref().is_some_and(|s| s.contains("already on your computer") && s.contains("into the file dialog on your desktop with computer_use")), "and that a file attached in the chat is on its computer, and how to upload it (hermes::FILES)");
    assert!(told.as_ref().is_some_and(|s| s.contains("put MEDIA: and its absolute path on a line of its own")), "and how it attaches a file in the chat (hermes::SENDING)");
    assert!(told.is_some_and(|s| s.contains("with the clarify tool and its choices")), "every turn's system prompt says how to ask (the image's platform_hints.relay)");
}

/// Goal (the hermes e2e lane, 2026-10-14: a Stop on a turn waiting on a
/// clarify's card closed the card, but the turn ran on until the clarify's
/// hour was out): with real Hermes, its asker's Stop while a clarify's card
/// of choices waits ends the turn stopped, its card closed `stopped`, no
/// choice made for its owner; the bridge's answer to the clarify (`skip`:
/// Hermes' clarify never reads an interrupt) starts no turn of its own; and
/// the chat's next message is answered.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_stop_lets_its_clarify_go() {
    let (fake, _model, chat, c) = hermes_running().await;
    let turn_of = |said: serde_json::Value| fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    let of = |w: &support::fake::World, turn: &str, kind: &str| w.bodies(&chat, "work", kind).into_iter().filter(|b| b["turn"] == turn).collect::<Vec<_>>();
    let turn = turn_of(fake.say(&chat, &person("paul"), json!({ "text": CARD_ASKED })));
    within(&fake, &chat, &c, 120_000, "the clarify's card", |w| !of(w, &turn, "turn.prompt").is_empty()).await;
    fake.say(&chat, &person("paul"), json!({ "kind": "stop", "turn": turn }));
    // well inside the card's hour
    within(&fake, &chat, &c, 60_000, "the stopped turn's end", |w| !of(w, &turn, "turn.end").is_empty()).await;
    let next = turn_of(fake.say(&chat, &person("paul"), json!({ "text": "good morning" })));
    within(&fake, &chat, &c, 120_000, "the next message's end", |w| !of(w, &next, "turn.end").is_empty()).await;
    fake.with(|w| {
        assert_eq!(of(w, &turn, "turn.end")[0]["outcome"], "stopped");
        assert_eq!(of(w, &turn, "turn.prompt.closed")[0]["outcome"], "stopped");
        let replies = |t: &str| w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == t).map(|r| r["text"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
        // Hermes may answer once more, its model told the question was
        // skipped: never a choice made for the person
        assert!(replies(&turn).iter().all(|r| r.contains("\"status\": \"skipped\"") && !r.contains("\"user_response\": \"")), "no choice made: {:?}", replies(&turn));
        assert!(replies(&next).iter().any(|r| r.contains("good morning")), "{:?}", replies(&next));
        assert_eq!(w.bodies(&chat, "work", "turn.start").len(), 3, "hello, the stopped one and good morning: `skip` is no turn");
    });
}

// ---- a turn a restart cuts, then the next message (P5; F10) ----

/// The text of a model message (its content, or its parts' text).
fn content_text(m: &serde_json::Value) -> String {
    match &m["content"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// Of the first model request whose last user message says `said`: that
/// message's text, and the message before it (its role and text).
fn asked_with(calls: &[support::model::Call], said: &str) -> Option<(String, (String, String))> {
    calls.iter().filter(|c| c.path.ends_with("/chat/completions")).find_map(|c| {
        let messages = c.body["messages"].as_array()?;
        let at = messages.iter().rposition(|m| m["role"] == "user")?;
        let last = content_text(&messages[at]);
        if !last.contains(said) {
            return None;
        }
        let before = at.checked_sub(1).map(|i| (messages[i]["role"].as_str().unwrap_or("").to_string(), content_text(&messages[i]))).unwrap_or_default();
        Some((last, before))
    })
}

/// Goal (P5; F10, with real Hermes v0.21.5): a turn a restart cuts while its
/// card waits (here an owner's sleep: the hold, a save of `/data`, SIGTERM)
/// is never redone by the chat's next message. Woken from that save, the
/// image's boot closes the cut turn in Hermes' session with Hermes' own
/// failed-turn boundary, and the bridge tells the next turn what was cut,
/// from the journal. So the model's request for "good morning" ends with a
/// user message of its own (the note as its channel context, then the
/// message) after an assistant message (the boundary), never joined to the
/// cut request; no request after the wake has the model ask the cut
/// question again, no card is shown again, "good morning" is answered, and
/// the message after it is told nothing.
///
/// Method: the image with a 20 s card, the scripted model asking a question
/// with choices (`CARD_ASKED`, a card), and the DO's owner's sleep under the
/// card, taken as the expiry test above takes an idle one; then a wake from
/// that save. Before P5 (no closing at boot, no note) Hermes joined the
/// next message to the cut request (its model was given `[paul] do the
/// risky thing\n\n[paul] good morning`), the scripted model did the cut
/// request again, and its card was shown again
/// (`FRAGMENT_DOCKER_SKIP_BUILD=1 FRAGMENT_DOCKER_HERMES_TAG=<an image>`
/// runs this against another image).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_turn_cut_by_a_restart_is_closed_and_told() {
    let (x, c) = Expiry::start().await;
    let (fake, chat) = (&x.fake, x.chat.as_str());
    let risky = x.say(CARD_ASKED);
    within(fake, chat, &c, 120_000, "the card", |w| !x.cards(w, &risky).is_empty()).await;
    // the owner's sleep under the card: held, saved, stopped
    let left_out = hold(&c).unwrap_or_else(|| panic!("no answer to the hold; {}", told(fake, chat, &c)));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cut-told");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let save = dir.join("asleep.tar");
    save_data(&c, &left_out, &save);
    let (took, _) = c.sigterm();
    eprintln!("cut: held, saved and stopped under the card ({} ms to exit)", took.as_millis());
    let _ = told(fake, chat, &c);
    drop(c);
    fake.until(30_000, "its sockets closed", |w| w.live_sockets() == 0).await;
    let calls_before = x.model.calls.lock().unwrap().len();

    // the next message wakes it from the save
    let next = x.say("good morning");
    let c = x.container(&[("RESTORE_PENDING", "1")]);
    let tar = std::fs::File::open(&save).expect("the save");
    let cp = Command::new(docker()).args(["cp", "-a", "-", &format!("{}:/data", c.id)]).stdin(tar).output().expect("docker runs");
    assert!(cp.status.success(), "the restore: {}", String::from_utf8_lossy(&cp.stderr));
    let (code, said) = c.exec_code(&["/usr/local/bin/computer-check"]);
    assert_eq!(code, 0, "the save checks: {said}");
    assert!(c.exec(&["touch", "/run/computer/restored"]));
    // its end, or the cut card shown again (before P5: the fold)
    within(fake, chat, &c, 240_000, "the next message's turn to end", |w| !x.ends(w, &next).is_empty() || !x.cards(w, &next).is_empty()).await;
    let again = fake.with(|w| x.cards(w, &next));
    assert!(again.is_empty(), "the cut card is not shown again: {again:?}; the model saw {:#?}", x.model_saw(calls_before));
    let after = x.say("and after that");
    within(fake, chat, &c, 120_000, "the turn after's end", |w| !x.ends(w, &after).is_empty()).await;

    let calls: Vec<support::model::Call> = x.model.calls.lock().unwrap()[calls_before..].to_vec();
    let redone: Vec<String> = calls.iter().filter(|c| c.path.ends_with("/chat/completions")).filter_map(|c| support::model::answer(&c.body).1).map(|call| call.to_string()).filter(|call| call.contains("clarify")).collect();
    let logs = c.logs();
    let closed = logs.lines().find(|l| l.contains("boot.cut_turns_closed") || l.contains("boot.cut_turns_failed")).unwrap_or("no word from the boot's closer").to_string();
    let saw = x.model_saw(calls_before);
    fake.with(|w| {
        assert_eq!(x.ends(w, &risky).len(), 1, "the cut turn ends once");
        assert_eq!(x.ends(w, &risky)[0]["error"], "lost when the computer restarted");
        assert!(redone.is_empty(), "no request after the wake has the model ask the cut question again: {redone:?}; the model saw {saw:#?}");
        assert!(x.cards(w, &next).is_empty() && x.cards(w, &after).is_empty(), "no card shown again: {:?}; the model saw {saw:#?}", x.cards(w, &next));
        assert!(x.reply(w, &next).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("good morning")), "good morning is answered: {:?}; the model saw {saw:#?}", x.reply(w, &next));
    });
    let (asked, (before_role, before)) = asked_with(&calls, "good morning").unwrap_or_else(|| panic!("a model request for good morning; it saw {saw:#?}"));
    assert!(asked.starts_with("[Recent channel messages]\n"), "the note first, nothing joined before it: {asked:?}");
    let answering = format!("It was answering: “{CARD_ASKED}”");
    for said in ["Your previous turn in this chat was cut short: your computer restarted before it finished.", "Check what it already did before you do any of it again", &answering, "[New message]\n[paul] good morning"] {
        assert!(asked.contains(said), "{said:?} in the model's request: {asked:?}");
    }
    assert!(!asked.contains(&format!("[paul] {CARD_ASKED}")), "the cut request is never joined to it: {asked:?}");
    assert_eq!(before_role, "assistant", "the cut turn is closed before it (Hermes' failed-turn boundary; {closed}): {before:?}");
    let (asked_after, _) = asked_with(&calls, "and after that").expect("a request for the message after");
    assert!(!asked_after.contains("cut short"), "the turn after is told nothing: {asked_after:?}");
    eprintln!("cut: {closed}\ncut: the boundary the model saw: {before:?}\ncut: the message: {asked:?}");
}

/// Goal: offered operator placeholders select Hermes' real backends;
/// removing them restores its defaults. Two FAL scopes submit interleaved,
/// then their result workers run outside either scope, retaining each key.
/// Only the vendor HTTP transport is faked, never the SDK or profile writer.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn tool_profiles_select_offered_operator_keys() {
    let (fake, _model, _chat, c) = hermes_running().await;
    let config = || c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/config.yaml"]);
    let absent = config();
    for selection in ["web:", "image_gen:", "tts:", "cloud_provider:"] {
        assert!(!absent.contains(selection), "no offered provider: {absent}");
    }
    let credentials: Vec<serde_json::Value> = [
        ("firecrawl", "FIRECRAWL_API_KEY"), ("fal", "FAL_KEY"),
        ("elevenlabs", "ELEVENLABS_API_KEY"), ("browser-use", "BROWSER_USE_API_KEY"),
        ("perplexity", "PERPLEXITY_API_KEY"), ("xai", "XAI_API_KEY"),
    ].iter().map(|(provider, env)| json!({
        "provider": provider, "kind": "operator", "env": [env],
        "placeholder": format!("fck_{provider}_{}", "a".repeat(32)), "hosts": ["api.test"]
    })).collect();
    fake.with(|w| w.computer["agents"][0]["credentials"] = json!(credentials));
    let t = Instant::now();
    while !config().contains("  cloud_provider: \"local\"") {
        assert!(t.elapsed() < Duration::from_secs(30), "profile did not refresh: {}\n{}", config(), c.logs());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let checked = hermes_python(&c, &["HERMES_HOME=/data/hermes/profiles/juniper--k3x9"], r#"
import json, sys
sys.path.insert(0, '/opt/fragment')
import tool_keys
tool_keys.install()
from hermes_cli.config import load_config
from agent.secret_scope import set_secret_scope, reset_secret_scope
from tools.tool_backend_helpers import read_selection
from tools import image_generation_tool as images
from pm.extras import available
cfg = load_config()
assert cfg['web']['backend'] == cfg['web']['search_backend'] == cfg['web']['extract_backend'] == 'firecrawl'
assert cfg['image_gen']['provider'] == 'fal' and cfg['image_gen']['model'] == 'fal-ai/flux-2/klein/9b'
assert cfg['tts']['provider'] == 'elevenlabs'
assert cfg['browser']['cloud_provider'] == 'local' and cfg['browser']['backend'] == 'off'
assert all(available(e) for e in ('edge-tts', 'firecrawl', 'fal', 'tts-premium'))
import fal_client, httpx
real = fal_client.SyncClient
seen, clients = [], []
def transport(request):
    seen.append(request.headers['Authorization'])
    if request.method == 'POST':
        body = {'request_id': 'test', 'status_url': 'https://queue.fal.run/status',
                'response_url': 'https://queue.fal.run/result', 'cancel_url': 'https://queue.fal.run/cancel'}
    elif request.url.path == '/status':
        body = {'status': 'COMPLETED', 'logs': None, 'metrics': {}}
    else:
        body = {'images': []}
    return httpx.Response(200, json=body)
def client(**kwargs):
    c = real(**kwargs)
    c.__dict__['_client'] = httpx.Client(transport=httpx.MockTransport(transport), headers={'Authorization': c._auth.header_value})
    clients.append(c)
    return c
fal_client.SyncClient = client
handles = []
for letter in ('a', 'b'):
    token = set_secret_scope({'FAL_KEY': 'fck_fal_' + letter * 32})
    try:
        assert read_selection('image_gen') == 'fal'
        handles.append(images._submit_fal_request(cfg['image_gen']['model'], {'prompt': 'test'}))
    finally:
        reset_secret_scope(token)
assert seen == ['Key fck_fal_' + 'a' * 32, 'Key fck_fal_' + 'b' * 32]
for handle in handles:
    assert handle.get() == {'images': []}
assert seen[2:] == ['Key fck_fal_' + 'a' * 32] * 2 + ['Key fck_fal_' + 'b' * 32] * 2
assert all(c._client.is_closed for c in clients)
token = set_secret_scope({})
try:
    try:
        images._submit_fal_request(cfg['image_gen']['model'], {'prompt': 'test'})
        raise AssertionError('missing scoped key was accepted')
    except ValueError:
        pass
finally:
    reset_secret_scope(token)
assert len(seen) == 6
print(json.dumps({'selections': True, 'extras': True, 'scoped_fal': True}))
"#);
    assert_eq!(checked, json!({"selections": true, "extras": true, "scoped_fal": true}));
    fake.with(|w| w.computer["agents"][0]["credentials"] = json!([]));
    let t = Instant::now();
    while config() != absent {
        assert!(t.elapsed() < Duration::from_secs(30), "revoked selections remain: {}", config());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

// ---- Hermes' catalog refresh under the hold (2026-10-07:
// `held_nothing_under_data_changes` failed once, in a parallel run, when the
// refresh landed inside its hold) ----

/// Python in Hermes' own environment, as its gateway runs it (the hermes
/// user, its home), with `env` added: the JSON of its last line.
fn hermes_python(c: &Container, env: &[&str], code: &str) -> serde_json::Value {
    let mut cmd = vec!["/command/s6-setuidgid", "hermes", "env", "HOME=/data/hermes", "HERMES_HOME=/data/hermes"];
    cmd.extend_from_slice(env);
    cmd.extend_from_slice(&["/opt/hermes/.venv/bin/python", "-c", code]);
    let (code, said) = c.exec_code(&cmd);
    assert_eq!(code, 0, "Hermes' Python: {said}");
    let last = said.lines().rev().find(|l| l.starts_with('{')).unwrap_or_else(|| panic!("no JSON from Hermes' Python: {said}"));
    serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {last}"))
}

/// What the gateway's `_model_catalog_refresh_watcher` (Hermes'
/// gateway/run_watchers.py) runs at each tick, 30 s after it starts and
/// every 20 minutes: `refresh_catalogs`, which first refreshes the manifest
/// (`get_catalog(force_refresh=True)`), then OpenRouter's and Nous' lists.
const REFRESH: &str = "import json\nfrom hermes_cli import model_catalog as m\nenabled = m._load_catalog_config()['enabled']\nrefreshed = m.refresh_catalogs()\nprint(json.dumps({'enabled': enabled, 'refreshed': refreshed, 'catalog': m.get_catalog(force_refresh=True)}))";

/// Goal: Hermes' model-catalog refresh, which wrote four caches in its home
/// whenever its timer landed inside a hold (2026-10-07), writes nothing in
/// our image, held or not: `model_catalog.enabled: false` in the managed
/// overlay (docs/durable-computers.md, "What changes under the hold"); and a
/// wake from a save whose catalog caches are torn answers as any other, so
/// a torn cache never breaks Hermes on wake.
/// Method: Hermes held; the watcher's own refresh forced inside the hold,
/// as the gateway runs it, with the image's overlay: it reads the catalog
/// off, fetches and writes nothing, and nothing the save keeps changes but
/// what Hermes' timers rewrite whole (`kept_hot`). Then the same manifest
/// refresh with the catalog on (a managed overlay of this test's, the
/// manifest a local file, so no network): the write the failure saw, a
/// cache file the save keeps, rewritten under the hold. Then each of the
/// four torn as an older save could carry them (invalid UTF-8 cut short,
/// JSON cut short, empty, not JSON, and a temp file caught mid-write), the
/// save taken, the computer stopped and woken from it as the platform does
/// (`RESTORE_PENDING`, its check, the gate), and a message answered.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_catalog_refresh_forced_under_the_hold_writes_nothing() {
    let (x, c) = Expiry::start().await;
    let (fake, chat) = (&x.fake, x.chat.as_str());
    // held once Hermes' start-up writes are done: its kanban dispatcher
    // makes its board's database some seconds after the gateway starts, and
    // until its first page is written (a WAL database's file starts empty)
    // it is no database the hold copies, so its `-wal` and `-shm` would
    // change inside the hold, named by nothing (seen under load, 2026-10-08)
    let t = Instant::now();
    // bounded: two minutes
    while c.exec_out(&["head", "-c", "15", "/data/hermes/kanban.db"]) != "SQLite format 3" {
        assert!(t.elapsed() < Duration::from_secs(120), "Hermes never wrote its kanban board's database");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("catalog: Hermes' kanban board written {} ms after its first reply", t.elapsed().as_millis());
    let left_out = hold(&c).unwrap_or_else(|| panic!("no answer to the hold; {}", told(fake, chat, &c)));
    let before = files(&c, &left_out);
    let forced = hermes_python(&c, &[], REFRESH);
    let after = files(&c, &left_out);
    let changed_off = changed(&before, &after);
    eprintln!("catalog: forced under the hold, off: {forced}; changed: {changed_off:?}");
    assert_eq!(forced["enabled"], false, "Hermes reads its catalog off, from the image's overlay: {forced}");
    assert_eq!(forced["refreshed"], false, "the refresh fetched nothing: {forced}");
    assert_eq!(forced["catalog"], json!({}), "and has no catalog: {forced}");
    assert!(unexplained(&changed_off).is_empty(), "nothing the save keeps changed but what Hermes' timers rewrite whole: {:?}", unexplained(&changed_off));
    for cache in CATALOG_CACHES {
        assert!(!c.exec(&["test", "-e", &format!("/data/hermes/cache/{cache}")]), "no {cache}");
    }

    // what the failure saw: the catalog on, its manifest a local file
    let manifest = json!({ "version": 1, "providers": { "openrouter": { "models": [{ "id": "fragment/test", "description": "a test's" }] } } });
    let overlay = "model_catalog:\n  enabled: true\n  url: \"file:///tmp/catalog-on/model-catalog.json\"\n";
    let setup = "mkdir -p /tmp/catalog-on && printf '%s' \"$1\" > /tmp/catalog-on/config.yaml && printf '%s' \"$2\" > /tmp/catalog-on/model-catalog.json && chmod -R a+rX /tmp/catalog-on";
    assert!(c.exec(&["sh", "-c", setup, "sh", overlay, &manifest.to_string()]), "the test's overlay");
    let before = files(&c, &left_out);
    let on = hermes_python(&c, &["HERMES_MANAGED_DIR=/tmp/catalog-on"], "import json\nfrom hermes_cli import model_catalog as m\nprint(json.dumps({'enabled': m._load_catalog_config()['enabled'], 'catalog': m.get_catalog(force_refresh=True)}))");
    let after = files(&c, &left_out);
    let changed_on = changed(&before, &after);
    eprintln!("catalog: forced under the hold, on: {on}; changed: {changed_on:?}");
    assert_eq!(on["enabled"], true, "{on}");
    assert_eq!(on["catalog"], manifest, "the manifest fetched: {on}");
    assert_eq!(unexplained(&changed_on), vec!["/data/hermes/cache/model_catalog.json"], "on, the refresh rewrites a cache the save keeps, under the hold");
    whole(&c, &changed_on);

    // the four torn, as an older save could carry them; then a sleep and a wake
    let tear = "cd /data/hermes/cache && printf '{\"version\": 1, \"providers\": {\"openrouter\": {\"mo\\377\\376' > model_catalog.json && printf '{\"fetched_at\": 17' > openrouter_curated_catalog.json && : > nous_recommended_cache.json && printf 'not json' > reasoning_caps.json && printf '{\"version\": 1, \"prov' > .model_catalog_torn.tmp";
    let (code, said) = c.exec_code(&["/command/s6-setuidgid", "hermes", "sh", "-c", tear]);
    assert_eq!(code, 0, "the caches torn: {said}");
    let torn = c.exec_out(&["sh", "-c", "cd /data/hermes/cache && sha256sum model_catalog.json openrouter_curated_catalog.json nous_recommended_cache.json reasoning_caps.json .model_catalog_torn.tmp"]);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("catalog-torn");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let save = dir.join("asleep.tar");
    save_data(&c, &left_out, &save);
    let (took, _) = c.sigterm();
    eprintln!("catalog: held, saved with torn caches, stopped ({} ms to exit)", took.as_millis());
    drop(c);
    fake.until(30_000, "its sockets closed", |w| w.live_sockets() == 0).await;
    let next = x.say("good morning");
    let c = x.container(&[("RESTORE_PENDING", "1")]);
    let tar = std::fs::File::open(&save).expect("the save");
    let cp = Command::new(docker()).args(["cp", "-a", "-", &format!("{}:/data", c.id)]).stdin(tar).output().expect("docker runs");
    assert!(cp.status.success(), "the restore: {}", String::from_utf8_lossy(&cp.stderr));
    let (code, said) = c.exec_code(&["/usr/local/bin/computer-check"]);
    assert_eq!(code, 0, "the save checks: {said}");
    assert_eq!(c.exec_out(&["sh", "-c", "cd /data/hermes/cache && sha256sum model_catalog.json openrouter_curated_catalog.json nous_recommended_cache.json reasoning_caps.json .model_catalog_torn.tmp"]), torn, "the wake has the torn caches");
    assert!(c.exec(&["touch", "/run/computer/restored"]));
    within(fake, chat, &c, 240_000, "the message after the wake answered", |w| x.reply(w, &next).is_some()).await;
    let woke = hermes_python(&c, &[], REFRESH);
    assert_eq!(woke["catalog"], json!({}), "woken, its catalog is still off, the torn caches unread: {woke}");
    fake.with(|w| assert!(x.reply(w, &next).is_some_and(|r| r["text"].as_str().unwrap_or("").contains("good morning")), "good morning is answered: {:?}", x.reply(w, &next)));
}

// ---- nothing installed at run time (Paul, 2026-10-07) ----

/// Goal: Hermes installs nothing at run time. Its lazy installs are off, as
/// Hermes itself reads them, both in the gateway's own scope and in an
/// agent's profile (upstream's image turns them on, into its home's
/// `installs`). Its `text_to_speech` is still offered, Edge's SDK being in
/// the image (Hermes' image leaves it to a first-use install). A feature
/// asked for at run time (local Whisper, as a voice note's local fallback
/// would) is refused. And nothing an install leaves is under `/data`.
/// Method: Hermes' own PM (`lazy_installs_allowed`, `extras.available`,
/// `ensure_import`, as its tools call it) and `check_tts_requirements`, run
/// as its gateway runs them, in each scope; then `/data` searched.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn nothing_is_installed_at_run_time() {
    let (_fake, _model, _chat, c) = hermes_running().await;
    const PROBE: &str = "import json, os\nos.chdir('/opt/hermes')\nfrom pm import lazy_installs_allowed, ensure_import\nfrom pm.extras import available\nfrom tools.tts_tool import check_tts_requirements\ntry:\n    ensure_import('stt-whisper')\n    forced = 'installed'\nexcept Exception as e:\n    forced = str(e)\nprint(json.dumps({'allow': lazy_installs_allowed(), 'edge': available('edge-tts'), 'whisper': available('stt-whisper'), 'speaks': check_tts_requirements(), 'forced': forced}))";
    for home in ["/data/hermes", "/data/hermes/profiles/juniper--k3x9"] {
        let seen = hermes_python(&c, &[&format!("HERMES_HOME={home}")], PROBE);
        eprintln!("lazy: {home}: {seen}");
        assert_eq!(seen["allow"], json!(false), "lazy installs are off in {home}: {seen}");
        assert_eq!(seen["edge"], json!(true), "Edge's SDK is in the image: {seen}");
        assert_eq!(seen["speaks"], json!(true), "text_to_speech is offered in {home}: {seen}");
        assert_eq!(seen["whisper"], json!(false), "local Whisper is not: {seen}");
        assert!(seen["forced"].as_str().is_some_and(|f| f.contains("lazy installs are disabled")), "an install asked for at run time is refused in {home}: {seen}");
    }
    // no package, generation or installer cache of an install under /data
    let left = c.exec_out(&["sh", "-c", "find /data -maxdepth 7 \\( -path '*/lazy-packages/*' -o -path '*/installs/*/site-packages' -o -path '*/.cache/uv' -o -iname 'edge_tts*' -o -iname '*faster_whisper*' \\) | head -5"]);
    assert_eq!(left.trim(), "", "no install left anything under /data");
}

// ---- a voice memo (decision 9: a voice memo is one the agent transcribes
// itself, through the platform's model route) ----

/// Goal (decision 9; Paul, 2026-10-07): a voice note a person attaches in
/// the chat reaches Hermes, which transcribes it through the model route's
/// `whisper` (OpenAI's transcription shape, its agent named by its key,
/// `agent:<name>`, since Hermes' STT client sends no header of ours; no
/// language forced, so Whisper detects it), and answers once, having heard
/// it: no transcript echoed as a message of its own, and no local Whisper
/// installed for it. Method: a memo (a WAV that says its words, as the
/// scripted model reads them) put in the chat's blobs and said as an
/// attachment; the scripted model's calls, the turn's reply, and `/data`.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_voice_memo_is_transcribed_through_the_route() {
    use sha2::Digest;
    let (fake, model, chat, c) = hermes_running().await;
    let words = "please remember the blue door";
    let audio = support::model::memo(words);
    let sha = fragment_bridge::records::hex(&sha2::Sha256::digest(&audio));
    fake.with(|w| w.fragments.get_mut(&chat).expect("the chat").blobs.insert(sha.clone(), ("audio/wav".into(), bytes::Bytes::from(audio.clone()))));
    let calls_before = model.calls.lock().unwrap().len();
    let said = fake.say(&chat, &person("paul"), json!({ "attachments": [{ "sha256": sha, "size": audio.len(), "type": "audio/wav", "name": "memo.wav" }] }));
    let t = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    fake.until(180_000, "the memo's turn to end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t)).await;
    let calls: Vec<support::model::Call> = model.calls.lock().unwrap()[calls_before..].to_vec();
    let heard: Vec<&support::model::Call> = calls.iter().filter(|c| c.path.ends_with("/audio/transcriptions")).collect();
    eprintln!("memo: transcriptions {:?}", heard.iter().map(|c| (&c.body, &c.authorization)).collect::<Vec<_>>());
    assert_eq!(heard.len(), 1, "one transcription through the route: {:?}", calls.iter().map(|c| &c.path).collect::<Vec<_>>());
    let h = heard[0];
    assert_eq!(h.authorization.as_deref(), Some("Bearer agent:juniper--k3x9"), "its key names its agent");
    assert_eq!((h.body["model"].clone(), h.body["language"].clone(), h.body["audio_bytes"].clone()), (json!("whisper"), serde_json::Value::Null, json!(audio.len())), "the route's whisper, the memo whole, no language forced");
    let asked = calls.iter().filter(|c| c.path.ends_with("/chat/completions")).any(|c| c.body["messages"].to_string().contains(words));
    assert!(asked, "the turn's model call carries what the memo said: {:#?}", calls.iter().map(|c| c.body["messages"].to_string().chars().take(400).collect::<String>()).collect::<Vec<_>>());
    let replies = fake.with(|w| w.bodies(&chat, "chat", "reply").into_iter().filter(|r| r["turn"] == t).collect::<Vec<_>>());
    eprintln!("memo: replies {replies:?}");
    assert_eq!(replies.len(), 1, "one reply, no transcript echoed as a message of its own: {replies:?}");
    assert!(!replies[0]["text"].as_str().unwrap_or("").contains('🎙'), "{replies:?}");
    assert_eq!(c.exec_out(&["sh", "-c", "find /data -iname '*faster_whisper*' -o -iname 'ctranslate2*' | head -3"]).trim(), "", "no local Whisper installed for it");
}

// ---- Hermes' Bot Mode (images/hermes/boot/src/bots.rs, images/hermes/botmode.py) ----

/// The model calls made as `agent` whose messages hold `text`.
fn calls_of(model: &Model, agent: &str, text: &str) -> Vec<support::model::Call> {
    model.calls.lock().unwrap().iter().filter(|m| m.agent.as_deref() == Some(agent) && m.body["messages"].to_string().contains(text)).cloned().collect()
}

/// What Hermes keeps in `profile`'s Bot Chat: its key, and its messages'
/// texts (each cut short).
fn bot_chat_of(c: &Container, profile: &str) -> serde_json::Value {
    hermes_python(
        c,
        &[],
        &format!(
            "import json\nfrom pathlib import Path\nfrom hermes_state import SessionDB\ndb = SessionDB(db_path=Path('/data/hermes/profiles/{profile}/state.db'), read_only=True)\ns = db.get_session_by_title('Bot Chat') or {{}}\ntexts = [str(m.get('content'))[:300] for m in db.get_messages_as_conversation(s['id'])] if s else []\nprint(json.dumps({{'key': s.get('session_key'), 'texts': texts}}))"
        ),
    )
}

/// Goal (Hermes' Bot Mode, docs/computers.md): each agent is a bot whose
/// own chat is its Bot Chat, so there its model has Hermes' teammate roster
/// (each teammate's name and role, and the gateway's own `@hermes`, said to
/// be no agent) and the `message_agent` tool; a message one bot sends
/// another is a hand-off in the other's own chat, posted as the sender, so
/// the person sees it and the other's answer is a turn of the bridge's
/// there (its Bot Chat, as that bot, metered to it); that answer comes back
/// to the first bot, which says it in its own chat.
/// Method: two agents, each with its own chat (`<label>-chat`); a first
/// message in each makes its session, which the image's hook titles as the
/// turn starts, and the image's keeper then holds as Hermes' live owner;
/// then juniper is asked to message maple (the scripted model's `dm:`).
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn bots_message_each_other() {
    let tag = hermes_tag();
    build(&repo_dir(), "images/hermes/Dockerfile", &tag);
    let fake = Fake::start("0.0.0.0:0", &["juniper", "maple", "fern"]).await;
    let model = Model::start("0.0.0.0:0").await;
    // each its own chat, labelled for it, its suffix its own: the boot finds it
    let (jchat, mchat) = (fake.chat_named("juniper-chat--h6j7", &["juniper"]), fake.chat_named("maple-chat--z8w6", &["maple"]));
    // and chats that are no Bot Chat: juniper's second (the shell's
    // `<label>-chat-<n>`), and fern's only one, so fern has no Bot Chat
    let (jchat2, garden) = (fake.chat_named("juniper-chat-2--m4q8", &["juniper"]), fake.chat_named("garden--r2d5", &["fern"]));
    let c = Container::run(&tag, fake.addr.port(), model.addr.port(), &[]);
    fake.until(180_000, "Hermes' bridge to follow the agents' chats", |w| w.live_sockets() >= 7).await;
    let yaml = c.exec_out(&["cat", "/data/hermes/profiles/maple--k3x9/profile.yaml"]);
    assert!(yaml.contains("display_name: \"Maple\"\n") && yaml.contains("ui_meta:\n  hermes-bots:\n    title: \"Maple\"\n"), "maple is a bot, named by her fragment's title: {yaml}");
    // renamed while it runs (its fragment's title, as Settings or `fragment
    // agent --name` sets it: docs/api.md, "An agent's name and picture"),
    // a bot's identity follows within seconds, nothing restarted
    fake.with(|w| w.fragments.get_mut("fern--k3x9").expect("fern's fragment").title = "Fernando".into());
    let fern = || c.exec_out(&["cat", "/data/hermes/profiles/fern--k3x9/profile.yaml"]);
    let t = Instant::now();
    while !fern().contains("display_name: \"Fernando\"\n") {
        assert!(t.elapsed() < Duration::from_secs(40), "fern's Bot Mode identity follows her new name: {}", fern());
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(fern().contains("ui_meta:\n  hermes-bots:\n    title: \"Fernando\"\n"), "her bot's title too: {}", fern());
    assert_eq!(c.logs().matches("\"boot.init\"").count(), 1, "nothing restarted for it");
    // each bot's first message in its own chat makes its session there
    for (chat, agent) in [(&jchat, "juniper--k3x9"), (&mchat, "maple--k3x9")] {
        let said = fake.say(chat, &person("paul"), json!({ "text": "hello" }));
        let turn = fragment_bridge::records::turn_id(agent, chat, "chat", said["seq"].as_u64().unwrap());
        within(&fake, chat, &c, 180_000, "a bot's first reply", |w| w.bodies(chat, "work", "turn.end").iter().any(|e| e["turn"] == turn)).await;
    }
    // titled as their first turns started, so those turns had the roster
    assert_eq!(c.logs().matches("\"botmode.titled\"").count(), 2, "each bot's own chat titled Bot Chat once: {}", told(&fake, &jchat, &c));
    for agent in ["juniper--k3x9", "maple--k3x9"] {
        let first = calls_of(&model, agent, "hello").into_iter().next().unwrap_or_else(|| panic!("{agent}'s first turn"));
        assert!(first.body["messages"][0]["content"].to_string().contains("## Messaging other agents"), "{agent}'s first turn in its Bot Chat has the roster");
    }
    assert_eq!(bot_chat_of(&c, "maple--k3x9")["key"], "agent:maple--k3x9:relay:group:maple-chat--z8w6/maple--k3x9", "maple's Bot Chat is her own chat's session");
    let hooked = c.exec_out(&["readlink", "/data/hermes/profiles/maple--k3x9/hooks/fragment-bot-chat"]);
    assert_eq!(hooked.trim(), "/opt/fragment/hooks/fragment-bot-chat");

    // juniper, asked in her Bot Chat, messages maple
    let reply = |w: &support::fake::World, chat: &str, turn: &str| w.bodies(chat, "chat", "reply").into_iter().find(|r| r["turn"] == turn).and_then(|r| r["text"].as_str().map(str::to_string));
    let jturn = asked(&fake, &jchat, &c, "dm: maple: ping from juniper", |w, turn| reply(w, &jchat, turn).is_some()).await;
    let ack = fake.with(|w| reply(w, &jchat, &jturn)).unwrap_or_default();
    eprintln!("bots: juniper's message_agent said: {}", ack.chars().take(400).collect::<String>());
    assert!(ack.contains("queued"), "message_agent took juniper's message: {ack}");
    let asking = calls_of(&model, "juniper--k3x9", "dm: maple").into_iter().next().expect("juniper's model was asked");
    let system = asking.body["messages"][0]["content"].to_string();
    let offered = asking.body["tools"].to_string();
    assert!(offered.contains("\"message_agent\""), "juniper's Bot Chat has message_agent: {offered}");
    assert!(system.contains("## Messaging other agents") && system.contains("`@maple--k3x9`"), "and the roster, maple on it: {system}");
    assert!(system.contains("`@hermes` — this computer's gateway, not an agent"), "the gateway's own profile, said to be no agent: {system}");
    assert!(!system.contains("`@juniper--k3x9` —"), "juniper is not her own teammate: {system}");

    assert!(ack.contains("live Bot Chat owner"), "queued for maple's live owner, the keeper: {ack}");
    // the message is juniper's, in maple's own chat, for maple: a hand-off
    let t = Instant::now();
    let asked_maple = |w: &support::fake::World| w.records(&mchat, "chat").into_iter().find(|r| r["principal"] == "npub1juniper" && r["body"]["to"] == json!(["npub1maple"]));
    within(&fake, &mchat, &c, 120_000, "juniper's message in maple's chat", |w| asked_maple(w).is_some()).await;
    let record = fake.with(|w| asked_maple(w)).unwrap();
    eprintln!("bots: juniper's message in maple's chat {} ms after her turn: {}", t.elapsed().as_millis(), record["body"]);
    let text = record["body"]["text"].as_str().unwrap_or_default();
    assert!(text.starts_with("Message from 🤖 Juniper (@juniper--k3x9): ping from juniper\n\n"), "as Hermes attributes it, by her title: {text}");
    assert!(text.ends_with("(Your reply here goes back to Juniper on its own: answer here, and don't message_agent Juniper about this.)"), "and told her reply there goes back: {text}");
    // maple's answer is a turn of the bridge's there, as maple
    let mturn = fragment_bridge::records::turn_id("maple--k3x9", &mchat, "chat", record["seq"].as_u64().unwrap());
    within(&fake, &mchat, &c, 180_000, "maple's turn", |w| w.bodies(&mchat, "work", "turn.end").iter().any(|e| e["turn"] == mturn)).await;
    let manswer = fake.with(|w| reply(w, &mchat, &mturn)).unwrap_or_default();
    eprintln!("bots: maple answered in her chat: {manswer}");
    assert!(manswer.contains("ping from juniper"), "maple answered juniper's message: {manswer}");
    // and juniper says it in her own chat
    within(&fake, &jchat, &c, 180_000, "juniper to say maple's answer", |w| w.bodies(&jchat, "chat", "reply").iter().any(|r| r["text"].as_str().is_some_and(|t| t.contains("relayed:") && t.contains("ping from juniper")))).await;
    eprintln!("bots: juniper said maple's answer {} ms after her turn", t.elapsed().as_millis());
    let kept = bot_chat_of(&c, "maple--k3x9");
    let texts = kept["texts"].to_string();
    assert!(texts.contains("Message from") && texts.contains("ping from juniper"), "the message is in maple's Bot Chat: {kept}");
    let joined = fake.with(|w| w.requests.iter().filter(|(r, ..)| r.starts_with("PUT ") && r.contains("/members/")).map(|(r, ..)| r.clone()).collect::<Vec<_>>());
    assert!(joined.is_empty(), "juniper is added to no chat: {joined:?}");
    let logs = c.logs();
    let delivered: Vec<&str> = logs.lines().filter(|l| l.contains("\"botmode.delivered\"")).collect();
    assert!(delivered.len() == 1 && delivered[0].contains("\"status\": \"settled\"") && delivered[0].contains("\"sender\": \"juniper--k3x9\""), "one delivery, settled: {delivered:?}");
    // no turn of Hermes' own answered it: maple's every model call that was handed it is the bridge's turn's
    let handed: Vec<_> = calls_of(&model, "maple--k3x9", "ping from juniper").into_iter().filter(|m| m.body["messages"].as_array().and_then(|ms| ms.iter().rev().find(|x| x["role"] == "user")).is_some_and(|u| u.to_string().contains("ping from juniper"))).collect();
    assert_eq!(handed.len(), 1, "one turn of maple's answered it");

    // Asked to message juniper back as well, maple's answer is still said
    // once: the message she sends back is a hand-off in juniper's chat,
    // which juniper passes on, and each one's woken turn after it, told it
    // has had that answer, says nothing (Hermes' `[SILENT]`)
    let jturn2 = asked(&fake, &jchat, &c, "dm: maple: ping again and dm back", |w, turn| reply(w, &jchat, turn).is_some()).await;
    let t = Instant::now();
    // bounded: three minutes
    while c.logs().matches("\"botmode.delivered\"").count() < 3 {
        assert!(t.elapsed() < Duration::from_secs(180), "both messages delivered: {}", told(&fake, &jchat, &c));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // what either woken turn would say, said by now
    tokio::time::sleep(Duration::from_secs(20)).await;
    let logs = c.logs();
    let delivered: Vec<&str> = logs.lines().filter(|l| l.contains("\"botmode.delivered\"")).collect();
    eprintln!("bots: delivered {delivered:#?}");
    let had = |why: &str, sender: &str| delivered.iter().any(|l| l.contains(&format!("\"had\": \"{why}\"")) && l.contains(&format!("\"sender\": \"{sender}\"")));
    assert!(had("answered", "juniper--k3x9") && had("reply", "maple--k3x9"), "juniper's message answered directly, and maple's a reply: {delivered:?}");
    let seq_of = |w: &support::fake::World, chat: &str, by: &str, says: &str| w.records(chat, "chat").into_iter().find(|r| r["principal"] == by && r["body"]["text"].as_str().is_some_and(|t| t.contains(says))).and_then(|r| r["seq"].as_u64());
    let said_after = |w: &support::fake::World, chat: &str, agent: &str, since: u64, not: &str| -> Vec<String> {
        w.records(chat, "chat").into_iter().filter(|r| r["principal"] == agent && r["seq"].as_u64() > Some(since) && r["body"].get("kind").is_none() && r["body"]["turn"] != not).filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect()
    };
    let (jsaid, msaid) = fake.with(|w| {
        let asked_at = seq_of(w, &jchat, &person("paul"), "ping again and dm back").unwrap_or(0);
        let handed_at = seq_of(w, &mchat, "npub1juniper", "ping again and dm back").unwrap_or(0);
        let mturn2 = fragment_bridge::records::turn_id("maple--k3x9", &mchat, "chat", handed_at);
        (said_after(w, &jchat, "npub1juniper", asked_at, &jturn2), said_after(w, &mchat, "npub1maple", handed_at, &mturn2))
    });
    eprintln!("bots: after asking again, juniper said {jsaid:?}; maple said more {msaid:?}");
    assert!(jsaid.len() == 1 && jsaid[0].contains("pong"), "juniper passes maple's message on once, and her woken turn says nothing: {jsaid:?}");
    assert!(msaid.is_empty(), "maple's woken turn says nothing: {msaid:?}");
    assert!(fake.with(|w| seq_of(w, &jchat, "npub1maple", "pong").is_some()), "maple's message back is a hand-off in juniper's chat");

    // After a restart (p5, 2026-10-09: a deploy, then the first message),
    // maple's answer still comes back, and a background job's completion
    // in any chat: Hermes reads each session back from disk, with no live
    // adapter, and reaches it through the relay only because its agent's
    // profile is routed (bots.rs, `profile_routes`). Sessions in the other
    // chats first, made before the restart.
    let first = |chat: &str, agent: &str, text: &str| {
        let said = fake.say(chat, &person("paul"), json!({ "text": text }));
        fragment_bridge::records::turn_id(agent, chat, "chat", said["seq"].as_u64().unwrap())
    };
    for (chat, agent) in [(&jchat2, "juniper--k3x9"), (&garden, "fern--k3x9")] {
        let turn = first(chat, agent, "hello in another chat");
        within(&fake, chat, &c, 180_000, "a first reply in a chat that is no Bot Chat", |w| w.bodies(chat, "work", "turn.end").iter().any(|e| e["turn"] == turn)).await;
    }
    // each agent is told its name, its agent fragment's title, at every
    // turn (hermes.rs, `told_name`), in its Bot Chat and any other chat:
    // fern, renamed before her first turn, and again while her session
    // runs, is told the new one at her next turn, nothing restarted
    let system = |agent: &str, said: &str| calls_of(&model, agent, said).into_iter().next().map(|m| content_text(&m.body["messages"][0])).unwrap_or_default();
    let said = system("juniper--k3x9", "hello");
    assert!(said.contains("Your name is Juniper: your agent fragment's title"), "juniper is told her name in her Bot Chat: {said}");
    let said = system("fern--k3x9", "hello in another chat");
    assert!(said.contains("Your name is Fernando: ") && !said.contains("Your name is Fern:"), "fern is told the name she was given: {said}");
    fake.with(|w| w.fragments.get_mut("fern--k3x9").expect("fern's fragment").title = "Fernanda".into());
    let config = || c.exec_out(&["cat", "/data/hermes/profiles/fern--k3x9/config.yaml"]);
    let t = Instant::now();
    while !config().contains("Your name is Fernanda: ") {
        assert!(t.elapsed() < Duration::from_secs(40), "fern's config follows her new name: {}", config());
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let turn = first(&garden, "fern--k3x9", "who are you now");
    within(&fake, &garden, &c, 180_000, "fern's turn after her rename", |w| w.bodies(&garden, "work", "turn.end").iter().any(|e| e["turn"] == turn)).await;
    let said = system("fern--k3x9", "who are you now");
    assert!(said.contains("Your name is Fernanda: ") && !said.contains("Your name is Fernando"), "her next turn, in the same session, is told her new name: {said}");
    assert_eq!(c.logs().matches("\"boot.init\"").count(), 1, "nothing restarted for it");
    let started = c.logs().matches("\"botmode.started\"").count();
    c.restart();
    let t = Instant::now();
    // bounded: three minutes
    while !c.logs().split("\"botmode.started\"").nth(started + 1).is_some_and(|after| after.matches("\"botmode.owned\"").count() >= 2) {
        assert!(t.elapsed() < Duration::from_secs(180), "the keeper to hold both Bot Chats again: {}", told(&fake, &jchat, &c));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    asked(&fake, &jchat, &c, "dm: maple: ping after a restart", |w, turn| reply(w, &jchat, turn).is_some()).await;
    within(&fake, &jchat, &c, 180_000, "juniper to say maple's answer after a restart", |w| w.bodies(&jchat, "chat", "reply").iter().any(|r| r["text"].as_str().is_some_and(|t| t.contains("relayed:") && t.contains("ping after a restart")))).await;
    // a job started in juniper's second chat, and in fern's (who has no
    // Bot Chat): each one's completion comes back, a turn of its own there
    let completed = |w: &support::fake::World, chat: &str| w.bodies(chat, "chat", "reply").iter().any(|r| r["text"].as_str().is_some_and(|t| t.contains("[IMPORTANT: Background process")));
    for (chat, agent) in [(&jchat2, "juniper--k3x9"), (&garden, "fern--k3x9")] {
        first(chat, agent, "notify: sleep 2 && echo the-job-ran");
        within(&fake, chat, &c, 120_000, &format!("{agent}'s job's completion after a restart"), |w| completed(w, chat)).await;
    }
    // one route a profile reaches its sessions in every chat: each bot by
    // its Bot Chat, fern by her chat, and none for juniper's second chat
    let routes = c.exec_out(&["cat", "/etc/hermes/config.yaml"]);
    let route = |agent: &str, chat: &str| format!("  - name: \"{agent}\"\n    platform: \"relay\"\n    profile: \"{agent}\"\n    chat_id: \"{chat}/{agent}\"\n");
    for (agent, chat) in [("juniper--k3x9", "juniper-chat--h6j7"), ("maple--k3x9", "maple-chat--z8w6"), ("fern--k3x9", "garden--r2d5")] {
        assert!(routes.contains(&route(agent, chat)), "each agent routed to its profile by one chat: {routes}");
    }
    assert_eq!(routes.matches("  - name: ").count(), 3, "one route a profile: {routes}");

    // `/new` in her Bot Chat (its owner's command, docs/chat-records.md): a
    // new session under the chat's own key, which the hook titles `Bot
    // Chat` as its first turn starts, so that turn has the roster again
    let titled = c.logs().matches("\"botmode.titled\"").count();
    let fresh = fake.say(&jchat, &person("paul"), json!({ "kind": "command", "command": "new" }));
    let tnew = fragment_bridge::records::turn_id("juniper--k3x9", &jchat, "chat", fresh["seq"].as_u64().unwrap());
    within(&fake, &jchat, &c, 120_000, "/new's end", |w| w.bodies(&jchat, "work", "turn.end").iter().any(|e| e["turn"] == tnew)).await;
    eprintln!("bots: /new said {:?}", fake.with(|w| reply(w, &jchat, &tnew)));
    asked(&fake, &jchat, &c, "a fresh start, juniper", |w, turn| reply(w, &jchat, turn).is_some()).await;
    let kept = bot_chat_of(&c, "juniper--k3x9");
    eprintln!("bots: juniper's Bot Chat after /new: {kept}");
    assert_eq!(kept["key"], "agent:juniper--k3x9:relay:group:juniper-chat--h6j7/juniper--k3x9", "still her own chat's session: {kept}");
    let texts = kept["texts"].to_string();
    assert!(texts.contains("a fresh start, juniper") && !texts.contains("ping from juniper"), "the title is the new session's, which holds none of the chat before: {kept}");
    assert_eq!(c.logs().matches("\"botmode.titled\"").count(), titled + 1, "titled again as its first turn started");
    let first = calls_of(&model, "juniper--k3x9", "a fresh start, juniper").into_iter().next().expect("the first turn after /new");
    assert!(first.body["messages"][0]["content"].to_string().contains("## Messaging other agents"), "and that turn has the roster");
    assert!(!first.body["messages"].to_string().contains("ping from juniper"), "a new session: none of the chat before");
}

/// Goal (docs/computers.md, "An agent's own model"; Paul, 2026-10-08): an
/// agent whose owner connected a provider that serves models, and picked
/// one of them for it in settings (its `agent.json`'s `model`), runs its
/// next turn on that model, at the provider's base URL the platform names,
/// its key the provider's placeholder (the swap adds the person's key on
/// the way out; the guest never holds it), with nothing restarted; and
/// once its owner disconnects, its next turn is its tier's again, through
/// the route. Method: the Hermes image, its owner's credential and choice
/// set on the fake API while it runs; the scripted model answers both the
/// route (`/v1`) and the provider (`/own/api/v1`), and records each call's
/// path, model and key.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn an_agent_runs_on_its_owners_model() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[("HERMES_BOOT_MODEL_MS", "1000")]);
    fake.until(120_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let replied = |w: &support::fake::World, turn: &str| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == turn);
    let calls_of = |since: usize| model.calls.lock().unwrap()[since..].iter().filter(|c| c.path.ends_with("/chat/completions")).cloned().collect::<Vec<_>>();
    let config = || c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/config.yaml"]);

    // its tier, through the route
    let before = model.calls.lock().unwrap().len();
    asked(&fake, &chat, &c, "hello on the route", replied).await;
    let calls = calls_of(before);
    assert!(!calls.is_empty() && calls.iter().all(|c| c.path == "/v1/chat/completions" && c.model == CHEAP_ID), "its tier, through the route: {calls:?}");

    // Decision 65: platform Fast uses High even with an old Low setting.
    // (the turn's own calls: with its tools, its last user message its
    // message; Hermes' title for the chat, an auxiliary call of its own
    // with no tools, asks for no reasoning, `none`, which the route takes
    // as `low`)
    let efforts = |calls: &[support::model::Call], said: &str| -> Vec<serde_json::Value> {
        let turns = calls.iter().filter(|c| c.body.get("tools").is_some() && c.body["messages"].as_array().and_then(|m| m.iter().rev().find(|m| m["role"] == "user")).is_some_and(|m| m["content"].to_string().contains(said)));
        turns.map(|c| c.body["reasoning_effort"].clone()).collect()
    };
    let high = efforts(&calls, "hello on the route");
    assert!(!high.is_empty() && high.iter().all(|e| e == "high"), "it thinks hard by default: {high:?}");
    fake.with(|w| {
        w.fragments.get_mut("juniper--k3x9").unwrap().files.insert("agent.json".into(), bytes::Bytes::from_static(br#"{"tier":"cheap","reasoning":"low"}"#));
    });
    let before = model.calls.lock().unwrap().len();
    asked(&fake, &chat, &c, "hello thinking less", replied).await;
    let low = efforts(&calls_of(before), "hello thinking less");
    assert!(!low.is_empty() && low.iter().all(|e| e == "high"), "Fast's effort comes from the route: {low:?}");

    // its owner connects the provider and picks one of its models
    let placeholder = format!("fck_openrouter_{}", "0123456789abcdef0123456789abcdef");
    let base = format!("http://model.fragment.internal:{}/own/api/v1", model.addr.port());
    let id = "anthropic/claude-sonnet-5.5";
    let deep_id = "deepseek/deepseek-v4.1-flash";
    fake.with(|w| {
        w.computer["agents"][0]["credentials"] = json!([{ "provider": "openrouter", "kind": "own", "env": ["OPENROUTER_API_KEY"], "placeholder": placeholder, "hosts": ["openrouter.ai"], "modelBase": base }]);
        let agent_json = json!({"tier":"cheap","model":{"provider":"openrouter","id":id,"deep":{"provider":"openrouter","id":deep_id,"reasoning":"max"}},"reasoning":"low","color":"blue"}).to_string();
        w.fragments.get_mut("juniper--k3x9").unwrap().files.insert("agent.json".into(), bytes::Bytes::from(agent_json));
    });
    let t = Instant::now();
    while !config().contains(&format!("  default: \"{id}\"\n")) {
        assert!(t.elapsed() < Duration::from_secs(30), "the profile never named its own model; its config:\n{}\nthe container said:\n{}", config(), c.logs());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let before = model.calls.lock().unwrap().len();
    asked(&fake, &chat, &c, "hello on my own model", replied).await;
    let calls = calls_of(before);
    let main: Vec<_> = calls.iter().filter(|c| c.body["messages"].to_string().contains("hello on my own model")).collect();
    assert!(!main.is_empty(), "the turn's model call: {calls:?}");
    let own_effort = efforts(&calls, "hello on my own model");
    assert!(!own_effort.is_empty() && own_effort.iter().all(|e| e == "low"), "BYO retains the owner's reasoning pick: {own_effort:?}");
    assert!(
        main.iter().all(|c| c.path == "/own/api/v1/chat/completions" && c.model == id && c.authorization.as_deref() == Some(format!("Bearer {placeholder}").as_str())),
        "its own model, at its provider, the placeholder its key: {main:?}"
    );
    assert!(!c.exec_out(&["cat", "/data/hermes/profiles/juniper--k3x9/config.yaml"]).contains("sk-or-"), "no key of the person's anywhere in its config");
    // `/model` shows the model it runs: its owner's, by its id
    let shown = model_shown(&fake, &chat, &c).await;
    assert!(shown.contains(id), "/model shows its own model: {shown:?}");

    // Its separate Deep selection is used by native background delegates.
    // The result's file still reaches the chat after the parent ended.
    let before = model.calls.lock().unwrap().len();
    let parent = asked(&fake, &chat, &c, "delegate the gate work", replied).await;
    within(&fake, &chat, &c, 90_000, "BYO Deep's background result and file", |w| w.bodies(&chat, "chat", "reply").iter().any(|r| r["turn"] != parent && r["attachments"].as_array().is_some_and(|a| a.iter().any(|a| a["name"] == "delegate.txt")))).await;
    let calls = calls_of(before);
    let child: Vec<_> = calls.iter().filter(|c| support::model::delegate_goal(&c.body)).collect();
    assert!(!child.is_empty(), "a real native delegate ran");
    assert!(child.iter().all(|c| c.path == "/own/api/v1/chat/completions" && c.model == deep_id && c.authorization.as_deref() == Some(format!("Bearer {placeholder}").as_str()) && c.tier.is_none() && c.body["reasoning_effort"] == "max" && c.body["max_tokens"] == 32768), "Deep uses its owner's model, credential, effort and output cap: {child:?}");
    let main: Vec<_> = calls.iter().filter(|c| c.body.get("tools").is_some() && !support::model::delegate_goal(&c.body)).collect();
    assert!(!main.is_empty() && main.iter().all(|c| c.model == id && c.body["reasoning_effort"] == "low"), "Fast keeps its own selection: {main:?}");
    assert!(config().contains("  fallback_providers: []\n"), "BYO never inherits platform Deep fallbacks");
    assert_eq!(c.logs().matches("\"boot.init\"").count(), 1, "both choices took effect without restarting");

    // disconnected: its tier again, through the route
    fake.with(|w| w.computer["agents"][0]["credentials"] = json!([]));
    let t = Instant::now();
    while !config().contains(&format!("  default: \"{CHEAP_ID}\"\n")) {
        assert!(t.elapsed() < Duration::from_secs(30), "the profile never went back to its tier; its config:\n{}", config());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let before = model.calls.lock().unwrap().len();
    asked(&fake, &chat, &c, "hello on the route again", replied).await;
    let calls = calls_of(before);
    assert!(!calls.is_empty() && calls.iter().all(|c| c.path == "/v1/chat/completions" && c.model == CHEAP_ID), "its tier again: {calls:?}");

    // the deployment's models change while it runs (a release): its view
    // names the new id, and the next turn's calls name it, nothing restarted
    let next = "z-ai/glm-5.4-flash";
    fake.with(|w| w.computer["models"]["cheap"] = json!(next));
    let t = Instant::now();
    while !config().contains(&format!("  default: \"{next}\"\n")) {
        assert!(t.elapsed() < Duration::from_secs(30), "the profile never named the new id; its config:\n{}", config());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let before = model.calls.lock().unwrap().len();
    asked(&fake, &chat, &c, "hello on the new model", replied).await;
    let calls = calls_of(before);
    assert!(!calls.is_empty() && calls.iter().all(|c| c.model == next), "the new id, read from its view: {calls:?}");
}

/// What `/model` (no model named) answers in the chat, its owner's command:
/// Hermes' own reply, no model call.
async fn model_shown(fake: &Fake, chat: &str, c: &Container) -> String {
    let said = fake.say(chat, &person("paul"), json!({ "kind": "command", "command": "model", "args": "" }));
    let t = fragment_bridge::records::turn_id("juniper--k3x9", chat, "chat", said["seq"].as_u64().unwrap());
    within(fake, chat, c, 120_000, "/model's end", ended_turn(chat, &t)).await;
    fake.with(|w| w.bodies(chat, "chat", "reply").into_iter().find(|r| r["turn"] == t.as_str())).and_then(|r| r["text"].as_str().map(str::to_string)).unwrap_or_default()
}

// ---- SimpleX on our Hermes image (docs/bridge.md, "SimpleX") ----

/// What `c`'s screen port answers `method path` (with `body`, JSON) with:
/// its status and JSON.
fn screen_call(c: &Container, method: &str, path: &str, body: Option<&serde_json::Value>) -> (u16, serde_json::Value) {
    let (url, sent) = (format!("http://127.0.0.1:{}{path}", c.port(6080)), body.map(|b| b.to_string()).unwrap_or_default());
    let mut args = vec!["-s", "-X", method, "-w", "\n%{http_code}", url.as_str()];
    if body.is_some() {
        args.extend(["-H", "content-type: application/json", "--data-binary", sent.as_str()]);
    }
    let out = Command::new("curl").args(&args).output().expect("curl runs");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", "0"));
    (code.trim().parse().unwrap_or(0), serde_json::from_str(body).unwrap_or(serde_json::Value::Null))
}

/// `c`'s SimpleX link, as the page of `session` (its link ticket's nonce)
/// sees it, once `done` holds.
async fn simplex_until(c: &Container, session: &str, ms: u64, what: &str, done: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let t = Instant::now();
    // bounded by `ms`
    loop {
        let (code, v) = screen_call(c, "GET", &format!("/simplex/state?agent=juniper--k3x9&s={session}"), None);
        if code == 200 && done(&v) {
            return v;
        }
        assert!(t.elapsed() < Duration::from_millis(ms), "{what}: not within {ms} ms; last {code} {v}\n{}", c.logs().lines().filter(|l| l.contains("simplex")).collect::<Vec<_>>().join("\n"));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// How many SimpleX cores run in `c`.
fn simplex_cores(c: &Container) -> usize {
    c.exec_out(&["sh", "-c", "for p in /proc/[0-9]*/comm; do cat $p 2>/dev/null; done"]).lines().filter(|l| l.trim() == "simplex-chat").count()
}

/// Goal (SimpleX, decision 32, Paul's "work well across restarts and
/// updates"; a chat of its own since 2026-10-26): our Hermes image, always
/// on, links its agent to a person's SimpleX app from its screen port, by
/// its name as its title says it now (renamed before the link, and again
/// after it: the person's app follows), and makes its SimpleX chat for its
/// owner; the person's message lands there,
/// a turn of Hermes' asked as its owner, its session told the chat is their
/// phone, answered on SimpleX; words typed in that chat on the web, and a
/// routine with no chat while their home is that chat, are answered on
/// SimpleX too (one with nothing to tell, its `[SILENT]`, sends nothing),
/// and a reply in the agent's own chat is not; a voice note is
/// transcribed through the route and answered; a hold stops the core before
/// the image answers it and its copies name none of the core's databases;
/// a restart keeps the link; and a wake from a save older than the life
/// before it (a crash) heals the link with the forced sync, without an announcement,
/// and both ways work. Method: a lab (a local SMP and XFTP server, and the
/// person's simplex-chat) on a Docker network the image joins; the fake API
/// and the scripted model as every case here.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn simplex_on_the_hermes_image() {
    use support::simplex::{received, recovered, say, tag, Lab};
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let mut lab = Lab::up_with("hermes", true);
    let mut person = lab.start("person", false).await;
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let own = fake.chat_named("juniper-chat--h6j7", &["juniper"]);
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let net = format!("{}-net", lab.prefix);
    let xftp = lab.xftp.clone().expect("a lab with files");
    let servers = [("BRIDGE_SIMPLEX_SMP", lab.server.as_str()), ("BRIDGE_SIMPLEX_XFTP", xftp.as_str())];
    let c = Container::run_with(Runtime::Docker, &hermes_tag(), fake.addr.port(), model.addr.port(), &servers, &["--network", &net]);
    fake.until(180_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    // its bridge's reads say the image does SimpleX (so its owner's Settings offer it)
    assert!(fake.with(|w| w.simplex_said) >= 1, "the bridge's read of its computer says SimpleX");
    simplex_until(&c, "", 60_000, "its SimpleX, off", |v| v["phase"] == "off").await;
    assert_eq!(simplex_cores(&c), 0, "nothing linked, no core");
    assert!(fake.with(|w| w.made.is_empty()), "nothing linked, no SimpleX chat");

    // linked from its screen port, as its owner's pane does: by the ticket
    // their Settings were given (none, no link: Hermes' tools reach this
    // port, and hold none)
    let (code, _) = screen_call(&c, "POST", "/simplex/link?agent=juniper--k3x9", None);
    assert_eq!(code, 403, "no ticket, no link");
    // renamed while its bridge runs, before it is linked (Paul's, 2026-10-26:
    // his app showed the name it had when its bridge started)
    let rename = |title: &str| fake.with(|w| w.fragments.get_mut("juniper--k3x9").expect("its fragment").title = title.into());
    rename("Basil");
    let ticket = fake.ticket("juniper", "link");
    let session = ticket["nonce"].as_str().expect("a nonce").to_string();
    let (code, _) = screen_call(&c, "POST", "/simplex/link?agent=juniper--k3x9", Some(&ticket));
    assert_eq!(code, 200, "its owner's ticket links it");
    let v = simplex_until(&c, &session, 60_000, "an invitation", |v| v["phase"] == "inviting").await;
    let hidden = simplex_until(&c, "", 1_000, "another page's view", |v| v["phase"] == "inviting").await;
    assert!(hidden.get("link").is_none() && hidden["hidden"] == true, "the invitation is the page's that asked alone: {hidden}");
    person.cmd(&format!("/connect {}", v["link"].as_str().expect("its link"))).await;
    simplex_until(&c, "", 90_000, "linked", |v| v["phase"] == "linked").await;
    assert!(fake.with(|w| w.doors.contains_key("juniper--k3x9")), "its door's key registered with the ticket");
    let met = person.wait(Duration::from_secs(60), |e| tag(e) == "contactConnected").await;
    let at_person = met["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    // its name in the person's app: its title now, as it connected or just after
    let renamed = |name: &'static str| move |e: &serde_json::Value| tag(e) == "contactUpdated" && e["toContact"]["profile"]["displayName"] == name;
    if met["contact"]["profile"]["displayName"] != "Basil" {
        person.wait(Duration::from_secs(30), renamed("Basil")).await;
    }
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("Hi, it's Basil. You're linked"))).await;
    // renamed while linked: its profile follows, within its read (10 s)
    let t0 = Instant::now();
    rename("Waffle. Bot");
    person.wait(Duration::from_secs(30), renamed("Waffle Bot")).await;
    eprintln!("simplex: a rename in the person's app {} ms after it", t0.elapsed().as_millis());
    // its SimpleX chat, made for its owner as it is linked
    fake.until(30_000, "its SimpleX chat made", |w| w.made.len() == 1).await;
    let chat = fake.with(|w| w.made[0].clone());
    assert!(chat.starts_with("juniper-simplex--") && fake.with(|w| w.fragments[&chat].title == "SimpleX"), "{chat}");

    // a message: in its SimpleX chat, Hermes' turn as its owner, answered on SimpleX
    let t0 = Instant::now();
    say(&mut person, at_person, "hello from SimpleX").await;
    let got = received(&mut person, Duration::from_secs(180), |got| got.iter().any(|t| t.starts_with("scripted:") && t.contains("hello from SimpleX"))).await;
    eprintln!("simplex: a message to Hermes' reply on SimpleX: {} ms ({got:?})", t0.elapsed().as_millis());
    assert!(fake.with(|w| w.records(&chat, "chat").iter().any(|r| r["body"]["relayed"]["via"] == "simplex")), "its message in its SimpleX chat");
    let asked = model.calls.lock().unwrap().iter().filter(|m| m.agent.as_deref() == Some("juniper--k3x9")).map(|m| m.body["messages"].to_string()).find(|m| m.contains("[paul] hello from SimpleX"));
    let asked = asked.expect("the model was asked as juniper, the message its owner's ([paul])");
    for said in [fragment_bridge::runtime::relay::wire::SIMPLEX_TOPIC, "never install, run or set up SimpleX", "Your owner's home is your chat juniper-chat"] {
        assert!(asked.contains(said), "its session says the chat is its owner's phone, its prompt never to set SimpleX up and where home is: {said}");
    }
    // words typed in its SimpleX chat on the web: answered on the phone too
    fake.say(&chat, "npub1paul", json!({ "text": "typed on the web" }));
    received(&mut person, Duration::from_secs(180), |got| got.iter().any(|t| t.starts_with("scripted:") && t.contains("typed on the web"))).await;
    // its home that chat (agent.json, as Settings writes it): a routine with no chat answered on the phone, the agent told so
    fake.with(|w| w.fragments.get_mut("juniper--k3x9").expect("its fragment").files.insert("agent.json".into(), bytes::Bytes::from(r#"{"tier":"cheap","home":"simplex"}"#)));
    let told = |m: &support::model::Call| m.body["messages"].to_string().contains("Your owner's home is their SimpleX, your chat juniper-simplex");
    let t0 = Instant::now();
    // (its prompt rewritten from agent.json within its 10 s read, then a fire)
    tokio::time::sleep(Duration::from_secs(12)).await;
    fake.with(|w| w.append("juniper--k3x9", "tasks", "npub1juniper", json!({ "kind": "routine", "text": "remind me to water the basil" })));
    received(&mut person, Duration::from_secs(180), |got| got.iter().any(|t| t.starts_with("scripted:") && t.contains("remind me to water the basil"))).await;
    eprintln!("simplex: a routine at home to its answer on SimpleX: {} ms", t0.elapsed().as_millis());
    assert!(model.calls.lock().unwrap().iter().rev().find(|m| m.agent.as_deref() == Some("juniper--k3x9")).is_some_and(told), "its prompt says its owner's home is their SimpleX");
    // a routine at home with nothing to tell, its answer `[SILENT]`: nothing
    // in its SimpleX chat, so nothing on the phone (below, with what it heard)
    let fired = fake.with(|w| w.append("juniper--k3x9", "tasks", "npub1juniper", json!({ "kind": "routine", "text": "check the basil; if it is fine, end this turn with exactly [SILENT]" })));
    let quiet = fragment_bridge::records::turn_id("juniper--k3x9", "juniper--k3x9", "tasks", fired["seq"].as_u64().expect("its seq"));
    fake.until(180_000, "the silent routine's end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == quiet)).await;
    assert!(fake.with(|w| !w.bodies(&chat, "chat", "reply").iter().any(|r| r["turn"] == quiet)), "the silent routine says nothing in its SimpleX chat");
    // words typed in its own chat: answered there alone
    fake.say(&own, "npub1paul", json!({ "text": "typed in its own chat" }));
    fake.until(180_000, "its own chat's reply", |w| w.bodies(&own, "chat", "reply").iter().any(|b| b["text"].as_str().is_some_and(|t| t.contains("typed in its own chat")))).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    person.drain(Duration::from_millis(500)).await;
    let heard = received(&mut person, Duration::from_secs(1), |_| true).await;
    assert!(!heard.iter().any(|t| t.contains("typed in its own chat")), "a reply in its own chat stays there: {heard:?}");
    assert!(!heard.iter().any(|t| t.contains("silence marker") || t.contains("SILENT") || t.contains("check the basil")), "nor did the silent routine send the phone anything: {heard:?}");
    // the agent's own user finds no SimpleX core to run, and reads none of
    // the bridge's SimpleX state (its keys, its door's key, its databases)
    let as_agent = |cmd: &str| c.exec_code(&["/command/s6-setuidgid", "hermes", "bash", "-lc", cmd]);
    let (code, said) = as_agent("command -v simplex-chat");
    assert_ne!(code, 0, "simplex-chat is on no PATH of the agent's: {said}");
    let (code, said) = as_agent("/opt/fragment/bin/simplex-chat --version");
    assert_ne!(code, 0, "nor runs as the agent: {said}");
    for path in ["/data/simplex", "/data/simplex/juniper--k3x9/door.key", "/data/simplex/juniper--k3x9/sx_chat.db"] {
        let (code, said) = as_agent(&format!("ls {path} || cat {path}"));
        assert!(code != 0 && said.contains("Permission denied"), "{path} is the bridge's alone: {said}");
    }
    assert!(simplex_cores(&c) >= 1, "while the bridge's own core runs");

    // a voice note: transcribed through the route, its words the turn's, answered there
    let words = "please water the basil";
    let memo = lab.db_dir("person").join("files").join("memo.wav");
    std::fs::write(&memo, support::model::memo(words)).expect("the memo");
    let body = json!([{ "filePath": "/data/files/memo.wav", "msgContent": { "type": "voice", "text": "", "duration": 3 } }]);
    assert_eq!(tag(&person.cmd(&format!("/_send @{at_person} json {body}")).await), "newChatItems");
    let t0 = Instant::now();
    received(&mut person, Duration::from_secs(180), |got| got.iter().any(|t| t.starts_with("scripted:") && t.contains(words))).await;
    eprintln!("simplex: a voice note to its answer on SimpleX: {} ms", t0.elapsed().as_millis());
    let heard = model.calls.lock().unwrap().iter().filter(|m| m.path.ends_with("/audio/transcriptions")).count();
    assert!(heard >= 1, "transcribed through the route");

    // a reply's files, which Hermes attaches (MEDIA:): an MP3 a real encoder
    // made, in the person's app as a voice message of its length, and a PNG
    // as an image, each the chat's own attachment, whole
    let files = lab.db_dir("person").join("files");
    let attached = |w: &support::fake::World, file: &str| w.bodies(&chat, "chat", "reply").into_iter().filter_map(|r| r["attachments"].as_array().cloned()).flatten().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(file)));
    for (asked, kind, file) in [
        ("send: ffmpeg -loglevel error -f lavfi -i sine=frequency=440:duration=2 -ac 1 -y /tmp/hello.mp3 && echo made=/tmp/hello.mp3", "voice", "hello.mp3"),
        ("send: echo iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg== | base64 -d > /tmp/dot.png && echo made=/tmp/dot.png", "image", "dot.png"),
    ] {
        let t0 = Instant::now();
        say(&mut person, at_person, asked).await;
        let (content, got) = support::simplex::received_file(&mut person, &files, kind, Duration::from_secs(180)).await;
        eprintln!("simplex: {file} as {content} in the person's app {} ms after it was asked", t0.elapsed().as_millis());
        let a = fake.with(|w| attached(w, file)).unwrap_or_else(|| panic!("{file} is the chat's reply's attachment"));
        assert_eq!(a["sha256"].as_str(), Some(support::simplex::sha256_of(&got).as_str()), "{file}, whole");
        if kind == "voice" {
            assert_eq!(content["duration"], json!(2), "its length, as the encoder made it: {content}");
        }
    }
    let t0 = Instant::now();
    // bounded: a minute, the uploads' ends having come already
    while !c.exec_out(&["sh", "-c", "ls -A /data/simplex/juniper--k3x9/out"]).trim().is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(60), "their copies let go once uploaded");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // the hold: the core stopped before the image answers, none of its databases copied
    let left_out = hold(&c).unwrap_or_else(|| panic!("no answer to the hold\n{}", c.logs().lines().filter(|l| l.contains("simplex") || l.contains("held")).collect::<Vec<_>>().join("\n")));
    assert_eq!(simplex_cores(&c), 0, "no core under the hold");
    assert!(!left_out.iter().any(|l| l.contains("simplex")), "its copies name none of the core's databases: {left_out:?}");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("simplex-hermes");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let save = dir.join("save.tar");
    save_data(&c, &left_out, &save);
    unhold(&c);
    simplex_until(&c, "", 30_000, "its core again", |v| v["running"] == true).await;

    // after the save: an exchange the save does not hold
    say(&mut person, at_person, "said after the save").await;
    received(&mut person, Duration::from_secs(180), |got| got.iter().any(|t| t.contains("said after the save"))).await;

    // a restart, its disk kept: linked still, and answering
    c.restart();
    simplex_until(&c, "", 180_000, "linked and running after a restart", |v| v["phase"] == "linked" && v["running"] == true).await;
    say(&mut person, at_person, "after the restart").await;
    received(&mut person, Duration::from_secs(180), |got| got.iter().any(|t| t.contains("after the restart"))).await;

    // a crash: the next start wakes from the save before the exchange
    let (took, _) = c.sigterm();
    eprintln!("simplex: stopped ({} ms)", took.as_millis());
    drop(c);
    fake.until(30_000, "its sockets closed", |w| w.live_sockets() == 0).await;
    fake.with(|w| {
        w.computer["generation"] = json!(3);
        w.computer["restored"] = json!({ "generation": 3, "rollback": true, "from": "backup", "at": 1 });
    });
    let c = Container::run_with(Runtime::Docker, &hermes_tag(), fake.addr.port(), model.addr.port(), &[servers[0], servers[1], ("RESTORE_PENDING", "1")], &["--network", &net]);
    let tar = std::fs::File::open(&save).expect("the save");
    let cp = Command::new(docker()).args(["cp", "-a", "-", &format!("{}:/data", c.id)]).stdin(tar).output().expect("docker runs");
    assert!(cp.status.success(), "the restore: {}", String::from_utf8_lossy(&cp.stderr));
    let (code, said) = c.exec_code(&["/usr/local/bin/computer-check"]);
    assert_eq!(code, 0, "the save checks: {said}");
    let t0 = Instant::now();
    assert!(c.exec(&["touch", "/run/computer/restored"]));
    recovered(&mut person, at_person, Duration::from_secs(240), || {
        c.logs().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).any(|e| e["event"] == "simplex.told" && e["agent"] == "juniper--k3x9" && e["notice"] == "Resynced")
    }).await;
    eprintln!("simplex: woken from an older save, both ends synchronized {} ms after the gate", t0.elapsed().as_millis());
    say(&mut person, at_person, "after the rollback").await;
    let replies = received(&mut person, Duration::from_secs(180), |got| got.iter().any(|t| t.starts_with("scripted:") && t.contains("after the rollback"))).await;
    assert_eq!(replies.len(), 1, "only the requested reply, with no restart announcement: {replies:?}");
    let relayed = fake.with(|w| w.records(&chat, "chat").iter().filter(|r| r["body"]["relayed"]["via"] == "simplex").count());
    eprintln!("simplex: {relayed} relayed records on its SimpleX chat");
    assert!(fake.with(|w| w.made.as_slice() == std::slice::from_ref(&chat)), "its SimpleX chat found again after the restarts, never made twice");
}

/// Goal (#627; the review of #664): a step says how its call ended, as
/// real Hermes observed it (its `post_tool_call`; its progress text is
/// off): done, or failed with its exit code, blocked before it ran, timed
/// out past Hermes' own deadline, stopped by its owner; each with Hermes'
/// preview of the call, never its output. Method: a command
/// each, in a computer whose Hermes gives a call 4 s
/// (`HERMES_CONCURRENT_TOOL_TIMEOUT_S`, its own setting); a call carrying
/// Hermes' context-compression marker is one it refuses to run.
#[tokio::test]
#[ignore = "needs Docker: cargo test -p fragment-bridge --test docker -- --ignored"]
async fn a_step_says_how_its_call_ended() {
    build(&repo_dir(), "images/hermes/Dockerfile", &hermes_tag());
    let fake = Fake::start("0.0.0.0:0", &["juniper"]).await;
    let model = Model::start("0.0.0.0:0").await;
    let chat = fake.chat("talk", &["juniper"]);
    let c = Container::run(&hermes_tag(), fake.addr.port(), model.addr.port(), &[("HERMES_CONCURRENT_TOOL_TIMEOUT_S", "4")]);
    within(&fake, &chat, &c, 120_000, "Hermes' bridge to follow its chat", |w| w.live_sockets() >= 2).await;
    let ended = |w: &support::fake::World, t: &str| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == t);
    let steps = |t: &str| fake.with(|w| w.bodies(&chat, "work", "turn.step").into_iter().filter(|s| s["turn"] == t).collect::<Vec<_>>());
    let check = |preview: &str, got: &[serde_json::Value], status: &str, excerpt: &str| {
        eprintln!("{preview}: {got:?}");
        assert_eq!(got.len(), 1, "{preview}: one step, its end's: {}", told(&fake, &chat, &c));
        let s = &got[0];
        assert_eq!((&s["tool"], &s["args"], &s["status"], &s["ok"], &s["excerpt"]), (&json!("terminal"), &json!(preview), &json!(status), &json!(status == "completed"), &json!(excerpt)), "{preview}: {s}");
        assert!(s["tool_call_id"].as_str().is_some_and(|id| !id.is_empty()) && s["duration_ms"].is_u64(), "{s}");
        assert!(!s.to_string().contains("-42"), "never its output: {s}");
    };
    let marked = "echo ⟪HERMES-CONTEXT-COMPRESSION: 12 of 40 chars";
    // Hermes' preview of a command of two: its first, and how many more
    for (command, preview, status, excerpt) in [("echo done-$((40+2))", "echo done-$((40+2))", "completed", "Exit code: 0"), ("echo failed-$((40+2)); exit 7", "echo failed-$((40+2)) + 1 command", "failed", "Exit code: 7"), (marked, marked, "blocked", ""), ("sleep 9", "sleep 9", "timed_out", "")] {
        let turn = asked(&fake, &chat, &c, &format!("run: {command}"), ended).await;
        check(preview, &steps(&turn), status, excerpt);
    }
    // its owner's Stop while the first of two runs: that one is killed (as
    // Hermes says it, a command that failed with 130); the second never
    // starts, and Hermes says nothing of it (v0.21.7 says `cancelled` only
    // of a call it abandons or skips before any starts): no step
    let said = fake.say(&chat, &person("paul"), json!({ "text": "both: sleep 3 | echo never-ran" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    within(&fake, &chat, &c, 60_000, "the command running", |_| c.exec(&["pgrep", "-f", "^sleep 3$"])).await;
    fake.say(&chat, &person("paul"), json!({ "kind": "stop", "turn": turn }));
    within(&fake, &chat, &c, 60_000, "the stopped turn's end", |w| ended(w, &turn)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    check("sleep 3", &steps(&turn), "failed", "Exit code: 130");
}
