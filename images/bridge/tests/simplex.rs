//! The SimpleX lab: a local SMP server (and, for files, an XFTP server) and
//! simplex-chat v7.0.3 clients in Docker (`tests/simplex/lab.Dockerfile`),
//! each driven over its bot API,
//! with the bridge's `simplex` module (its wire and its link's rules) on the
//! agent's side, against the real core.
//!
//! Lower rung, labeled so: the real simplex-chat and a real SMP server, but
//! one local server on a Docker network, not SimpleX's preset ones on the
//! internet, and the test in place of the image's process.
//!
//! Run by hand (it pulls the SMP server's image and builds the lab's once):
//! `cargo test -p fragment-bridge --test simplex -- --ignored --nocapture`.
//! CI's Docker lane runs docker.rs alone, not this.

mod support;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use fragment_bridge::simplex::link::{Action, Inbound, Input, Link, Notice, Phase};
use fragment_bridge::simplex::wire::{self, Resp};
use serde_json::{json, Value};
use support::simplex::*;

/// The lab's computer: the bridge with SimpleX's core
/// (`tests/simplex/bridge.Dockerfile`).
const BRIDGE_IMAGE: &str = "fragment-simplex-bridge:test";


/// The agent's side as the image will run it: the bridge's `link` rules,
/// fed what the real core says through the bridge's `wire`, its commands
/// sent to the core; what it forwards and tells is kept to check.
struct Agent {
    api: Api,
    link: Link,
    forwarded: Vec<Inbound>,
    told: Vec<Notice>,
    /// Every event the core said, as it said it.
    seen: Vec<Value>,
}

impl Agent {
    fn new(api: Api) -> Agent {
        Agent { api, link: Link::default(), forwarded: vec![], told: vec![], seen: vec![] }
    }

    /// The link's rules on `input`, and everything they ask, until nothing
    /// more is asked.
    async fn apply(&mut self, input: Input<'_>) {
        let actions = self.link.apply(input);
        self.run(actions).await;
    }

    async fn run(&mut self, actions: Vec<Action>) {
        let mut todo: VecDeque<Action> = actions.into();
        // bounded: each answer asks at most a few commands, and the lab's are few
        let mut asked = 0;
        while let Some(a) = todo.pop_front() {
            match a {
                Action::Ask(cmd) => {
                    asked += 1;
                    assert!(asked < 64, "the link asks for a bounded number of commands");
                    let answer = self.api.cmd(&cmd.line()).await;
                    let frame = wire::parse(&json!({ "corrId": "1", "resp": answer }).to_string()).expect("the core's answer reads");
                    let next = match &frame.resp {
                        Resp::Error { what } => self.link.apply(Input::Failed { cmd: &cmd, what }),
                        resp => self.link.apply(Input::Said(resp)),
                    };
                    todo.extend(next);
                }
                Action::Forward(i) => self.forwarded.push(i),
                Action::Tell(n) => self.told.push(n),
                Action::Save => {}
            }
        }
    }

    /// The core's events, through the link, until `done` holds (within `within`).
    async fn until(&mut self, within: Duration, done: impl Fn(&Agent) -> bool) {
        let t0 = Instant::now();
        // bounded by `within`
        while !done(self) {
            assert!(t0.elapsed() < within, "not within {within:?}: link {:?}, told {:?}, forwarded {:?}", self.link, self.told, self.forwarded.iter().map(|i| &i.text).collect::<Vec<_>>());
            self.poll().await;
        }
    }

    async fn poll(&mut self) {
        if self.api.events.is_empty() {
            self.api.drain(Duration::from_millis(200)).await;
        }
        while let Some(resp) = self.api.events.pop_front() {
            self.seen.push(resp.clone());
            let frame = wire::parse(&json!({ "resp": resp }).to_string()).unwrap_or_else(|e| panic!("an event of the core's reads: {e}: {resp}"));
            let actions = self.link.apply(Input::Said(&frame.resp));
            self.run(actions).await;
        }
    }

    fn texts(&self) -> Vec<&str> {
        self.forwarded.iter().map(|i| i.text.as_str()).collect()
    }
}


/// Goal: the link against the real core (simplex-chat v7.0.3): linked by
/// its one-time invitation to the person who used it and no one else;
/// their messages forwarded by their own ids, once however often read; a
/// long reply in parts that arrive whole; a restart that loses nothing; a
/// rollback of the agent's database healed by the forced sync its start
/// asks, the reply held through it delivered after; a core whose process
/// died (its disk intact) caught up from its own chat; and the person's own
/// delete letting the link go.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn the_link_against_the_real_core() {
    let mut lab = Lab::up("link");
    let mut agent = Agent::new(lab.start("agent", true).await);
    let mut person = lab.start("person", false).await;
    let mut stranger = lab.start("stranger", false).await;

    // linked by the invitation, to whoever used it
    agent.apply(Input::Want).await;
    let Phase::Inviting { link, .. } = agent.link.phase.clone() else { panic!("an invitation: {:?}", agent.link) };
    let r = person.cmd(&format!("/connect {link}")).await;
    assert_eq!(tag(&r), "sentConfirmation", "{r}");
    agent.until(Duration::from_secs(90), |a| a.link.contact().is_some()).await;
    assert_eq!(agent.told, vec![Notice::Linked { name: "person".into() }]);
    let at_person = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    let t0 = Instant::now();

    // the spent invitation is refused to anyone after
    let r = stranger.cmd(&format!("/connect {link}")).await;
    assert!(tag(&r) == "chatCmdError" && r.to_string().contains("AUTH"), "the spent link: {r}");
    // a contact the link did not make is let go, and nothing of theirs forwarded
    let other = agent.api.cmd("/connect").await;
    let other_link = other["connLinkInvitation"]["connFullLink"].as_str().expect("a link").to_string();
    stranger.cmd(&format!("/connect {other_link}")).await;
    agent.until(Duration::from_secs(90), |a| a.told.contains(&Notice::Refused)).await;
    assert!(agent.forwarded.is_empty());
    println!("linked, and a stranger refused: {} ms", t0.elapsed().as_millis());

    // both ways
    let id = say(&mut person, at_person, "hello agent").await;
    agent.until(Duration::from_secs(30), |a| !a.forwarded.is_empty()).await;
    assert_eq!(agent.forwarded[0].id, id, "forwarded by the id its sender's app gave it");
    assert_eq!(agent.forwarded[0].text, "hello agent");
    let long: String = (0..3_000).map(|i| format!("line {i}\n")).collect();
    assert!(long.len() > 2 * wire::SEND_JSON_MAX_BYTES);
    agent.apply(Input::Reply(&long)).await;
    let parts = received(&mut person, Duration::from_secs(30), |got| got.concat() == long).await;
    assert!(parts.len() >= 3, "{} parts", parts.len());
    assert!(agent.told.iter().all(|n| !matches!(n, Notice::Failed { .. })), "{:?}", agent.told);

    // a restart (SIGTERM, as a sleep's stop after its save): nothing lost, nothing twice
    let saved = agent.link.clone();
    drop(agent);
    lab.stop("agent");
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved.to_json()).expect("the link reads back");
    agent.apply(Input::Started { rollback: false }).await;
    assert_eq!(agent.forwarded.iter().filter(|i| i.id == id).count(), 1, "the catch-up forwards it again, by the same id");
    say(&mut person, at_person, "after the restart").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"after the restart")).await;
    agent.apply(Input::Reply("answered after the restart")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after the restart")).await;

    // a rollback: the save before an exchange both ways, restored
    drop(agent);
    lab.stop("agent");
    lab.snapshot("agent", "save");
    let saved = saved.to_json();
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved).expect("the link reads back");
    agent.apply(Input::Started { rollback: false }).await;
    say(&mut person, at_person, "said after the save").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"said after the save")).await;
    agent.apply(Input::Reply("answered after the save")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after the save")).await;
    drop(agent);
    lab.stop("agent");
    lab.restore("agent", "save");
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved).expect("the save's link");
    let t0 = Instant::now();
    agent.apply(Input::Started { rollback: true }).await;
    assert_eq!(agent.told, vec![Notice::MaybeLost]);
    agent.apply(Input::Reply("held through the sync")).await;
    assert_eq!(agent.link.outbox.len(), 1, "held while out of step");
    agent.until(Duration::from_secs(60), |a| a.told.contains(&Notice::Resynced)).await;
    println!("a rollback's forced sync: in step {} ms after the start", t0.elapsed().as_millis());
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "held through the sync")).await;
    say(&mut person, at_person, "after the rollback").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"after the rollback")).await;
    agent.apply(Input::Reply("answered after the rollback")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after the rollback")).await;

    // the core's process dies mid-burst, its disk intact (the bridge starts it again)
    let ids: Vec<String> = {
        let mut ids = vec![];
        for i in 0..5 {
            ids.push(say(&mut person, at_person, &format!("burst {i}")).await);
        }
        ids
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let saved = agent.link.to_json();
    drop(agent);
    docker(&["kill", "-s", "KILL", &lab.container("agent")]).expect("killed");
    docker(&["rm", "-f", &lab.container("agent")]).expect("gone");
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved).expect("the link reads back");
    agent.apply(Input::Started { rollback: false }).await;
    agent.until(Duration::from_secs(30), |a| ids.iter().all(|id| a.forwarded.iter().any(|i| &i.id == id))).await;

    // the person deletes the agent in their app: the link lets go
    let r = person.cmd(&format!("/_delete @{at_person} full notify=on")).await;
    assert_eq!(tag(&r), "contactDeleted", "{r}");
    agent.until(Duration::from_secs(30), |a| a.link.phase == Phase::Off).await;
    assert!(agent.told.contains(&Notice::Unlinked { by_them: true }), "{:?}", agent.told);

    // and an invitation withdrawn before anyone used it
    agent.apply(Input::Want).await;
    assert!(matches!(agent.link.phase, Phase::Inviting { .. }));
    agent.apply(Input::Unlink).await;
    assert_eq!(agent.link.phase, Phase::Off);
    println!("frames in {}", lab.dir.display());
}


