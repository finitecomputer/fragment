// secrets.mjs: the store's bindings stood in for where the runtime has
// none (docs/self-host.md, seam 12). Valid (each binding named answers
// its value through `get()`, as the store's does, the rest of the env as it
// came), invalid (each wrong config refused, naming why), and none at all
// (no FRAGMENT_SECRETS: the env itself, as on Cloudflare).
import { test } from "node:test";
import assert from "node:assert/strict";
import { config, withSecrets } from "./secrets.mjs";

const SECRETS = [
  { binding: "HOST_SECRET", secret_name: "fragment-host-secret" },
  { binding: "CODESTORAGE_KEY", secret_name: "fragment-codestorage-key" },
];

test("without FRAGMENT_SECRETS the env is the runtime's own", () => {
  const env = { HOST_SECRET: { get: async () => "from the store" } };
  assert.equal(withSecrets(env), env);
  assert.equal(withSecrets({ ...env, FRAGMENT_SECRETS: "" }).HOST_SECRET, env.HOST_SECRET);
  assert.equal(withSecrets(undefined), undefined);
});

test("vars: each binding answers the variable of its name through get(), the rest as it came", async () => {
  const env = { FRAGMENT_SECRETS: JSON.stringify({ backend: "vars", secrets: SECRETS }), HOST_SECRET: "h", CODESTORAGE_KEY: "", OTHER: 7 };
  const out = withSecrets(env);
  assert.equal(out.HOST_SECRET.constructor.name, "Fetcher", "named as workerd names the store's (workers-rs reads it by that name)");
  assert.equal(await out.HOST_SECRET.get(), "h");
  assert.equal(await out.CODESTORAGE_KEY.get(), null, "an empty value is none, as the store's missing one is");
  assert.equal(out.OTHER, 7);
  assert.equal(withSecrets(env), out, "one stand-in env per env");
});

test("a wrong config is refused, saying why", () => {
  const refused = (cfg, says) => assert.throws(() => config(typeof cfg === "string" ? cfg : JSON.stringify(cfg)), (e) => e.message.includes(says), says);
  refused("{not json", "is JSON");
  refused({ backend: "nope", secrets: [] }, "backend is one of vars");
  refused({ backend: "vars", secrets: [], addr: "x" }, "not addr");
  refused({ backend: "vars", secrets: {} }, "a list of at most 64");
  refused({ backend: "vars", secrets: Array.from({ length: 65 }, (_, i) => ({ binding: `B${i}`, secret_name: `s${i}` })) }, "a list of at most 64");
  refused({ backend: "vars", secrets: [{ binding: "lower", secret_name: "a" }] }, "names each secret's binding");
  refused({ backend: "vars", secrets: [{ binding: "A", secret_name: "has space" }] }, "names each secret's binding");
  refused({ backend: "vars", secrets: [SECRETS[0], SECRETS[0]] }, "binds HOST_SECRET twice");
  assert.deepEqual(config(JSON.stringify({ backend: "vars", secrets: SECRETS })).secrets, SECRETS);
});
