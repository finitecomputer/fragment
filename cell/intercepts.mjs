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

// Each of `hosts` caught by the swap, on HTTPS and on plain HTTP alike:
// the swap always sends on over HTTPS, so a credential never crosses the
// internet in the clear.
function swapped(c, egress, hosts) {
  return hosts.flatMap((host) => [() => c.interceptOutboundHttp(host, egress("swap")), () => c.interceptOutboundHttps(host, egress("swap"))]);
}

// Registers every egress intercept of a computer's guest on container `c`,
// all at once (one round trip, not one each): the platform's hosts, a
// local fleet's code.storage (`codestorage`: at a loopback address the
// container cannot reach), each of `swapHosts` (`swapped`), each
// `ComputerEgress` with its route as `egress(route)` makes it. None
// survives a stop: each start registers them again.
export function intercept(c, egress, swapHosts, codestorage) {
  const each = [
    () => c.interceptOutboundHttp("api.fragment.internal", egress("api")),
    () => c.interceptOutboundHttp("model.fragment.internal", egress("model")),
  ];
  if (codestorage) each.push(() => c.interceptOutboundHttp("codestorage.fragment.internal", egress("codestorage")));
  each.push(...swapped(c, egress, swapHosts));
  return landed(each);
}

// `hosts` more for the swap of a running container (a Vault secret's, set
// since it started or named by its guest's first read of its view): these
// alone, at once, never the rest again.
export function interceptSwap(c, egress, hosts) {
  return landed(swapped(c, egress, hosts));
}

// Each of `each` (a call that registers one intercept) at once, answered
// once every one has: one that fails fails it (the first's reason), never
// while another is still on its way to the runtime, so nothing of a failed
// start's lands after it.
async function landed(each) {
  const answers = await Promise.allSettled(each.map((f) => settled(f)));
  const failed = answers.find((a) => a.status === "rejected");
  if (failed) throw failed.reason;
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
export async function startIntercepted(c, opts, intercepted, overtaken) {
  try {
    await intercepted();
  } catch (e) {
    if (overtaken()) return false;
    throw e;
  }
  if (overtaken()) return false;
  c.start(opts);
  return true;
}