/// A phone away for several recovery timer ticks must see a single key
/// negotiation when it returns. Re-forcing an accepted sync replaces its
/// private keys while earlier public keys are already queued on SMP.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn a_rollback_sync_waits_for_an_offline_phone() {
    let mut lab = Lab::up("offline-sync");
    let mut agent = Agent::new(lab.start("agent", true).await);
    let mut person = lab.start("person", false).await;
    agent.apply(Input::Want).await;
    let Phase::Inviting { link, .. } = agent.link.phase.clone() else { panic!("an invitation") };
    assert_eq!(tag(&person.cmd(&format!("/connect {link}")).await), "sentConfirmation");
    agent.until(Duration::from_secs(30), |a| a.link.contact().is_some()).await;
    let at_person = person.wait(Duration::from_secs(30), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent's contact");
    say(&mut person, at_person, "before going offline").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"before going offline")).await;
    drop(person);
    lab.stop("person");

    agent.apply(Input::Started { rollback: true }).await;
    agent.apply(Input::Reply("held while the phone is offline")).await;
    // The manager supplies these every 30 s. Drive the same inputs without
    // waiting a minute: the phone cannot consume any queued keys yet.
    agent.apply(Input::Resync).await;
    agent.apply(Input::Resync).await;
    // An awake save stops and restarts the core while the phone is away.
    let saved = agent.link.to_json();
    drop(agent);
    lab.stop("agent");
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved).expect("the link survives the save");
    agent.apply(Input::Started { rollback: false }).await;
    agent.apply(Input::Resync).await;
    let mut person = lab.start("person", false).await;
    let decrypt_error = |e: &Value| e["chatItems"].as_array().is_some_and(|items| items.iter().any(|i| i["chatItem"]["content"]["type"] == "rcvDecryptionError")) || e["chatItem"]["chatItem"]["content"]["type"] == "rcvDecryptionError";
    agent.until(Duration::from_secs(30), |a| !a.link.syncing || a.seen.iter().any(decrypt_error)).await;
    assert!(!agent.seen.iter().any(decrypt_error), "recovery introduced a decryption error; synthetic frames: {}", lab.dir.display());
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "held while the phone is offline")).await;
    assert!(!person.events.iter().any(decrypt_error), "the phone saw a decryption error; synthetic frames: {}", lab.dir.display());
    say(&mut person, at_person, "after coming online").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"after coming online")).await;
    agent.apply(Input::Reply("answered after coming online")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after coming online")).await;
    // Read both clients' whole synthetic event logs: `received` consumes
    // other events too, so the remaining queue alone cannot prove this.
    for log in [&agent.api.log, &person.log] {
        for line in std::fs::read_to_string(log).expect("the client's frames").lines() {
            let v: Value = serde_json::from_str(line).expect("a logged frame");
            assert!(!decrypt_error(&v["frame"]["resp"]), "a decryption error in {}", log.display());
        }
    }
    let commands = std::fs::read_to_string(&agent.api.log).expect("the agent's commands");
    assert_eq!(commands.lines().filter(|line| {
        let v: Value = serde_json::from_str(line).expect("a logged frame");
        v["dir"] == "out" && v["frame"]["cmd"].as_str().is_some_and(|c| c.starts_with("/_sync "))
    }).count(), 1, "exactly one negotiation, through timer ticks and a core restart");

    // Inject the earlier bug using raw core commands. The fixed bridge
    // prevents this damage; a link already damaged this way can remain
    // stuck in started/agreed. Verify that relinking restores messaging.
    drop(person);
    lab.stop("person");
    let contact = agent.link.contact().expect("still linked");
    for _ in 0..3 {
        let r = agent.api.cmd(&wire::Cmd::Sync { contact, force: true }.line()).await;
        assert_eq!(tag(&r), "contactRatchetSyncStarted");
    }
    agent.apply(Input::Started { rollback: false }).await;
    let mut person = lab.start("person", false).await;
    agent.until(Duration::from_secs(30), |a| a.seen.iter().any(decrypt_error)).await;
    agent.apply(Input::Unlink).await;
    agent.apply(Input::Want).await;
    let Phase::Inviting { link, .. } = agent.link.phase.clone() else { panic!("a fresh invitation") };
    assert_eq!(tag(&person.cmd(&format!("/connect {link}")).await), "sentConfirmation");
    agent.until(Duration::from_secs(30), |a| a.link.contact().is_some()).await;
    let at_person = person.wait(Duration::from_secs(30), |e| tag(e) == "contactConnected" && e["contact"]["contactId"] != at_person).await["contact"]["contactId"].as_i64().expect("the new contact");
    say(&mut person, at_person, "the relinked phone answers").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"the relinked phone answers")).await;
    agent.apply(Input::Reply("answered after relinking")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after relinking")).await;
}

