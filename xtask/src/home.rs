//! `cargo xtask home render`: a Nitro home's release (sandcastle's
//! docs/nitro-home.md, a spike): the cell's release build staged with the
//! config celld deploys, and the configs of what runs beside it in the
//! home (the identity door, macrofiche, Caddy, celld's secrets backend),
//! every one free of secrets; and, apart, the files the home's sealed
//! bundle holds, which never leave this machine unsealed.
//!
//!   cargo xtask build
//!   cargo xtask home render --conf <home.json> --secrets <dir> --out <dir>
//!
//! The deployment is the self-hosted stack's (docs/self-host.md): devstack's
//! `Fleet`, rendered as `cargo xtask dev --runtime celld` renders it, its
//! secrets bound by name in the node's backend (OpenBao, in the home), and
//! its settings, which `celld dev` reads from `.dev.vars` and `celld
//! deploy` does not, written into the config's `vars`. Sign-in is the
//! identity door's, by people's own passkeys (its passkey mode: relying
//! party the platform's host, the door at `<platform>/__door`, its state
//! on the home's encrypted disk), behind the WorkOS endpoints the cell
//! calls; models a deployment's `models` block on real APIs, files
//! macrofiche's. The
//! front door is Caddy, which ends TLS in the home with certificates it
//! gets itself (TLS-ALPN-01): the platform's at its start, a fragment's on
//! its first visit once the home says the fragment exists. Nothing here
//! changes the cell.
//!
//! `--secrets` holds the home's inputs, owner-only. Made when missing:
//! `host-secret`, `codestorage-key.pem`, `door-client-secret` (the cell's
//! `WORKOS_KEY` at the door), `door/secret` (the door's own). Given by the
//! operator: `model-key`, and `aws/access-key-id` and
//! `aws/secret-access-key` (the fleet bucket's).
//!
//! Out: `<out>/release/` (the image's, no secret in it) and
//! `<out>/sealed-files/` (owner-only: what the bundle seals, by the names
//! the home reads them under).

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest as _;

/// The client the cell is bound as at the door (`WORKOS_CLIENT`).
const DOOR_CLIENT: &str = "client_fragment_home";
/// The sealed fleet bucket's file in `--secrets` and in the bundle (celld's
/// `CELLD_BUCKET_SEAL`, celld#30): optional.
const BUCKET_SEAL: &str = "bucket-seal.json";
/// The door's path on the platform's host, which Caddy hands it: its
/// pages run their passkey ceremonies on the platform's own origin.
const DOOR_PATH: &str = "/__door";
/// macrofiche's path on the platform's host: the CLI and computers reach
/// the code store there (the storage token's `api`), the cell too.
const GIT_PATH: &str = "/__git";
/// The home's loopback ports, as its entrypoint starts each.
const DOOR_PORT: u16 = 8803;
const MACROFICHE_PORT: u16 = 8792;
/// The home's answer to Caddy's "may I get a certificate for this name?"
/// (`nitro-home-guest ask`, sandcastle's crates/nitro-home).
const ASK_PORT: u16 = 8805;
const CELLD_PORT: u16 = 8080;
const OPENBAO_ADDR: &str = "http://127.0.0.1:8200";
/// Where the home writes its opened secrets (a tmpfs), and its own state
/// (its encrypted disk).
const SECRETS_IN_HOME: &str = "/run/fragment-home/secrets";
const STATE_IN_HOME: &str = "/var/lib/fragment-home";
/// Where the home's image holds this release (sandcastle's entry.sh `R`).
const RELEASE_IN_HOME: &str = "/opt/fragment-home/release";
/// What the release names in place of the fragment tree it was rendered
/// from (a computer image's build context, which a home never builds: its
/// images are preloaded by their content key): no path of the rendering
/// machine reaches the image, so two machines render the same release.
const SOURCE_IN_HOME: &str = "/opt/fragment-home/source";
const GIT_PREFIX_IN_HOME: &str = "/opt/macrofiche/git-2.55.0";
const CODESTORE_ORG: &str = "fragment-home";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Computers {
    /// The image a new computer is pinned to, by its name in `images`.
    image: String,
    /// By name, each as the cell's config names an image (a content key,
    /// `celld-image:<sha256>`, for an image the node's engine holds).
    images: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeConf {
    /// The platform's host; its fragments are `<name>.<fragment_suffix>`.
    platform_host: String,
    fragment_suffix: String,
    /// A file holding a deployment's `models` block (deploy/example.jsonc's
    /// shape), every slot named on an OpenAI-compatible API calling with
    /// `--secrets`' `model-key`.
    models_file: PathBuf,
    #[serde(default)]
    default_plan: Option<String>,
    #[serde(default)]
    operators: Vec<String>,
    /// The one key (an npub) whose signed asks start new accounts here: a
    /// hosted platform's that adds private agents (its `private_home.key`;
    /// fragment docs/api.md, "A private agent"). The door trusts it alone
    /// (`passkeys.inviter`), and for an email with an account makes nothing.
    #[serde(default)]
    inviter: Option<String>,
    /// Without it, the home makes no computers (its node runs none).
    #[serde(default)]
    computers: Option<Computers>,
    /// The parent's address when its 443 is an SNI router (several homes
    /// on one parent, sandcastle's home-router.sh) rather than forwarded
    /// untouched: its connections open with the PROXY protocol (v2), which
    /// carries each client's address, and the front door takes that header
    /// from this address alone. Without it, nothing changes.
    #[serde(default)]
    proxy_from: Option<std::net::Ipv4Addr>,
}

fn args(rest: &[String]) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let flag = |name: &str| rest.windows(2).find(|w| w[0] == name).map(|w| PathBuf::from(&w[1]));
    match rest.first().map(String::as_str) {
        Some("render") => Ok((flag("--conf").context("--conf <home.json>")?, flag("--secrets").context("--secrets <dir>")?, flag("--out").context("--out <dir>")?)),
        _ => bail!("usage: cargo xtask home render --conf <home.json> --secrets <dir> --out <dir>"),
    }
}

