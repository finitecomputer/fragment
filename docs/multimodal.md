# Images, and what an agent's model sees

Paul, 2026-10-22: "how does hermes handle multi-modal models? I'm using
deepseek but it feels like it keeps handing stuff off for vision tasks
instead of doing it itself? or maybe it is doing it itself it's hard to
tell. that will be something to explore."

An exploration, not yet a decision: what Hermes does with an image, what
our image tells it to do and why, what it would take for a model that
reads images to see them itself, and a proposal. Nothing here changes
behavior yet.

Read 2026-10-22 from Hermes v0.21.7 (tag `v0.21.7`, commit
`71d34bad1045`), the source in our pinned base image
(images/hermes/Dockerfile's `HERMES`, under `/opt/hermes`; Hermes paths
below are relative to it), and from master at `7440bdd1`. OpenRouter's
catalog was read the same day (`GET /api/v1/models`,
`/api/v1/models/<id>/endpoints`, `/api/v1/endpoints/zdr`).

## In short

- **It is handing off, every time, and that is our config.** Every
  agent's profile names an auxiliary vision model (hermes-boot,
  images/hermes/boot/src/hermes.rs:410-428), and Hermes takes an explicit
  `auxiliary.vision` to mean every image is described in words for the
  main model (agent/image_routing.py:6-8, 387-388). So no main model of ours
  has ever been shown a pixel: not the tiers' models, not DeepSeek, not a
  person's own model (Claude, GPT through their OpenRouter). Each image
  goes to the route's `vision` (Workers AI's GLM-5.3 Flash) and the main
  model gets its words.
- **Hermes can show the pixels to the main model itself** ("native"):
  an attached image as a part of the user's message, and each
  screenshot as the tool's result. It does so only when told, per model,
  that the model reads images: it cannot learn it from our route.
- **Our model route already passes images through**, and meters them as
  the input tokens they are. What it lacks is knowing which of its models
  read images, a fallback that keeps images away from a model that reads
  none, and room: a call is at most 6 MiB, and native images ride the
  conversation.
- **Unknown until tried**: whether DeepSeek V4.1 Flash, under Finite's
  OpenRouter routing, takes images at all, and inside a tool's result.

## How Hermes routes an image

### Two modes, one decision

`agent.image_input_mode` is `auto` (the default), `native` or `text`
(agent/image_routing.py:27; hermes_cli/config_defaults.py:273-278).
`decide_image_input_mode` (agent/image_routing.py:375-391):

- `native` or `text`, set outright, is the answer.
- In `auto`, an explicit `auxiliary.vision` backend (a provider other
  than `auto`, a model, or a base URL: `_explicit_aux_vision_override`,
  :262-271) makes it `text`, "even for vision-capable main models".
- Otherwise it is `native` only when the main model "supports vision"
  (`_lookup_supports_vision`, :330-372), in this order: the config's own
  word, `model.supports_vision`, then
  `providers.<provider>.models.<model>.supports_vision` (or `vision`),
  then legacy `custom_providers` (:141-173); then three probes: a managed
  local runtime, the models.dev catalog (fetched over the network, cached
  4 h: :285-305), and Ollama for a local endpoint (:308-318). Unknown is
  `text`.

Our profiles' provider is `custom`, a tier's (at the intercept) and an
owner's own model's (at OpenRouter) alike (hermes.rs:385, 394).
models.dev knows no `custom` (agent/models_dev.py:110-130; a custom
provider reaches it only through a `providers.<name>.catalog_provider`
alias, :180-195), and `model.fragment.internal` is no local endpoint to
probe (agent/model_metadata.py:675-706). So even without our
`auxiliary.vision`, `auto` would choose `text`: only the config can say
that a model reads images.

### What each kind of image does

