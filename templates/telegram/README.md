# telegram

The Telegram connector (docs/api.md, "An agent's doors": a hosted door), a
blessed template (decision 40): a chat with its owner's agent that is also
their direct messages with a bot of their own. Its repo names the
template; the platform's release serves the rest. Its page is the chat's
(its files the chat template's, linked) with the owner's setup above it
(`site/telegram.js`).

- **Setup, its owner's, on its page:** the bot's token, pasted (from
  @BotFather), and a webhook secret the browser makes, each set as this
  fragment's secret toward `api.telegram.org` (`PUT __secrets/{KEY}`:
  written, never read back, never through its code or a model); then
  `connect` checks the token (`getMe`) and points the bot's webhook at
  this chat's inbox (`setWebhook`, with the secret as its
  `secret_token`). The inbox takes a delivery only with that secret in
  Telegram's header (`fragment.json` `inbox`), so no member, the agent
  included, can forge one.
- **Pairing:** `pair` makes a link that opens the bot (`/start <nonce>`,
  single-use, ten minutes); the account that answers is proposed, and its
  owner confirms it on the page (`confirm`, an owner's operation an agent
  cannot call, whomever it acts for). Two accounts answering one link (a
  member who read its nonce in the inbox, racing the owner's own
  `/start`) propose no one, and the owner pairs again. Its numeric user and chat ids are
  bound; its name is only shown. Another bot's token forgets the account.
- **The door:** its owner links it to their agent on the page (`__door`),
  which makes the agent a member. From then on `intake`, the inbox's
  trigger alone, publishes each direct message from the paired account
  here as their words to the agent (`relayed: {via: "telegram", for, agent}`),
  which the platform takes only from a connector bound as that agent's
  door, while it runs this template unforked and the deployment allows
  `telegram`. `mirror` sends each reply an agent posts here (not its
  interim words) to the paired chat, in at most four messages.
- **What it is not:** end-to-end encrypted (Telegram's servers, and the
  platform, carry the text), a group bot, or a carrier of files and voice
  (a reply's files are named, not sent; a message without text is
  answered so). Telegram topics, several chats per connector, come later:
  each connector chat is its own door.