/// The voice message `text`'s sender sent with file `name`: its file's id,
/// protocol and status as the receiving core said them (`seen`, newest last).
fn voice_item<'a>(seen: impl Iterator<Item = &'a Value>, name: &str) -> Option<(i64, String, String)> {
    seen.filter(|e| tag(e) == "newChatItems").flat_map(|e| e["chatItems"].as_array().cloned().unwrap_or_default()).find_map(|ci| {
        let item = &ci["chatItem"];
        let file = &item["file"];
        (item["content"]["msgContent"]["type"] == "voice" && file["fileName"] == name).then(|| (file["fileId"].as_i64().expect("its file id"), file["fileProtocol"].as_str().unwrap_or("?").to_string(), file["fileStatus"]["type"].as_str().unwrap_or("?").to_string()))
    })
}

/// Goal (a reply's voice message): the length the bridge reads of a real
/// encoder's audio is what ffprobe reads, to the second: MP3s as Hermes'
/// voices make them (constant and variable bit rates, MPEG-1 at 44.1 kHz
/// and MPEG-2 at 24 kHz) and MPEG-4 AAC as SimpleX's apps record. Needs the
/// host's ffmpeg.
#[test]
#[ignore = "needs ffmpeg; run by hand"]
fn real_encoders_lengths_read() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("voice-lengths");
    std::fs::create_dir_all(&dir).expect("a directory");
    for (name, secs, args, media_type) in [
        ("cbr.mp3", 7, vec!["-ar", "44100", "-b:a", "128k"], "audio/mpeg"),
        ("vbr.mp3", 12, vec!["-ar", "44100", "-q:a", "4"], "audio/mpeg"),
        ("tts.mp3", 5, vec!["-ar", "24000", "-ac", "1", "-b:a", "48k"], "audio/mpeg"),
        ("app.m4a", 9, vec!["-ar", "16000", "-ac", "1", "-c:a", "aac", "-b:a", "32k"], "audio/mp4"),
    ] {
        let path = dir.join(name);
        let src = format!("sine=frequency=330:duration={secs}");
        let out = Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", &src]).args(&args).arg(&path).output().expect("the host has ffmpeg");
        assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        let read = fragment_bridge::simplex::audio::secs(media_type, &std::fs::read(&path).expect("made"));
        let probed = Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(&path).output().expect("ffprobe");
        let probed: f64 = String::from_utf8_lossy(&probed.stdout).trim().parse().expect("a length");
        println!("{name}: read {read:?} s, ffprobe {probed:.3} s");
        assert_eq!(read, Some(probed.round() as u32), "{name}");
    }
}

/// Receives file `id` at a core whose events go to `events`: `/freceive`,
/// then its `rcvFileComplete`, within `within`; how long it took.
async fn freceive(api: &mut Api, id: i64, within: Duration) -> Duration {
    let t0 = Instant::now();
    let r = api.cmd(&format!("/freceive {id}")).await;
    assert!(!tag(&r).contains("rror"), "/freceive {id}: {r}");
    api.wait(within, |e| tag(e) == "rcvFileComplete" && e["chatItem"]["chatItem"]["file"]["fileId"] == id).await;
    t0.elapsed()
}

/// Goal: voice both ways against the real core, through the link: a
/// person's voice message reaches the agent as the link's `voice` forward
/// and its file arrives whole, inline (under 255 KB) and over XFTP (past
/// it); the agent's voice reply reaches the person whole. Needs the host's
/// ffmpeg.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and ffmpeg; run by hand"]
async fn voice_against_the_real_core() {
    let mut lab = Lab::up_with("voice", true);
    let mut agent = Agent::new(lab.start("agent", true).await);
    let mut person = lab.start("person", false).await;
    agent.apply(Input::Want).await;
    let Phase::Inviting { link, .. } = agent.link.phase.clone() else { panic!("an invitation") };
    person.cmd(&format!("/connect {link}")).await;
    agent.until(Duration::from_secs(90), |a| a.link.contact().is_some()).await;
    let at_agent = agent.link.contact().expect("linked");
    let at_person = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");

    for (name, secs, noise) in [("short.m4a", 20, false), ("long.m4a", 150, true)] {
        let file = lab.db_dir("person").join("files").join(name);
        voice_file(&file, secs, noise);
        let bytes = std::fs::metadata(&file).expect("made").len();
        let body = json!([{ "filePath": format!("/data/files/{name}"), "msgContent": { "type": "voice", "text": "", "duration": secs } }]);
        let r = person.cmd(&format!("/_send @{at_person} json {body}")).await;
        assert_eq!(tag(&r), "newChatItems", "the person's voice message: {r}");
        let t0 = Instant::now();
        agent.until(Duration::from_secs(60), |a| a.forwarded.iter().any(|i| i.kind == "voice") && voice_item(a.seen.iter(), name).is_some()).await;
        let voice = agent.forwarded.iter().rev().find(|i| i.kind == "voice").expect("forwarded").clone();
        let (id, protocol, status) = voice_item(agent.seen.iter(), name).expect("its file");
        println!("{name}: {bytes} bytes, {secs} s; the item in {} ms, its file {protocol} {status}, forwarded as {:?} {:?}", t0.elapsed().as_millis(), voice.kind, voice.text);
        agent.forwarded.retain(|i| i.kind != "voice");
        person.drain(Duration::from_secs(5)).await;
        let theirs: Vec<String> = person.events.drain(..).map(|e| tag(&e)).filter(|t| t.contains("ile") || t.contains("rror")).collect();
        println!("  the sender's file events: {theirs:?}");
        // an XFTP file is received once its description is ready
        if protocol == "xftp" {
            let ready = |e: &Value| tag(e) == "rcvFileDescrReady" && e["chatItem"]["chatItem"]["file"]["fileId"] == id;
            if !agent.seen.iter().any(ready) {
                agent.api.wait(Duration::from_secs(60), ready).await;
            }
        }
        let took = freceive(&mut agent.api, id, Duration::from_secs(120)).await;
        let got = lab.db_dir("agent").join("files").join(name);
        assert_eq!(sha256_of(&got), sha256_of(&file), "{name} arrives whole");
        println!("  received whole in {} ms", took.as_millis());
    }

    // the agent's voice reply (a speech route's audio, made AAC as the apps make it)
    let reply = lab.db_dir("agent").join("files").join("reply.m4a");
    voice_file(&reply, 30, false);
    let body = json!([{ "filePath": "/data/files/reply.m4a", "msgContent": { "type": "voice", "text": "", "duration": 30 } }]);
    let r = agent.api.cmd(&format!("/_send @{at_agent} json {body}")).await;
    assert_eq!(tag(&r), "newChatItems", "the agent's voice reply: {r}");
    let item = person.wait(Duration::from_secs(60), |e| tag(e) == "newChatItems" && voice_item(std::iter::once(e), "reply.m4a").is_some()).await;
    let (id, protocol, _) = voice_item(std::iter::once(&item), "reply.m4a").expect("its file");
    let took = freceive(&mut person, id, Duration::from_secs(120)).await;
    assert_eq!(sha256_of(&lab.db_dir("person").join("files").join("reply.m4a")), sha256_of(&reply), "the reply arrives whole");
    println!("the agent's voice reply ({protocol}) received whole in {} ms", took.as_millis());
}

// ---- the bridge's SimpleX, as a computer runs it ----

