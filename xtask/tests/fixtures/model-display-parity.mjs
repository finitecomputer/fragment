// The same cases run through Hermes' real selection/profile code in its tests.
// Here execute the real agent-page display and Settings tier-label functions.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";
const fixtures = JSON.parse(readFileSync(new URL("../../../images/hermes/boot/tests/fixtures/model-display-parity.json", import.meta.url), "utf8"));
const page = readFileSync(new URL("../../../templates/agent/site/agent.js", import.meta.url), "utf8");
const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const runs = page.match(/^function runs\([^]*?^\}/m)?.[0];
const defaultTier = shell.match(/^const DEFAULT_AGENT_TIER = .*;$/m)?.[0];
const tierLabel = shell.slice(shell.indexOf("const TIER ="), shell.indexOf("const modelValue ="));
assert.ok(runs && defaultTier && tierLabel.startsWith("const TIER ="));
for (const c of fixtures.cases) {
  test(c.name, () => {
    const models = c.models ?? fixtures.models;
    const context = createContext({ settings: c.settings, me: { modelProviders: c.modelProviders }, models, state: { computer: { models } } });
    runInContext(`${defaultTier}\n${tierLabel}\n${runs}`, context);
    assert.equal(runInContext("runs(settings, me, models)", context), c.display);
    const title = { cheap: "Cheap", medium: "Medium" }[c.tier];
    assert.ok(title, "fixture describes a currently enabled tier");
    assert.equal(runInContext("tierName(settings.tier)", context), `${title} (${models[c.tier]})`);
  });
}
test("without an owner's agent view the page does not claim a running model", () => {
  const context = createContext({ models: fixtures.models });
  runInContext(runs, context);
  assert.equal(runInContext('runs({model: {provider: "openrouter", id: "fixture/own:1"}}, null, models)', context), "");
  assert.equal(runInContext('runs({tier: "medium"}, {}, null)', context), "");
});
