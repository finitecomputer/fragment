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
6. `claude/simplex-door`: securing the link, Paul's design of 2026-10-23:
   the signed link start and the door key (a platform change).

Rebased on master of 2026-10-24 for shipping (#584's mid-turn replies, #562's keepalive ping, #586/#540's routes, #575, #580): what the rebase changed is in section 2 (the save pulse's ping) and section 3 (replies back: the interim words stay in the chat, a card's question goes to the phone).

Alex's review (2026-10-24), each finding reproduced and fixed in its own layer:

- #523: a start reads back, newest page first, to what its chat confirmed
  posted (`delivered`, at most 256 items), not only the newest 32, so a
  message read but never posted is carried at the next start; a reply the
  core refuses mid-sync waits, never dropped; a forced sync that fails holds
  replies and is asked again every 30 s until ok.
- #546: no core outlives the bridge (`stop_cores` before it exits, SIGKILL on
  the forced exit, a parent-death signal for a bridge killed outright, and
  any core an older bridge left is killed before a hold is answered); a core
  whose bot API closed is stopped and started again; an unlink while no core
  runs is finished at its next start (`cleanup`).
- #548: "posted" means on the chat (a refused post keeps its file and sends
  nothing to SimpleX), and the poster acknowledges each confirmed post, which
  alone moves `delivered`; a relayed message quoted is its owner's.
- #549: a stuck link offers Try again and Cancel; a poll from before a change
  shows nothing.
- #550: the lab's image is built when absent (CI's Docker lane).
- #560: a ticket is used only once written (else refused); the bound refuses
  new tickets rather than forget live ones; only the link taken last installs
  its key.

## The audit, in short

**What touches every computer, not only SimpleX users** (the aim is that
each is inert without a link):

- **Every Hermes computer's image:** an 18.8 MB compressed layer (the
  core), pulled once at the round that adds it. Its process does not run
  without a link on an always-on computer.
- **Every Hermes computer's bridge:**
  - an idle manager task (a 100 ms look at the hold file);
  - the hold's answer waiting on `cores_quiet`, which is always true with
    no core;
  - the screen port answering `/simplex/…`;
  - the engine checking each record an agent posts itself against a set
    that is empty without links;
  - each post carrying a `relay` flag, false without SimpleX.
- **Every Hermes computer's hermes-boot:** its hold copy passes
  `/data/simplex` over, a path that exists only with SimpleX.
- **Every chat page and every person's Settings** (platform assets, a
  platform release): the "via SimpleX" rendering, and a SimpleX section
  that says what it needs.
- **The platform (PR 6 alone; PRs 1 to 5 change no cell code):** the
  door (section 6): one owner route that makes a ticket, one guest route
  that redeems one, the agent's `key` in the guest view, two `meta` rows
  on an agent fragment, and one rule on every post: a record whose body
  has `relayed` is taken only signed by its agent's door. The wire is
  add-only; storage is two new `meta` keys (no schema step), which an
  older release ignores. Wasm cost: 19,292 bytes (5,407,371 to 5,426,663 of the 6,500,000 budget, on master of 2026-10-23; 1,073,337 left).
- **Not touched:** the platform's wire to computers beyond those (the
  bridge reads three fields the platform already sends, and `key`),
  registry state, billing, and the stub image's behavior.

**How "computers without SimpleX behave exactly as today" is held:**

- nothing of it runs unless the image names the core;
- no core runs unless the computer is always on and the agent is linked;
- `state.json` is byte-identical without a SimpleX turn (unit-tested);
- a relayed record from anyone but a linked agent, for its owner, is what
  it was before (unit-tested);
- the bridge's whole integration suite, the Docker lane and CI's four e2e
  shards pass on the stack's first two parts.

**The risks Paul should weigh:**

