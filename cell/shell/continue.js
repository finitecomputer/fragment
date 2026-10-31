// Continue (docs/chat-records.md, "History"): a conversation the person
// had, picked up as a new chat with one of their agents, of their
// choosing. Its source is a conversation of their Session Archive
// (`{ archive, key }`, an import: templates/sessions/README.md) or an
// archived chat of theirs (`{ chat }`). The new chat's first record is its
// `history`: where it came from, the last summary and the newest of what
// was said after it, bounded (`historyOf`, `historyOfChat`), which the
// chat's page shows above the chat and the agent is handed at its first
// turn there. The agent joins after it.
//
// One chat a (source, agent): its name is made from both (`nameFor`: the
// agent's chat label and a hash of the two), so a Continue asked again, in
// any tab or on any device, opens that chat, and finishes there whatever a
// try before it left undone. Each step can be taken again: a create that
// finds its name taken by the person's own chat of this source goes on in
// it, the history is posted with the same id, the agent added by a PUT.
// A name some other fragment holds (a hash's collision) gives way to one
// made at random, kept in the browser until its Continue is done.

// A history record's body, at most, as the cell measures a record's (its
// JSON: `limits::RECORD_BODY_MAX_BYTES`, 64 KiB), whatever its text is.
export const HISTORY_BODY_MAX_BYTES = 63 * 1024;
const LIMITS = { messages: 40, text: 4_000, summary: 8_000, title: 120 };
// a chat's records read for its history, at most (a page each)
const CHAT_RECORDS_READ = 500;
const bytesOf = (v) => new TextEncoder().encode(JSON.stringify(v)).length;
const cutTo = (text, max) => ([...text].length > max ? `${[...text].slice(0, max - 1).join("")}…` : text);
const iso = (ms) => (Number.isFinite(ms) ? new Date(ms).toISOString() : undefined);

// `base` with the newest of `said` that fit beside it, oldest first, each
// adding its JSON and a comma; a summary too large for the body alone cut
// until it fits; `earlier` how many others `all` messages hold.
function fill(base, said, all) {
  while (base.summary && bytesOf(base) > HISTORY_BODY_MAX_BYTES) base.summary = cutTo(base.summary, Math.floor([...base.summary].length / 2));
  let size = bytesOf({ ...base, messages: [], earlier: all });
  const tail = [];
  for (const m of [...said].reverse()) {
    if (tail.length >= LIMITS.messages) break;
    const one = { role: m.role, at: m.at, text: cutTo(m.text.trim(), LIMITS.text) };
    const more = bytesOf(one) + (tail.length ? 1 : 0);
    if (size + more > HISTORY_BODY_MAX_BYTES) break;
    size += more;
    tail.unshift(one);
  }
  return { ...base, messages: tail, earlier: all - tail.length };
}

// An imported conversation's history (`c`, its file): its last summary
// covers what was said before it, so the card holds it, then the newest of
// what was said after it. `agent`: who the conversation was with, when not
// the agent continuing it.
export function historyOf(c, key, archive, agent) {
  const all = c.messages ?? [];
  const isSaid = (m) => (m.role === "user" || m.role === "assistant") && typeof m.text === "string" && m.text.trim() && !m.summary && !m.tool_calls?.length;
  const last = all.findLastIndex((m) => m.summary && typeof m.text === "string" && m.text.trim());
  const summary = last >= 0 ? all[last].text : "";
  const base = {
    kind: "history", title: cutTo(c.title || "Untitled", LIMITS.title), archive, key, started_at: c.started_at, ended_at: c.ended_at,
    ...(agent ? { agent: cutTo(agent, LIMITS.title) } : {}), ...(summary ? { summary: cutTo(summary, LIMITS.summary) } : {}),
  };
  return fill(base, all.slice(last + 1).filter(isSaid), all.filter(isSaid).length);
}

// An archived chat's history, from its `chat` records (docs/chat-records.md:
// a body with text and no kind but "message"), each an agent's (`isAgent`
// its principal) or a person's.
export function historyOfChat(records, chat, title, isAgent, agent) {
  const said = records
    .filter((r) => r.body && typeof r.body === "object" && (r.body.kind ?? "message") === "message" && typeof r.body.text === "string" && r.body.text.trim())
    .map((r) => ({ role: isAgent(r.principal) ? "assistant" : "user", at: iso(r.at), text: r.body.text }));
  const base = {
    kind: "history", title: cutTo(title || "Untitled", LIMITS.title), chat, started_at: iso(records[0]?.at), ended_at: iso(records.at(-1)?.at),
    ...(agent ? { agent: cutTo(agent, LIMITS.title) } : {}),
  };
  return fill(base, said, said.length);
}

