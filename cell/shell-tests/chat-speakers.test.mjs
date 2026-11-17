// Whose words a chat message is (templates/chat/site/speakers.js): a
// relayed answer of the owner's come before the members are read is the
// owner's once they are, and a turn that asked the owner then waits on no
// one (Alex's review of #1078); a refresh that says the relay is no agent
// of theirs takes it back. Run by `cargo xtask check` on the pinned Node
// (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { noteSaid, saidSince, speakerOf } from "../../templates/chat/site/speakers.js";

const [OWNER, AGENT, OTHER] = ["npub1owner", "npub1agent", "npub1other"];
const relay = { principal: AGENT, turn: null, at: 300, relayed: { for: OWNER, by: AGENT, via: "simplex" } };
const asked = { asks: OWNER, at: 200 };

test("a relayed answer come before the members: the owner's once they are read", () => {
  const spoke = new Map();
  noteSaid(spoke, relay);
  // before the first read of the members, a relay is its poster's
  assert.equal(speakerOf(relay, []), AGENT);
  assert.equal(saidSince(spoke, [], asked.asks, asked.at), false);
  // read: the relay is the owner's agent
  const members = [{ principal: OWNER, kind: "person" }, { principal: AGENT, kind: "agent", owner: OWNER }];
  assert.equal(speakerOf(relay, members), OWNER);
  assert.equal(saidSince(spoke, members, asked.asks, asked.at), true, "the turn that asked waits on no one");
});

test("a refresh of the members is read too: a relay by no agent of the owner's is its poster's", () => {
  const spoke = new Map();
  noteSaid(spoke, relay);
  const theirs = [{ principal: AGENT, kind: "agent", owner: OWNER }];
  const anothers = [{ principal: AGENT, kind: "agent", owner: OTHER }];
  assert.equal(saidSince(spoke, theirs, OWNER, asked.at), true);
  assert.equal(saidSince(spoke, anothers, OWNER, asked.at), false);
  assert.equal(saidSince(spoke, anothers, AGENT, asked.at), true);
});

test("only words said since the question, and none of a turn's", () => {
  const spoke = new Map();
  noteSaid(spoke, { principal: OWNER, turn: null, at: 100, relayed: null });
  noteSaid(spoke, { principal: OWNER, turn: "t1", at: 400, relayed: null });
  assert.equal(saidSince(spoke, [], OWNER, 200), false);
  noteSaid(spoke, { principal: OWNER, turn: null, at: 250, relayed: null });
  assert.equal(saidSince(spoke, [], OWNER, 200), true);
});
