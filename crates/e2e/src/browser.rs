//! A headless Chrome driven over the DevTools protocol: enough to open
//! pages, evaluate JavaScript in them, and wait for what they show. Chrome
//! resolves `*.localhost` to the loopback address, so pages open on each
//! fragment's own origin exactly as a person's browser would.
//!
//! Why direct CDP (tooling review, 2026-10-30): chromiumoxide 0.9.1 has
//! 117 normal transitive packages, headless_chrome 1.0.22 without default
//! features 91, and fantoccini 0.22.1 with rustls only 108 (isolated cargo
//! tree -e normal, unique package/version pairs, excluding the named crate).
//! All have upstream activity, but none is a small replacement for send's
//! synchronous request/answer loop over the tungstenite we already use.
//! Context leases, bounded event evidence, held requests, third-party-cookie
//! blocking and teardown would still belong here; fantoccini also needs a
//! WebDriver server. Keep this adapter until a leaner transport removes more
//! machinery than it adds while preserving those contracts.

use std::cell::Cell;
use std::collections::HashMap;
use std::net::TcpStream;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Chrome starts tried before a lane gives up on it, and how long each may
/// take to answer on its DevTools port.
const CHROME_STARTS_MAX: u32 = 2;
const CHROME_START_WAIT: Duration = Duration::from_secs(30);
/// How long a reloaded page has to load its new document, resources and
/// all (`reload`), and how many reloads it is given.
const RELOAD_LOAD: Duration = Duration::from_secs(30);
const RELOADS_MAX: u32 = 2;
/// A watched page keeps at most this many of its events; past it, they are
/// counted (`Watched::dropped`). A small app's page says a few hundred.
pub const WATCHED_EVENTS_MAX: usize = 20_000;

pub struct Browser {
    child: Child,
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
    /// The browser context pages open in (`None`: Chrome's default one).
    context: Option<String>,
    /// More contexts the lease made (`another_context`), ended with it.
    others: Vec<String>,
    /// Chrome's own state for this launch, removed when it stops: never
    /// evidence, and tens of megabytes once a page has loaded.
    profile: PathBuf,
    /// The events of each watched page (`open_watched`), by its session.
    watched: HashMap<String, Watched>,
    /// Each holding page's held requests (`hold`), by its session: their
    /// ids and URLs.
    held: HashMap<String, Vec<(String, String)>>,
}

/// What a watched page's target said as it loaded and ran (its `Runtime`,
/// `Log` and `Network` events, as Chrome sent them, in order).
#[derive(Debug, Clone, Default)]
pub struct Watched {
    pub events: Vec<Value>,
    /// Events past `WATCHED_EVENTS_MAX`, counted and not kept.
    pub dropped: u64,
}

impl Watched {
    /// One more of its events: kept while it holds fewer than
    /// `WATCHED_EVENTS_MAX`, counted past it (the first ones kept, so what
    /// it did after them is unread: `dropped` says so).
    pub fn keep(&mut self, event: Value) {
        match self.events.len() < WATCHED_EVENTS_MAX {
            true => self.events.push(event),
            false => self.dropped += 1,
        }
    }
}

/// One Chrome for a whole run, started when a lane first asks for it.
/// Each lane leases it with a browser context of its own: its own cookies,
/// storage, cache, and service workers, as a fresh profile has, so no lane
/// sees another's session. The lease's pages close when it ends.
pub struct Shared {
    scratch: PathBuf,
    /// The browser between leases (`None`: not started, or out on a lease).
    idle: Rc<Cell<Option<Browser>>>,
}

impl Shared {
    pub fn new(scratch: &Path) -> Shared {
        Shared { scratch: scratch.to_path_buf(), idle: Rc::new(Cell::new(None)) }
    }

    /// The browser in a fresh context (`None` when no Chrome is installed).
    pub fn lease(&self) -> Result<Option<Lease>> {
        let mut browser = match self.idle.take() {
            Some(browser) => browser,
            None => match Browser::launch(&self.scratch)? {
                Some(browser) => browser,
                None => return Ok(None),
            },
        };
        // on an error the browser is dropped (Chrome stops); the next lease starts another
        browser.begin_context()?;
        Ok(Some(Lease { browser: Some(browser), idle: Rc::clone(&self.idle) }))
    }

    /// Stops Chrome (a lease still out stops it when it ends).
    pub fn close(&self) {
        drop(self.idle.take());
    }
}

/// A lane's hold on the shared browser; it ends when dropped, a lane that
/// stopped early included.
pub struct Lease {
    /// `Some` until the lease ends.
    browser: Option<Browser>,
    idle: Rc<Cell<Option<Browser>>>,
}

impl Deref for Lease {
    type Target = Browser;
    fn deref(&self) -> &Browser {
        self.browser.as_ref().expect("a lease holds its browser until it ends")
    }
}

impl DerefMut for Lease {
    fn deref_mut(&mut self) -> &mut Browser {
        self.browser.as_mut().expect("a lease holds its browser until it ends")
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(mut browser) = self.browser.take() else { return };
        // a browser whose context does not close is not lent again: dropped
        // here, Chrome stops, and the next lease starts another
        if browser.end_context().is_ok() {
            self.idle.set(Some(browser));
        }
    }
}

