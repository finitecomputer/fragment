import { icon } from "./icons.js";
import { createRestartFollow, FAILED, READ_FAILED } from "./restart-follow.js";
export const PHASE = { asleep: "Asleep", starting: "Starting", awake: "Awake", sleeping: "Going to sleep", wont_wake: "Won't start" };

const size = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

// What the person is told of a disk filling (the `disk_filling` notice),
// here too, in one line (Paul's words): the room left, and what to do.
export function diskSaid(n) {
  return `Your computer is almost full (${size(n.free)} left). Delete files you don't need, or ask an agent to tidy up.`;
}

export function createComputer({ api, el, seg, named, section, line, actionLine, computer, agents, computerFailed, support, setupPending, made, computerIs, openScreen, markOffer, reads, load }) {
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
  // ---- what the person should know of their computer (docs/computers.md,
  // "What its owner is told"): the view's notices, the most pressing first,
  // each in plain words with the one way back to working. Its computer tells
  // this page's list socket when they change, so they show while there is
  // still time. Times are the person's own clock. ----
  const clock = (ms) => {
    const d = new Date(ms);
    const today = d.toDateString() === new Date().toDateString();
    const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    return today ? time : `${d.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
  };
  // why a computer won't start, in plain words (the platform's own words
  // stay beside them, for whoever helps); no time it may not keep: a big
  // restore takes a start up to 15 minutes (#1001)
  function wontStart(why) {
    if (/not ready within/.test(why)) return "It didn't finish starting in time.";
    if (/kept stopping/.test(why)) return "It kept stopping as soon as it started.";
    if (/check/.test(why)) return "Its saved files didn't pass their check.";
    if (/restore|archive|backup/i.test(why)) return "Its save couldn't be restored.";
    return "It couldn't start.";
  }
  // a restart the person asked for in this page, and of which start: a
  // computer that still won't start after one says where to get help
  let restarted = 0;
  // That restart, followed until the computer says it is done
  // (restart-follow.js: its states and their ways out), shown where its
  // buttons were as the phase the computer reports; past `SLOW_MS` a line
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
  // the notices a restart answers (each with a Restart): not shown while it runs
  const RESTARTS = new Set(["wont_wake", "unsaved", "update"]);
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
  function noticeRow(n) {
    const row = el("div", "computer-notice");
    row.dataset.kind = n.kind;
    const copy = el("div", "notice-copy");
    const actions = el("div", "notice-actions");
    const say = (title, text, detail) => {
      copy.append(el("strong", null, title), el("p", null, text));
      if (detail) copy.append(el("p", "notice-detail", detail));
    };
    const restartButton = (label) => {
      const b = el("button", "primary", label);
      b.type = "button";
      b.dataset.action = "restart";
      b.onclick = () => restartComputer();
      actions.append(b);
    };
    if (n.kind === "restart") {
      const op = follow.op();
      if (op.state === FAILED) {
        row.classList.add("warn");
        say("Your computer didn't restart", op.error);
        restartButton("Try again");
      } else if (op.state === READ_FAILED) {
        // a read that failed: no news of the restart, which may go on
        copy.append(el("strong", null, "Couldn't check on the restart"), el("p", "notice-detail", op.error));
        const check = el("button", "primary", "Check again");
        check.type = "button";
        check.dataset.action = "restart-read";
        check.onclick = () => follow.readAgain();
        actions.append(check);
      } else {
        const step = restartStep(computer(), op);
        copy.append(el("strong", null, "Restarting your computer"));
        if (Date.now() - op.at >= SLOW_MS) copy.append(el("p", null, step === "Saving…" ? "A big save can take a few minutes." : "A big restore can take a few minutes."));
        const status = el("span", "restart-status", step);
        status.setAttribute("role", "status");
        actions.append(status);
      }
    } else if (n.kind === "wont_wake") {
      row.classList.add("warn");
      const again = restarted && computer()?.generation > restarted;
      say(
        again ? "Your computer still won't start" : "Your computer won't start",
        `${wontStart(n.why)} Your agents can't answer until it does.${again ? " If restarting again doesn't help, get help and give them the details below." : ""}`,
        `${computer()?.computer ?? ""}: ${n.why}`,
      );
      restartButton(again ? "Try again" : "Restart");
      if (again && support()) {
        const help = el("a", null, "Get help");
        help.href = support();
        help.target = "_blank";
        help.rel = "noopener";
        actions.append(help);
      }
    } else if (n.kind === "unsaved") {
      row.classList.add("warn");
      const then = n.save ? "starts again from that save" : "starts again with nothing kept";
      const stop = n.stopsAt ? ` If it still can't by ${clock(n.stopsAt)}, it stops and ${then}.` : "";
      say("Your computer can't save right now", `It hasn't saved since ${clock(n.since)}, so what its agents did after that isn't kept yet. It keeps trying.${stop} Restarting tries to save first.`, n.why);
      restartButton("Restart now");
    } else if (n.kind === "went_back") {
      // told only when something was done on it after that save (a stop
      // that lost nothing tells nothing), so it says what went, calmly
      const since = n.save && n.savedAt ? clock(n.savedAt) : null;
      const back = n.pending
        ? (since ? `When it starts again, changes on it since ${since} won't be there.` : "Nothing on it was saved yet, so it will start fresh.")
        : (since ? `Changes on it since ${since} weren't kept.` : "Nothing on it was saved yet, so it started fresh.");
      say(n.pending ? "Your computer stopped" : "Your computer restarted", `${back} Your chats are all here.`);
      const ok = el("button", null, "OK");
      ok.type = "button";
      ok.dataset.action = "seen";
      ok.onclick = () => sawNotice(n.life, ok);
      actions.append(ok);
    } else if (n.kind === "update") {
      // told only between turns, never restarted behind their back; an
      // always-on computer never sleeps, so never updates by itself. No
      // time promised: a restart restores its work, a big one up to 15
      // minutes (#1001)
      const itself = computer()?.alwaysOn ? "" : " It also updates by itself the next time your computer sleeps.";
      say("An update is ready for your computer", `Restart to get it; your chats and work are kept.${itself}`);
      restartButton("Restart");
      const later = el("button", null, "Later");
      later.type = "button";
      later.dataset.action = "later";
      later.onclick = () => updateLater(n.version, later);
      actions.append(later);
    } else if (n.kind === "disk_filling") {
      row.classList.add("warn");
      copy.append(el("strong", null, diskSaid(n)));
    } else if (n.kind === "deploying") {
      copy.append(el("strong", null, "Updating — back in a moment"));
    } else if (n.kind === "refresh") {
      copy.append(el("strong", null, computerFailed() ? "Couldn't refresh your computer" : "Couldn't refresh your apps"));
      const again = el("button", "primary", "Try again");
      again.type = "button";
      again.dataset.action = "refresh";
      again.onclick = () => readAgain(again);
      actions.append(again);
    } else return null;
    row.append(copy, actions);
    return row;
  }
  // a read of it that failed (a blip, a 5xx) says so first, over what the
  // last read said, with Try again; a confirmed deploy waits by itself
  function renderComputer() {
    // a view taken from elsewhere (the list's read) may say it is done
    follow.check();
    const restart = !!follow.op();
    if (drewRestart && !restart) redrawSection();
    drewRestart = restart;
    const box = document.getElementById("computer-notices");
    const told = (computer()?.notices ?? []).filter((n) => !(restart && RESTARTS.has(n.kind)));
    const rows = [...(reads().updating ? [{ kind: "deploying" }] : computerFailed() || reads().listFailed ? [{ kind: "refresh" }] : []), ...(restart ? [{ kind: "restart" }] : []), ...told].map(noticeRow).filter(Boolean);
    box.replaceChildren(...rows);
    box.hidden = !rows.length;
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
  // good save. It names the start the person saw, so pressed twice (or in two
  // tabs) it restarts once. Its progress shows where its buttons were
  // (`follow`), and so does its ask's failure, with Try again.
  function restartComputer() {
    const asked = computer()?.generation || 0;
    if (follow.press()) restarted = asked;
  }
  // The person read what a start went back to: told no more, anywhere.
  async function sawNotice(life, button) {
    const c = computer();
    if (!c) return;
    button.disabled = true;
    try {
      computerIs(await api("POST", `/api/computers/${seg(c.computer)}/notices/seen`, { life }));
    } catch {
      computerIs(await api("GET", `/api/computers/${seg(c.computer)}`).catch(() => null));
    }
  }
  // Later: this update is told no more, anywhere, until another is ready.
  async function updateLater(version, button) {
    const c = computer();
    if (!c) return;
    button.disabled = true;
    try {
      computerIs(await api("POST", `/api/computers/${seg(c.computer)}/notices/later`, { version }));
    } catch {
      computerIs(await api("GET", `/api/computers/${seg(c.computer)}`).catch(() => null));
    }
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
    const l = line("State", PHASE[c.phase] ?? c.phase);
    shown.state = l.querySelector(".settings-value");
    return l;
  }
  function paintSettings() {
    const c = computer();
    if (shown.state?.isConnected && c) shown.state.textContent = PHASE[c.phase] ?? c.phase;
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
    follow.dispose();
  } };
}
