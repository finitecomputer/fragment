# SimpleX: what it adds to the platform (the complexity ledger)

Paul (2026-10-22): "Try to keep track of ways this is complicating our
platform so I can carefully audit those before I merge." Every way the
SimpleX stack (design B: the bridge drives `simplex-chat`, and SimpleX is
another door into the agent's own chat with its owner) complicates the
platform is listed here. Each entry says:

- what it is, and where it lives;
- why it is needed;
- what it would take to remove it;
- its blast radius if it misbehaves.

**Every computer** marks what touches every computer, not only those of
people who linked SimpleX. The aim is that nothing does, or what does is
inert (a path that runs only with a link). Each pull request of the stack
adds its entries, and its body summarizes them.

The stack, in merge order:

1. `claude/simplex` (#523): the link's core, pure, and the lab.
2. `claude/simplex-process`: the bridge runs each linked agent's core;
   the image carries `simplex-chat`.
3. `claude/simplex-messages`: the relayed record, the engine's rule,
   replies back, "via SimpleX".
4. `claude/simplex-page`: the link page, its QR code, the shell's entry.
5. `claude/simplex-e2e`: the real Hermes image end to end in the Docker
   lane.

## 1. The link's core (#523)

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| `images/bridge/src/simplex/{wire,link}.rs`: the bot API typed, and one link's rules, pure | Every later piece drives the core through them | Delete the module (nothing else imports it here) | Nothing runs it in this PR | No: code in the bridge's library, run by nothing |
| `images/bridge/src/simplex/fixtures/*.json`: 12 frames the real core sent (about 35 KB) | Unit tests read the core's real shapes offline | Delete with the tests | A shape the core changes in a later release fails the tests, which is their job | No |
| The lab: `images/bridge/tests/simplex.rs` and `tests/simplex/lab.Dockerfile`, `#[ignore]`, run by hand | Proves the rules against the real core (link, rollback, voice) | Delete the file | It pulls two pinned images and a 79.5 MB binary, and builds one image, only when run by hand. CI's `cargo test --workspace` skips it, and the Docker lane runs `docker.rs` alone | No |
| **Third party: `simplex-chat` v7.0.3**, the release's Ubuntu 24.04 x86_64 binary, pinned by SHA-256 (`895fb14c…`) | It is SimpleX's own core: no maintained library implements the protocol (docs/explorations/simplex-connector.md, question 1) | A pure-Workers client (1.5–2 months, the spike's estimate) | A bug in it breaks SimpleX for linked agents only. Upgrading it is a pin and an image round, and its database migrates forward (`-y`), so an image rolled back after a bump may not open it (not yet tested) | Lab only in this PR (the image carries it from PR 2) |
| **Third party: SimpleX's preset SMP and XFTP servers** (SimpleX Chat's and Flux's), the core's defaults | No inbound TCP on Containers, so no router of ours on Cloudflare; Paul's v1 default | Run our own SMP/XFTP servers elsewhere, named with `-s`/`--xftp-server` | Their outage stops SimpleX messages, queued until they return. No account, no secret | No (from PR 2: only cores of linked agents talk to them) |

## 2. The bridge runs each linked agent's core (`claude/simplex-process`)

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **A new process per linked agent**: `simplex-chat`, a child of the bridge (`simplex/run.rs`), supervised by one manager task. A core that exits is restarted after 2 s, doubling; after 3 exits in 10 minutes it waits until its owner links again | The core is SimpleX's protocol; only the bridge drives it, so nothing else steals its events (finite-mono's warning) | Delete `simplex/run.rs` and unset `BRIDGE_SIMPLEX_BIN` | About 30–35 MB of RAM and 0% CPU per idle core. There is no cap: 16 linked agents would run 16 cores (about 0.5 GB). A crash loop is bounded at 3 starts per 10 minutes | No. It runs only with all of: the image names the core, the computer is always on, and that agent is linked |
| **The manager task**, started whenever the image names the core | It owns the cores, follows the hold, the gate and the links | As above | Idle, it stats `/run/computer/hold` and loops over its slots every 100 ms | **Yes, on every Hermes computer**, idle: the image names the core on all of them. The stub names none |
| **The hold now waits for the cores** (`answer_holds`' `cores_quiet`): every core is stopped (SIGTERM, then SIGKILL past 3 s) before the bridge answers, and none starts while the hold lasts | A save must take the core's databases whole; `/_start` after `/_stop` crashes v7.0.3, so it is the process that stops | Drop the flag (it is always true with no core) | It adds the cores' stop to the hold's answer, all in parallel: 170 ms measured, at most about 4 s, inside the platform's 20 s. A core that keeps running past its kill would hold the answer, and the platform then saves unheld (the core's databases taken hot) | **Code path on every computer**, but `cores_quiet` is true whenever no core runs, so behavior only changes on computers with a core |
| **hermes-boot's hold copy passes `/data/simplex` over** (`held::SIMPLEX`, both of main.rs's walks) | The cores are stopped when the copy runs, so the raw save is exact. A copy as the hermes user could not open a WAL database it cannot write beside | Drop the skip | None: a skip of a path that exists only with SimpleX | **Code path on every Hermes computer**, inert |
| **New files on `/data`**: `/data/simplex/<agent>/`, holding `sx_chat.db`, `sx_agent.db` (about 1.7 MB once linked), `files/`, `tmp/` and `link.json` | The core's state, and the link's | Delete the directory (an unlink keeps it; a person's wipe deletes it with the computer) | Saved raw, with the core stopped. A rollback takes both back together; the start's forced sync heals the link. A `link.json` that does not read, or is another agent's, is **set aside** (renamed, kept) and the link starts off | No |
| **The forced resync and its edges** (`link.rs`, `run::rolled_back`) | The core cannot see a rollback; a forced sync at a rollback start heals it (measured: about 0.5 s) | None | (a) A platform that sends no `restored` never forces: a silent break. (b) A pending invitation the lost life spent is made anew, and the owner sees a new QR. (c) **A rollback past the linking itself** turns the link off while the person's app keeps a dead contact: their messages go nowhere, unanswered, until the owner links again. Linking asks for a save at once to shrink that window to seconds. (d) A sync forced when nothing was out of step costs about 0.5 s. (e) A message the person sends during the sync may be lost; they are told it may be | No |
| **A save asked when a link changes**: a second keepalive socket, held for 3 s | So linking survives a crash right after it | Drop the pulse; the next turn's save covers it within 15 minutes of that turn | One extra keepalive open of 3 s per link or unlink | No |
| **The bridge reads three more fields of its view**: `alwaysOn`, `generation`, `restored` (`api::Computer`) | The tier gate and the rollback flag | Drop them | None: the platform already sends them, and an old one's absence reads as not always on and no rollback | No wire change |
| **Routes on the image's screen port**: `GET /simplex/state`, `POST /simplex/link`, `POST /simplex/unlink` (`simplex/http.rs`) | How the owner links, from the shell (PR 4 adds the page) | Delete `http.rs` | 404 on an image without SimpleX. **Security, for Paul:** the platform proxies the port to the owner alone, but any process in the container reaches it too, the agent's own terminal included. A prompt-injected agent could link a stranger's SimpleX, who would then speak as the owner, a lasting channel. That is the same trust boundary as root in the container (decisions 43–44: the agent can already open any channel out). Closing it would take a platform-signed header on proxied port requests, a cell change | **Code path on every computer** (the screen server checks the path first); answers 404 unless the image names the core |
| **The tier gate**: a core runs only while the view's `alwaysOn` is true | Paul: SimpleX for the $200 tier, which requires always on | Drop the check | A plan change reaches a computer within 60 s (its view is read every minute). Not always on, the link page says why | No |
| **A seat lapses** | Billing turns `alwaysOn` off (docs/billing.md) | — | Its cores stop within 60 s, and the computer idles to sleep. Links and identities are kept. Messages wait on the SMP servers, for as long as each keeps them (not checked). Paying again turns always on back on: the cores start and the queued messages arrive | No |
| **The image**: `simplex-chat` in `images/hermes/Dockerfile`, a layer of its own between the base and ours (79.5 MB, 18.8 MB compressed), and `BRIDGE_SIMPLEX_BIN` from `hermes-boot` | The core | Delete the layer and the env | Every Hermes computer pulls the layer once, at the round that adds it; later rounds share it until its pin moves | **Yes, every Hermes computer's image** (+18.8 MB to pull once) |
| **A new dependency**: `libc` in the bridge (SIGTERM to a core) | tokio's `Child` only kills | Use `nix`, or a `kill` command | None: it was in the lock already, through tokio and hermes-boot | No |
| **What an operator now has to know** | | | (a) A core's output is not logged (it echoes message text); `simplex.*` events say what it did, and its exits by code. (b) Set-aside directories (`<agent>.set-aside-<ms>`) are kept for them. (c) A `simplex-chat` bump migrates its databases forward (`-y`); an image rolled back after one may not open them (to check in that round's canary). (d) An agent's directory stays after the agent leaves the computer, until the person's wipe. (e) The bot's display name is set at its first start, from the agent's title; a later rename is not followed. (f) An unlink while the core cannot run (not always on) leaves the contact in the core's database and in the person's app | |
| **Tests**: `the_bridge_runs_a_linked_core` (the lab's computer image, `tests/simplex/bridge.Dockerfile`: the bridge's script runtime and the core on trixie), by hand, about 3 minutes; unit tests in `run.rs`, `http.rs`, `held.rs` | Proves the lifecycle against the real core: link, hold, gate, restart, rollback, unlink | | By hand: CI does not run it | No |
