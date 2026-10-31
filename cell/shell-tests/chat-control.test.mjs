// The chat template's turn control (templates/chat/site/control.js): who a
// turn's Stop, its owner's commands and the words answering its question
// are, as the bridge rules them (images/bridge, `who_answers`). A chat of
// Paul's, his agents Juniper (its lead) and Maple, and Rae with her agent
// Wren. Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { controlledBy, controllerOf } from "../../templates/chat/site/control.js";

const [PAUL, RAE, JUNIPER, MAPLE, WREN] = ["npub1paul", "npub1rae", "npub1juniper", "npub1maple", "npub1wren"];
const MEMBERS = [
  { principal: JUNIPER, kind: "agent", owner: PAUL },
  { principal: MAPLE, kind: "agent", owner: PAUL },
  { principal: WREN, kind: "agent", owner: RAE },
  { principal: PAUL, kind: "person" },
  { principal: RAE, kind: "person" },
];
const turn = (agent, asker, channel, startAt = 1, end = null) => ({ agent, asker, cause: { fragment: "x", channel, seq: 1 }, startAt, end });
// what the page's Stop and `/` take: the person's newest running turn, else the lead
const commandTarget = (turns, me) => controlledBy(turns, MEMBERS, me)?.agent ?? JUNIPER;

test("an answer come back is asked as the agent that answered, and its agent's owner's to stop, command and answer", () => {
  // Maple, not the chat's lead, passes on Juniper's answer to her question
  const answer = turn(MAPLE, JUNIPER, "tasks");
  assert.equal(controllerOf(answer, MEMBERS), PAUL);
  assert.equal(controlledBy([answer], MEMBERS, PAUL), answer, "Paul's Stop names it");
  assert.equal(commandTarget([answer], PAUL), MAPLE, "his commands go to Maple, not the lead");
  assert.equal(controlledBy([answer], MEMBERS, JUNIPER), null, "the agent that answered does not control it");
  assert.equal(controlledBy([answer], MEMBERS, RAE), null, "nor does anyone else in the chat");
  assert.equal(commandTarget([answer], RAE), JUNIPER);
  // a routine's is its owner's too
  assert.equal(controllerOf(turn(WREN, RAE, "tasks"), MEMBERS), RAE);
});

test("any other turn is its asker's", () => {
  assert.equal(controllerOf(turn(JUNIPER, PAUL, "chat"), MEMBERS), PAUL);
  // someone else's message to Paul's agent is theirs to stop, not his
  const raes = turn(JUNIPER, RAE, "chat");
  assert.equal(controlledBy([raes], MEMBERS, RAE), raes);
  assert.equal(controlledBy([raes], MEMBERS, PAUL), null);
  // an agent's hand-off in the chat is the handing agent's, not its owner's
  const handed = turn(MAPLE, JUNIPER, "chat");
  assert.equal(controllerOf(handed, MEMBERS), JUNIPER);
  assert.equal(controlledBy([handed], MEMBERS, PAUL), null);
  assert.equal(commandTarget([handed], PAUL), JUNIPER, "with nothing of his running, the lead");
});

test("only a running turn, the newest; a tasks turn of an agent no longer here is no one's", () => {
  const older = turn(JUNIPER, PAUL, "chat", 1);
  const newer = turn(MAPLE, JUNIPER, "tasks", 2);
  assert.equal(controlledBy([older, newer], MEMBERS, PAUL), newer);
  assert.equal(controlledBy([older, { ...newer, end: { outcome: "idle" } }], MEMBERS, PAUL), older);
  assert.equal(controlledBy([{ ...older, startAt: null }], MEMBERS, PAUL), null, "not started");
  assert.equal(controllerOf(turn("npub1gone", JUNIPER, "tasks"), MEMBERS), null);
  assert.equal(controlledBy([older], MEMBERS, undefined), null, "no one signed in");
});
