// templates/chat/site/frame-messages.js (cell/shell's, linked) is the one
// definition of the messages between the shell and the pages it frames
// (FIN-341): every kind any page or the shell sends or takes is one of
// its, the shell and the chat's page name them through `KIND` alone, and
// none of its is unused. Read from the sources themselves.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, realpathSync } from "node:fs";
import { MESSAGES, KIND } from "../shell/frame-messages.js";

const root = new URL("../../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const real = (path) => realpathSync(new URL(path, root));
const kinds = new Set(Object.values(KIND));

// the files that import the module: the shell's that send or take a
// message, and the chat's page (telegram's is a link to it)
const IMPORTERS = ["cell/shell/shell.js", "cell/shell/session-refresh.js", "cell/shell/connections.js", "cell/shell/launcher.js", "cell/shell/session-archive.js", "templates/chat/site/chat.js"];
// the pages that cannot import it, each sending or taking its kinds as
// literals: other templates, the cell's own pages, the alerts worker (a
// classic service worker) and the computer's ports (images/, a workspace
// of its own)
const LITERALS = [
  "templates/agent/site/agent.js", "templates/skills/site/skills.js", "templates/sessions/site/sessions.js",
  "cell/files.mjs", "cell/src/auth.rs", "cell/shell/alerts-sw.js",
  "images/bridge/src/simplex/page.html", "images/bridge/src/answer.html", "images/hermes/screen/index.html", "images/stub/screen/index.html",
];

// a kind written out: compared (`fragment === "busy"`), sent (`fragment:
// "busy"`, a Rust string's `\"busy\"` too), or listed (`["a", "b"].includes(d.fragment)`)
function literals(text) {
  const found = [...text.matchAll(/fragment\s*(?:===|!==|:)\s*\\?["']([a-z?-]+)\\?["']/g)].map((m) => m[1]);
  for (const m of text.matchAll(/\[((?:\s*["'][a-z?-]+["'],?)+)\s*\]\.includes\(\s*[\w.?]*fragment\s*\)/g)) found.push(...[...m[1].matchAll(/["']([a-z?-]+)["']/g)].map((x) => x[1]));
  return found;
}

test("every message has a kind of its own, and its sender, fields and meaning said", () => {
  assert.equal(kinds.size, Object.keys(MESSAGES).length);
  for (const [name, m] of Object.entries(MESSAGES)) {
    assert.match(m.kind, /^[a-z-]+\??$/, name);
    for (const field of ["from", "fields", "says"]) assert.ok(typeof m[field] === "string" && m[field].length > 0, `${name}.${field}`);
  }
});

test("the shell and the chat's page name every kind through KIND, and only kinds it has", () => {
  for (const path of IMPORTERS) {
    const text = read(path);
    assert.match(text, /^import \{ KIND \} from "\.\/frame-messages\.js";$/m, `${path} imports KIND`);
    assert.deepEqual(literals(text), [], `${path} writes no kind out`);
    for (const [, name] of text.matchAll(/\bKIND\.(\w+)/g)) assert.ok(name in KIND, `${path}: KIND.${name} is none of its`);
  }
  // telegram's chat page is the chat's own, the module beside it
  assert.equal(real("templates/telegram/site/chat.js"), real("templates/chat/site/chat.js"));
  assert.equal(real("templates/telegram/site/frame-messages.js"), real("templates/chat/site/frame-messages.js"));
  assert.equal(real("cell/shell/frame-messages.js"), real("templates/chat/site/frame-messages.js"));
});

test("the pages that cannot import it send and take its kinds alone", () => {
  for (const path of LITERALS) {
    const found = literals(read(path));
    assert.ok(found.length > 0, `${path} has its messages (a page that has none any more leaves the list)`);
    for (const kind of found) assert.ok(kinds.has(kind), `${path}: "${kind}" is no kind of the module's`);
  }
});

test("no other page of the shell's, the cell's or a template's writes a kind out of the module's sight", () => {
  const known = new Set([...IMPORTERS, ...LITERALS].map(real));
  const candidates = [
    ...readdirSync(new URL("cell/shell/", root)).filter((f) => f.endsWith(".js")).map((f) => `cell/shell/${f}`),
    ...readdirSync(new URL("cell/", root)).filter((f) => f.endsWith(".mjs")).map((f) => `cell/${f}`),
    ...readdirSync(new URL("templates/", root)).flatMap((t) => {
      try {
        return readdirSync(new URL(`templates/${t}/site/`, root)).filter((f) => f.endsWith(".js")).map((f) => `templates/${t}/site/${f}`);
      } catch {
        return [];
      }
    }),
  ];
  for (const path of candidates) {
    if (known.has(real(path)) || real(path) === real("cell/shell/frame-messages.js")) continue;
    assert.deepEqual(literals(read(path)), [], `${path} sends or takes a message: import KIND, or add it to LITERALS`);
  }
});

test("every kind of the module's is sent or taken somewhere", () => {
  const used = new Set();
  for (const path of IMPORTERS) for (const [, name] of read(path).matchAll(/\bKIND\.(\w+)/g)) used.add(KIND[name]);
  for (const path of LITERALS) for (const kind of literals(read(path))) used.add(kind);
  assert.deepEqual([...kinds].filter((k) => !used.has(k)), []);
});
