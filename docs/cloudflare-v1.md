# fragment's decisions

The decisions that bind fragment. Code cites them by number, so a number
is never reused: one that no longer binds stays as a line saying so.
Contracts live in their own docs (api.md, computers.md, bridge.md,
chat-records.md, ledger.md, billing.md, secrets.md); this file holds the
choices behind them.

## The rule: fragments and computers

The platform is **fragments and computers**, plus only what those two
need: identity, sign-in, metering and credentials. It stays as thin and
generic as it can.

- We run **Hermes** on computers today, and chat with it through a
  fragment. Hermes is not a platform concept: its image, the bridge in
  that image, and the chat and agent templates are data the platform
  runs. The platform contains no Hermes code.
- Anything not expressible as a fragment or a computer is listed, with
  its reason, in `SPECIAL-CASE-INVENTORY.md`. A thing is platform only
  when that is a security win, or it is the thin shell users can't
  break. Platform pages use the same public APIs as fragments.
- A change that teaches the platform about one agent runtime, one
  template or one vendor is a design bug. Move it into an image, a
  template or a skill.

## The product

fragment is a personal-agent product with the magic of fragments:
multiplayer apps, edited together in git, that your agents build for
you. You chat with your agents in chat fragments; they are fragments run
by a computer of your own, use your connected accounts, drive their own
screen, and publish apps, sites and brains that you share.

fragment is Finite v3. It runs entirely on Cloudflare, and anyone can
deploy it to their own account without forking it. finite.computer's
separate V3 Core (Linear FIN-10, FIN-11) was canceled on 2026-10-05, so
fragment's registry, sign-in, billing and ledger are the real ones, not
stand-ins. The self-hosted lane (celld for cells, sandcastle for
computers) returns once this product works.

## Decisions

### Platform

1. **Cloudflare for everything it can do.** Outside services only where
   it can't: WorkOS (AuthKit, Pipes), code.storage for git (until
   Cloudflare Artifacts has a commit API), Stripe (51), and the model
   APIs a deployment names (61).
2. **The Rust cell, ported to Workers (workers-rs), not rewritten**, with
   thin JS shims where workers-rs lacks an API. The cell is the one
   Worker; the CLI is Rust.
