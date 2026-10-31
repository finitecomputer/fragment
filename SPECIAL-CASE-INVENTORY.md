# Special-case inventory

The platform is fragments and computers (`docs/cloudflare-v1.md`, "The
rule"). This file lists everything else the platform does, with the
reason it is not a fragment or a computer. A thing belongs here only
when making it platform code is a security win, or it is the thin
shell users can't break. Each entry names the public API it sits on, so
a fragment could build the same thing on top of those APIs. The list
stays short (Paul, 2026-10-02).

Adding an entry needs its reason and its API. Removing one is always
welcome. Nothing here may name an agent runtime (Hermes), a template or
a vendor's product beyond the credential it holds.

| Surface | Why it is platform | Built on |
|---|---|---|
| Sign-in, sessions, CLI approval (`/auth/*`, `/cli`; the shell's tabs sign in through `/auth/frame`; a stranger's fragment asks first, at `/auth/fragment`) | Security: it mints the credentials every other surface trusts. Which of a browser's cookies count on a fragment's origin is the router's (Fetch Metadata), so another fragment's page reaches it as a stranger (docs/api.md, Sign-in). | WorkOS AuthKit; the registry |
| The shell (`/`, `/settings`, `/apps`, `/history`, `/welcome`): sidebar, desktop (its dock and windows), settings, History (every chat you have had, archived and imported ones too, and its search), the Apps page (each of your fragments but chats and agents: your dock, its card, its icon, who has access and its share sheet; one page, as it acts as you over all of them, which no fragment may), the launcher (search), first run (the welcome); your Session Archive (your one fragment of kind `sessions`: its chats' index listed in History, each opened read-only in the middle column, its own page framed as a chat is; its Continue, asked by that page and done by the shell as you, makes a chat with the agent of yours you pick (one a conversation and agent) whose first record is the conversation's history, and does the same for an archived chat of yours: templates/sessions/README.md, docs/chat-records.md); to a page of a fragment you own that asks, your agents, and adding one of them to that fragment; to your own chat's page, a way to ask for an app put in its composer (`compose`; docs/api.md, The shell); to a chat's page of yours, its alerts, which no frame may ask the browser for (the shell's alerts worker, one subscription a chat, made with the chat's own key); what your agent is doing, which it reads from your chats' rows' listed records by the chat's turn kinds (docs/chat-records.md): the shell is that contract's third reader, beside the bridge and the chat's page, so a change to it changes the shell too | Security: it holds your session and frames your fragments and computers. The thin page users can't break. A page asks it, never grants: the shell checks the page is your own fragment's, at its own origin, offers only your own agents, and adds one only once you confirm it in the shell's own dialog, every time. A chat's page takes `compose` only from the platform's origin (its hello's `platform`) and sends nothing: you do. | The public fragment, computer, identity and ledger APIs |
| Identity and delegation | Security: who an agent acts for, and at what role. | The registry |
| Optional public addresses (`GET`/`PUT /api/f/{name}/address`) | Security: first-come reservations coordinate across fragments and permanently bind an origin to one app identity, so a delete never gives another app its cookies or storage. | The registry and each fragment's own identity and owner |
| Connections and the egress swap | Security: it holds the route to your accounts' tokens and the operator's keys. | WorkOS Pipes; the computer's intercepts |
| An agent's doors: a ticket to link or unlink one (`/api/computers/{id}/agents/{fragment}/door`), its computer's redeeming of it (`/api/computer/door`), and the rule that a record which relays an owner (`relayed`) is taken only signed by its agent's door | Security: what comes through a door speaks as the agent's owner, so only the owner opens one, and only the door's key, never the agent's own (which its tools use), makes a record that does (docs/api.md, "An agent's doors"). | The computer API; each agent fragment's own key; a fragment's posts |
| Usage, credit and plans | Billing integrity. | The ledger API (read-only to fragments) |
| Seats and orgs (`/api/seat`, `/api/org`), an org's own credit (`/api/orgs/{org}/ledger`; an operator's credit, budget and payer) and its content governance (`/api/orgs/{org}/content-admin-offers`, `content-admins`, `content`; `/api/orgs/offers`), buying them (`/api/billing/*`, Stripe's webhook at `/api/stripe/webhook`), and the operators' admin (`/api/admin/*`, its page `/admin`) | Billing integrity: a seat sets its holder's plan, and only its org's admins, Stripe, and the deployment's operators change one; an org's credit and budget are an operator's to set (orgs pay out of band); its content admins are appointed by offers their people accept, apart from its billing (decision 69); Stripe's webhook is signed with this deployment's secret (docs/billing.md). | The registry; Stripe; each holder's ledger and computer |
| Finite's about page, privacy policy and terms (`/about`, `/privacy`, `/terms`) | Legal and public: the operator's own words, at the platform's origin for anyone, signed out too, where WorkOS's sign-in and Stripe's Checkout link them; static pages of the release, no script. | The release's files (cell/shell) |
| The share sheet (`/share/<name>`) | Security: it acts as the fragment's owner, so no fragment's code (which its author or an agent rewrites) may frame, fetch or script it (docs/api.md, Sharing). | The members and invites API |
| A person's export of everything that is theirs (`/api/export`, the shell's Your data, `fragment export`) | Account integrity: it reads across every fragment and computer of its person's (and the names of those shared with them), as them, which no fragment may; only its person starts, reads or downloads it, and it adds no power to anyone else (docs/api.md, Export). | Each fragment's own routes as its owner reads them, the registry's view of the person, their list, and their computer's saves through its Durable Object |
| A chat's transcript in its person's export (`transcript.md`; `fragment_core::export::transcript_part`) | Readability: an export is a person's own copy of everything, so their chats must read as conversations, not raw records. Chat is a blessed template whose record shapes are a platform contract (docs/chat-records.md), so the platform reads them; a change to those shapes changes the transcript too. | The chat's `chat` and `work` channels as its owner reads them |
| An operator's wipe of a person (`/api/people/{person}/wipe`, `fragment operator wipe`) | Security and account integrity: only the deployment's operators delete a person, and it reaches every object of theirs (their fragments, computer, ledger, lists and registry rows), which no fragment may (docs/api.md, Operators). | The registry, and each object's own end (a fragment's delete, a computer's saves) |
| Test levers and the e2e's sign-in (`/api/test/*`; previews and the local e2e only, 404 elsewhere) | Proof: the hosted e2e signs `@e2e.test` people in and pulls levers on a preview's real vendors, with no real account. A secret only a branch deploy takes gates it, and on a preview it reaches the e2e's own fragments and people alone (docs/secrets.md). | The registry, the ledger, and the fragment cells' test controls |

## Fragment plumbing (not special cases)

Every fragment gets these routes (docs/api.md, Serving), and none of
them knows what a fragment is for: `__fragment.js`, `__fragment.css`
and `__markdown.js`, `__signin` and `__signout`, `__op`, `__live` and `__watch`, `__people`, `__members`, `__schedules`,
`__blob`, `__files` and `__file` (the vault UI), `__sw.js` and
`__push-*`, `__preview.svg`, people's pictures
(`/api/users/{u}/picture`), and the frame-session redeem path the
shell's tabs use (`__signin?token=` in a frame, from `/auth/frame`). A
refusal of the router's that a browser navigates to is a page in the
platform's look, not JSON; an app's own answers pass as they are
(docs/api.md, Opening a fragment by its URL).

## Not special cases

These are fragments or computers, or components any fragment may use:

- Chats, agents, brains, the skills set and apps are fragments. The blessed
  ones run the platform's current template version.
- A computer's screen is a page its image serves, reached through the
  generic port proxy.
- Platform components any fragment may use: the vault UI (files and
  notes, used by brains), the browser library (`__fragment.js`), the
  markdown renderer (`__markdown.js`: markdown to DOM, never markup; the
  vault UI, the chat and the notes viewer use it), and the stylesheet a
  page may link (`__fragment.css`).

Which connectors may be linked, and the exact TCP targets a computer's relay carries for them, are the deployment's policy (`connectors`; docs/computers.md, "Connector policy"), never an image's declaration. Relay leases and the computer's pooled quotas stay the platform's.

A hosted door is the one case where a fragment's app may publish a record that speaks for a person (`relayed`): only a fragment made from a blessed connector template (`blessed::CONNECTORS`), that never ran other code, that no one but its owner edits or shares (every other caller, its agent acting for its owner included, is at most a viewer there, and no agent shares it), and that its owner bound in person as their agent's door; only `relayed` exactly `{via, for, agent}` (cell/src/doors.rs). Every other app publishes none.
