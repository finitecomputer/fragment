//! SimpleX set up from Settings through its connector (D3: every seat, as
//! Telegram's is), from the computers lane (its stub agent, whose
//! computer's image does no SimpleX), on a stack that runs doors
//! (`--only computers,simplex-door`), in Chrome, each step's screenshot at
//! 1280 and 390 wide, light and dark, in its scratch (`simplex-setup/`):
//! - Settings' SimpleX row of the agent is its connector's: Set up makes
//!   one, bound as the agent's door, and opens its setup page in Settings'
//!   dialog, named for the agent; the page starts its door and asks a link
//!   by itself (its inbox's secret made by the browser, `connect`, `link`),
//!   nothing to fill in.
//! - Locally the door reaches no SimpleX server, so no invitation comes:
//!   the page says so in a few words, with Try again. Done, its row has
//!   Open, its chat.
//! - Its agent's "Reaches you in" offers SimpleX, the door's chat: picked,
//!   a routine that names no chat runs there (the bridge's `home_chat`
//!   finds the chat bound as its SimpleX door), its bell on that chat.
//! - With `simplex-link` named too (`--only computers,simplex-door,
//!   simplex-link`), the door reaches SimpleX's servers, as a deployment's
//!   does, and so does a SimpleX app of the run's own: the lab's client
//!   (images/bridge/tests/simplex/lab.Dockerfile) on SimpleX's preset
//!   servers, as a phone's app is. The page shows the invitation's QR code,
//!   which an independent reader (`zbarimg`) reads from the screenshot as
//!   the invitation's link; the phone connects with that link, and the page
//!   says whom it linked; the phone's message reaches the agent, whose
//!   reply comes back to the phone; Unlink ends it.
//!
//! Its connector is deleted at the end, its door with it (FIN-614 item 4).

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

use super::simplex_door::Agent;
use super::telegram_settings::shoot;
use crate::api::Api;
use crate::browser::{Browser, Page};
use crate::Suite;

/// The run's name that takes its Settings' SimpleX to a real link.
pub const LINK: &str = "simplex-link";
/// The setup page's frame, as its URL names it.
const SETUP: &str = "setup.html?name=";
const WAIT: Duration = Duration::from_secs(30);
/// How long a door takes to start, and SimpleX's servers to answer.
const SLOW: Duration = Duration::from_secs(300);
/// The lab's client (images/bridge/tests/support/simplex.rs `LAB_IMAGE`).
const LAB_IMAGE: &str = "fragment-simplex-lab:7.0.3";
/// The person's name in their SimpleX app.
const PERSON: &str = "Robin";

/// What the setup page says now: its line, its buttons and links, its
/// tags, and its QR code's side (0: none).
const SAID: &str = "({ line: document.querySelector('#setup > p')?.textContent ?? null, buttons: [...document.querySelectorAll('#setup button, #setup .setup-row a')].map((x) => x.textContent), tags: [...document.querySelectorAll('#setup .setup-tag')].map((t) => t.textContent), qr: document.querySelector('#setup svg.setup-qr')?.getBoundingClientRect().width ?? 0 })";

fn framed(b: &mut Browser, expr: &str) -> Value {
    b.eval_in_isolated_frame(SETUP, expr).unwrap_or(Value::Null)
}

/// The setup page in its frame, once `done` takes what it says (within `within`).
fn until_said(s: &Suite, b: &mut Browser, within: Duration, done: impl Fn(&Value) -> bool) -> Value {
    let mut said = Value::Null;
    s.eventually(within, || {
        said = framed(b, SAID);
        done(&said)
    });
    said
}

fn press(b: &mut Browser, text: &str) -> bool {
    framed(b, &format!("(() => {{ const x = [...document.querySelectorAll('#setup button')].find((x) => x.textContent === {text:?}); if (!x || x.disabled) return false; x.click(); return true; }})()")) == true
}

