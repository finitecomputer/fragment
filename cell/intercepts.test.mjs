// intercepts.mjs: a computer's start registers every egress intercept of
// its guest first, all at once, and only then starts it, so the guest's
// first call, made at once as it boots, is answered and never tried again.
// Valid (that first call answered; every intercept asked at once, the
// swap's hosts on both schemes), invalid (a registration that fails starts
// nothing), the fence (a start overtaken while it registered starts
// nothing), and the settle (a registration a snapshot's start finds
// "temporarily unavailable" is asked again).
import { test } from "node:test";
import assert from "node:assert/strict";
import { intercept, startIntercepted } from "./intercepts.mjs";

const API = "api.fragment.internal";
const MODEL = "model.fragment.internal";

// A stand-in container. Each intercept lands a moment after it is asked,
// as a runtime's round trip does, and those in flight together are
// counted. Its start boots a guest whose first call goes out at once, as
// the bridge's read of its view does: to `API`, then `MODEL`. A call is
// answered when its host's intercept has landed by then; otherwise it
// fails, and the guest waits out its retry (the bridge's backoff, 0.5 to
// 1.5 s). `fail` names a registration the runtime refuses, `busy` one it
// answers "temporarily unavailable" once.
function container({ fail = null, busy = null } = {}) {
  const seen = { caught: new Set(), asked: [], inFlight: 0, inFlightMax: 0, started: 0, first: null };
  const register = (scheme) => async (host, binding) => {
    const name = `${scheme}://${host} → ${binding.route}`;
    seen.asked.push(name);
    if (name === fail) throw new Error("the runtime refused it");
    if (name === busy) {
      busy = null;
      throw new Error("the container is temporarily unavailable");
    }
    seen.inFlight++;
    seen.inFlightMax = Math.max(seen.inFlightMax, seen.inFlight);
    await new Promise((r) => setTimeout(r, 5));
    seen.inFlight--;
    seen.caught.add(`${scheme}://${host}`);
  };
  const c = {
    interceptOutboundHttp: register("http"),
    interceptOutboundHttps: register("https"),
    start(opts) {
      seen.started++;
      seen.opts = opts;
      seen.first = [API, MODEL].map((host) => [host, seen.caught.has(`http://${host}`) ? "answered" : "failed: tried again after a backoff"]);
    },
  };
  return { c, seen };
}

const egress = (route) => ({ route });
const backups = (c) => () => c.interceptOutboundHttp("backups.sandbox.internal", { route: "backups" });
const SWAP = ["api.example.com", "*.example.net"];

// What a computer's start asks of the host: its intercepts, then the start.
function start(c, overtaken = () => false) {
  return startIntercepted(c, { image: "stub" }, () => intercept(c, egress, SWAP, true, [backups(c)]), overtaken);
}

test("the guest's first call, made at once as it boots, is answered: never tried again", async () => {
  const { c, seen } = container();
  assert.equal(await start(c), true);
  assert.equal(seen.started, 1);
  assert.deepEqual(seen.opts, { image: "stub" });
  assert.deepEqual(seen.first, [
    [API, "answered"],
    [MODEL, "answered"],
  ]);
});

test("every intercept is in place before the start, all asked at once", async () => {
  const { c, seen } = container();
  await start(c);
  const every = [
    `http://${API} → api`,
    `http://${MODEL} → model`,
    "http://codestorage.fragment.internal → codestorage",
    "http://api.example.com → swap",
    "https://api.example.com → swap",
    "http://*.example.net → swap",
    "https://*.example.net → swap",
    "http://backups.sandbox.internal → backups",
  ];
  assert.deepEqual([...seen.asked].sort(), [...every].sort());
  assert.equal(seen.caught.size, every.length);
  // one round trip for them all, not one each
  assert.equal(seen.inFlightMax, every.length);
});

test("without a code.storage of its own or a swap host, the platform's two and the backups'", async () => {
  const { c, seen } = container();
  await startIntercepted(c, {}, () => intercept(c, egress, [], false, [backups(c)]), () => false);
  assert.deepEqual(seen.asked, [`http://${API} → api`, `http://${MODEL} → model`, "http://backups.sandbox.internal → backups"]);
  assert.equal(seen.started, 1);
});

test("a registration the runtime refuses starts nothing: no guest without its intercepts", async () => {
  const { c, seen } = container({ fail: "https://api.example.com → swap" });
  await assert.rejects(start(c), /the runtime refused it/);
  assert.equal(seen.started, 0);
});

test("a start overtaken while it registered (a wipe's destroy, a later start) starts nothing", async () => {
  const { c, seen } = container();
  let destroyed = false;
  const registering = start(c, () => destroyed);
  destroyed = true;
  assert.equal(await registering, false);
  assert.equal(seen.started, 0);
  assert.equal(seen.caught.size, 8, "what it registered stays: the next start registers the same again");
});

test("a registration answered temporarily unavailable is asked again, and the start waits for it", async () => {
  const { c, seen } = container({ busy: `http://${MODEL} → model` });
  assert.equal(await start(c), true);
  assert.equal(seen.asked.filter((a) => a === `http://${MODEL} → model`).length, 2);
  assert.deepEqual(seen.first, [
    [API, "answered"],
    [MODEL, "answered"],
  ]);
});
