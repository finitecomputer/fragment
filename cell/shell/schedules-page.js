// The Schedules page (`/schedules`; Paul, 2026-11-10): every scheduled job
// across the person's own fragments, read-only, in one list: the soonest
// next run first, the paused last. A job is
// - a scheduled record (`GET /api/f/{name}/schedules`): an agent's routine
//   (`{kind: "routine", text}` on its agent fragment's `tasks`, named by
//   its text's first line) or any other (named by its channel);
// - an app's cron trigger (`GET /api/f/{name}/triggers`), named by the job
//   it runs.
// A row says its name, its schedule in words (the platform's `said`, in
// UTC), its next run in the person's own time, and its fragment, which it
// opens; "Paused" on a paused one. Pausing and removing stay where they
// are (the CLI, an agent's own tools).
//
// Each fragment asked is read on each open, a few at a time, and the page
// drawn as each answers; one that failed is one quiet line, and the rest
// stand. What is asked: the person's own fragments but chats, skills and
// session archives (`asked`), so a person with many chats wakes none.
//
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `seg`, `art` (a fragment's icon), `named`, `fragments` (the person's
// list), `openApp` and `leaveSidebar`.

// fragments read at once
export const READS_AT_ONCE = 4;
// a routine's name: its text's first line, at most this many characters
const NAME_MAX = 80;

export const asked = (f) => f.role === "owner" && f.kind !== "chat" && f.kind !== "skills" && f.kind !== "sessions";

const firstLine = (text) => (typeof text === "string" ? text.split("\n").map((l) => l.trim()).find(Boolean) ?? "" : "");
const cut = (s) => (s.length > NAME_MAX ? `${s.slice(0, NAME_MAX - 1).trimEnd()}…` : s);

// A scheduled record's name: a routine's text, else its channel.
export function scheduleName(s) {
  const line = s.record?.kind === "routine" ? firstLine(s.record.text) : "";
  return line ? cut(line) : s.channel;
}

// The rows of what each fragment answered (`answers`: name → {schedules,
// triggers}, or {failed: true}), soonest first, paused last; and the
// fragments that failed, in the order given.
export function scheduleRows(answers) {
  const rows = [];
  const failed = [];
  for (const [fragment, a] of answers) {
    if (a.failed) {
      failed.push(fragment);
      continue;
    }
    for (const s of a.schedules ?? []) {
      rows.push({ key: `schedule:${fragment}:${s.id}`, fragment, name: scheduleName(s), said: s.said, nextAt: s.paused ? null : s.nextAt ?? null, paused: !!s.paused });
    }
    for (const t of a.triggers ?? []) {
      if (typeof t.cron !== "string") continue;
      rows.push({ key: `cron:${fragment}:${t.run}:${t.cron}`, fragment, name: t.run, said: t.said ?? t.cron, nextAt: t.paused ? null : t.nextAt ?? null, paused: !!t.paused });
    }
  }
  const at = (r) => (r.paused ? Infinity : r.nextAt ?? Number.MAX_SAFE_INTEGER);
  rows.sort((a, b) => Number(a.paused) - Number(b.paused) || at(a) - at(b) || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
  return { rows, failed };
}

// A next run in the person's own time: "Today 9:00 AM", else its day.
export function nextSaid(at, now = Date.now()) {
  const d = new Date(at), today = new Date(now);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === today.toDateString()) return `Today ${time}`;
  const year = d.getFullYear() === today.getFullYear() ? {} : { year: "numeric" };
  return d.toLocaleString([], { weekday: "short", month: "short", day: "numeric", ...year, hour: "numeric", minute: "2-digit" });
}

export function createSchedulesPage(page, h) {
  const { el } = h;
  let shown = false;
  // each open is a visit: what answers for an earlier one changes nothing
  let visit = 0;
  let answers = new Map();
  let reading = 0;
  let parts = null;

  function build() {
    const wrap = el("div", "schedules-layout");
    const head = el("header", "schedules-intro");
    head.append(el("h1", null, "Schedules"));
    const list = el("div", "schedules-list");
    const failed = el("div", "schedules-failed");
    wrap.append(head, list, failed);
    page.replaceChildren(wrap);
    parts = { list, failed };
  }

  function row(r) {
    const open = el("button", "schedules-row");
    open.type = "button";
    open.dataset.key = r.key;
    const face = el("span", "schedules-art");
    face.append(h.art(r.fragment));
    const main = el("span", "schedules-main");
    const line = el("span", "schedules-line");
    line.append(el("span", "label", r.name));
    if (r.paused) line.append(el("span", "schedules-chip", "Paused"));
    if (r.nextAt) {
      const time = el("time", null, nextSaid(r.nextAt));
      time.dateTime = new Date(r.nextAt).toISOString();
      time.title = new Date(r.nextAt).toLocaleString();
      line.append(time);
    }
    const sub = el("span", "schedules-sub");
    sub.append(el("span", "schedules-said", r.said.charAt(0).toUpperCase() + r.said.slice(1)), h.named(el("span", "schedules-fragment"), r.fragment));
    main.append(line, sub);
    open.append(face, main);
    open.onclick = () => {
      h.openApp(r.fragment);
      h.leaveSidebar();
    };
    return open;
  }

  function render() {
    if (!shown) return;
    if (!parts) build();
    const { rows, failed } = scheduleRows(answers);
    if (rows.length) parts.list.replaceChildren(...rows.map(row));
    else parts.list.replaceChildren(...(reading ? [] : [el("p", "muted schedules-empty", "No schedules")]));
    parts.failed.replaceChildren(...failed.map((name) => h.named(el("p", "muted"), name, (t) => `${t} was not read`)));
  }

  // One fragment's schedules and cron triggers, kept for this visit.
  async function read(name, mine) {
    const at = `/api/f/${h.seg(name)}`;
    const answer = await Promise.all([h.api("GET", `${at}/schedules`), h.api("GET", `${at}/triggers`)]).then(
      ([s, t]) => ({ schedules: s.schedules, triggers: t.triggers }),
      () => ({ failed: true }),
    );
    if (mine !== visit) return;
    answers.set(name, answer);
    render();
  }

  // Every fragment asked, `READS_AT_ONCE` at a time.
  async function readAll() {
    const mine = ++visit;
    answers = new Map();
    const names = h.fragments().filter(asked).map((f) => f.name);
    reading = names.length;
    let next = 0;
    const worker = async () => {
      while (next < names.length && mine === visit) {
        await read(names[next++], mine);
        if (mine === visit) reading--;
      }
    };
    render();
    await Promise.all(Array.from({ length: Math.min(READS_AT_ONCE, names.length) }, worker));
    if (mine === visit) render();
  }

  return {
    open() {
      shown = true;
      readAll();
    },
    close() {
      shown = false;
      ++visit;
    },
  };
}