1. **Security, Paul's chosen design (2026-10-23; section 6)**: once the
   owner has linked their SimpleX app from their own Settings, what it
   sends is the owner, with what the shell can do (their questions
   answered, word-cards included). The threat is the agent forging either
   end, and two things close it: only the owner's Settings start a link or
   an unlink (a ticket the platform signs, which the bridge checks and
   takes once, and the invitation shown only to that page), and only the
   link's door key makes a record that speaks for the owner (the platform
   refuses one the agent's own key posts). **What remains:** the agent
   runs as its own user (`hermes`), and `/data/simplex` is root's alone
   (0700, made so at every start) and `simplex-chat` root's and on no
   PATH (#615), so the agent's tools read neither the door key nor the
   core's databases. But its passwordless sudo (docs/computers.md, "Root
   in our Hermes image") reaches root, so a prompt-injected agent with a
   shell could still read the key and sign as the door, or drive a core
   over its bot API on loopback (no authentication). Closing that needs
   the agent without sudo, or the bridge and its cores out of its root's
   reach.
2. **A rollback past the linking itself** leaves the person's app talking
   to a contact that no longer exists. Their messages go unanswered until
   the owner links again. Linking asks for a save at once to keep the
   window to seconds.
3. **An upgrade of the core** migrates its databases forward, so an image
   rolled back after one may not open them. Check it in that round's
   canary.
4. **No cap on cores**: one per linked agent, about 30–35 MB each.
5. **Voice in carries no transcript text** (Hermes transcribes the
   attachment), so search does not find what a voice note said.
6. **Two things are not proven in a browser**: the shell's section and
   pane, and the chat page's "via SimpleX".

**A home, built (Paul, 2026-10-26): section 11.** SimpleX is a chat of
its own, and each agent has one home, its own chat or its SimpleX chat:
what the agent says in its SimpleX chat goes to the phone whatever started
the turn, and a routine with no chat runs at home. The per-turn rule and
its `via` are gone. Hermes' own home stays unset, since in its
multiplexed gateway it reaches nothing of ours (docs/hermes-relay.md).

**Decisions taken by default, Paul's to overturn:** SimpleX's preset
servers; the privacy line "encrypted between your SimpleX app and your
computer"; voice in as an attachment; a reply's audio out as a voice
message, its images as images, any other file as a file (section 10);
the home asked once as a link starts, yes by default, and set back to
the agent's own chat by an unlink; the home's mark in the sidebar shown
for every agent (its own chat until SimpleX is picked) (section 11).

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
| **The forced resync and its edges** (`link.rs`, `run::rolled_back`) | The core cannot see a rollback; a forced sync at a rollback start heals it (measured: about 0.5 s) | None | (a) A platform that sends no `restored` never forces: a silent break. (b) A pending invitation the lost life spent is made anew, and the owner sees a new QR. (c) **A rollback past the linking itself** turns the link off while the person's app keeps a dead contact: their messages go nowhere, unanswered, until the owner links again. Linking asks for a save at once to shrink that window to seconds. (d) A sync forced when nothing was out of step costs about 0.5 s. (e) A message the person sends during the sync may be lost; this is logged without a chat announcement | No |
| **A save asked when a link changes**: a second keepalive socket, held for 3 s (since #562 a keepalive pings and is opened again when silent; this one pings at the default 15 s, so in its 3 s it rarely does) | So linking survives a crash right after it | Drop the pulse; the next turn's save covers it within 15 minutes of that turn | One extra keepalive open of 3 s per link or unlink | No |
| **The bridge reads three more fields of its view**: `alwaysOn`, `generation`, `restored` (`api::Computer`) | The tier gate and the rollback flag | Drop them | None: the platform already sends them, and an old one's absence reads as not always on and no rollback | No wire change |
| **Routes on the image's screen port**: `GET /simplex/state`, `POST /simplex/link`, `POST /simplex/unlink` (`simplex/http.rs`) | How the owner links, from the shell (PR 4 adds the page) | Delete `http.rs` | 404 on an image without SimpleX. **Security, for Paul:** the platform proxies the port to the owner alone, but any process in the container reaches it too, the agent's own terminal included. A prompt-injected agent could link a stranger's SimpleX, who would then speak as the owner, a lasting channel. That is the same trust boundary as root in the container (decisions 43–44: the agent can already open any channel out). **Closed by section 6** (only a ticket from the owner's Settings links or unlinks; the invitation is shown to that page alone), but for root in the container | **Code path on every computer** (the screen server checks the path first); answers 404 unless the image names the core |
| **The tier gate**: a core runs only while the view's `alwaysOn` is true | Paul: SimpleX for the $200 tier, which requires always on | Drop the check | A plan change reaches a computer within 60 s (its view is read every minute). Not always on, the link page says why | No |
| **A seat lapses** | Billing turns `alwaysOn` off (docs/billing.md) | — | Its cores stop within 60 s, and the computer idles to sleep. Links and identities are kept. Messages wait on the SMP servers, for as long as each keeps them (not checked). Paying again turns always on back on: the cores start and the queued messages arrive | No |
| **The image**: `simplex-chat` in `images/hermes/Dockerfile`, a layer of its own between the base and ours (79.5 MB, 18.8 MB compressed), and `BRIDGE_SIMPLEX_BIN` from `hermes-boot` | The core | Delete the layer and the env | Every Hermes computer pulls the layer once, at the round that adds it; later rounds share it until its pin moves | **Yes, every Hermes computer's image** (+18.8 MB to pull once) |
| **A new dependency**: `libc` in the bridge (SIGTERM to a core) | tokio's `Child` only kills | Use `nix`, or a `kill` command | None: it was in the lock already, through tokio and hermes-boot | No |
| **What an operator now has to know** | | | (a) A core's output is not logged (it echoes message text); `simplex.*` events say what it did, and its exits by code. (b) Set-aside directories (`<agent>.set-aside-<ms>`) are kept for them. (c) A `simplex-chat` bump migrates its databases forward (`-y`); an image rolled back after one may not open them (to check in that round's canary). (d) An agent's directory stays after the agent leaves the computer, until the person's wipe. (e) The bot's display name is set at its first start, from the agent's title; a later rename is not followed. (f) An unlink while the core cannot run (not always on) leaves the contact in the core's database and in the person's app | |
| **Tests**: `the_bridge_runs_a_linked_core` (the lab's computer image, `tests/simplex/bridge.Dockerfile`: the bridge's script runtime and the core on trixie), by hand, about 3 minutes; unit tests in `run.rs`, `http.rs`, `held.rs` | Proves the lifecycle against the real core: link, hold, gate, restart, rollback, unlink | | By hand: CI does not run it | No |

