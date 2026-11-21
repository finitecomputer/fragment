# simplex

The SimpleX connector (docs/api.md, "An agent's doors": a hosted door), a
blessed connector template (decision 40; `blessed::CONNECTORS`), shaped as
Telegram's is (templates/telegram): a chat with its owner's agent that is
also their conversation with their SimpleX app. Its other end is the
platform's SimpleX door: a small container of its own, always on, running
SimpleX's core, its databases saved after every change (cell/door.mjs,
images/simplex-door, docs/computers.md "F"). The agent's computer is not
that container: it sleeps as any does, and wakes for a message here as
for any other. Its page is the chat's own (`site/index.html` and every
file it loads are the chat template's, linked, so no copy drifts).

- **Its owner's alone to edit**, as every connector: no one but its owner
  sets its secrets, writes its pages, deploys its code or calls an
  operation of an owner's; a fork, or any other code, makes it no door
  again, for good, and removes its secrets.
- **Its door is its own.** Its jobs reach the door at
  `https://door.fragment.internal/<ask>`, which the platform sends to the
  door named as this fragment is, never out to the internet; no other
  fragment's fetch of that host is sent anywhere (`door_route`,
  crates/core/src/door.rs). A door is started only where the deployment
  runs doors (its config's `doors`); elsewhere `connect` says so and keeps
  nothing.
- **Setup, its owner's:** the inbox's secret (`SIMPLEX_DOOR_SECRET`, toward
  `door.fragment.internal`), then `connect {name}` starts the door, its
  SimpleX profile `name`, and gives it that secret, filled at the
  platform's egress. The door delivers each of its owner's messages to
  this chat's inbox with it (`fragment.json` `inbox`), as a webhook does,
  in order, one at a time; what the inbox did not take at once, the door's
  watch sends again every 30 s.
- **The SimpleX link:** `link` asks the door for a one-time invitation and
  keeps it for its owner alone (`state`, an owner's query: never a run's
  output, which every viewer reads, its agent included); `unlink` ends it.
- **The door (the binding):** its owner binds it to their agent (`POST
  /api/f/{name}/door`, in person), which makes the agent a viewer here.
  Then `intake`, the inbox's trigger alone, publishes each text message
  the door delivers, once by its SimpleX id (`seen`), as its owner's
  words to the agent (`job.door`: `relayed` `{via: "simplex", for,
  agent}`, `to` that agent): no door key, no signature, since the
  platform knows which fragment publishes. Unbound, it tells their app
  so; another kind of message, that only text is carried for now.
  `mirror` sends each reply its bound agent posts here (not its interim
  words) through the door, by its record, asking again until the door's
  runner says it is sent, its link ended, or there is none.

Made only by a test or the e2e for now: no Settings page makes one yet,
and an agent's existing SimpleX chat (its computer's) is not moved here
(docs/technical-debt-ledger.md, "Two SimpleX doors").
