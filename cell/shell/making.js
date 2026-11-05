// The chat a New chat is making: the one thing the shell shows before the
// platform has it (Paul, 2026-11-04), so it is one explicit state, changed
// by these transitions alone (FIN-395). Pure: shell.js does what each
// answers.
//
//   none ──start──▶ shown    its room is the middle column's view
//   shown ──left──▶ away     another view was given since (a chat,
//                            Settings, Apps, History): its room is hidden,
//                            its row stays, inactive, until the create answers
//   shown|away ──renamed──▶ the same, its name drawn again (taken: 409)
//   shown|away ──settled──▶ none
//       made, shown:     its chat opens in its place
//       refused, shown:  the view it was started from comes back
//       away, either:    nothing moves; the list says what there is
//
// Its name is chosen before the create (`CreateFragment.name`), so the
// row the list reads for it is known as it, whichever comes first: the
// live list's read, or the create's answer.

/// `{agent, name, title, before}`: `before`, the view it was started from
/// (`{page, current}`), for a refusal while it is shown.
export function start({ agent, name, title, before }) {
  return { phase: "shown", agent, name, title, before };
}

/// Its name was taken (the create's 409): the one drawn next is its name.
export function renamed(m, name) {
  return m ? { ...m, name } : m;
}

/// Another view was given: a shown one is away from now on.
export function left(m) {
  return m?.phase === "shown" ? { ...m, phase: "away" } : m;
}

/// What its create's answer (`made` or `refused`) does: `open` its chat,
/// `restore` the view before, or `stay` as the screen is.
export function settled(m, outcome) {
  if (m?.phase !== "shown") return "stay";
  return outcome === "made" ? "open" : "restore";
}

/// The chat on screen in the middle column: the one being made while its
/// room is shown, else the open chat unless a page is (`page`).
export function shownChat(m, current, page) {
  if (m?.phase === "shown") return m.name;
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