/// The lab's computer, a container of `BRIDGE_IMAGE` on the lab's network:
/// its fragment API the fake on this host, its `/data` the directory
/// `data` here, its screen port on this host's loopback.
struct Computer {
    name: String,
    port: u16,
    /// The nonce of the link ticket its page was handed last: the session
    /// its invitation is shown to.
    session: std::sync::Mutex<String>,
}

impl Computer {
    fn start(lab: &Lab, fake: &support::fake::Fake, data: &Path) -> Computer {
        let smp = format!("BRIDGE_SIMPLEX_SMP={}", lab.server);
        let xftp = format!("BRIDGE_SIMPLEX_XFTP={}", lab.xftp.as_deref().expect("a lab with files"));
        Computer::start_with(lab, fake, data, &["-e", &smp, "-e", &xftp])
    }

    /// `start`, its cores on SimpleX's preset servers, as a computer's are.
    fn start_on_presets(lab: &Lab, fake: &support::fake::Fake, data: &Path) -> Computer {
        Computer::start_with(lab, fake, data, &[])
    }

    fn start_with(lab: &Lab, fake: &support::fake::Fake, data: &Path, servers: &[&str]) -> Computer {
        let name = lab.container("computer");
        let _ = docker(&["rm", "-f", &name]);
        std::fs::create_dir_all(data).expect("its /data");
        let (net, ids, vol) = (format!("{}-net", lab.prefix), uid_gid(), format!("{}:/data", data.display()));
        let api = format!("FRAGMENT_API=http://api.fragment.internal:{}", fake.addr.port());
        let mut args = vec!["run", "-d", "--name", &name, "--network", &net, "--add-host", "api.fragment.internal:host-gateway", "--user", &ids, "-e", "HOME=/data", "-e", &api];
        args.extend(servers);
        args.extend(["-e", "FRAGMENT_COMPUTER=computer:00aa", "-v", &vol, "-p", "127.0.0.1::6080", BRIDGE_IMAGE]);
        docker(&args).expect("the computer starts");
        let port = docker(&["port", &name, "6080/tcp"]).expect("its port").lines().next().and_then(|l| l.rsplit_once(':')).and_then(|(_, p)| p.parse().ok()).expect("a port");
        Computer { name, port, session: std::sync::Mutex::new(String::new()) }
    }

    /// `method` on its screen port, as its page asks (with its session):
    /// the status and the JSON answered.
    fn http(&self, method: &str, path: &str) -> (u16, Value) {
        self.http_with(method, path, None)
    }

    /// `http` as another page would ask (the agent's tools, say): no session.
    fn http_bare(&self, method: &str, path: &str) -> (u16, Value) {
        self.http_as("", method, path, None)
    }

    fn http_with(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let session = self.session.lock().expect("its session").clone();
        self.http_as(&session, method, path, body)
    }