/// Settings' SimpleX row of `agent`: its buttons, whether it is its
/// connector's (not its computer's), its Open's chat; null while not drawn.
fn row(agent: &str) -> String {
    format!(
        "(() => {{ const s = document.getElementById('settings-simplex'); const go = s?.querySelector('[data-simplex-for={agent:?}]'); const line = go?.closest('.settings-line'); if (!s || s.hidden || !line || line.hidden) return null; return {{ buttons: [...line.querySelectorAll('button')].map((x) => x.textContent), connector: !('simplexComputer' in go.dataset), open: line.querySelector('[data-simplex-open]')?.dataset.simplexOpen ?? null }}; }})()"
    )
}

/// The connector's state, as its owner's query reads it.
fn state(api: &Api, a: &Agent<'_>, name: &str) -> Value {
    api.op(a.owner, name, "state", "state", json!({})).map(|r| r.body["result"].clone()).unwrap_or_default()
}

/// Each shot light, then dark (the shell's dark, which it tells the frame).
fn shoot_both(b: &mut Browser, page: &Page, dir: &Path, name: &str, focus: Option<&str>) -> Result<()> {
    shoot(b, page, dir, name, focus)?;
    b.color_scheme(page, "dark")?;
    shoot(b, page, dir, &format!("{name}-dark"), focus)?;
    b.color_scheme(page, "light")?;
    Ok(())
}

pub(super) fn from_settings(s: &mut Suite, api: &Api, a: &Agent<'_>) -> Result<()> {
    if !s.runs_doors() {
        s.skip("SimpleX set up from Settings through its connector", "the stack runs no doors (cargo xtask e2e --only computers,simplex-door)");
        return Ok(());
    }
    let real = s.names(LINK);
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for Settings' SimpleX (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let shots = s.dir("simplex-setup");
    let session = api.sign_in(&Api::email_of(a.owner))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/settings", api.base))?;
    b.viewport(&page, 1280, 900, false)?;
    let mut seen = Value::Null;
    s.eventually(WAIT, || {
        seen = b.eval(&page, &row(a.agent)).unwrap_or(Value::Null);
        seen["buttons"] == json!(["Set up"])
    });
    let tags = b.eval(&page, "[...document.querySelectorAll('#settings-simplex .connector-tag')].map((t) => t.textContent)")?;
    s.ok(
        "Settings' SimpleX row of an agent whose computer does no SimpleX (the stub's, as every agent's own computer's) is its connector's: Set up",
        seen["connector"] == true && tags == json!(["Experimental", "Secure"]),
        json!({ "row": seen, "tags": tags }),
    );
    shoot_both(&mut b, &page, &shots, "a1-settings", Some("#settings-simplex"))?;

    // Set up: its connector made, bound as the agent's door, its setup page
    // in the dialog, which starts its door and asks a link by itself
    b.eval(&page, &format!("(document.querySelector('#settings-simplex [data-simplex-for={:?}]')?.click(), true)", a.agent))?;
    let mut dialog = Value::Null;
    s.eventually(WAIT, || {
        dialog = b.eval(&page, "(() => { const d = document.querySelector('dialog.connector-setup[open]'); return d && { title: d.querySelector('h2')?.textContent, fragment: d.querySelector('iframe')?.dataset.fragment ?? null, src: decodeURIComponent(d.querySelector('iframe')?.dataset.src ?? '') }; })()").unwrap_or(Value::Null);
        dialog["fragment"].is_string()
    });
    let name = dialog["fragment"].as_str().unwrap_or_default().to_string();
    let bound = api.signed(a.owner, "GET", &format!("/api/f/{name}/door"), None)?;
    let connected = s.eventually(SLOW, || state(api, a, &name)["connected"] == true);
    s.ok(
        "its Set up makes its connector, bound as the agent's SimpleX door, and opens its setup page in Settings' dialog, named for the agent; the page starts its door by itself",
        dialog["title"] == "SimpleX" && dialog["src"].as_str().is_some_and(|src| src.contains(SETUP)) && bound.body["bound"] == json!({ "door": "simplex", "agent": a.agent }) && connected,
        json!({ "dialog": dialog, "bound": bound.body, "state": state(api, a, &name) }),
    );
    let status = api.signed(a.owner, "GET", &format!("/api/f/{name}/status"), None)?;
    let door = fragment_core::door::door_id(&name, status.body["npub"].as_str().unwrap_or_default()).with_context(|| format!("its door's id: {status}"))?;

    if real {
        linked_for_real(s, api, a, &mut b, &page, &shots, &name)?;
    } else {
        // no SimpleX server reachable here: no invitation, said so
        let said = until_said(s, &mut b, SLOW, |x| x["line"] == "Couldn't reach SimpleX. Try again.");
        s.ok(
            "its door reaching no SimpleX server (the e2e's reach none), its link finds no invitation, and the page says so in a few words, with Try again",
            said["buttons"] == json!(["Try again"]) && said["tags"] == json!(["Experimental", "Secure"]) && state(api, a, &name)["why"]["text"] == "no invitation yet",
            json!({ "said": said, "state": state(api, a, &name) }),
        );
        shoot_both(&mut b, &page, &shots, "a2-no-server", None)?;
    }

    // Done: its row has Open, its chat
    b.eval(&page, "([...document.querySelectorAll('dialog.connector-setup .rename-actions button')].find((x) => x.textContent === 'Done')?.click(), true)")?;
    s.eventually(WAIT, || {
        seen = b.eval(&page, &row(a.agent)).unwrap_or(Value::Null);
        seen["open"] == name.as_str()
    });
    s.ok("Done, its row has Open, its connector's chat, and Set up", seen["buttons"] == json!(["Open", "Set up"]) && seen["connector"] == true, &seen);
    shoot_both(&mut b, &page, &shots, "a5-settings-done", Some("#settings-simplex"))?;
    home_in_its_door(s, api, a, &mut b, &page, &shots, &name)?;

    // deleted, its door ends with it (FIN-614 item 4)
    let deleted = api.signed(a.owner, "DELETE", &format!("/api/f/{name}"), None)?;
    let (ended, files) = super::simplex_door::ended(api, &door, SLOW)?;
    s.ok(
        "its connector deleted, its door ends with it: its saves' files and its storage gone",
        deleted.status == 200 && ended && files.as_array().is_some_and(Vec::is_empty),
        json!({ "deleted": deleted.status, "ended": ended, "files": files }),
    );
    Ok(())
}