pub fn home(rest: &[String]) -> Result<()> {
    let (conf, secrets, out) = args(rest)?;
    let c: HomeConf = serde_json::from_str(&devstack::strip_comments(&fs::read_to_string(&conf).with_context(|| format!("read {}", conf.display()))?)).with_context(|| format!("parse {}", conf.display()))?;
    render(&c, &secrets, &out)
}

/// A file of `--secrets`, which must be its owner's alone.
fn read_secret(dir: &Path, name: &str) -> Result<String> {
    let path = dir.join(name);
    let meta = fs::symlink_metadata(&path).with_context(|| format!("{} (the home's input {name})", path.display()))?;
    anyhow::ensure!(meta.is_file() && meta.mode() & 0o077 == 0, "{}: a plain file, its owner's alone", path.display());
    Ok(fs::read_to_string(&path)?.trim_end_matches('\n').to_string())
}

/// `name` in `--secrets`, made by `make` when missing (owner-only).
fn made_secret(dir: &Path, name: &str, make: impl FnOnce() -> String) -> Result<String> {
    let path = dir.join(name);
    if !path.exists() {
        if let Some(d) = path.parent() {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(d)?;
        }
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        std::io::Write::write_all(&mut f, make().as_bytes())?;
    }
    read_secret(dir, name)
}

/// Writes `text` to `path`, owner-only when `private`.
fn write(path: &Path, text: &str, private: bool) -> Result<()> {
    if let Some(d) = path.parent() {
        fs::DirBuilder::new().recursive(true).mode(if private { 0o700 } else { 0o755 }).create(d)?;
    }
    let _ = fs::remove_file(path);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(if private { 0o600 } else { 0o644 }).open(path).with_context(|| format!("write {}", path.display()))?;
    std::io::Write::write_all(&mut f, text.as_bytes())?;
    Ok(())
}

/// `.dev.vars` as `write_dev_vars` writes it: `KEY=value`, a newline in a
/// value as `\n`.
fn dev_vars(text: &str) -> Result<serde_json::Map<String, Value>> {
    let mut vars = serde_json::Map::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (k, v) = line.split_once('=').with_context(|| format!("a .dev.vars line without =: {}", line.chars().take(40).collect::<String>()))?;
        anyhow::ensure!(vars.insert(k.to_string(), json!(v.replace("\\n", "\n"))).is_none(), "{k} twice in .dev.vars");
    }
    Ok(vars)
}

