//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"), over
//! the API: the person puts, lists and removes their secrets, each a value
//! that no answer, list or log ever names; an agent, even their own, is
//! refused every route; another person sees none of them; each refusal of
//! a bad secret writes nothing; the limit holds; and a restart keeps them.
use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Reply};
use crate::{Need, Suite};

/// A value no answer or log may name, random per run.
fn sentinel() -> String {
    format!("vault-sentinel-{}", Keys::generate().pubkey_hex()[..24].to_owned())
}

/// The names in `keys`' vault, as `GET /api/vault` lists them.
fn names(api: &Api, keys: &Keys) -> Result<(Reply, Vec<String>)> {
    let r = api.signed(keys, "GET", "/api/vault", None)?;
    let names = r.body["secrets"].as_array().map(|l| l.iter().filter_map(|s| s["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    Ok((r, names))
}

fn put(api: &Api, keys: &Keys, name: &str, body: &Value) -> Result<Reply> {
    api.signed(keys, "PUT", &format!("/api/vault/{name}"), Some(body))
}

/// The CLI in `home`, `value` and a line end piped on its stdin, as a
/// person's shell pipes a secret: never on its command line.
fn piped_cli(s: &Suite, api: &Api, home: &std::path::Path, args: &[&str], value: &str) -> Result<std::process::Output> {
    use std::io::Write;
    let mut c = s.bare_cli();
    c.args(args).env("HOME", home).env("FRAGMENT_HOST", &api.base).env_remove("FRAGMENT_OUTPUT");
    let mut child = c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn()?;
    child.stdin.take().expect("piped").write_all(format!("{value}\n").as_bytes())?;
    Ok(child.wait_with_output()?)
}

/// Every node log the run has written so far, whole.
pub(crate) fn logs(s: &Suite) -> String {
    let mut text = String::new();
    for entry in std::fs::read_dir(&s.scratch).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("node-") && name.ends_with(".log") {
            text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    text
}

pub fn vault(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("vault", &[Need::Node]) {
        return Ok(());
    }
    let ada = api.person()?;
    let bob = api.person()?;
    let (agent, _) = super::delegation::agent_of(api, &ada)?;
    let value = sentinel();
    let rotated = sentinel();
    let agents = json!(["ada--k3x9"]);

    // valid: put, answered and listed by name, never by value
    let r = put(api, &ada, "linear", &json!({ "value": value, "hosts": ["api.linear.app"], "agents": agents }))?;
    s.ok(
        "a person puts a secret: answered with its name, hosts, place (the default spelled out) and agents, never its value",
        r.status == 200
            && r.body["name"] == "linear"
            && r.body["hosts"] == json!(["api.linear.app"])
            && r.body["place"] == json!({ "header": "authorization", "format": "Bearer {}" })
            && r.body["agents"] == agents
            && r.body["setAt"].as_i64().is_some_and(|t| t > 0)
            && !r.text.contains(&value),
        &r,
    );
    let (l, listed) = names(api, &ada)?;
    s.ok("their list names it, and holds no value", l.status == 200 && listed == ["linear"] && !l.text.contains(&value), &l);
    let r = put(api, &ada, "linear", &json!({ "value": rotated, "hosts": ["api.linear.app"], "place": { "header": "x-api-key" }, "agents": agents }))?;
    let (l, listed) = names(api, &ada)?;
    s.ok(
        "set again, a secret is replaced whole (its value and its place), still one",
        r.status == 200 && r.body["place"] == json!({ "header": "x-api-key", "format": "{}" }) && listed == ["linear"] && !l.text.contains(&rotated),
        &r,
    );
    let r = put(api, &ada, "telegram-ada", &json!({ "value": "123456:abc-DEF", "hosts": [], "agents": agents }))?;
    s.ok("a secret with no hosts is kept, parked", r.status == 200 && r.body["hosts"] == json!([]), &r);
    let nsec = Keys::generate().secret_hex();
    let r = put(api, &ada, "nostr-ada", &json!({ "value": nsec, "hosts": ["finite.site"], "place": { "sign": "nip98" }, "agents": agents }))?;
    s.ok("a Nostr key is kept to sign, toward its hosts", r.status == 200 && r.body["place"] == json!({ "sign": "nip98" }) && !r.text.contains(&nsec), &r);

    // invalid: each refused, nothing written
    let before = names(api, &ada)?.1;
    for (why, name, body) in [
        ("an address for a host", "k1", json!({ "value": "v", "hosts": ["1.2.3.4"], "agents": agents })),
        ("the platform's internal host", "k1", json!({ "value": "v", "hosts": ["api.fragment.internal"], "agents": agents })),
        ("a host with a scheme", "k1", json!({ "value": "v", "hosts": ["https://api.linear.app"], "agents": agents })),
        ("no agent", "k1", json!({ "value": "v", "hosts": ["api.linear.app"], "agents": [] })),
        ("an agent that is no fragment name", "k1", json!({ "value": "v", "hosts": ["api.linear.app"], "agents": ["ada"] })),
        ("a value with a space", "k1", json!({ "value": "has space", "hosts": ["api.linear.app"], "agents": agents })),
        ("a name of another form", "Linear_1", json!({ "value": "v", "hosts": ["api.linear.app"], "agents": agents })),
        ("a provider's name", crate::SWAP_ROUTER, json!({ "value": "v", "hosts": ["api.linear.app"], "agents": agents })),
        ("two places", "k1", json!({ "value": "v", "hosts": ["api.linear.app"], "place": { "header": "authorization", "query": "key" }, "agents": agents })),
        ("a field it does not know", "k1", json!({ "value": "v", "hosts": ["api.linear.app"], "agents": agents, "env": "LINEAR" })),
        ("a signing key off the curve", "k1", json!({ "value": "0".repeat(64), "hosts": ["finite.site"], "place": { "sign": "nip98" }, "agents": agents })),
    ] {
        let r = put(api, &ada, name, &body)?;
        let after = names(api, &ada)?.1;
        s.ok(&format!("{why} is refused (400), and nothing is written"), r.status == 400 && after == before, &r);
    }

    // who: their own agent, and another person
    let a = api.signed(&agent, "GET", "/api/vault", None)?;
    let p = put(api, &agent, "mine", &json!({ "value": "v", "hosts": ["api.linear.app"], "agents": agents }))?;
    let d = api.signed(&agent, "DELETE", "/api/vault/linear", None)?;
    s.ok(
        "their own agent is refused every route (403): it uses secrets, never sets, lists or removes one",
        a.status == 403 && p.status == 403 && d.status == 403 && names(api, &ada)?.1 == before,
        format!("{a} {p} {d}"),
    );
    let (b, bob_names) = names(api, &bob)?;
    let gone = api.signed(&bob, "DELETE", "/api/vault/linear", None)?;
    s.ok(
        "another person's vault is their own: it lists none of hers, and removing hers by name removes nothing",
        b.status == 200 && bob_names.is_empty() && gone.status == 200 && gone.body["removed"] == false && names(api, &ada)?.1 == before,
        format!("{b} {gone}"),
    );

    // the limit: at most 64, a 65th refused, setting one again still fine
    for i in 0..64 {
        let r = put(api, &bob, &format!("k{i}"), &json!({ "value": "v", "hosts": ["api.example.com"], "agents": agents }))?;
        anyhow::ensure!(r.status == 200, "put {i}: {r}");
    }
    let over = put(api, &bob, "k64", &json!({ "value": "v", "hosts": ["api.example.com"], "agents": agents }))?;
    let again = put(api, &bob, "k3", &json!({ "value": "w", "hosts": ["api.example.com"], "agents": agents }))?;
    s.ok("a vault keeps at most 64 secrets: a 65th is refused, and one set again is not a new one", over.status == 400 && again.status == 200 && names(api, &bob)?.1.len() == 64, format!("{over} {again}"));

    // restart: what was put is still there
    s.stop()?;
    let api = s.start(false)?;
    let (l, listed) = names(&api, &ada)?;
    s.ok("a restart keeps a vault's secrets", l.status == 200 && listed == before, &l);

    // remove: once, then nothing
    let r = api.signed(&ada, "DELETE", "/api/vault/linear", None)?;
    let again = api.signed(&ada, "DELETE", "/api/vault/linear", None)?;
    let (_, listed) = names(&api, &ada)?;
    s.ok(
        "removed, a secret is gone from the list; removed again, nothing was there",
        r.status == 200 && r.body["removed"] == true && again.status == 200 && again.body["removed"] == false && !listed.contains(&"linear".to_string()),
        format!("{r} {again}"),
    );

    // the CLI: a value piped on stdin, never on its command line, and never
    // in what it prints
    let home = s.dir("vault-home");
    s.login(&api, &home);
    let piped = sentinel();
    let out = piped_cli(s, &api, &home, &["vault", "set", "cli-key", "--agent", "ada--k3x9", "--to", "api.example.com", "--header", "x-api-key", "--json"], &piped)?;
    let printed = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let listed = s.cli(&api, &home, &["vault", "list"]);
    let listed_text = format!("{}{}", String::from_utf8_lossy(&listed.stdout), String::from_utf8_lossy(&listed.stderr));
    s.ok(
        "the CLI sets a secret from stdin (its line end trimmed) and lists it, never printing the value",
        out.status.success() && printed.contains("\"x-api-key\"") && !printed.contains(&piped) && listed.status.success() && listed_text.contains("cli-key  api.example.com  for ada--k3x9") && !listed_text.contains(&piped),
        format!("{printed} / {listed_text}"),
    );
    let empty = piped_cli(s, &api, &home, &["vault", "set", "cli-empty", "--agent", "ada--k3x9"], "")?;
    let argv = s.cli(&api, &home, &["vault", "set", "cli-argv", &piped, "--agent", "ada--k3x9"]);
    let gone = s.cli(&api, &home, &["vault", "rm", "cli-key"]);
    s.ok(
        "an empty value is refused before anything is sent, a value on the command line is no argument the CLI takes, and rm removes it",
        !empty.status.success() && !argv.status.success() && gone.status.success() && String::from_utf8_lossy(&gone.stdout).contains("secret cli-key removed"),
        format!("{} / {} / {}", String::from_utf8_lossy(&empty.stderr), String::from_utf8_lossy(&argv.stderr), String::from_utf8_lossy(&gone.stdout)),
    );

    // never in a log
    let text = logs(s);
    s.ok(
        "no value put (a token, its rotation, a Nostr key) is in any node log",
        !text.is_empty() && !text.contains(&value) && !text.contains(&rotated) && !text.contains(&nsec) && !text.contains(&piped),
        format!("{} bytes of logs", text.len()),
    );
    Ok(())
}
