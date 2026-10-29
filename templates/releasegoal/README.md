# Sandbox release goal

CLI scaffold (`fragment new note --template releasegoal`), intentionally
not a blessed kind or a general goal engine. Pair with the managed
`release-goal-finite` Hermes skill. The image only adds the CLI scaffold; its runtime is unchanged.

One desired outcome: publish an immutable release note to a **local fake**
after tomorrow's review and one owner approval. `draft` commits
`artifacts/release.md` and the goal identity; `state` is authoritative for
status, next action, wake condition, artifact paths, approval reference and
effect key/receipt. `review`, `approve`, `deliver`, and `receipt` advance it.
A new note needs a new fragment and key. All operations require editor access.
The skill verifies the owner's card answer; the app trusts its editors and
does not independently authenticate a chat approval reference.

Exactly one logical scheduled follow-up uses the agent's existing routine
records, removed on the review turn. Scheduled records recur: a crash before
removal can deliver again, but `review` cannot advance twice. Routine creation
and the draft are not an atomic transaction. Reconcile the exact routine on
recovery; do not add a differently worded replacement. Routine fires over an
hour old are skipped by the existing bridge. A missing card or skipped wake
needs an owner continuation; this slice promises no autonomous liveness.

The only egress allowed here is `http://127.0.0.1:<port>/release`, served by
the e2e fake. Hosted egress refuses loopback. The fake returns
`{"id":"sandbox-release","key":"<Idempotency-Key>"}`, retaining one result
per key and rejecting another payload for that key. `deliver` is a short
ordinary job: read checkpoint, fetch, save receipt. It never wraps Hermes.
Its explicit key stays the same across different runs, replay and restart,
even if the vendor acted before the response was kept. Exactly-once behavior
is conditional on vendor deduplication and key retention; an arbitrary vendor
cannot offer it. A lost response is not evidence of failure or success.

Hermes turns are fresh decisions over the checkpoint. Neither native session
history nor a restored `/data` is the authority for the external effect.
The #654 delegated-child hold, #660 late approval turns and #664 structured
tool evidence remain unchanged. Inspect terminal `turn.step` observations,
the delivery run's steps and the goal receipt together. No paid calls or
real publications belong in this demonstration.

Validation: `cargo xtask e2e --only release-goal` runs the real app under
workerd and loses the fake vendor's first answer after commit. It restarts
the cell before review, while waiting for the owner and after the answer.
`cargo xtask e2e --only hermes` also exercises the routine/card/continuation
on real Hermes: its model alone is scripted, the test drives the owner's
answer and routine removal, and the vendor is local. The routine's minute
is advanced with the existing test lever; this does not wait a literal day
or prove an unscripted model will follow the skill. After killing the
approved turn the test sends an explicit fresh continuation; there is no
automatic recovery scheduler for interrupted Hermes turns.