/// An SPKI PEM of a PKCS#8 P-256 private key: the org's public key,
/// which macrofiche checks the cell's tokens with.
fn public_pem(private_pem: &str) -> Result<String> {
    use p256::pkcs8::{DecodePrivateKey as _, EncodePublicKey as _, LineEnding};
    let key = p256::ecdsa::SigningKey::from_pkcs8_pem(private_pem).context("the code store's key is a PKCS#8 P-256 PEM")?;
    key.verifying_key().to_public_key_pem(LineEnding::LF).context("its public key")
}

/// What every file under `dir` hashes to, by its path: the release's id.
fn tree_sha(dir: &Path) -> Result<String> {
    let mut files = vec![];
    let mut stack = vec![dir.to_path_buf()];
    // bounded by the tree
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut h = sha2::Sha256::new();
    for f in files {
        h.update(f.strip_prefix(dir)?.to_string_lossy().as_bytes());
        h.update([0]);
        h.update(sha2::Sha256::digest(fs::read(&f)?));
    }
    Ok(format!("{:x}", h.finalize()))
}

fn render(c: &HomeConf, secrets: &Path, out: &Path) -> Result<()> {
    let meta = fs::metadata(secrets).with_context(|| format!("{} (--secrets)", secrets.display()))?;
    anyhow::ensure!(meta.is_dir() && meta.mode() & 0o077 == 0, "{}: the home's inputs are its owner's alone (0700)", secrets.display());
    let host_secret = made_secret(secrets, "host-secret", || devstack::random_hex(32))?;
    let codestore_key = made_secret(secrets, "codestorage-key.pem", fragment_fakes::codestorage::generate_org_key_pem)?;
    let model_key = read_secret(secrets, "model-key")?;
    for name in ["aws/access-key-id", "aws/secret-access-key"] {
        read_secret(secrets, name)?;
    }
    let door_client_secret = made_secret(secrets, "door-client-secret", || format!("sk_home_{}", devstack::random_hex(24)))?;
    made_secret(secrets, "door/secret", || devstack::random_hex(32))?;
    // the models, checked as a deploy checks them; their slots on an
    // OpenAI-compatible API call with the one key
    let models_text = fs::read_to_string(&c.models_file).with_context(|| format!("read {}", c.models_file.display()))?;
    let models: Value = serde_json::from_str(&devstack::strip_comments(&models_text)).context("a deployment's `models` block")?;
    let (models_cell, slot_keys) = crate::deploy::models_cell(&models, false)?;
    anyhow::ensure!(!slot_keys.is_empty(), "the home's models name no slot on an OpenAI-compatible API: celld has no Workers AI");

    // the cell's release build, staged, and its deployment rendered into it
    let _ = fs::remove_dir_all(out);
    let release = out.join("release");
    let cell = devstack::stage_project(&release.join("cell"))?;
    // the packages the cell's entry imports (the Sandbox SDK), from the
    // pinned npm's node_modules: `celld deploy` bundles the entry in the
    // home, beside them
    copy_modules(&cell, &ENTRY_PACKAGES)?;
    let platform_url = format!("https://{}", c.platform_host);
    let door_url = format!("{platform_url}{DOOR_PATH}");
    // a computer's image is one the node's engine holds, by its content key
    // (celld's `celld-image:<sha256>`), which devstack's scoping of an
    // image built from a Dockerfile does not take: set after the render
    let computer_image = match &c.computers {
        Some(k) => {
            anyhow::ensure!(k.images.contains_key(&k.image), "computers.image {} is none of computers.images", k.image);
            Some(k.image.clone())
        }
        None => None,
    };
    let fleet = devstack::Fleet {
        host_secret,
        codestorage_org: CODESTORE_ORG.into(),
        codestorage_key_pem: codestore_key.clone(),
        codestorage_url: format!("https://{}{GIT_PATH}", c.platform_host),
        host_suffix: c.fragment_suffix.clone(),
        host_label_suffix: None,
        // the cell's own default (Cloudflare's): a fragment pushed to in
        // the last day polls its code store this often, and a home's
        // freshly made fragments otherwise woke every 30 s for a day, each
        // wake a write to the bucket and no cell ever idle enough to evict
        poll_interval_s: 300,
        egress_local: false,
        job_retry_delay_s: 10,
        upload_max_bytes: None,
        ai_url: None,
        ai_gateway: None,
        models: Some(models_cell.to_string()),
        model_keys: slot_keys.iter().map(|(slot, name)| devstack::ModelKey { slot: slot.clone(), name: name.clone(), value: model_key.clone() }).collect(),
        mail_url: None,
        mail_from: None,
        default_plan: c.default_plan.clone(),
        delivery_retry_s: None,
        workos: Some(devstack::WorkOsVars { client_id: DOOR_CLIENT.into(), api_key: door_client_secret.clone(), api_url: Some(door_url.clone()) }),
        stripe: None,
        observability: None,
        platform_url: platform_url.clone(),
        operators: (!c.operators.is_empty()).then(|| c.operators.join(",")),
        migrations: None,
        signins_pending_max: None,
        cli_begins_per_ip_minute: None,
        test_secret: None,
        computer_image,
        images: vec![],
        computer_snapshots: false,
        // a home runs no SimpleX doors (the e2e's door lane's alone)
        doors: false,
        computer_internet: true,
        computer_unsaved_max_ms: None,
        providers: Some("[]".into()),
        operator_key_values: vec![],
        connection_secret_values: vec![],
        swap_upstream: None,
        connectors: Some(devstack::celld::connectors()),
        secrets: devstack::Secrets::Vars,
    };
    let tools = devstack::Tools::locate()?;
    fleet.configure(&tools, &cell)?;
    let mut config: Value = serde_json::from_str(&fs::read_to_string(devstack::celld::celld_config(&cell)?)?)?;
    let vars = dev_vars(&fs::read_to_string(cell.join(".dev.vars"))?)?;
    let obj = config.as_object_mut().context("a config is an object")?;
    anyhow::ensure!(!obj.contains_key("vars"), "cell/wrangler.jsonc names no vars of its own");
    obj.insert("vars".into(), Value::Object(vars));
    // when the release was made, from its inputs rather than the clock, so
    // the same inputs render the same release (and so the same image and
    // PCR0): SOURCE_DATE_EPOCH, else the fragment tree's HEAD commit time
    let made_at = made_at(&devstack::cell_dir())?;
    obj.get_mut("vars").and_then(Value::as_object_mut).context("its vars")?.insert("FRAGMENT_DEPLOYED_AT".into(), Value::String(made_at.to_string()));
    // the AI binding the local render drops (wrangler dev would open a
    // remote session for it): celld binds it, every call rejecting, "this
    // node has no Workers AI" (docs/self-host.md), as a node without it
    let checked_in = devstack::read_config(&devstack::cell_dir())?;
    obj.insert("ai".into(), checked_in.get("ai").cloned().context("cell/wrangler.jsonc binds AI")?);
    if let Some(k) = &c.computers {
        let images = obj.get_mut("containers").and_then(|c| c.get_mut(0)).and_then(|c| c.get_mut("images")).context("the cell's config names its computer's images")?;
        *images = json!(k.images);
    }
    write(&cell.join("wrangler.home.jsonc"), &serde_json::to_string_pretty(&config)?, false)?;
    // the store's values, by their names, go to the bundle alone; the
    // local renders celld deploys none of go too
    let store: Value = serde_json::from_str(&fs::read_to_string(devstack::celld::secrets_file(&cell))?)?;
    let sealed = out.join("sealed-files");
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&sealed)?;
    for (name, value) in store["values"].as_object().context("the vars backend's values")? {
        write(&sealed.join("store").join(name), value.as_str().context("a secret is text")?, true)?;
    }
    for f in [".dev.vars", ".celld.secrets.json", "wrangler.local.jsonc", "wrangler.celld.jsonc", "wrangler.admin.local.jsonc"] {
        let _ = fs::remove_file(cell.join(f));
    }
    for name in ["door/secret", "aws/access-key-id", "aws/secret-access-key"] {
        write(&sealed.join(name), &read_secret(secrets, name)?, true)?;
    }
    // the sealed fleet bucket (celld's CELLD_BUCKET_SEAL: its key, and its
    // counter's bucket and credentials), when the home has one
    if secrets.join(BUCKET_SEAL).exists() {
        let seal = read_secret(secrets, BUCKET_SEAL)?;
        let v: Value = serde_json::from_str(&seal).with_context(|| format!("{BUCKET_SEAL} is JSON"))?;
        anyhow::ensure!(v["key"].is_string() && v["counter"]["bucket"].as_str().is_some_and(|b| b.starts_with("s3://")) && v["counter"]["name"].is_string(), "{BUCKET_SEAL}: {{key, counter: {{bucket: s3://…, name, region, access_key_id, secret_access_key}}}}");
        write(&sealed.join(BUCKET_SEAL), &seal, true)?;
    }

    // what runs beside celld in the home, configured
    // the door: people's own passkeys, its relying party the platform's
    // host, its state on the encrypted disk
    let mut passkeys = json!({ "rp_id": c.platform_host, "origin": platform_url, "state_dir": format!("{STATE_IN_HOME}/door") });
    if let Some(inviter) = &c.inviter {
        anyhow::ensure!(fragment_core::npub::parse(inviter).is_some(), "inviter is an npub, not {inviter:?}");
        passkeys["inviter"] = json!(inviter);
    }
    let door = json!({
        "listen": format!("127.0.0.1:{DOOR_PORT}"),
        "public_url": door_url,
        "secret_file": format!("{SECRETS_IN_HOME}/door/secret"),
        "passkeys": passkeys,
        "clients": [{ "client_id": DOOR_CLIENT, "client_secret_sha256": format!("{:x}", sha2::Sha256::digest(door_client_secret.as_bytes())), "redirect_uris": [format!("{platform_url}/auth/callback")] }],
    });
    write(&release.join("door.json"), &serde_json::to_string_pretty(&door)?, false)?;
    let macrofiche = json!({
        "listen": format!("127.0.0.1:{MACROFICHE_PORT}"),
        "state_dir": format!("{STATE_IN_HOME}/macrofiche"),
        "orgs": [{ "name": CODESTORE_ORG, "keys": [{ "pem": public_pem(&codestore_key)?, "kid": "k1" }] }],
        "git_prefix": GIT_PREFIX_IN_HOME,
        "jail": "enforce",
    });
    write(&release.join("macrofiche.json"), &serde_json::to_string_pretty(&macrofiche)?, false)?;
    // celld's secrets backend: OpenBao on the home's loopback, its token a
    // file the entrypoint mints at each boot (no secret here)
    let backend = json!({ "backend": "openbao", "addr": OPENBAO_ADDR, "mount": "secret", "token_file": "/run/fragment-home/celld/openbao.token" });
    write(&release.join("celld-secrets.json"), &backend.to_string(), true)?;
    write(&release.join("Caddyfile"), &caddyfile(c), false)?;
    // the platform's own name, for the stack's calls to its own URL: this
    // home's Caddy
    write(&release.join("hosts"), &format!("127.0.0.1 {}\n", c.platform_host), false)?;
    in_home_paths(&release, &devstack::cell_dir())?;
    let id = tree_sha(&release)?;
    write(&release.join("release-id"), &format!("{id}\n"), false)?;
    let names: Vec<String> = walk_names(&sealed)?;
    println!("{}", json!({ "release": release, "release_id": id, "sealed_files": names, "sealed_dir": sealed }));
    Ok(())
}

