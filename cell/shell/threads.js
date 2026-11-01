// Delegation threads (Paul, 2026-10-30; docs/chat-records.md, "An ask"):
// one of the person's agents asks another (`fragment ask --return-to`), the
// asked agent's turn runs in the two agents' chat with their owner, its pair
// chat, and the answer comes back as the asker's own turn in the chat it
// asked from. The person sees each thread as one line over that chat: who,
// the first line of what was asked, its state, how long, and Stop; the pair
// chat is its transcript, one click away, and leaves the chat list.
//
// Nothing is kept of it but what the platform has. A pair chat's state is
// its row's listed records (live on the list's socket); what was asked, when,
// where its answer goes and how each turn ended are its newest records, read
// again whenever its row changes. A line stays until its answer has come:
// the answer on its asker's `tasks` (`an:<asked turn>`), and the asker's turn
// of it in the asking chat, ended (looked for again whenever that chat's row
// changes, or another of its threads is seen to end). So a thread lost or
// stopped is said, never dropped.

// The newest records a pair chat's read takes: its last questions, and their
// turns' starts and ends among the steps between.
const CHAT_RECORDS = 100;
const WORK_RECORDS = 400;
// An asker's `tasks` records an answer is looked for among (its routines,
// its joins, its answers).
const TASK_RECORDS = 100;
// A page's cursor past any record: the newest, read backwards.
const NEWEST = Number.MAX_SAFE_INTEGER;
// A pair chat's label (`fragment ask`'s, cli/src/ask.rs `pair_label`): the two
// agents' labels in order, joined by `-`, each cut to fit when both do not:
// proto's `LABEL_ROOM_MIN_BYTES` (shell.js's `LABEL_ROOM`).
const LABEL_ROOM = 39;
const STATE_SAYS = { asked: "Asked", working: "Working", you: "Waiting for you", done: "Done", stopped: "Stopped", lost: "Lost" };

export function pairLabel(a, b) {
  const [x, y] = [a, b].sort();
  if (x.length + 1 + y.length <= LABEL_ROOM) return `${x}-${y}`;
  const cut = (s) => s.slice(0, (LABEL_ROOM - 1) >> 1).replace(/-+$/, "");
  return `${cut(x)}-${cut(y)}`;
}

