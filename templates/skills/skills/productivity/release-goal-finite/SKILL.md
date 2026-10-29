---
name: release-goal-finite
description: Run the sandbox release-note durable-goal demonstration across fresh turns, a scheduled review and a late owner approval. Only for the releasegoal template and a local fake vendor.
---

# Sandbox release note

Use `fragment new <folder> --template releasegoal`, create and deploy that
folder using the ordinary CLI. This is one immutable note per fragment.
Read its README and `fragment guide` for operation/schedule syntax.
Never point this demonstration at a real service.

Every invocation is a fresh turn: query `state` before doing anything,
advance one phase, checkpoint through an operation, and exit. Never resume
or replay an interrupted Hermes turn. Native memory, skills, terminal and
clarify remain available; memory is context, not goal authority.

- Initially: write the note through `draft` with a local fake URL and a
  globally unique stable key (include the full fragment name and a fresh
  goal nonce). Do not derive keys from turns, runs, attempts or time of retry.
  Add exactly one routine on your agent's `tasks`, targeting the current
  chat, for tomorrow's UTC date/hour/minute. Its text names the goal
  fragment and asks to use this skill. Repeating the identical schedule add
  is safe. Checkpoint is `scheduled`; exit. The artifact is in Fragment's
  files, not solely on the computer's disk.
- Scheduled follow-up: remove that routine (list and match its exact goal
  fragment/text first), then call `review`. Ask once with native `clarify`:
  `Publish the sandbox release note?` / `Publish` / `Keep private`.
  The checkpoint already says `waiting_owner`. Hold no process or child
  task open while waiting. If a cut happens, inspect the chat's original
  card; do not ask a second question. A lost card before it was recorded
  requires owner recovery, not an automatic guessed answer.
- Owner answer, including the stack's fresh late-answer turn: read state,
  verify the chosen answer belongs to the original question in this chat,
  call `approve` with that choice and `source` identifying chat/turn/prompt
  or answer record. Never infer approval from a restart or schedule fire.
  End this phase. If needed, the owner sends a fresh continuation message.
- Continuation: read state. Only `approved` permits `deliver`, with operation
  id `release-delivery-v1`. Inspect the run until succeeded or held; a held
  run needs diagnosis and explicit replay, not a new key. Read state again
  and report the vendor receipt. `done` and `canceled` do nothing further.

A terminal tool completing establishes only that its CLI command completed.
Inspect the job and goal receipt before claiming the external effect succeeded.
The stack's structured `turn.step` evidence records supported terminal call
identity/outcome/source; link that work record together with the job run and
`effect.key`/`effect.receipt`. Missing evidence is not success.
