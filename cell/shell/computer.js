import { icon } from "./icons.js";
import { active, createRestartFollow, FAILED, READ_FAILED } from "./restart-follow.js";
export const PHASE = { asleep: "Asleep", starting: "Starting", awake: "Awake", sleeping: "Going to sleep", wont_wake: "Won't start" };
// Settings' State: its phase, but a restart's save is Saving, not Going to
// sleep (Paul on #1053), while a restart this page asked for is followed
// (`restarting`: restart-follow.js's `active`, never a failure kept on
// screen); any other sleep, or one asked elsewhere, is going to sleep
export function stateSaid(c, restarting) {
  return c.phase === "sleeping" && restarting ? "Saving" : PHASE[c.phase] ?? c.phase;
}

// A notice's countdown (`until`, FIN-458) in words at `now`: the server's
// words, then how long until its time, in whole minutes up; the last
// minute, under a minute.
export function untilSaid(u, now) {
  const left = u.at - now;
  if (left <= 60_000) return `${u.said} in under a minute.`;
  const minutes = Math.ceil(left / 60_000);
  if (minutes <= 60) return `${u.said} in ${minutes} min.`;
  const h = Math.floor(minutes / 60);
  return `${u.said} in ${h} h${minutes % 60 ? ` ${minutes % 60} min` : ""}.`;
}

