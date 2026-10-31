# telegram

The Telegram connector (docs/api.md, "An agent's doors": a hosted door), a
blessed connector template (decision 40; `blessed::CONNECTORS`): a chat
with its owner's agent that is also their direct messages with a bot of
their own. Its repo names the template; the platform's release serves the
rest. Its page is the chat's (its files the chat template's, linked) with
the owner's setup above it (`site/telegram.js`).

- **Its owner's alone to edit.** As a connector, no one but its owner edits
  it, from its making: every other caller, its agent acting for its owner
  included, is at most a viewer here, so no one else sets its secrets,
  writes its pages, deploys its code or calls an operation of an
  editor's; and a fork, or any other code, makes it no door again, for
  good. So no one but its owner, and Telegram, holds what makes its words
  theirs.
- **Setup, its owner's, on its page:** the bot's token, pasted (from
  @BotFather), and a webhook secret the browser makes, each set as this
  fragment's secret toward `api.telegram.org` (`PUT __secrets/{KEY}`:
  written, never read back, never through its code or a model); then
  `connect` checks the token (`getMe`) and points the bot's webhook at
  this chat's inbox (`setWebhook`, with the secret as its
  `secret_token`). The inbox takes a delivery only with that secret in
  Telegram's header (`fragment.json` `inbox`); only its owner may set it,
  so no one else can forge a delivery. One bot's token in two connectors:
  the second `connect` takes the bot's webhook from the first.
- **Pairing:** `pair` makes a link that opens the bot (`/start <nonce>`,
  single-use, ten minutes); the account that answers is proposed, and its
  owner confirms it on the page (`confirm`, an owner's operation). Two
  accounts answering one link (a member who read its nonce in the inbox,
  racing the owner's own `/start`) propose no one, and the owner pairs
  again. Its numeric user and chat ids are bound; its name is only shown.
  Another bot's token forgets the account, and the updates it took.
- **The door:** its owner links it to their agent on the page (`__door`),
  which makes the agent a viewer here if it was no member. From then on
  `intake`, the inbox's trigger alone, publishes each direct message from
  the paired account here as their words to the agent (`job.door`: its
  `relayed`, `{via: "telegram", for, agent}`, `to` that agent), which the
  platform takes only from a connector its owner bound as that agent's
  door, while it runs this template unforked and the deployment allows
  `telegram`. `mirror` sends each reply an agent posts here (not its
  interim words) to the paired chat, in at most four messages, while the
  door is open. Unlinked, or turned off by the deployment, nothing is
  relayed or sent (the inbox still keeps what Telegram delivers, as any
  inbox does); unlinked, Telegram is told so, and the membership the
  link gave goes.
- **What it is not:** end-to-end encrypted (Telegram's servers, and the
  platform, carry the text), a group bot, or a carrier of files and voice:
  the agent, a viewer here, attaches no file (its reply says it could
  not), and a message without text is answered so. Telegram topics,
  several chats per connector, come later: each connector chat is its own
  door.
