// cell/shell/session-text.js: an imported conversation's search snippet,
// said for a person (History, the launcher's Knowledge). Its fixtures are
// templates/sessions/README.md's "Section text", verbatim
// (`fragment_core::session_search`'s test holds the README to what it writes).
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { speakers } from "../shell/session-text.js";

const TURN = "Person: when do I plant beans? [plot.png]\nAgent: (used web_search)\nAgent: In spring, after the frost.";
const SUMMARY = "Summary: Ann plants beans in spring.";

test("a turn: You and the agent by its name, its files and tools left out", () => {
  assert.equal(speakers(TURN, "Juniper"), "You: when do I plant beans? · Juniper: In spring, after the frost.");
  assert.equal(speakers(SUMMARY, "Juniper"), "Summary: Ann plants beans in spring.");
});

test("an import that names no agent: Agent", () => {
  assert.equal(speakers(TURN), "You: when do I plant beans? · Agent: In spring, after the frost.");
  assert.equal(speakers(TURN, "  "), "You: when do I plant beans? · Agent: In spring, after the frost.");
});

test("a snippet cut at either end: its first words no one's, its marks cut short taken off too", () => {
  assert.equal(speakers("…ant beans? [plot.png]\nAgent: In spring, after the fro…", "Juniper"), "…ant beans? · Juniper: In spring, after the fro…");
  assert.equal(speakers("Person: and peas?\nAgent: Later, in April (used web_se…", "Juniper"), "You: and peas? · Juniper: Later, in April");
  assert.equal(speakers("Person: see the photo [garden.jp…", "Juniper"), "You: see the photo");
});

// FIN-365: a snippet cut inside a speaker's line keeps that line's label
// (fragment_core::search_index `snippet`), so its words still say whose.
test("a snippet cut inside a speaker's line, its label kept: that speaker's", () => {
  assert.equal(speakers("Agent: …tomatoes and peppers in the two beds. Pole beans fix nitrogen…", "Juniper"), "Juniper: …tomatoes and peppers in the two beds. Pole beans fix nitrogen…");
  assert.equal(speakers("Person: …and the fence?\nAgent: Beans can climb it.", "Juniper"), "You: …and the fence? · Juniper: Beans can climb it.");
});

test("a message's own lines continue its speaker; one speaker's lines in a row are one", () => {
  assert.equal(speakers("Person: first line\nsecond line\nAgent: ok\nAgent: and more", "Juniper"), "You: first line second line · Juniper: ok and more");
});

test("words that look like marks are taken off only at a speaker's end", () => {
  assert.equal(speakers("Person: what does [x] mean in (used y) terms?", "Juniper"), "You: what does [x] mean in (used y) terms?");
  // a person's own words never lose a tool mark: only an agent's do
  assert.equal(speakers("Person: try (used tape)", "Juniper"), "You: try (used tape)");
});

test("nothing, or only marks: nothing", () => {
  assert.equal(speakers("", "Juniper"), "");
  assert.equal(speakers(undefined), "");
  assert.equal(speakers("Agent: (used web_search)", "Juniper"), "");
});