/// One tab, addressed by its DevTools session.
pub struct Page {
    session: String,
    target: String,
}

/// For `open_with_script`: the page's first WebSocket whose URL holds
/// `part`, held as an upgrade never answered is (a dropped connection, a
/// phone's network changing under it): it stays connecting, never opening
/// or closing by itself, until the page closes it, which closes it as a
/// browser's does (code 1006, not clean). `window.__heldSockets` counts
/// those held, `__heldClosed` those the page closed. Its top window's alone.
pub fn hold_first_socket(part: &str) -> String {
    let part = serde_json::to_string(part).expect("a string encodes");
    format!(
        "(() => {{ if (window !== window.top) return; const part = {part}; const Real = window.WebSocket; let held = false; \
         Object.assign(window, {{ __heldSockets: 0, __heldClosed: 0 }}); \
         class Held extends EventTarget {{ \
           constructor(url) {{ super(); Object.assign(this, {{ url: String(url), readyState: 0, protocol: '', extensions: '', binaryType: 'blob', bufferedAmount: 0 }}); window.__heldSockets++; }} \
           send() {{ throw new DOMException('the socket is not open', 'InvalidStateError'); }} \
           close() {{ if (this.readyState >= 2) return; this.readyState = 3; window.__heldClosed++; \
             setTimeout(() => {{ const e = new CloseEvent('close', {{ code: 1006, reason: '', wasClean: false }}); this.onclose?.(e); this.dispatchEvent(e); }}, 0); }} \
         }} \
         window.WebSocket = new Proxy(Real, {{ construct(target, args) {{ if (!held && String(args[0]).includes(part)) {{ held = true; return new Held(args[0]); }} return new target(...args); }} }}); \
         }})();"
    )
}

/// A second person's browser beside the lease's own: a browser context of
/// its own in the same Chrome (its own cookies, storage, and cache), which
/// ends with the lease. Its pages open with `Browser::open_in`.
pub struct BrowserContext(String);

/// A cookie as a response from `url` sets it: over https (a preview), one
/// of the cell's host cookies under its `__Host-` name, Secure at the
/// host's root, as the cell itself names it there (api.rs `https_cookies`).
fn cookie(url: &str, name: &str, value: &str) -> Value {
    let https = url.starts_with("https://");
    match https && crate::api::HOST_COOKIES.contains(&name) {
        true => json!({ "name": format!("{}{name}", crate::api::HOST_PREFIX), "value": value, "url": url, "path": "/", "secure": true, "httpOnly": true, "sameSite": "Lax" }),
        false => json!({ "name": name, "value": value, "url": url, "httpOnly": true, "sameSite": "Lax" }),
    }
}

/// One of Chrome's DevTools messages, read. A request's body in an event
/// (`Fetch.requestPaused`'s `postData`, a held upload's image) is its bytes
/// as text, each byte that is no UTF-8 a lone UTF-16 surrogate's `\u`
/// escape, which serde_json refuses: such a message is read again with
/// each surrogate's escape as U+FFFD's.
fn message(t: &str) -> Result<Value> {
    serde_json::from_str(t).or_else(|_| Ok(serde_json::from_str(&without_surrogates(t))?))
}

