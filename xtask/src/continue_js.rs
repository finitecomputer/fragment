//! The shell's Continue (cell/shell/continue.js) on the pinned Node, against
//! a scripted platform: the history record it posts is bounded by its
//! body's bytes as the cell measures a record's (`limits::RECORD_BODY_MAX_BYTES`),
//! whatever its text, before anything is made, and holds a summary then
//! what was said after it; one chat a (source, agent), named from both, so
//! a Continue cut short at any step, or whose answer was lost, or asked
//! again from another tab, is finished in that chat, never another; the
//! agent offered first is the import's own by name, an archived chat's lead
//! one, else the person's main one; and an archived chat's history is made
//! from its records. The e2e's sessions-ui section runs the same file in
//! Chrome on a real platform.

use std::io::Write;
use std::process::Stdio;

use fragment_devstack as devstack;
use serde_json::Value;

/// The platform as Continue meets it: the person's fragments (each with its
/// agents, as their list has them), files, records and members, each step
/// able to fail once before it acts (`fail`) or after it acted with its
/// answer lost (`lose`). A post checks its body's size as the cell does, and
/// ids as the cell does: the same id and body answer the record again,
/// another body is 409.
const WORLD: &str = r##"
const RECORD_MAX = 64 * 1024;
const bytes = (v) => new TextEncoder().encode(JSON.stringify(v)).length;
const results = [];
const check = (name, ok, detail) => results.push({ name, ok: !!ok, detail: detail === undefined ? null : detail });
const ARCHIVE = "sessions--aaaa";
const KEY = "hermes-c1";
const err = (status, message) => Object.assign(new Error(message), { status });
// the person's agents: identity → their computer's agent, and titles
const AGENTS = [["npub1datastar", "datastar-pro--k3x9", "Datastar Pro"], ["npub1juniper", "juniper--m4n5", "Juniper"]];

function world(conversation, extra = []) {
  const w = { fragments: new Map([[ARCHIVE, { role: "owner", kind: "sessions", agents: [] }]]), files: new Map(), records: new Map(), creates: 0, posts: 0, fail: {}, lose: {} };
  for (const [identity, fragment, title] of AGENTS) w.fragments.set(fragment, { role: "owner", kind: "agent", title, agents: [identity] });
  for (const [name, f] of extra) w.fragments.set(name, f);
  w.files.set(`${ARCHIVE}:sessions/${KEY}.json`, JSON.stringify(conversation));
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
    if (method === "GET" && (m = path.match(/^\/api\/f\/([^/]+)\/channels\/chat\?(after|before)=(\d+)&limit=(\d+)$/))) {
      const all = w.records.get(decodeURIComponent(m[1])) ?? [];
      const n = Number(m[4]);
      return { records: m[2] === "after" ? all.slice(0, n) : all.slice(-n) };
    }
    if (method === "POST" && path === "/api/fragments")
      return step("create", () => {
        if (!/^[a-z0-9-]+--[a-km-np-z2-9]{4}$/.test(body.name ?? "")) throw err(400, "no name in full");
        if (w.fragments.has(body.name)) throw err(409, "a name taken");
        w.creates++;
        w.fragments.set(body.name, { role: "owner", kind: "chat", title: body.title, agents: [] });
        return { name: body.name };
      });
    if (method === "POST" && (m = path.match(/^\/api\/f\/([^/]+)\/channels\/chat$/)))
      return step("history", () => {
        if (bytes(body.body) > RECORD_MAX) throw err(413, "a record's body is past 64 KiB");
        const name = decodeURIComponent(m[1]);
        const all = w.records.get(name) ?? [];
        const had = all.find((r) => r.id === body.id);
        if (had && JSON.stringify(had.body) !== JSON.stringify(body.body)) throw err(409, "the same id with another body");
        if (!had) {
          w.posts++;
          all.push({ id: body.id, seq: all.length + 1, at: Date.now(), principal: "npub1paul", kind: "message", body: body.body });
          w.records.set(name, all);
        }
        return { replayed: !!had };
      });
    if (method === "PUT" && (m = path.match(/^\/api\/f\/([^/]+)\/members\/(.+)$/)))
      return step("member", () => {
        const f = w.fragments.get(decodeURIComponent(m[1]));
        const who = decodeURIComponent(m[2]);
        if (!f.agents.includes(who)) f.agents.push(who);
        return {};
      });
    throw err(404, `no route ${method} ${path}`);
  };
  return w;
}

