// An agent's question in a chat app: what a door sends its owner's app of
// a card the agent posts on `work` (a `turn.prompt`: docs/chat-records.md),
// since a card shows only in the chat. Its words, its choices named, and
// how to answer: the owner's next message answers a card that takes words
// (the bridge's engine, `told`), and a card that takes a press alone says
// so. One way for every door, as the computer's own SimpleX shows one
// (images/bridge/src/driver.rs `for_simplex`), the two held to the same
// cases (images/bridge/src/questions.json). Shared by every connector,
// one file (templates/telegram's is a link to templates/simplex's).
//
// Who may answer is the platform's: a connector sends only a card that
// asks its owner, and decides nothing of what the agent may do.

// `body`'s question as its owner's app shows it, or null when it is none.
export function asked(body) {
  if (body?.kind !== "turn.prompt" || !Array.isArray(body.options)) return null;
  const text = typeof body.text === "string" ? body.text : "";
  const words = body.options.some((o) => o?.words === true);
  const labels = body.options.filter((o) => o?.words !== true).map((o) => o?.label).filter((l) => typeof l === "string");
  if (!words) return `${text}\n\n(This one needs a button: answer it in our chat.)`;
  if (labels.length === 0) return text;
  return `${text}\n\n(Answer with one of: ${labels.join(", ")}, or in your own words.)`;
}
