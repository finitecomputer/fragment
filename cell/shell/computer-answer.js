// Who may hand a card's answer to its agent's computer (shell.js; docs/
// bridge.md, "An answer for the computer"): a frame of one of the person's
// own fragments (its role "owner", the only frames the shell hands its
// roster), whose page is at that fragment's own origin. A fragment shared
// with them is another person's page: it never acts as the computer's owner.

/// Where a frame's `card-to-computer` goes: `{agent}`, the fragment of the
/// person's own agent its card is of; `{refused}`, in words, for a frame of
/// theirs whose card is of no agent of their computer's; null, said nothing
/// to, for any other sender (`role` not "owner", an unknown frame, or a page
/// not at its fragment's origin). Only an `{agent}` is handed a port ticket.
export function answerTo({ frame, role, origin, sentFrom, agent, agents, computer }) {
  if (!frame || role !== "owner" || !origin || sentFrom !== origin) return null;
  const fragment = agents.get(agent)?.fragment;
  if (!computer || !fragment) return { refused: "Only its owner answers this, from their own Finite." };
  return { agent: fragment };
}
