// cell/shell/chat-list.js's `askedAgentOf` (Paul, 2026-10-31: an agent's
// chosen name, every surface): whom the desktop's and the dock's ways to
// ask for an app name, as the chat their prompt goes to (shell.js
// `promptChat`): the open chat's agent of the person's, the lead first, when
// that chat is their own; else their main agent.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { askedAgentOf } from "../shell/chat-list.js";

const main = { fragment: "datastar-pro--a1b2", identity: "npub-main" };
const fred = { fragment: "fred--c3d4", identity: "npub-fred" };
const juniper = { fragment: "juniper--e5f6", identity: "npub-juniper" };
const mine = [main, fred, juniper];
const chats = {
  fred: { name: "fred-chat--1", kind: "chat", role: "owner", agents: ["npub-fred"] },
  group: { name: "group--2", kind: "chat", role: "owner", agents: ["npub-someone", "npub-juniper", "npub-fred"] },
  shared: { name: "theirs--3", kind: "chat", role: "editor", agents: ["npub-fred"] },
  nobody: { name: "quiet--4", kind: "chat", role: "owner", agents: ["npub-someone"] },
  app: { name: "todo--5", kind: "app", role: "owner" },
};
const asked = (open) => askedAgentOf({ open, own: (f) => f.role === "owner", agentsOf: (name) => Object.values(chats).find((f) => f.name === name)?.agents ?? [], mine, main });

test("the open chat of theirs: its agent, not their main one", () => {
  assert.equal(asked(chats.fred), fred);
});

test("a group of theirs: its first agent of theirs, in the chat's order", () => {
  assert.equal(asked(chats.group), juniper);
});

test("no chat open, a chat shared with them, one with no agent of theirs, or an app: their main agent", () => {
  assert.equal(asked(null), main);
  assert.equal(asked(chats.shared), main);
  assert.equal(asked(chats.nobody), main);
  assert.equal(asked(chats.app), main);
});
