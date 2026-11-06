// The chat template's reading of a channel page's `more`
// (templates/chat/site/pages.js), as its earlier messages and its work
// read-back use it: said, it is the answer, a short page that says more
// included (a page cut at its bytes); unsaid (an older release's page, one
// open across a rollback), a full page reads on and a short one is the last.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { moreOf } from "../../templates/chat/site/pages.js";

const records = (n) => Array.from({ length: n }, (_, i) => ({ seq: i + 1 }));

for (const asked of [100, 1000]) {
  test(`a page of ${asked} asked: more said is the answer, whatever it holds`, () => {
    assert.equal(moreOf({ records: records(3), more: true }, asked), true, "cut at its bytes, more remain");
    assert.equal(moreOf({ records: records(asked), more: false }, asked), false, "full, and the last");
    assert.equal(moreOf({ records: [], more: false }, asked), false);
  });
  test(`a page of ${asked} asked, from an older release (no more): full reads on, short is the last`, () => {
    assert.equal(moreOf({ records: records(asked) }, asked), true);
    assert.equal(moreOf({ records: records(asked - 1) }, asked), false);
    assert.equal(moreOf({ records: [] }, asked), false);
  });
}
