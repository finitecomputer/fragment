// Whose an agent's turn in the chat is: who stops it, whom its owner's
// commands go to while it runs, and whose words answer a question it asks
// (docs/chat-records.md, "Stop"). Its asker's, but for a turn caused on
// its agent's own `tasks` (a routine's, or an ask's answer, asked as the
// agent that answered: Paul, 2026-10-30), its agent's owner's, as the
// bridge rules it (images/bridge, `who_answers`). Apart from chat.js, which
// imports the platform's own modules, so Node's tests import it alone
// (cell/shell-tests/chat-control.test.mjs).

// a turn this page has seen start and not end
export const running = (t) => t.startAt !== null && !t.end;

// who controls turn `t`, by the chat's members (`__members`, each agent's
// owner beside it): null when no one here may
export function controllerOf(t, members) {
  if (t.cause?.channel !== "tasks") return t.asker ?? null;
  return members.find((m) => m.principal === t.agent && m.kind === "agent")?.owner ?? null;
}

// the newest running turn `me` controls, else null
export function controlledBy(turns, members, me) {
  if (!me) return null;
  return [...turns].filter((t) => running(t) && controllerOf(t, members) === me).sort((a, b) => b.startAt - a.startAt)[0] ?? null;
}