/// JSON text with each `\uD800`…`\uDFFF` escape made `\ufffd`; every other
/// escape, and every other byte, as it was.
fn without_surrogates(t: &str) -> String {
    let b = t.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let code = b.get(i + 2..i + 6).filter(|_| b.get(i + 1) == Some(&b'u')).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u16::from_str_radix(h, 16).ok());
        match code {
            Some(c) if (0xD800..=0xDFFF).contains(&c) => {
                out.extend_from_slice(b"\\ufffd");
                i += 6;
            }
            // an escape is two bytes, both ASCII (`\\` among them, so the
            // backslash it escapes starts nothing)
            _ => {
                out.extend_from_slice(&b[i..(i + 2).min(b.len())]);
                i += 2;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn chrome() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CHROME_BIN") {
        return Some(PathBuf::from(p));
    }
    ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Chromium.app/Contents/MacOS/Chromium", "/usr/bin/google-chrome", "/usr/bin/chromium"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

impl Browser {
    /// `None` when no Chrome is installed (the section says so and skips).
    pub fn launch(scratch: &Path) -> Result<Option<Browser>> {
        Browser::launch_with(scratch, &[], None)
    }

    /// A Chrome of a lane's own: `args` added to its command line, and
    /// `preferences` (Chrome's own settings file) in its profile first.
    pub fn launch_with(scratch: &Path, args: &[&str], preferences: Option<&Value>) -> Result<Option<Browser>> {
        let Some(bin) = chrome() else { return Ok(None) };
        // a start that never comes up (a slow runner, a port taken between
        // free_port and Chrome's bind) is tried once more, on a fresh port
        let mut tries = 0;
        let (child, url, profile) = loop {
            tries += 1;
            match Browser::start(&bin, scratch, args, preferences) {
                Ok(started) => break started,
                Err(e) if tries < CHROME_STARTS_MAX => eprintln!("      (Chrome did not come up, trying again: {e:#})"),
                Err(e) => return Err(e),
            }
        };
        let (ws, _) = tungstenite::connect(url.as_str())?;
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_secs(30)))?;
        }
        Ok(Some(Browser { child, ws, next: 0, context: None, others: vec![], profile, watched: HashMap::new(), held: HashMap::new() }))
    }

    /// One Chrome, up: its process, its DevTools socket's URL, and its
    /// profile, whose `chrome.log` keeps what it said (a start that fails
    /// says why).
    fn start(bin: &Path, scratch: &Path, args: &[&str], preferences: Option<&Value>) -> Result<(Child, String, PathBuf)> {
        let port = fragment_devstack::free_port()?;
        let profile = scratch.join(format!("chrome-{port}"));
        std::fs::create_dir_all(profile.join("Default"))?;
        if let Some(p) = preferences {
            std::fs::write(profile.join("Default/Preferences"), p.to_string())?;
        }
        let log = std::fs::File::create(profile.join("chrome.log"))?;
        let mut child = Command::new(bin)
            .args(args)
            .args([
                "--headless=new",
                &format!("--remote-debugging-port={port}"),
                &format!("--user-data-dir={}", profile.display()),
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-gpu",
                "--disable-extensions",
                // two people's windows at once (the phase7 section): one
                // that covers another must not hide it, or the hidden page
                // stops drawing (no requestAnimationFrame) and pausing its timers
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-background-timer-throttling",
                // a microphone that plays a tone, granted without a prompt:
                // a chat's voice memo (the chat section) records from it
                "--use-fake-device-for-media-stream",
                "--use-fake-ui-for-media-stream",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .context("start Chrome")?;
        let t0 = Instant::now();
        let said = || std::fs::read_to_string(profile.join("chrome.log")).unwrap_or_default().lines().rev().take(5).collect::<Vec<_>>().join(" | ");
        loop {
            let found = reqwest::blocking::get(format!("http://127.0.0.1:{port}/json/version"))
                .ok()
                .and_then(|r| r.json::<Value>().ok())
                .and_then(|v| v["webSocketDebuggerUrl"].as_str().map(str::to_string));
            if let Some(u) = found {
                return Ok((child, u, profile));
            }
            if let Ok(Some(status)) = child.try_wait() {
                bail!("Chrome exited ({status}) before its DevTools endpoint came up on :{port}: {}", said());
            }
            if t0.elapsed() > CHROME_START_WAIT {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Chrome's DevTools endpoint did not come up on :{port} in {CHROME_START_WAIT:?}: {}", said());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Pages open in a new browser context from here on.
    fn begin_context(&mut self) -> Result<()> {
        assert!(self.context.is_none(), "one browser context at a time");
        let made = self.send("Target.createBrowserContext", json!({}), None)?;
        let id = made["browserContextId"].as_str().context("Target.createBrowserContext answers its id")?;
        self.context = Some(id.to_string());
        Ok(())
    }

    /// Disposes of the context, and any more the lease made, closing their pages.
    fn end_context(&mut self) -> Result<()> {
        let id = self.context.take().expect("a browser context to end");
        // its pages close with it
        self.watched.clear();
        for other in std::mem::take(&mut self.others) {
            self.send("Target.disposeBrowserContext", json!({ "browserContextId": other }), None)?;
        }
        self.send("Target.disposeBrowserContext", json!({ "browserContextId": id }), None)?;
        Ok(())
    }

    /// Another browser context for this lease: a second person's browser,
    /// signed in to nothing, ended with the lease.
    pub fn another_context(&mut self) -> Result<BrowserContext> {
        assert!(self.context.is_some(), "a leased browser makes more contexts");
        let made = self.send("Target.createBrowserContext", json!({}), None)?;
        let id = made["browserContextId"].as_str().context("Target.createBrowserContext answers its id")?.to_string();
        self.others.push(id.clone());
        Ok(BrowserContext(id))
    }

    /// Sends one command and waits for its answer (events are skipped, but
    /// a watched page's, which are kept).
    fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws.send(Message::Text(msg.to_string().into()))?;
        loop {
            let Message::Text(t) = self.ws.read()? else { continue };
            let v = message(&t)?;
            if v["id"].as_u64() == Some(id) {
                if !v["error"].is_null() {
                    bail!("{method}: {}", v["error"]);
                }
                return Ok(v["result"].clone());
            }
            self.keep(v);
        }
    }

    /// An event of a watched page's, kept (bounded); a request a holding
    /// page's `hold` paused, held.
    fn keep(&mut self, event: Value) {
        if !event["method"].is_string() {
            return;
        }
        if event["method"] == "Fetch.requestPaused" {
            if let Some(held) = event["sessionId"].as_str().and_then(|s| self.held.get_mut(s)) {
                let paused = &event["params"];
                held.push((paused["requestId"].as_str().unwrap_or("").to_string(), paused["request"]["url"].as_str().unwrap_or("").to_string()));
            }
        }
        if let Some(w) = event["sessionId"].as_str().and_then(|s| self.watched.get_mut(s)) {
            w.keep(event);
        }
    }

    /// Opens `url` in a page whose events are kept from before its first
    /// document: its uncaught exceptions and console (`Runtime`), what the
    /// browser logged (`Log`), and its requests and sockets, their frames
    /// included (`Network`), as a preview card's shot hears a page and more.
    pub fn open_watched(&mut self, url: &str) -> Result<Page> {
        let page = self.open("about:blank")?;
        self.watched.insert(page.session.clone(), Watched::default());
        for domain in ["Runtime", "Log", "Network"] {
            self.send(&format!("{domain}.enable"), json!({}), Some(&page.session))?;
        }
        self.send("Page.navigate", json!({ "url": url }), Some(&page.session))?;
        Ok(page)
    }

    /// What a watched page has said so far: every event Chrome sent before
    /// a command asked now answers.
    pub fn watched(&mut self, page: &Page) -> Result<Watched> {
        anyhow::ensure!(self.watched.contains_key(&page.session), "the page was opened watched");
        self.send("Runtime.evaluate", json!({ "expression": "0" }), Some(&page.session))?;
        Ok(self.watched.get(&page.session).cloned().unwrap_or_default())
    }

    /// The body of a response a watched page had (its request's id, from its
    /// `Network` events), as text; while the page is open.
    pub fn response_body(&mut self, page: &Page, request: &str) -> Result<String> {
        use base64::Engine;
        let r = self.send("Network.getResponseBody", json!({ "requestId": request }), Some(&page.session))?;
        let body = r["body"].as_str().unwrap_or_default();
        Ok(match r["base64Encoded"].as_bool() {
            Some(true) => String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(body)?).into_owned(),
            _ => body.to_string(),
        })
    }

    pub fn open(&mut self, url: &str) -> Result<Page> {
        let context = self.context.clone();
        self.open_at(context.as_deref(), url)
    }

    /// A browser-only transport fault installed before the page starts.
    pub fn open_with_script(&mut self, url: &str, script: &str) -> Result<Page> {
        let page = self.open("about:blank")?;
        self.send("Page.enable", json!({}), Some(&page.session))?;
        self.send("Page.addScriptToEvaluateOnNewDocument", json!({ "source": script }), Some(&page.session))?;
        self.send("Page.navigate", json!({ "url": url }), Some(&page.session))?;
        Ok(page)
    }

    /// Opens a page in another of the lease's contexts (`another_context`).
    pub fn open_in(&mut self, context: &BrowserContext, url: &str) -> Result<Page> {
        self.open_at(Some(&context.0), url)
    }

    /// A page whose requests to URLs matching `patterns` (`*` any run of
    /// characters) are refused from its first document, as a resource that
    /// never comes; `unblock` lets them through again.
    pub fn open_blocking(&mut self, url: &str, patterns: &[&str]) -> Result<Page> {
        let page = self.open("about:blank")?;
        self.send("Network.enable", json!({}), Some(&page.session))?;
        self.send("Network.setBlockedURLs", json!({ "urls": patterns }), Some(&page.session))?;
        self.send("Page.navigate", json!({ "url": url }), Some(&page.session))?;
        Ok(page)
    }

    pub fn unblock(&mut self, page: &Page) -> Result<()> {
        self.send("Network.setBlockedURLs", json!({ "urls": [] }), Some(&page.session))?;
        Ok(())
    }

    fn open_at(&mut self, context: Option<&str>, url: &str) -> Result<Page> {
        let mut params = json!({ "url": url });
        if let Some(context) = context {
            params["browserContextId"] = json!(context);
        }
        let target = self.send("Target.createTarget", params, None)?["targetId"].as_str().unwrap_or("").to_string();
        self.attach(&target)
    }

    /// A page `pages` listed (a popup a page opened), to drive as one `open` answered.
    pub fn attach(&mut self, target: &str) -> Result<Page> {
        let session = self.send("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None)?["sessionId"]
            .as_str()
            .unwrap_or("")
            .to_string();
        Ok(Page { session, target: target.to_string() })
    }

    pub fn close(&mut self, page: Page) -> Result<()> {
        self.watched.remove(&page.session);
        self.send("Target.closeTarget", json!({ "targetId": page.target }), None)?;
        Ok(())
    }

    /// The pages open in this lease's contexts, popups a page opened
    /// included: (target id, URL).
    pub fn pages(&mut self) -> Result<Vec<(String, String)>> {
        let v = self.send("Target.getTargets", json!({}), None)?;
        let context = self.context.clone();
        let ours = |id: &Value| context.as_deref().is_none_or(|c| id == c || self.others.iter().any(|o| id == o.as_str()));
        Ok(v["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page" && ours(&t["browserContextId"]))
            .map(|t| (t["targetId"].as_str().unwrap_or("").to_string(), t["url"].as_str().unwrap_or("").to_string()))
            .collect())
    }

    /// Evaluates `expr` in the page (awaiting a promise) and returns its value.
    pub fn eval(&mut self, page: &Page, expr: &str) -> Result<Value> {
        let r = self.send("Runtime.evaluate", json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }), Some(&page.session))?;
        if let Some(e) = r.get("exceptionDetails") {
            bail!("the page threw: {}", e["exception"]["description"].as_str().unwrap_or(&e.to_string()));
        }
        Ok(r["result"]["value"].clone())
    }

    /// Waits until `expr` is truthy in the page.
    #[track_caller]
    pub fn until(&mut self, page: &Page, expr: &str, timeout: Duration) -> bool {
        let at = std::panic::Location::caller();
        let t0 = Instant::now();
        while t0.elapsed() < timeout {
            if self.eval(page, &format!("!!({expr})")).ok() == Some(Value::Bool(true)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // as `Suite::eventually` says it: a wait that ran out, and where
        println!("      (a wait ran out its {timeout:.0?} at {}:{})", at.file(), at.line());
        false
    }

    /// Brings the page's window to the front, as a person looks at the
    /// window they are using: shown, it draws.
    pub fn front(&mut self, page: &Page) -> Result<()> {
        self.send("Page.bringToFront", json!({}), Some(&page.session))?;
        Ok(())
    }

    /// Reloads the page and waits until its new document has loaded
    /// (`readyState` complete; the old one, marked, is not taken for it).
    /// One still loading after RELOAD_LOAD is reloaded once more, said in
    /// the run's output: the local node can leave a request of a page's
    /// unanswered, and a module script that never arrives leaves a page
    /// whose script never runs (run 39632307934: two of the shell's
    /// modules never reached the node, and its page stayed blank). A page
    /// still loading after that is the caller's checks' to find.
    pub fn reload(&mut self, page: &Page) -> Result<()> {
        for tried in 1..=RELOADS_MAX {
            // (a page between documents answers once it has its new one)
            let _ = self.eval(page, "(window.__e2eBeforeReload = true, true)");
            self.send("Page.reload", json!({}), Some(&page.session))?;
            if self.until(page, "!window.__e2eBeforeReload && document.readyState === 'complete'", RELOAD_LOAD) {
                return Ok(());
            }
            let left = self.eval(page, "document.readyState").unwrap_or(Value::Null);
            println!("      (reload {tried} of {RELOADS_MAX}: the page did not load in {RELOAD_LOAD:.0?}, its document {left})");
        }
        Ok(())
    }

    /// The page as it looks now, a PNG at `path` (evidence for a person).
    pub fn screenshot(&mut self, page: &Page, path: &Path) -> Result<()> {
        use base64::Engine;
        let shot = self.send("Page.captureScreenshot", json!({ "format": "png" }), Some(&page.session))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(shot["data"].as_str().unwrap_or_default())?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Changes reachability while keeping the page's origin and identity.
    /// Every request of the page's own (its frames of other sites aside)
    /// answered `ms` late, a navigation's too, as a slow phone's are; 0 as
    /// they come.
    pub fn latency(&mut self, page: &Page, ms: u64) -> Result<()> {
        self.send("Network.enable", json!({}), Some(&page.session))?;
        self.send("Network.emulateNetworkConditions", json!({ "offline": false, "latency": ms, "downloadThroughput": -1, "uploadThroughput": -1 }), Some(&page.session))?;
        Ok(())
    }

    pub fn offline(&mut self, page: &Page, offline: bool) -> Result<()> {
        self.send("Network.enable", json!({}), Some(&page.session))?;
        self.send("Network.emulateNetworkConditions", json!({ "offline": offline, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 }), Some(&page.session))?;
        Ok(())
    }

    /// A cookie the browser holds for `url`, as if a response had set it.
    pub fn set_cookie(&mut self, url: &str, name: &str, value: &str) -> Result<()> {
        let mut params = json!({ "cookies": [cookie(url, name, value)] });
        if let Some(context) = &self.context {
            params["browserContextId"] = json!(context);
        }
        self.send("Storage.setCookies", params, None)?;
        Ok(())
    }

    /// The id DevTools knows the service worker registration at `scope` by,
    /// as `page`'s origin's registrations are reported once asked
    /// (`ServiceWorker.workerRegistrationUpdated`, after `enable`).
    pub fn registration_id(&mut self, page: &Page, scope: &str) -> Result<String> {
        self.next += 1;
        let asked = json!({ "id": self.next, "method": "ServiceWorker.enable", "params": {}, "sessionId": page.session });
        self.ws.send(Message::Text(asked.to_string().into()))?;
        let t0 = Instant::now();
        let mut found = None;
        while found.is_none() && t0.elapsed() < Duration::from_secs(30) {
            let Message::Text(t) = self.ws.read()? else { continue };
            let v = message(&t)?;
            if v["method"] == "ServiceWorker.workerRegistrationUpdated" {
                let registrations = v["params"]["registrations"].as_array().cloned().unwrap_or_default();
                found = registrations.iter().find(|r| r["scopeURL"] == scope && r["isDeleted"] != true).and_then(|r| r["registrationId"].as_str().map(str::to_string));
            }
        }
        found.with_context(|| format!("no service worker registration at {scope}"))
    }

    /// A running service worker of this lease's contexts whose script's URL
    /// contains `url_part`, attached (as `frame` attaches a frame), so its
    /// own requests can be refused (`block`).
    pub fn worker(&mut self, url_part: &str) -> Result<Page> {
        let v = self.send("Target.getTargets", json!({}), None)?;
        let target = v["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["type"] == "service_worker" && t["url"].as_str().is_some_and(|u| u.contains(url_part)))
            .and_then(|t| t["targetId"].as_str().map(str::to_string))
            .with_context(|| format!("no running service worker at {url_part}"))?;
        self.attach(&target)
    }

    /// A push message delivered to the registration DevTools knows as
    /// `registration` (`registration_id`), as a push service would deliver
    /// one: its worker's `push` event, with `data`.
    pub fn deliver_push(&mut self, page: &Page, origin: &str, registration: &str, data: &str) -> Result<()> {
        self.send("ServiceWorker.enable", json!({}), Some(&page.session))?;
        self.send("ServiceWorker.deliverPushMessage", json!({ "origin": origin, "registrationId": registration, "data": data }), Some(&page.session))?;
        Ok(())
    }

    /// Lets the lease's context save what it downloads into `dir` (a click
    /// on a link that answers an attachment).
    pub fn allow_downloads(&mut self, dir: &Path) -> Result<()> {
        let mut params = json!({ "behavior": "allow", "downloadPath": dir.to_string_lossy() });
        if let Some(context) = &self.context {
            params["browserContextId"] = json!(context);
        }
        self.send("Browser.setDownloadBehavior", params, None)?;
        Ok(())
    }

    /// What the lease's context tells pages of `origin` of the web
    /// permission `name` (`navigator.permissions`: "microphone") from here
    /// on: "granted", "denied" or "prompt", as a person's choice in the
    /// browser's settings. Chrome's fake media UI (its command line) still
    /// answers a page's request for the microphone yes.
    pub fn set_permission(&mut self, origin: &str, name: &str, setting: &str) -> Result<()> {
        let mut params = json!({ "permission": { "name": name }, "setting": setting, "origin": origin });
        if let Some(context) = &self.context {
            params["browserContextId"] = json!(context);
        }
        self.send("Browser.setPermission", params, None)?;
        Ok(())
    }

    /// `set_cookie` in another of the lease's contexts (a second person's browser).
    pub fn set_cookie_in(&mut self, context: &BrowserContext, url: &str, name: &str, value: &str) -> Result<()> {
        let params = json!({ "cookies": [cookie(url, name, value)], "browserContextId": context.0 });
        self.send("Storage.setCookies", params, None)?;
        Ok(())
    }

    /// Text typed into the focused element, as a keyboard's input (the
    /// page hears `input`, never a key it could tell from a person's).
    pub fn type_text(&mut self, page: &Page, text: &str) -> Result<()> {
        self.send("Input.insertText", json!({ "text": text }), Some(&page.session))?;
        Ok(())
    }

    /// One key pressed and let go on the focused element (`Enter`, `Escape`,
    /// `Tab`, `ArrowDown`, `ArrowUp`), with Shift held when `shift`.
    pub fn press(&mut self, page: &Page, key: &str, shift: bool) -> Result<()> {
        let code = match key {
            "Enter" => 13,
            "Escape" => 27,
            "Tab" => 9,
            "ArrowUp" => 38,
            "ArrowDown" => 40,
            _ => bail!("no key code for {key}"),
        };
        for kind in ["keyDown", "keyUp"] {
            let mut event = json!({ "type": kind, "key": key, "code": key, "windowsVirtualKeyCode": code, "modifiers": if shift { 8 } else { 0 } });
            // a key that types: Enter's keydown carries its character
            if kind == "keyDown" && key == "Enter" {
                event["text"] = json!("\r");
            }
            self.send("Input.dispatchKeyEvent", event, Some(&page.session))?;
        }
        Ok(())
    }

    /// Files chosen in the first file input `selector` matches, as a
    /// person picks them (the page hears `change`).
    pub fn choose_files(&mut self, page: &Page, selector: &str, files: &[&Path]) -> Result<()> {
        let doc = self.send("DOM.getDocument", json!({ "depth": 0 }), Some(&page.session))?;
        let found = self.send("DOM.querySelector", json!({ "nodeId": doc["root"]["nodeId"], "selector": selector }), Some(&page.session))?;
        let node = found["nodeId"].as_u64().filter(|n| *n != 0).with_context(|| format!("nothing on the page matches {selector}"))?;
        let files: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        self.send("DOM.setFileInputFiles", json!({ "nodeId": node, "files": files }), Some(&page.session))?;
        Ok(())
    }

    /// Every cookie the browser holds (in this lease's context): a
    /// partitioned one with its `partitionKey`.
    pub fn cookies(&mut self) -> Result<Vec<Value>> {
        let mut params = json!({});
        if let Some(context) = &self.context {
            params["browserContextId"] = json!(context);
        }
        Ok(self.send("Storage.getCookies", params, None)?["cookies"].as_array().cloned().unwrap_or_default())
    }

    /// A touch screen's pointer (`on`): coarse, five touch points, as a
    /// phone's; off, the page's own again (headless: none at all).
    pub fn touch(&mut self, page: &Page, on: bool) -> Result<()> {
        let params = if on { json!({ "enabled": true, "maxTouchPoints": 5 }) } else { json!({ "enabled": false }) };
        self.send("Emulation.setTouchEmulationEnabled", params, Some(&page.session))?;
        Ok(())
    }

    /// The page's viewport, as a phone's (`mobile`) or a desktop's.
    pub fn viewport(&mut self, page: &Page, width: u32, height: u32, mobile: bool) -> Result<()> {
        let params = json!({ "width": width, "height": height, "deviceScaleFactor": 1, "mobile": mobile });
        self.send("Emulation.setDeviceMetricsOverride", params, Some(&page.session))?;
        Ok(())
    }

    /// The page's `prefers-color-scheme`: `dark` or `light`.
    pub fn color_scheme(&mut self, page: &Page, scheme: &str) -> Result<()> {
        let params = json!({ "features": [{ "name": "prefers-color-scheme", "value": scheme }] });
        self.send("Emulation.setEmulatedMedia", params, Some(&page.session))?;
        Ok(())
    }

    /// Evaluates `expr` inside the page's first frame whose URL contains
    /// `url_part` (a same-site frame shares the page's process), in a world
    /// of its own that shares the frame's DOM. A frame of another site has
    /// a process of its own (site isolation), so it is a target of its own:
    /// it is evaluated there (`eval_in_isolated_frame`).
    pub fn eval_in_frame(&mut self, page: &Page, url_part: &str, expr: &str) -> Result<Value> {
        fn find(tree: &Value, part: &str) -> Option<String> {
            if tree["frame"]["url"].as_str().is_some_and(|u| u.contains(part)) {
                return tree["frame"]["id"].as_str().map(str::to_string);
            }
            tree["childFrames"].as_array()?.iter().find_map(|c| find(c, part))
        }
        let tree = self.send("Page.getFrameTree", json!({}), Some(&page.session))?;
        let Some(frame) = find(&tree["frameTree"], url_part) else { return self.eval_in_isolated_frame(url_part, expr) };
        let world = self.send("Page.createIsolatedWorld", json!({ "frameId": frame }), Some(&page.session))?;
        let r = self.send(
            "Runtime.evaluate",
            json!({ "expression": expr, "contextId": world["executionContextId"], "awaitPromise": true, "returnByValue": true }),
            Some(&page.session),
        )?;
        if let Some(e) = r.get("exceptionDetails") {
            bail!("the frame threw: {}", e["exception"]["description"].as_str().unwrap_or(&e.to_string()));
        }
        Ok(r["result"]["value"].clone())
    }

    /// `eval_in_frame` in a frame of another site than the page's: the
    /// first frame target of this lease's contexts whose URL contains
    /// `url_part`, attached for the evaluation. Called alone, it reaches
    /// the frame while its page is between documents (a navigation under
    /// way, which holds the page's own session back until it commits).
    pub fn eval_in_isolated_frame(&mut self, url_part: &str, expr: &str) -> Result<Value> {
        let frame = self.frame(url_part)?;
        let r = self.eval(&frame, expr).map_err(|e| anyhow::anyhow!("the frame threw: {e:#}"));
        self.detach(frame)?;
        r
    }

    /// The first frame target of this lease's contexts whose URL contains
    /// `url_part` (a page of another site than its parent's: a process of
    /// its own), attached until `detach`: driven as a page, its own
    /// requests (`block`) and its loads its own.
    pub fn frame(&mut self, url_part: &str) -> Result<Page> {
        let v = self.send("Target.getTargets", json!({}), None)?;
        let context = self.context.clone();
        let ours = |id: &Value| context.as_deref().is_none_or(|c| id == c || self.others.iter().any(|o| id == o.as_str()));
        let target = v["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["type"] == "iframe" && ours(&t["browserContextId"]) && t["url"].as_str().is_some_and(|u| u.contains(url_part)))
            .and_then(|t| t["targetId"].as_str().map(str::to_string))
            .with_context(|| format!("no frame at {url_part}"))?;
        self.attach(&target)
    }

    /// The page's (or frame's) requests to URLs matching `patterns` (`*`
    /// any run of characters) refused from now on, as a resource that
    /// never comes; none (`&[]`) lets them through again.
    pub fn block(&mut self, page: &Page, patterns: &[&str]) -> Result<()> {
        self.send("Network.enable", json!({}), Some(&page.session))?;
        self.send("Network.setBlockedURLs", json!({ "urls": patterns }), Some(&page.session))?;
        Ok(())
    }

    /// The page's requests to URLs matching `patterns` (`*` any run of
    /// characters, `?` any one) held from now on, unsent, as a server that
    /// has not answered yet holds them, until `release`.
    pub fn hold(&mut self, page: &Page, patterns: &[&str]) -> Result<()> {
        let patterns: Vec<Value> = patterns.iter().map(|p| json!({ "urlPattern": p, "requestStage": "Request" })).collect();
        self.held.insert(page.session.clone(), Vec::new());
        self.send("Fetch.enable", json!({ "patterns": patterns }), Some(&page.session))?;
        Ok(())
    }

    /// The URLs of the requests `hold` holds now.
    pub fn held(&mut self, page: &Page) -> Result<Vec<String>> {
        // what Chrome said before this answer is read
        self.send("Runtime.evaluate", json!({ "expression": "0" }), Some(&page.session))?;
        Ok(self.held.get(&page.session).into_iter().flatten().map(|(_, url)| url.clone()).collect())
    }

    /// Every held request sent on, or (`fail`) failed as a dropped
    /// connection fails one; none held from now on.
    pub fn release(&mut self, page: &Page, fail: bool) -> Result<()> {
        self.held(page)?;
        for (id, _) in self.held.remove(&page.session).unwrap_or_default() {
            match fail {
                true => self.send("Fetch.failRequest", json!({ "requestId": id, "errorReason": "ConnectionClosed" }), Some(&page.session))?,
                false => self.send("Fetch.continueRequest", json!({ "requestId": id }), Some(&page.session))?,
            };
        }
        self.send("Fetch.disable", json!({}), Some(&page.session))?;
        Ok(())
    }

    pub fn detach(&mut self, frame: Page) -> Result<()> {
        self.send("Target.detachFromTarget", json!({ "sessionId": frame.session }), None)?;
        Ok(())
    }

    /// A click at a point, as a hand makes it: a user gesture, so a popup
    /// it opens is allowed (a script's `click()` is not one).
    pub fn click_at(&mut self, page: &Page, (x, y): (f64, f64)) -> Result<()> {
        let mouse = |kind: &str, buttons: u8| json!({ "type": kind, "x": x, "y": y, "button": "left", "buttons": buttons, "clickCount": 1 });
        self.send("Input.dispatchMouseEvent", mouse("mouseMoved", 0), Some(&page.session))?;
        self.send("Input.dispatchMouseEvent", mouse("mousePressed", 1), Some(&page.session))?;
        self.send("Input.dispatchMouseEvent", mouse("mouseReleased", 0), Some(&page.session))?;
        Ok(())
    }

    /// `click_at` the middle of the first element `selector` matches.
    pub fn click(&mut self, page: &Page, selector: &str) -> Result<()> {
        let at = self.middle_of(page, selector)?;
        self.click_at(page, at)
    }

    /// The mouse moved to the middle of the first element `selector`
    /// matches, and no click: the page hears `pointermove`, as a hand's.
    pub fn hover(&mut self, page: &Page, selector: &str) -> Result<()> {
        let (x, y) = self.middle_of(page, selector)?;
        let moved = json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 });
        self.send("Input.dispatchMouseEvent", moved, Some(&page.session))?;
        Ok(())
    }

    /// The middle of the first element `selector` matches, in the
    /// viewport's coordinates, scrolled into view first.
    fn middle_of(&mut self, page: &Page, selector: &str) -> Result<(f64, f64)> {
        let at = self.eval(
            page,
            &format!("(() => {{ const e = document.querySelector({selector:?}); if (!e) return null; e.scrollIntoView({{ block: 'nearest' }}); const r = e.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()"),
        )?;
        let (Some(x), Some(y)) = (at[0].as_f64(), at[1].as_f64()) else { bail!("nothing on the page matches {selector}") };
        Ok((x, y))
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over https a host cookie is set as the cell sets it there (`__Host-`,
    /// Secure, the host's root); over http, and any other cookie, as named.
    #[test]
    fn a_cookie_is_set_as_its_host_names_it() {
        let c = cookie("https://e2e.finite.place/", "fragment_session", "s");
        assert_eq!((c["name"].as_str(), c["secure"].as_bool(), c["path"].as_str()), (Some("__Host-fragment_session"), Some(true), Some("/")));
        let c = cookie("http://127.0.0.1:8790/", "fragment_session", "s");
        assert_eq!((c["name"].as_str(), c.get("secure")), (Some("fragment_session"), None));
        let c = cookie("https://e2e.finite.place/", "theme", "dark");
        assert_eq!((c["name"].as_str(), c.get("secure")), (Some("theme"), None));
    }

    /// A message serde_json reads is read as it is; one holding a body's
    /// bytes as lone surrogates is read with U+FFFD in their place, every
    /// other escape (an escaped backslash before `ud800` among them) kept;
    /// one that is no JSON is still refused.
    #[test]
    fn a_message_with_a_bodys_bytes_is_read() {
        let plain = r#"{"id":3,"result":{"s":"a\"b\\c\u00e9"}}"#;
        assert_eq!(message(plain).unwrap(), serde_json::from_str::<Value>(plain).unwrap());
        let paused = r#"{"method":"Fetch.requestPaused","params":{"requestId":"interception-job-1.0","request":{"postData":"\u0089PNG\ud800\udc00x\udfff","url":"http://x/icon"}}}"#;
        assert!(serde_json::from_str::<Value>(paused).is_err(), "serde_json refuses a lone surrogate");
        let v = message(paused).unwrap();
        assert_eq!(v["params"]["requestId"], "interception-job-1.0");
        assert_eq!(v["params"]["request"]["postData"], "\u{89}PNG\u{fffd}\u{fffd}x\u{fffd}");
        let escaped = r#"{"a":"\\ud800","b":"\udc00"}"#;
        assert_eq!(message(escaped).unwrap(), json!({ "a": "\\ud800", "b": "\u{fffd}" }));
        assert!(message(r#"{"id":"#).is_err());
    }
}
