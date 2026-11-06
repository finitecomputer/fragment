# Native iOS client and pairing

Status: **native CLI-key pairing and scoped web sessions implemented; hosted rollout and TestFlight remain**.
The client is in `ios/`, SwiftUI on iOS 26+, with URLSession HTTP/WebSockets.
The app shares wire shapes with Fragment, not its implementation, and talks to
a real platform only: a deployment, or `cargo xtask dev` in development.

## Problem and acceptance

Make Finite.Computer feel at home on iPhone: open your agent, resume a chat, send a
message, see its reply stream, and open an ordinary app without leaving the
conversation. Follow the later, simpler Finite iOS design and the existing
Fragment mobile colours. The signed-in home is a native fullscreen version
of Fragment's Command-K launcher (Paul, 2026-10-14): one input searches chats,
apps and message text; the first action and Return start a new chat with the
complete input. Search hints alone are capped at eight words and 256 bytes;
older search responses never replace newer results. Empty input stays quiet.
Paul, 2026-11-04: use web organization with native iOS presentation. Open on
its search launcher over the web desktop wallpaper. A left sidebar groups chats
by agent (multiple agents supported), with History, Your Apps and Settings. Chat has
sidebar left, title, search/new chat and Your Apps right; chat actions live in the
agent hero menus in the sidebar and chat-row context menus, with no chat ellipsis; Your Apps uses a native list over the shared photo background
and Add an app opens the built-in prompt sheet, without a separate suggestion empty state.
Your Apps has Manage Apps at top right, with all apps (including hidden and archived)
searchable in a native sheet. Per-app settings mirror web sharing and permissions,
personal show/hide/archive, naming, icons and preview generation.
Use system typography and system glass controls, a single glass composer with
aligned 44-point controls and no recipient caption, and actual fragment icons
in Your Apps. The installed hand icon is a layered Icon Composer asset; product copy is Finite.Computer,
Finite for short, and messages name their actual agent. App viewers are native large sheets with a visible drag handle and swipe-down
dismissal, without a Done button; chat state is retained. Native history, settings categories, attachments, voice
memos, reply/copy/share and group addressing follow the existing wire contracts.
No Rust FFI, embedded chat webpage, model key or parallel chat protocol.

Acceptance: real HTTP
and WebSockets, a working composer above the keyboard, whole-text drafts
replaced once by committed replies, Stop, retry with the original id,
reconnect/catch-up without duplicates or regressing either channel's cursor,
earlier history independent of live catch-up, selectable native Markdown,
light/dark, and ordinary fragments in dismissible WKWebView sheets. Build
with Swift 6 strict concurrency and warnings as errors. See `ios/README.md`
for commands and evaluation. Unit tests on a stub of the platform's answers
lose a committed post's acknowledgement, refuse a post and drop a stream to
test recovery.

## Existing contract; what is missing

The live service uses these routes already (`docs/api.md` is authoritative):