/// Its agent's "Reaches you in": SimpleX offered once its door is bound,
/// picked, and a routine that names no chat runs in the door's chat; then
/// back to its own chat, as the lanes after it expect.
fn home_in_its_door(s: &mut Suite, api: &Api, a: &Agent<'_>, b: &mut Browser, page: &Page, shots: &Path, door: &str) -> Result<()> {
    let pick = format!("document.querySelector('select[data-home-for={:?}]')", a.agent);
    let offered = s.eventually(WAIT, || b.eval(page, &format!("(() => {{ const p = {pick}; return !!p && p.dataset.read === '1' && [...p.options].some((o) => o.value === 'simplex'); }})()")).is_ok_and(|v| v == true));
    let choose = |b: &mut Browser, value: &str| b.eval(page, &format!("(() => {{ const p = {pick}; p.value = {value:?}; p.dispatchEvent(new Event('change')); return true; }})()"));
    let home_now = || api.signed(a.owner, "GET", &format!("/api/f/{}/file?path=agent.json", a.agent), None).ok().and_then(|r| serde_json::from_str::<Value>(&r.text).ok()).and_then(|j| j.get("home").cloned());
    choose(b, "simplex")?;
    let picked = s.eventually(WAIT, || home_now() == Some(json!("simplex")));
    // its bell in the list, on the door's chat (`data-key`: `chat:<name>`)
    let bell = s.eventually(WAIT, || b.eval(page, &format!("[...document.querySelectorAll('#chats .home-mark')].some((m) => m.closest('[data-key]')?.dataset.key === {:?})", format!("chat:{door}"))).is_ok_and(|v| v == true));
    let _ = b.eval(page, &format!("({pick}?.scrollIntoView({{ block: 'center' }}), true)"));
    shoot_both(b, page, shots, "a6-home-simplex", None)?;
    let routine = json!({ "kind": "routine", "text": "say hi through the door" });
    let scheduled = api.signed(a.owner, "POST", &format!("/api/f/{}/schedules", a.agent), Some(&json!({ "cron": "0 0 1 1 *", "channel": "tasks", "record": routine })))?;
    let fired = super::computers::cron_now(api, a.agent)?;
    let ran = s.eventually(a.wake + Duration::from_secs(60), || {
        super::computers::agent_replies(&super::jobs::records(api, a.owner, door, "chat"), a.identity).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains("[your routine] say hi through the door")))
    });
    s.ok(
        "its agent's \"Reaches you in\" offers SimpleX once its door is bound; picked, its bell is on the door's chat, and a routine that names no chat runs there",
        offered && picked && bell && scheduled.status == 200 && fired.body["records"] == 1 && ran,
        json!({ "offered": offered, "picked": picked, "bell": bell, "scheduled": scheduled.status, "fired": fired.body, "replies": super::computers::agent_replies(&super::jobs::records(api, a.owner, door, "chat"), a.identity) }),
    );
    let id = scheduled.body["schedule"]["id"].as_i64().unwrap_or(0);
    let removed = api.signed(a.owner, "DELETE", &format!("/api/f/{}/schedules/{id}", a.agent), None)?;
    choose(b, "")?;
    let back = s.eventually(WAIT, || home_now().is_none());
    anyhow::ensure!(removed.status == 200 && back, "its routine removed ({}) and its home its own chat again ({back})", removed.status);
    Ok(())
}

