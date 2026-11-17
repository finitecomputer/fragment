//! A move's stage, read by the bridge for its SimpleX identity
//! (simplex/stage.rs; FIN-463), against the fake platform's stage route:
//! by HTTP ranges, as the bound agent, from zips another writer made
//! (Python's `zipfile`, zip64 and deflated, as the mover's are).

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use bytes::Bytes;
use fragment_bridge::api::Api;
use fragment_bridge::simplex::stage::{self, Refusal, Staged};
use support::fake::Fake;

/// A zip of `files` (path, bytes), written by Python's `zipfile`:
/// deflated, every entry forced to zip64's form.
fn zip(dir: &Path, files: &[(&str, Vec<u8>)]) -> Bytes {
    let src = dir.join("src");
    let _ = std::fs::remove_dir_all(&src);
    let mut names = vec![];
    for (i, (name, bytes)) in files.iter().enumerate() {
        std::fs::create_dir_all(&src).expect("its folder");
        let at = src.join(format!("{i}"));
        std::fs::write(&at, bytes).expect("written");
        names.push(format!("{}={name}", at.display()));
    }
    let out = dir.join("stage.zip");
    let script = "import sys, zipfile\nwith zipfile.ZipFile(sys.argv[1], 'w', zipfile.ZIP_DEFLATED) as z:\n    for a in sys.argv[2:]:\n        src, name = a.split('=', 1)\n        with open(src, 'rb') as f, z.open(name, 'w', force_zip64=True) as w:\n            w.write(f.read())\n";
    let st = Command::new("python3").arg("-I").arg("-c").arg(script).arg(&out).args(&names).status().expect("python3 runs");
    assert!(st.success(), "the zip is written");
    Bytes::from(std::fs::read(&out).expect("read"))
}

/// `n` bytes no compressor shrinks (xorshift, from `seed`).
fn noise(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn db(seed: u64, n: usize) -> Vec<u8> {
    let mut v = b"SQLite format 3\0".to_vec();
    v.extend(noise(seed, n));
    v
}

fn home() -> Vec<(&'static str, Vec<u8>)> {
    vec![("agent/hermes-home/SOUL.md", b"# me".to_vec()), ("workspace/photos.bin", noise(9, 12_000_000))]
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("stage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a folder");
    d
}

/// Goal: the stage's SimpleX identity is set aside whole, by ranges, as its
/// bound agent alone, from a zip64 of deflated files; a stage without one
/// is no identity; one not whole, not SQLite, or not the contract's is
/// refused; a stage not the agent's, or not importing, is gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_stages_identity_is_set_aside_whole() {
    let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
    let api = Api::new(&fake.url()).expect("an api");
    let dir = scratch("whole");
    let (chat, agent) = (db(1, 5_000_000), db(2, 1_200_000));
    let mut files = home();
    files.extend([(stage::CHAT, chat.clone()), (stage::AGENT, agent.clone()), (stage::OWNER, br#"{"contact":3}"#.to_vec())]);
    let z = zip(&dir, &files);
    fake.with(|w| w.stages.insert("old1.p1.1".into(), ("juniper--k3x9".into(), z.clone())));
    let at = dir.join("juniper--k3x9");
    stage::begin(&at, "old1.p1.1").expect("begun");
    let got = stage::take_aside(&api, "juniper--k3x9", "old1.p1.1", &at).await;
    assert_eq!(got, Ok(Staged { stage: "old1.p1.1".into(), owner: 3 }));
    assert_eq!(std::fs::read(at.join("staged/sx_chat.db")).expect("kept"), chat, "whole");
    assert_eq!(std::fs::read(at.join("staged/sx_agent.db")).expect("kept"), agent);
    assert_eq!(stage::ready(&at), got.ok());
    let reads = fake.with(|w| w.stage_reads.clone());
    assert!(reads.iter().all(|(_, a, b)| b - a < 4 * 1024 * 1024), "by ranges, each bounded");
    let read: u64 = reads.iter().map(|(_, a, b)| b - a + 1).sum();
    assert!(read < z.len() as u64, "never the whole home: {read} of {}", z.len());

    // no SimpleX in it: nothing to keep
    fake.with(|w| w.stages.insert("old2.p1.1".into(), ("juniper--k3x9".into(), zip(&dir, &home()))));
    assert_eq!(stage::take_aside(&api, "juniper--k3x9", "old2.p1.1", &at).await, Err(Refusal::Absent));
    // another agent's stage, or one not importing: gone
    assert!(matches!(stage::take_aside(&api, "fred--k3x9", "old1.p1.1", &at).await, Err(Refusal::Gone(_))), "another agent's");
    assert!(matches!(stage::take_aside(&api, "juniper--k3x9", "old9.p1.1", &at).await, Err(Refusal::Gone(_))), "not importing");

    // not as the contract says: refused, nothing marked ready
    let bad = |chat: Vec<u8>, owner: &[u8], extra: Option<(&'static str, Vec<u8>)>| {
        let mut f = home();
        f.extend([(stage::CHAT, chat), (stage::AGENT, db(2, 100)), (stage::OWNER, owner.to_vec())]);
        f.extend(extra);
        f
    };
    for (i, (why, files)) in [
        ("not SQLite", bad(b"not a database at all".to_vec(), br#"{"contact":3}"#, None)),
        ("no owner", bad(db(1, 100), br#"{"contact":0}"#, None)),
        ("a journal", bad(db(1, 100), br#"{"contact":3}"#, Some((".finite-stage/simplex/chat.db-wal", b"w".to_vec())))),
    ]
    .into_iter()
    .enumerate()
    {
        let stage = format!("bad{i}.p1.1");
        fake.with(|w| w.stages.insert(stage.clone(), ("juniper--k3x9".into(), zip(&dir, &files))));
        let to = dir.join(format!("bad{i}"));
        stage::begin(&to, &stage).expect("begun");
        assert!(matches!(stage::take_aside(&api, "juniper--k3x9", &stage, &to).await, Err(Refusal::Invalid(_))), "{why}");
        assert_eq!(stage::ready(&to), None, "{why}: nothing kept as ready");
    }
    // a byte changed in a database's deflated bytes: not whole
    let mut broken = z.to_vec();
    let at_chat = broken.windows(stage::CHAT.len()).position(|w| w == stage::CHAT.as_bytes()).expect("its local header");
    broken[at_chat + stage::CHAT.len() + 20 + 5_000] ^= 0x55;
    fake.with(|w| w.stages.insert("broken.p1.1".into(), ("juniper--k3x9".into(), Bytes::from(broken))));
    let to = dir.join("broken");
    stage::begin(&to, "broken.p1.1").expect("begun");
    assert!(matches!(stage::take_aside(&api, "juniper--k3x9", "broken.p1.1", &to).await, Err(Refusal::Invalid(_))), "its CRC");
    assert_eq!(stage::ready(&to), None);
}
