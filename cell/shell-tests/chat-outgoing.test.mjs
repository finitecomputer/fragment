// The chat page's outgoing post lifecycle (templates/chat/site/outgoing.js).
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { OutgoingPosts } from "../../templates/chat/site/outgoing.js";

const message = (id, text) => ({ id, body: { text }, status: "sending", error: null, seq: `sending:${id}`, at: 10 });
const record = (text, seq = 20) => ({ seq, body: { text } });

test("sending becomes posted once and keeps the post id on its server record", () => {
  const posts = new OutgoingPosts();
  posts.transition("p1", "posting", { message: message("p1", "hello") });
  assert.equal(posts.get("p1").status, "sending");

  const completed = posts.transition("p1", "posted", { record: record("hello") });
  assert.equal(completed.retried, false);
  assert.deepEqual(posts.get("p1"), { ...message("p1", "hello"), status: "posted", seq: 20 });
  assert.equal(posts.postId(20), "p1");
  assert.equal(posts.postId(21), null);
});

test("failed becomes retrying and then posted without changing message identity", () => {
  const posts = new OutgoingPosts();
  posts.transition("p1", "posting", { message: message("p1", "try me") });
  posts.transition("p1", "failed", { error: "offline" });
  assert.deepEqual(posts.get("p1"), { ...message("p1", "try me"), status: "failed", error: "offline" });

  posts.transition("p1", "posting");
  assert.equal(posts.get("p1").status, "retrying");
  assert.equal(posts.get("p1").error, "offline", "the error and Retry UI remain while the retry is under way");
  assert.equal(posts.get("p1").seq, "sending:p1");
  assert.equal(posts.get("p1").at, 10);

  const completed = posts.transition("p1", "posted", { record: record("try me", 30) });
  assert.equal(completed.retried, true, "a successful retry starts waiting for its agent turn");
  assert.equal(posts.get("p1").status, "posted");
  assert.equal(posts.get("p1").error, null);
  assert.equal(posts.postId(30), "p1");
});

test("a socket record before the HTTP answer is authoritative and terminal", () => {
  const posts = new OutgoingPosts();
  posts.transition("p1", "posting", { message: message("p1", "race") });
  posts.transition("p2", "posting", { message: message("p2", "race") });

  const fromSocket = posts.record(record("race", 40));
  assert.equal(fromSocket.post.id, "p1", "identical bodies match in post order, as before");
  assert.equal(posts.get("p1").status, "posted");
  assert.equal(posts.get("p2").status, "sending");

  const fromHttp = posts.transition("p1", "posted", { record: record("race", 40) });
  assert.equal(fromHttp.retried, false, "the later answer does not complete the post twice");
  posts.transition("p1", "failed", { error: "late failure" });
  assert.equal(posts.get("p1").status, "posted", "a late failure cannot resurrect a posted message");
  assert.equal(posts.get("p1").error, null);
  assert.equal(posts.postId(40), "p1");
});

test("a retry's socket record also wins over its later HTTP answer", () => {
  const posts = new OutgoingPosts();
  posts.transition("p1", "posting", { message: message("p1", "again") });
  posts.transition("p1", "failed", { error: "offline" });
  posts.transition("p1", "posting");

  const fromSocket = posts.record(record("again", 50));
  assert.equal(fromSocket.retried, true);
  assert.equal(posts.transition("p1", "posted", { record: record("again", 50) }).retried, false);
  assert.equal(posts.get("p1").status, "posted");
});
