export const PHASE = { asleep: "Asleep", starting: "Starting", awake: "Awake", sleeping: "Going to sleep", wont_wake: "Won't start" };

export function createComputer({ api, el, seg, named, section, line, actionLine, computer, agents, computerFailed, support, setupPending, made, computerIs, reopenSettings, openScreen, reads, load }) {
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
  // stay beside them, for whoever helps)
  function wontStart(why) {
    if (/not ready within/.test(why)) return "It didn't finish starting within a few minutes.";
    if (/kept stopping/.test(why)) return "It kept stopping as soon as it started.";
    if (/check/.test(why)) return "Its saved files didn't pass their check.";
    if (/restore|archive|backup/i.test(why)) return "Its save couldn't be restored.";
    return "It couldn't start.";
  }
  // a restart the person asked for in this page, and of which start: a
  // computer that still won't start after one says where to get help
  let restarted = 0;
  function noticeRow(n) {
    const row = el("div", "computer-notice");
    row.dataset.kind = n.kind;
    const copy = el("div", "notice-copy");
    const actions = el("div", "notice-actions");
    const say = (title, text, detail) => {
      copy.append(el("strong", null, title), el("p", null, text));
      if (detail) copy.append(el("p", "notice-detail", detail));
    };
    const restart = (label) => {
      const b = el("button", "primary", label);
      b.type = "button";
      b.dataset.action = "restart";
      b.onclick = () => restartComputer(b);
      actions.append(b);
    };
    if (n.kind === "wont_wake") {
      row.classList.add("warn");
      const again = restarted && computer()?.generation > restarted;
      say(
        again ? "Your computer still won't start" : "Your computer won't start",
        `${wontStart(n.why)} Your agents can't answer until it does.${again ? " If restarting again doesn't help, get help and give them the details below." : ""}`,
        `${computer()?.computer ?? ""}: ${n.why}`,
      );
      restart(again ? "Try again" : "Restart");
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
      restart("Restart now");
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
      // always-on computer never sleeps, so never updates by itself
      const itself = computer()?.alwaysOn ? "" : " It also updates by itself the next time your computer sleeps.";
      say("An update is ready for your computer", `Restart to get it — about a minute; your chats and work are kept.${itself}`);
      restart("Restart");
      const later = el("button", null, "Later");
      later.type = "button";
      later.dataset.action = "later";
      later.onclick = () => updateLater(n.version, later);
      actions.append(later);
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
    const box = document.getElementById("computer-notices");
    const rows = [...(reads().updating ? [{ kind: "deploying" }] : computerFailed() || reads().listFailed ? [{ kind: "refresh" }] : []), ...(computer()?.notices ?? [])].map(noticeRow).filter(Boolean);
    box.replaceChildren(...rows);
    box.hidden = !rows.length;
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
  // tabs) it restarts once. What went wrong is said beside the button.
  async function restartComputer(button) {
    const c = computer();
    if (!c) return;
    const buttons = [...document.querySelectorAll("[data-action=restart]")];
    for (const b of buttons) b.disabled = true;
    const label = button.textContent;
    button.textContent = "Restarting…";
    try {
      restarted = c.generation || 0;
      computerIs(await api("POST", `/api/computers/${seg(c.computer)}/restart`, { generation: c.generation || 0 }));
      await reopenSettings();
    } catch (e) {
      button.textContent = label;
      const said = button.parentElement.querySelector(".form-error") ?? el("p", "form-error");
      said.textContent = `It did not restart: ${e.message}`;
      button.parentElement.append(said);
    } finally {
      for (const b of buttons) b.disabled = false;
    }
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
            line("State", PHASE[c.phase] ?? c.phase),
            line("Version", c.version ?? c.latest ?? c.image),
            ...(behind
              ? [line("Latest version", c.latest), el("p", "muted", `Restart to get the latest version.${c.alwaysOn ? "" : " It also updates by itself the next time your computer sleeps."}`)]
              : []),
            ...(c.why ? [el("p", "settings-warning", c.why)] : []),
          ]
        : // never read: a failed read is not "none" (shown again once one answers: `load`)
          [el("p", "muted", computerFailed() ? "Couldn't load your computer." : "Your computer starts with your first agent.")]),
    );
    // each agent's own desktop
    const screens = [];
    for (const a of c ? agents().values() : []) {
      const screen = named(el("button", "quiet"), a.fragment, (t) => `Open ${t}'s screen`);
      screen.type = "button";
      screen.dataset.screenFor = a.fragment;
      screen.onclick = () => openScreen(a.fragment);
      screens.push(screen);
    }
    if (screens.length) computer.append(actionLine("Screens", ...screens));
    // the way back to working when something is wrong with it: asked once
    // more before it cuts what its agents are doing
    if (c) {
      computer.append(el("p", "muted", "Restarting saves your computer if it can, then starts it fresh from that save. Anything its agents are doing stops."));
      const row = el("span", "settings-actions");
      const go = el("button", "quiet", "Restart computer");
      go.type = "button";
      go.dataset.action = "restart-ask";
      go.onclick = () => {
        const sure = el("button", "quiet", "Restart now");
        sure.type = "button";
        sure.dataset.action = "restart";
        sure.onclick = () => restartComputer(sure);
        const cancel = el("button", "quiet", "Cancel");
        cancel.type = "button";
        cancel.onclick = () => row.replaceChildren(go);
        row.replaceChildren(sure, cancel);
      };
      row.append(go);
      computer.append(actionLine("Restart", row));
    }
    return computer;
  }
  return { computerOf, prewake, renderComputer, restartComputer, computerSection, dispose: () => document.removeEventListener("visibilitychange", prewake) };
}
