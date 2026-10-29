# Splitting the shell

2026-10-28, Paul's Apps iteration. This is a module map, not a rewrite
commitment. The separate Solid 2 experiment is on `spike/solid-shell-2`.

`cell/shell/shell.js` is 4,199 lines after moving the shared name editor
to `fragment-title.js`. It already delegates the launcher, Apps, billing,
icons, layout, and viewer. The next boundaries are features with a
lifecycle, rather than arbitrary chunks of the file.

| Module | Rough source lines to move | Owns | Shell supplies |
|---|---:|---|---|
| `feedback.js` | 164 in #643 | form, screenshots, send/retry, late answers | API write/release guards, shared UI helpers |
| `connections.js` | 377 | provider rows, accounts, connect/disconnect, consent refresh | API, computer/agent reads, section mounting |
| `onboarding.js` | 448 | sign-in/welcome, seats, first chat/agent, retry and resume | API, navigation, shared computer/list state |
| `settings.js` | 402 | section navigation, mounting and disposal | narrower feature sections |
| `agent-settings.js` | about 320 | model/home settings, name and picture | API, agent/computer state, shared name editor |
| `session-refresh.js` | 272 | deployment detection, safe reload, reload bounds | write count, active editors/frames, navigation callback |
| `chat-list.js` | about 330 | list, unread/activity, invites and chat menu | list state, navigation and share actions |
| `computer.js` | about 250 | wake, notices, restart/update actions | API, state-change callback |
| `exports.js` | 79 | export state, polling, download | API, section helper |

The feedback extraction is prepared separately on `codex/feedback-module`,
based on the current #643 head. Its shell delegates to `createFeedback`;
the module receives a `beginWrite` guard whose returned cleanup is called
in `finally`. No form selectors, limits, request shapes, or retry semantics
change. This keeps the new feature out of the monolith without waiting on
a framework decision.

Keep the API, authoritative fragment list, routing, trusted frame messages,
and composition in the shell's small root. Each feature owns its transient
state and cleanup and exports a small interface (`open`, `close`, `listed`,
or a section factory). Avoid passing the entire shell to every module, or
exporting all its internals just to make a line count smaller.

Extract one concern per PR. Keep the existing public wire and frame
contracts. Exercise the relevant existing browser lane after each move;
the hardest contracts are first-run resume, frame-origin checks, writes
still in flight during navigation, drafts, and safe reload during deploys.

## What a Solid rewrite would change

TypeScript and explicit component props catch missing/null fields and
accessibility attribute mistakes. Solid's keyed components remove the
hand-built candidate DOM, `data-shows` signatures, and whole-list redraws.
Loading/error boundaries and component cleanup can replace much of the
visit-token and timer bookkeeping for reads. Server writes still need
their actual idempotency and ordering; framework cancellation is no
substitute for those.

A compiler and a runtime become part of the release, on the pinned Node.
The spike measures that cost, rather than comparing unminified vanilla
source with minified framework output. It does not claim a full shell
rewrite or compare an Apps-only prototype with the full shell as if they
had equal features. A bundle-only pass and these module extractions are
useful baselines before choosing a framework.
