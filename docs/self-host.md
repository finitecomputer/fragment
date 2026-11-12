# fragment self-hosted

Self-hosting runs master's cell on other implementations of the interfaces
it is written against. It lands on master seam by seam: each is switched
on by configuration, and master's Cloudflare path is unchanged. Where this
disagrees with `docs/cloudflare-v1.md`, that doc wins.

## The rule: one cell, interfaces, placement as configuration

The cell is written against Cloudflare's runtime APIs (Durable Objects,
the Worker Loader and facets, Workflows, Queues, R2, `ctx.container`) and
protocols for what sits outside a runtime (OpenAI's chat completions, the
code-store contract, the DevTools Protocol, Web Push). Self-hosting
supplies other implementations of the same interfaces. The cell never
asks "am I self-hosted?": it reads what it has from its configuration and
its bindings. A mode flag is a design bug.

The e2e checks the cell without a service on wrangler too: its `unbound`
section restarts the node with those bindings left out of its config
(`devstack::Fleet::unbound`), so master's own runs keep each degradation
working.

## 3. Models from a file

`cargo xtask dev` reads `FRAGMENT_MODELS_FILE`: a deployment's `models`
block alone, as its config writes it, checked as a deploy checks it
(`deploy::models_cell`) but for its URLs, which may be this machine's
(`http://127.0.0.1:<port>`, as a local fleet's egress reaches it). The key
its OpenAI-compatible slots name is the value in `FRAGMENT_MODEL_KEY_FILE`
(one; a server that takes none still gets a word). Slots it leaves out,
and image steps, stay on the Workers AI fake. It is `--real-agent`'s
models without our Hermes image, and the two are one or the other. A
self-hosted model names its price in its slot, zero for the box's own:

```jsonc
{
  "cheap":  { "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "key": "local-key", "model": "local-27b",
              "price": { "input": 0, "output": 0 } },
  "medium": { … the same }
}
```

```sh
FRAGMENT_MODELS_FILE=models.jsonc FRAGMENT_MODEL_KEY_FILE=model-key cargo xtask dev
```

## Mail

Master mails through Email Sending's binding (`EMAIL`), or in dev and the
e2e through the fake. A self-hosted deployment has neither: `mail::send`
sends nothing, logs one `mail.unsent` warning ("email not sent: this
runtime has no mailer, not yet implemented") and answers no message id,
never an error. An invite still waits for its person's yes; a key is still
added; an org's `mailed` and a trial's `sent` say false. Debt:
docs/technical-debt-ledger.md, "No mail without Email Sending".

## Bindings a runtime may lack, or class differently

A self-hosted runtime binds what it has. The cell reads a binding by what
it does, or whether it is there, and never asks which runtime it is on:

| Binding | Without it | How the cell runs |
|---|---|---|
| `r2_buckets` (`BLOBS`, the move's `STAGE`), `assets` (`ASSETS`) | there, but its class need not be workerd's (celld's read as `Object`) | read by what it does (`js::by_what_it_does`), not by workers-rs's class check |
| `browser` (`BROWSER`) | no cards | a live is skipped once, `card.skipped` `no_browser`; a shot asked for again is refused, typed (`host_failed`) |
| `images` (`IMAGES`) | no icons drawn | an icon's making, and an upload that needs fitting, refused at once, typed (`host_failed`), before anything is asked or charged; a PNG at the icon's size is kept as it is |
| `containers` | no computers (`FRAGMENT_COMPUTER_IMAGE` unset: a computer is refused, typed) | the container host reads no container as none running, so a person's wipe still empties their computer's rows |
| `send_email` (`EMAIL`) | no mail | Mail, above |
