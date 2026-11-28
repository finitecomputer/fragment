// Whose an agent's turn in the chat is: who stops it and whom its owner's
// commands go to while it runs (docs/chat-records.md, "Stop"). Its asker's,
// except:
// - a turn caused on its agent's own `tasks` (a routine's, or an ask's
//   answer, asked as the agent that answered: Paul, 2026-10-30), and
// - a turn in a door chat (`agent.door`) or room (`turn.start.room`) whose
//   asker is not a chat member (FIN-508: the owner can stop an outside
//   speaker's turn in a door chat, with or without `room`).
// Both belong to the agent's owner, as the bridge rules it (images/bridge,
// `who_answers`). Apart from chat.js, which imports the platform's own
// modules, so Node's tests import it alone
// (cell/shell-tests/chat-control.test.mjs).

// a turn this page has seen start and not end
export const running = (t) => t.startAt !== null && !t.end;

// who controls turn `t` (Stop / `/`), by the chat's members (`__members`,
// each agent's owner and optional `door` beside it): null when no one here may
export function controllerOf(t, members) {
  const agent = members.find((m) => m.principal === t.agent && m.kind === "agent");
  const outside = members.length > 0 && !members.some((m) => m.principal === t.asker) && (!!t.room || !!agent?.door);
  if (t.cause?.channel !== "tasks" && !outside) return t.asker ?? null;
  return agent?.owner ?? null;
}

// the newest running turn `me` controls, else null
export function controlledBy(turns, members, me) {
  if (!me) return null;
  return [...turns].filter((t) => running(t) && controllerOf(t, members) === me).sort((a, b) => b.startAt - a.startAt)[0] ?? null;
}
