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
