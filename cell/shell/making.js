// The chat a New chat is making: the one thing the shell shows before the
// platform has it (Paul, 2026-11-04), so it is one explicit state, changed
// by `step` alone (FIN-395). Pure: shell.js does what each step answers.
//
//   phase   event                        next    then
//   none    start {by,agent,name,title,before} shown stay
//   shown   answered made, listed        none    open     its chat opens in its place
//   shown   answered made, unlisted      made    wait     its room stays for the list
//   shown   answered refused             none    restore  the view it was started from
//   away    answered (either)            none    stay     the list says what there is
//   made    listed, has it               none    open     its chat opens in its place
//   shown|away|made|none listed (else)   same    stay
//   shown   left                         away    stay     another view was given: its
//                                                         room goes, its row stays
//   made    left                         none    stay     nothing is left to wait for
//   away|none left                       same    stay
//   shown|away renamed {name}            same    stay     its name was taken (409)
//   any     reload                       none    stay     memory only: the list is the truth
//
// Any other pair is a mistake of the shell's, and throws (`MakingError`),
// as does an `answered` or `renamed` from another New chat than the one
// that started it (`by`, its owner: each New chat owns its own; `owns`).
//
// Two answers come of it, never one: the chat whose row is active
// (`shownChat`: its own while its room shows), and the chat whose messages
// are on screen (`readChat`: none while its room shows, even once the list
// has its chat), which alone is marked seen and shows no unread.
//
// Its name is chosen before the create (`CreateFragment.name`), so the
// row the list reads for it is known as it, whichever comes first: the
// live list's read, or the create's answer.

export const PHASES = ["none", "shown", "away", "made"];
export const EVENTS = ["start", "answered", "listed", "left", "renamed", "reload"];

/// An event its phase does not take: the shell's mistake, never a person's.
export class MakingError extends Error {}

const phaseOf = (m) => m?.phase ?? "none";
const go = (next, then = "stay") => ({ next, then });
// an event only its owner sends: refused from any other
const own = (take) => (m, event) => {
  if (!owns(m, event.by)) throw new MakingError(`a chat being made takes ${event.type} from the New chat that started it alone`);
  return take(m, event);
};

// each phase's events: (m, event) => {next, then}
const TABLE = {
  none: {
    start: (_, { by, agent, name, title, before }) => {
      if (!by) throw new MakingError("a chat being made is started by its New chat (`by`)");
      return go({ phase: "shown", by, agent, name, title, before });
    },
    listed: (m) => go(m),
    left: (m) => go(m),
    reload: () => go(null),
  },
  shown: {
    answered: own((m, { outcome, listed }) => {
      if (outcome === "refused") return go(null, "restore");
      if (outcome !== "made") throw new MakingError(`an answer is made or refused, not ${outcome}`);
      return listed ? go(null, "open") : go({ ...m, phase: "made" }, "wait");
    }),
    listed: (m) => go(m),
    left: (m) => go({ ...m, phase: "away" }),
    renamed: own((m, { name }) => go({ ...m, name })),
    reload: () => go(null),
  },
  away: {
    answered: own(() => go(null)),
    listed: (m) => go(m),
    left: (m) => go(m),
    renamed: own((m, { name }) => go({ ...m, name })),
    reload: () => go(null),
  },
  made: {
    listed: (m, { has }) => (has(m.name) ? go(null, "open") : go(m)),
    left: () => go(null),
    reload: () => go(null),
  },
};

/// `m` (null: none) after `event` (`{type, …}`): `{next, then}`, where
/// `then` is what shows: `open` its chat, `restore` the view before, `wait`
/// in its room, or `stay` as it is.
export function step(m, event) {
  const phase = phaseOf(m);
  const take = TABLE[phase]?.[event?.type];
  if (!take) throw new MakingError(`a chat being made takes no ${event?.type} while ${phase}`);
  return take(m, event);
}

/// Whether the New chat `by` (its token) started `m`: its own answers and
/// renames are taken from it alone.
export function owns(m, by) {
  return !!m && !!by && m.by === by;
}

/// Whether its room has the middle column (shown, or made and waiting).
export function holdsView(m) {
  return m?.phase === "shown" || m?.phase === "made";
}

/// The chat whose row is active: the one being made while its room has
/// the middle column, else the open chat unless a page is (`page`).
export function shownChat(m, current, page) {
  if (holdsView(m)) return m.name;
  return page ? null : current;
}

/// The chat whose messages are on screen, the one marked seen and counted
/// nothing unread: none while a room has the middle column (it shows no
/// messages, even once the list has its chat), else the open chat unless a
/// page is.
export function readChat(m, current, page) {
  if (holdsView(m)) return null;
  return page ? null : current;
}

/// Whether its own row is drawn: until the list has the chat it makes
/// (`listed(name)`), whose row then stands in its place.
export function ownRow(m, listed) {
  return !!m && !listed(m.name);
}

// A name's suffix, decision 47's alphabet (`fragment_proto::NAME_SUFFIX_ALPHABET`,
// four of it: `NAME_SUFFIX_BYTES`).
const SUFFIX = "abcdefghijkmnpqrstuvwxyz23456789";
/// A chat's name drawn from its label (`label--xxxx`), as the platform draws
/// one for a label (cell/src/lib.rs `create_fragment`).
export function drawName(label) {
  return `${label}--${[...crypto.getRandomValues(new Uint8Array(4))].map((b) => SUFFIX[b & 31]).join("")}`;
}