| Need | Existing API |
| --- | --- |
| Person, owned agent | `GET /api/identities/me`, `GET /api/fragments`, `GET /api/computers` (its agents' order and identities: the first is the main agent, as the web's sidebar takes it); an agent not on it, `GET /api/f/{name}/status` for its npub |
| New chat | `POST /api/fragments {label, template:"chat", title}` then `PUT /api/f/{chat}/members/{agent npub} {role:"editor"}`; keep a successful create across a failed join |
| Messages, Stop, prompt answer | `POST /api/f/{chat}/channels/chat {id, body}`, as `docs/chat-records.md` defines |
| Stream/catch-up | `GET /f/{chat}/__live`, subscribe to `chat` and `work` with `last` initially, `after` on reconnect; follow `subscribed.more` until caught up |
| Earlier messages | `GET /api/f/{chat}/channels/chat?before=&limit=50`; never advance live cursors from this page |
| List invalidation | `GET /api/fragments/watch`; refetch the authenticated list on `changed`, a burst as one read 200 ms after (the web's `LIST_SETTLE_MS`) |
| Chat title | A chat of the person's still titled "New chat" with a message: `GET /api/f/{chat}/channels/chat?after=0&limit=20`, then its manifest's `meta.title` from its first line with words (48 characters at most), written and deployed only if the manifest still says "New chat" (the web's `titleFromFirst`) |
| Launcher search | `GET /api/search?q=` → fragments by title/label and messages by text, archived included; a message result opens its conversation |
| Read marker | `PUT /api/fragments/{chat}/seen`, after the visible chat catches up |
| App URL | `GET /api/f/{name}/status` → `urls.canonical` |
| App management | Personal `PUT /api/fragments/{name}/dock` and `/archived`; app `/status`, `/visibility`, `/members`, `/invites`, `/rotate`, `/icon`, `/icon/make`, and `/card` use the existing web contracts |

## Connect an account

The native client reuses `fragment login`; no second WorkOS integration or device
token protocol. The phone generates a fresh secp256k1 key, saves it to Keychain,
and signs `POST /cli/begin`. The person types the returned code on the platform’s
`/cli` page, signs in through the existing WorkOS flow if needed, checks the request,
and approves. The phone polls signed `GET /api/identities/me` every two seconds
while foregrounded, up to the code’s ten-minute expiry. That existing person and
email become the connected account. An npub is an identifier; possession of the
approved private key authenticates requests. The CLI’s private key is never copied.

`DeviceIdentity.swift` uses P256K 0.21.1 (Bitcoin Core’s libsecp256k1) for BIP-340.
NIP-98 binds the actual HTTP URL, method, body hash, and clock. A WebSocket’s tag
names the HTTP upgrade URL, not `wss:`. Every HTTP path (history, attachments,
icons and previews included) and live socket signs through `FragmentAPI`.
Credentials go only to the selected platform origin. Signed sessions are ephemeral,
hold no browser cookies, and refuse redirects. Release accepts HTTPS origins;
Debug also permits loopback and local development hostnames over HTTP.

Keychain stores the origin, private key and approved account together, using
`kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, without synchronization. No key in
UserDefaults, logs, URLs, a fragment page, or its JavaScript. A 401 is checked
against the identity endpoint and returns the app to connection when authorization
has ended. Disconnect revokes this phone’s key through the existing key API and
removes its local account data; it leaves other devices and browser sessions alone.
A failed/offline revocation is shown as an error, not reported as a successful logout.
The separate Remove from this iPhone action clears local credentials and drafts
after confirmation, explicitly leaving server revocation for the website. This
also lets a person leave an unreachable server without getting stuck.

## What an agent's runtime offers

Each runtime offers what it supports, and we don't build parity across
runtimes (AGENTS.md). The app offers nothing per agent beyond its chats today
(no Profile, Screen or agent skills), which is right for every runtime, so it
reads no runtime. Should it show one of those, the platform would report what
each agent offers, an add-only `offers` on the agent's status, rather than the
app copying the shell's table (`cell/shell/agent-runtimes.js`). Renaming an
agent rewrites its job (SOUL.md) as well as its title, a rule the shell and the
CLI hold, so the app leaves it to the web until one route does both.

## Apps and brains in web sheets

A signed `POST /api/f/{name}/session` reuses the registry’s existing single-use
redemption and top-level fragment session. It returns a one-fragment `__signin`
URL, good for 60 seconds, that the sheet’s nonpersistent WKWebView opens. Only
that fragment’s cookie enters the webview. Native authorization headers and private
keys never do. A stranger’s app asks for consent before learning the person’s
identity; owned/shared apps use their existing membership. General access and
membership permissions still belong to each fragment.

Each approved native key holds one internal parent session, expiring after 30 days,
which cannot authenticate as a browser’s platform session. The registry checks the
bound key when redeeming and on every derived session lookup; revoking the key
invalidates existing web cookies and pending redemptions. Existing per-parent
redemption and per-fragment session bounds apply. Expired parents are replaced on
the next authenticated mint. The key itself follows normal CLI-key revocation,
not a separate mobile expiration rule. Schema step: `0009_key_sessions.sql`.

The viewer follows same-origin navigation and sends unrelated clicked web links
to the system browser. It validates the redemption against the fragment’s canonical
origin, reports main-frame HTTP errors, and supports swipe dismissal. Add is a
native prompt sheet, matching the web shell’s built-in Add hand; there is no real
server fragment named `add-app`.

## Local data and reliability

Real-account drafts and pending sends are protected, atomic files in Application
Support, scoped by platform and person and excluded from backup. Bounds: 32 saved
chats, one MiB per file, 16 pending sends per chat, eight attachments per message.
The complete post and its original id are saved before HTTP. After termination,
ambiguous sends appear for explicit retry with that same id/body; they are never
silently resent with a new id. The channel takes the same id and body again as
the same post (`docs/chat-records.md`). A refusal (4xx, but 408 and 429) is final:
it is never retried; its words return to an empty composer at once, or, beside a
draft, wait in the chat to be edited once the composer is empty, with the server's
message (a draft and a post joined might be more than a message holds). Editing,
saving and loading keep one set of bounds (`SavedChat`): a paste past 32 KiB is
cut at a character, and said so. A saved file past its bounds loads what fits and
says what was cut; one that is no saved chat is cleared and says so. A chat
always opens and sends. Draft edits are debounced and flushed on leaving a chat or
backgrounding. Empty drafts are removed.

The app keeps no copy of a limit the server enforces: it shows the server's
refusal. Its own bounds are its storage's (above) and what it sends over a
phone's network: an attachment of at most 25 MiB, and an icon fitted inside
256×256 on the phone, as the server would fit it.

JSON text subscriptions match the Worker’s live protocol. A chat reads its last
50 messages and a page of `work` (1000, `CHANNEL_PAGE`). Which turns run follows
the web's chat page (`templates/chat/site/chat.js`): a turn is its agent's and its
id, a record counts only toward its own poster's turn, and a turn runs once its
start is read and until its end is, in either order; this is kept apart from the
shown records. Earlier work is read back as the web reads it, to the oldest message
shown, and also while a turn was read without its start, in batches of 50 (each
answer within the app's 4 MiB) and at most 1000 records a visit, so Stop shows for
a long turn however the chat was opened. An archive's chats index is
optional: an archive of other kinds alone has none, and a failed read never
stops the list or its agents. An export is followed while it builds (every two
seconds, as the web's Settings), downloads with the phone's signature to a file of
its own, is shared from there, and can be made again beside it. Its reads and makes
run one after another, so an older read never lands after a newer export; the file
is kept with its export, deleted once the export shown is another, and a download
still in flight for one no longer shown is never kept. A chat opened is loading
(a quiet spinner) until both its channels' backlogs are read to their end, then
shown at its end at once: never as an empty chat, nor its backlog streaming in. No
turn is said to run while no connection is open, nor until each connection has
read its `work`, so a catch-up's replayed start never shows as running. Titling
checks the manifest it read, not its write, as the web's does: a rename landing
between them is overwritten, as `POST files` takes no expectation. Its view observes what each part draws (rows, drafts,
running turns), each set only when it changes, so a reply's words redraw that
reply alone. Pictures are read once a session, kept by their versioned URL. Links
in a message are found as the web's renderer finds them (bare http(s) URLs too)
and underlined; a tap opens a page of a fragment of the person's in the app, at
that page, with an Open beside the message as the web's chat gives one, and any
other address in Safari; a long press copies or shares each. Your Apps is pushed
onto the page it was opened from, so Back returns there; a chat's top bar has no
New chat (the sidebar's starts one). Jumping to the latest holds scrolling off
for the jump, which stops a fling's momentum, and while following the latest the
bottom stays pinned as rows are drawn taller than the lazy stack's guess. Connected accounts follow the signed list watcher, discard
stale list reloads, and mark visible, caught-up chats read. HTTP/WebSocket
interruption recovery, independent cursors, Stop and history remain unchanged. Push delivery, approval-card controls, imported-chat continuation, and
full Markdown tables are not implemented. Notification permission alone does not
deliver background replies.

## Validation and rollout

`cargo xtask check` checks guards, schema lock, host tests and host/wasm Clippy.
`cargo xtask e2e --only signin,identities,native,restart` covers native-key web redemption,
unknown/revoked/agent keys, unsafe returns, one-use and wrong-fragment refusal,
consent, revocation isolation, and session survival across graceful restart/crash.
The existing sign-in lanes also exercise actual WorkOS/CLI browser approval.
Native tests cover NIP-98 URL/method/body/time binding and Schnorr verification,
credential origin boundaries, Keychain persistence/deletion, account-scoped drafts,
original-id outbox recovery, and the existing transport and UI flows.

For local commands, see `ios/README.md`. The app always opens on Connect until a
key is approved. Debug accepts loopback and `.localhost` HTTP origins, and
`--pair-server <origin>` fills in Connect for `cargo xtask dev`. A UI test's
launch (`--ui-test`, Debug only) keeps its account and drafts in a store of its
own, emptied as it starts, and refuses any origin past loopback before a request
(`LaunchScope`): a test never reads a person's account or reaches a deployment.

Hosted use needs the server changes deployed for private app/brain web sheets;
existing signed native API calls reuse the already deployed CLI authentication.
Deploy to preview, approve a phone key there, and verify an existing chat, stream,
Stop, private app/brain, membership removal and key revocation before production.
Run the same flows on a physical iPhone before TestFlight. Signing/team identifiers,
privacy disclosures and App Store submission remain release work. No deployment,
production account enrollment, or paid model call is authorized by this document.
