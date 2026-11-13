//! OpenBao's state and config, and, in the tests marked `ignore`, the
//! pinned OpenBao itself (fetched into target/tools): `cargo test -p
//! fragment-devstack openbao -- --include-ignored`. How celld reads it,
//! the node's secrets backend, is celld's to test (its secrets.rs).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fragment-openbao-{name}-{}", crate::random_hex(6)))
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// The secrets one test seeds: a store name, and a value.
const SEEDED: [(&str, &str); 2] = [("fragment-host-secret", "hs-e2e-0123456789abcdef0123456789abcdef"), ("fragment-oidc-client-secret", "oidc-e2e-secret")];

// Goal: a state directory is made once, its secrets owner-only and of the
// shape OpenBao needs, and kept as they are by every later start; one made
// loose, cut short, or gone with its data still there is refused, saying
// which. Method: prepare twice, then spoil each in turn.
#[test]
fn prepare_makes_its_secrets_once_and_refuses_them_spoiled() {
    let dir = scratch("prepare");
    let layout = prepare(&dir).expect("a fresh state");
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&layout.data()), 0o700);
    let key = fs::read(layout.seal_key()).unwrap();
    assert_eq!(key.len(), SEAL_KEY_BYTES, "the static seal's key is 32 raw bytes");
    for f in [layout.seal_key(), layout.admin_role_id(), layout.admin_secret_id()] {
        assert_eq!(mode(&f), 0o600, "{}", f.display());
    }
    let ids: Vec<String> = [layout.admin_role_id(), layout.admin_secret_id()].iter().map(|f| fs::read_to_string(f).unwrap()).collect();
    assert!(ids.iter().all(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())), "ids are 32 random bytes as hex");
    assert_ne!(ids[0], ids[1]);

    let again = prepare(&dir).expect("the same state again");
    assert_eq!(again, layout);
    assert_eq!(fs::read(layout.seal_key()).unwrap(), key, "the key is made once");
    assert_eq!(fs::read_to_string(layout.admin_secret_id()).unwrap(), ids[1]);

    fs::set_permissions(layout.seal_key(), fs::Permissions::from_mode(0o640)).unwrap();
    let e = prepare(&dir).unwrap_err().to_string();
    assert!(e.contains("seal.key") && e.contains("mode 640"), "{e}");
    fs::set_permissions(layout.seal_key(), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(layout.seal_key(), &key[..16]).unwrap();
    let e = prepare(&dir).unwrap_err().to_string();
    assert!(e.contains("16 bytes"), "{e}");
    fs::remove_file(layout.seal_key()).unwrap();
    let e = prepare(&dir).unwrap_err().to_string();
    assert!(e.contains("is gone") && e.contains("remove"), "a seal key gone with its data still there: {e}");
    assert!(!layout.seal_key().exists(), "no new key is made over data it would not open");
    fs::remove_dir_all(&dir).unwrap();

    let quoted = std::env::temp_dir().join(format!("fragment-openbao-\"q\"-{}", crate::random_hex(4)));
    assert!(matches!(prepare(&quoted), Err(OpenBaoError::BadPath(_))));
    assert!(!quoted.exists());
}

// Goal: the config listens on loopback alone, keeps its data in the state,
// unseals with the key file by an id named from the key, and initialises
// the mount, the two policies, the cell's role and the stack's AppRole, in
// that order, reading the AppRole's ids from their files: no secret is in
// the config itself. The cell's policy reads the mount and nothing else.
#[test]
fn the_config_unseals_by_its_key_file_and_holds_no_secret() {
    let dir = scratch("config");
    let layout = prepare(&dir).unwrap();
    let config = render_config(&layout, 18_402, false).unwrap();
    let key_id = format!("fragment-{}", &crate::node::sha256_hex(&fs::read(layout.seal_key()).unwrap())[..16]);
    for want in [
        "address = \"127.0.0.1:18402\"".to_string(),
        "tls_disable = true".into(),
        "disable_clustering = true".into(),
        format!("storage \"pebbledb\" {{\n  path = \"{}\"", layout.data().display()),
        format!("current_key_id = \"{key_id}\""),
        format!("current_key = \"file://{}\"", layout.seal_key().display()),
        format!("path = \"{}\"", layout.admin_secret_id().display()),
    ] {
        assert!(config.contains(&want), "{want} in:\n{config}");
    }
    let order: Vec<usize> = ["\"mount\"", "\"cell-policy\"", "\"admin-policy\"", "\"cell-role\"", "\"approle\"", "\"admin-role\"", "\"admin-role-id\"", "\"admin-secret-id\""]
        .iter()
        .map(|r| config.find(&format!("request {r}")).unwrap_or_else(|| panic!("request {r}")))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "the requests in order: {order:?}");
    for secret in [layout.admin_role_id(), layout.admin_secret_id()] {
        assert!(!config.contains(fs::read_to_string(secret).unwrap().trim()), "no id is in the config");
    }
    assert!(!config.contains("audit"), "no audit log unless asked");
    assert!(render_config(&layout, 18_402, true).unwrap().contains(&format!("file_path = \"{}\"", layout.audit().display())));
    assert_eq!(cell_policy(), "path \"fragment/data/*\" { capabilities = [\"read\"] }\n");
    assert!(!admin_policy().contains("delete") && !admin_policy().contains("sys/"), "the stack's policy deletes nothing and reaches no sys path");
    fs::write(layout.seal_key(), [7u8; SEAL_KEY_BYTES]).unwrap();
    assert!(!render_config(&layout, 18_402, false).unwrap().contains(&key_id), "a new key is a new id");
    fs::remove_dir_all(&dir).unwrap();
}

