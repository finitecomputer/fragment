// cell/shell/skills-page.js: what the person's agents know, and the Hermes
// optional skills they switch on. What a listing says (the managed set,
// and the `optional/hermes/<id>.on` switches), a search, a slug in words,
// a SKILL.md's description; and the page itself: loading, the On tab and
// the Library, a switch written as one file, put back when the write
// fails, a labelled skill asking first, a switch the catalog no longer
// names, a skills fragment without the catalog, and the shell making one
// skills fragment however often it is asked meanwhile.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { byCategory, createSkillsPage, descriptionOf, FILE_MAX_BYTES, markerPath, matches, onRows, readListing, words } from "../shell/skills-page.js";
import { createSettings } from "../shell/settings.js";

test("a listing's managed skills, and the optional skills it switches on", () => {
  const { managed, on } = readListing([
    { path: "fragment.json" },
    { path: "catalog/hermes.json" },
    { path: "skills/software-development/apps-finite/SKILL.md" },
    { path: "skills/software-development/apps-finite/references/x.md" },
    { path: "skills/music-generation-finite/SKILL.md" },
    { path: "skills/archive/SKILL.md", size: FILE_MAX_BYTES + 1 },
    { path: "optional/hermes/creative/excalidraw.on" },
    { path: "optional/hermes/mlops/training/unsloth.on" },
    { path: "optional/hermes/creative/sketch.off" },
  ]);
  // one the computer refuses for its size is never installed: not listed
  assert.deepEqual(managed.map((m) => [m.name, m.folder]), [["apps-finite", "skills/software-development/apps-finite/"], ["music-generation-finite", "skills/music-generation-finite/"]]);
  assert.deepEqual([...on], ["creative/excalidraw", "mlops/training/unsloth"]);
  assert.equal(markerPath("mlops/training/unsloth"), "optional/hermes/mlops/training/unsloth.on");
});

test("a slug in words, a search over a skill's words, a SKILL.md's description", () => {
  assert.equal(words("autonomous-ai-agents"), "Autonomous AI agents");
  assert.equal(words("mlops"), "MLOps");
  assert.equal(words("3-statement-model"), "3 statement model");
  const s = { id: "creative/excalidraw", name: "excalidraw", description: "Hand-drawn Excalidraw JSON diagrams." };
  assert.ok(matches(s, "diagrams") && matches(s, "Hand DRAWN") && matches(s, "creative"));
  assert.ok(!matches(s, "diagrams video"), "every word");
  assert.equal(descriptionOf('---\nname: x\ndescription: "Calls: \\"and\\" texts."\n---\n'), 'Calls: "and" texts.');
  assert.equal(descriptionOf("---\ndescription: 'It''s here'\n---\n"), "It's here");
  assert.equal(descriptionOf("---\ndescription: Plain words.\n---\n# body"), "Plain words.");
  assert.equal(descriptionOf("# no frontmatter"), "");
  assert.deepEqual(byCategory([{ id: "research/a" }, { id: "creative/b" }, { id: "creative/c" }]).map(([c, l]) => [c, l.length]), [["creative", 2], ["research", 1]]);
});

// The page itself, its DOM faked as far as it draws.
class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, attrs: {}, hidden: false });
  }
  append(...nodes) {
    this.children.push(...nodes);
  }
  replaceChildren(...nodes) {
    this.children = [...nodes];
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
}
const walk = (n) => [n, ...n.children.flatMap(walk)];
const settle = () => new Promise((r) => setTimeout(r, 0));

const CATALOG = {
  runtime: "hermes",
  hermes: "v0.23.2",
  source: "https://github.com/NousResearch/hermes-agent/tree/v0.23.2/optional-skills",
  skills: [
    { id: "creative/excalidraw", name: "excalidraw", description: "Hand-drawn diagrams.", license: "MIT" },
    { id: "mlops/training/unsloth", name: "unsloth", description: "Fast fine-tuning." },
    { id: "productivity/telephony", name: "telephony", description: "Calls and texts.", labels: ["spends", "contacts"] },
  ],
};

function page({ on = [], catalog = CATALOG } = {}) {
  const calls = [];
  const opened = [];
  const h = {
    api: (method, path, body) => new Promise((ok, fail) => calls.push({ method, path, body, ok, fail })),
    el: (tag, cls, text) => new Node(tag, cls, text),
    seg: (s) => s,
    skillsFragment: async () => "skills--z8w6",
    text: async () => "---\nname: apps-finite\ndescription: Publish and share apps.\n---\n",
    openApp: (name) => opened.push(["app", name]),
    openFiles: (name, folder) => opened.push(["files", name, folder]),
  };
  const root = new Node("section");
  const p = createSkillsPage(root, h);
  const find = (cls) => walk(root).filter((n) => n.className.split(" ").includes(cls));
  const answerReads = () => {
    for (const c of calls.filter((c) => c.method === "GET" && !c.done)) {
      c.done = true;
      if (c.path.endsWith("/files")) c.ok({ files: [{ path: "skills/software-development/apps-finite/SKILL.md" }, ...on.map((id) => ({ path: markerPath(id) }))] });
      else if (catalog) c.ok(catalog);
      else c.fail(Object.assign(new Error("not found"), { status: 404 }));
    }
  };
  const item = (id) => find("skills-item").find((n) => n.dataset.skill === id);
  const toggle = (id) => walk(item(id)).find((n) => n.className.includes("skills-switch"));
  const writes = () => calls.filter((c) => c.method === "POST");
  return { p, root, calls, opened, find, answerReads, item, toggle, writes };
}

