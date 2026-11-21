// cell/shell/apps-page.js: what the Apps and Brains pages list (FIN-352). Run by
// `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { accessView, ACCESS_FILTERS, listed, shelfOf, shoots, SHELVES } from "../shell/apps-page.js";

const row = (kind, reach) => ({ name: `${kind}--a1b2`, kind, role: "owner", sharing: { visibility: "members", ...(reach === undefined ? {} : { reach }) } });

test("apps and brains are listed, whatever their reach, and a kind it does not know", () => {
  for (const kind of ["app", "brain", "site"]) {
    for (const reach of [undefined, false, true]) assert.equal(listed(row(kind, reach)), true, `${kind} ${reach}`);
  }
});

test("a chat that reaches their accounts (a connector) is listed; one that does not, or says nothing, is not", () => {
  assert.equal(listed(row("chat", true)), true);
  assert.equal(listed(row("chat", false)), false);
  assert.equal(listed(row("chat")), false);
});

// Paul's preview console, 2026-11-19: a members-only brain of his was
// asked for its card 65 times in a session, each a 404, though the
// platform never shoots a members-only page (docs/api.md, Cards).
test("a card is shot of an app or brain unless their own row says it is members only", () => {
  const of = (kind, role, visibility) => ({ name: `${kind}--a1b2`, kind, role, ...(visibility ? { sharing: { visibility } } : {}) });
  for (const kind of ["app", "brain"]) {
    assert.equal(shoots(of(kind, "owner", "members")), false, `their own members-only ${kind}`);
    for (const visibility of ["link", "public"]) assert.equal(shoots(of(kind, "owner", visibility)), true, `their own ${visibility} ${kind}`);
    // only an owner's row says its visibility: anyone else's may be shot
    for (const role of ["editor", "viewer"]) assert.equal(shoots(of(kind, role)), true, `a ${kind} shared with them as ${role}`);
  }
  for (const kind of ["chat", "agent", "skills", "sessions", "site"]) assert.equal(shoots(of(kind, "owner", "link")), false, `a ${kind}`);
});

// Alex on #918: an agent's door is its SimpleX link, opened and closed in
// Settings; listed here, its Reaches would read no door and offer no Unlink.
test("an agent, skills and a Session Archive are never listed, even with reach", () => {
  for (const kind of ["agent", "skills", "sessions"]) {
    for (const reach of [undefined, false, true]) assert.equal(listed(row(kind, reach)), false, `${kind} ${reach}`);
  }
});

// A brain is on the Brains page alone; everything else listed, a kind it
// does not know and a connector's chat too, on the Apps page alone.
test("each listed fragment is on exactly one shelf: a brain on Brains, the rest on Apps", () => {
  assert.equal(shelfOf(row("brain")), "brains");
  for (const kind of ["app", "site"]) assert.equal(shelfOf(row(kind)), "apps", kind);
  assert.equal(shelfOf(row("chat", true)), "apps");
  for (const f of [row("app"), row("brain"), row("site"), row("chat", true)]) assert.ok(SHELVES[shelfOf(f)], f.kind);
});

test("each shelf's visits are an ask kind of its own", async () => {
  const { KINDS } = await import("../shell/asks.js");
  const asks = Object.values(SHELVES).map((s) => s.ask);
  assert.equal(new Set(asks).size, asks.length);
  for (const ask of asks) assert.ok(KINDS.includes(ask), ask);
});

// Austin, 2026-11-20: their own apps apart from those shared with them, and
// what they may edit apart from what they may only view.
const as = (name, role) => ({ name, kind: "app", role });
const names = (rows) => rows.map((f) => f.name);

test("their own and those shared with them are two sections, each in the list's order", () => {
  const live = [as("b", "editor"), as("a", "owner"), as("c", "viewer"), as("d", "owner")];
  const view = accessView(live, []);
  assert.equal(view.filter, "all");
  assert.deepEqual(names(view.yours), ["a", "d"]);
  assert.deepEqual(names(view.shared), ["b", "c"]);
  assert.deepEqual(view.filters.map((f) => [f.key, f.count]), [["all", 4], ["yours", 2], ["shared", 2], ["edit", 3], ["view", 1]]);
});

test("each filter keeps only what it names, archived ones too", () => {
  const live = [as("own", "owner"), as("edit", "editor"), as("view", "viewer")];
  const archived = [as("old-own", "owner"), as("old-view", "viewer")];
  const of = (filter) => { const v = accessView(live, archived, filter); return [names(v.yours), names(v.shared), names(v.archived)]; };
  assert.deepEqual(of("yours"), [["own"], [], ["old-own"]]);
  assert.deepEqual(of("shared"), [[], ["edit", "view"], ["old-view"]]);
  assert.deepEqual(of("edit"), [["own"], ["edit"], ["old-own"]]);
  assert.deepEqual(of("view"), [[], ["view"], ["old-view"]]);
});

test("with nothing shared with them there are no filters, and a filter chosen before shows everything", () => {
  const live = [as("a", "owner"), as("b", "owner")];
  for (const filter of ["all", "shared", "view"]) {
    const view = accessView(live, [], filter);
    assert.deepEqual(view.filters, [], filter);
    assert.equal(view.filter, "all", filter);
    assert.deepEqual(names(view.yours), ["a", "b"], filter);
  }
});

test("one shared with them only in Archived still offers the filters", () => {
  const view = accessView([as("a", "owner")], [as("z", "viewer")], "shared");
  assert.equal(view.filter, "shared");
  assert.deepEqual(names(view.archived), ["z"]);
  assert.equal(view.filters.length, ACCESS_FILTERS.length);
});

test("a filter it does not know is All", () => {
  const view = accessView([as("a", "owner"), as("b", "viewer")], [], "nonsense");
  assert.equal(view.filter, "all");
  assert.deepEqual([names(view.yours), names(view.shared)], [["a"], ["b"]]);
});