| The image | `text` (every agent today) | `native` |
|---|---|---|
| **One a person attaches** | Described before the turn, by `vision_analyze_tool` on the auxiliary model ("Concisely describe this image in 2-4 sentences…"); the turn's text begins `[The user sent an image~ Here's what I can see: …]` and `[If you need a closer look, use vision_analyze with image_url: <path> ~]` (gateway/run_inbound.py:1469-1503, 1856-1897). No step in the chat: it runs before the turn. | The user's message becomes `[{text}, {image_url: data:…}]` (gateway/run_turn_runner.py:1665-1683; agent/image_routing.py:533-563), at the image's own size (PNG, JPEG, GIF, WebP as sent, anything else made a PNG: :501-530), shrunk only after a provider refuses it for size (:394-397). |
| **A `computer_use` capture** (the agent's desktop, 1440x900 unless set: tools/bot_desktop/runtime.py:553-556) | The PNG, at most 1456 px on its long edge (tools/computer_use/tool.py:864), described by the auxiliary model with the screen's element list; the tool's result is JSON text with `vision_analysis` and `"vision_analysis_routed_via": "auxiliary.vision"` (:921-965). | The tool's result is the PNG itself, as it was captured (a `_multimodal` envelope: :710-720), sent as the `tool` message's content parts. |
| **`browser_vision`** | The auxiliary model's analysis (tools/browser_tool.py:1285-1296; tools/browser_tool_vision.py:85-). | The screenshot, made at most 256 KB and 1568 px, as the tool's result (tools/browser_tool_vision.py:52-82; tools/vision_tools_history_budget.py:25; tools/vision_tools.py:355-364). |
| **`vision_analyze`** (the agent asks) | The auxiliary model, told to "fully describe and explain everything about this image, then answer" the agent's question (tools/vision_tools.py:1012-1026, 887-925). | The image into the agent's context as the tool's result, at most 256 KB (:694-752, 593-614). |

Every tool takes one gate (tools/vision_tools.py:572-579): the mode is
`native` *and* the provider takes images inside a tool's result
(`_accepts_tool_result_images`, :559-569: a provider known to, among
them `openrouter`, `anthropic` and `openai` (:516-521), which `custom`
is not, or else the model's `supports_vision` again).

### What Hermes already does when it goes wrong

- **A model that reads none, mid-session.** Before every chat
  completion, when the model about to be called does not report vision,
  each image in the history is replaced by a description from the
  auxiliary model, made once per image and kept (agent/
  vision_message_prep.py:188-205, 92-131; called at agent/
  chat_completion_helpers.py:1456, 1501). A tool's image for such a model
  becomes its text summary; a capture's, an error asking for a model that
  reads images (:207-256). So a switch to `fallback_providers` that reads
  no images keeps working, as long as `auxiliary.vision` names a
  describer.
- **A provider that refuses images.** Known refusals ("no endpoints
  found that support image input", OpenRouter's; "unknown variant
  `image_url`, expected `text`", DeepSeek's; and others: agent/
  message_sanitization.py:395-416) strip the images and try again
  without them, remembered for that model for the session (:435-446).
- **A provider that refuses an image inside a tool's result** (a 400 on
  list-shaped tool content): the result keeps its text parts and is sent
  again, remembered (agent/turn_recovery.py:683-695; agent/
  vision_message_prep.py:258-294). The screenshot is then gone, not
  described.
- **Size.** An image a provider refuses as too large, in words Hermes
  knows (agent/error_classifier.py:197-), is shrunk once and sent again
  (agent/turn_recovery.py:671-681). A 413 is not that: it is "payload too
  large" (agent/error_classifier.py:1228), answered by compressing the
  conversation, then dropping tools' images, and failing the turn when
  neither helps (agent/turn_overflow.py:228-284). The image a person
  attached to the message being answered is in neither.
- **History.** A tool's native image rides every later call until the
  request passes 20 images or 24 MB, then the oldest go in batches
  (agent/image_eviction_policy.py:26-27; agent/context_compressor.py:
  1512-); compression keeps the newest 3 (:1144). A person's uploads are
  never evicted, only compressed away.

## What we configure today, and why

hermes-boot writes into every profile (images/hermes/boot/src/
hermes.rs:410-428):

```yaml
auxiliary:
  vision:
    provider: "custom"
    base_url: "http://model.fragment.internal/v1"
    model: "vision"
    api_key: "fragment-model"