/// The setup page's invitation, read by its QR code, linked by a SimpleX
/// app of the run's own; its message to the agent and the reply; Unlink.
fn linked_for_real(s: &mut Suite, api: &Api, a: &Agent<'_>, b: &mut Browser, page: &Page, shots: &Path, name: &str) -> Result<()> {
    let said = until_said(s, b, SLOW, |x| x["qr"].as_f64().is_some_and(|w| w > 100.0));
    let invitation = state(api, a, name)["invitation"].clone();
    let link = invitation["short"].as_str().or(invitation["link"].as_str()).unwrap_or_default().to_string();
    shoot_both(b, page, shots, "a2-qr", None)?;
    // the QR code as a phone's camera reads it, from the screenshot
    let read = read_qr(&shots.join("1280-a2-qr.png"));
    s.ok(
        "its door reaching SimpleX's servers, the page shows the invitation's QR code, with Open in SimpleX and Copy link; an independent reader reads the code as the invitation's link",
        said["line"] == "Scan with SimpleX, or open the link." && said["buttons"] == json!(["Open in SimpleX", "Copy link"]) && !link.is_empty() && read.as_deref() == Some(link.as_str()),
        json!({ "said": said, "invitation": { "short": invitation["short"].is_string(), "qr": invitation["qr"]["size"] }, "read": read.as_deref() == Some(link.as_str()) }),
    );

    let mut phone = Phone::start(&s.dir("simplex-phone"), &s.name("phone"))?;
    let linked_up = (|| -> Result<(i64, Value)> {
        let r = phone.cmd(&format!("/c {link}"))?;
        ensure!(!tag(&r).contains("rror"), "the phone's connect: {r}");
        let c = phone.wait(SLOW, |e| tag(e) == "contactConnected")?;
        Ok((c["contact"]["contactId"].as_i64().unwrap_or(0), c))
    })();
    let (contact, connected) = match linked_up {
        Ok(c) => c,
        Err(e) => {
            s.ok("a SimpleX app connects with the invitation's link", false, format!("{e:#}\n{}", phone.logs()));
            return Ok(());
        }
    };
    let said = until_said(s, b, SLOW, |x| x["line"] == format!("Linked with {PERSON}."));
    s.ok(
        "a SimpleX app on SimpleX's servers, as a phone's, connects with that link: the page says whom it linked, with Unlink",
        said["buttons"] == json!(["Unlink"]) && state(api, a, name)["linked"]["name"] == PERSON && state(api, a, name)["invitation"].is_null(),
        json!({ "said": said, "connected": connected["contact"]["localDisplayName"], "state": state(api, a, name) }),
    );
    shoot_both(b, page, shots, "a3-linked", None)?;

    // the phone's message reaches the agent, whose reply comes back
    let text = "hello from my phone";
    let sent = phone.say(contact, text);
    let back = phone.received(a.wake + SLOW, |got| got.iter().any(|g| g.contains(text)));
    s.ok(
        "the phone's message reaches the agent through the connector, and its reply comes back to the phone",
        sent.is_ok() && back.as_ref().is_ok_and(|got| got.iter().any(|g| g.contains(text))),
        json!({ "sent": sent.map_err(|e| e.to_string()), "received": back.map_err(|e| e.to_string()) }),
    );

    // Unlink: its link ends, and the page offers Link
    let unlinked = press(b, "Unlink");
    let said = until_said(s, b, SLOW, |x| x["line"] == "Not linked.");
    s.ok(
        "Unlink ends its link: the page says it is not linked, with Link",
        unlinked && said["buttons"] == json!(["Link"]) && state(api, a, name)["linked"].is_null(),
        json!({ "said": said, "state": state(api, a, name) }),
    );
    shoot_both(b, page, shots, "a4-unlinked", None)?;
    phone.stop();
    Ok(())
}

