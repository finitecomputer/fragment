import { billingSections, returnSaid, returning } from "./billing.js";
import { dataSection } from "./exports.js";

export function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

export function section(title, ...children) {
  const s = el("section", "settings-section");
  s.append(el("h2", null, title), ...children);
  return s;
}
export function line(label, value, action) {
  const p = el(action ? "div" : "p", action ? "settings-line settings-line-with-action" : "settings-line");
  const key = el("span", "settings-key", label);
  const text = el("span", "settings-value", value);
  if (action) {
    const info = el("div", "settings-info");
    info.append(key, text);
    const control = el("div", "settings-control");
    control.append(action);
    p.append(info, control);
  } else p.append(key, text);
  return p;
}
export function actionLine(label, ...actions) {
  const row = el("div", "settings-line settings-action-line");
  const key = el("span", "settings-key");
  key.append(label);
  const control = el("div", "settings-control");
  control.append(...actions);
  row.append(key, control);
  return row;
}
// a sentence whose `quoted` parts are code
function say(text) {
  const p = el("p");
  text.split("`").forEach((part, i) => p.append(i % 2 ? el("code", null, part) : part));
  return p;
}

export function createSettings({ api, $, seg, account, navigation, notices, sections, agents, releaseInfo, allFragments, own, freeLabel, load, named, openApp }) {
  const { person, setPerson, isGuest, setGuest, setSeat, support, seatIs, switchForm, noSeatSaid } = account;
  const { at, path: SETTINGS, isOpen, enter, entered } = navigation;
  const { hideNotice, noticeCurrent, notice } = notices;
  const { computerSection, connectionsSection, simplexSection } = sections;
  const { getComputer, getAgents, faceControls, agentSettings, addAgentForm, renderAgentFaces, paintHomes } = agents;
  // ---- settings: the person's account, credit, computer and agents ----
  const usd = (micros) => (micros / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD" });
  // what a standing short of `ok` stops, and why (crates/core/src/ledger.rs `Refused`)
  const STOPPED = { agents_stopped: "Your agents are stopped", read_only: "Your fragments are read-only, and your agents are stopped" };
  const WHY = {
    seat_canceled: "the seat was canceled",
    no_credit: "the credit is used up; adding credit starts them again",
    overdrawn: "the balance reached the overdraft; credit that brings it above zero ends it",
  };
  // The CLI's one-line install, for macOS and Linux (cli/SKILL.md's: `cargo
  // xtask check` holds the two to each other and to the release's assets),
  // and a coding agent's skill, in Claude Code's folder: behind settings'
  // "Install it yourself", for whoever wants them (a coding agent given the
  // prompt reads /llms.txt and installs it itself).
  const INSTALL = "mkdir -p ~/.local/bin && curl -fsSL https://github.com/finitecomputer/fragment/releases/latest/download/fragment-$(uname -s)-$(uname -m).tar.gz | tar -xzf - -C ~/.local/bin";
  const SKILL = "mkdir -p ~/.claude/skills/fragment && fragment skill > ~/.claude/skills/fragment/SKILL.md";
  // Finite's privacy policy and terms (`/privacy`, `/terms`), small and
  // muted, each in a tab of its own, at the end of Settings' Account (the
  // first run's footer, index.html, has About, Contact and these)
  function legalLinks() {
    const p = el("p", "muted legal-links");
    const pages = [["/privacy", "Privacy"], ["/terms", "Terms"]];
    pages.forEach(([href, text], i) => {
      const a = el("a", null, text);
      a.href = href;
      a.target = "_blank";
      a.rel = "noopener";
      p.append(...(i ? [" · ", a] : [a]));
    });
    return p;
  }
  // The release this page is (`releaseInfo().mine`, its commit linked on GitHub,
  // the repo being public), when it was deployed, and the version its
  // computer runs, small and muted at the end of Settings: what whoever
  // looks is looking at (Paul, 2026-10-24). None on an unfilled page.
  const COMMIT = /^[0-9a-f]{7,40}$/;
  function releaseLine(c) {
    if (!releaseInfo().mine) return [];
    const p = el("p", "muted settings-release");
    p.dataset.release = releaseInfo().mine;
    let id = releaseInfo().mine;
    if (COMMIT.test(id)) {
      id = el("a", null, releaseInfo().mine);
      id.href = `https://github.com/finitecomputer/fragment/commit/${releaseInfo().mine}`;
      id.target = "_blank";
      id.rel = "noopener";
    }
    p.append("Release ", id);
    if (releaseInfo().deployed) {
      const at = new Date(releaseInfo().deployed * 1000);
      const t = el("time", null, at.toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }));
      t.dateTime = at.toISOString();
      p.append(" · ", t);
    }
    const v = c?.version ?? c?.latest;
    if (v) p.append(` · Computer ${v}`);
    return [p];
  }
  function renderSettingsPage(page, sections) {
    const layout = el("div", "settings-layout");
    const nav = el("nav", "settings-nav");
    nav.setAttribute("aria-label", "Settings sections");
    nav.append(el("span", "settings-nav-title", "Settings"));
    const content = el("div", "settings-content");
    const intro = el("header", "settings-intro");
    intro.append(el("h1", null, "Settings"), el("p", null, isGuest() ? "Manage your account and billing." : "Manage your account, your computer, and the tools your agents use."));
    content.append(intro);
    const links = [];
    sections.forEach((section, index) => {
      const heading = section.querySelector(":scope > h2");
      if (heading) {
        if (!section.id) section.id = `settings-section-${index}`;
        heading.tabIndex = -1;
        const jump = el("button", "settings-nav-link", heading.textContent);
        jump.type = "button";
        // a hidden section (SimpleX on an image without it) hides its link too
        jump.dataset.section = section.id;
        jump.hidden = section.hidden;
        jump.onclick = () => {
          section.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "start" });
          heading.focus({ preventScroll: true });
        };
        nav.append(jump);
        links.push([section, jump]);
      }
      content.append(section);
    });
    layout.append(nav, content);
    page.replaceChildren(layout);
    page.onscroll = () => {
      const visible = links.filter(([section]) => !section.hidden);
      const top = page.getBoundingClientRect().top + page.clientTop + 130;
      const current = [...visible].reverse().find(([section]) => section.getBoundingClientRect().top <= top)?.[1] ?? visible[0]?.[1];
      if (current === nav.querySelector('[aria-current="location"]')) return;
      for (const [, button] of links) {
        if (button === current) button.setAttribute("aria-current", "location");
        else button.removeAttribute("aria-current");
      }
      if (current) {
        const button = current.getBoundingClientRect();
        const bounds = nav.getBoundingClientRect();
        if (button.left < bounds.left) nav.scrollLeft -= bounds.left - button.left;
        else if (button.right > bounds.right) nav.scrollLeft += button.right - bounds.right;
      }
    };
    page.onscroll();
  }
  // Each load of settings is numbered, and only the latest renders, while
  // settings are shown: an older one that answers last (an action's reload, a
  // second press, Back) read older state, and is dropped (#441).
  let settingsLoads = 0;
  // Each visit to settings, from another page (never a reload of them): an
  // action's failure is said only in the visit it was pressed in (#439).
  let visits = 0;
  // A read of settings that failed on its way (no answer reached the page, or
  // a gateway's or a deploy's 502, 503, 504) is asked again by itself, calmly
  // (3 s doubling to 30 s, for READ_AGAIN_MS from the first such failure).
  // Until every one answers, settings say what could not be read; then they
  // are drawn again once, as an action's reload draws them. A failure that
  // would come again (a 4xx, a 500) is said as it is (#775: one seat read
  // that never reached the node left "Your seat could not be read" up for
  // good).
  const READ_AGAIN_MS = 8 * 60_000;
  const again = { since: null, wait: 3000, timer: null };
  const passing = (e) => !e?.status || [502, 503, 504].includes(e.status);
  const read = (path) => api("GET", path).then((value) => ({ path, value }), (error) => ({ path, error }));
  function readAgain(ticket, failed) {
    clearTimeout(again.timer);
    if (!failed.length) {
      again.since = null;
      again.wait = 3000;
      return;
    }
    if (!failed.every((r) => passing(r.error))) return;
    again.since ??= Date.now();
    if (Date.now() - again.since >= READ_AGAIN_MS) return;
    again.timer = setTimeout(async () => {
      const current = () => ticket === settingsLoads && isOpen();
      if (!current()) return;
      const now = await Promise.all(failed.map((r) => read(r.path)));
      if (!current()) return;
      if (now.some((r) => r.error)) return readAgain(ticket, now.filter((r) => r.error));
      again.since = null;
      again.wait = 3000;
      reopenSettings();
    }, again.wait);
    again.wait = Math.min(again.wait * 2, 30_000);
  }
  // `failed`: what the action that asked for this load says once it renders
  // (a failure's notice, [title, text]).
  async function openSettings(push = true, failed = null) {
    const ticket = ++settingsLoads;
    const lost = () => ticket !== settingsLoads || !isOpen();
    if (push) at(SETTINGS);
    // settings render with no notice: one of the page they came from stays
    // there, an action that worked takes its earlier failure's away, and
    // one that failed says so once this load renders, if nothing newer (a
    // load, a page, a notice) has come since it began
    const gen = hideNotice();
    const tell = () => {
      if (failed && noticeCurrent(gen)) notice(...failed);
    };
    if (!isOpen()) {
      visits++;
      // a visit's reads are its own: asked again from its first failure
      clearTimeout(again.timer);
      again.since = null;
      again.wait = 3000;
    }
    enter();
    const page = $("settings-page");
    page.hidden = false;
    // in the loaded page's own layout, so nothing moves as it loads
    renderSettingsPage(page, [el("p", "muted settings-loading", "Loading…")]);
    entered();
    // a credit pack's Checkout returning, before the ledger is read (its
    // settlement every load awaits, said by the one that renders)
    const billingHelpers = { api, el, section, line, actionLine, usd, reopen: () => reopenSettings(), noSeatSaid, switchForm, support: support(), seatIs };
    const back = await returning(billingHelpers);
    if (lost()) return;
    const reads = await Promise.all([
      read("/api/ledger"),
      read("/api/seat"),
      read("/api/connections"),
      getComputer() ? read(`/api/computers/${seg(getComputer().computer)}/uses`) : { value: null },
    ]);
    if (lost()) return;
    const [ledger, mySeat, linked, uses] = reads.map((r) => r.value ?? null);
    // what failed on its way is read again by itself once these render
    const settle = () => readAgain(ticket, reads.filter((r) => r.error));
    if (mySeat) setGuest(mySeat.guest);
    setSeat(mySeat?.seat ?? null);
    const emails = (person().subjects ?? []).map((x) => x.email).filter(Boolean);
    // who is signed in (their latest sign-in's email), signing out, and
    // another account: a seat may wait under another email
    const signout = el("form");
    signout.method = "post";
    signout.action = "/auth/logout";
    const out = el("button", "quiet", "Sign out");
    out.type = "submit";
    signout.append(out);
    const ways = el("span", "settings-actions");
    ways.append(signout, switchForm(SETTINGS));
    const who = line("Signed in as", "", ways);
    who.classList.add("settings-who", "settings-stack");
    who.querySelector(".settings-value").append(person().email ? el("strong", null, person().email) : "—");
    const id = line("Identity", person().id);
    id.lastChild.classList.add("mono");
    const account = section("Account", who, id);
    // a picture: PNG, JPEG, WebP or GIF, at most 256 KiB (PUT /api/identities/me/picture)
    const picture = el("label", "quiet picture");
    const shown = el("img");
    shown.alt = "";
    shown.hidden = !person().picture;
    if (person().picture) shown.src = person().picture;
    const file = el("input");
    file.type = "file";
    file.accept = "image/png,image/jpeg,image/webp,image/gif";
    file.hidden = true;
    picture.append(shown, el("span", null, person().picture ? "Change your picture" : "Add a picture"), file);
    file.onchange = async () => {
      const f = file.files?.[0];
      if (!f) return;
      try {
        await api("PUT", "/api/identities/me/picture", f, f.type);
      } catch (e) {
        picture.querySelector("span").textContent = e.message || "That picture was not taken";
        return;
      }
      setPerson(await api("GET", "/api/identities/me"));
      reopenSettings();
    };
    account.append(actionLine("Picture", picture));
    // their sign-ins, when they linked several before linking went (off
    // until WorkOS offers an account picker: docs/api.md, Sign-in)
    if (emails.length > 1) account.append(line("Sign-ins", emails.join(", ")));
    account.append(legalLinks());
    // the person's seat, credit and org (billing.js), and why agents stop
    const billing = await billingSections(billingHelpers, ledger, mySeat, back);
    if (lost()) return;
    // a guest has no agents to stop, nor a computer, skills or connections
    // for them, nor apps to publish: a seat to get comes first, then who they
    // are, then everything that is theirs, to download (what is shared with
    // them, named); Your data is built only by a load that renders, as it
    // reads its export again while one builds
    if (isGuest()) {
      returnSaid(back);
      renderSettingsPage(page, [...billing, account, dataSection({ api, el, section, line, actionLine, isOpen: () => isOpen() }), ...releaseLine(null)]);
      settle();
      return tell();
    }
    const standing = ledger?.standing?.standing;
    if (standing && standing !== "ok") {
      billing[0].append(el("p", "settings-warning", [STOPPED[standing] ?? "Your agents are stopped", WHY[ledger.standing.why]].filter(Boolean).join(": ") + "."));
    }
    const c = getComputer();
    const computer = computerSection(c);
    const agents = section("Agents");
    const mine = [...getAgents().values()];
    // the models a provider the person connected offers (its own credit pays)
    const modeled = (linked?.providers ?? []).filter((p) => p.models?.length && p.state === "set");
    if (mine.length && modeled.length) agents.append(say("Each agent runs on the platform's own model, included in your credit, unless you pick one of a provider you connected, which your account there pays for."));
    agents.append(...(mine.length ? mine.flatMap((a) => {
      const row = el("button", "row", "Open");
      row.type = "button";
      row.onclick = () => openApp(a.fragment);
      const name = el("span", "settings-agent-name");
      name.dataset.agentFace = a.fragment;
      const [actions, said] = faceControls(a);
      return [agentSettings(a, actionLine(name, actions, row), modeled), said];
    }) : [el("p", "muted", "None yet.")]), ...(mine.length ? addAgentForm() : []));
    const connections = connectionsSection(linked, uses);
    const skills = await skillsSection();
    if (lost()) return;
    returnSaid(back);
    renderSettingsPage(page, [account, ...billing, computer, agents, skills, connections, simplexSection(), buildSection(), dataSection({ api, el, section, line, actionLine, isOpen: () => isOpen() }), ...releaseLine(c)]);
    renderAgentFaces();
    // a home read that answered while these were built shown now they are in
    paintHomes();
    settle();
    tell();
  }
  // An action's reload of settings, only while they are shown: an answer that
  // comes once the person has gone to a chat leaves them there, and says
  // nothing there (#439). `failed`: openSettings'.
  async function reopenSettings(failed = null) {
    if (isOpen()) await openSettings(false, failed);
  }

  // Building with a coding agent: one prompt that names this platform's
  // /llms.txt (its handoff: the install, `fragment login --host` here, the
  // manual), copied into Claude Code or Codex with what to build. The
  // install line stays, folded, for whoever runs it themselves.
  function buildSection() {
    const prompt = el("textarea", "build-prompt");
    prompt.id = "build-prompt";
    prompt.rows = 3;
    prompt.spellcheck = false;
    prompt.setAttribute("aria-label", "A prompt for your coding agent");
    prompt.value = `Read ${location.origin}/llms.txt and help me build: `;
    const copy = el("button", "quiet", "Copy");
    copy.type = "button";
    copy.dataset.action = "copy-prompt";
    // a clipboard that refuses (no permission, a webview's, none at all) is
    // said under the prompt, which is selected to copy by hand; Copy stays
    // Copy, to try again (#438)
    const said = el("p", "form-error");
    said.id = "build-prompt-said";
    said.setAttribute("role", "status");
    said.hidden = true;
    // each press, numbered: one that answers after a newer press changes
    // nothing (that one says how it went)
    let presses = 0;
    copy.onclick = async () => {
      const mine = ++presses;
      said.hidden = true;
      try {
        await navigator.clipboard.writeText(prompt.value);
      } catch {
        if (mine !== presses) return;
        copy.textContent = "Copy";
        said.textContent = "Couldn't copy it. The prompt is selected: copy it yourself (Ctrl+C, or ⌘C on a Mac).";
        said.hidden = false;
        prompt.focus();
        prompt.select();
        return;
      }
      if (mine !== presses) return;
      copy.textContent = "Copied";
      setTimeout(() => {
        if (mine === presses) copy.textContent = "Copy";
      }, 1500);
    };
    const actions = el("div", "settings-actions");
    actions.append(copy);
    const yourself = el("details", "settings-install");
    yourself.append(
      el("summary", null, "Install it yourself"),
      say("On macOS or Linux (on Windows, in WSL), then pair it with you:"),
      el("pre", "command", `${INSTALL}\nfragment login --host ${location.origin}`),
      say("To give Claude Code its skill:"),
      el("pre", "command", SKILL),
    );
    return section(
      "Build with Claude Code or Codex",
      say("Paste this into your coding agent and finish the sentence: it sets up the `fragment` command line itself, and you approve it here once."),
      prompt,
      actions,
      said,
      yourself,
    );
  }

  // ---- skills (decision 17): the managed set, the person's skills fragment's
  // (the platform's release, which their computers install for every agent),
  // by category, and each agent's own (its fragment's skills/, which win on a
  // name). Both read as the agents' computers read them: the fragments' files.
  // A skill is skills/<category>/<name>/SKILL.md, or skills/<name>/SKILL.md.
  function skillsIn(paths) {
    const out = [];
    for (const path of paths) {
      const parts = path.split("/");
      if (parts[0] !== "skills" || parts[parts.length - 1] !== "SKILL.md") continue;
      if (parts.length === 4) out.push({ category: parts[1], name: parts[2] });
      else if (parts.length === 3) out.push({ category: "", name: parts[1] });
    }
    return out.sort((a, b) => a.category.localeCompare(b.category) || a.name.localeCompare(b.name));
  }
  const skillsFragmentOf = () => allFragments().find((f) => f.kind === "skills" && own(f));
  // The person's skills fragment, made from the blessed template when they
  // have none: at setup, as their default agent is, or from settings' Skills
  // for a person without one (deleted, or set up before 2026-10-03).
  async function skillsFragment() {
    const have = skillsFragmentOf();
    if (have) return have.name;
    const made = await api("POST", "/api/fragments", { label: freeLabel("skills"), template: "skills" });
    await load();
    return made.name;
  }
  function skillNames(list) {
    const value = el("span", "settings-value");
    list.forEach((s, i) => {
      if (i) value.append(", ");
      const name = el("span", "skill", s.name);
      name.dataset.skill = s.name;
      value.append(name);
    });
    return value;
  }
  async function skillsSection() {
    const s = section("Skills");
    s.id = "settings-skills";
    const filesOf = (name) => api("GET", `/api/f/${name}/files`).then((v) => (v.files ?? []).map((f) => f.path)).catch(() => null);
    const mine = skillsFragmentOf();
    if (!mine) {
      const add = el("button", "quiet", "Add the managed skills");
      add.type = "button";
      add.onclick = () => skillsFragment().then(() => reopenSettings()).catch((e) => { add.textContent = e.message; });
      s.append(el("p", "muted", "Your agents have no managed skills yet."), actionLine("Managed skills", add));
    } else {
      const paths = await filesOf(mine.name);
      if (!paths) s.append(el("p", "muted", "Your skills could not be read."));
      else {
        const groups = new Map();
        for (const k of skillsIn(paths)) groups.set(k.category, [...(groups.get(k.category) ?? []), k]);
        s.dataset.fragment = mine.name;
        s.append(...[...groups].map(([category, list]) => {
          const row = el("p", "settings-line");
          row.dataset.category = category || "general";
          row.append(el("span", "settings-key", (category || "general").replace(/-/g, " ")), skillNames(list));
          return row;
        }));
        const open = el("button", "quiet", "Open your skills");
        open.type = "button";
        open.onclick = () => openApp(mine.name);
        s.append(actionLine("Managed skills", open));
      }
    }
    // each agent's own skills, from its fragment
    const agents = allFragments().filter((f) => f.kind === "agent" && own(f));
    const owns = await Promise.all(agents.map((a) => filesOf(a.name)));
    agents.forEach((a, i) => {
      const row = el("p", "settings-line");
      row.dataset.agent = a.name;
      const list = skillsIn(owns[i] ?? []);
      row.append(named(el("span", "settings-key"), a.name, (t) => `${t}'s own`), list.length ? skillNames(list) : el("span", "settings-value muted", owns[i] ? "None yet" : "Not read"));
      s.append(row);
    });
    return s;
  }
  return { openSettings, reopenSettings, skillsFragment, visit: () => visits };
}