## 3. Messages both ways (`claude/simplex-messages`)

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **A new record shape, forever (add-only)**: the relayed message, `{text, to: [agent], relayed: {via: "simplex", for: owner}, attachments?}`, id `sx:<32 hex>` (docs/chat-records.md, "A relayed message"; `records::relayed`, `relayed_message`, `relayed_id`) | The chat is the one authority for what was said (docs/durable-computers.md), so SimpleX messages live there, posted honestly by the agent | Stop posting it. Records already posted stay, and read as the agent's words | An older bridge reads it as the agent's own words and starts nothing. An older page shows it as the agent's message, its words visible, with no "via SimpleX" | The shape is the wire's, for every reader; only linked agents post it |
| **The engine's rule** (`engine.rs`, `record`): an agent's own `relayed` record, for its owner, while its link is bound (`Input::Relaying`), is its owner's message, asked as them | So the turn is the owner's: their Stop, their question answered, their name to the runtime | Delete the branch and the input | A relayed record from anyone else, for anyone else, by another way, or while unlinked, is what it was before: unit-tested | **Code path on every computer's engine**: each record an agent posts itself is checked against a set that is empty without links, so behavior is unchanged |
| **Trust, for Paul**: while linked, anything that can post as the agent (its runtime, the `fragment` CLI in its terminal) can make a turn "asked as its owner" by posting a relayed record | The bridge cannot tell its own posts from its runtime's: both are the agent, in a container its runtime has root in | Gate it on a secret the runtime cannot read (there is none in a container it is root in), or have the platform post it (a cell change) | The agent can already act `for` its owner through the API (docs/api.md), so nothing beyond what it could do already. What changes is what the chat shows: a message from the owner they did not send, labeled "via SimpleX". **Closed by section 6**: the platform takes a relayed record only signed by the link's door key, which the agent's own key is not; root in the container can still read that key | No (only while linked) |
| ~~**`via` on a turn**~~ (`Turn::via`, in `state.json`): **removed in section 11** with the per-turn rule; an older state's is read and let go | | | | |
| **Replies back** (`driver.rs`: `for_simplex`, a post's `relay`, `Handle::reply`): what the agent says in its SimpleX chat is sent to the owner's app once its post lands, whatever started its turn (section 11; until then, only a SimpleX message's turn): every reply but the interim ones (#584's words before a tool call, up to 200 a turn, stay in the chat, as its push leaves them out), and a card's question with its choices (a card shows only in the chat; the next SimpleX message answers one that takes words, and one that needs a button says to answer in the chat) | Paul, 2026-10-26: the SimpleX chat is the phone. The interim rule is the rebase's choice (2026-10-24): each SimpleX message buzzes the phone, as a push does | Drop `relay` | At most once, and only once its post is on the chat (`driver::Sent::Posted`; a refused one sends nothing: Alex's review): a crash between its post and its send loses the SimpleX copy, and the chat keeps it. Its files go after its text (section 10). A turn whose only words were interim (it ended after a tool call with no answer of its own) sends nothing there, and while a long turn works the phone hears nothing until its answer | Code path on every computer's lanes, inert (`relay` is false without SimpleX) |
| **Voice in, as an attachment only**: the voice file is the relayed record's `audio/mp4` attachment, its text empty | Hermes already transcribes every attached voice note through the platform's route before its turn (docs/computers.md, "Its speech-to-text"), as for a voice note sent on the web | Add a transcript to the text | **A departure from the default ("attachment plus transcript"), for Paul**: a transcript in the record too would bill the route twice and hand the model the words twice. The shell shows the voice note as it shows one sent on the web, with no words, and search does not find what was said | No |
| **Files received** into `/data/simplex/<agent>/files`, uploaded as the chat's blobs, deleted once the post is on the chat (a refused post keeps it for the next start's catch-up; Alex's review). At most 25 MiB each (past that, a note instead); at most 32 waiting per agent | A message's voice note or photo | Drop files: post the text alone | A file that does not arrive is posted as a note ("…did not arrive") | No |
| **A poster per linked agent** (a task): its SimpleX chat found or made (section 11; until then its Bot Chat looked up), every 30 s until it is, then each message posted in order, retried as every post is | Posts in order, off the manager's loop | | At most 64 waiting; past that, the next start's catch-up posts them by the same id. While its chat is neither found nor made it waits, saying so in its log | No |
| **`bot_chat` moved** from hermes-boot into the bridge (`api::bot_chat`) | One definition of an agent's own chat with its owner, for Bot Mode and SimpleX | | None: hermes-boot re-exports it and its tests run unchanged | No |
| **The chat page** (`templates/chat/site/chat.js`, `.css`): a relayed record shows as its owner's message, "via SimpleX", once the members say its poster is their agent | Paul: "chats showing up in the shell" | Drop `relayedOwner` | **Not yet proven in a browser** (the e2e has no agent that posts one; parsed by `node --check`). On every chat page, a record from anyone else renders as before | **Every chat page's code** (a platform release, not an image round), inert without relayed records |
| **Tests**: `messages_go_both_ways_through_the_chat`, by hand, 18 s; engine and records unit tests | A SimpleX message to its turn's reply on SimpleX in 414–473 ms; a voice note on the chat in 291–339 ms, the runtime handed its file; a web message's reply not sent; a restart posts nothing twice | | By hand | No |

## 4. The link page and the shell's entry (`claude/simplex-page`)

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **The link page**, `GET /simplex/` on the screen port (`simplex/page.html`, about 8 KB embedded in the bridge; inline script and style; no outside resource but a link to simplex.chat) | Where the owner links: the QR code, and whom they are linked to | Delete the page and its route | It is served by every bridge, so an image without SimpleX serves it too, and it says SimpleX isn't available there. It reaches its routes by relative paths under `/p/6080/` | Code on every computer's bridge, served only when asked |
| **The QR code**, `GET /simplex/qr.svg` (`http::qr_svg`), drawn by the bridge | The person scans it with their phone | Draw it in the page's JavaScript (a vendored encoder) | Proven by decoding (rsvg-convert, zbarimg): it says the invitation's short link, and the person's app links by it (`the_link_pages_code_links`) | No |
| **Third party: the `qrcode` crate** 0.14.1 (its SVG renderer alone, no transitive dependencies), in `images/` | Finite-mono's agentd draws its SimpleX QR with the same crate | As above | A bug draws a code that does not scan; the page also offers Open in SimpleX and Copy link | Built into every image's bridge |
| **The shell**: Settings › SimpleX (`simplexSection`), a row per agent with Set up, which opens the link page in a pane (`openSimplex`, a port ticket as a screen's) | Paul: the QR flow, as finite-mono's | Delete both | The section shows only on a computer whose image does SimpleX (section 7). Not always on, it says SimpleX needs the always-on plan and offers nothing. **Not proven in a browser**: the shell-ui e2e lane has no SimpleX case, since its stub image carries no core | **Every person's Settings**: a shell asset, in a platform release, not an image round |
| **Tests**: `the_link_pages_code_links`, by hand, 53 s; unit tests of the page's routes and its code | | | | No |

## 5. The real Hermes image, end to end (`claude/simplex-e2e`)

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **A Docker-lane case**, `simplex_on_the_hermes_image` (`images/bridge/tests/docker.rs`), run by CI's `docker` job with every other `--ignored` case of docker.rs | Paul: "work well across restarts and updates", on the image computers run. It covers: linked from the screen port; a SimpleX message answered by Hermes as its owner; a voice note transcribed through the route; the hold; a restart; a wake from an older save healed | Delete the case | **New CI cost**: the job now also pulls the pinned SMP and XFTP server images, builds the lab's client image (Debian slim plus the 79.5 MB core, from GitHub releases), and runs the case alongside the rest. Its network: Docker Hub and GitHub, as the Hermes image's own build already needs | CI only |
| **The lab's harness, shared** (`tests/support/simplex.rs`), and `Container::run_with` in docker.rs (more `docker run` arguments: the lab's network) | One lab for the by-hand tests and the Docker lane | Inline it back | None at run time: tests only | No |
| **A file's type by its name first** (`run::media_type`), then by its message's kind | A voice note sent as a `.wav` is `audio/wav`, not the `audio/mp4` the apps' own recordings are | | None | No |

## 6. Securing the link: the signed link start and the door key (`claude/simplex-door`)

Paul (2026-10-23): "no B and no C". A genuine SimpleX message is the
owner; the threat is only the agent forging either end. This PR closes the
two security entries above (section 2's routes on the screen port, and
section 3's trust note) as far as a container whose agent is root allows.

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **The door, a new platform concept** (`crates/core/src/door.rs`; docs/api.md, "An agent's doors"): a ticket, a door key, a relayed record's signature, each a tagged SHA-256 digest signed with BIP-340 (k256, already in the cell for NIP-98) | A link must start from the owner alone, and only the link may speak for them | Delete the module and its three uses below | A digest that drifted between the cell and the bridge refuses every link and every message (held by shared test vectors in both workspaces) | Code in every release |
| **An owner route**: `POST /api/computers/{id}/agents/{fragment}/door {door, act}` → a signed ticket (computer.rs; the agent fragment makes it, runs_on.rs `door-ticket`). A link's is kept on the agent fragment as `meta` `door_ticket`; an unlink's deletes `meta` `door`, so the door closes as the owner asks | Paul: "a link can only begin from the owner's Settings" | Delete the route | Anyone else 404; an agent on another computer 403. A link ticket made replaces the one before (only the newest redeems) | No: an owner's call |
| **A guest route**: `POST /api/computer/door {agent, door, nonce, key}` (computer.rs `egress_api`, runs_on.rs `door-key`): the newest link ticket, once, within 10 minutes, registers the door's public key as `meta` `door` | So only an owner-started link registers a key | Delete the route | 403 for anything else. The agent itself could call it, but holds no ticket: it never sees one (the owner's page hands it to the bridge) unless, as root, it reads the bridge's traffic or memory | Every computer's egress answers it; nothing calls it without a link |
| **A rule on every post** (channels.rs `door_checked`, and `effects::check_record` for apps): a body with `relayed` is taken only from an agent acting as itself, for its owner, signed by its door within 10 minutes; its agent fragment checks the signature (`door-check`, one call between Durable Objects per such post) | The agent's own key (its tools, the CLI) can no longer speak as its owner | Drop the check (and the engine trusts what it reads again) | **Wire, for Paul:** a body field name is now the platform's. Any app's record with a `relayed` key is refused (403 on a post, 400 for an operation's or job's), which changes what was accepted before. Only the chat template uses it, and only from this stack (not yet merged), so nothing shipped is refused today | **Every post on the platform** pays one key lookup on its body; only relayed posts pay the call |
| **The agent's key in the guest view** (`agents[].key`, 64 hex; proto's `ComputerAgent.key`) | The public key the bridge checks tickets with | Drop it (the bridge then refuses every link) | Add-only: an older bridge ignores it. A bridge on a platform without it starts no link, saying so | Every guest view carries it |
| **Storage**: two `meta` keys on agent fragments, `door` (`<door> <key hex>`) and `door_ticket` (`<door> <nonce> <exp>`) | Where the door's state lives | Delete them | No schema step: an older release ignores unknown `meta` keys, so a rollback of the cell keeps working (it would also stop checking relayed posts: the bridge does not check them again) | Agent fragments with a door only |
| **The bridge: tickets** (`simplex/door.rs`, `run.rs` `take`): link and unlink take a ticket in their body, checked against the agent's `key`, this computer, its time, and the nonces taken (written to `link.json` before anything acts, else refused; at most 64 live, past which a new ticket waits rather than one still holding being forgotten; a link's key installed only by the link ticket taken last, so a registration answered late installs nothing: Alex's review); a link's only while unlinked | Paul: "the bridge refuses `/simplex/link` (and unlink) without a valid one, and rejects replays" | Drop `take` | No ticket, a forged one, another agent's or computer's, a stale one, or one used before: 403, and nothing moves. A disk rollback forgets nonces taken since its save; the platform still takes a link's once | Code on every bridge; only link pages call it |
| **The bridge: the door key** (`door.key` beside `link.json`, 0600 in a 0700 directory, generated from `/dev/urandom` per link, in no environment): registered before any invitation exists, signing every relayed record at each post try (`driver::post`'s `door`), forgotten when the link goes | Paul: the key "lives in a file only the bridge reads … never in the environment Hermes' tools see" | Drop it (and the platform's rule) | **Residual risk, for Paul: root in the container can read it.** Hermes' tools run as root today, so a prompt-injected agent with a shell could sign as the door. The real fix is running the agent as a non-root user that cannot read `/data/simplex` (later). A disk rollback that restores an older key than the platform holds refuses what the link carries (logged) until the owner links again; a link with no key at all tells the owner's app to link again | No: only linked agents |
| **The invitation shown to one page** (`View::hidden`, `?s=` the link ticket's nonce): `state` and `qr.svg` give no link to anyone else | Paul: "the QR is shown only in the owner's Settings" (the agent's tools reach the port, and the first to use a one-time invitation is linked) | Drop the session | A page reloaded, or the bridge restarted, while inviting asks for the code again with a new ticket: the same invitation, shown | No |
| **The page and the shell** (`page.html`, shell.js): Link and Unlink ask the platform's page for a ticket by `postMessage`, which asks the platform and answers only a pane of its own onto that agent's link page | The ticket comes from the owner's session, in their Settings | Drop both | 15 s with no answer: "your Settings didn't answer". **Not proven in a browser** (the shell-ui lane's stub carries no core) | Every person's Settings code (a platform release) |
| **No "Approve" step**: not built | Paul made it optional, if nearly free: it is not (a second state, a second ticket, a second page action), and the QR is already the owner's alone | — | — | — |
| **The bridge's new dependency: `k256`** (=0.13.4, BIP-340 only, the cell's own pin), in `images/` | Checking tickets and signing as the door | — | Adds k256's curve arithmetic to every image's bridge (about a dozen crates in the images lock) | Every image's bridge |
| **Known gaps** | | | (a) An unlink made in Settings whose ticket never reaches the bridge (its pane closed first) closes the door at the platform, while the person's app stays linked; what they send is refused at the platform and logged, until they unlink in their app or the owner links again. (b) Unlinked from the person's side, the bridge forgets its key, but the platform keeps the public half until the owner's next link or unlink (it signs nothing: its private half is gone) | |
| **Tests**: the platform's half in the `computers` lane (the stub's `door` commands): a stranger is given no ticket; the owner's is signed by the agent fragment's key, for this computer, for 10 minutes; a relayed record with no door's signature, posted with the agent's own key, is refused; one signed by another key, before and after linking, is refused; the bridge's check of the ticket passes and its redeem registers a key; the same ticket again is refused; a record the door's key signs is taken; after the unlink, it is refused. Unit tests: the shared vectors (core and bridge), `take` (unsigned, forged, another act, replay, after a restart, while linked, no key), the session's view, the key's file mode, a sealed record, `effects` refusing an app's relayed record. By hand and in the Docker lane: no ticket 403, a forged one 403, the invitation hidden from another page, the key registered, its file 0600 and in no process's environment, the unlink closing the door | | | | |

## 7. Offered only where the image does it (`claude/simplex-gate`)

The coordinator (2026-10-25): production's computers run an image with no
SimpleX, so a cell promoted ahead of its image must not offer every
always-on owner a Set up that opens nothing.

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **The bridge says so**: its reads of its computer are `GET /api/computer?simplex=1` when the image names the core (`api::computer`, `driver.rs`); hermes-boot's own reads never say it | The image is what does SimpleX, and only it knows | Drop the query | None: a cell before this ignores the query | Every Hermes computer's reads, every minute |
| **What the Computer DO keeps**: `meta` `simplex`, the version of the image its running start came up on (`version_running`) whose bridge last said so; written only when that is news, its owner's open pages told | So the view knows while the computer sleeps, and an image that never said so (a rollback's, production's before SimpleX) is offered nothing | Delete the key | No schema step: an older release ignores the key. A word said while it starts is not taken (a pin or a deploy may already have changed the image its next start runs, and a crash then would leave the wrong one kept: Alex's review), so a computer is offered SimpleX up to a minute after it is up, at its bridge's next read | Computers whose bridge said so |
| **The view's `simplex`** (proto's `ComputerView`, `fragment_core::computer::does_simplex`): true while the computer is on the version that said so | Settings' gate | Drop the field | Add-only, absent when false | Every owner's view |
| **The shell** (`shell.js`): Settings › SimpleX only with `simplex`, hidden (its nav link too) without it, and following the view while Settings are open (`refreshSimplex`: the list's socket tells the page, which redraws that section alone; Alex's review); `openSimplex` and the ticket listener refuse without it too, so no call to `/simplex/` or the door route is made for a computer without it | The coordinator: no broken SimpleX page for production's owners | Show it always again | **Order of rounds**: an image whose bridge predates this (the canary built from 145dd55 among them) never says so, so on a cell with this its owner sees no SimpleX until their computer runs an image built with it | Every person's Settings code (a platform release) |
| **Tests**: `simplex_is_offered_only_on_an_image_whose_bridge_said_so` (core: never said, said, asleep, a deploy, a rollback); `its_reads_say_whether_its_image_does_simplex` (bridge, in process: with a core named and without); the Docker lane's stub (never says) and Hermes (says) cases; the shell-ui lane: a stub computer's view has no `simplex` and Settings show no SimpleX, then, Settings left open, the lever's word shows the section and its forgetting hides it, what was typed elsewhere kept. The core case covers a pin during a start and a crash before the bridge speaks again (Alex's review) | | | | |

