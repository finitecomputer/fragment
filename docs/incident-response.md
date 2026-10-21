# Incident evidence for the Cloudflare deployment

Use this to locate a failure and hand over a small, reviewed report. These
are reads against an explicitly selected deployment, using access the
operator already has. They do not deploy, restart computers, retry jobs,
change logging settings, or create credentials. If access is refused, keep
the status and stop that read; do not switch accounts or broaden access.

## Establish the release and scope

Record the UTC start/end, preview versus production, affected surface, and
whether one person/fragment or several are affected. Use aliases such as
`person-A` and `chat-A` in a shared report. Separate observed facts from
suspected causes. Start with the original failure; do not reproduce a paid
model call or repeat a write merely to obtain logs.

In Bash, set the intended platform origin explicitly (no trailing slash).
The reserved example address below must be replaced; it is not a deployment.

```bash
set -o pipefail
FRAGMENT_INCIDENT_HOST='https://platform.example.test'
date -u '+%Y-%m-%dT%H:%M:%SZ'
curl --silent --show-error --fail --max-time 15 --head \
  "$FRAGMENT_INCIDENT_HOST/healthz" |
  awk 'tolower($0) ~ /^(http\/|x-fragment-deploy:|cf-ray:|retry-after:)/'
```

Expected: HTTP 200 and `x-fragment-deploy` identifying the served release.
Compare that value with the deployment's successful CI run, not merely the
latest branch commit. A missing header, unexpected release, redirect, HTTP
failure or timeout is evidence to retain. Do not follow redirects or disable
TLS verification to make this check pass. `cf-ray`, if present, is an edge
reference, not a guaranteed application-wide correlation ID.

`/healthz` only proves that this route answers; it does not exercise a
fragment's storage, a computer, sign-in, billing, or a model provider.
The implementation is in [cell/src/lib.rs](../cell/src/lib.rs), with release
compatibility described in [MODEL.md](MODEL.md).

## Read the affected fragment without collecting its contents

Use an already paired `fragment` CLI and `jq`. Set the actual fragment name
locally; replace it with an alias in the report. These projections omit
status tokens, repository URLs, titles, event summaries/data and run
input/output/error text. Do not attach the unfiltered responses or enable
`--verbose`. If a command fails, record its exit status and inspect the
failure locally before sharing anything.

```bash
FRAGMENT_INCIDENT_NAME='replace-with-the-affected-fragment'
fragment --host "$FRAGMENT_INCIDENT_HOST" --json status "$FRAGMENT_INCIDENT_NAME" |
  jq -e 'if .ok != true then error("status read failed") else
    .data | {kind, role, pins, counts, code: {id: .code.id}} end'
fragment --host "$FRAGMENT_INCIDENT_HOST" --json events "$FRAGMENT_INCIDENT_NAME" --tail 20 |
  jq -e 'if .ok != true then error("events read failed") else
    {events: [.data.events[] | {id, at, kind}]} end'
fragment --host "$FRAGMENT_INCIDENT_HOST" --json runs "$FRAGMENT_INCIDENT_NAME" --limit 20 |
  jq -e 'if .ok != true then error("runs read failed") else
    {runs: [.data.runs[] | {id, status, attempt, via}]} end'
```

These are bounded snapshots, not a complete history. A missing run is not
proof that an action was never accepted. A `held` or `blocked` run calls for
reading the existing failure privately, not automatically replaying it.
For a computer failure, copy the already displayed phase and short image
version from Settings; preserve the time and release alongside them.
The read contracts are in [api.md](api.md); CLI output is implemented in
[cli/src/main.rs](../cli/src/main.rs).

## Match existing telemetry to the symptom

Use the deployment's existing Cloudflare log access and the incident's time
window. Do not enable a new log destination, increase retention or turn on
provider prompt logging. These fields already exist:

| Symptom | Existing evidence | What it can establish |
| --- | --- | --- |
| Message post is slow or repeated | `post`: `at`, `ms`, `waits`, `channel`, `seq`, `replayed` | Where I/O waited and whether that post was replayed; not CPU time or a complete trace |
| Model is slow or falls back | `model.settled`: `first_ms`, `ms`, `calls`, `tier`; `model.fallback`: `status`, `ms` | First-data/total timing, tries and fallback; inspect `ref`/`logId` privately for correlation |
| Model credential or settlement fails | `model.platform-key`, `model.settle-failed`, `model.release-failed` | Which stage failed; an error message may include upstream detail and needs manual redaction |
| Computer will not wake or save | Computer lifecycle `event`, `phase`, `actions`; `wake`, `snapshot`, `generation` where present | The recorded transition and outcome; computer IDs, image references and failure text stay private |
| OAuth connection did not finish | `connection-sign-in` with `unfinished`, or `own-key-sign-in` with `no-key` | The recorded failure category, not proof of a completed grant |

Sources: [js.rs Laps](../cell/src/js.rs), [channels.rs](../cell/src/channels.rs),
[models.rs](../cell/src/models.rs), [computer.rs](../cell/src/computer.rs) and
[connections.rs](../cell/src/connections.rs). Fields are not universal:
computer `event` is structured lifecycle data, while model `event` is a
string. `Laps` measures waits on the Workers clock, not CPU duration.

For an operator who already has a prepared checkout and Cloudflare access,
the existing Wrangler can tail model events. Set paths to that checkout's
pinned Node and installed Wrangler (see [the pinned Node](../README.md#the-pinned-node));
do not download an unpinned CLI during the incident. Set the deployed Worker
name explicitly. Stop with Ctrl-C after the observation window; this command
does not reproduce a model call.

```bash
FRAGMENT_INCIDENT_NODE='/absolute/path/to/pinned/node'
FRAGMENT_INCIDENT_WRANGLER='/absolute/path/to/node_modules/wrangler/bin/wrangler.js'
FRAGMENT_INCIDENT_WORKER='replace-with-the-deployed-worker-name'
"$FRAGMENT_INCIDENT_NODE" "$FRAGMENT_INCIDENT_WRANGLER" tail "$FRAGMENT_INCIDENT_WORKER" \
  --format json --search 'model.' |
  jq --unbuffered -c '.logs[]?.message[]? | fromjson? | select(type == "object") |
    select(.event == "model.settled" or .event == "model.fallback" or .event == "model.platform-key") |
    {event, first_ms, ms, calls, tier, status}'
```

The projection deliberately omits raw request headers, URLs, log references,
model names and error text. It does not include every error category in the
table; investigate those privately through the existing log view. An empty
tail does not establish health: sampling, timing or missing log access can
hide events. Tail options are documented by
[Cloudflare](https://developers.cloudflare.com/workers/wrangler/commands/workers/#tail).

## Support handoff

Review even the projected output before sharing. Replace person, fragment,
computer and organization identifiers with consistent aliases unless an
authorized private recipient needs the real identifier. Never attach a raw
HAR, cookie, authorization header, key file, environment dump, deploy config,
OAuth URL/code, signed download URL, prompt, reply, attachment, or full run
record. Screenshots should be cropped and checked for account names and chat
contents. No support bundle or export is required by this recipe.

```text
Environment: preview/production; private deployment alias
UTC interval:
Served release and expected deployment/CI link:
Impact: surface; number affected; aliases
Observed action → expected result → actual result:
HTTP status/error code; edge reference if present:
Selected event/run IDs, phase, timing and image version (when relevant):
What changed recently, and what is only a hypothesis:
Checks completed, failures/refusals, and checks not run:
Next owner and next read-only check:
```

A report is evidence for the next decision, not permission to restart,
replay, roll back, deploy, change credentials or raise a limit. Record the
outcome of any separately authorized mitigation against the same interval
and release; a successful health check alone is not recovery verification.