3. **Accounts.** Dev and previews run in a dev account on WorkOS's
   staging environment; production in Finite's production account on
   WorkOS's production one. CI deploys from a clean config, as a
   stranger would. Only Paul updates `production`: his push or merge is
   a production deploy's approval. There is no canary: one image name,
   `hermes`, at master's digest on preview; production's is the digest
   preview proved, copied and pinned by hand (docs/computers.md, "Preview
   and production").
4. **Self-deploy without forking:** a config file outside the repo
   (deploy/example.jsonc), secrets in the account's Secrets Store, and
   `cargo xtask deploy --config <file>`. Updating is `git pull` and the
   same deploy.
5. **Domains.** The platform on one host, its fragments as
   `<name>.<suffix>` on another site (`platform_host`, `fragment_suffix`;
   production: `finite.chat`, `*.finite.place`). One zone for both is
   allowed, at weaker isolation. fragment.club and fragment.boats stay
   celld's until cutover, and nothing on them migrates.

### What a person sees

6. **The shell is the platform's one page:** sign-in, a person's
   fragments and computer, and frames for them. It knows nothing about
   Hermes.
7. **Layout.** A flat list of the person's chats, newest first, each row
   naming its agent; apps in a dock on the desktop; a launcher (Command
   K) that searches or starts a chat with an agent. One UI for any
   number of agents. Nothing is mocked or optimistic. It installs as a
   PWA.
8. **Chats are chat fragments**, a person and their agents as members. In
   a group an `@mention` picks who answers, else the lead (the first agent
   added). Agents ask each other by `@` or `fragment ask`. People
   collaborate in other fragments, not in chats.
9. **Chat comes from the chat template** (docs/chat-records.md), push
   included. A voice memo is an attachment the agent transcribes itself
   through the model route (`whisper`); the platform runs no speech to
   text of its own. Every model call names its agent one way, by key
   (`agent:<name>`) where its client sends no header. Search across
   chats, apps and messages is the shell's, over what the person may read.
10. **First run:** sign in, a seat, then the shell makes the person's
    computer (the operator's default image), agent, and a chat of the two
    through public APIs. The agent names itself and draws its picture
    while the computer starts.
11. **A computer serves its own screen:** a VNC page with Take over and
    Give back, reached through decision 41's ports.
12. **Settings:** the person's account, agents and skills, computer,
    connections, usage, credit and plan.

### Agents and computers

13. **One computer per person for now**, an entity of its own so other
    machines can come later. The image, not the platform, decides what
    runs. The default size is 2 vCPU and 6 GiB (`2vcpu-6gib`).
14. **An agent is a fragment** (kind `agent`) with its own identity: its
    repo (`SOUL.md`, `memories/`, `skills/`, its avatar), SQLite, cron,
    members, page and cap. The computer that runs it is its cell meta
    (`MetaKey::Computer`), not its repo.
15. **An agent runs on the runtime its `agent.json` names** (`runtime`,
    `hermes` when it names none). Under Hermes it is a profile, checked
    out from its fragment and committed back; one Hermes gateway serves
    every profile (`gateway.multiplex_profiles`).
16. **Making an agent** gives it a job (`SOUL.md`); it names itself and
    draws its picture, which its owner may change. Nothing technical asks
    the person: Hermes' approvals are off, and an agent asks its owner
    only about outcomes.
17. **Skills are git-tracked.** The managed set is the blessed `skills`
    template, versioned with the platform; an agent's own skills are in
    its fragment's `skills/` and win on a name.
18. **A computer's state survives in saves** of its `/data`, and a wake
    restores the newest good one. Time a computer is kept for its failed
    saves is free to its owner, who is told and has one way back,
    Restart. The agent's self is in its fragment (docs/computers.md).
19. **Image updates.** An image is pinned per computer. A new default
    reaches a sleeping computer at its next wake; an awake one's owner is
    told and restarts to take it, never behind their back. A pin moves
    one computer to another of the deployment's images; a rollback is the
    image before pinned back, plus a point-in-time restore when the new
    image moved its data's schema (not built: the debt ledger).
20. **Preview environments.** Every branch deploys a complete, separately
    named copy with one command, at `<branch>.<zone>` and its fragments
    at `<name>--<branch>.<zone>`; a teardown removes it (an account holds
    at most 500 Durable Object classes). Worker Previews can't be this:
    their Workflows, Queues and bindings stay on production.
21. **The bridge lives in the image.** It drives the runtimes its agents
    name and speaks the fragment API outward. With Hermes it speaks
    Hermes' Relay locally (`draft`; `prompt`, `prompt_response`;
    `task_card`; `send_media`, `follow_up`, `react`, `typing`, `edit`,
    `delete`). Another runtime brings its own module, or its own bridge
    (docs/bridge.md).

### Connections, models, money

22. **Connections through WorkOS Pipes,** which holds the tokens. A
    computer holds per-agent placeholders in the variables SDKs read; its
    egress swaps in a short-lived token for an agent allowed it (44),
    counting each call by agent and month. Gmail needs Google's
    verification (CASA) past 100 people.
23. **Models through the platform's route,** on AI Gateway with zero data
    retention and its logs off. Tiers: `cheap` (the default) and `medium`;
    `high` is off until Cloudflare raises Unified Billing's Opus limit,
    with no key of our own on AI Gateway and no sharding to get round
    it (a person's own key is 60's). `vision` is for image calls and is
    no tier. A call that fails before answering is made once more on its
    fallback; an answer that began is final. A call that answers nothing
    is the runtime's to give up on. Jobs ask typed decisions on the Clef
    family.
24. **Every per-person cost is metered** in integer micro-dollars into the
    payer's ledger, at Cloudflare's cost plus the operator's margin
    (`price::MARGIN_BP_DEFAULT`). No global object sits on a hot path.
25. **Plans.** A guest is free, uses what is shared, has no agents and
    creates nothing. `seat` ($100) includes $50 of credit a month and a
    computer that sleeps; `seat_always_on` ($200) includes $100, an
    always-on computer and SimpleX. Only the $200 seat and the $25 pack
    are sold; `seat` stays for comps and a deployment's `default_plan`.
26. **A fragment's costs bill its owner**, under a monthly cap per
    fragment (`ledger::CAP_DEFAULT`); past it, AI steps and agent turns
    stop for everyone but the owner.
27. **At zero credit agents stop**, and the shell says why. Fragments
    serve on an overdraft (`ledger::OVERDRAFT_DEFAULT`), then go
    read-only; their cron and triggers record runs as blocked until the
    owner has credit.
28. **Sign-up is open; creating needs a seat:** decision 49.

### Fragments, brains, sites

29. **All of celld's fragment model is ported:** done.
30. **A brain is a fragment app** (template `brain`) keeping Finite
    Brain's data architecture, wiki skill and FTS5 search. Access is per
    brain. There is no encryption.
31. **Sites are fragment apps.** A deploy takes a screenshot for the app's
    card, as a visitor without an account sees it (docs/api.md, Cards).
32. **SimpleX on $200 seats only,** driven by the bridge while always on:
    another door into the agent's chat with its owner, mirrored in its
    own chat. Only the link's door key speaks for the owner. Other doors
    are connector fragments their owner binds, as the deployment's
    `connectors` allow (docs/api.md, "An agent's doors"). An agent's one
    home is where routines with no chat run (docs/api.md, "An agent's
    home").

### The cut

33. **Hard cut.** Gone from master, kept at the tag `celld-final`: goose
    (on computers and in fragments), the platform's own chat, the desktop
    template, sandcastle and its Hermes, the celld fork, native `KEYS`,
    `crates/node`, the Fly fleet and their lanes. An app's own model
    calls are its AI steps.
34. **Infra comes down after cutover**, one irreversible step at a time,
    each confirmed by Paul on the day.
35. **master is the Cloudflare line.** fragment.club deploys only from the
    `celld` branch (tag `celld-final`) until cutover.

### Agents acting for people

36. **Sharing with a person shares with their agents,** each acting for
    them as itself, never above their role (R17): share with Skyler as an
    editor, and Skyler's agents edit it. Someone else's agent is never
    added on its own; it comes in with its person (64). Only an owner
    holds an agent below themselves. What an agent makes is its person's.
    An agent shares what its owner owns as its owner would, but never
    deletes or sets a cap (`access::agent_shares`).
37. **Agents can do anything they could do in Finite.** Services that act
    as you are connections (22); paid APIs that need only a key use the
    operator's keys, priced per call (`price::DEFAULT_KEYS`); a person's
    own keys are sealed by their computer and never metered.
38. **Routines are fragment cron:** scheduled records on any fragment, an
    agent's on its own `tasks` (`fragment schedule add --routine`).
    Hermes' scheduler is off (docs/api.md, "Scheduled records").

### The generic pieces

39. **A subscription wakes a computer.** A record on a channel its bridge
    follows wakes it. It stays awake while a port tab or the guest's
    keepalive socket to the Computer DO is open (container traffic does
    not count), then sleeps after `computer::IDLE_MS`. A page that opens
    a fragment it follows pre-wakes it (`PREWAKE_MS`). An agent added to
    a fragment wakes it with `{kind: "joined"}` on its `tasks`.
40. **Blessed templates run the platform's release.** A chat, agent, brain
    or skills fragment runs its template's code from the release, so one
    deploy updates them all. Forking makes the code your own.
41. **A computer exposes ports to its owner** (delegates later), as an
    authenticated tab or socket. Nothing else reaches it from outside.
42. **Only an agent's owner answers its cards.** A card has an id and an
    expiry; the first answer wins. Waiting on one never holds the
    computer: an answer after a sleep or a restart starts one fresh turn
    (docs/bridge.md, "A card outlives its turn").
43. **Computers reach the internet.** The guest holds no secrets, so
    traffic that bypasses the egress carries nothing of ours.
44. **A person's agents aren't fenced from each other.** Each may see
    everything its person has. An owner may narrow an agent's connections
    to a list (`connections: null` is all): specialization, never a
    security boundary.

### People

45. **A person is an npub; others find them by email.** A first sign-in
    makes a person a key, whose npub names them for good; an agent's is
    its fragment's. A person's email is their sign-in's verified email
    and names at most one person. Linking a second sign-in is off until
    WorkOS offers an account picker.
46. **The registry keeps a person's key**, sealed (`cell/src/keys.rs`),
    never in WorkOS. Browsers act through sessions. The platform signs
    with it only to cross into an outside service, one named purpose at a
    time, never for a page's script.
47. **No usernames.** A fragment's name is one DNS label, its maker's
    label, `--` and a random suffix (`todo--k3x9`), fixed for life. In
    the CLI a bare label means the caller's own.
48. **Sharing is by email,** with a role; the invite waits for its
    person's yes (63) and is mailed, capped per sharer per day. Members
    see emails. Agents have no email: an owner adds their own agents
    directly, and anyone else's come in with their person (64). Anything
    shaped like an email is an email. `link`, the default visibility, is
    a rotatable `?view=<token>`; a bare address opens nothing; no bearer
    invites. A chat or an agent is made `members`. The share link is the
    owner's.
49. **Sign-up is open; creating needs a seat,** bought, given by an org's
    admin, or comped by an operator.
50. **Later: paying with a key** (not built): a CLI pays over HTTP 402
    with no sign-in. So an identity is an npub whether or not a sign-in
    names it, nothing may need the registry to hold a person's secret,
    and a payment is credit granted on a ledger.

### Seats, orgs and Stripe (docs/billing.md)

51. **An org is who pays.** Anyone may buy a seat; a self-payer is an org
    of one. A person is in at most one org with at most one seat. Nobody
    joins without accepting. Who pays is no other admin's to remove.
52. **Seats are paid or comped, held or pending.** A pending seat is an
    offer on an email and bills from the offer; paid seats ride one
    subscription, prorated. Comps are an operator's, outside Stripe.
53. **Standing is Stripe's status:** `trialing`, `active`, `past_due` are
    good; a lapse stops agents and deletes nothing, and paying restores.
54. **Stripe automatic tax; prices before tax.**
55. **One credit pack, $25, kept until spent.** Usage never goes to
    Stripe.
56. **Trials are our codes on Stripe's trial:** no card, one per person
    ever.
57. **A $200 seat's computer stays awake** unless its owner lets it sleep.
58. **Previews have their own Stripe sandbox endpoint.**
59. **The operator's admin** (`/admin`) acts through an operator's browser
    session, logging every write; to anyone else it is a 404. A wipe
    stays a signed CLI call.

### Models

60. **A person may connect their own OpenRouter account from Settings**
    (its PKCE exchange: nothing pasted) and pick a model per agent
    (`agent.json` `model`). Their computer keeps the key sealed, and their
    credit pays: no platform fee. ChatGPT and Claude subscriptions are not
    offered: their terms do not allow a hosted app to use them.
61. **Where each model runs is the deployment's config** (`models`): a
    Workers AI model, or one on an OpenAI-compatible API with the
    deployment's key. A call sends only the agent's chat fields
    (`models::CLIENT_KEYS`) and the slot's `body`. An OpenRouter slot is
    priced at its ceiling (`max_price`), refused without one; a slot on
    another API at the list price it names (`price`). The platform's own
    key trouble falls back like a 5xx. A tier slot's `label` names the
    route Settings offers for it (Private, Standard), picked per agent as
    its tier. A private tier's every slot must be private, and never
    falls back to one that is not. Finite Private
    is priced as Workers AI's GLM-5.3 Flash, so a person pays the same
    whichever answered.

### Billing never stops an agent

62. **A spend is gated by a cached standing and charged after**
    (`ledger::may`; docs/ledger.md, "The gate"). A ledger that does not
    answer lets the spend start; nothing is held before a call, so a
    person may overspend by what starts within the cache.

### Invites and members

63. **An invite waits for its person's yes,** given in the shell or by
    their agent. It names no agent. Adding someone directly works only
    within one person: the owner's own agents. The wire is added to,
    never changed.
64. **A person's membership is their agents'**: rows kept in step, in and
    out together (`members.rs`: `agents_join`, `remove_member`,
    `agent_made`). The inviter sees the person "with their agents". A
    chat warns its inviter that people in it can talk to their agent.

### Fast and Deep

65. **Every tier runs two modes:** Fast for an agent's turns, Deep for its
    delegates, each with its own model, reasoning, cap and fallback
    (`models::SlotOpts`). Private runs both on private models and never
    falls back out of them; Standard has Cloudflare behind both. A
    person's own models name a model and a reasoning for each mode.

### Orgs own fragments

An org owns and pays for fragments as a person does, and the people it
appoints control who may use them.

66. **An org pays from credit of its own.** Its ledger (`org:<id>`) holds
    credit operators grant it, as orgs pay out of band, the person it is
    invoiced to named in its governance log. It has no plan, seat,
    included or free credit, and buys none itself; a seat's included
    credit stays its holder's.
67. **Org credit pays for the org's fragments, never a person's agent.**
    An org-owned app's hosting, storage and AI steps bill the org; a
    person's agent turns, desktop and computer bill that person, in an
    org's fragment too.
68. **No overdraft, an explicit budget, the cap for everyone.** At zero
    an org's fragments are read-only; past the monthly budget an operator
    sets (zero until set) their AI steps stop. Each fragment's cap holds
    for every caller, admins included.
69. **Content admins are appointed, never inherited:** a role apart from
    billing admins. Who pays appoints the first, by an offer they accept;
    no billing admin is promoted; the last is never removed.
70. **Deletion, full export and transfer out are deliberate:** each a
    content admin's own action, never an agent's; a transfer waits for
    its target's yes. Offboarding does none of them.
71. **Content admins control access to the org's fragments, visibly:**
    every grant is in the org's governance log and the fragment's events.
    They may grant themselves access, but administer no personal
    fragment, read no secret, and act as no one else.

### Carried from the ROADMAP

- **R4. Sharing is the platform's.** The share sheet and accepting an
  invite are platform pages, so a fragment's page never grants anything.
  On a fragment not yours or shared with you, the platform asks once
  before it learns who you are (docs/api.md, Asking first).
- **R15. Identity.** People, agents and fragments are identities
  (`fragment_core::registry`):
  1. **Identities, not keys:** grants name an identity's npub, so
     replacing a key rewrites no grant.
  2. **A verified `(issuer, subject)` signs a person in**; another login
     is another issuer, linked explicitly (off: 45), never by email.
  3. **Keys stay with their callers**, but a person's own (46): browsers
     act through sessions; the CLI and agents sign (NIP-98).
  4. **An identity has one or more keys**, each added with proof of
     possession by its human and revoked on its own. An agent keeps one.
  5. **Every agent has one human owner**, who reads what it reads, as a
     viewer, and never acts through it.
  6. **Services own permissions:** the registry stores no grant.
  7. **Lookups are live and fail visibly:** no cache outlives a
     revocation, and a signed request fails when the registry cannot
     answer.
- **R16. One DNS label per fragment**, so one wildcard certificate covers
  every fragment (one per host runs into Let's Encrypt's limits).
  Sessions are `__Host-` cookies.
- **R17. An agent acts for whoever asked, capped:** with the lower of the
  asker's role and its cap (its own role, or its owner's on a fragment
  the owner belongs to), never above `editor`.
- **R18. Postable channels.** A declared channel may name a `post` role,
  and the platform appends a member's record itself: the one exception to
  "clients never append" (docs/MODEL.md). `"signedIn": true` refuses an
  anonymous poster.

## Lessons from cloudflare/agents

1. **Split work by authority:** the channel owns what was said, the
   runtime the turn, the Computer DO the lifecycle, the ledger usage; the
   bridge owns nothing.
2. **A turn is admitted once,** by its record's `(channel, seq)`; a
   scheduled record once, by its schedule and minute.
3. **One job queue drives each object's one alarm** (`crates/core`): the
   newest push wins, a deadman catches a hung job.
4. **Classify a failure before retrying it;** a wake that keeps failing
   shows "won't wake" instead of paying for starts.
5. **Deploys are routine and noisy:** clients reconnect with jitter.
6. **A container can outlive a deploy:** a new isolate adopts it, never
   destroys it, and waits for `running` to be false before a `start()`.
7. **The model intercept is bounded,** and keeps only a call's final
   usage, never its request or stream.
8. **Read a gateway call's id from its response header.** GLM calls carry
   `x-session-affinity` for prefix-cache hits.
9. **Native snapshots:** in use. Done.
10. **Facets have no alarm:** done.
11. **A late page gets the draft in flight,** and the final replaces it by
    id (docs/chat-records.md).
12. **The person's index is a fenced projection:** constrained updates,
    never upserts, repaired by pulling. Search is FTS5 in the Principal.
13. **Fixtures assert meaning, not bytes,** with a scripted model that is
    a pure function of the transcript.
14. **Log one JSON line per event:** `wrangler tail` drops lines.

## Bugs the port must fix

1. **A generated image between 256 KiB and 1 MiB could not be saved:**
   fixed.
2. **A retried image step bought a new image:** a paid answer is kept and
   charged once.
3. **A failed step kept its reservation:** gone with the holds (62).
4. **`fragment deploy` reported success when refused:** fixed.
5. **An image is served by its path's extension, not its bytes:**
   `job.ai.image` takes only `.jpg` or `.jpeg`.
6. **Small job-written files had no cacheable URL:** a file's version
   (`__file?path=&v=<commit>`) is `immutable` (FIN-340).
7. **`fragment create` printed secrets:** fixed (`--show-tokens`).
