// Whose words a chat message is, and where it came from
// (templates/chat/site/speakers.js): relayed owner messages, outside speakers
// heard in a room, room metadata, prior room context, and anti-spoofing
// against members, app code, and non-door authors. Run by `cargo xtask check`
// on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  contextOf,
  doorName,
  heardSpeaker,
  noteSaid,
  parseDoorFields,
  relayedOwner,
  roomOf,
  saidSince,
  speakerOf,
  viaLabel,
} from "../../templates/chat/site/speakers.js";

const [OWNER, AGENT, OTHER, CONN] = ["npub1owner", "npub1agent", "npub1other", "npub1conn"];
const MEMBERS = [
  { principal: OWNER, kind: "person" },
  { principal: AGENT, kind: "agent", owner: OWNER },
  { principal: OTHER, kind: "person" },
];
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

test("outside speaker heard in a room: parsed, attributed, and formatted", () => {
  const body = {
    text: "@Juniper is the deploy green?",
    heard: { via: "slack", from: "U0456", name: " Maple \n Lee " },
    room: { id: "C0123", name: " #deploys\t", thread: "1790000000.000100" },
    context: [
      { name: "Sam", text: "deploy started", at: 1790000000000 },
      { name: "Alex", text: "watching\nlogs" },
    ],
  };
  const parsed = parseDoorFields(body, CONN, null, false);
  const m = { principal: CONN, turn: null, at: 350, author: "door", code: false, ...parsed };

  // Before members load, nothing is trusted
  assert.equal(heardSpeaker(m, []), null);
  assert.equal(roomOf(m, []), null);
  assert.deepEqual(contextOf(m, []), []);
  assert.equal(viaLabel(m, []), null);

  // Once members load, connector key is a non-member with author "door"
  assert.deepEqual(heardSpeaker(m, MEMBERS), { via: "slack", from: "U0456", name: "Maple Lee" });
  assert.deepEqual(roomOf(m, MEMBERS), { id: "C0123", name: "#deploys", thread: "1790000000.000100" });
  assert.equal(contextOf(m, MEMBERS).length, 2);
  assert.equal(contextOf(m, MEMBERS)[1].text, "watching logs");
  assert.equal(viaLabel(m, MEMBERS), "Slack · #deploys");
  assert.equal(doorName("slack"), "Slack");

  // An outside speaker never counts as the owner or satisfies an owner question
  assert.equal(relayedOwner(m, MEMBERS), null);
  assert.equal(speakerOf(m, MEMBERS), CONN);
  const spoke = new Map();
  noteSaid(spoke, m);
  assert.equal(saidSince(spoke, MEMBERS, OWNER, 200), false);
});

test("owner relayed in a room shows both door and room, and falls back to room id when unnamed", () => {
  const parsed = parseDoorFields(
    {
      text: "check the build",
      relayed: { via: "slack", for: OWNER, agent: "juniper--k3x9" },
      room: { id: "C0123", name: "#deploys" },
      context: [{ name: "Sam", text: "build red" }],
    },
    CONN,
    null,
    false,
  );
  const m = { principal: CONN, turn: null, at: 400, author: "door", code: false, ...parsed };
  assert.equal(relayedOwner(m, MEMBERS), OWNER);
  assert.equal(heardSpeaker(m, MEMBERS), null);
  assert.equal(viaLabel(m, MEMBERS), "Slack · #deploys");
  assert.equal(contextOf(m, MEMBERS).length, 1);

  const unnamed = parseDoorFields(
    {
      text: "hi",
      heard: { via: "slack", from: "U9", name: "Sam" },
      room: { id: "D9999" },
    },
    CONN,
    null,
    false,
  );
  const mUnnamed = { principal: CONN, turn: null, at: 410, author: "door", code: false, ...unnamed };
  assert.equal(viaLabel(mUnnamed, MEMBERS), "Slack · D9999");
});

test("anti-spoofing: members, app code, anonymous visitors, and non-door authors cannot forge heard/room/context", () => {
  const body = {
    text: "fake",
    heard: { via: "slack", from: "U1", name: "CEO" },
    room: { id: "C0123", name: "#general" },
    context: [{ name: "Sam", text: "hi" }],
  };
  const parsed = parseDoorFields(body, OWNER, null, false);
  for (const forged of [
    { principal: OWNER, author: "door", code: false, ...parsed }, // chat member
    { principal: AGENT, author: "door", code: false, ...parsed }, // agent member
    { principal: CONN, author: "person", code: false, ...parsed }, // external person on public chat
    { principal: CONN, author: "code", code: true, ...parsed }, // app code
    { principal: "anon:1234", author: "door", code: false, ...parsed }, // anonymous visitor
  ]) {
    assert.equal(heardSpeaker(forged, MEMBERS), null);
    assert.equal(roomOf(forged, MEMBERS), null);
    assert.deepEqual(contextOf(forged, MEMBERS), []);
    assert.equal(viaLabel(forged, MEMBERS), null);
  }
});

test("strict parsing rejects contradictory relayed+heard, missing/bad room, and out-of-bounds context", () => {
  const both = parseDoorFields(
    {
      text: "x",
      relayed: { via: "slack", for: OWNER, agent: "juniper--k3x9" },
      heard: { via: "slack", from: "U1", name: "Maple" },
      room: { id: "C1", name: "#deploys" },
    },
    CONN,
    null,
    false,
  );
  assert.equal(both.relayed, null);
  assert.equal(both.heard, null);
  assert.equal(both.room, null);

  // heard without room is ignored
  const noRoom = parseDoorFields({ text: "x", heard: { via: "slack", from: "U1", name: "Maple" } }, CONN, null, false);
  assert.equal(noRoom.heard, null);

  // relayed with malformed room is ignored
  const badRoom = parseDoorFields(
    { text: "x", relayed: { via: "slack", for: OWNER, agent: "juniper--k3x9" }, room: { id: "../evil" } },
    CONN,
    null,
    false,
  );
  assert.equal(badRoom.relayed, null);
  assert.equal(badRoom.room, null);

  // >20 context items or >16 KiB context yields empty context without dropping heard/room
  const tooMany = Array.from({ length: 21 }, (_, i) => ({ name: "Sam", text: `m${i}` }));
  const over = parseDoorFields(
    { text: "x", heard: { via: "slack", from: "U1", name: "Maple" }, room: { id: "C1" }, context: tooMany },
    CONN,
    null,
    false,
  );
  assert.ok(over.heard);
  assert.ok(over.room);
  assert.deepEqual(over.context, []);
});