    fn http_as(&self, session: &str, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let url = format!("http://127.0.0.1:{}{path}{}", self.port, if session.is_empty() { String::new() } else { format!("&s={session}") });
        let sent = body.map(Value::to_string).unwrap_or_default();
        let mut args = vec!["-s", "-X", method, "-w", "\n%{http_code}", url.as_str()];
        if body.is_some() {
            args.extend(["-H", "content-type: application/json", "--data-binary", sent.as_str()]);
        }
        let out = Command::new("curl").args(&args).output().expect("curl runs");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let (body, code) = text.rsplit_once('\n').unwrap_or(("", "0"));
        (code.trim().parse().unwrap_or(0), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    /// Its owner links juniper from their Settings: with the ticket the
    /// platform (the fake) signed as the agent's key.
    fn link(&self, fake: &support::fake::Fake) -> (u16, Value) {
        let t = fake.ticket("juniper", "link");
        *self.session.lock().expect("its session") = t["nonce"].as_str().expect("a nonce").to_string();
        self.http_with("POST", "/simplex/link?agent=juniper--k3x9", Some(&t))
    }

    /// Its owner unlinks juniper from their Settings.
    fn unlink(&self, fake: &support::fake::Fake) -> (u16, Value) {
        self.http_with("POST", "/simplex/unlink?agent=juniper--k3x9", Some(&fake.ticket("juniper", "unlink")))
    }

    /// Its SimpleX state, once `done` holds (within `within`).
    async fn state_until(&self, within: Duration, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let t0 = Instant::now();
        // bounded by `within`
        loop {
            let (code, v) = self.http("GET", "/simplex/state?agent=juniper--k3x9");
            if code == 200 && done(&v) {
                return v;
            }
            assert!(t0.elapsed() < within, "{what}: not within {within:?}; last {code} {v}\n{}", wire::cut(&logs(&self.name), 6_000));
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    fn exec(&self, cmd: &str) -> String {
        docker(&["exec", &self.name, "sh", "-c", cmd]).unwrap_or_default()
    }

    /// How many SimpleX cores run in it now.
    fn cores(&self) -> usize {
        self.exec("for c in /proc/[0-9]*/comm; do cat $c 2>/dev/null; done").lines().filter(|l| l.trim() == "simplex-chat").count()
    }

    fn events(&self, kind: &str) -> Vec<Value> {
        logs(&self.name).lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["event"] == kind).collect()
    }

    /// SIGTERM, as a sleep's stop sends it; gone.
    fn stop(&self) {
        let _ = std::fs::write(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{}.log", self.name)), logs(&self.name));
        docker(&["stop", "-t", "10", &self.name]).expect("it stops");
        docker(&["rm", "-f", &self.name]).expect("it goes");
    }
}

impl Drop for Computer {
    fn drop(&mut self) {
        let _ = std::fs::write(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{}.log", self.name)), logs(&self.name));
        let _ = docker(&["rm", "-f", &self.name]);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).expect("a directory");
    for e in std::fs::read_dir(from).expect("a directory").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type().expect("its type").is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copied");
        }
    }
}

fn build_bridge_image() {
    if std::env::var("FRAGMENT_DOCKER_SKIP_BUILD").is_ok() {
        return;
    }
    let images = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("images/").to_path_buf();
    let status = Command::new("docker").args(["build", "--platform", "linux/amd64", "-q", "-f", "bridge/tests/simplex/bridge.Dockerfile", "-t", BRIDGE_IMAGE, "."]).current_dir(images).status().expect("docker runs");
    assert!(status.success(), "the lab's computer builds");
}

/// Goal: the bridge runs a linked agent's core as the platform's lifecycle
/// asks, against the real core: linked from its routes (the owner's link
/// asked, the invitation shown, the person's app bound, greeted, a save
/// asked); stopped under a hold before the bridge answers it, its databases
/// whole, and started again after; stopped while the computer is not always
/// on, and started again when it is; through a planned restart, as it was;
/// and after a rollback of its `/data`, its forced sync heals the link
/// without sending a chat announcement, and both directions work.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn the_bridge_runs_a_linked_core() {
    build_bridge_image();
    let mut lab = Lab::up_with("bridge", true);
    let mut person = lab.start("person", false).await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let data = lab.dir.join("computer-data");
    let c = Computer::start(&lab, &fake, &data);
    let v = c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    assert_eq!(v["alwaysOn"], true);
    assert_eq!(c.cores(), 0, "nothing linked, no core");

    // linked from its routes: only with its owner's ticket (the agent's
    // tools reach this port too, and hold none)
    let (code, v) = c.http("POST", "/simplex/link?agent=juniper--k3x9");
    assert_eq!((code, v["error"].as_str()), (403, Some("forbidden")), "no ticket, no link: {v}");
    let mut forged = fake.ticket("juniper", "link");
    forged["sig"] = json!("ab".repeat(64));
    let (code, _) = c.http_with("POST", "/simplex/link?agent=juniper--k3x9", Some(&forged));
    assert_eq!(code, 403, "a forged ticket");
    assert_eq!(c.state_until(Duration::from_secs(5), "still off", |v| v["phase"] == "off").await["phase"], "off");
    let (code, _) = c.link(&fake);
    assert_eq!(code, 200);
    let v = c.state_until(Duration::from_secs(60), "an invitation", |v| v["phase"] == "inviting").await;
    let link = v["link"].as_str().expect("its link").to_string();
    let (_, unseen) = c.http_bare("GET", "/simplex/state?agent=juniper--k3x9");
    assert_eq!((unseen["phase"].as_str(), unseen.get("link"), unseen["hidden"].as_bool()), (Some("inviting"), None, Some(true)), "another page sees no invitation: {unseen}");
    assert!(fake.with(|w| w.doors.contains_key("juniper--k3x9")), "its door's key, registered with the ticket");
    assert_eq!(c.exec("stat -c %a /data/simplex/juniper--k3x9/door.key").trim(), "600", "its private half readable by the bridge's user alone");
    let secret = c.exec("cat /data/simplex/juniper--k3x9/door.key").trim().to_string();
    assert_eq!(secret.len(), 64);
    assert!(!c.exec("for e in /proc/[0-9]*/environ; do tr '\\0' '\\n' < $e; done 2>/dev/null").contains(&secret), "in no process's environment");
    assert!(link.starts_with("simplex:/invitation#"));
    let r = person.cmd(&format!("/connect {link}")).await;
    assert_eq!(tag(&r), "sentConfirmation", "{r}");
    let v = c.state_until(Duration::from_secs(90), "linked", |v| v["phase"] == "linked").await;
    assert_eq!(v["name"], "person");
    let at_person = person.wait(Duration::from_secs(60), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    let greeted = received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("You're linked"))).await;
    assert!(greeted.iter().any(|t| t.starts_with("Hi, it's Juniper.")), "named by its title: {greeted:?}");
    fake.until(20_000, "a save asked as the link was made", |w| w.keepalive_log.windows(2).any(|p| p == [true, false])).await;
    assert_eq!(c.cores(), 1);

    // the hold: stopped before it is answered, whole, started again after
    c.exec("touch /run/computer/hold");
    let t0 = Instant::now();
    // bounded: 20 s, the platform's wait for an answer
    while c.exec("test -e /run/computer/held && echo yes").trim() != "yes" {
        assert!(t0.elapsed() < Duration::from_secs(20), "the hold answered within 20 s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("the hold answered in {} ms, its core stopped first", t0.elapsed().as_millis());
    assert_eq!(c.cores(), 0, "no core runs under the hold");
    let files: Vec<String> = c.exec("ls /data/simplex/juniper--k3x9").lines().map(str::to_string).collect();
    assert!(files.contains(&"sx_chat.db".into()) && files.contains(&"sx_agent.db".into()) && files.contains(&"link.json".into()), "{files:?}");
    assert!(!files.iter().any(|f| f.ends_with("-wal")), "whole: {files:?}");
    c.exec("rm -f /run/computer/hold /run/computer/held");
    c.state_until(Duration::from_secs(30), "running again", |v| v["running"] == true).await;
    say(&mut person, at_person, "after the hold").await;
    let t0 = Instant::now();
    // bounded by 30 s
    while !c.events("simplex.forward").iter().any(|e| e["kind"] == "text") {
        assert!(t0.elapsed() < Duration::from_secs(30), "the message after the hold reached the bridge");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // not always on: no core; always on again: its core again
    fake.with(|w| w.computer["alwaysOn"] = json!(false));
    let v = c.state_until(Duration::from_secs(75), "stopped, not always on", |v| v["running"] == false && v["alwaysOn"] == false).await;
    assert!(v["why"].as_str().is_some_and(|w| w.contains("always-on")), "{v}");
    assert_eq!(c.cores(), 0);
    fake.with(|w| w.computer["alwaysOn"] = json!(true));
    c.state_until(Duration::from_secs(75), "running, always on again", |v| v["running"] == true).await;

    // a planned restart (SIGTERM, as a sleep's stop), then its save: kept as it was
    c.stop();
    let save = lab.dir.join("computer-save");
    copy_tree(&data, &save);
    fake.with(|w| {
        w.computer["generation"] = json!(2);
        w.computer["restored"] = json!({ "generation": 2, "rollback": false, "from": "backup", "at": 1 });
    });
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "linked and running after a restart", |v| v["phase"] == "linked" && v["running"] == true).await;
    let before = c.events("simplex.forward").len();
    say(&mut person, at_person, "said after the save").await;
    let t0 = Instant::now();
    // bounded by 30 s
    while c.events("simplex.forward").len() <= before {
        assert!(t0.elapsed() < Duration::from_secs(30), "a message after the restart reached the bridge");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(c.events("simplex.core_up").iter().all(|e| e["rollback"] == false), "a restart that saved forces nothing");

    // a rollback: the save before that message, restored, as a crash's next start
    c.stop();
    copy_tree(&save, &data);
    fake.with(|w| {
        w.computer["generation"] = json!(3);
        w.computer["restored"] = json!({ "generation": 3, "rollback": true, "from": "backup", "at": 1 });
    });
    let t0 = Instant::now();
    let c = Computer::start(&lab, &fake, &data);
    recovered(&mut person, at_person, Duration::from_secs(60), || {
        c.events("simplex.told").iter().any(|e| e["agent"] == "juniper--k3x9" && e["notice"] == "Resynced")
    }).await;
    println!("after a rollback, encryption recovered {} ms after the start", t0.elapsed().as_millis());
    assert!(c.events("simplex.core_up").iter().any(|e| e["rollback"] == true), "its start forced a sync");
    let before = c.events("simplex.forward").len();
    say(&mut person, at_person, "after the rollback").await;
    let t0 = Instant::now();
    // bounded by 30 s
    while c.events("simplex.forward").len() <= before {
        assert!(t0.elapsed() < Duration::from_secs(30), "a message after the rollback reached the bridge");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // a bridge started again in the same start forces nothing twice
    c.stop();
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "running again", |v| v["running"] == true).await;
    assert!(c.events("simplex.core_up").iter().all(|e| e["rollback"] == false), "the same start's rollback is handled once");

    // unlinked from its routes, by its owner's ticket: its contact let go,
    // at the person's too, its door closed at the platform and its key gone
    let (code, _) = c.http("POST", "/simplex/unlink?agent=juniper--k3x9");
    assert_eq!(code, 403, "no ticket, no unlink");
    let (code, v) = c.unlink(&fake);
    assert_eq!((code, v["phase"].as_str()), (200, Some("off")));
    assert!(fake.with(|w| !w.doors.contains_key("juniper--k3x9")), "its door closed at the platform");
    assert!(c.exec("ls /data/simplex/juniper--k3x9").lines().all(|f| f != "door.key"), "its key gone");
    person.wait(Duration::from_secs(30), |e| tag(e) == "contactDeletedByContact").await;
    c.state_until(Duration::from_secs(30), "its core stopped", |v| v["running"] == false).await;
    // an agent this bridge does not run has no link here
    let (code, _) = c.http("GET", "/simplex/state?agent=nobody--k3x9");
    assert_eq!(code, 404);
}

/// The relayed records on `chat` (an agent's, carrying its owner's words).
fn relayed_on(fake: &support::fake::Fake, chat: &str) -> Vec<Value> {
    fake.with(|w| w.records(chat, "chat").into_iter().filter(|r| r["body"]["relayed"]["via"] == "simplex").collect())
}

/// Goal: messages both ways through the agent's SimpleX chat with its
/// owner (Paul, 2026-10-26: a chat of its own), against the real core: the
/// link makes the chat for its owner; the owner's SimpleX message is a
/// relayed record there, by the agent, for its owner, by its sender's id,
/// and a turn asked as its owner (the scripted runtime's echo names them);
/// that turn's reply reaches their app, and so does a reply to words typed
/// in that chat on the web, and a routine with no chat while their home is
/// that chat; a reply in the agent's own chat does not; a voice message is
/// the record's attachment, whole, as an `audio/mp4`; and a start again
/// (its catch-up) posts nothing twice and makes no second chat.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and ffmpeg; run by hand"]
async fn messages_go_both_ways_through_the_chat() {
    build_bridge_image();
    let mut lab = Lab::up_with("messages", true);
    let mut person = lab.start("person", false).await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    let own = fake.chat_named("juniper-chat--h6j7", &["juniper"]);
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let data = lab.dir.join("computer-data");
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    c.link(&fake);
    let v = c.state_until(Duration::from_secs(60), "an invitation", |v| v["phase"] == "inviting").await;
    person.cmd(&format!("/connect {}", v["link"].as_str().expect("its link"))).await;
    c.state_until(Duration::from_secs(90), "linked", |v| v["phase"] == "linked").await;
    let at_person = person.wait(Duration::from_secs(60), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("You're linked"))).await;
    fake.until(30_000, "its SimpleX chat made for its owner", |w| w.made.len() == 1).await;
    let chat = fake.with(|w| w.made[0].clone());
    assert!(chat.starts_with("juniper-simplex--"), "{chat}");
    fake.with(|w| assert_eq!(w.fragments[&chat].title, "SimpleX"));

    // the owner's message: a relayed record in its SimpleX chat, a turn as theirs, its reply back
    let t0 = Instant::now();
    let sent_id = say(&mut person, at_person, "hello from my phone").await;
    let got = received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "echo: [paul] hello from my phone")).await;
    println!("SimpleX message to the agent's reply on SimpleX: {} ms ({got:?})", t0.elapsed().as_millis());
    let r = relayed_on(&fake, &chat);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["principal"], "npub1juniper", "posted by the agent");
    assert_eq!(r[0]["body"]["relayed"]["for"], "npub1paul", "for its owner");
    assert_eq!(r[0]["body"]["text"], "hello from my phone");
    assert_eq!(r[0]["body"]["to"], json!(["npub1juniper"]));
    assert!(relayed_on(&fake, &own).is_empty(), "never in its own chat");
    let id = fragment_bridge::records::relayed_id("juniper--k3x9", &sent_id);
    assert_eq!(fake.with(|w| w.fragments[&chat].channels["chat"].ids.get(&id).map(|(seq, _)| *seq)), r[0]["seq"].as_u64(), "by its sender's id");