## 8. Reaching SimpleX's servers from a Cloudflare computer (`claude/simplex-port`)

Paul's first link on preview (2026-10-25) failed: "SimpleX said:
errorAgent BROKER". Measured from a preview computer on the canary image
(an e2e person's agent ran the probe in its terminal): DNS, IPv4 and IPv6
work, and TCP connects on 443 and 5223. A TLS hello on 443 that names no
server (SNI) is reset, to SimpleX's servers and to example.com alike; the
same hello naming one completes. On 5223 both complete. A SimpleX server
on 443 serves its website to a hello with a name and its protocol to one
without, so the core sends none there, and so its default (presets on
443) fails on Cloudflare. The core with SMP on 5223 made its invitation
there and subscribed. XFTP's preset servers listen on 443 alone (checked
on 5223, 5443, 8443): an upload from the computer was reset the same way.

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **SMP on 5223** (`--smp-web-port-servers off`, `run.rs` `core_args`) | Cloudflare Containers reset a TLS hello without SNI on 443 | Drop the flag (once Cloudflare passes such a hello) | A network that blocks 5223 but not 443 would reach no SMP server: not Cloudflare's. The phone's own servers are reached on 5223 too | Linked agents' cores |
| **Files did not arrive** (XFTP, on 443 alone): until the XFTP relay (section 9), Paul's choice of (a) | The same reset, with no other port to use | One of: (a) a TCP relay through the cell (a Worker's `connect()`, raw TCP that Containers do not inspect), held to SimpleX's XFTP hosts on 443; (b) Tor in the image (its hello names a server; SimpleX's presets have onion addresses); (c) Cloudflare passing a hello without SNI where nothing intercepts it (an issue for Paul to post); (d) a SOCKS proxy off Cloudflare. **A decision for Paul** | A voice note or photo from the phone is posted, 3 minutes on (`FILE_WAIT_MS`: the core retries an unreachable server for good, and says nothing), as a note that it did not arrive, and the agent answers that; its receiving is cancelled at the core (`/fcancel`), and one that completes anyway is removed, not posted (Alex's review); a text is unaffected. The agent's replies never send files (they are noted) | Linked agents' cores |
| **The error says how** (`wire.rs` `error_of`: `brokerErr` and its `networkError`), as "errorAgent BROKER NETWORK NETLSError" | It said only "BROKER", which hid this | | None | No |

## 9. The XFTP relay: SimpleX's files through the platform (`claude/simplex-relay`)

Paul (2026-10-25) chose (a) of section 8: a TCP relay through the cell, a
Worker's `connect()`, held to SimpleX's XFTP servers on 443.

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **A guest route**, `GET /api/computer/relay?host=&port=`, a WebSocket (computer.rs `egress_api`; docs/computers.md) | The core's no-SNI TLS on 443 is reset by Containers; `connect()` is raw TCP nothing inspects | Delete the route (files stop arriving; texts unaffected) | 403 for any host or port not in the list, 400 without an upgrade, 429 past a computer's bounds | Every computer's egress answers it; only a SimpleX core's files use it |
| **The list**: `relay::HOSTS`, xftp1–6.simplex.im and xftp1–6.simplexonflux.com (SimpleX Chat's and Flux's presets, the operators SimpleX's apps and core v7.0.3 offer), port 443 only, matched whole and in any case | No open proxy: exactly the file servers a phone sends to by default | Change the list (a cell release) | A person whose app uses other XFTP servers: their files do not arrive, and the agent is told so after 3 minutes. A preset list a later core changes needs this list changed with it | — |
| **Bounds** (`fragment_core::relay`): 4 open at once per computer; each at most 32 MiB (or what is left of the day's budget past the open ones' reservations, reserved as it opens: Alex's review) and 5 minutes, closed with 4008 past either; 2 GiB a UTC day per computer; at most 1 MiB waiting to be written toward the server, and frames of at most 256 KiB, in the shim | A voice note is about 1.2 MB, a file here at most 25 MiB; abuse stays small; toward the computer a Worker's WebSocket shows no backlog, so the per-relay bytes times 4 bound what one computer's relays can hold in the Worker's memory (128 MiB) | Change the constants | Past them, the core's next connection is refused and it retries; a file larger than one relay's bound would not arrive (none is, at 25 MiB) | — |
| **What the Computer DO keeps**: each open relay's lease in its memory (lapsing 30 s after its own bound), and `meta` `relay_day` (`<day> <bytes>`) | The count and the day's bytes, per computer | Delete both | No schema step: an older release ignores the key. A restart of the object forgets the open leases, so for one relay's life (5 minutes) up to 4 more may open: bounded | Computers that relay |
| **The shim's pipe** (entry.mjs `relay`, in `ComputerEgress`): a WebSocketPair and a `connect()` socket, each binary frame bytes passed untouched, counted, ended once at a bound or either side's end, its open and close logged (host, port, bytes, time; never a payload), its bytes told to the DO as it closes | A stream of bytes has nothing in it for Rust to decide; the policy (the list, the bounds) is Rust's, which answers with the lease, in headers only the shim reads. **Only the relay route's own answer pipes** (Alex's P1): the shim pipes only an answer to that route's own GET with an upgrade, and the egress strips the relay's headers from every other answer, since an app's own route can answer with any headers it likes and an agent can ask it through its egress (the computers lane does exactly that, and gets the app's answer back as itself) | Delete it with the route | **Workers' terms:** each relay is one Worker invocation holding a socket and a WebSocket; Workers bill requests and CPU time, not wall time or bytes, and piping is little CPU; one invocation may hold 6 open connections, this holds 1 (and the DO call at its close). Its own bounds hold its time. A close that is never heard (the isolate evicted) leaves the lease to lapse and its bytes uncounted | Every computer's egress code |
| **What the platform sees** | | | The host, port, byte counts and times; never the content: the core's TLS runs end to end to the server's own certificate (pinned by the fingerprint in its address), around XFTP's own per-file encryption | — |
| **Who can use it**: any process in the computer, as the computer | The intercept authenticates the computer; no secret in a container its agent is root in tells the bridge from its tools | — | An agent could move bytes to SimpleX's file servers, within the same hosts and bounds; it reaches no other host or port through it | — |
| **The guest view's `relay`** (`{hosts, port}`, the guest's view alone; proto's `RelayOffer`) | The bridge relays exactly what the platform carries, and a platform without it nothing | Drop it | Add-only: an older bridge ignores it; a bridge on an older platform sends everything straight out (files do not arrive, as before) | Every guest view |
| **The cores' proxy** (`images/bridge/src/simplex/socks.rs`): SOCKS5 on loopback, CONNECT only, no authentication or a user name and password it reads and ignores (the core sends some, to keep its sessions apart), at most 64 sessions, 10 s for a handshake, 15 s to connect straight out; a relayed session drives its three parts at once (the core's bytes up, the relay's read, its writes to the core through a queue of 64 frames), so a core that stops reading ends its own session and never blocks its bytes up, the relay's close or `stop`, and a write that takes 30 s ends it; a relay that closes cleanly first has what it sent before its close written to the core (Alex's reviews); the cores run with `--socks-proxy` and `--host-mode public` | The core has one proxy setting for every connection; the proxy sends what the relay carries through it and the rest (SMP, on 5223) straight out | Drop the flags and the module | A proxy that fails to bind leaves the cores straight out (files do not arrive). With a proxy the core thinks its IP protected, so it may skip private message routing to a server it does not know (its default `--smp-proxy-fallback protected`): a small change in SMP's privacy, not its encryption | Linked agents' cores |
| **Tests** | | | Core: the list (presets in any case; a suffix, a prefix, a trailing dot, a port in the host, an address, another port: refused), the bounds (a full house, a lost close lapsing, a close twice or unheld, a restart reading the day back, the day's bytes). Bridge: the handshake (the core's, a plain one, and every refusal), what goes through the relay, and, in process, the proxy relaying through the fake platform's relay (40,000 bytes each way, untouched), straight out to an address, and refused by the platform. The real core through the proxy straight out: the Docker lane's Hermes case and the lab | |


## 10. A reply's files out over SimpleX (`claude/simplex-reply-files`)

Paul, 2026-10-26, on the preview: his agent answered a SimpleX voice memo with a voice reply, and SimpleX received only "(I sent a file, in our chat.)". Now a reply's files go after its text, each as SimpleX's apps show it.

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **The send** (`run.rs` `Handle::reply`, the one way anything goes out over SimpleX; wire `Cmd::SendFile`, `/_send @<contact> json [{filePath, msgContent}]`) | A reply's files go where its text goes | Drop the files from `Handle::reply`: back to the note | The core refuses it (`chatCmdError`): its copy goes and its note is sent | Linked agents' replies alone |
| **What it goes as** (`file_kind`; `audio.rs`, about 60 lines): a voice message when the file is MP3 (MPEG-1, 2 or 2.5 Layer III frames, as Hermes speaks) or MPEG-4 (its `mvhd`, as the apps record) and its length reads; an image for a JPEG, PNG or GIF, with the fixed 64 px preview SimpleX's own CLI sends; anything else a file | SimpleX's apps show a voice message by its `duration`, and an image's preview until it downloads | A decoder crate (symphonia) would bring every codec into the bridge to read one number | Audio whose length does not read goes as a file, which still plays once downloaded. An image's preview is generic, not a thumbnail of it | — |
| **The copies** (`<dir>/<agent>/out/<life>-<n>/<its name>`, one folder each, the name made safe) | The core reads the file while it uploads, and the runtime lets go of its own upload on its own schedule | — | A bridge killed between a send and its answer leaves a folder no send names, and it is swept at the next start. The copies are under `/data`, so a save carries any upload under way (at most 8 × 25 MiB) | Linked agents' |
| **`link.json`'s `sending`** (file id → folder and kind, at most 8) | The core resumes an upload after a restart, and its end then lets the copy go | Drop the field | Add-only (`serde(default)`): an older bridge ignores it, and its copies stay until a newer bridge's sweep | — |
| **Bounds**: 25 MiB a file (the chat's own bound; padded into XFTP's 4 MB chunks it fits one relay's 32 MiB), 8 under way, 5 minutes to upload (`SEND_WAIT_MS`), 1,024 entries a sweep | — | Constants | Past any of them, the note goes instead. An upload given up on is cancelled once (`/fcancel`, whose answer settles nothing): if the core sends it anyway, it arrives after its note | — |
| **When the link cannot send now** (unlinked, its core down under a hold, or its ratchet syncing) | Files are not held; text is held, in the outbox, while syncing | — | The note goes in its place: held with the replies while syncing, and dropped while the core is down, as a text reply is today | — |
| **The relay, upward** | Every file goes over XFTP, even a 47 KB voice reply (measured), so on a computer every send goes through the relay | — | **Unproven on the cell's relay.** The shim ends a relay once more than 1 MiB waits to be written toward the server, and a Worker's WebSocket gives the computer no backpressure, so a multi-MB upload that outruns the server could be cut (the core then retries the chunk). Voice replies, tens to hundreds of KB, stay under it. To check on the canary with a larger file | Computers that send |
| **The scripted runtime's `speak`** (a 3 s silent MP3) | The stub's voice reply, for the lab | Delete it | — | The stub |
| **Tests** | | | Host: the length of MP3s and MPEG-4s (valid; tagged; another type, garbage, a lone sync word, a box past its end, past the frame bound: refused), the send's lines and the core's frames (the lab's own: a sent voice message, its upload's end, a cancel's answer), the lifecycle against the manager (each kind; refused, failed or sent; ends replayed or unknown; past 8, syncing, the core down, unlinked; given up on after 5 minutes; a restart keeping its sends and sweeping the rest), names and notes. The lab, against the real core: a voice message of 3 s and a file, whole and playable (ffprobe), in the person's app 1.7 s after asking, with the copies let go; the same on SimpleX's preset servers through the fake platform's relay to the real XFTP servers (4.2 s, 231 KB up the relay). The Docker lane on the real image: Hermes' MP3 (made by its ffmpeg, so a real encoder's) as a voice message of its length, and a PNG as an image | |

## 11. A chat of its own, and one home (`claude/simplex-home`, `claude/simplex-chat`)

Paul (2026-10-26): "it has to be clear obvious and reliable where
notifications will go (again, why hermes has a 'home' concept)", which he
called a simplification. Two pull requests: the platform's half (the home
in Settings, the sidebar, the link's ask: shell and docs, no cell code)
and the image's (the chat, the mirror, routines at home, what the agent is
told).

**Removed:** a turn's `via` (`Turn::via` in `state.json`, an older
state's read and let go), the per-turn rule that only a SimpleX message's
turn speaks there (`driver.rs`'s `asked_there`), the message's mark beside
its sender's name and its rule in the relay hint (#615's `[paul | via
SimpleX]`, `TurnStart::via`, `wire::sender_name`), and the poster's Bot
Chat lookup. Nothing is kept per turn: the chat is the phone.

| What, where | Why | To remove it | If it misbehaves | Every computer? |
|---|---|---|---|---|
| **The SimpleX chat, made by the bridge** (`run::ensure_chat`, `Saved::chat`, `api::simplex_chat_name`, `api::create_named_for`): its owner's chat labelled `<label>-simplex`, titled SimpleX, its name in full chosen once as the link is first bound and written to `link.json` before anything makes it (write-ahead), then made by that name (`POST /api/fragments?for= {name}`, the agent its maker and an editor, a `joined` telling the bridge to follow it), or, 409, found made from its members | Paul: "SimpleX is a chat of its own", clearly labelled, mirrored both ways. One name across restarts (Alex's review of #620: a list lagging a make made a second chat) | Post into the agent's own chat again (and bring back a rule for which of its turns speak there) | (a) A name someone else holds (one in a million a label) is chosen again. (b) A make the platform refuses (a guest's, past the overdraft, a label past a 16-byte branch's room: an agent label of more than 32 bytes there) is tried again every 30 s, said in the log, and the owner's messages wait (64 at most, then the next start's catch-up). (c) Deleted by its owner, the next refused post makes it again by its name, its history gone. (d) A chat the owner shares: a teammate's message there is answered on the owner's phone too. (e) A rollback of `/data` to before the name was written (the seconds between a link bound and its write) chooses another: a second chat, the first empty | No: only linked agents |
| **The mirror, per chat** (`Input::Relaying` now each linked agent with its chat's name, read from `link.json` at the bridge's start; `Engine::mirror`, the driver's `Effect::Post`) | Everything the agent says there reaches the phone, whatever started the turn | Mark turns again (the removed rule) | The name is known before the chat exists, so no post there goes unmirrored for want of it (Alex's review of #620: a lookup's first second lost replies). Another agent added to the chat is never mirrored, only the linked one | Code path on every computer's engine and lanes, inert (the map is empty without links) |
| **The chat's topic** (relay `wire::SIMPLEX_TOPIC`, `TurnStart::simplex`, `Command::Aside`'s `simplex`): "Your owner's SimpleX: what you say here reaches their phone.", as `source.chat_topic` on every inbound there | Paul: the agent knows, "briefly and in the cheapest reliable place" | Drop the field | About 15 tokens, once, in that chat's session (Hermes pins its session context and renders it again only when it changes). Hermes reads a topic as an untrusted label, so a model may weigh it lightly; nothing depends on it (the bridge mirrors whatever the model thinks) | A field on every turn's start, false without SimpleX; nothing is sent |
| **The home** (`agent.json`'s `home`, `"simplex"` or none; `api::Home`, `api::home_chat`; the shell's Settings pick, sidebar bell, and its link's ask; the link page's box, ticked, and its one line when the shell could not save it) | One home per agent, always visible, asked once at linking (yes by default) | Delete the key, the pick, the mark and the box | No cell change, no schema step, no wire change: a file of the owner's, as the model is. A `simplex` with no SimpleX chat, or its link let go, is its own chat for a routine; an unlink's ticket sets it back; an unlink from the phone's side leaves it, so the shell's bell stays on the SimpleX chat while a routine runs in its own chat; a write that fails is said where it was asked | Every person's Settings and sidebar (a platform release): one read of agent.json per agent a page |
| **A routine with no chat runs at home** (`records::homeless`, `homed`; the driver's `feed` and `home_of`; the CLI's `--chat` optional) | "Everything proactive goes home": a routine scheduled without a chat follows the home | Require `--chat` again | Each such fire reads agent.json and its owner's list (two calls, at most every 15 minutes a schedule). A read that fails ends the follow, which reads the routine again from the cursor after its backoff (1 s, doubling to 30 s): late, never lost, until the hour a missed routine is skipped after. A SimpleX chat named and not made yet (its making refused for now, or a restart between its naming and its making) is waited for the same way, up to `limits::HOME_CHAT_WAIT_MS` (10 minutes) from the fire, then the routine runs in its own chat, never dropped (Alex's review of #620: its claim's 404 dropped it); meanwhile that agent's other routines wait behind it. An agent with no chat with its owner runs it nowhere (`routine.homeless`). **A bridge before this passes it over**: a laptop CLI that leaves `--chat` out, for an agent on an image older than this round, schedules a routine that runs nowhere until its next round (the image's own CLI comes with its bridge) | Code on every computer's `tasks` follow, inert unless a routine names no chat |
| **The agent told where home is** (hermes-boot's `told_home`, in its profile's `agent.system_prompt`, beside its name) | So "remind me…" goes home, and it says where | Drop the sentence | About 35 tokens a turn, every chat, in the per-turn system prompt; it changes only with the home (read with agent.json every 10 s, written only when changed, its next turn's; a running turn keeps its prompt) | Every Hermes computer |
| **Hermes' own home: not set** | Read from v0.21.7: in a multiplexed gateway a relay home reaches nothing of ours (`send_message` is no model tool, background notices go to their own session, restart and shutdown notices skip a secondary profile, the prompt's home lines are the launch profile's) | — | `RELAY_HOME_CHANNEL=none` stays, only to stop Hermes' `/sethome` ask | — |
| **Tests** | | | Unit: the chat's name and the home either way (`an_agents_home_is_its_simplex_chat_or_its_own`), a routine homeless and homed, what goes back (per chat: a relayed message is not), the engine (`turns_in_its_simplex_chat_are_its_owners_phone`: linked, found, unlinked, a routine, an aside; an older state's `via` let go), the manager (`a_linked_agents_simplex_chat_is_named_once`: written before it is made, the same after a restart and at the bridge's start, kept through an unlink, another when taken), the page (the shell's answer kept, its `homeError` said), the topic on every inbound there, the prompt's home either way, the CLI. In process: `a_routine_with_no_chat_runs_at_home` (its own chat, SimpleX by its link's name, a failed read, no chat at all, a restart with its link let go), `a_routine_waits_for_its_simplex_chat` (past the wait, its own chat; waiting across a restart; run there once made; nothing twice after a restart) and `its_simplex_chat_is_made_once_by_its_name` (made, found made after a restart with stale lists, someone else's or one it is not in taken, the platform down). The e2e: the computers lane's routine at home on the stub (its own chat; `simplex` with no link, its own chat still), the shell-ui lane's home checks. The Docker lane's `simplex_on_the_hermes_image` on the real image and core: the chat made, its topic in the model's prompt, words typed there and a routine at home answered on SimpleX, a reply in its own chat not, one chat through restarts and a rollback | | | | |