export function createComputer({ api, el, seg, named, section, line, actionLine, computer, notices, agents, computerFailed, setupPending, made, computerIs, openScreen, markOffer, reads, load, openPage, timers = globalThis, now = () => Date.now() }) {
  // ---- the computer: awake while the person is here, and its updates ----
  let woke = 0;
  // asking for a wake as the person arrives or focuses a chat hides the
  // computer's start (decision 39's pre-wake, from the shell)
  // The person's computer, made once (making it again is the same one:
  // docs/api.md).
  let making = null;
  function computerOf() {
    making ??= api("POST", "/api/computers", {}).then((c) => { made(c); return c; });
    making.catch(() => { making = null; });
    return making;
  }
  // A computer that won't start is not woken by the person's presence: its
  // starts kept failing, so the next is theirs to ask for (Restart, in its
  // notice), never one every visit pays for unasked. While the first run
  // makes the agent, its computer starts once it is in its chat (the
  // platform's `joined`), with the agent assigned, not before.
  function prewake() {
    if (setupPending()) return;
    const c = computer();
    if (!c || document.hidden || Date.now() - woke < 60_000 || ["awake", "starting", "wont_wake"].includes(c.phase)) return;
    woke = Date.now();
    api("POST", `/api/computers/${seg(c.computer)}/wake`, {}).then(computerIs).catch(() => {});
  }
  document.addEventListener("visibilitychange", prewake);
  // ---- what the person is told (FIN-458, `GET /api/notices`): each
  // notice in the server's own words, the most pressing first, drawn the
  // same way whatever its kind, with its actions. Each owner tells this
  // page's list socket when its notices change, and the page reads them
  // again (shell.js `load`). Beside them, the page's own rows: a deploy, a
  // read that failed, a restart under way. ----
  // A restart the person asked for in this page (a notice's Restart, or
  // Settings'), followed until the computer says it is done
  // (restart-follow.js: its states and their ways out), shown where its
  // notice was as the phase the computer reports; past `SLOW_MS` a line
  // says a big one takes minutes. Hidden, the page reads nothing.
  const SLOW_MS = 30_000;
  const follow = createRestartFollow({
    read: () => (computer() ? api("GET", `/api/computers/${seg(computer().computer)}`) : Promise.reject(new Error("no computer"))),
    ask: (asked) => api("POST", `/api/computers/${seg(computer().computer)}/restart`, { generation: asked }),
    view: () => computer(),
    // a view unchanged draws only the restart's line (its slow line comes)
    take: (v) => (v?.computer && JSON.stringify(v) !== JSON.stringify(computer()) ? computerIs(v) : renderComputer()),
    changed: () => renderComputer(),
    hidden: () => document.hidden,
  });
  const followVisibility = () => follow.visibility();
  document.addEventListener("visibilitychange", followVisibility);
  // whether the last drawing showed a restart: one gone since is done
  let drewRestart = false;
  // what the restart is doing, as the computer last said
  function restartStep(c, op) {
    if (c?.phase === "sleeping") return "Saving…";
    if (c?.phase === "starting") return "Starting…";
    // a start of the restart's that failed, tried again after a pause
    if (c?.phase === "asleep" && c.generation > op.asked) return "Starting again…";
    return "Restarting…";
  }
  // A notice's action that restarts the computer is its restart, followed
  // (its progress is the page's own row, not a notice's): the one route
  // the band knows, never a kind.
  const restarts = (a) => !!computer() && a.post === `/api/computers/${seg(computer().computer)}/restart`;
  // Settings' Computer as last built: its State's words, and its Restart's
  // buttons, painted again as the computer changes (`paintSettings`); its
  // buttons only when the restart's step changed, so a Restart now asked
  // stays until pressed or cancelled.
  const shown = { section: null, state: null, asks: null, said: null, painted: null };
  // a button with its icon before its words; `words`, the span its words
  // are in (a named one: renderNames rewrites only its text)
  const iconed = (name, words, cls = "quiet") => {
    const b = el("button", `${cls} with-icon`);
    b.type = "button";
    b.append(icon(name), typeof words === "string" ? el("span", null, words) : words);
    return b;
  };
  // the notices whose last action failed, by id, and why: the server's own
  // words for its refusal (Paul, 2026-11-09), else the page's for a call
  // that never reached it; said under their words until one goes through
  // or the notice is gone
  const failed = new Map();
  // the countdowns as last drawn, each its line and its time (`countdown`)
  let counting = [];
  let tick = null;
  // A notice, as the server said it: its title, its words, its countdown,
  // the platform's own words small, and its actions, the first the main one.
  function noticeRow(n) {
    const row = el("div", n.level === "warn" ? "computer-notice warn" : "computer-notice");
    row.dataset.kind = n.kind;
    row.dataset.id = n.id;
    const copy = el("div", "notice-copy");
    const actions = el("div", "notice-actions");
    copy.append(el("strong", null, n.title));
    if (n.text) copy.append(el("p", null, n.text));
    if (n.until) {
      const line = el("p", null, untilSaid(n.until, now()));
      counting.push({ u: n.until, line });
      copy.append(line);
    }
    if (failed.has(n.id)) copy.append(el("p", null, failed.get(n.id)));
    if (n.detail) copy.append(el("p", "notice-detail", n.detail));
    (n.actions ?? []).forEach((a, i) => actions.append(actionOf(n, a, i === 0)));
    row.append(copy, actions);
    return row;
  }
  function actionOf(n, a, main) {
    if (a.open) {
      const link = el("a", null, a.label);
      link.href = a.open;
      link.target = "_blank";
      link.rel = "noopener";
      return link;
    }
    const b = el("button", main ? "primary" : null, a.label);
    b.type = "button";
    b.onclick = () => act(n, a, b);
    return b;
  }
  // An action: its route posted with its body, then the notices read again,
  // one more read in the page's sequence (`load`: the newest wins), which
  // decides what shows whatever the post answered. Failed, the notice says
  // why, as that read finds it, its button back to tap again. Its answer
  // may name a page to open (Paul, 2026-11-09: an invite's app, a
  // checkout), opened once that read has it (an app accepted is in the list).
  async function act(n, a, button) {
    // the start the server named, so a stale row's press restarts nothing newer
    if (restarts(a)) return restartComputer(a.body?.generation);
    button.disabled = true;
    let answer = null;
    try {
      answer = await api("POST", a.post, a.body ?? {});
      failed.delete(n.id);
    } catch (e) {
      failed.set(n.id, e?.status ? e.message : "Couldn't do that. Try again.");
    }
    // as a change told this page: only the cards whose rows it touched read again
    await load(true).catch(() => {});
    if (typeof answer?.open === "string") openPage(answer.open);
  }
  // The countdowns, live: each line said again as its minute changes (in
  // place: a button under the person's finger stays), and the band drawn
  // again as the first ends, so an idle page drops a notice whose time
  // passed. One timer at a time.
  function countdown() {
    if (tick !== null) timers.clearTimeout(tick);
    tick = null;
    const t = now();
    if (counting.some((c) => c.u.at <= t)) return renderComputer();
    for (const c of counting) c.line.textContent = untilSaid(c.u, t);
    const next = counting.map((c) => {
      const left = c.u.at - t;
      return left > 60_000 ? left - (Math.ceil(left / 60_000) - 1) * 60_000 : left;
    });
    if (next.length) tick = timers.setTimeout(countdown, Math.min(...next));
  }
  // the page's own rows: a deploy, a read that failed, a restart under way
  function pageRow(kind) {
    const row = el("div", "computer-notice");
    row.dataset.kind = kind;
    const copy = el("div", "notice-copy");
    const actions = el("div", "notice-actions");
    const button = (label, action, onclick) => {
      const b = el("button", "primary", label);
      b.type = "button";
      b.dataset.action = action;
      b.onclick = onclick;
      actions.append(b);
    };
    if (kind === "restart") {
      const op = follow.op();
      if (op.state === FAILED) {
        row.classList.add("warn");
        copy.append(el("strong", null, "Your computer didn't restart"), el("p", null, op.error));
        button("Try again", "restart", () => restartComputer());
      } else if (op.state === READ_FAILED) {
        // a read that failed: no news of the restart, which may go on
        copy.append(el("strong", null, "Couldn't check on the restart"), el("p", "notice-detail", op.error));
        button("Check again", "restart-read", () => follow.readAgain());
      } else {
        const step = restartStep(computer(), op);
        copy.append(el("strong", null, "Restarting your computer"));
        if (Date.now() - op.at >= SLOW_MS) copy.append(el("p", null, step === "Saving…" ? "A big save can take a few minutes." : "A big restore can take a few minutes."));
        const status = el("span", "restart-status", step);
        status.setAttribute("role", "status");
        actions.append(status);
      }
    } else if (kind === "deploying") {
      copy.append(el("strong", null, "Updating — back in a moment"));
    } else {
      copy.append(el("strong", null, computerFailed() || reads().noticesFailed ? "Couldn't refresh your computer" : "Couldn't refresh your apps"));
      const again = el("button", "primary", "Try again");
      again.type = "button";
      again.dataset.action = "refresh";
      again.onclick = () => readAgain(again);
      actions.append(again);
    }
    row.append(copy, actions);
    return row;
  }
  // a read that failed (a blip, a 5xx) says so first, over what the last
  // read said, with Try again; a confirmed deploy waits by itself
  function renderComputer() {
    // a view taken from elsewhere (the list's read) may say it is done
    follow.check();
    const restart = !!follow.op();
    if (drewRestart && !restart) redrawSection();
    drewRestart = restart;
    const box = document.getElementById("computer-notices");
    const t = now();
    const all = notices() ?? [];
    for (const id of failed.keys()) if (!all.some((n) => n.id === id)) failed.delete(id);
    // a notice a restart answers waits while it runs; one whose time passed is gone
    const told = all.filter((n) => !(n.until && n.until.at <= t) && !(restart && (n.actions ?? []).some(restarts)));
    counting = [];
    // a read that failed, its notices' too: what the last said stays, under its line
    const own = reads().updating ? ["deploying"] : computerFailed() || reads().listFailed || reads().noticesFailed ? ["refresh"] : [];
    const rows = [...[...own, ...(restart ? ["restart"] : [])].map(pageRow), ...told.map(noticeRow)];
    box.replaceChildren(...rows);
    box.hidden = !rows.length;
    countdown();
    paintSettings();
  }
  // The restart, done: awake on a newer start, or a newer start that won't
  // (its notice then says so, with Try again). Settings' Computer, shown,
  // is drawn again in place (its version, the latest), under its heading,
  // which settings' links name: nothing else read again, nothing moved.
  function redrawSection() {
    const old = shown.section;
    const c = computer();
    if (!old?.isConnected || !c) return;
    const fresh = computerSection(c);
    old.replaceChildren(old.querySelector(":scope > h2"), ...[...fresh.children].slice(1));
    shown.section = old;
  }
  // Try again: the list and the computer read again (`load`, which goes on
  // with what the failure put off). The list's read failing too, the line
  // stays, to try again.
  async function readAgain(button) {
    button.disabled = true;
    reads().since = null;
    reads().wait = 3000;
    await load().catch(() => {});
    button.disabled = false;
  }
  // Restart: the computer saves if it can, then starts fresh from its newest
  // good save. It names the start the person saw (`asked`: a notice's own,
  // else the view's), so pressed twice (or in two tabs, or on a stale row)
  // it restarts once. Its progress shows where its notice was (`follow`),
  // and so does its ask's failure, with Try again.
  function restartComputer(asked) {
    follow.press(asked);
  }

  function computerSection(c) {
    // the version it runs (asleep: the one it starts on), and the latest when
    // it runs an older one (docs/computers.md, "What its owner is told")
    const behind = !!(c?.version && c.latest && c.version !== c.latest);
    const computer = section(
      "Computer",
      ...(c
        ? [
            stateLine(c),
            line("Version", c.version ?? c.latest ?? c.image),
            ...(behind
              ? [line("Latest version", c.latest), el("p", "muted", `Restart to get the latest version.${c.alwaysOn ? "" : " It also updates by itself the next time your computer sleeps."}`)]
              : []),
            ...(c.why ? [el("p", "settings-warning", c.why)] : []),
          ]
        : // never read: a failed read is not "none" (shown again once one answers: `load`)
          [el("p", "muted", computerFailed() ? "Couldn't load your computer." : "Your computer starts with your first agent.")]),
    );
    // a row of Computer: its label, a short line under it, and its buttons
    // on the right
    const row = (key, sub, ...buttons) => {
      const line = el("div", "settings-line settings-line-with-action");
      const info = el("div", "settings-info");
      info.append(el("span", "settings-key", key), el("span", "settings-value settings-sub", sub));
      const control = el("div", "settings-control");
      control.append(...buttons);
      line.append(info, control);
      return line;
    };
    // each agent's own desktop, where its runtime offers one (`markOffer`:
    // agent-runtimes.js), and the line gone with the last
    const screens = [];
    for (const a of c ? agents().values() : []) {
      const screen = iconed("screen", named(el("span"), a.fragment, (t) => `Open ${t}'s screen`));
      screen.dataset.screenFor = a.fragment;
      screen.onclick = () => openScreen(a.fragment);
      screens.push(markOffer(screen, a.fragment, "screen"));
    }
    if (screens.length) {
      const shown = row("Screens", "Watch your agent work on its own desktop, and take over.", ...screens);
      shown.dataset.offers = "";
      shown.hidden = screens.every((s) => s.hidden);
      computer.append(shown);
    }
    // the way back to working when something is wrong with it: asked once
    // more before it cuts what its agents are doing
    if (c) {
      const asks = el("span", "settings-actions");
      const restartRow = row("Restart", "Saves your computer if it can, then starts it fresh from that save. Anything its agents are doing stops.", asks);
      // why its ask failed, under its words (never beside its button, which
      // would squeeze them)
      const said = el("p", "form-error");
      restartRow.querySelector(".settings-info").append(said);
      Object.assign(shown, { asks, said });
      paintAsks(asks);
      computer.append(restartRow);
    }
    shown.section = computer;
    return computer;
  }
  function stateLine(c) {
    const l = line("State", stateSaid(c, active(follow.op())));
    shown.state = l.querySelector(".settings-value");
    return l;
  }
  function paintSettings() {
    const c = computer();
    if (shown.state?.isConnected && c) shown.state.textContent = stateSaid(c, active(follow.op()));
    if (shown.asks?.isConnected && restartSaid() !== shown.painted) paintAsks(shown.asks);
  }
  // what settings' Restart says now: nothing but its button, the restart's
  // step, or why its ask failed
  function restartSaid() {
    const op = follow.op();
    return !op ? "" : op.state === FAILED || op.state === READ_FAILED ? `${op.state}: ${op.error}` : restartStep(computer(), op);
  }
  function paintAsks(asks) {
    const step = restartSaid();
    const op = follow.op();
    shown.painted = step;
    shown.said.textContent = op?.state === FAILED ? `It didn't restart: ${op.error}` : op?.state === READ_FAILED ? `Couldn't check on the restart: ${op.error}` : "";
    shown.said.hidden = !shown.said.textContent;
    if (op?.state === READ_FAILED) {
      const check = iconed("reload", "Check again");
      check.dataset.action = "restart-read";
      check.onclick = () => follow.readAgain();
      asks.replaceChildren(check);
      return;
    }
    if (op && op.state !== FAILED) {
      const status = el("span", "restart-status", step);
      status.setAttribute("role", "status");
      asks.replaceChildren(status);
      return;
    }
    const go = iconed("reload", op ? "Try again" : "Restart computer");
    go.dataset.action = "restart-ask";
    go.onclick = () => {
      const sure = iconed("reload", "Restart now");
      sure.dataset.action = "restart";
      sure.onclick = () => restartComputer();
      const cancel = el("button", "quiet", "Cancel");
      cancel.type = "button";
      cancel.onclick = () => asks.replaceChildren(go);
      asks.replaceChildren(cancel, sure);
    };
    asks.replaceChildren(go);
  }
  return { computerOf, prewake, renderComputer, restartComputer, computerSection, dispose: () => {
    document.removeEventListener("visibilitychange", prewake);
    document.removeEventListener("visibilitychange", followVisibility);
    if (tick !== null) timers.clearTimeout(tick);
    follow.dispose();
  } };
}