/// The packages the cell's shim imports by name (cell/entry.mjs).
const ENTRY_PACKAGES: [&str; 1] = ["@cloudflare/sandbox"];
/// A package's dependencies, at most this deep (the SDK's are two deep).
const MODULE_DEPTH_MAX: usize = 8;

/// `names` and every package they depend on, copied from the repo's
/// node_modules (npm's flat layout, as `npm ci` makes it) into the
/// project's own.
fn copy_modules(project: &Path, names: &[&str]) -> Result<()> {
    fn copy_dir(from: &Path, to: &Path) -> Result<()> {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            let target = to.join(e.file_name());
            if e.file_type()?.is_dir() {
                copy_dir(&e.path(), &target)?;
            } else if e.file_type()?.is_file() {
                fs::copy(e.path(), &target)?;
            }
        }
        Ok(())
    }
    let from = devstack::repo_root().join("node_modules");
    let mut todo: Vec<(String, usize)> = names.iter().map(|n| (n.to_string(), 0)).collect();
    let mut done = std::collections::BTreeSet::new();
    // bounded: each package once, MODULE_DEPTH_MAX deep
    while let Some((name, depth)) = todo.pop() {
        anyhow::ensure!(depth <= MODULE_DEPTH_MAX, "{name}: dependencies past {MODULE_DEPTH_MAX} deep");
        if !done.insert(name.clone()) {
            continue;
        }
        let dir = from.join(&name);
        let manifest: Value = serde_json::from_str(&fs::read_to_string(dir.join("package.json")).with_context(|| format!("{} (npm ci installs it)", dir.display()))?)?;
        copy_dir(&dir, &project.join("node_modules").join(&name))?;
        for dep in manifest["dependencies"].as_object().into_iter().flatten().map(|(k, _)| k) {
            todo.push((dep.clone(), depth + 1));
        }
    }
    Ok(())
}

