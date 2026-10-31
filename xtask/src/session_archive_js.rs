//! The shell's Session Archive (cell/shell/session-archive.js) on the pinned
//! Node, its Continue against a scripted platform: the history record it
//! posts is bounded by its body's bytes as the cell measures a record's
//! (`limits::RECORD_BODY_MAX_BYTES`), whatever its text, before anything is
//! made; and a Continue cut short at any step, or whose answer was lost, is
//! finished by the next in the same chat, never another. The e2e's
//! sessions-ui section runs the same file in Chrome on a real platform.

use std::io::Write;
use std::process::Stdio;

use fragment_devstack as devstack;
use serde_json::Value;

/// The platform as Continue meets it: the person's fragments, files,
/// records and members, each step able to fail once before it acts
/// (`fail`) or after it acted with its answer lost (`lose`). A post checks
/// its body's size as the cell does, and ids as the cell does: the same id
/// and body answer the record again, another body is 409.
const WORLD: &str = r##"
globalThis.document = { addEventListener() {}, hidden: false };
globalThis.addEventListener = () => {};
const RECORD_MAX = 64 * 1024;
const bytes = (v) => new TextEncoder().encode(JSON.stringify(v)).length;
const results = [];
const check = (name, ok, detail) => results.push({ name, ok: !!ok, detail: detail === undefined ? null : detail });
const ARCHIVE = "archive--aaaa";
const err = (status, message) => Object.assign(new Error(message), { status });
const element = () => ({ hidden: false, setAttribute() {}, append() {}, dataset: {}, onclick: null });

function world(conversation) {
  const w = { fragments: new Map([[ARCHIVE, { role: "owner", kind: "sessions" }]]), files: new Map(), records: new Map(), members: new Map(), creates: 0, posts: 0, fail: {}, lose: {} };
  w.files.set(`${ARCHIVE}:index/chat.json`, JSON.stringify({ sessions: [{ key: "hermes-c1", title: conversation.title }] }));
  w.files.set(`${ARCHIVE}:sessions/hermes-c1.json`, JSON.stringify(conversation));
  const step = (name, act) => {
    if (w.fail[name]) {
      const status = w.fail[name];
      delete w.fail[name];
      throw err(status, `${name} failed`);
    }
    const answer = act();
    if (w.lose[name]) {
      delete w.lose[name];
      throw new TypeError(`${name}: the answer was lost`);
    }
    return answer;
  };
  w.api = async (method, path, body) => {
    let m;
    if (method === "GET" && (m = path.match(/^\/api\/f\/([^/]+)\/file\?path=(.*)$/))) {
      const f = w.files.get(`${decodeURIComponent(m[1])}:${decodeURIComponent(m[2])}`);
      if (f === undefined) throw err(404, "no such file");
      return JSON.parse(f);
    }
    if (method === "GET" && (m = path.match(/^\/api\/f\/([^/]+)\/status$/))) {
      const f = w.fragments.get(decodeURIComponent(m[1]));
      if (!f) throw err(404, "no such fragment");
      return { name: decodeURIComponent(m[1]), role: f.role };
    }
    if (method === "POST" && path === "/api/fragments")
      return step("create", () => {
        if (!/^[a-z0-9-]+--[a-km-np-z2-9]{4}$/.test(body.name ?? "")) throw err(400, "no name in full");
        if (w.fragments.has(body.name)) throw err(409, "a name taken");
        w.creates++;
        w.fragments.set(body.name, { role: "owner", kind: "chat", title: body.title });
        return { name: body.name };
      });
    if (method === "POST" && (m = path.match(/^\/api\/f\/([^/]+)\/channels\/chat$/)))
      return step("history", () => {
        if (bytes(body.body) > RECORD_MAX) throw err(413, "a record's body is past 64 KiB");
        const k = `${decodeURIComponent(m[1])}:${body.id}`;
        const had = w.records.get(k);
        if (had !== undefined && JSON.stringify(had) !== JSON.stringify(body.body)) throw err(409, "the same id with another body");
        if (had === undefined) {
          w.posts++;
          w.records.set(k, body.body);
        }
        return { replayed: had !== undefined };
      });
    if (method === "PUT" && (m = path.match(/^\/api\/f\/([^/]+)\/members\/(.+)$/)))
      return step("member", () => (w.members.set(`${decodeURIComponent(m[1])}:${decodeURIComponent(m[2])}`, body.role), {}));
    if (method === "POST" && (m = path.match(/^\/api\/f\/([^/]+)\/files$/)))
      return step("marker", () => {
        for (const f of body.files) w.files.set(`${decodeURIComponent(m[1])}:${f.path}`, f.text);
        return {};
      });
    throw err(404, `no route ${method} ${path}`);
  };
  return w;
}