    // words typed in its SimpleX chat on the web: answered there, and on the phone
    fake.say(&chat, "npub1paul", json!({ "text": "typed on the web" }));
    received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "echo: [paul] typed on the web")).await;
    // a routine with no chat, its owner's home that chat: on the phone
    fake.with(|w| {
        w.fragments.get_mut("juniper--k3x9").expect("its fragment").files.insert("agent.json".into(), bytes::Bytes::from(r#"{"tier":"cheap","home":"simplex"}"#));
        w.append("juniper--k3x9", "tasks", "npub1juniper", json!({ "kind": "routine", "text": "remind me to water the basil" }));
    });
    received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "echo: [your routine] remind me to water the basil")).await;
    // words typed in its own chat: answered there alone
    fake.say(&own, "npub1paul", json!({ "text": "typed in its own chat" }));
    fake.until(30_000, "its own chat's reply", |w| w.bodies(&own, "chat", "reply").iter().any(|b| b["text"] == "echo: [paul] typed in its own chat")).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    person.drain(Duration::from_millis(500)).await;
    let late: Vec<String> = person
        .events
        .iter()
        .filter(|e| tag(e) == "newChatItems")
        .flat_map(|e| e["chatItems"].as_array().cloned().unwrap_or_default())
        .filter_map(|ci| ci["chatItem"]["content"]["msgContent"]["text"].as_str().map(str::to_string))
        .collect();
    assert!(!late.iter().any(|t| t.contains("typed in its own chat")), "a reply in its own chat stays there: {late:?}");

    // a voice message: its file, whole, the record's attachment
    let voice = lab.db_dir("person").join("files").join("memo.m4a");
    voice_file(&voice, 20, false);
    let body = json!([{ "filePath": "/data/files/memo.m4a", "msgContent": { "type": "voice", "text": "", "duration": 20 } }]);
    let r = person.cmd(&format!("/_send @{at_person} json {body}")).await;
    assert_eq!(tag(&r), "newChatItems", "{r}");
    let t0 = Instant::now();
    fake.until(60_000, "the voice message's record", |w| w.records(&chat, "chat").iter().any(|r| r["body"]["relayed"]["via"] == "simplex" && r["body"]["attachments"].is_array())).await;
    println!("a voice message on the agent's chat: {} ms after it was sent", t0.elapsed().as_millis());
    let rec = relayed_on(&fake, &chat).into_iter().find(|r| r["body"]["attachments"].is_array()).expect("its record");
    let a = &rec["body"]["attachments"][0];
    assert_eq!(a["type"], "audio/mp4");
    assert_eq!(a["sha256"].as_str(), Some(sha256_of(&voice).as_str()), "whole");
    assert_eq!(a["size"].as_u64(), std::fs::metadata(&voice).ok().map(|m| m.len()));
    assert!(fake.with(|w| w.fragments[&chat].blobs.contains_key(a["sha256"].as_str().unwrap())), "its blob uploaded");
    let got = received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t.starts_with("echo: [paul]") && t.contains("memo.m4a"))).await;
    println!("the voice message answered on SimpleX: {got:?} (the runtime was handed its file)");
    assert!(c.exec("ls /data/simplex/juniper--k3x9/files").trim().is_empty(), "its file let go once carried");

    // a reply's files: its text, then a voice message and a file in the
    // person's app, each the chat's own attachment, whole
    let t0 = Instant::now();
    say(&mut person, at_person, "speak and draw").await;
    reply_files_arrive(&lab, &mut person, &fake, &chat, &c).await;
    println!("a reply's voice message and file, whole in the person's app: {} ms after they asked", t0.elapsed().as_millis());

    // a start again (SIGTERM, a save, a start): its catch-up posts nothing twice
    let before = relayed_on(&fake, &chat).len();
    c.stop();
    fake.with(|w| w.computer["generation"] = json!(2));
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "running again", |v| v["running"] == true).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(relayed_on(&fake, &chat).len(), before, "the catch-up's forwards are replays");
    say(&mut person, at_person, "after the restart").await;
    received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "echo: [paul] after the restart")).await;
    fake.with(|w| assert_eq!(w.made.as_slice(), std::slice::from_ref(&chat), "its SimpleX chat found again, never made twice"));
}