// Goal: only the pinned tarball is installed, an intranet's copy
// (`FRAGMENT_OPENBAO_TARBALL`) checked as a fetch is, and only its `bao`,
// a regular file, comes out of one. Method: a tarball that is not the pin,
// by path; tarballs whose `bao` is a link, or missing.
#[test]
fn only_the_pinned_tarball_and_its_bao_are_taken() {
    let pack = |entries: &[(&str, tar::EntryType, &[u8])]| {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, kind, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            if *kind == tar::EntryType::Symlink {
                header.set_link_name("/bin/sh").unwrap();
            }
            header.set_cksum();
            tar.append_data(&mut header, path, *content).unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar.into_inner().unwrap()).unwrap();
        gz.finish().unwrap()
    };
    let good = pack(&[("LICENSE", tar::EntryType::Regular, b"MPL-2.0"), ("bao", tar::EntryType::Regular, b"#!/bin/sh\necho not bao\n")]);
    assert_eq!(binary_from(&good, "t.tar.gz").unwrap(), b"#!/bin/sh\necho not bao\n");
    assert!(binary_from(&pack(&[("bao", tar::EntryType::Symlink, b"")]), "t.tar.gz").unwrap_err().to_string().contains("not a regular file"));
    assert!(binary_from(&pack(&[("LICENSE", tar::EntryType::Regular, b"x")]), "t.tar.gz").unwrap_err().to_string().contains("holds no bao"));

    let tools = scratch("tools");
    fs::create_dir_all(&tools).unwrap();
    let copy = tools.join("intranet-copy.tar.gz");
    fs::write(&copy, &good).unwrap();
    let e = locate_from(&tools, Some(copy)).unwrap_err();
    assert!(matches!(e, OpenBaoError::HashMismatch { .. }), "{e}");
    let tarball = tarball_for(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    assert!(!tools.join(tarball.dir_name()).exists() && !tools.join(".partial-openbao").exists(), "nothing of it is installed");
    fs::remove_dir_all(&tools).unwrap();
}

