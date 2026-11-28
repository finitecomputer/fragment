# slack

The Slack connector (docs/api.md, "An agent's doors": a hosted door), a
blessed connector template (decision 40; `blessed::CONNECTORS`): a chat
of its owner's that is also their agent's own Slack app (FIN-508, Paul's
decisions of 2026-11-16: an app an agent, one chat a workspace, a session
a room, anyone in a room may ask). Its page is the chat's own (its site's
files are the chat template's, linked); its owner's setup is a page of its
own (`site/setup.html`), which Settings opens in a dialog, or, where that
frame can't sign in, in a tab. Its rules as a connector are Telegram's
(templates/telegram/README.md): its owner's alone to edit, made and bound
from Settings, its secrets theirs, a fork no door again.

- **Setup, its owner's, on its setup page:**
  - Create the app: a link to Slack's "create an app from a manifest"
    (`setup.js` `manifest`), filled in with the agent's name, the scopes
    below, and this chat's inbox as the app's Request URL for
    `app_mention` and `message.im`. Its owner picks the workspace and
    installs it (a workspace that needs an admin's approval asks one).
    Slack's free plan installs at most 10 apps.
  - Its bot token (OAuth & Permissions) and signing secret (Basic
    Information), pasted, each set as this fragment's secret toward
    `slack.com` (`PUT __secrets/{KEY}`: written, never read back, never
    through its code or a model). `connect` checks the token
    (`auth.test`) and keeps its workspace and bot user.
  - Slack's check of the Request URL: the inbox takes a delivery only
    signed by the signing secret (`fragment.json` `inbox`, `hmac`), and
    answers Slack's `url_verification` with its challenge (`handshake`).
    Slack may check before the secret is here: the page says, until a
    signed delivery comes (`state.verified`), to press Retry by the
    Request URL in the app's Event Subscriptions.
  - Pairing: `pair` makes a code (`pair <16 hex>`, single use, ten
    minutes) that its owner sends the bot in a Slack DM; the account that
    sends it is proposed, and its owner confirms it on the page. Two
    accounts sending one code propose no one. Its user and DM ids are
    bound; its name is only shown. Another workspace's or bot's token
    forgets the account, and everything it remembered.
- **Scopes:** `app_mentions:read`, `chat:write`, `im:history` (what it
  takes and says), `channels:history`, `groups:history` (a channel's
  context), `channels:read`, `groups:read`, `users:read` (rooms' and
  people's names), `reactions:write` (👀).
- **What comes in** (`intake`, the inbox's trigger alone): a message that
  mentions the bot in a channel it was added to, or a DM, from a person
  of the workspace it connected to; never a bot's (its own included), an
  edit, a deletion or another subtype. Each message once (`seen`, by its
  channel and `ts`: Slack sends again until it hears a 2xx, and a DM's
  mention twice). While the door is open (`job.door`), it is published
  here to the agent (docs/chat-records.md, "A message heard in a room"):
  - its owner's DM, as a relayed message: their words, in the chat's own
    conversation;
  - its owner's words in a channel, a relayed message with its `room`;
  - anyone else's, as someone heard (`heard: {via: "slack", from: <user
    id>, name}`), in its `room` (a channel, `#name`, or their DM, `DM`).
  A channel's message comes with what was said there just before
  (`context`: in a thread its earlier replies, otherwise the channel's
  latest; at most 20 lines, 600 characters each, 16 KiB in all; a
  thread's read up to its first 100 replies), less the
  bot's own words and the messages that mentioned it, which the room's
  conversation has. Slack's mentions and links are said by name.
  👀 goes on the message as it is taken.
- **Who may ask, and the cap:** anyone in a room the bot is in, and anyone
  who DMs it, on its owner's computer and credits. Others' messages in a
  room are taken at most 30 an hour (`ROOM_TURNS_PER_HOUR`); past them,
  the first says once that the agent is resting, and the rest are left
  until the hour turns. Its owner's are never counted. Its owner pauses
  it all as any fragment's runs pause, or unlinks it.
- **What goes out** (`mirror`, each reply its bound agent posts here,
  not interim): in the thread of the message its turn answers (`took`,
  kept as its record is published: the turn's id is the record's), 👀
  off once it goes; a reply whose turn it doesn't know, in its `room`, or
  else its owner's DM. As Markdown (`markdown_text`), at most 4 messages
  of 12,000 characters; the rest, and any file, stays in this chat.
  A question the agent asks its owner (a card on `work`, worded by
  `applib/question.mjs`, every connector's) goes where their answer
  counts: its turn's thread (their words in a room answer only a question
  asked there), or else their DM. One asking anyone else stays here.
- **Remembered, each bounded:** messages taken (2000), names (2000, a
  day each), turns' threads (1000), and each room's count this hour.

Slack carries it all as plain text to and from its servers, and everyone
in a room reads what the agent says there. Text only, for now. Not built
yet: edits and deletions, files in or out, the agent speaking first in a
room, Slack Connect and Enterprise Grid, and the app's own picture (its
owner sets it in Slack). Off on a self-hosted stack, whose platform Slack
can't reach (docs/self-host.md).
