import { billingSections, returnSaid, returning } from "./billing.js";
import { dataSection } from "./exports.js";
import { icon } from "./icons.js";

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

export function createSettings({ api, $, seg, account, navigation, notices, sections, agents, releaseInfo, allFragments, own, freeLabel, load, named, openApp, openFiles, markOffer }) {
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
    // who is signed in: their picture, changed from the picture itself (a
    // pencil over it on hover or focus, as an agent's: agent-settings.js),
    // their latest sign-in's email beside it, and signing out, and another
    // account: a seat may wait under another email
    const signout = el("form");
    signout.method = "post";
    signout.action = "/auth/logout";
    const out = el("button", "quiet", "Sign out");
    out.type = "submit";
    signout.append(out);
    const ways = el("span", "settings-actions");
    ways.append(signout, switchForm(SETTINGS));
    // a picture: PNG, JPEG, WebP or GIF, at most 256 KiB (PUT
    // /api/identities/me/picture); none, their email's first letter. The
    // file input stays in reach of the keyboard, unseen, its words the
    // label's name and its tooltip's.
    const words = person().picture ? "Change your picture" : "Add a picture";
    const picture = el("label", "picture profile-picture");
    picture.dataset.tooltip = words;
    const face = el("span", "profile-avatar", (person().email ?? "").trim().charAt(0).toUpperCase() || "?");
    if (person().picture) {
      const img = el("img");
      img.alt = "";
      img.src = person().picture;
      img.onerror = () => img.remove();
      face.append(img);
    }
    const file = el("input");
    file.type = "file";
    file.accept = "image/png,image/jpeg,image/webp,image/gif";
    const over = el("span", "profile-picture-over");
    over.append(icon("rename"));
    picture.append(face, over, el("span", "profile-picture-words", words), file);
    // what became of a picture chosen: saving it, or why it was not taken
    const said = el("p", "profile-said");
    said.setAttribute("role", "status");
    said.hidden = true;
    const email = el("span", "settings-value");
    email.append(person().email ? el("strong", null, person().email) : "—");
    const text = el("div", "profile-text");
    text.append(el("span", "settings-key", "Signed in as"), email, said);
    const info = el("div", "settings-info profile-info");
    info.append(picture, text);
    const control = el("div", "settings-control");
    control.append(ways);
    const who = el("div", "settings-line settings-line-with-action settings-who settings-stack profile");
    who.append(info, control);
    file.onchange = async () => {
      const f = file.files?.[0];
      if (!f) return;
      file.disabled = true;
      said.classList.remove("form-error");
      said.textContent = "Saving your picture…";
      said.hidden = false;
      try {
        await api("PUT", "/api/identities/me/picture", f, f.type);
      } catch (e) {
        said.classList.add("form-error");
        said.textContent = e.message || "That picture was not taken";
        file.disabled = false;
        file.value = "";
        return;
      }
      setPerson(await api("GET", "/api/identities/me"));
      reopenSettings();
    };
    const account = section("Account", who);
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
      // its profile, where its runtime offers one (agent-runtimes.js)
      const row = markOffer(el("button", "row", "Open"), a.fragment, "profile");
      row.type = "button";
      row.onclick = () => openApp(a.fragment);
      // its own skills: its fragment's skills/, in its files, where its
      // runtime reads them (Paul, 2026-11-04: no copy, no move, and none
      // of this in the Edit dialog)
      const skills = markOffer(el("button", "row", "Skills"), a.fragment, "skills");
      skills.type = "button";
      skills.dataset.skillsFor = a.fragment;
      skills.onclick = () => openFiles(a.fragment, "skills/");
      const name = el("span", "settings-agent-name");
      name.dataset.agentFace = a.fragment;
      const [actions, said, renameIt, pictures, picture] = faceControls(a);
      // its picture's pencil over its picture, and its pencil and Remove
      // picture beside its name, all outside the name: renderAgentFaces
      // redraws it (its picture and its title)
      const face = el("span", "agent-face-edit");
      face.append(name, picture);
      const title = el("span", "settings-agent-title");
      title.append(face, renameIt, pictures);
      return [agentSettings(a, actionLine(title, actions, skills, row), modeled), said];
    }) : [el("p", "muted", "None yet.")]), ...(mine.length ? addAgentForm() : []));
    const connections = connectionsSection(linked, uses);
    const skills = skillsSection();
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
  // manual), copied into Claude Code or Codex with what to build: one line,
  // quiet, its Copy a copy icon at its end (a check once copied; its words,
  // Copy and Copied, its label and its tooltip's). The install line stays,
  // folded, for whoever runs it themselves.
  function buildSection() {
    const prompt = el("input", "build-prompt");
    prompt.id = "build-prompt";
    prompt.type = "text";
    prompt.spellcheck = false;
    prompt.setAttribute("aria-label", "A prompt for your coding agent");
    prompt.value = `Read ${location.origin}/llms.txt and help me build: `;
    const copy = el("button", "icon-button build-copy");
    copy.type = "button";
    copy.dataset.action = "copy-prompt";
    const word = el("span", "build-copy-word");
    const shows = (copied) => {
      word.textContent = copied ? "Copied" : "Copy";
      copy.setAttribute("aria-label", copied ? "Copied" : "Copy the prompt");
      copy.replaceChildren(icon(copied ? "check" : "copy"), word);
    };
    shows(false);
    const field = el("div", "build-prompt-field");
    field.append(prompt, copy);
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
        shows(false);
        said.textContent = "Couldn't copy it. The prompt is selected: copy it yourself (Ctrl+C, or ⌘C on a Mac).";
        said.hidden = false;
        prompt.focus();
        prompt.select();
        return;
      }
      if (mine !== presses) return;
      shows(true);
      setTimeout(() => {
        if (mine === presses) shows(false);
      }, 1500);
    };
    // a command to run, its Copy a copy icon in its corner (a check once
    // copied); a clipboard that refuses selects it, to copy by hand
    const command = (text) => {
      const pre = el("pre", "command", text);
      const button = el("button", "icon-button command-copy");
      button.type = "button";
      const showing = (copied) => {
        button.setAttribute("aria-label", copied ? "Copied" : "Copy the command");
        button.replaceChildren(icon(copied ? "check" : "copy"));
      };
      showing(false);
      let pressed = 0;
      button.onclick = async () => {
        const mine = ++pressed;
        try {
          await navigator.clipboard.writeText(text);
        } catch {
          const range = document.createRange();
          range.selectNodeContents(pre);
          getSelection()?.removeAllRanges();
          getSelection()?.addRange(range);
          return;
        }
        showing(true);
        setTimeout(() => mine === pressed && showing(false), 1500);
      };
      const block = el("div", "command-block");
      block.append(pre, button);
      return block;
    };
    // its fold: a chevron that turns as it opens
    const fold = el("summary");
    fold.append(icon("caret"), "Install it yourself");
    const yourself = el("details", "settings-install");
    yourself.append(
      fold,
      say("On macOS or Linux (on Windows, in WSL), then pair it with you:"),
      command(`${INSTALL}\nfragment login --host ${location.origin}`),
      say("To give Claude Code its skill:"),
      command(SKILL),
    );
    const about = say("Paste this into your coding agent and finish the sentence: it sets up the `fragment` command line itself, and you approve it here once.");
    about.classList.add("muted");
    return section("Build with Claude Code or Codex", about, field, said, yourself);
  }

  // ---- skills (decision 17): the managed set, the person's skills fragment's
  // (the platform's release, which their computers install for every agent),
  // opened from one row (Paul, 2026-11-04). Each agent's own are its
  // fragment's skills/, opened from its item in Agents. ----
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
  // Skills: one row as Computer's are (its label, a short line under it,
  // Open with an icon). Open is the person's skills fragment, made first
  // from the blessed template when they have none. The line says nothing
  // of which agents get them: only our Hermes image installs the set
  // (docs/computers.md, "Skills and the CLI")
  function skillsSection() {
    const s = section("Skills");
    s.id = "settings-skills";
    const mine = skillsFragmentOf();
    if (mine) s.dataset.fragment = mine.name;
    const words = el("span", null, "Open");
    const open = el("button", "quiet with-icon");
    open.type = "button";
    open.dataset.action = "open-skills";
    open.append(icon("folder"), words);
    open.onclick = async () => {
      open.disabled = true;
      try {
        openApp(await skillsFragment());
      } catch (e) {
        words.textContent = e.message;
      } finally {
        open.disabled = false;
      }
    };
    const row = el("div", "settings-line settings-line-with-action");
    const info = el("div", "settings-info");
    info.append(el("span", "settings-key", "Skills"), el("span", "settings-value settings-sub", "What your agents know how to do, kept in one place."));
    const control = el("div", "settings-control");
    control.append(open);
    row.append(info, control);
    s.append(row);
    return s;
  }
  return { openSettings, reopenSettings, skillsFragment, visit: () => visits };
}