// a tab of the shell: its own module, the browser's store shared
function tab(w, store) {
  const t = { notices: [], opened: [] };
  t.archive = createSessionArchive({
    api: w.api,
    ui: { $: () => element(), el: () => element(), patch() {}, renderHeading() {}, notice: (title, words) => t.notices.push([title, words]) },
    data: {
      allFragments: () => [...w.fragments].map(([name, f]) => ({ name, ...f })), own: (f) => f.role === "owner", seg: encodeURIComponent,
      primaryAgent: () => ({ identity: "npub1agent" }), agents: () => new Map([["npub1agent", { fragment: "juniper--k3x9" }]]), chatLabel: () => "juniper-chat",
    },
    selection: { current: () => null, page: () => null, getFrame: () => null },
    navigation: { openChat: (name) => t.opened.push(name), leaveSidebar() {} },
    load: async () => {},
    store,
  });
  return t;
}

function storeOf() {
  const kept = new Map();
  return { get: (k, fallback) => (kept.has(k) ? JSON.parse(kept.get(k)) : fallback), set: (k, v) => kept.set(k, JSON.stringify(v)) };
}

const said = (n, text) => Array.from({ length: n }, (_, i) => ({ role: i % 2 ? "assistant" : "user", at: `2026-10-01T09:00:${String(i % 60).padStart(2, "0")}.000Z`, text: `${i} ${text}` }));
const conversation = (messages, summary) => ({ id: "c1", title: "Beans", started_at: "2026-10-01T09:00:00.000Z", ended_at: "2026-10-01T10:00:00.000Z", messages: [...(summary ? [{ role: "user", summary: true, text: summary }] : []), ...messages] });
"##;

const CASES: &str = r##"
// ---- the history is bounded by its body's bytes, before anything is made
const cjk = "漢".repeat(4000);
for (const [name, c] of [
  ["six messages of 4,000 CJK characters", conversation(said(6, cjk))],
  ["a summary of 8,000 CJK characters and 40 messages of 4,000", conversation(said(40, cjk), "要".repeat(8000))],
  ["text that JSON escapes (quotes, backslashes, control characters)", conversation(said(40, '"\\\u0001'.repeat(1300)), '"\\\u0002'.repeat(2600))],
]) {
  const h = historyOf(c, "hermes-c1", ARCHIVE);
  const last = c.messages[c.messages.length - 1];
  check(`${name}: its history fits a record's body, the newest kept`, bytes(h) <= HISTORY_BODY_MAX_BYTES && HISTORY_BODY_MAX_BYTES < RECORD_MAX && h.messages.length > 0 && h.messages[h.messages.length - 1].text.startsWith(last.text.slice(0, 20)) && h.earlier === c.messages.filter((m) => !m.summary).length - h.messages.length, { bytes: bytes(h), kept: h.messages.length, earlier: h.earlier, summary: !!h.summary });
  const w = world(c);
  const t = tab(w, storeOf());
  await t.archive.load();
  await t.archive.continueSession("hermes-c1");
  check(`${name}: Continue makes one chat, its history posted, its agent in it`, w.creates === 1 && w.posts === 1 && w.members.size === 1 && t.opened.length === 1 && !t.notices.length, { creates: w.creates, posts: w.posts, notices: t.notices });
}
{
  const c = conversation(said(3, "short"), "earlier, summarized");
  const h = historyOf(c, "hermes-c1", ARCHIVE);
  check("a short conversation is kept whole, with its summary", h.messages.length === 3 && h.earlier === 0 && h.summary === "earlier, summarized", h);
}