fn walk_names(dir: &Path) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(dir)?.to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// `SOURCE_DATE_EPOCH` (whole seconds), else the commit time of the tree's
/// HEAD: when the release was made, the same for the same inputs.
fn made_at(cell_dir: &Path) -> Result<u64> {
    if let Ok(v) = std::env::var("SOURCE_DATE_EPOCH") {
        return v.trim().parse().with_context(|| format!("SOURCE_DATE_EPOCH {v:?}: whole seconds"));
    }
    let out = std::process::Command::new("git").arg("-C").arg(cell_dir).args(["log", "-1", "--format=%ct", "HEAD"]).output().context("git log")?;
    anyhow::ensure!(out.status.success(), "the fragment tree's HEAD commit time: {}", String::from_utf8_lossy(&out.stderr).trim());
    String::from_utf8_lossy(&out.stdout).trim().parse().context("a commit time is whole seconds")
}

/// The rendered configs name the release's own directory and the fragment
/// tree's by their paths on the rendering machine; in the home they are
/// `RELEASE_IN_HOME` and `SOURCE_IN_HOME`, the same on every machine.
fn in_home_paths(release: &Path, cell_dir: &Path) -> Result<()> {
    let release_at = fs::canonicalize(release)?.to_string_lossy().into_owned();
    let tree_at = fs::canonicalize(cell_dir.join(".."))?.to_string_lossy().into_owned();
    let cell = release.join("cell");
    let mut files = vec![cell.join("wrangler.home.jsonc"), cell.join("wrangler.jsonc")];
    if let Ok(dir) = fs::read_dir(cell.join(".wrangler/images")) {
        files.extend(dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "Dockerfile")));
    }
    for f in files.into_iter().filter(|f| f.exists()) {
        let text = fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?;
        // the release's path first: it may lie inside the tree's
        let given = release.to_string_lossy();
        let moved = text.replace(&release_at, RELEASE_IN_HOME).replace(given.as_ref(), RELEASE_IN_HOME).replace(&format!("{tree_at}/cell/.."), SOURCE_IN_HOME).replace(&tree_at, SOURCE_IN_HOME);
        if moved != text {
            fs::write(&f, moved).with_context(|| format!("write {}", f.display()))?;
        }
    }
    Ok(())
}

