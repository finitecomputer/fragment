// A computer's egress intercepts and its start behind them (docs/computers.md,
// "Environment"), apart from entry.mjs, which imports what only workerd
// has, so node's runner tries them against a stand-in container
// (intercepts.test.mjs).

// A start from a snapshot answers "temporarily unavailable" for a while
// (spike S3b): a call that does is tried again, every 100 ms for up to two
// minutes.
const SETTLE_MS = 120_000;
const SETTLE_PAUSE_MS = 100;

export async function settled(f) {
  const t0 = Date.now();
  // bounded: past SETTLE_MS the failure is the answer
  for (;;) {
    try {
      return await f();
    } catch (e) {
      if (!/temporarily unavailable/.test(String(e)) || Date.now() - t0 > SETTLE_MS) throw e;
      await new Promise((r) => setTimeout(r, SETTLE_PAUSE_MS));
    }
  }
}

// A container's 128 entries spent (Cloudflare's words for it): the
// intercepts registered before keep working, and none is ever given back
// while it runs (fragment_core::computer::egress).
const FULL = /more than 128 egress interceptors/;

// Each of `hosts` caught by the swap, on HTTPS and on plain HTTP alike:
// the swap always sends on over HTTPS, so a credential never crosses the
// internet in the clear. Each registration names its host: one the
// container has no room for is left (`landed`).
function swapped(c, egress, hosts) {
  return hosts.flatMap((host) => [
    { host, f: () => c.interceptOutboundHttp(host, egress("swap")) },
    { host, f: () => c.interceptOutboundHttps(host, egress("swap")) },
  ]);
}

// Registers every egress intercept of a computer's guest on container `c`,
// all at once (one round trip, not one each): the platform's hosts, a
// local fleet's code.storage (`codestorage`: at a loopback address the
// container cannot reach), each of `swapHosts` (`swapped`), each
// `ComputerEgress` with its route as `egress(route)` makes it. None
// survives a stop: each start registers them again. Answers the swap
// hosts left for want of room (`landed`). `live()`: whether the start
// these are for is still the running one (`landed`). `reserve`, a start's
// alone: the backups' host's registration, asked with the platform's,
// before any swap host (entry.mjs `#intercept`).
export function intercept(c, egress, swapHosts, codestorage, live, reserve) {
  const each = [
    { f: () => c.interceptOutboundHttp("api.fragment.internal", egress("api")) },
    { f: () => c.interceptOutboundHttp("model.fragment.internal", egress("model")) },
  ];
  if (codestorage) each.push({ f: () => c.interceptOutboundHttp("codestorage.fragment.internal", egress("codestorage")) });
  if (reserve) each.push({ f: reserve });
  each.push(...swapped(c, egress, swapHosts));
  return landed(each, live);
}

// `hosts` more for the swap of a running container (a Vault secret's, set
// since it started or named by its guest's first read of its view): these
// alone, at once, never the rest again.
export function interceptSwap(c, egress, hosts, live) {
  return landed(swapped(c, egress, hosts), live);
}

// A registration asked once its start is no longer the running one (`live`):
// never sent, since the container may be another start's by then.
const SUPERSEDED = "its start is no longer the running one";

// Each of `each` (a call `f` that registers one intercept, of a swap
// `host` or the platform's) at once, answered once every one has: one that
// fails fails it (the first's reason), never while another is still on its
// way to the runtime, so nothing of a failed start's lands after it. A swap
// host the container has no room for (`FULL`: a container taken over may
// hold hosts set before; two arms may race for the last room) is no
// failure: it goes uncaught, and the hosts so left, each once, are the
// answer. The platform's own never are. Each is sent, and asked again
// (`settled`), only while `live()`: a try asked again once its start has
// gone would land on the container of the start after it, there spending
// the room its own arms counted (Sol on #1427), so it fails instead.
async function landed(each, live = () => true) {
  const sent = (f) => () => {
    if (!live()) throw new Error(SUPERSEDED);
    return f();
  };
  const answers = await Promise.allSettled(each.map((e) => settled(sent(e.f))));
  const full = new Set();
  for (const [i, a] of answers.entries()) {
    if (a.status === "fulfilled") continue;
    if (each[i].host && FULL.test(String(a.reason))) full.add(each[i].host);
    else throw a.reason;
  }
  return [...full];
}

// Starts container `c` (`opts`) once `intercepted()` has registered its
// guest's intercepts (`intercept`), never before: the guest's first call,
// made at once as it boots, finds its host caught, where a start that
// registered them after raced it, and a call that lost waited out its
// retry. Cloudflare's own word: "Call it before or after starting the
// container". A registration that fails starts nothing. Nor does a start
// `overtaken()` while they registered (a start after it, or a destroy of
// whatever runs: a sleep's, a wipe's), answering false, whether or not one
// failed meanwhile: an overtaken start's failure is no one's to report.
// A swap host `intercepted()` found no room for (`landed`: a container
// holds registrations it was asked before this start, an overtaken
// start's) is logged, and the start goes on with it uncaught: a start
// that failed before starting would keep them all, and fail so again.
export async function startIntercepted(c, opts, intercepted, overtaken) {
  try {
    const full = await intercepted();
    if (full && full.length) console.error(JSON.stringify({ arm: "start", left_out: full.length }));
  } catch (e) {
    if (overtaken()) return false;
    throw e;
  }
  if (overtaken()) return false;
  c.start(opts);
  return true;
}