export function createThreads({ $, el, patch, api, seg, avatar, data, navigation, rendered }) {
  const { byName, own, chats, titleOf, labelOf, agentFace, ownAgents, current, page } = data;
  const { openChat, leaveSidebar } = navigation;
  // a pair chat's last read: its row's signature then, and its threads
  const reads = new Map();
  // the asked turns whose answer has come; an asking chat's row's signature
  // when its answers were last looked for
  const answeredTurns = new Set();
  const answerChecks = new Map();
  let reading = 0;

  // The person's pair chats: theirs, with exactly two of their agents, and
  // labelled as `fragment ask` labels their pair. A group chat of theirs
  // otherwise named is theirs to talk in, and stays in the list.
  function isPair(f) {
    if (!f || f.kind !== "chat" || !own(f) || f.agents?.length !== 2) return false;
    const mine = new Map(ownAgents().map((a) => [a.identity, a]));
    const [a, b] = f.agents.map((id) => mine.get(id));
    return !!a && !!b && labelOf(f.name) === pairLabel(labelOf(a.fragment), labelOf(b.fragment));
  }
  // A row as it was sent: its newest message's time, and every member's
  // newest listed record. Each is a member's newest only, sent at most once
  // in 3 s (docs/api.md, Listed records), so a turn's start or end may be
  // past before a row says it: any change is read again for (#779 review).
  const signature = (f) => JSON.stringify([f.previewAt ?? null, (f.listed ?? []).map((r) => [r.channel, r.principal, r.kind, r.at])]);

  // A pair chat's threads, from its newest records: each question one of
  // their agents asked another (its `to`, and its `return_to`), its turn's
  // start and end when they are among them.
  async function threadsOf(name) {
    const [said, work] = await Promise.all([
      api("GET", `/api/f/${seg(name)}/channels/chat?before=${NEWEST}&limit=${CHAT_RECORDS}`),
      api("GET", `/api/f/${seg(name)}/channels/work?before=${NEWEST}&limit=${WORK_RECORDS}`),
    ]);
    const mine = new Set(ownAgents().map((a) => a.identity));
    const turns = (work.records ?? []).map((r) => ({ at: r.at, principal: r.principal, ...r.body }));
    const out = [];
    for (const r of said.records ?? []) {
      const b = r.body ?? {};
      const asked = Array.isArray(b.to) ? b.to[0] : null;
      if (!mine.has(r.principal) || !mine.has(asked) || typeof b.return_to !== "string" || typeof b.text !== "string") continue;
      const start = turns.find((t) => t.kind === "turn.start" && t.principal === asked && t.cause?.channel === "chat" && t.cause?.seq === r.seq);
      const end = start && turns.find((t) => t.kind === "turn.end" && t.principal === asked && t.turn === start.turn);
      out.push({ pair: name, seq: r.seq, asker: r.principal, asked, text: b.text.trim().split("\n")[0], returnTo: b.return_to, at: r.at, turn: start?.turn ?? null, startAt: start?.at ?? null, end: end ? { outcome: end.outcome, error: end.error ?? null, at: end.at } : null });
    }
    return out;
  }

  // Reads again the pair chats whose rows say their threads changed (each
  // one's reads at once; a pair chat whose read failed is read at its next
  // change); then the lines are drawn.
  async function refresh() {
    const mine = ++reading;
    const pairs = chats().filter(isPair);
    for (const name of [...reads.keys()]) if (!pairs.some((f) => f.name === name)) reads.delete(name);
    const due = pairs.filter((f) => reads.get(f.name)?.signature !== signature(f));
    await Promise.all(due.map(async (f) => {
      const sig = signature(f);
      const threads = await threadsOf(f.name).catch(() => null);
      if (threads) reads.set(f.name, { signature: sig, threads });
    }));
    await answersCame().catch(() => {});
    if (mine === reading) rendered();
  }
  // Each ended thread's answer, looked for in the chat it was asked from
  // once that chat's row changes, or a thread of it is newly seen to end
  // (its answer may have come before the pair's row said so: #779 review):
  // its answer on its asker's `tasks` (`an:<asked turn>`,
  // docs/chat-records.md), then the asker's turn of that record in the
  // asking chat, ended.
  async function answersCame() {
    const waiting = [...reads.values()].flatMap((r) => r.threads).filter((t) => t.end && !answeredTurns.has(t.turn));
    const checkOf = (name) => JSON.stringify([signature(byName(name)), waiting.filter((t) => t.returnTo === name).map((t) => t.turn).sort()]);
    const chatsOf = [...new Set(waiting.map((t) => t.returnTo))].filter((name) => byName(name) && answerChecks.get(name) !== checkOf(name));
    await Promise.all(chatsOf.map(async (name) => {
      const check = checkOf(name);
      const ours = waiting.filter((t) => t.returnTo === name);
      const work = await api("GET", `/api/f/${seg(name)}/channels/work?before=${NEWEST}&limit=${WORK_RECORDS}`);
      for (const asker of new Set(ours.map((t) => t.asker))) {
        const agent = agentNamed(asker);
        if (!agent) continue;
        const tasks = await api("GET", `/api/f/${seg(agent.fragment)}/channels/tasks?before=${NEWEST}&limit=${TASK_RECORDS}`);
        for (const t of ours.filter((x) => x.asker === asker)) {
          const answer = (tasks.records ?? []).find((r) => r.principal === asker && r.body?.kind === "answer" && r.body?.turn === t.turn);
          const start = answer && (work.records ?? []).find((r) => r.principal === asker && r.body?.kind === "turn.start" && r.body?.cause?.fragment === agent.fragment && r.body?.cause?.channel === "tasks" && r.body?.cause?.seq === answer.seq);
          if (start && (work.records ?? []).some((r) => r.principal === asker && r.body?.kind === "turn.end" && r.body?.turn === start.body.turn)) answeredTurns.add(t.turn);
        }
      }
      answerChecks.set(name, check);
    }));
  }

  // A thread's state: how its turn ended, else a card waiting for the person
  // (the asked agent's newest record in the pair chat), else working, or
  // asked while its turn has not started.
  function stateOf(t) {
    if (t.end) return t.end.outcome === "idle" ? "done" : t.end.outcome === "stopped" ? "stopped" : "lost";
    if (!t.turn) return "asked";
    const newest = (byName(t.pair)?.listed ?? []).find((r) => r.channel === "work" && r.principal === t.asked);
    return newest?.kind === "turn.prompt" ? "you" : "working";
  }
  // Whether its answer has come (`answersCame`).
  const answered = (t) => !!t.end && answeredTurns.has(t.turn);
  // The threads asked from chat `name` still to show, the oldest first.
  const shownIn = (name) => [...reads.values()].flatMap((r) => r.threads).filter((t) => t.returnTo === name && !answered(t)).sort((x, y) => x.at - y.at);
  // The agent chat `name` waits on, while a thread asked from it runs.
  const waitingOn = (name) => {
    const t = shownIn(name).find((x) => !x.end);
    return t ? agentNamed(t.asked) : null;
  };
  const agentNamed = (identity) => ownAgents().find((a) => a.identity === identity) ?? null;

  // How long it has run: from its turn's start (its question's, before it
  // started) to its end, else now.
  function took(t) {
    const ms = Math.max(0, (t.end?.at ?? Date.now()) - (t.startAt ?? t.at));
    const m = Math.max(1, Math.round(ms / 60_000));
    return m < 60 ? `${m}m` : `${Math.floor(m / 60)}h ${m % 60}m`;
  }
  // Stop: its asked agent's owner's `stop` in the pair chat (a person's
  // Stop is its asker's, the asking agent's: docs/chat-records.md, "A
  // command"), one record however often it is pressed.
  const stopping = new Set();
  async function stop(t, button) {
    stopping.add(t.turn);
    button.disabled = true;
    button.textContent = "Stopping…";
    try {
      await api("POST", `/api/f/${seg(t.pair)}/channels/chat`, { id: `stop:${t.turn}`, body: { kind: "command", command: "stop", to: [t.asked] } });
    } catch {
      stopping.delete(t.turn);
      button.disabled = false;
      button.textContent = "Stop";
    }
  }
  function line(t) {
    const agent = agentNamed(t.asked);
    const state = stateOf(t);
    const row = el("div", "thread");
    row.dataset.key = `${t.pair}:${t.seq}`;
    row.dataset.state = state;
    const open = el("button", "thread-open");
    open.type = "button";
    open.append(avatar(agentFace(agent?.fragment, t.asked)), el("span", "thread-who", agent ? titleOf(agent.fragment) : ""), el("span", "thread-what", t.text), el("span", `thread-state ${state}`, STATE_SAYS[state]), el("span", "thread-took", took(t)));
    if (t.end?.error) open.title = t.end.error;
    open.onclick = () => { openChat(t.pair); leaveSidebar(); };
    row.append(open);
    if ((state === "working" || state === "you") && t.turn) {
      const halt = el("button", "quiet thread-stop", stopping.has(t.turn) ? "Stopping…" : "Stop");
      halt.type = "button";
      halt.disabled = stopping.has(t.turn);
      halt.onclick = () => stop(t, halt);
      row.append(halt);
    }
    return row;
  }
  // The open chat's lines, over it; drawn again each minute while one runs.
  let tick = 0;
  function render() {
    clearTimeout(tick);
    const name = !page() && current();
    const shown = name ? shownIn(name) : [];
    patch($("threads"), shown.map(line));
    $("threads").hidden = !shown.length;
    if (shown.some((t) => !t.end)) tick = setTimeout(render, 30_000);
  }
  return { isPair, refresh, render, waitingOn };
}
