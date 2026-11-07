# telegram

The Telegram connector (docs/api.md, "An agent's doors": a hosted door), a
blessed connector template (decision 40; `blessed::CONNECTORS`): a chat
with its owner's agent that is also their direct messages with a bot of
their own. Its repo names the template; the platform's release serves the
rest. Its page is the chat's own (`site/index.html` and every file it
loads are the chat template's, linked, so no copy drifts from it). Its
owner's setup is a page of its own (`site/setup.html`), which Settings
opens in a dialog (Paul, 2026-11-07: the setup flow is in Settings), or,
where that frame can't sign in, in a tab.

- **Its owner's alone to edit.** As a connector, no one but its owner edits
  it, from its making: every other caller, its agent acting for its owner
  included, is at most a viewer here, so no one else sets its secrets,
  writes its pages, deploys its code or calls an operation of an
  editor's; nor does anyone else share it (no agent sets its visibility,
  members or invites, or holds its share link, whomever it acts for); and
  a fork, or any other code, makes it no door again, for good, and
  removes its secrets (its bot's token and webhook's secret are not the
  new code's to send). So no one but its owner, and Telegram, holds what
  makes its words theirs.
- **Made from Settings:** an agent's Set up, with no Telegram chat bound
  as that agent's door yet, makes one and binds it (below), then opens its
  setup page; with one, Set up opens that chat's.
- **Setup, its owner's, on its setup page:** the bot's token, pasted (from
  @BotFather), and the first time a webhook secret the browser makes,
  each set as this fragment's secret toward `api.telegram.org` (`PUT
  __secrets/{KEY}`: written, never read back, never through its code or
  a model); then `connect` checks the token (`getMe`) and points the
  bot's webhook at this chat's inbox (`setWebhook`, with the secret as
  its `secret_token`); `state`'s `checked` counts its answers, so the
  page sees one come. The inbox takes a delivery only with that secret in
  Telegram's header (`fragment.json` `inbox`); only its owner may set it,
  so no one else can forge a delivery. One bot's token in two connectors:
  the second `connect` takes the bot's webhook from the first.
- **Reconnecting** (the bot stopped, or another bot): Settings, Set up,
  New token, and the token pasted again. The webhook's secret is kept, so
  Telegram's deliveries still come in while a token it refused is put
  right; the same bot keeps its account paired, another forgets it.
- **Pairing:** `pair` makes a link that opens the bot (`/start <nonce>`,
  single-use, ten minutes); the account that answers is proposed, and its
  owner confirms it on the setup page (`confirm`, an owner's operation). Two
  accounts answering one link (a member who read its nonce in the inbox,
  racing the owner's own `/start`) propose no one, and the owner pairs
  again. Its numeric user and chat ids are bound; its name is only shown.
  Another bot's token forgets the account, and the updates it took.
- **The door:** Settings binds it to its agent as it makes it (`POST
  /api/f/{name}/door`: its owner, in person), which makes the agent a
  viewer here; its setup page reads `__door` only to say so when it is
  not bound. No page of it unlinks it for now (Paul, 2026-11-07:
  feature-limited is fine): Unpair stops what it relays. From then on
  `intake`, the inbox's trigger alone, publishes each direct message from
  the paired account here as their words to the agent (`job.door`: its
  `relayed`, `{via: "telegram", for, agent}`, `to` that agent), which the
  platform takes only from a connector its owner bound as that agent's
  door, while it runs this template unforked and the deployment allows
  `telegram`. `mirror` sends each reply an agent posts here (not its
  interim words) to the paired chat, in at most four messages, while the
  door is open. Unlinked, or turned off by the deployment, nothing is
  relayed or sent (the inbox still keeps what Telegram delivers, as any
  inbox does); unlinked, Telegram is told so, and the agent stays a
  viewer of the chat, its words there going nowhere else.
- **Marked so:** its setup page and Settings tag it "Experimental" and "Less secure" (Paul,
  2026-10-31), the latter literally: Telegram's servers read its messages,
  where SimpleX's (tagged "Secure" in Settings) are end to end.
- **What it is not:** end-to-end encrypted (Telegram's servers, and the
  platform, carry the text), a group bot, or a carrier of files and voice:
  the agent, a viewer here, attaches no file (its reply says it could
  not), and a message without text is answered so. Telegram topics,
  several chats per connector, come later: each connector chat is its own
  door.