test("the page: loading, then the On tab (what is on, what is included) and the Library by category", async () => {
  const t = page({ on: ["mlops/training/unsloth"] });
  t.p.open();
  await settle();
  assert.equal(t.find("skills-loading")[0]?.textContent, "Loading skills…");
  assert.deepEqual(t.calls.map((c) => c.path), ["/api/f/skills--z8w6/files", "/api/f/skills--z8w6/file?path=catalog%2Fhermes.json"]);
  t.answerReads();
  await settle();
  await settle();
  const tabs = t.find("skills-tab").map((n) => [n.textContent, n.attrs["aria-selected"]]);
  assert.deepEqual(tabs, [["On · 2", "true"], ["Library · 3", "false"]]);
  assert.deepEqual(t.find("skills-heading").map((n) => n.children[0].textContent), ["You turned on", "Included with Fragment"]);
  assert.ok(t.toggle("mlops/training/unsloth").checked, "what is on, switched on");
  assert.deepEqual(t.find("skills-desc").map((n) => n.textContent), ["Fast fine-tuning.", "Publish and share apps."], "a managed skill's description, read from its SKILL.md");
  // a managed skill opens its folder in the files
  t.find("skills-row-managed")[0].onclick();
  assert.deepEqual(t.opened, [["files", "skills--z8w6", "skills/software-development/apps-finite/"]]);
  // the Library: every skill, by category, its labels as chips
  t.find("skills-tab")[1].onclick();
  assert.deepEqual(t.find("skills-heading").map((n) => n.children[0].textContent), ["Creative", "MLOps", "Productivity"]);
  assert.deepEqual(t.find("skills-chip").map((n) => n.textContent), ["Spends money", "Contacts people"]);
  assert.ok(t.find("skills-note")[0].textContent.includes("v0.23.2"));
});