```

Since commit `6c8cce88` (2026-10-05, "A vision model for Hermes'
screenshots: the route's `vision`, GLM-5.3 Flash"; Paul: "yes, vision
model for computer_use"; docs/cloudflare-v1.md:502-516, decision 23's
status). Then the medium tier ran GLM-5.3, which reads no images, and a
capture went to the agent's own tier's model; the fallback, Workers AI's
DeepSeek V4 Flash, reads none either. So every agent's images went to one
model that reads them, named by the route's name and not its id: an id a
tier runs is that tier's slot, whose call falls back to a model that may
read none, and `vision` has no fallback (hermes.rs:170-176; cell/src/
models.rs:264-269; docs/computers.md, Models).

What follows from it:

- Our explicit `auxiliary.vision` makes every agent `text`, whatever its
  model. An owner's own model too (docs/computers.md, "An agent's own
  model": "Vision … stays on the route's `vision`"): an agent on Claude
  Sonnet 5.5 through its owner's OpenRouter has its images described by
  GLM-5.3 Flash, on the platform's ledger.
- `vision` is Workers AI's GLM-5.3 Flash on Finite's deployment and on
  every preview (deploy/example.jsonc:130; crates/core/src/
  models.rs:105-112).
- Finite's tier model, OpenRouter's `z-ai/glm-5.3-flash`, reads images
  too (its catalog: text, image, video), and is never shown one.
- DeepSeek V4.1 Flash with images is on OpenRouter now
  (`deepseek/deepseek-v4.1-flash`: text and image in; 40 endpoints, 35 of
  them zero data retention), which decision 23's status found only on
  DeepSeek's own API.

**How to tell a hand-off.** An agent's `vision_analyze` is a step in the
chat (it asks when the note invites "a closer look"). A capture's result
says `vision_analysis_routed_via: "auxiliary.vision"`, which the model
may repeat ("the vision model says…"). The cell logs each call as
`model.settled` with `"tier": "vision"` (cell/src/models.rs:197-200),
and the owner's ledger has a row for it at the vision model's price
(GLM-5.3 Flash; on Finite's deployment the fallback's model too, so a row
alone does not tell the two apart). An attachment's description is no
step in the chat: it is made before the turn.

## If the main model saw images itself

### Through our model route

- **Images pass as written.** `bound` changes only the model, the token
  cap, the effort and the stream (crates/core/src/models.rs:903-931); an
  OpenAI-compatible API is sent the client's `messages` among
  `CLIENT_KEYS` (:224-240); Workers AI gets the same input (cell/src/
  models.rs:466-472). Nothing to change to let them through.
- **Size.** A call is at most 6 MiB (`MODEL_BODY_MAX_BYTES`, crates/core/
  src/models.rs:126-139), at the intercept (cell/src/computer.rs:2889)
  and at the route (cell/src/models.rs:256-259, 553), sized for Hermes'
  one vision call of an image it shrank to 5 MiB. Native, a person's
  photo goes at its own size (the bridge takes 8 attachments of up to 25
  MiB a message: images/bridge/src/limits.rs:94-97; a HEIC becomes a PNG,
  larger), and each screenshot rides the conversation until Hermes' 20
  images or 24 MB. Our cap comes first: its 413 makes Hermes compress,
  which recovers from screenshots at the cost of a compression, and fails
  the turn for a photo over it on its own.
- **Metering.** An image is input tokens in the answer's
  `usage.prompt_tokens`, which `usage_of` reads and the book prices
  (crates/core/src/models.rs:946-957; crates/core/src/price.rs:291-309):
  nothing new to meter. What changes is how much. Native, an image's
  tokens are paid on every later call that still carries it; `text` pays
  one vision call and then carries some 150 words. The book prices an
  OpenRouter row's cached input at its prompt ceiling (price.rs:270-298),
  so the repeat is not cheaper for the person when the provider caches.
  How many tokens a 1440x900 screenshot is on DeepSeek V4.1 Flash is not
  measured.
- **The route's fallback.** A tier's call whose model fails before
  answering (429, a 5xx, unreached; on OpenRouter also 401 to 404) is made
  again, the same body, images included, on `models.fallback` (cell/src/
  models.rs:264-270, 381-403; crates/core/src/models.rs:745-753). On
  Finite's deployment that is Workers AI's GLM-5.3 Flash, which reads
  images; on a preview, DeepSeek V4 Flash, which reads none, and its
  refusal is in Workers AI's words, which Hermes may not know as one (the
  fake answers 400: crates/fakes/src/workers_ai.rs:46-48, 125-129).
- **OpenRouter's routing.** OpenRouter answers 404 "No endpoints found
  that support image input" when its routing leaves no endpoint that
  takes images. Finite's routing (zero data retention, fp8 or bf16, a
  price ceiling, `require_parameters`: deploy/example.jsonc:118-131)
  narrows the endpoints, and a 404 falls back, so an image call could
  quietly land on the fallback every time. OpenRouter lists no modality
  per endpoint.
- **DeepSeek V4.1 Flash as a tier** is not priced on master (crates/core/
  src/price.rs:291-309), so `Models::parse` refuses it (crates/core/src/
  models.rs:423-428): the tier's move needs its row, in its own change.

### Through Hermes

- **Per model.** `providers.custom.models.<id>.supports_vision: true`,
  where hermes-boot already writes the tier model's
  `stale_timeout_seconds` (hermes.rs:455-456). Not `model.supports_vision`,
  which Hermes reads first whatever model is active (agent/
  image_routing.py:158-161), so it would speak for the fallback too.
- **And `agent.image_input_mode: native`**, since `auxiliary.vision`
  stays: it describes images for a model that reads none (the fallback,
  after Hermes' own switch). Without it Hermes picks a describer itself,
  among providers we configure none of.
- **Images inside a tool's result.** A screenshot goes back as the `tool`
  message's content parts. OpenAI's chat completion shape has a tool's
  content as text; OpenRouter passes it to the provider; a provider may
  refuse it (Hermes then drops the screenshot, above), take it, or take
  it and ignore the image, the worst, where the model answers as if it
  had seen. Unknown for DeepSeek V4.1 Flash's endpoints and for Workers
  AI's GLM-5.3 Flash.
- **Not verified**: that the gateway reads `agent.image_input_mode` from
  the profile's own config when it routes an inbound image under
  multiplexing (it reads it with `load_config`, gateway/
  run_inbound.py:1811-1854). Today's `text` holds either way.
- **An owner's own model** is a `custom` profile at OpenRouter (hermes.rs:
  389-396): the same two lines would turn it on, and Hermes cannot find
  out alone. Its calls go to OpenRouter directly, not through the route:
  no 6 MiB cap there, OpenRouter's own.

## Proposal

**0. Probe first** (no code; a few test cents, Paul's or the coordinating
session's to spend). From an agent's terminal on the preview (`curl
$FRAGMENT_MODEL/v1/chat/completions -H 'x-fragment-agent: <agent>'`, so
the call is routed and metered as an agent's), a few calls with tools
offered: a PNG in the user's message; a PNG in a `tool` message after a
tool call; a 3 MB photo. Each on DeepSeek V4.1 Flash under Finite's
routing (on a preview whose `models` names it) and on Workers AI's
GLM-5.3 Flash (the fallback). Read the status, the provider that served
it, `prompt_tokens`, and whether the answer names a word drawn only in
the image. This says whether native can work on our routing at all, and
what an image costs.

**1. The platform says which of its models read images.**
- A `models` slot may say `"images": true`; one that does not reads
  none. An id is one model, so its slots agree (as `same_place` already
  holds them to one place); the `vision` slot reads images, or the deploy
  and the cell refuse it (crates/core/src/models.rs, `Upstream`,
  `Models::parse`). The defaults' GLM-5.3 Flash does.
- The guest's view's `models` (`ModelIds`: crates/proto/src/
  computer.rs:72-81; images/bridge/src/api.rs:98-106) gains `images`,
  the ids that read them. An older image ignores the field.
- The route: a tier's call that carries an image falls back only to a
  fallback that reads images; otherwise the first model's failure is the
  call's, and Hermes' own retry and fallback switch take it, describing
  images for a model that reads none (cell/src/models.rs:264-269).

  Said by the deployment, not read from OpenRouter's catalog: the
  deployment's routing decides which endpoints serve a call, and a model
  whose catalog says "image" may have no endpoint under that routing that
  takes one (step 0 is how the deployment knows). And nothing is fetched
  at boot, nor written into Hermes' home while the computer is held (why
  its model catalog is off: hermes.rs:279-286).

**2. hermes-boot.** A tier agent whose tier's id is in `images` gets
`agent.image_input_mode: "native"` and
`providers.custom.models.<id>.supports_vision: true`; the fallback's id
gets `supports_vision: true` when it is in `images` too, so it sees
after Hermes' switch as well. `auxiliary.vision` stays. An agent whose
model reads none is as today.

**3. Room, before step 2 ships** (decision for Paul). Either the bridge
hands Hermes an attached image a call can carry (a copy at most 1568 px
on its long edge, Hermes' own size for an image it keeps in a
conversation, the original kept for the agent's files), or the route
takes larger tier calls (what OpenRouter and Workers AI take is to
learn; the cell holds a body in memory, 128 MB an isolate). Screenshots
fill a 6 MiB call after some tens: raise the cap for a tier's call, or
accept a compression there.

**4. Own models.** The catalog row's `offer` may say `"images": true` of
a model (deploy/example.jsonc:235-242); settings writes it into
`agent.json`'s `model` with the pick, and hermes-boot reads it with the
model (`OwnModel::of`, hermes.rs:79-84) and writes the same two lines. A
model named by hand says nothing and stays `text`. Or hermes-boot reads
OpenRouter's public catalog (`architecture.input_modalities`) through the
computer's egress: no config, a network read at boot.

**The config-only alternative**: point `vision` at a better describer
(DeepSeek V4.1 Flash on OpenRouter, once priced). Still a hand-off: the
describer sees an attachment without the conversation, in 2 to 4
sentences, and the main model asks again with `vision_analyze` for more.

## What to test

- **hermes-boot** (hermes.rs's tests): a tier whose id is in `images`
  writes `image_input_mode: "native"` and `supports_vision: true` under
  its id, and keeps `auxiliary.vision`; one not in it writes neither; the
  fallback's line when its id reads images.
- **crates/core**: `Models::parse` with `images`; a `vision` slot that
  reads none refused; two slots of one id that disagree refused;
  `ids_by_name` carries `images`.
- **The Docker lane** (real Hermes, the scripted model: images/bridge/
  tests/docker.rs, tests/support/model.rs), the view naming `CHEAP_ID` in
  `images`: an attached PNG reaches the tier's call as an `image_url` part
  of the user's message, and no call names `vision`; "look at your
  screen" sends the capture to the next tier call as a `tool` message
  with an `image_url` part, and no call names `vision` (docker.rs:
  1263-1277 asserts the opposite, rightly, for an agent whose model reads
  none, which keeps it); a `stall` moves the agent to `FALLBACK_ID`, not
  in `images`, whose call carries no image and is preceded by a `vision`
  call describing it. It also settles where the gateway reads
  `image_input_mode` from.
- **The e2e** (local, the fake): in the hermes lane, a cheap-tier agent
  (GLM-5.3 Flash, which the fake lets read images) answers from an
  attached image what only its pixels say, the image on the tier's call
  and none on `vision` (hermes.rs:743-776 is today's medium-tier check);
  in the ledger lane, a tier's call carrying an image whose model fails
  falls back only to a fallback that reads images.
- **agent-smoke** (hosted, a real model): a step that uploads an image
  with a word drawn in it and asks for the word: the answer has it, the
  chat shows no `vision_analyze` step, and no `vision` call was made (no
  ledger row at the vision model's price in the step, where that model is
  not the fallback's too). And the same with a word shown only on the
  agent's desktop. These tell "it saw it itself" from "it handed it off",
  Paul's question.

## Open questions

1. Native at all? The model sees the image in the conversation's
   context, and asks no one; it pays the image's tokens on every later
   call that carries it, and needs room our cap does not give.
2. Where "reads images" is said: the deployment's `models` (proposed), a
   table in the code beside the price book, or OpenRouter's catalog.
3. Room: the bridge sizes attachments down, or the route takes larger
   tier calls; and how many screenshots a call may carry.
4. The route's fallback with an image: pass the failure to Hermes
   (proposed), or have the cell take the images out for a fallback that
   reads none.
5. Own models: `images` in the catalog's offer (proposed), OpenRouter's
   catalog at boot, or text for them as today.
6. `vision`, the describer: stay on Workers AI's GLM-5.3 Flash (also the
   fallback, so it outlives OpenRouter being down), or move to DeepSeek
   V4.1 Flash with the tiers.
7. Should a hand-off show? An attachment's description is no step in the
   chat today.
