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
// hosts left for want of room (`landed`).
export function intercept(c, egress, swapHosts, codestorage) {
  const each = [
    { f: () => c.interceptOutboundHttp("api.fragment.internal", egress("api")) },
    { f: () => c.interceptOutboundHttp("model.fragment.internal", egress("model")) },
  ];
  if (codestorage) each.push({ f: () => c.interceptOutboundHttp("codestorage.fragment.internal", egress("codestorage")) });
  each.push(...swapped(c, egress, swapHosts));
  return landed(each);
}

// `hosts` more for the swap of a running container (a Vault secret's, set
// since it started or named by its guest's first read of its view): these
// alone, at once, never the rest again.
export function interceptSwap(c, egress, hosts) {
  return landed(swapped(c, egress, hosts));
}

// Each of `each` (a call `f` that registers one intercept, of a swap
// `host` or the platform's) at once, answered once every one has: one that
// fails fails it (the first's reason), never while another is still on its
// way to the runtime, so nothing of a failed start's lands after it. A swap
// host the container has no room for (`FULL`: a container taken over may
// hold hosts set before; two arms may race for the last room) is no
// failure: it goes uncaught, and the hosts so left, each once, are the
// answer. The platform's own never are.
async function landed(each) {
  const answers = await Promise.allSettled(each.map((e) => settled(e.f)));
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
// Nor does one whose container had no room for a swap host `intercepted()`
// answers (`landed`): a container not yet started holds none of its own,
// and the hosts a start asks fit its entries (`egress::armed`), so one
// left is a budget gone wrong, never a guest started without it.
export async function startIntercepted(c, opts, intercepted, overtaken) {
  try {
    const full = await intercepted();
    if (full && full.length) throw new Error(`its container had no room for ${full.length} of its swap hosts`);
  } catch (e) {
    if (overtaken()) return false;
    throw e;
  }
  if (overtaken()) return false;
  c.start(opts);
  return true;
}