/// The home's front door, Caddy: TLS ended here, in the home, with
/// certificates it gets itself by TLS-ALPN-01 on the port the parent
/// forwards untouched (443), so no DNS token is ever in the home. The
/// platform's is got at its start; a fragment's on its first visit, once
/// `ask` (the home's own check: the cell knows the name) says yes, so a
/// name no fragment holds gets no certificate and Let's Encrypt's limit
/// for the registered domain is spent on real names alone. Certificates
/// and the ACME account live on the home's encrypted disk. Each request
/// goes on to celld on loopback; Caddy replaces the forwarded headers
/// (it trusts no proxy), which celld trusts
/// (`CELLD_TRUST_FORWARDED_HEADERS`), and sets `CF-Connecting-IP` to the
/// client's address (docs/self-host.md, found 48). Behind a parent's SNI
/// router (`proxy_from`), that address is the PROXY protocol's, taken
/// from the parent alone; the TLS still ends here.
fn caddyfile(c: &HomeConf) -> String {
    let servers = match c.proxy_from {
        Some(parent) => format!("\tservers {{\n\t\tlistener_wrappers {{\n\t\t\tproxy_protocol {{\n\t\t\t\ttimeout 5s\n\t\t\t\tallow {parent}/32\n\t\t\t}}\n\t\t\ttls\n\t\t}}\n\t}}\n"),
        None => String::new(),
    };
    let proxy = format!(
        "\treverse_proxy 127.0.0.1:{CELLD_PORT} {{\n\t\theader_up X-Forwarded-Host {{host}}\n\t\theader_up X-Forwarded-Proto https\n\t\theader_up CF-Connecting-IP {{remote_host}}\n\t\tflush_interval -1\n\t}}\n\trequest_body {{\n\t\tmax_size 200MB\n\t}}\n"
    );
    let issuer = "\t\tissuer acme {\n\t\t\tdir https://acme-v02.api.letsencrypt.org/directory\n\t\t\tdisable_http_challenge\n\t\t}\n";
    format!(
        "# The home's front door (xtask/src/home.rs), rendered for one home.\n\
{{\n\
\tadmin off\n\
\tauto_https disable_redirects\n\
\tstorage file_system {STATE_IN_HOME}/caddy\n\
\ton_demand_tls {{\n\
\t\task http://127.0.0.1:{ASK_PORT}/ask\n\
\t}}\n\
\tlog {{\n\
\t\toutput file {STATE_IN_HOME}/log/caddy.log\n\
\t}}\n\
{servers}}}\n\
\n\
{platform}:443 {{\n\
\ttls {{\n\
{issuer}\t}}\n\
\thandle_path {DOOR_PATH}/* {{\n\
\t\treverse_proxy 127.0.0.1:{DOOR_PORT}\n\
\t}}\n\
\thandle_path {GIT_PATH}/* {{\n\
\t\treverse_proxy 127.0.0.1:{MACROFICHE_PORT}\n\
\t}}\n\
\thandle {{\n\
{inner}\t}}\n\
\trequest_body {{\n\
\t\tmax_size 200MB\n\
\t}}\n\
}}\n\
\n\
*.{suffix}:443 {{\n\
\ttls {{\n\
\t\ton_demand\n\
{issuer}\t}}\n\
{proxy}}}\n",
        platform = c.platform_host,
        suffix = c.fragment_suffix,
        inner = proxy.lines().take_while(|l| !l.contains("request_body")).map(|l| format!("\t{l}\n")).collect::<String>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Goal: `.dev.vars` as devstack writes it reads back to its values, a
    // newline in one restored; a name twice is refused.
    #[test]
    fn dev_vars_read_back() {
        let v = dev_vars("A=1\nB=x\\ny\nC={\"j\":1}\n").unwrap();
        assert_eq!(v["A"], "1");
        assert_eq!(v["B"], "x\ny");
        assert_eq!(v["C"], "{\"j\":1}");
        assert!(dev_vars("A=1\nA=2\n").is_err());
        assert!(dev_vars("no equals\n").is_err());
    }

    // Goal: the code store's private key gives the SPKI public key
    // macrofiche takes, the same every time.
    #[test]
    fn the_org_key_gives_its_public_key() {
        let pem = fragment_fakes::codestorage::generate_org_key_pem();
        let a = public_pem(&pem).unwrap();
        assert!(a.starts_with("-----BEGIN PUBLIC KEY-----") && !a.contains("PRIVATE"));
        assert_eq!(public_pem(&pem).unwrap(), a);
        assert!(public_pem("not a key").is_err());
    }

    // Goal: the front door ends TLS with certificates it gets itself by
    // TLS-ALPN-01 (never HTTP-01, never a DNS token), the platform's at
    // its start and a fragment's on demand behind the home's own check, on
    // its encrypted disk; it replaces the forwarded headers.
    #[test]
    fn the_front_door_gets_its_own_certificates() {
        let c: HomeConf = serde_json::from_value(json!({
            "platform_host": "home.example.com", "fragment_suffix": "home.example.com", "models_file": "/m.jsonc"
        }))
        .unwrap();
        let f = caddyfile(&c);
        assert!(f.contains("home.example.com:443 {") && f.contains("*.home.example.com:443 {"));
        assert_eq!(f.matches("disable_http_challenge").count(), 2);
        assert_eq!(f.matches("on_demand\n").count(), 1, "the fragments' names alone on demand");
        assert!(f.contains("ask http://127.0.0.1:8805/ask") && f.contains("storage file_system /var/lib/fragment-home/caddy"));
        assert!(f.contains("admin off") && f.contains("header_up CF-Connecting-IP {remote_host}") && f.contains("header_up X-Forwarded-Proto https"));
        assert!(!f.contains("dns"), "no DNS challenge, no token");
        assert!(f.contains("handle_path /__door/* {\n\t\treverse_proxy 127.0.0.1:8803"), "the door on the platform's own origin");
        assert!(f.contains("handle_path /__git/* {\n\t\treverse_proxy 127.0.0.1:8792"), "the code store, which the CLI reaches");
        assert_eq!(f.matches("handle_path").count(), 2, "on the platform's host alone, no fragment's");
        assert!(!f.contains("proxy_protocol"), "forwarded untouched: no PROXY header taken");
    }

    // Goal: a rendered release names no path of the machine that rendered
    // it: its own directory and the fragment tree's become the home's fixed
    // ones in its configs and image files, and the rest is untouched.
    #[test]
    fn a_release_names_no_path_of_the_rendering_machine() {
        let root = std::env::temp_dir().join(format!("home-paths-{}", std::process::id()));
        let (tree, release) = (root.join("tree"), root.join("out/release"));
        fs::create_dir_all(tree.join("cell")).unwrap();
        fs::create_dir_all(release.join("cell/.wrangler/images")).unwrap();
        let (r, t) = (fs::canonicalize(&release).unwrap(), fs::canonicalize(&tree).unwrap());
        let config = format!(r#"{{"dockerfile": "{r}/cell/.wrangler/images/stub.Dockerfile", "context": "{t}/cell/..", "file": "{t}/cell/../images/stub/Dockerfile", "keep": "/elsewhere"}}"#, r = r.display(), t = t.display());
        fs::write(release.join("cell/wrangler.home.jsonc"), &config).unwrap();
        fs::write(release.join("cell/.wrangler/images/stub.Dockerfile"), format!("LABEL dev.fragment.project=\"{}/cell\"\n", r.display())).unwrap();
        in_home_paths(&release, &tree.join("cell")).unwrap();
        let got = fs::read_to_string(release.join("cell/wrangler.home.jsonc")).unwrap();
        assert_eq!(got, r#"{"dockerfile": "/opt/fragment-home/release/cell/.wrangler/images/stub.Dockerfile", "context": "/opt/fragment-home/source", "file": "/opt/fragment-home/source/images/stub/Dockerfile", "keep": "/elsewhere"}"#);
        assert_eq!(fs::read_to_string(release.join("cell/.wrangler/images/stub.Dockerfile")).unwrap(), "LABEL dev.fragment.project=\"/opt/fragment-home/release/cell\"\n");
        fs::remove_dir_all(&root).unwrap();
    }

    // Goal: behind a parent's SNI router, the front door takes the PROXY
    // protocol's client address from the parent's address alone, before
    // its own TLS; an address that is not IPv4 is refused.
    #[test]
    fn behind_a_router_the_client_address_is_the_proxy_protocols() {
        let c: HomeConf = serde_json::from_value(json!({
            "platform_host": "org.homes.example.com", "fragment_suffix": "org.homes.example.com", "models_file": "/m.jsonc", "proxy_from": "10.1.3.1"
        }))
        .unwrap();
        let f = caddyfile(&c);
        assert!(f.contains("\tservers {\n\t\tlistener_wrappers {\n\t\t\tproxy_protocol {\n\t\t\t\ttimeout 5s\n\t\t\t\tallow 10.1.3.1/32\n\t\t\t}\n\t\t\ttls\n"), "{f}");
        assert!(f.find("servers {").unwrap() < f.find("org.homes.example.com:443 {").unwrap(), "a global option");
        assert!(f.contains("header_up CF-Connecting-IP {remote_host}"));
        for bad in ["10.1.3.0/24", "host", "::1"] {
            assert!(serde_json::from_value::<HomeConf>(json!({ "platform_host": "a", "fragment_suffix": "a", "models_file": "/m", "proxy_from": bad })).is_err(), "{bad}");
        }
    }
}
