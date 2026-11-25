// The connectors' question module (templates/simplex/applib/question.mjs,
// every connector's: blessed.rs `every_connector_asks_one_way`)
// against the cases the bridge's own SimpleX wording is held to
// (images/bridge/src/questions.json, driver.rs
// `a_question_is_worded_as_every_door_words_it`): one way for every door.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { asked } from "../../../templates/simplex/applib/question.mjs";

const { cases } = JSON.parse(readFileSync(new URL("../../../images/bridge/src/questions.json", import.meta.url), "utf8"));

test("a question is worded as every door words it", () => {
  assert.ok(cases.length >= 8, `${cases.length} cases`);
  for (const c of cases) assert.equal(asked(c.body), c.said, c.of);
});

test("what is no card is no question", () => {
  for (const body of [undefined, null, "turn.prompt", 7, [], { kind: "message", text: "hi" }]) assert.equal(asked(body), null, JSON.stringify(body));
});