// Goal: a cached `bao` that is not the pinned bytes is refused before
// anything runs it, `bao version` included (Codex round 1 on #1254: a
// replaced cache executable ran as the developer, printed the pin's
// version, and was launched as the secrets server), whether it is found
// by `locate` or handed to `OpenBao::start`. Method: an impostor in the
// pinned cache directory that writes a marker and prints what the pin
// prints. Replay: refused again, the marker never written.
#[test]
fn a_cached_bao_that_is_not_the_pinned_bytes_is_refused_unrun() {
    let tools = scratch("impostor");
    let tarball = tarball_for(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    let home = tools.join(tarball.dir_name());
    fs::create_dir_all(&home).unwrap();
    let marker = tools.join("impostor-ran");
    let impostor = home.join(BINARY);
    fs::write(&impostor, format!("#!/bin/sh\ntouch '{}'\necho 'OpenBao v{OPENBAO_VERSION} (0000000000000000000000000000000000000000), built 2026-10-26T00:00:00Z'\n", marker.display())).unwrap();
    fs::set_permissions(&impostor, fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..2 {
        let e = locate_from(&tools, None).expect_err("a cached bao that is not the pin");
        assert!(e.to_string().contains(&impostor.display().to_string()), "it names the path: {e}");
        assert!(!marker.exists(), "refused unrun: {e}");
    }
    let dir = scratch("impostor-state");
    let started = OpenBao::start(&impostor, Options { dir: dir.clone(), port: 0, audit: false });
    assert!(started.is_err() && !marker.exists(), "start refuses it unrun too: {:?}", started.err());
    let _ = fs::remove_dir_all(&dir);
    fs::remove_dir_all(&tools).unwrap();
}

/// The pinned `bao`, installed on first use.
fn pinned() -> PathBuf {
    locate(&crate::repo_root().join(crate::TOOLS_DIR)).expect("the pinned OpenBao")
}

/// The pinned OpenBao, started on a fresh state in `dir`.
fn started(dir: &Path, audit: bool) -> OpenBao {
    let bin = pinned();
    let port = crate::free_port().unwrap();
    OpenBao::start(&bin, Options { dir: dir.to_path_buf(), port, audit }).unwrap_or_else(|e| panic!("{e}")).0
}

// Goal: OpenBao initialises itself once, on its first start, and unseals
// itself from its key file at every start after, unattended; a seeding
// again writes nothing new; a key that does not open its data stops the
// start, saying so. Method: start, seed, stop, start again, seed again;
// then start with another key.
#[test]
#[ignore = "runs the pinned OpenBao (fetched into target/tools)"]
fn openbao_initialises_once_and_unseals_itself_at_every_start() {
    let dir = scratch("restarts");
    let mut bao = started(&dir, false);
    let target = bao.target();
    assert_eq!(target.seed(&SEEDED).unwrap(), Seeded { written: 2, kept: 0 });
    let key = fs::read(target.layout.seal_key()).unwrap();
    bao.down().unwrap();
    bao.up().expect("started again: unsealed by its key file");
    assert_eq!(target.seed(&SEEDED).unwrap(), Seeded { written: 0, kept: 2 }, "a restart writes no new version");
    assert_eq!(target.seed(&[("fragment-host-secret", "rotated")]).unwrap(), Seeded { written: 1, kept: 0 });
    bao.down().unwrap();
    bao.up().unwrap();
    assert_eq!(initialisations(&target.layout).unwrap(), 1, "initialised on the first start alone");
    assert_eq!(fs::read(target.layout.seal_key()).unwrap(), key);
    bao.down().unwrap();

    fs::write(target.layout.seal_key(), [9u8; SEAL_KEY_BYTES]).unwrap();
    let e = bao.up().unwrap_err().to_string();
    assert!(e.contains("stays sealed") && e.contains("does not open"), "{e}");
    assert!(!bao.running());
    fs::write(target.layout.seal_key(), &key).unwrap();
    bao.up().expect("its own key opens it again");
    drop(bao);
    fs::remove_dir_all(&dir).unwrap();
}

// Goal: the cell's token reads the mount and nothing else: no write, no
// other mount, no sys path; a second start keeps (renews) it; one that is
// not OpenBao's any more is replaced, owner-only. The stack's own token
// reaches no sys path either. The audit log counts the token's reads, and
// keeps tokens and values HMAC'd. The node's secrets file names the token
// by its path, never its value.
#[test]
#[ignore = "runs the pinned OpenBao (fetched into target/tools)"]
fn the_cells_token_reads_its_mount_and_nothing_else() {
    let dir = scratch("policy");
    let bao = started(&dir, true);
    let target = bao.target();
    target.seed(&SEEDED).unwrap();
    let token = target.cell_token().unwrap();
    assert_eq!(mode(&target.layout.cell_token()), 0o600);
    assert_eq!(target.cell_token().unwrap(), token, "kept, renewed");
    assert!(target.renew_cell_token().unwrap());
    let get = |path: &str, t: &str| target.call(reqwest::Method::GET, path, Some(t), None, path).unwrap().0;
    let put = |path: &str, t: &str| target.call(reqwest::Method::POST, path, Some(t), Some(&json!({ "data": { "value": "x" } })), path).unwrap().0;
    assert_eq!(get("fragment/data/fragment-host-secret", &token), 200);
    assert_eq!(get("fragment/data/nothing-here", &token), 404, "a missing secret is a 404 to it");
    assert_eq!(put("fragment/data/fragment-host-secret", &token), 403, "it writes nothing");
    assert_eq!(put("fragment/data/new-one", &token), 403);
    for refused in ["sys/mounts", "sys/policies/acl/fragment-admin", "cubbyhole/x", "fragment/metadata/fragment-host-secret", "auth/token/lookup-self"] {
        assert_eq!(get(refused, &token), 403, "{refused}");
    }
    let admin = target.admin_token().unwrap();
    assert_eq!(get("sys/mounts", &admin), 403, "the stack's token reaches no sys path");
    assert_eq!(get("fragment/data/nothing-here", "s.not-a-token"), 403, "a wrong token is refused");
    let audit = audited(bao.layout()).unwrap();
    assert_eq!(audit.cell_reads, 2, "the audit log holds the token's two reads of the mount: {audit:?}");
    let log = fs::read_to_string(bao.layout().audit()).unwrap();
    assert!(!log.contains(&token) && !log.contains(SEEDED[0].1), "the audit log keeps tokens and values HMAC'd");
    let node = target.node_config().to_string();
    assert!(!node.contains(&token) && node.contains(&target.layout.cell_token().display().to_string()), "{node}");

    fs::write(target.layout.cell_token(), "s.lapsed-or-forged").unwrap();
    let new = target.cell_token().unwrap();
    assert_ne!(new, token);
    assert_eq!(get("fragment/data/fragment-host-secret", &new), 200);
    assert_eq!(mode(&target.layout.cell_token()), 0o600);
    drop(bao);
    fs::remove_dir_all(&dir).unwrap();
}