/// The scripted agent's answer to "speak and draw", in the person's app: its
/// text, then a voice message (its length, playable: the host's ffprobe
/// decodes it) and a file, each the chat's own attachment byte for byte;
/// and the computer's copies of them let go once uploaded.
async fn reply_files_arrive(lab: &Lab, person: &mut Api, fake: &support::fake::Fake, chat: &str, c: &Computer) {
    use fragment_bridge::runtime::script::SPOKEN_SECS;
    let files = lab.db_dir("person").join("files");
    let within = Duration::from_secs(180);
    let (voice, heard) = received_file(person, &files, "voice", within).await;
    let (file, drawn) = received_file(person, &files, "file", within).await;
    received(person, within, |got| got.iter().any(|t| t == "echo: [paul] speak and draw")).await;
    println!("  a voice message {voice} ({} bytes) and a file {file} ({} bytes)", std::fs::metadata(&heard).map_or(0, |m| m.len()), std::fs::metadata(&drawn).map_or(0, |m| m.len()));
    assert_eq!((voice["type"].as_str(), voice["duration"].as_u64()), (Some("voice"), Some(u64::from(SPOKEN_SECS))), "a voice message, its length: {voice}");
    assert_eq!(file["type"], "file", "{file}");
    let reply = fake.with(|w| w.bodies(chat, "chat", "reply").into_iter().find(|b| b["text"] == "echo: [paul] speak and draw")).expect("the chat's reply");
    let shas: Vec<&str> = reply["attachments"].as_array().expect("its files").iter().filter_map(|a| a["sha256"].as_str()).collect();
    for got in [&heard, &drawn] {
        assert!(shas.contains(&sha256_of(got).as_str()), "{} is the chat's own attachment, whole: {shas:?}", got.display());
    }
    assert_eq!(std::fs::read(&heard).expect("the voice"), fragment_bridge::simplex::audio::silence_mp3(SPOKEN_SECS));
    let probe = Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration:stream=codec_name", "-of", "csv=p=0"]).arg(&heard).output().expect("the host has ffprobe");
    let probed = String::from_utf8_lossy(&probe.stdout).to_string();
    assert!(probe.status.success() && probed.contains("mp3"), "it plays: {probed}");
    println!("  ffprobe: {}", probed.split_whitespace().collect::<Vec<_>>().join(" "));
    let t0 = Instant::now();
    // bounded: a minute, the uploads' ends having come already
    while !c.exec("ls -A /data/simplex/juniper--k3x9/out 2>/dev/null").trim().is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(60), "the copies let go: {}", c.exec("ls -R /data/simplex/juniper--k3x9/out"));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Goal (a reply's files, through the relay): a reply's voice message and
/// file reach a person on SimpleX's own preset servers, uploaded by the
/// agent's core through the bridge's proxy and a relay that pipes bytes to
/// the real XFTP servers on 443, as a computer on Cloudflare must (never
/// straight out); its SMP straight out on 5223. Lower rung, labeled so: the
/// fake platform's relay (a byte pipe), not the cell's; the internet's
/// servers, SimpleX Chat's and Flux's.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker, ffprobe and the internet; run by hand"]
async fn reply_files_go_out_through_the_relay() {
    build_bridge_image();
    let mut lab = Lab::up("relay");
    let mut person = lab.start_on_presets("person").await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    let chat = fake.chat_named("juniper-chat--h6j7", &["juniper"]);
    let hosts: Vec<String> = ["simplex.im", "simplexonflux.com"].iter().flat_map(|d| (1..=6).map(move |n| format!("xftp{n}.{d}"))).collect();
    let mut to = std::collections::BTreeMap::new();
    for h in &hosts {
        let addr = tokio::net::lookup_host((h.as_str(), 443)).await.ok().and_then(|mut a| a.find(|a| a.is_ipv4())).unwrap_or_else(|| panic!("{h} resolves"));
        to.insert(h.clone(), addr);
    }
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
        w.computer["relay"] = json!({ "hosts": hosts, "port": 443 });
        w.relay_to = to;
    });
    let data = lab.dir.join("computer-data");
    let c = Computer::start_on_presets(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    c.link(&fake);
    let v = c.state_until(Duration::from_secs(120), "an invitation", |v| v["phase"] == "inviting").await;
    person.cmd(&format!("/connect {}", v["link"].as_str().expect("its link"))).await;
    c.state_until(Duration::from_secs(180), "linked", |v| v["phase"] == "linked").await;
    let at_person = person.wait(Duration::from_secs(120), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t.contains("You're linked"))).await;
    let t0 = Instant::now();
    say(&mut person, at_person, "speak and draw").await;
    reply_files_arrive(&lab, &mut person, &fake, &chat, &c).await;
    let (up, down, closed) = fake.with(|w| (w.relay_up, w.relay_down, w.relayed.clone()));
    println!("a reply's files through the relay to SimpleX's XFTP servers, whole in the person's app: {} ms; relayed {up} bytes up, {down} down; closed {closed:?}", t0.elapsed().as_millis());
    let sent = fragment_bridge::simplex::audio::silence_mp3(fragment_bridge::runtime::script::SPOKEN_SECS).len() as u64;
    assert!(up > sent, "the upload went through the relay: {up} bytes up, the voice alone {sent}");
}

/// What a QR code `svg` says, as the host's zbar reads it from its picture.
fn decode_qr(svg: &[u8], dir: &Path) -> String {
    let (svg_path, png) = (dir.join("qr.svg"), dir.join("qr.png"));
    std::fs::write(&svg_path, svg).expect("written");
    let made = Command::new("rsvg-convert").arg("-w").arg("480").arg("-o").arg(&png).arg(&svg_path).status().expect("the host has rsvg-convert");
    assert!(made.success(), "drawn as a picture");
    let out = Command::new("zbarimg").args(["-q", "--raw"]).arg(&png).output().expect("the host has zbarimg");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Goal: the link page, as the shell's pane shows it: served under its
/// port's path; its QR code says the one-time invitation (its short link,
/// the server's), and the person's app links by scanning it; then the page
/// says whom it is linked to.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker, rsvg-convert and zbarimg; run by hand"]
async fn the_link_pages_code_links() {
    build_bridge_image();
    let mut lab = Lab::up_with("page", true);
    let mut person = lab.start("person", false).await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let c = Computer::start(&lab, &fake, &lab.dir.join("computer-data"));
    c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    let page = Command::new("curl").args(["-s", &format!("http://127.0.0.1:{}/simplex/?agent=juniper--k3x9", c.port)]).output().expect("curl runs");
    let page = String::from_utf8_lossy(&page.stdout);
    assert!(page.contains("<title>SimpleX</title>") && page.contains("Link SimpleX"), "its page");
    let (code, _) = c.http("GET", "/simplex/qr.svg?agent=juniper--k3x9");
    assert_eq!(code, 404, "no code before an invitation");
    c.link(&fake);
    let v = c.state_until(Duration::from_secs(60), "an invitation", |v| v["phase"] == "inviting").await;
    let (code, _) = c.http_bare("GET", "/simplex/qr.svg?agent=juniper--k3x9");
    assert_eq!(code, 404, "no code for a page that did not ask for it");
    let session = c.session.lock().expect("its session").clone();
    let svg = Command::new("curl").args(["-s", &format!("http://127.0.0.1:{}/simplex/qr.svg?agent=juniper--k3x9&s={session}", c.port)]).output().expect("curl runs").stdout;
    let said = decode_qr(&svg, &lab.dir);
    let short = v["short"].as_str().expect("the lab's server makes a short link");
    assert_eq!(said, short, "its code says its short link");
    let r = person.cmd(&format!("/connect {said}")).await;
    println!("the person scanned the page's code: {}", tag(&r));
    let v = c.state_until(Duration::from_secs(90), "linked by its code", |v| v["phase"] == "linked").await;
    assert_eq!(v["name"], "person");
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("You're linked"))).await;
}

