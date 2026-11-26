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
  door of this fragment's life (its name and its npub's digits, so a name
  made again reaches another), never out to the internet; no other
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
  keeps it, with its QR code as the door draws it, unless an unlink came
  after it (each numbered, `asking`, as the door is told), for its owner
  alone (`state`, an owner's query: never a run's output, which every
  viewer reads, its agent included); it then watches for their app to
  answer it, about 8 minutes, and keeps whom it linked (`linked`); `check`
  looks once, later; `unlink` ends it. A step that fails says why in
  `state` (`failed`), the door's code with it.
- **Its setup page** (`site/setup.html`), which Settings opens in a dialog
  (or a tab, where the frame can't sign in), as Telegram's: its owner's
  alone, nothing to fill in. It sets the inbox's secret, made by the
  browser, and `connect`s as the agent's name Settings gives it, asks a
  link, and shows the invitation's QR code with its link to open or copy;
  then whom it linked, with Unlink. It follows `state` live.
- **The door (the binding):** its owner binds it to their agent (`POST
  /api/f/{name}/door`, in person), which makes the agent a viewer here.
  Then `intake`, the inbox's trigger alone, publishes each text message
  its own door delivers (each names its door, kept at `connect`: an
  earlier life's door is no one's here), once by its SimpleX id (`seen`), as its owner's
  words to the agent (`job.door`: `relayed` `{via: "simplex", for,
  agent}`, `to` that agent): no door key, no signature, since the
  platform knows which fragment publishes. A message's file (a voice
  message, a photo, a file), which its door holds, is kept first as one
  of this chat's blobs (`job.fetch(…/file/<sha256>, {keep: true})`) and
  goes as its attachment, and its door is told it was kept (`kept`, by
  the message's id); one kept by none is said so in its words.
  Unbound, it tells their app so; a message with nothing to carry, that
  only text and files are carried. `mirror` sends each reply its bound
  agent posts here (not its interim words), and each question it asks
  its owner (a card on `work`, worded by `applib/question.mjs` as the
  computer's own SimpleX words one), through the door, by its record,
  asking again until the door's runner says it is sent, its link ended,
  or there is none. Their next message answers a question that takes
  words; one that needs a button says so. A reply's files (which the
  agent, a viewer here, uploads), each given to the door first
  (`job.fetch(…/file/<sha256>, {method: "PUT", blob})`), go after its
  text; one the door does not take, or a door whose runner carries none,
  stays in the chat, said so.

Settings makes one for an agent (`Set up`, bound as its door as it is
made) on every seat, but where an always-on computer that does SimpleX
keeps the agent's own; an agent's existing SimpleX chat (its computer's)
is not moved here (docs/technical-debt-ledger.md, "Two SimpleX doors").
