// intercepts.mjs: a computer's start registers every egress intercept of
// its guest first, all at once, and only then starts it, so the guest's
// first call, made at once as it boots, is answered and never tried again.
// Valid (that first call answered; every intercept in place at the start,
// asked at once, the swap's hosts on both schemes), invalid (a registration
// that fails starts nothing, and fails only once its siblings have landed),
// the fence (a start overtaken while it registered starts nothing), the
// settle (a registration a snapshot's start finds "temporarily
// unavailable" is asked again), and a running container's Vault hosts
// (those alone, never the platform's again).
import { test } from "node:test";
import assert from "node:assert/strict";
import { intercept, interceptSwap, startIntercepted } from "./intercepts.mjs";

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
  const seen = { caught: new Set(), asked: [], inFlight: 0, inFlightMax: 0, started: 0, first: null, atStart: null };
  const register = (scheme) => async (host, binding) => {
    const name = `${scheme}://${host} → ${binding.route}`;
    seen.asked.push(name);
    if (name === fail) {
      // refused at once, while the others are still on their way
      throw new Error("the runtime refused it");
    }
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
      seen.atStart = new Set(seen.caught);
      seen.first = [API, MODEL].map((host) => [host, seen.caught.has(`http://${host}`) ? "answered" : "failed: tried again after a backoff"]);
    },
  };
  return { c, seen };
}

const egress = (route) => ({ route });
const SWAP = ["api.example.com", "*.example.net"];

// What a computer's start asks of the host: its intercepts, then the start.
function start(c, overtaken = () => false) {
  return startIntercepted(c, { image: "stub" }, () => intercept(c, egress, SWAP, true), overtaken);
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

test("every intercept is in place at the start itself, all asked at once", async () => {
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
  ];
  assert.deepEqual([...seen.asked].sort(), [...every].sort());
  // what had landed when the container started, not after
  assert.deepEqual([...seen.atStart].sort(), every.map((e) => e.split(" → ")[0]).sort());
  // one round trip for them all, not one each
  assert.equal(seen.inFlightMax, every.length);
});

test("without a code.storage of its own or a swap host, the platform's two; never the backups' host, whose saves and restores grant their own", async () => {
  const { c, seen } = container();
  await startIntercepted(c, {}, () => intercept(c, egress, [], false), () => false);
  assert.deepEqual(seen.asked, [`http://${API} → api`, `http://${MODEL} → model`]);
  assert.equal(seen.started, 1);
});

test("a registration the runtime refuses starts nothing, and fails only once the others have landed", async () => {
  const { c, seen } = container({ fail: "https://api.example.com → swap" });
  await assert.rejects(start(c), /the runtime refused it/);
  assert.equal(seen.started, 0);
  // none of the failed start's still on its way, to land after a later start's
  assert.equal(seen.inFlight, 0);
  assert.equal(seen.caught.size, 6);
});

test("a start overtaken while it registered (a wipe's destroy, a later start) starts nothing", async () => {
  const { c, seen } = container();
  let destroyed = false;
  const registering = start(c, () => destroyed);
  destroyed = true;
  assert.equal(await registering, false);
  assert.equal(seen.started, 0);
  assert.equal(seen.caught.size, 7, "what it registered stays: the next start registers the same again");
});

test("a start overtaken while it registered answers overtaken, not the failure of a registration refused meanwhile", async () => {
  const { c, seen } = container({ fail: "https://api.example.com → swap" });
  let slept = false;
  const registering = start(c, () => slept);
  // a sleep's destroy, while the registrations are on their way
  slept = true;
  assert.equal(await registering, false);
  assert.equal(seen.started, 0);
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

test("a Vault host its guest's view names, running, is registered alone: never the platform's again", async () => {
  const { c, seen } = container();
  await start(c);
  seen.asked.length = 0;
  await interceptSwap(c, egress, ["api.vault.test"]);
  assert.deepEqual(seen.asked, ["http://api.vault.test → swap", "https://api.vault.test → swap"]);
  assert.equal(seen.started, 1, "it starts nothing");
});

// A running container with Cloudflare's rules for its 128 entries (#1427):
// each target (a scheme and a host) two of them, one registered again none
// more, none given back; past them, its own words. `held`: its targets.
const LIMIT = "You can't configure more than 128 egress interceptors";
function full(held) {
  const targets = new Set(held);
  const register = (scheme) => async (host) => {
    const target = `${scheme}://${host}`;
    if (!targets.has(target) && 2 * (targets.size + 1) > 128) throw new Error(LIMIT);
    targets.add(target);
  };
  return { targets, interceptOutboundHttp: register("http"), interceptOutboundHttps: register("https") };
}
const hosts = (prefix, n) => Array.from({ length: n }, (_, i) => `${prefix}${String(i).padStart(2, "0")}.example.test`);
const both = (list) => list.flatMap((h) => [`http://${h}`, `https://${h}`]);
const PLATFORM = [`http://${API}`, `http://${MODEL}`];

test("a container taken over holding hosts set before: one it has no room for is left and answered, never a failed arm", async () => {
  // the platform's two, 30 swap hosts and one stray: 126 of its 128
  const c = full([...PLATFORM, ...both(hosts("old", 30)), "http://stray.example.test"]);
  assert.deepEqual(await intercept(c, egress, [...hosts("old", 30), "new.example.test"], false), ["new.example.test"]);
  assert.equal(c.targets.size * 2, 128);
});

test("two Vault hosts racing for a running container's last room: both answer, the one left named", async () => {
  // 124 of its 128: room for one host on both schemes
  const c = full([...PLATFORM, ...both(hosts("held", 30))]);
  const answers = await Promise.all([interceptSwap(c, egress, ["za.example.test"]), interceptSwap(c, egress, ["zb.example.test"])]);
  assert.deepEqual(answers, [[], ["zb.example.test"]]);
});

test("no room for the platform's own still fails, and so does any other refusal of a swap host", async () => {
  await assert.rejects(intercept(full(both(hosts("held", 32))), egress, [], false), { message: LIMIT });
  const { c } = container({ fail: "https://api.example.com → swap" });
  await assert.rejects(intercept(c, egress, SWAP, false), /refused/);
});

test("a start whose container has no room for a swap host it asks starts nothing", async () => {
  // a budget gone wrong: 32 swap hosts beside the platform's two, 132 entries
  const c = { ...full([]), started: 0, start() { this.started++; } };
  await assert.rejects(startIntercepted(c, { image: "stub" }, () => intercept(c, egress, hosts("s", 32), false), () => false), /no room for 1 of its swap hosts/);
  assert.equal(c.started, 0);
});
