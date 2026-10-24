# Fragment

Fragment is a platform for stateful, multiplayer web apps and the agents
that make and use them. It powers [Finite](https://finite.chat), our
hosted product: agents that work for you on a computer of your own, and
the apps they make, each at its own address on `*.finite.place`. Fragment
is also built to be self-hosted: it runs entirely on your own Cloudflare
account (below). Self-hosting on your own hardware is coming soon.

A fragment is a folder of files in git, an app of named operations over
its own SQLite, channels that pages follow live, and members with roles.
Hosting, permissions, sharing, crons, jobs and channels are built in.
Your **agents** are fragments too, run by a computer of your own (Hermes,
in a container), and act on other fragments as their members. On
Cloudflare, each fragment is a Durable Object that sleeps when idle, with
its app in a Worker of its own
([docs/cloudflare-v1.md](docs/cloudflare-v1.md)), and each computer is a
Container beside an object of its own ([docs/computers.md](docs/computers.md)).

This repo holds:

- **the cell** (`cell/`): Rust (workers-rs) on Cloudflare Workers, with a
  small JavaScript shim. It answers the control API, serves sites, runs each
  fragment's app in its own loaded worker, and runs jobs as Workflows.
- **the `fragment` CLI** (`cli/`): the whole control surface, built for
  agents. `fragment guide` prints the agent guide.
- **the computer images** (`images/`): the bridge between an agent runtime
  and the fragment API, and our Hermes image.
- **the harness** (`xtask/`, `crates/`): the dev stack, Rust fakes for each
  vendor, and the e2e suite that drives the real cell, CLI, and a browser.

Read [docs/MODEL.md](docs/MODEL.md) for the model,
[docs/api.md](docs/api.md) for the wire contract, and
[docs/cloudflare-v1.md](docs/cloudflare-v1.md) for the plan of record.
MIT licensed; see [LICENSE](LICENSE).

## Use Finite

Sign in at https://finite.chat. To build an app with your coding agent
(Claude Code, Codex), tell it:

```
Read https://finite.chat/llms.txt and help me build: …
```

Or install the CLI yourself (macOS or Linux, no sudo; put `~/.local/bin`
on your PATH if it is not), then pair it with you:

```
mkdir -p ~/.local/bin && curl -fsSL https://github.com/finitecomputer/fragment/releases/latest/download/fragment-$(uname -s)-$(uname -m).tar.gz | tar -xzf - -C ~/.local/bin
fragment login --host https://finite.chat
```

`fragment login --host` saves its host for every later command; with
none ever named, the CLI calls https://finite.chat. `fragment guide` is
the manual; `fragment skill` prints a SKILL.md for your coding agent.
Any deployment's `/llms.txt` names its own URLs.

## Self-host on Cloudflare

Fragment deploys to your own Cloudflare account from one config file kept
outside the repo. [deploy/example.jsonc](deploy/example.jsonc) names every
field and what it needs: a Workers Paid account, two hosts that are each a
zone on it (one zone may be both), a code.storage org for files, and a
WorkOS environment for sign-in; Stripe, models and tool keys are optional.
In your copy, replace the example's `zone` (a branch deployment's) with
`platform_host`, where people sign in, and `fragment_suffix`, under which
each fragment gets its own address. Secrets live in your account's Secrets
Store, set by name ([docs/secrets.md](docs/secrets.md)):

```
cp deploy/example.jsonc ~/fragment.jsonc        # fill it in: platform_host and fragment_suffix, not zone
cargo xtask secret gen fragment-host-secret --config ~/fragment.jsonc
cargo xtask secret set <name> --config ~/fragment.jsonc   # each other secret it names
cargo xtask deploy --config ~/fragment.jsonc
```

`deploy` refuses, before building anything, when a secret the config
names is missing, and says which. A step-by-step guide for a fresh
account is coming. Finite's own preview and production deploy the same
way, from CI ([AGENTS.md](AGENTS.md)).

## Try it locally

One-time setup:

```
rustup target add wasm32-unknown-unknown
cargo install worker-build --version 0.8.5 --locked
```

No Node to install: xtask fetches its own pinned one into `target/tools`
and runs its npm ([docs/pinned-node.md](docs/pinned-node.md), which also says how to move the pin).

Then:

```
cargo xtask dev            # the cell on :8790 under wrangler dev, the code.storage and WorkOS fakes
cargo xtask try todo       # in another terminal: todo | inbox | notes
```

To try a real agent locally, run its computers on our Hermes image and
its text on real models (deploy/example.jsonc's, or `--models <config>`'s),
paid by the key in a file of yours:

```
FRAGMENT_DEV_MODEL_KEY_FILE=~/.config/fragment/secrets/openrouter-dev-key cargo xtask dev --real-agent
```

Sign in at http://127.0.0.1:8790/ with any email; your first run makes a
fresh agent. To start from zero again, wipe that person (the command dev
prints) and sign in with the same email.

`try` creates and deploys a fragment from a template under
`target/devstack/try/`, and prints the link to open and a `fragment`
alias pointed at the dev stack. Fragments are served at
`http://<name>.fragment.localhost:8790/`.

## Tests

```
cargo xtask check          # node --check on our JavaScript, host tests, clippy on host and wasm, warnings denied
cargo xtask e2e            # the full suite against a fresh wrangler dev node (--only a,b | --except a,b)
cargo xtask e2e --shard 2/4
                           # one of CI's four shards (crates/e2e/src/lanes/mod.rs, SHARDS)
```

The e2e stages its own copy of the cell, so it runs alongside
`cargo xtask dev`. Its browser sections drive headless Chrome
(`CHROME_BIN` to choose one).

## Layout

```
cell/          the cell: router, registry, fragment supervisor, jobs, files, blobs, deliveries, ledger
cli/           the fragment CLI and GUIDE.md (the agent guide)
crates/proto   wire types and limits
crates/core    the cell's pure logic, host-tested (schemas, cron, globs, the ledger and price book, web push)
crates/nip98   NIP-98 signing and verification
crates/templates  the release's files (templates/, the shell, the agent docs): embedded in the CLI, an index in the cell (their bytes are its Static Assets)
crates/fakes   code.storage, Workers AI (and Finite Private and OpenRouter: its OpenAI-compatible face), WorkOS, and push-service fakes

crates/devstack  runs wrangler dev and the fakes
crates/e2e     the end-to-end suite
templates/     blank, calories, doc, inbox, notes, todo; the blessed agent, brain, chat, skills
xtask/         build, dev, try, check, e2e, deploy, teardown
deploy/        example.jsonc: a deployment's config (yours lives outside the repo)
docs/          the plan (cloudflare-v1.md), model, contract, the debt ledger
```
