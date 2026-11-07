// What a message carries follows the platform's latest word (limits.js;
// cell/client.mjs `limits()`): a page left open across a deploy that
// changes `upload_max_bytes` hears it in the hello its socket's reconnect
// brings, and checks a file it takes then, and every file queued before,
// against that, a rise and a fall alike. The real client, over a fake
// socket; its reconnect waits run on node's mock clock, not a real one.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { over, refusal } from "../../templates/chat/site/limits.js";

const MB = 1_000_000;
const sockets = [];
class FakeSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  constructor() {
    this.readyState = FakeSocket.CONNECTING;
    sockets.push(this);
  }
  send() {}
  close() {
    this.readyState = 3;
    this.onclose?.({ code: 1006, reason: "" });
  }
  // the fragment's side: open, then its hello
  hello(limits) {
    this.readyState = FakeSocket.OPEN;
    this.onopen?.();
    this.onmessage?.({ data: JSON.stringify({ type: "hello", id: "s", principal: "npub1me", role: "editor", platform: "https://platform.test", presence: [], limits }) });
  }
}
globalThis.WebSocket = FakeSocket;
mock.timers.enable({ apis: ["setTimeout"] });
const fragment = await import("../client.mjs");

const at = (most) => ({ uploadMaxBytes: most * MB, filesPerMessage: 8 });
const file = (name, mb) => ({ name, size: mb * MB });

// the socket dropped (a deploy), dialled again after its backoff, and the
// new release's hello
function redeploy(limits) {
  sockets.at(-1).close();
  mock.timers.tick(60_000);
  sockets.at(-1).hello(limits);
}

test("a page's limits are the latest hello's: a deploy's rise, then a fall, each heard on reconnect", async () => {
  assert.equal(fragment.limits(), null, "none before the first hello");
  // a channel followed keeps the socket wanted, as the chat's are
  fragment.subscribe("chat", () => {});
  sockets.at(-1).hello(at(16));
  assert.deepEqual(fragment.limits(), at(16));
  const queued = [file("atlas.tiff", 20)];
  assert.equal(refusal(fragment.limits(), queued), `atlas.tiff is ${over(at(16))}.`, "20 MB is past 16");

  redeploy(at(32));
  assert.deepEqual(fragment.limits(), at(32), "the rise is heard");
  assert.deepEqual((await fragment.me()).limits, at(32), "and me() answers the same hello");
  assert.equal(refusal(fragment.limits(), queued), null, "the file refused before is taken now");
  const kept = [...queued, file("map.png", 30)];
  assert.equal(refusal(fragment.limits(), kept), null, "both taken while 32 MB holds");

  redeploy(at(16));
  assert.deepEqual(fragment.limits(), at(16), "the fall is heard");
  assert.equal(refusal(fragment.limits(), kept), `atlas.tiff is ${over(at(16))}.`, "files queued before it are checked again as they are sent");
  assert.equal(refusal(fragment.limits(), [file("note.txt", 1)], kept.length), null);

  redeploy(undefined);
  assert.equal(fragment.limits(), null, "an older platform's hello names none (its upload route decides)");
  assert.equal(refusal(fragment.limits(), kept), null);
});

test("a message carries at most its files, counting those it holds; a file's name or none is said", () => {
  const limits = { uploadMaxBytes: 4 * MB, filesPerMessage: 3 };
  assert.equal(refusal(limits, [file("a", 1)], 2), null);
  assert.equal(refusal(limits, [file("a", 1)], 3), "A message carries at most 3 files.");
  assert.equal(refusal(limits, [file("", 5)]), `A file is ${over(limits)}.`);
  assert.equal(over(limits), "over 4 MB, the most a chat takes");
  assert.equal(refusal(limits, [file("edge", 4)]), null, "at the bound is taken");
});