/// The text a QR code in `png` holds, as `zbarimg` reads it (none: no
/// reader here, or no code).
fn read_qr(png: &Path) -> Option<String> {
    let out = Command::new("zbarimg").args(["--raw", "-q"]).arg(png).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn tag(v: &Value) -> String {
    v["type"].as_str().unwrap_or("?").to_string()
}

fn docker(args: &[&str]) -> Result<String> {
    let out = Command::new("docker").args(args).output().context("docker runs")?;
    ensure!(out.status.success(), "docker {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A SimpleX app of the run's own, as a phone's is: the lab's client on
/// SimpleX's preset servers, its database in the run's scratch, its bot
/// API on loopback (`{"corrId", "cmd"}` out; `{"corrId", "resp"}` its
/// answer, `{"resp"}` what it says on its own).
struct Phone {
    ctr: String,
    ws: tungstenite::WebSocket<TcpStream>,
    next: u64,
    events: Vec<Value>,
}

impl Phone {
    fn start(dir: &Path, name: &str) -> Result<Phone> {
        if docker(&["image", "inspect", "--format", "{{.Id}}", LAB_IMAGE]).is_err() {
            let context: PathBuf = fragment_devstack::repo_root().join("images/bridge/tests/simplex");
            docker(&["build", "--platform", "linux/amd64", "-q", "-f", &context.join("lab.Dockerfile").to_string_lossy(), "-t", LAB_IMAGE, &context.to_string_lossy()])?;
        }
        for sub in ["files", "tmp"] {
            std::fs::create_dir_all(dir.join(sub))?;
        }
        let id = |flag: &str| Command::new("id").arg(flag).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let (user, vol) = (format!("{}:{}", id("-u"), id("-g")), format!("{}:/data", dir.display()));
        docker(&[
            "run", "-d", "--name", name, "--user", &user, "-e", "HOME=/data", "-e", "LAB_API_PORT=5226", "-p", "127.0.0.1::5226", "-v", &vol, LAB_IMAGE, "-d", "/data/sx", "-p", "5225", "--user-display-name", PERSON, "-y",
            "--files-folder", "/data/files", "--temp-folder", "/data/tmp",
        ])?;
        let addr = docker(&["port", name, "5226/tcp"])?.lines().next().unwrap_or_default().to_string();
        let t0 = Instant::now();
        // bounded: a minute for its core to start
        let ws = loop {
            let tried = TcpStream::connect(&addr).map_err(anyhow::Error::from).and_then(|tcp| tungstenite::client(format!("ws://{addr}"), tcp).map(|(ws, _)| ws).map_err(|e| anyhow::anyhow!("{e}")));
            match tried {
                Ok(ws) => break ws,
                Err(e) if t0.elapsed() > Duration::from_secs(60) => return Err(e.context(format!("{name}'s bot API"))),
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        };
        ws.get_ref().set_read_timeout(Some(Duration::from_millis(250)))?;
        Ok(Phone { ctr: name.to_string(), ws, next: 1, events: vec![] })
    }

    /// The next frame, within a read's timeout.
    fn read(&mut self) -> Result<Option<Value>> {
        match self.ws.read() {
            Ok(tungstenite::Message::Text(t)) => Ok(Some(serde_json::from_str(t.as_str())?)),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Sends `cmd`; its answer, what it said meanwhile kept.
    fn cmd(&mut self, cmd: &str) -> Result<Value> {
        let id = self.next.to_string();
        self.next += 1;
        self.ws.send(tungstenite::Message::text(json!({ "corrId": id, "cmd": cmd }).to_string()))?;
        let t0 = Instant::now();
        // bounded: a minute for its answer
        while t0.elapsed() < Duration::from_secs(60) {
            if let Some(v) = self.read()? {
                if v["corrId"].as_str() == Some(id.as_str()) {
                    return Ok(v["resp"].clone());
                }
                self.events.push(v["resp"].clone());
            }
        }
        anyhow::bail!("no answer to `{cmd}` in 60 s")
    }

    /// The first thing it said that `pred` takes, within `within`.
    fn wait(&mut self, within: Duration, pred: impl Fn(&Value) -> bool) -> Result<Value> {
        let t0 = Instant::now();
        // bounded by `within`
        loop {
            if let Some(i) = self.events.iter().position(&pred) {
                return Ok(self.events.remove(i));
            }
            ensure!(t0.elapsed() < within, "nothing such in {within:?}: {:?}", self.events.iter().map(tag).collect::<Vec<_>>());
            if let Some(v) = self.read()? {
                self.events.push(v["resp"].clone());
            }
        }
    }

    fn say(&mut self, contact: i64, text: &str) -> Result<()> {
        let body = json!([{ "msgContent": { "type": "text", "text": text } }]);
        let r = self.cmd(&format!("/_send @{contact} json {body}"))?;
        ensure!(tag(&r) == "newChatItems", "the phone's send: {r}");
        Ok(())
    }

    /// What it received from its contacts, in order, once `done` takes it.
    fn received(&mut self, within: Duration, done: impl Fn(&[String]) -> bool) -> Result<Vec<String>> {
        let mut got = vec![];
        let t0 = Instant::now();
        // bounded by `within`
        loop {
            for e in std::mem::take(&mut self.events) {
                if tag(&e) != "newChatItems" {
                    continue;
                }
                for ci in e["chatItems"].as_array().into_iter().flatten() {
                    let item = &ci["chatItem"];
                    if item["chatDir"]["type"] == "directRcv" && item["content"]["type"] == "rcvMsgContent" {
                        got.push(item["content"]["msgContent"]["text"].as_str().unwrap_or("").to_string());
                    }
                }
            }
            if done(&got) {
                return Ok(got);
            }
            ensure!(t0.elapsed() < within, "not received in {within:?}: {got:?}");
            if let Some(v) = self.read()? {
                self.events.push(v["resp"].clone());
            }
        }
    }

    fn logs(&self) -> String {
        Command::new("docker").args(["logs", "--tail", "40", &self.ctr]).output().map(|o| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))).unwrap_or_default()
    }

    fn stop(&self) {
        let _ = docker(&["rm", "-f", &self.ctr]);
    }
}

impl Drop for Phone {
    fn drop(&mut self) {
        self.stop();
    }
}