// a tab of the shell: its own module, a store of its own (another browser,
// unless shared)
function tab(w, store = storeOf(), main = "npub1datastar") {
  const t = { notices: [], opened: [] };
  t.c = createContinue({
    api: w.api,
    ui: { el: () => ({}), notice: (title, words) => t.notices.push([title, words]) },
    data: {
      allFragments: () => [...w.fragments].map(([name, f]) => ({ name, ...f })),
      agents: () => new Map(AGENTS.map(([identity, fragment]) => [identity, { identity, fragment }])),
      primaryAgent: () => ({ identity: main }),
      titleOf: (name) => w.fragments.get(name)?.title ?? name,
      seg: encodeURIComponent,
      chatBase: (agent) => `${agent.replace(/--[a-km-np-z2-9]{4}$/, "")}-chat`,
    },
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
const conversation = (messages, summary, agent = "Juniper") => ({ id: "c1", title: "Beans", agent, started_at: "2026-10-01T09:00:00.000Z", ended_at: "2026-10-01T10:00:00.000Z", messages: [...(summary ? [{ role: "user", summary: true, text: summary }] : []), ...messages] });
const SOURCE = { archive: ARCHIVE, key: KEY };
const chats = (w) => [...w.fragments].filter(([, f]) => f.kind === "chat").map(([n]) => n);
"##;

const CASES: &str = r##"
// ---- the history: bounded by its body's bytes, before anything is made
const cjk = "漢".repeat(4000);
for (const [name, c] of [
  ["six messages of 4,000 CJK characters", conversation(said(6, cjk))],
  ["a summary of 8,000 CJK characters and 40 messages of 4,000", conversation(said(40, cjk), "要".repeat(8000))],
  ["text that JSON escapes (quotes, backslashes, control characters)", conversation(said(40, '"\\\u0001'.repeat(1300)), '"\\\u0002'.repeat(2600))],
]) {
  const h = historyOf(c, KEY, ARCHIVE);
  const last = c.messages[c.messages.length - 1];
  check(`${name}: its history fits a record's body, the newest kept`, bytes(h) <= HISTORY_BODY_MAX_BYTES && HISTORY_BODY_MAX_BYTES < RECORD_MAX && h.messages.length > 0 && h.messages.at(-1).text.startsWith(last.text.slice(0, 20)), { bytes: bytes(h), kept: h.messages.length });
  const w = world(c);
  const t = tab(w);
  const made = await t.c.continueInto(SOURCE, "npub1juniper");
  check(`${name}: Continue makes one chat, its history posted, its agent in it`, made && w.creates === 1 && w.posts === 1 && w.fragments.get(made).agents.includes("npub1juniper") && t.opened[0] === made && !t.notices.length, { made, notices: t.notices });
}
{
  // a summary covers what was said before it: the card holds it, then what
  // was said after it, never what it summarized
  const c = { id: "c1", title: "Beans", messages: [
    { role: "user", at: "2026-10-01T09:00:00.000Z", text: "before one" },
    { role: "assistant", at: "2026-10-01T09:00:01.000Z", text: "before two" },
    { role: "user", at: "2026-10-02T09:00:00.000Z", text: "what we said before", summary: true },
    { role: "user", at: "2026-10-02T09:00:01.000Z", text: "after one" },
    { role: "assistant", at: "2026-10-02T09:00:02.000Z", text: "after two" },
  ] };
  const h = historyOf(c, KEY, ARCHIVE);
  check("a summary is followed by what was said after it alone, the rest counted earlier", h.summary === "what we said before" && JSON.stringify(h.messages.map((m) => m.text)) === JSON.stringify(["after one", "after two"]) && h.earlier === 2, h);
}

// ---- the agent offered first
{
  const w = world(conversation(said(2, "hi")), [["fred-chat--k3x9", { role: "owner", kind: "chat", title: "Fred's", agents: ["npub1juniper"], archived: true }], ["other-chat--k3x9", { role: "owner", kind: "chat", agents: ["npub1someone"] }]]);
  const t = tab(w);
  check("an import is offered to the person's agent of its own agent's name", (await t.c.defaultAgent(SOURCE)) === "npub1juniper");
  check("an import whose agent has a name none of theirs has goes to their main one", (await tab(world(conversation(said(2, "hi"), null, "Hal"))).c.defaultAgent(SOURCE)) === "npub1datastar");
  check("an import says its agent's name itself, as its index has it, read from nothing", (await t.c.defaultAgent({ ...SOURCE, agent: "Datastar Pro" })) === "npub1datastar");
  check("an archived chat is offered to its lead agent, theirs", (await t.c.defaultAgent({ chat: "fred-chat--k3x9" })) === "npub1juniper");
  check("an archived chat whose lead agent is none of theirs goes to their main one", (await t.c.defaultAgent({ chat: "other-chat--k3x9" })) === "npub1datastar");
  check("the agents offered are the person's own, titled", JSON.stringify(t.c.agents().map((a) => a.title)) === JSON.stringify(["Datastar Pro", "Juniper"]));
}

// ---- one chat a (source, agent)
{
  const w = world(conversation(said(4, "hello")));
  const [a, b] = [tab(w), tab(w)];
  const first = await a.c.continueInto(SOURCE, "npub1juniper");
  const again = await b.c.continueInto(SOURCE, "npub1juniper");
  check("Continue asked again from another tab (another store) opens the same chat, making none", first && again === first && w.creates === 1 && w.posts === 1, { first, again, creates: w.creates });
  check("its name says its agent and is the person's to find", /^juniper-chat-[a-km-np-z2-9]{4}--[a-km-np-z2-9]{4}$/.test(first) && (await a.c.continued(SOURCE, "npub1juniper")) === first, first);
  check("another agent's has none yet", (await a.c.continued(SOURCE, "npub1datastar")) === null);
  const other = await a.c.continueInto(SOURCE, "npub1datastar");
  check("Continue with another agent makes that agent's own chat", other && other !== first && w.creates === 2 && chats(w).length === 2, { other, first });
  w.fragments.delete(first);
  const remade = await b.c.continueInto(SOURCE, "npub1juniper");
  check("a chat deleted since is made again, by the same name", remade === first && w.creates === 3, remade);
}
for (const [name, set] of [
  ["the history's post fails after the chat", (w) => (w.fail.history = 503)],
  ["the agent's add fails after the chat and its history", (w) => (w.fail.member = 503)],
  ["the create's answer is lost (the chat was made)", (w) => (w.lose.create = true)],
  ["the history's answer is lost (it was posted)", (w) => (w.lose.history = true)],
  ["the agent's add's answer is lost", (w) => (w.lose.member = true)],
]) {
  const w = world(conversation(said(4, "hello")));
  set(w);
  const t = tab(w);
  const cut = await t.c.continueInto(SOURCE, "npub1juniper");
  const first = t.notices.at(-1)?.[0] ?? null;
  const done = await t.c.continueInto(SOURCE, "npub1juniper");
  check(
    `${name}: it says the chat is not ready, and Continue again finishes that chat, making none other`,
    cut === null && first === "The chat is not ready yet" && done && chats(w).length === 1 && w.creates === 1 && w.posts === 1 && w.fragments.get(done).agents.includes("npub1juniper") && t.opened.at(-1) === done,
    { first, done, chats: chats(w), creates: w.creates, posts: w.posts, notices: t.notices },
  );
}
{
  const w = world(conversation(said(2, "hello")));
  w.fail.create = 402;
  const t = tab(w);
  await t.c.continueInto(SOURCE, "npub1juniper");
  check("a create refused says the chat was not made", t.notices.at(-1)?.[0] === "The chat was not made" && w.creates === 0, t.notices);
}
{
  // a name the hash gave that some other fragment holds: one at random, kept
  // in the browser until its Continue is done
  const w = world(conversation(said(2, "hello")));
  const probe = tab(world(conversation(said(2, "hello"))));
  const taken = await probe.c.continueInto(SOURCE, "npub1juniper");
  w.fragments.set(taken, { role: "viewer", kind: "chat", agents: [] });
  const store = storeOf();
  const t = tab(w, store);
  w.fail.member = 503;
  await t.c.continueInto(SOURCE, "npub1juniper");
  const made = await t.c.continueInto(SOURCE, "npub1juniper");
  check("a name another's fragment holds gives way to one at random, the same on a try again, then let go", made && made !== taken && w.creates === 1 && (await t.c.continued(SOURCE, "npub1juniper")) === null && JSON.stringify(store.get("shell.continue.pending.v1", {})) === "{}", { made, taken, creates: w.creates });
}
{
  // a Continue from before chats were named by their source: its marker's chat
  const w = world(conversation(said(2, "hello")), [["datastar-pro-chat-13--ke3c", { role: "owner", kind: "chat", agents: ["npub1datastar"] }]]);
  w.files.set(`${ARCHIVE}:continued/${KEY}.json`, JSON.stringify({ chat: "datastar-pro-chat-13--ke3c" }));
  const t = tab(w);
  const opened = await t.c.continueInto(SOURCE, "npub1datastar");
  const fresh = await t.c.continueInto(SOURCE, "npub1juniper");
  check("an archive's marker from before opens its chat for its agent, and another agent's is made", opened === "datastar-pro-chat-13--ke3c" && fresh && fresh !== opened && w.creates === 1, { opened, fresh });
}

// ---- who it was with, and an archived chat's history
{
  const w = world(conversation(said(2, "hello"), null, "Juniper"));
  const t = tab(w);
  const own = await t.c.continueInto(SOURCE, "npub1juniper");
  const other = await t.c.continueInto(SOURCE, "npub1datastar");
  const body = (name) => w.records.get(name)[0].body;
  check("an import continued with its own agent says no other; with another, whose it was", !("agent" in body(own)) && body(other).agent === "Juniper", { own: body(own), other: body(other) });
}
{
  const records = [
    { seq: 1, at: Date.parse("2026-10-20T09:00:00Z"), principal: "npub1paul", kind: "message", body: { text: "what's for dinner?" } },
    { seq: 2, at: Date.parse("2026-10-20T09:00:05Z"), principal: "npub1juniper", kind: "message", body: { text: "soup" } },
    { seq: 3, at: Date.parse("2026-10-20T09:00:06Z"), principal: "npub1juniper", kind: "turn.start", body: { kind: "turn.start" } },
    { seq: 4, at: Date.parse("2026-10-20T09:01:00Z"), principal: "npub1paul", kind: "message", body: { text: "which?" } },
  ];
  const w = world(conversation(said(2, "x")), [["juniper-chat-7--k3x9", { role: "owner", kind: "chat", title: "Dinner", agents: ["npub1juniper"], archived: true }]]);
  w.records.set("juniper-chat-7--k3x9", records);
  const t = tab(w);
  const made = await t.c.continueInto({ chat: "juniper-chat-7--k3x9" }, "npub1datastar");
  const h = w.records.get(made)[0].body;
  check(
    "an archived chat continued with another agent: its history from its records, whose words whose, and where it was",
    h.kind === "history" && h.chat === "juniper-chat-7--k3x9" && h.title === "Dinner" && h.agent === "Juniper" && JSON.stringify(h.messages.map((m) => [m.role, m.text])) === JSON.stringify([["user", "what's for dinner?"], ["assistant", "soup"], ["user", "which?"]]) && h.started_at === "2026-10-20T09:00:00.000Z" && !("archive" in h),
    h,
  );
  const again = await tab(w).c.continueInto({ chat: "juniper-chat-7--k3x9" }, "npub1datastar");
  check("and continued again, the same chat", again === made && w.creates === 1, again);
}
console.log(JSON.stringify(results));
"##;

/// Goal: Continue's history fits a record whatever its text and holds a
/// summary then what came after; one chat a (source, agent), finished by
/// the next Continue whatever cut the last short; the agent offered first
/// the right one; an archived chat's history from its records. Method: the
/// module imported by Node into a scripted platform (`WORLD`), every case
/// checked there, the results read back as JSON.
#[test]
fn continue_makes_one_chat_a_source_and_agent() {
    let root = devstack::repo_root();
    let node = devstack::node::locate(&root.join(devstack::TOOLS_DIR)).expect("the pinned Node");
    let url = format!("file://{}", root.join("cell/shell/continue.js").display());
    let script = format!("const {{ createContinue, historyOf, HISTORY_BODY_MAX_BYTES }} = await import({});\n{WORLD}\n{CASES}", serde_json::to_string(&url).unwrap());
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
    assert!(results.len() >= 24, "the cases ran: {}", results.len());
    let failed: Vec<String> = results.iter().filter(|r| r["ok"] != true).map(|r| format!("{}: {}", r["name"], r["detail"])).collect();
    assert!(failed.is_empty(), "{} of {} failed:\n{}", failed.len(), results.len(), failed.join("\n"));
}
