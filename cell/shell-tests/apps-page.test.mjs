// cell/shell/apps-page.js: what the Apps page lists (FIN-352). Run by
// `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { listed } from "../shell/apps-page.js";

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
