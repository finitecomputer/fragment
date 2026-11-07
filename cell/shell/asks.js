// What a late answer may still do (FIN-395): the shell asks, and answers
// come back in any order, after the person has moved on. Every kind of ask
// has one latest; an answer counts only while the ask it answers is the
// latest of its kind (`current`). This is the one place that is decided,
// in place of a counter by hand at each site (#462, #585, #619, #640, #1040,
// #1052…).
//
//   view           the middle column given a view (a chat, Settings, Apps,
//                  History, a chat being made): the launcher's late answer
//                  changes the screen only if none was given since
//   notice         the page-wide notice shown or taken away: what finishes
//                  later says or takes one away only under its own
//   computer       a view of the computer taken in from an action's answer:
//                  a list read asked before it never shows the computer as it was
//   apps           a visit to the Apps page (its open, its close)
//   settings       a visit to Settings from another page: an action's
//                  failure is said only in the visit it was pressed in
//   settings-load  one load of Settings: only the latest renders
//
// And what is in flight, one at a time (`hold`): a New chat being made
// (`start`), and a chat being titled from its first message (`title`, by
// chat, kept once titled: once a page).
//
// Pure (no page, no timers); a page has one (shell.js). Nothing is stored:
// a reload starts every kind afresh, as the page does.

export const KINDS = ["view", "notice", "computer", "apps", "settings", "settings-load"];
export const HOLDS = ["start", "title"];

export function createAsks() {
  const latest = new Map(KINDS.map((kind) => [kind, Object.freeze({ kind, n: 0 })]));
  const held = new Set();
  const kindOf = (kind) => {
    if (!latest.has(kind)) throw new RangeError(`no ask of kind ${kind}`);
    return kind;
  };
  const holdOf = (kind, key) => {
    if (!HOLDS.includes(kind)) throw new RangeError(`nothing is held of kind ${kind}`);
    return `${kind}\0${key}`;
  };
  return {
    /// A new ask of `kind`: its token, and none before it of that kind
    /// counts from now on.
    ask(kind) {
      const token = Object.freeze({ kind, n: latest.get(kindOf(kind)).n + 1 });
      latest.set(kind, token);
      return token;
    },
    /// The latest ask of `kind`, asking nothing new: an answer begun now
    /// counts while it is still the latest (`current`).
    now(kind) {
      return latest.get(kindOf(kind));
    },
    /// Whether an answer to `token` still counts: no newer ask of its kind.
    current(token) {
      return !!token && latest.get(token.kind) === token;
    },
    /// `key` of `kind` taken while nothing holds it: its token, else null
    /// (one already in flight).
    hold(kind, key = "") {
      const at = holdOf(kind, key);
      if (held.has(at)) return null;
      held.add(at);
      return Object.freeze({ kind, key });
    },
    /// What `token` held, let go (null: nothing).
    letGo(token) {
      if (token) held.delete(holdOf(token.kind, token.key));
    },
    /// Whether `key` of `kind` is held.
    held(kind, key = "") {
      return held.has(holdOf(kind, key));
    },
  };
}