test("a switch is one file written or removed; a failed write puts it back and says so", async () => {
  const t = page();
  t.p.open();
  await settle();
  t.answerReads();
  await settle();
  t.find("skills-tab")[1].onclick();
  const sw = t.toggle("creative/excalidraw");
  sw.checked = true;
  sw.onchange();
  assert.ok(t.toggle("creative/excalidraw").disabled && t.toggle("creative/excalidraw").checked, "on at once, while it saves");
  assert.deepEqual(t.writes().map((c) => [c.path, c.body.files]), [["/api/f/skills--z8w6/files", [{ path: "optional/hermes/creative/excalidraw.on", text: "on\n" }]]]);
  t.writes()[0].ok({ commit: "c1" });
  await settle();
  assert.ok(t.toggle("creative/excalidraw").checked && !t.toggle("creative/excalidraw").disabled);
  assert.match(t.find("skills-status")[0].textContent, /Excalidraw is on\. Your Hermes agents pick it up at their next skills update\./);
  // switched off, and that write fails: back on, said
  const off = t.toggle("creative/excalidraw");
  off.checked = false;
  off.onchange();
  assert.deepEqual(t.writes()[1].body.files, [{ path: "optional/hermes/creative/excalidraw.on", delete: true }]);
  t.writes()[1].fail(new Error("offline"));
  await settle();
  assert.ok(t.toggle("creative/excalidraw").checked, "put back");
  assert.match(t.find("skills-status")[0].textContent, /Couldn't turn off Excalidraw: offline\. Try again\./);
});

test("a labelled skill asks before it is on: Cancel leaves it off, Turn on writes it", async () => {
  const t = page();
  t.p.open();
  await settle();
  t.answerReads();
  await settle();
  t.find("skills-tab")[1].onclick();
  const sw = t.toggle("productivity/telephony");
  sw.checked = true;
  sw.onchange();
  assert.equal(t.writes().length, 0, "nothing written yet");
  const asking = walk(t.item("productivity/telephony")).find((n) => n.className === "skills-ask");
  assert.equal(asking.children[0].textContent, "Telephony can spend money and can contact people for you. Your agents won't ask before they use it.");
  assert.ok(!t.toggle("productivity/telephony").checked);
  walk(asking).find((n) => n.textContent === "Cancel").onclick();
  assert.ok(!walk(t.item("productivity/telephony")).some((n) => n.className === "skills-ask"));
  const again = t.toggle("productivity/telephony");
  again.checked = true;
  again.onchange();
  walk(t.item("productivity/telephony")).find((n) => n.textContent === "Turn on").onclick();
  assert.deepEqual(t.writes().map((c) => c.body.files[0].path), ["optional/hermes/productivity/telephony.on"]);
});

test("a search narrows both tabs, and one that finds nothing offers to clear it", async () => {
  const t = page();
  t.p.open();
  await settle();
  t.answerReads();
  await settle();
  t.find("skills-tab")[1].onclick();
  const search = t.find("skills-search")[0];
  search.value = "fine-tuning";
  search.oninput();
  assert.deepEqual(t.find("skills-item").map((n) => n.dataset.skill), ["mlops/training/unsloth"]);
  search.value = "nothing like it";
  search.oninput();
  assert.equal(t.find("skills-empty")[0].children[0].textContent, "No skills match “nothing like it”.");
  walk(t.find("skills-empty")[0]).find((n) => n.textContent === "Clear search").onclick();
  assert.equal(t.find("skills-item").length, 3);
});

test("reopened while a switch saves, the page reads once the write lands, and shows it", async () => {
  const t = page({ on: ["creative/excalidraw"] });
  t.p.open();
  await settle();
  t.answerReads();
  await settle();
  const sw = t.toggle("creative/excalidraw");
  sw.checked = false;
  sw.onchange();
  t.p.close();
  t.p.open();
  await settle();
  const reads = () => t.calls.filter((c) => c.method === "GET").length;
  assert.equal(reads(), 2, "no read while the write is out");
  assert.equal(t.find("skills-loading").length, 1);
  t.writes()[0].ok({ commit: "c2" });
  await settle();
  await settle();
  for (const c of t.calls.filter((c) => c.method === "GET" && !c.done)) {
    c.done = true;
    c.ok(c.path.endsWith("/files") ? { files: [] } : CATALOG);
  }
  await settle();
  t.find("skills-tab")[1].onclick();
  assert.ok(!t.toggle("creative/excalidraw").checked, "off, as written");
});

test("a switch whose id the catalog no longer names stays in On, by its id, to be switched off", async () => {
  assert.deepEqual(onRows(CATALOG.skills, new Set(["creative/excalidraw", "creative/retired"])).map((s) => [s.id, s.name, !!s.gone]), [["creative/excalidraw", "excalidraw", false], ["creative/retired", "retired", true]]);
  const t = page({ on: ["creative/retired"] });
  t.p.open();
  await settle();
  t.answerReads();
  await settle();
  assert.equal(t.find("skills-tab")[0].textContent, "On · 2");
  const sw = t.toggle("creative/retired");
  assert.ok(sw.checked);
  sw.checked = false;
  sw.onchange();
  assert.deepEqual(t.writes()[0].body.files, [{ path: "optional/hermes/creative/retired.on", delete: true }]);
  t.writes()[0].ok({ commit: "c3" });
  await settle();
  assert.equal(t.item("creative/retired"), undefined, "gone once off");
  assert.equal(t.find("skills-tab")[0].textContent, "On · 1");
});

test("a skills fragment without the catalog (no longer on the template) still shows what is on, and its files", async () => {
  const t = page({ catalog: null });
  t.p.open();
  await settle();
  t.answerReads();
  await settle();
  await settle();
  assert.deepEqual(t.find("skills-tab").map((n) => n.textContent), ["On · 1", "Library · 0"]);
  assert.deepEqual(t.find("skills-desc").map((n) => n.textContent), ["Publish and share apps."]);
  t.find("skills-files")[0].onclick();
  assert.deepEqual(t.opened, [["app", "skills--z8w6"]]);
  t.find("skills-tab")[1].onclick();
  assert.match(t.find("skills-empty")[0].children[0].textContent, /The library isn't in this skills fragment/);
});

test("asked twice while it has none, the shell makes one skills fragment, and names the one the computers pick", async () => {
  let fragments = [];
  const posts = [];
  const api = (method, path, body) => {
    assert.equal(`${method} ${path}`, "POST /api/fragments");
    return new Promise((ok) => posts.push({ body, ok }));
  };
  const settings = createSettings({
    api, account: {}, navigation: {}, notices: {}, sections: {}, agents: {},
    allFragments: () => fragments, own: (f) => f.role === "owner", freeLabel: (l) => l,
    load: async () => { fragments = [{ name: "skills--zzzz", kind: "skills", role: "owner" }, { name: "skills--aaaa", kind: "skills", role: "owner" }]; },
  });
  const first = settings.skillsFragment();
  const second = settings.skillsFragment();
  await settle();
  assert.equal(posts.length, 1, "one made");
  // another tab made one meanwhile: both pages name the one Hermes takes
  posts[0].ok({ name: "skills--zzzz" });
  assert.deepEqual(await Promise.all([first, second]), ["skills--aaaa", "skills--aaaa"]);
});
