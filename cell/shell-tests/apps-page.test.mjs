// cell/shell/apps-page.js: what the Apps and Brains pages list (FIN-352). Run by
// `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { listed, shelfOf, SHELVES } from "../shell/apps-page.js";

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