/// The agent's profile renamed to `name`, as the bridge asks it
/// (`Cmd::Profile`): the core's answer, read through the wire.
async fn rename(agent: &mut Agent, name: &str) -> Resp {
    let r = agent.api.cmd(&wire::Cmd::Profile { name: name.into() }.line()).await;
    wire::parse(&json!({ "corrId": "1", "resp": r }).to_string()).expect("the core's answer reads").resp
}

/// Goal (Paul, 2026-10-26: his SimpleX app showed his agent by the name it
/// had before he renamed it): why, and the cure, against the real core. A
/// profile is made once, by `--create-bot-display-name` at the core's first
/// start, and a start with another name keeps it; the bridge's `/p` renames
/// it, keeping it a bot's, and the core sends the new name to the linked
/// contact's app; asked again, it changes nothing and sends nothing. Every
/// name `display_name` makes of a title, however odd, the core takes as it
/// is (it refuses a `'`: invalidDisplayName).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn a_rename_reaches_the_owners_app() {
    use fragment_bridge::simplex::run::display_name;
    let mut lab = Lab::up("rename");
    let mut agent = Agent::new(lab.start("agent", true).await);
    let mut person = lab.start("person", false).await;
    agent.apply(Input::Want).await;
    let Phase::Inviting { link, .. } = agent.link.phase.clone() else { panic!("an invitation: {:?}", agent.link) };
    person.cmd(&format!("/connect {link}")).await;
    agent.until(Duration::from_secs(90), |a| a.link.contact().is_some()).await;
    let met = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await;
    assert_eq!(met["contact"]["profile"]["displayName"], "agent", "the name its first start made");
    let shown = |e: &Value| e["toContact"]["profile"].clone();

    // renamed: the person's app is told, and it is a bot's profile still
    let name = display_name(Some("Waffle. Bot"), "agent");
    assert_eq!(rename(&mut agent, &name).await, Resp::Profile { name: "Waffle Bot".into(), changed: true });
    let told = person.wait(Duration::from_secs(30), |e| tag(e) == "contactUpdated").await;
    assert_eq!((shown(&told)["displayName"].as_str(), shown(&told)["peerType"].as_str()), (Some("Waffle Bot"), Some("bot")), "{told}");
    // asked again: nothing changes, nothing is sent
    assert_eq!(rename(&mut agent, &name).await, Resp::Profile { name: "Waffle Bot".into(), changed: false });
    person.drain(Duration::from_secs(3)).await;
    assert!(!person.events.iter().any(|e| tag(e) == "contactUpdated"), "no second update: {:?}", person.events.iter().map(tag).collect::<Vec<_>>());

    // whatever its title, what the core keeps of it (`wire::valid_name`),
    // asked as it is, is taken as it is: never refused, never changed
    // (Alex's review: a combining mark only after a letter)
    let titles = ["Paul's Bot", "  Dr.   Who  ", "--x a--b _c @d #e f-_g", "日本語 ボット R2-D2 x² Ⅻ", "Zoë the 2nd", "алиса", "Waffle 🧇!", "Wa<ff>le >= 2 & co", "\u{93e}Bot", "Bot \u{93e}x", "a-\u{93e}b", "a\u{93e}", "Zoe\u{301}", "J ... Doe", &"long ".repeat(20)];
    for title in titles {
        let name = display_name(Some(title), "agent");
        let said = rename(&mut agent, &name).await;
        assert!(matches!(&said, Resp::Profile { name: n, .. } if *n == name), "{title:?} as {name:?}: {said:?}");
    }
    // and 200 titles drawn from every kind of char the rule tells apart
    // (letters of several scripts, digits, letter numbers and others,
    // marks, symbols of each kind, punctuation of each kind, the prohibited
    // ones, spaces of several kinds, controls), seeded: each the core takes
    // as `display_name` makes it
    let pool: Vec<char> = "aZé日ボалж7٣Ⅻ²½\u{11f04}\u{1c89}\u{2ebf0}🫩\u{301}\u{93e}\u{20dd}🧇©€±^.,;/\\#@'\"`~-_!?:()[]{}«»&*% \u{a0}\u{2003}\t\n\u{1}\u{200b}".chars().collect();
    let mut seed: u64 = 0x5eed_f0a9;
    for i in 0..200 {
        let title: String = (0..1 + i % 24)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                pool[(seed >> 33) as usize % pool.len()]
            })
            .collect();
        let name = display_name(Some(&title), "agent");
        let said = rename(&mut agent, &name).await;
        assert!(matches!(&said, Resp::Profile { name: n, .. } if *n == name), "{title:?} as {name:?}: {said:?}");
    }
    // its Unicode (Alex's review): 15.0's chars are letters and symbols to it,
    // and kept; 15.1's, 16.0's and 17.0's are none, and dropped as it drops them
    for (title, kept) in [("a\u{870}", "a\u{870}"), ("a\u{11f04}", "a\u{11f04}"), ("a\u{1e4d0}", "a\u{1e4d0}"), ("a🩵", "a🩵"), ("Bot \u{2ebf0}", "Bot"), ("Bot \u{2ffc}", "Bot"), ("Bot \u{1c89}", "Bot"), ("Bot 🫩", "Bot"), ("Bot \u{1faea}", "Bot")] {
        let name = display_name(Some(title), "agent");
        let said = rename(&mut agent, &name).await;
        assert!(name == kept && matches!(&said, Resp::Profile { name: n, .. } if *n == name), "{title:?} as {name:?}: {said:?}");
    }
    // what it refuses: the bridge's old names kept `.` and `'`, a mark
    // anywhere, and its newest chars
    for refused in ["Dr. Who", "Pauls.Bot", "\u{93e}Bot", "a-\u{93e}b", "a\u{2ebf0}", "Bot 🫩", "🫩", "a\u{1c89}"] {
        let r = agent.api.cmd(&format!("/p '{refused}'")).await;
        assert!(r.to_string().contains("invalidDisplayName") && !wire::name_ok(refused), "{refused}: {r}");
    }
    assert_eq!(rename(&mut agent, "Waffle Bot").await, Resp::Profile { name: "Waffle Bot".into(), changed: true });

    // a start with its first name keeps the profile it has: why a rename needs `/p`
    drop(agent);
    lab.stop("agent");
    let mut agent = Agent::new(lab.start("agent", true).await);
    let r = agent.api.cmd("/u").await;
    assert_eq!(r["user"]["profile"]["displayName"], "Waffle Bot", "--create-bot-display-name names only a new profile: {r}");
    assert_eq!(rename(&mut agent, "Waffle Bot").await, Resp::Profile { name: "Waffle Bot".into(), changed: false }, "a start's ask, idempotent");
    person.drain(Duration::from_secs(1)).await;
    let last = person.events.iter().rev().find(|e| tag(e) == "contactUpdated").map(shown);
    assert_eq!(last.as_ref().and_then(|p| p["displayName"].as_str()), Some("Waffle Bot"), "the person's app shows its name now");
    println!("frames in {}", lab.dir.display());
}
