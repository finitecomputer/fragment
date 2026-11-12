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