// A chat's name from (source, agent): the agent's chat label and four
// characters of a hash of both, then four more as its suffix (decision
// 47's alphabet: `fragment_proto::NAME_SUFFIX_ALPHABET`).
const SUFFIX = "abcdefghijkmnpqrstuvwxyz23456789";
export async function nameFor(source, agent, base) {
  const id = source.chat ? `chat:${source.chat}` : `archive:${source.archive}/${source.key}`;
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`${id}\n${agent}`)));
  const four = (at) => [0, 1, 2, 3].map((i) => SUFFIX[hash[at + i] & 31]).join("");
  return `${base}-${four(0)}--${four(4)}`;
}
const random = () => [...crypto.getRandomValues(new Uint8Array(4))].map((b) => SUFFIX[b & 31]).join("");

export function createContinue({
  api, ui: { el, notice }, data: { allFragments, agents: computerAgents, primaryAgent, titleOf, seg, chatBase },
  navigation: { openChat, leaveSidebar }, load: loadList, store,
}) {
  // a chat's title, as the sidebar's room has it
  const TITLE_MAX = 48;
  // the chat a Continue not yet finished is making, by source and agent,
  // when its own name was another's (`nameFor`)
  const PENDING = "shell.continue.pending.v1";
  const status = (name) => api("GET", `/api/f/${seg(name)}/status`).catch((e) => (e.status === 404 ? null : Promise.reject(e)));
  const fileOf = (archive, path) => api("GET", `/api/f/${seg(archive)}/file?path=${encodeURIComponent(path)}`);
  const slotOf = (source, agent) => `${source.chat ? `chat ${source.chat}` : `${source.archive} ${source.key}`} ${agent}`;

  // the person's own agents, as their computer runs them
  function agents() {
    return [...computerAgents().values()].filter((a) => a.fragment).map((a) => ({ identity: a.identity, fragment: a.fragment, title: titleOf(a.fragment) }));
  }
  const titled = (identity) => agents().find((a) => a.identity === identity)?.title ?? "";
  const same = (a, b) => typeof a === "string" && typeof b === "string" && a.trim().toLocaleLowerCase() === b.trim().toLocaleLowerCase();

  // the agent to offer first: an import's, the person's agent of its name
  // (`source.agent`, as its archive's index names it; read from its file
  // when the source does not say); an archived chat's, its lead agent when
  // it is theirs; else their main one
  const conversations = new Map();
  const conversation = (source) => {
    const at = `${source.archive} ${source.key}`;
    if (!conversations.has(at)) conversations.set(at, fileOf(source.archive, `sessions/${source.key}.json`).catch(() => null));
    return conversations.get(at);
  };
  async function defaultAgent(source) {
    const mine = agents();
    if (source.chat) {
      const lead = allFragments().find((f) => f.name === source.chat)?.agents?.[0];
      if (mine.some((a) => a.identity === lead)) return lead;
    } else {
      const named = "agent" in source ? source.agent : (await conversation(source))?.agent;
      const match = mine.find((a) => same(a.title, named));
      if (match) return match.identity;
    }
    return primaryAgent()?.identity ?? mine[0]?.identity ?? null;
  }

  // the chat a Continue of `source` with `agent` made, as the person's list
  // has it, or null
  async function continued(source, agent) {
    const fragment = computerAgents().get(agent)?.fragment;
    if (!fragment) return null;
    const pending = store.get(PENDING, {})[slotOf(source, agent)];
    const names = [await nameFor(source, agent, chatBase(fragment)), pending].filter(Boolean);
    return names.find((n) => allFragments().some((f) => f.name === n && f.kind === "chat" && f.role === "owner")) ?? null;
  }

  // whether the chat named `name` is this Continue's: the person's own, its
  // first record (when it has one) this source's history
  async function ours(name, source) {
    if ((await status(name))?.role !== "owner") return false;
    const first = (await api("GET", `/api/f/${seg(name)}/channels/chat?after=0&limit=1`)).records?.[0];
    const b = first?.body;
    return !first || (b?.kind === "history" && (source.chat ? b.chat === source.chat : b.archive === source.archive && b.key === source.key));
  }

  // a Continue from before chats were named by their source (one archive
  // marker, `continued/<key>.json`): its chat, when it is with `agent`
  async function marked(source, agent) {
    if (source.chat) return null;
    const m = await fileOf(source.archive, `continued/${source.key}.json`).catch(() => null);
    const chat = typeof m?.chat === "string" ? allFragments().find((f) => f.name === m.chat && f.kind === "chat") : null;
    return chat?.agents?.includes(agent) ? chat.name : null;
  }

  // the source's history and the new chat's title, read now
  async function historyFor(source, agent) {
    if (source.chat) {
      const row = allFragments().find((f) => f.name === source.chat);
      const page = await api("GET", `/api/f/${seg(source.chat)}/channels/chat?before=${Number.MAX_SAFE_INTEGER}&limit=${CHAT_RECORDS_READ}`);
      const theirs = new Set(row?.agents ?? []);
      const lead = row?.agents?.[0];
      const other = lead && lead !== agent ? titled(lead) || "another agent" : "";
      const title = row?.title || titleOf(source.chat);
      return { title, body: historyOfChat(page.records ?? [], source.chat, title, (p) => theirs.has(p) || computerAgents().has(p), other) };
    }
    const c = await conversation(source);
    if (!c) throw new Error("its conversation could not be read");
    const other = c.agent && !same(c.agent, titled(agent)) ? c.agent : "";
    return { title: c.title || "Untitled", body: historyOf(c, source.key, source.archive, other) };
  }

  let busy = false;
  // makes (or finishes, or opens) the chat of `source` with `agent`: its
  // name once it is open, else null (said in a notice)
  async function continueInto(source, agent) {
    const fragment = computerAgents().get(agent)?.fragment;
    if (busy) return null;
    if (!fragment) {
      notice("No agent yet", "Continue needs one of your agents.");
      return null;
    }
    busy = true;
    const slot = slotOf(source, agent);
    const remember = (name) => {
      const all = store.get(PENDING, {});
      if (name) all[slot] = name;
      else delete all[slot];
      store.set(PENDING, all);
    };
    let made = false;
    try {
      const before = await marked(source, agent);
      if (before) {
        await loadList();
        openChat(before);
        leaveSidebar();
        return before;
      }
      // bounded before anything is made
      const { title, body } = await historyFor(source, agent);
      let name = store.get(PENDING, {})[slot] ?? (await nameFor(source, agent, chatBase(fragment)));
      try {
        await api("POST", "/api/fragments", { name, template: "chat", title: cutTo(title, TITLE_MAX) });
      } catch (e) {
        // its answer lost, it may be made: the next try finds it by its name
        if (!(e.status >= 400)) made = true;
        if (e.status !== 409) throw e;
        // this source's chat with this agent, made before (or by a try whose
        // answer was lost); else a name another holds: one at random, kept
        if (!(await ours(name, source))) {
          name = `${chatBase(fragment)}-${random()}--${random()}`;
          remember(name);
          await api("POST", "/api/fragments", { name, template: "chat", title: cutTo(title, TITLE_MAX) });
        }
      }
      made = true;
      // posted by a try before (the same id; another body when the source changed since)
      await api("POST", `/api/f/${seg(name)}/channels/chat`, { id: "history", body }).catch((e) => (e.status === 409 ? null : Promise.reject(e)));
      // adding it wakes its computer (the platform's `joined`)
      await api("PUT", `/api/f/${seg(name)}/members/${seg(agent)}`, { role: "editor" });
      remember(null);
      await loadList();
      openChat(name);
      leaveSidebar();
      return name;
    } catch (e) {
      if (made) notice("The chat is not ready yet", `${e.message} Continue again to finish it.`);
      else notice("The chat was not made", e.message);
      return null;
    } finally {
      busy = false;
    }
  }

  // "Continue with [agent ▾]": one line, for a History row or the archive's
  // ask; "Continue" alone when the person has one agent, "Open" once the
  // chosen agent's chat of this source is made. `done(name)` once its chat
  // is open.
  function continueControl(source, done = () => {}) {
    const box = el("div", "continue-control");
    const go = el("button", "continue-go", "Continue");
    go.type = "button";
    const mine = agents();
    if (!mine.length) {
      go.disabled = true;
      go.title = "Continue needs one of your agents";
      box.append(go);
      return box;
    }
    const pick = el("select", "continue-agent");
    pick.setAttribute("aria-label", "The agent to continue with");
    for (const a of mine) {
      const o = el("option", null, a.title);
      o.value = a.identity;
      pick.append(o);
    }
    const said = () => {
      const open = !!go.dataset.chat;
      go.textContent = box.classList.contains("busy") ? "Continuing…" : open ? "Open" : mine.length > 1 ? "Continue with" : "Continue";
    };
    const refresh = async () => {
      go.dataset.chat = (await continued(source, pick.value).catch(() => null)) ?? "";
      said();
    };
    go.onclick = async () => {
      if (box.classList.contains("busy")) return;
      if (go.dataset.chat) {
        openChat(go.dataset.chat);
        leaveSidebar();
        return done(go.dataset.chat);
      }
      box.classList.add("busy");
      go.disabled = pick.disabled = true;
      said();
      let name = null;
      try {
        name = await continueInto(source, pick.value);
      } finally {
        box.classList.remove("busy");
        go.disabled = pick.disabled = false;
        await refresh();
      }
      if (name) done(name);
    };
    pick.onchange = () => void refresh();
    box.append(go);
    if (mine.length > 1) box.append(pick);
    void defaultAgent(source).then((identity) => {
      if (identity) pick.value = identity;
      return refresh();
    });
    return box;
  }

  // The archive's page asks to continue one of its conversations: the same
  // control, in a dialog of its own.
  function ask(source, title) {
    const dialog = el("dialog", "continue-ask");
    const close = el("button", "continue-close", "Close");
    close.type = "button";
    close.onclick = () => dialog.close();
    dialog.append(el("p", "continue-title", `Continue “${title || "this conversation"}”`), continueControl(source, () => dialog.close()), close);
    dialog.addEventListener("close", () => dialog.remove());
    document.body.append(dialog);
    dialog.showModal();
    return dialog;
  }

  return { agents, defaultAgent, continued, continueInto, continueControl, ask };
}
