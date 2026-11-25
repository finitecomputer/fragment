// An agent's question in a chat app: what a door sends its owner's app of
// a card the agent posts on `work` (a `turn.prompt`: docs/chat-records.md),
// since a card shows only in the chat. Its words, its choices named, and
// how to answer: the owner's next message answers a card that takes words
// (the bridge's engine, `told`), and a card that takes a press alone says
// so. A card answered on the agent's computer (a sign-in's code) is never
// asked for here, nor its words sent (written for the card, they say to
// paste it "here"): typed here, it would be a message kept in the chat and
// no answer (the engine takes none for it), so the app is told only to
// answer it on its card in the chat, which sends it to the computer alone.
// One way for every door, as the computer's own SimpleX shows one
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
  if (body.options.some((o) => o?.computer === true)) return "I need something from you on a card in our chat. Type it there, not here: from there it goes to your computer alone.";
  const words = body.options.some((o) => o?.words === true);
  const labels = body.options.filter((o) => o?.words !== true).map((o) => o?.label).filter((l) => typeof l === "string");
  if (!words) return `${text}\n\n(This one needs a button: answer it in our chat.)`;
  if (labels.length === 0) return text;
  return `${text}\n\n(Answer with one of: ${labels.join(", ")}, or in your own words.)`;
}
