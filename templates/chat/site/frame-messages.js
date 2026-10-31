// The messages between the shell (the platform's page) and the pages it
// frames: a person's fragments (a chat, an app's window, an agent's page)
// and their computer's ports (an agent's screen, its SimpleX link page).
// This module is their one definition (Paul's Fragment Way: the code is the
// truth; FIN-341): every message is `{fragment: <kind>, ...}`, its kind one
// of these, and docs/api.md (The shell) points here.
//
// The rules around them: the shell takes a message only from a frame it
// made, at that frame's own origin, and answers that frame alone; a framed
// page takes messages only from its parent at the platform's origin (the
// `__live` hello's `platform`). A page's message asks, never grants.
//
// The shell (cell/shell/, a symlink to this file) and the chat's page
// (templates/chat, telegram's too) name every kind through `KIND`; the
// pages that cannot import this file (the other templates, the cell's own
// `__files` and `__signin` pages, the alerts worker, the computer's ports
// in images/) send and take the same kinds as literals, held to these by
// cell/shell-tests/frame-messages.test.mjs, which also finds every kind
// here sent and taken.

export const MESSAGES = Object.freeze({
  // ---- a frame of the person's own fragment asks for their agents ----
  agentsAsk: {
    kind: "agents?",
    from: "a frame of a fragment the person owns → the shell",
    fields: "{}",
    says: "Asks for their agents, answered `agents`, again whenever they change. A frame of a fragment shared with them gets no answer.",
  },
  agents: {
    kind: "agents",
    from: "the shell → that frame",
    fields: "{agents: [{identity, fragment, name, title, picture?, phase?, modelProviders?}], models?, setup}",
    says: "The agents their computer runs (`name` the label `@` uses, `title` and `picture` the face, `phase` the computer's), `models` and `modelProviders` from its view (`GET /api/computers/{id}`); `setup` the first run's chat's steps while its agent is made (`{steps: [{text, state: done | doing | todo | failed}], error}`), else null.",
  },
  addAgent: {
    kind: "add-agent",
    from: "a frame of a fragment the person owns → the shell",
    fields: "{identity, nonce}",
    says: "Asks to add one of their agents: the shell asks the person in its own dialog (\"Add Fred to …?\", Add armed after 800 ms) and only on Add makes the agent an editor of the frame's fragment. Nothing is remembered.",
  },
  agentAdded: {
    kind: "agent-added",
    from: "the shell → that frame",
    fields: "{nonce, identity, ok, error?}",
    says: "`add-agent`'s answer: `error` is \"declined\", \"not answered\" (90 s), \"busy\", or the API's refusal.",
  },
  retrySetup: {
    kind: "retry-setup",
    from: "the first run's chat → the shell",
    fields: "{}",
    says: "Tries a failed setup step again.",
  },
  // ---- a chat's page and the shell ----
  compose: {
    kind: "compose",
    from: "the shell → a chat it framed, once the chat asked `agents?`",
    fields: "{text}",
    says: "The chat replaces what was typed with it and focuses its composer; sends nothing; taken only from a person who may post.",
  },
  busy: {
    kind: "busy",
    from: "a chat → the shell",
    fields: "{busy}",
    says: "Whether it holds what no storage keeps (files chosen, a memo, a message on its way or failed, …): the shell does not reload under it.",
  },
  said: {
    kind: "said",
    from: "a chat → the shell",
    fields: "{}",
    says: "The person's message was sent there: a chat they archived is unarchived (`PUT /api/fragments/{name}/archived`), back in their sidebar.",
  },
  forgetDraft: {
    kind: "forget-draft",
    from: "the shell → each chat, before signing out",
    fields: "{}",
    says: "The chat drops its stored draft, answered `draft-forgotten`.",
  },
  draftForgotten: {
    kind: "draft-forgotten",
    from: "a chat → the shell",
    fields: "{}",
    says: "`forget-draft`'s answer: its draft is gone.",
  },
  alertsAsk: {
    kind: "alerts?",
    from: "a frame of one of the person's chats → the shell",
    fields: "{nonce}",
    says: "Asks for the state of its alerts (docs/api.md, A chat's alerts are the shell's), answered `alerts`.",
  },
  alerts: {
    kind: "alerts",
    from: "a chat → the shell, and the shell's answer back",
    fields: "asked: {nonce, on, key?}; answered: {nonce, state: on | off | denied | unsupported | failed, subscription?, endpoint?, error?}",
    says: "Turns its alerts on or off (on: the release's chat alone), and the shell's answer to it or to `alerts?`.",
  },
  // ---- any frame and the shell ----
  open: {
    kind: "open",
    from: "any frame the shell made → the shell",
    fields: "{url, title?}",
    says: "A URL of the frame's own origin opens a file's window; a page of another fragment of the deployment (the frame's origin with another name in its place) opens as that fragment's window. Anything else opens nothing.",
  },
  openLauncher: {
    kind: "open-launcher",
    from: "the focused frame of a listed fragment → the shell",
    fields: "{}",
    says: "Opens the launcher (⌘K inside a frame).",
  },
  theme: {
    kind: "theme",
    from: "the shell → a frame",
    fields: "{mode, mobile}",
    says: "Its light or dark mode, and whether it is laid out for a phone.",
  },
  sessionArchive: {
    kind: "session-archive",
    from: "the person's Session Archive's frame → the shell",
    fields: "{key} | {continue}",
    says: "`key`: the conversation it shows (null: its list); `continue`: a conversation to pick up as a new chat (the shell asks with which of their agents, or opens the chat one made of it).",
  },
  signinBlocked: {
    kind: "signin-blocked",
    from: "a fragment's `__signin`, or `/auth/frame`'s note → the parent",
    fields: "{name, why?}",
    says: "The frame could not sign in, and why (docs/api.md, Frame sessions): the shell says so in its place.",
  },
  // ---- the computer's ports, Settings' and the desktop's ----
  screen: {
    kind: "screen",
    from: "the shell → an agent's screen page",
    fields: "{agent, name}",
    says: "The agent's name, as the page loads and on a rename.",
  },
  simplex: {
    kind: "simplex",
    from: "the shell → a SimpleX link page, and that page (Settings framed it, for its own agent) → the shell",
    fields: "to the page: {agent, name}; asked: {agent, want: link | unlink, home?}; answered: {agent, for, ticket?, error?, home?, homeError?}",
    says: "The agent's name as the page loads and on a rename; a door ticket for a link (docs/api.md, An agent's doors), a link's `home` setting its home and an unlink's taking it back.",
  },
  // ---- the alerts worker ----
  openChat: {
    kind: "open-chat",
    from: "the alerts worker → an open shell",
    fields: "{name}",
    says: "Opens the chat an alert was for.",
  },
});

// Each message's kind by its name here (`KIND.busy` is "busy"): what the
// shell and the chat's page send and compare, so no kind is spelled twice.
export const KIND = Object.freeze(Object.fromEntries(Object.entries(MESSAGES).map(([name, m]) => [name, m.kind])));