// ---- a Continue cut short, or whose answer was lost, is finished in the same chat
for (const [name, set] of [
  ["the marker's write fails after the chat, history and agent", (w) => (w.fail.marker = 503)],
  ["the agent's add fails after the chat and its history", (w) => (w.fail.member = 503)],
  ["the history's post fails after the chat", (w) => (w.fail.history = 503)],
  ["the create's answer is lost (the chat was made)", (w) => (w.lose.create = true)],
  ["the history's answer is lost (it was posted)", (w) => (w.lose.history = true)],
  ["the marker's answer is lost (it was written)", (w) => (w.lose.marker = true)],
]) {
  const w = world(conversation(said(4, "hello")));
  set(w);
  const store = storeOf();
  const t = tab(w, store);
  await t.archive.load();
  await t.archive.continueSession("hermes-c1");
  const first = t.notices.at(-1)?.[0] ?? null;
  await t.archive.continueSession("hermes-c1");
  const chats = [...w.fragments].filter(([, f]) => f.kind === "chat").map(([n]) => n);
  const marker = w.files.get(`${ARCHIVE}:continued/hermes-c1.json`);
  check(
    `${name}: it says the chat is not ready, and Continue again finishes that chat, making none other`,
    first === "The chat is not ready yet" && chats.length === 1 && w.creates === 1 && w.posts === 1 && w.members.size === 1 && JSON.parse(marker).chat === chats[0] && t.opened.at(-1) === chats[0] && store.get("shell.archive.continuing.v1", {})[`${ARCHIVE} hermes-c1`] === undefined,
    { first, chats, creates: w.creates, posts: w.posts, members: w.members.size, opened: t.opened, notices: t.notices },
  );
}
{
  // a refusal before anything is made says so
  const w = world(conversation(said(2, "hello")));
  w.fail.create = 402;
  const t = tab(w, storeOf());
  await t.archive.load();
  await t.archive.continueSession("hermes-c1");
  check("a create refused says the chat was not made", t.notices.at(-1)?.[0] === "The chat was not made" && w.creates === 0, t.notices);
}
{
  // a stale tab: another tab continued it; this one, asked again, opens that chat
  const w = world(conversation(said(2, "hello")));
  const [a, b] = [tab(w, storeOf()), tab(w, storeOf())];
  await a.archive.load();
  await b.archive.load();
  await a.archive.continueSession("hermes-c1");
  await b.archive.continueSession("hermes-c1");
  check("a stale tab's Continue opens the chat another made, making none", w.creates === 1 && b.opened[0] === a.opened[0] && !b.notices.length, { creates: w.creates, a: a.opened, b: b.opened });
  // its chat deleted since, a Continue makes another
  w.fragments.delete(a.opened[0]);
  await b.archive.continueSession("hermes-c1");
  check("a continued conversation whose chat is gone makes a new one", w.creates === 2 && b.opened.at(-1) !== a.opened[0] && JSON.parse(w.files.get(`${ARCHIVE}:continued/hermes-c1.json`)).chat === b.opened.at(-1), { creates: w.creates, b: b.opened });
}
{
  // a name kept, then taken by someone else's fragment: a new one next time
  const w = world(conversation(said(2, "hello")));
  const store = storeOf();
  store.set("shell.archive.continuing.v1", { [`${ARCHIVE} hermes-c1`]: "juniper-chat--zzzz" });
  w.fragments.set("juniper-chat--zzzz", { role: "viewer", kind: "chat" });
  const t = tab(w, store);
  await t.archive.load();
  await t.archive.continueSession("hermes-c1");
  await t.archive.continueSession("hermes-c1");
  check("a kept name another's fragment holds is let go, and the next Continue makes the chat", t.notices[0]?.[0] === "The chat was not made" && w.creates === 1 && t.opened.length === 1 && t.opened[0] !== "juniper-chat--zzzz", { notices: t.notices, opened: t.opened });
}
console.log(JSON.stringify(results));
"##;

/// Goal: Continue's history fits a record whatever its text, and a
/// Continue cut short at any step, or whose answer was lost, is finished
/// in the same chat, never another; a stale tab opens the chat another
/// made. Method: the module imported by Node into a scripted platform
/// (`WORLD`), every case checked there, the results read back as JSON.
#[test]
fn continue_fits_its_history_and_finishes_what_it_began() {
    let root = devstack::repo_root();
    let node = devstack::node::locate(&root.join(devstack::TOOLS_DIR)).expect("the pinned Node");
    let url = format!("file://{}", root.join("cell/shell/session-archive.js").display());
    let script = format!("const {{ createSessionArchive, historyOf, HISTORY_BODY_MAX_BYTES }} = await import({});\n{WORLD}\n{CASES}", serde_json::to_string(&url).unwrap());
    let mut child = node
        .command(&root.join(devstack::CACHE_DIR))
        .expect("a Node command")
        .args(["--input-type=module", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node runs");
    child.stdin.take().expect("stdin is piped").write_all(script.as_bytes()).expect("the script is written");
    let out = child.wait_with_output().expect("node finishes");
    assert!(out.status.success(), "node failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let results: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert!(results.len() >= 16, "the cases ran: {}", results.len());
    let failed: Vec<String> = results.iter().filter(|r| r["ok"] != true).map(|r| format!("{}: {}", r["name"], r["detail"])).collect();
    assert!(failed.is_empty(), "{} of {} failed:\n{}", failed.len(), results.len(), failed.join("\n"));
}
