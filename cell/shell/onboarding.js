import { seatOffer } from "./billing.js";
import { DEFAULT_AGENT_TIER } from "./agent-settings.js";
import { PHASE } from "./computer.js";

export function createOnboarding({ api, ui, account, fragments, computer, navigation, reads, load, skillsFragment, rosterChanged }) {
  const { $, el, notice } = ui;
  const { person, isGuest, setGuest, hasInvites } = account;
  const { allFragments, chats, own, labelOf, titleOf, primaryAgent, agentsOwnChat, freeLabel, slug, seg } = fragments;
  const { computerView, computerFailed, agents, computerOf, computerIs } = computer;
  const { start, openChat, atHome, hasFrame, welcomePath: WELCOME } = navigation;
  // A new agent's name, when its maker names none (Paul, 2026-10-03): the
  // beige boxes of an alternate 1990s, never a real one's.
  const NAMES = ["XBT-2000", "Starfire 40K", "Turbo Quasar 486", "Novatron DX", "Hyperion 9000", "Cobalt Prism 66", "Megastation LX", "Orbitron 3D", "Datastar Pro", "Pulsar 360", "Zephyr XL", "Titan MX", "Vortex 7", "Nimbus 4K", "Galaxion SE", "Powerframe 99"];

  function pickName() {
    const taken = new Set(allFragments().map((f) => titleOf(f.name)));
    const free = NAMES.filter((n) => !taken.has(n));
    return free.length ? free[Math.floor(Math.random() * free.length)] : `Unit ${allFragments().length + 1}`;
  }
  // The agent speaks first: a routine on its own `tasks`, which its computer
  // takes as a turn in its chat asked by its owner (docs/chat-records.md,
  // `tasks`), so its chat opens with its hello and no message of the
  // person's. Posted once it is in the chat, where its turn posts; its id
  // makes a retry the same record.
  const hello = (agent, chat) =>
    api("POST", `/api/f/${agent}/channels/tasks`, {
      id: "hello",
      body: { kind: "routine", chat, text: "You were just made, and this is your chat with the person you work for. Say hello in a sentence or two: who you are, and what you will do for them (your SOUL.md). Start no work until they answer." },
    });

  // ---- first run: sign in, a seat, then the default agent is made
  // (decision 10); it never goes through settings ----
  // Match the chat composer's rim reflection, including its 64px approach
  // and rounded corners, on every first-run card (Skyler's onboarding design,
  // 2026-10-22). Listen once: firstRun replaces only the card's children.
  const splashPointer = matchMedia("(hover: hover) and (pointer: fine)");
  let splashEdgeFrame = 0;
  let splashEdgePointer = null;
  function clearSplashEdge() {
    cancelAnimationFrame(splashEdgeFrame);
    splashEdgeFrame = 0;
    $("first-run-card").style.setProperty("--edge-light", "0");
  }
  $("first-run").addEventListener("pointermove", (event) => {
    if (!splashPointer.matches || event.pointerType === "touch") return;
    splashEdgePointer = { x: event.clientX, y: event.clientY };
    if (splashEdgeFrame) return;
    splashEdgeFrame = requestAnimationFrame(() => {
      splashEdgeFrame = 0;
      const card = $("first-run-card");
      const rect = card.getBoundingClientRect();
      const x = splashEdgePointer.x - rect.left;
      const y = splashEdgePointer.y - rect.top;
      const radius = parseFloat(getComputedStyle(card).borderRadius);
      const qx = Math.abs(x - rect.width / 2) - rect.width / 2 + radius;
      const qy = Math.abs(y - rect.height / 2) - rect.height / 2 + radius;
      const distance = Math.max(0, Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius);
      const proximity = Math.max(0, 1 - distance / 64);
      card.style.setProperty("--edge-x", `${x}px`);
      card.style.setProperty("--edge-y", `${y}px`);
      card.style.setProperty("--edge-light", String(proximity * proximity));
    });
  });
  $("first-run").addEventListener("pointerleave", clearSplashEdge);
  $("first-run").addEventListener("scroll", clearSplashEdge, { passive: true });
  addEventListener("blur", clearSplashEdge);
  addEventListener("resize", clearSplashEdge);
  // The two ways the shell is first drawn (its first run's card, here, and
  // its layout, `showLayout`) take away "Still loading…" (index.html), so it
  // is never said beside the shell, and is said while nothing is drawn: its
  // scripts not come, or their first answers.
  function firstRun(...children) {
    $("still-loading")?.remove();
    clearSplashEdge();
    stopCardStep();
    $("layout").hidden = true;
    $("first-run").hidden = false;
    $("first-run-year").textContent = String(new Date().getFullYear());
    $("first-run-card").replaceChildren(...children);
  }
  // One step of the first run's card to another, in place (the welcome's
  // seat and its trial code: billing.js seatOffer): the card's height moves
  // from the one to the other and the new step's parts rise in, as Skyler's
  // design moves them; at once with reduced motion. Focus that was in the
  // card goes to the new step's heading.
  let cardMotion = { run: 0, animations: [] };
  function stopCardStep() {
    cardMotion.run++;
    for (const a of cardMotion.animations) a.cancel();
    cardMotion.animations = [];
    $("first-run-card").classList.remove("moving");
  }
  async function cardStep(from, to) {
    const card = $("first-run-card");
    const before = card.getBoundingClientRect().height;
    const focused = card.contains(document.activeElement);
    stopCardStep();
    const run = cardMotion.run;
    from.hidden = true;
    to.hidden = false;
    const heading = to.querySelector("h1");
    if (focused && heading) {
      heading.tabIndex = -1;
      heading.focus({ preventScroll: true });
    }
    if (!before || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const after = card.getBoundingClientRect().height;
    const ease = "cubic-bezier(.22, 1, .36, 1)";
    card.classList.add("moving");
    cardMotion.animations = [
      card.animate([{ height: `${before}px` }, { height: `${after}px` }], { duration: 520, easing: ease }),
      ...[...to.children].map((part, i) => part.animate([{ opacity: 0, transform: "translateY(9px)" }, { opacity: 1, transform: "none" }], { duration: 380, delay: 65 + Math.min(i, 8) * 45, easing: ease, fill: "backwards" })),
    ];
    await Promise.all(cardMotion.animations.map((a) => a.finished.catch(() => {})));
    if (run === cardMotion.run) stopCardStep();
  }
  function showLayout() {
    $("still-loading")?.remove();
    $("first-run").hidden = true;
    $("layout").hidden = false;
  }
  const PITCH = "Welcome to Finite";
  const SUBTITLE = "Superintelligent agents for you and your organization";
  function signIn() {
    // back to this view, with what its address carries (a mailed trial's
    // code), whichever screen of AuthKit's each opens on. Sign In is a
    // sign-in, not a switch: one WorkOS answers from a session it holds in
    // the browser comes back here too, as that account.
    const back = location.pathname + location.search;
    const go = el("a", "primary", "Sign Up");
    go.href = `/auth/login?screen_hint=sign-up&return=${encodeURIComponent(back)}`;
    const other = el("a", "secondary splash-secondary", "Sign In");
    other.href = `/auth/login?screen_hint=sign-in&return=${encodeURIComponent(back)}`;
    firstRun(el("h1", null, PITCH), el("p", null, SUBTITLE), go, other);
  }
  // "Use another account" (`POST /auth/switch`, docs/api.md, Sign-in), back
  // to `back`: WorkOS keeps its own sign-in in the browser (clearing this
  // site's data leaves it), and Sign in comes back as that account without
  // asking. A switch ends this session here, if there is one, and that one
  // there (whose logout comes back to the platform's root, not `back`), so
  // the next sign-in asks who is signing in. A form: the shell's
  // `form-action 'self'` lets the browser leave for WorkOS only from the
  // page its answer is.
  function switchForm(back, cls = "quiet") {
    const other = el("form");
    other.method = "post";
    other.action = `/auth/switch?return=${encodeURIComponent(back)}`;
    const use = el("button", cls, "Use another account");
    use.type = "submit";
    other.append(use);
    return other;
  }
  // Who is signed in, wherever someone is told they have no seat (a guest:
  // `GET /api/seat`): a browser's autofill can sign someone in as an email
  // their seat is not under, a new guest, whose seat waits under the other
  // (2026-10-20, in production). The email is their latest sign-in's.
  function noSeatSaid() {
    const p = el("p", "no-seat");
    if (person().email) p.append("You're signed in as ", el("strong", null, person().email), ". ");
    p.append("This account has no seat. If your team gave you a seat under another email, use another account.");
    return p;
  }
  // A new person's welcome (docs/billing.md, "The shell"): what this is, the
  // one seat and a trial code, then Stripe's Checkout, whose return comes
  // back here (`?checkout=<session>`, or `?checkout=canceled`), as a mailed
  // trial's link does (`?trial=<code>`). Someone who is no guest (a seat
  // bought, an org's claimed at sign-in, or the deployment's plan) goes home,
  // or on to the first agent; a guest is offered the seat, whatever they
  // own (one whose seat ended keeps their chats).
  async function welcome() {
    const q = new URLSearchParams(location.search);
    const back = q.get("checkout");
    const paid = back && back !== "canceled" ? back : null;
    const goHome = () => {
      history.replaceState(null, "", "/");
      return start();
    };
    if (paid && chats().some(own)) {
      // a seat paid for again: applied, then home
      await api("POST", `/api/billing/sessions/${encodeURIComponent(paid)}`, {}).catch(() => {});
      return goHome();
    }
    if (paid) return creatingAgent({ session: paid });
    const mine = await api("GET", "/api/seat").catch(() => null);
    if (mine) setGuest(mine.guest);
    if (!isGuest()) return chats().some(own) ? goHome() : creatingAgent(mine?.seat ? {} : null);
    history.replaceState(null, "", WELCOME);
    // LATER (fbfc's import; not built): "Import your finite.computer agent",
    // a choice beside the seats for someone moving their agent here. Its
    // place is kept, hidden, until the import is built.
    const imports = el("button", "plan", "Import your finite.computer agent");
    imports.type = "button";
    imports.dataset.slot = "import";
    imports.hidden = true;
    // who is signed in, and another account (back here, its trial code kept),
    // under whichever step the card shows
    const trial = q.get("trial");
    const foot = el("div", "welcome-foot");
    foot.append(noSeatSaid(), switchForm(trial ? `${WELCOME}?trial=${encodeURIComponent(trial)}` : WELCOME, "text"));
    // someone shared something with them first, or invited them: it is still
    // theirs to open, or to answer
    if (allFragments().length || hasInvites()) {
      const home = el("a", "text", "Not now");
      home.href = "/";
      foot.append(home);
    }
    const out = el("form");
    out.method = "post";
    out.action = "/auth/logout";
    const leave = el("button", "text", "Sign out");
    leave.type = "submit";
    out.append(leave);
    foot.append(out);
    const canceled = el("div", "ticket");
    canceled.append(el("p", null, "Checkout was canceled: nothing was bought."));
    const top = [el("h1", null, PITCH), el("p", null, SUBTITLE), ...(back === "canceled" ? [canceled] : [])];
    firstRun(...seatOffer({ api, el, step: cardStep }, mine, trial, top), imports, foot);
  }
  // A seat before the first agent. Checkout's return names its session,
  // applied at once (whatever Stripe's webhook's timing; again, it changes
  // nothing) and kept in the address until its seat holds, so a reload
  // applies it again. Then the seat's plan reaches the ledger from the
  // registry, a moment after, and the first run's creates are a seat's.
  // Bounded: SEAT_WAIT_MS.
  const SEAT_WAIT_MS = 30_000;
  async function seated(session) {
    if (session) {
      // a refusal said plainly, with what to do (Try again retries it; the
      // platform's words go to the console, for us)
      const mine = await api("POST", `/api/billing/sessions/${encodeURIComponent(session)}`, {}).catch((e) => {
        console.warn("A seat's Checkout return was not applied:", e);
        throw new Error(
          e.status === 404
            ? "This payment wasn't found. Choose a seat to start again."
            : "We couldn't confirm your payment yet. If you were charged, try again in a moment.",
        );
      });
      if (!mine.seat?.good) throw new Error("Your payment has not reached us yet: try again in a moment.");
      history.replaceState(null, "", "/");
    }
    const t0 = Date.now();
    for (;;) {
      const ledger = await api("GET", "/api/ledger").catch(() => null);
      if (ledger && ledger.plan !== "guest") return;
      if (Date.now() - t0 > SEAT_WAIT_MS) throw new Error("Your seat has not reached your account yet: try again in a moment.");
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  // The first agent (Paul, 2026-10-03: no question asked): the person's
  // default agent, in charge, made with its computer and its chat. The chat
  // is made first and opens at once (Skyler, 2026-10-12), so they may write
  // while the rest is made; its page sends only once the agent is a member
  // (a message from before its joining starts nothing: docs/chat-records.md).
  // What the chat shows of the making is the platform's word alone (Paul,
  // 2026-10-13: nothing fake or optimistic): a step is done once its call
  // answered, the computer's phase is its view's, and nothing is said in the
  // agent's name. Nothing of it is kept in this browser: a reload reads the
  // list again (`owesFirstAgent`, `resumeAdd`) and goes on from what was made.
  const SETUP_WAIT_MS = 4 * 60_000;
  // the first run or an Add while it runs, or after it failed (Try again, in
  // its chat): { label, title, role, job, added, agent, written, chat, made,
  // joined, ready, error }; null otherwise
  let setup = null;
  let setupRun = null;
  // What the first run's chat shows of it (the roster's `setup`, to that
  // chat's frame alone): each step done, being done, not yet, or failed,
  // and what failed.
  function setupFor(name) {
    if (!setup || setup.chat !== name) return null;
    // its name as the list says it once it is made (renamed meanwhile, the new one)
    const made = allFragments().find((f) => f.kind === "agent" && own(f) && labelOf(f.name) === setup.label);
    const t = made ? titleOf(made.name) : setup.title;
    const step = (text, done, now) => ({ text, state: done ? "done" : !now ? "todo" : setup.error ? "failed" : "doing" });
    return {
      steps: [
        step(setup.made ? `${t} is made` : `Making ${t}`, setup.made, !setup.made),
        step(setup.joined ? `${t} is in this chat` : `Adding ${t} to this chat`, setup.joined, setup.made && !setup.joined),
        step(setup.ready ? `${t} is ready` : computerView()?.phase === "awake" ? `${t} is starting on its computer` : `Its computer: ${PHASE[computerView()?.phase] ?? "not made yet"}`, setup.ready, setup.joined && !setup.ready),
      ],
      error: setup.error,
    };
  }
  function setupChanged(change) {
    Object.assign(setup, change);
    rosterChanged();
  }
  // Its job (its SOUL.md, written once: an agent already set up keeps its
  // own), never its name: that is its agent fragment's title alone, which a
  // rename changes (Paul, 2026-10-16: renamed, its job still said the old one).
  const firstSoul = (owner) => `You are ${owner}'s default agent: the first one they talk to, and in charge of the rest. Help with whatever they ask. When a job would be better as an app, or as an agent of its own, say so and offer to set it up. The first time you talk, say hello briefly and ask what they'd like to start with.\n`;
  // Its chat's own title (Paul, 2026-10-16), never the agent's name, which the
  // sidebar shows above it.
  const FIRST_CHAT = "Your first chat";
  // The name of NAMES an earlier try's label was made from (`freeLabel(slug(name))`,
  // maybe with its `-<n>`), so a retry names its agent as that try would have.
  const nameOfLabel = (label) => NAMES.find((n) => new RegExp(`^${slug(n)}(-[0-9]+)?$`).test(label));
  // a chat an earlier try made before its agent: theirs, `<label>-chat`, no agent in it
  const unjoined = (f) => own(f) && !f.agents?.length && /-chat$/.test(labelOf(f.name));
  // Whether the first run is owed, from the platform alone: a seat's person
  // with no agent of theirs in a chat of their own. No chats at all; or only
  // the chat an earlier try made; or their agent not on their computer, or
  // not a member of its own chat (asked of the chat: a row may not name its
  // agents yet). With chats, and their computer's read failed, their agents
  // are unknown, not none: the question is put off until a read answers
  // (`resumeFirstRun`).
  async function owesFirstAgent() {
    if (isGuest()) return false;
    if (reads().listFailed || reads().updating) {
      firstRunPutOff = true;
      return false;
    }
    if (!chats().length) return true;
    if (computerFailed()) {
      firstRunPutOff = true;
      return false;
    }
    if (chats().some((f) => own(f) && (f.agents ?? []).some((id) => agents().has(id)))) return false;
    const agent = primaryAgent();
    if (!agent) return chats().every(unjoined);
    const { identity: id } = agent;
    const itsChat = agentsOwnChat(agent.fragment);
    if (!id || !itsChat) return true;
    const members = await api("GET", `/api/f/${seg(itsChat.name)}/members`).catch(() => null);
    return !!members && !(members.members ?? []).some((m) => m.principal === id);
  }
  // The first run's question, put off while their computer could not be read
  // (`owesFirstAgent`), is asked again by each read that answers (`load`:
  // Try again's, or the list's socket's) and each arrival home (`openChat`,
  // the one way there), while the person is at home, and after it too (they
  // may have gone to Settings meanwhile: then their way back asks it). Owed,
  // it is made once (`creatingAgent`); not owed, an Add a reload cut short
  // goes on (`resumeAdd`), as the shell's start asks too.
  let firstRunPutOff = false;
  async function resumeFirstRun() {
    if (!atHome()) return;
    firstRunPutOff = false;
    const owed = await owesFirstAgent();
    if (!atHome()) firstRunPutOff = true;
    else if (owed) creatingAgent();
    else await resumeAdd();
  }
  // A making a reload cut short once the first agent is made (Settings'
  // Add agent): a chat of theirs, `<label>-chat`, with no message yet (its
  // page sends nothing before its agent is in it), and their agent
  // `<label>`. Not yet in it, it is made on from there, named as it was:
  // Add made its agent, its job and its role before its chat (`makeAgent`).
  // In it, only its hello may be missing: it is posted again (the same
  // record), once a page. Put off, as the first run is, while a read failed.
  const helloAgain = new Set();
  async function resumeAdd() {
    if (setup || isGuest()) return;
    if (reads().listFailed || reads().updating || computerFailed()) {
      firstRunPutOff = true;
      return;
    }
    for (const chat of chats().filter((f) => own(f) && /-chat$/.test(labelOf(f.name)) && !f.previewAt && !f.listed?.length)) {
      const label = labelOf(chat.name).slice(0, -"-chat".length);
      const agent = allFragments().find((f) => f.kind === "agent" && own(f) && labelOf(f.name) === label);
      if (!agent) continue;
      const id = [...agents().values()].find((a) => a.fragment === agent.name)?.identity;
      // made, in another chat of this label (this one a create's whose answer was lost)
      if (id && chats().some((c) => c !== chat && own(c) && labelOf(c.name) === labelOf(chat.name) && (c.agents ?? []).includes(id))) continue;
      let joined = !!id && (chat.agents ?? []).includes(id);
      if (id && !joined) {
        // asked of the chat: a row may not name its agents yet
        const members = await api("GET", `/api/f/${seg(chat.name)}/members`).catch(() => null);
        if (!members) continue;
        joined = (members.members ?? []).some((m) => m.principal === id);
      }
      if (joined) {
        if (!helloAgain.has(chat.name)) {
          helloAgain.add(chat.name);
          hello(agent.name, chat.name).catch(() => helloAgain.delete(chat.name));
        }
        continue;
      }
      if (setup) return;
      setup = { label, title: titleOf(agent.name), role: null, job: addedJob, added: true, agent: agent.name, written: false, chat: null, made: false, joined: false, ready: false, error: null };
      return creatingAgent();
    }
  }
  // Another agent's job (Settings' Add agent): what it does is its role
  // (its `meta.role`, which its runtime tells it), never copied here.
  const addedJob = (owner) => `You are one of ${owner}'s agents. Help with whatever they ask, in your role. The first time you talk, say hello briefly and ask what they'd like to start with.\n`;
  // Another agent, named and given a role by its person (Settings' Agents,
  // Add; Paul, 2026-10-30), made as the first is but for its order: the
  // agent with its job and role, then its own chat, open at once, the
  // making's steps in it, then its hello (`resumeAdd` goes on after a
  // reload). One making at a time: one under way, or failed and waiting for
  // Try again in its chat, is the one to finish first. An agent of theirs
  // of this name on no computer and with no chat of its own (an Add cut
  // short before its chat, or one made elsewhere) is the one made on.
  function addAgent({ name, role, runtime = "hermes" }) {
    if (setup) return Promise.reject(new Error("Another agent is being made: finish it in its chat first."));
    const placed = new Set([...agents().values()].map((a) => a.fragment));
    const left = allFragments().find((f) => f.kind === "agent" && own(f) && titleOf(f.name) === name && !placed.has(f.name) && !agentsOwnChat(f.name));
    setup = { label: left ? labelOf(left.name) : freeLabel(slug(name)), title: name, role: role || null, runtime, job: addedJob, added: true, agent: left?.name ?? null, written: false, chat: null, made: false, joined: false, ready: false, error: null };
    return creatingAgent();
  }
  // The first run's agent, named as an earlier try named it, if one did.
  async function defaultAgent() {
    await load();
    if (!setup) {
      // an earlier try's names, by what it made: its agent, else its chat's
      // label (the chat's title is its own)
      const agent = primaryAgent();
      const chat = agent ? null : chats().find(unjoined);
      const earlier = agent ? labelOf(agent.fragment) : chat ? labelOf(chat.name).slice(0, -"-chat".length) : null;
      const title = agent ? titleOf(agent.fragment) : (earlier && nameOfLabel(earlier)) || pickName();
      setup = { label: earlier ?? freeLabel(slug(title)), title, role: null, job: firstSoul, added: false, agent: null, written: false, chat: null, made: false, joined: false, ready: false, error: null };
    }
    return makeAgent();
  }
  // An agent and its own chat, as `setup` names them (the first run's, and
  // Settings' Add agent's): each step reuses what an earlier try made,
  // including a create whose response was lost. The chat's frame opens
  // once: a draft in it stays put.
  async function makeAgent() {
    setupChanged({ error: null });
    const { label } = setup;
    if (setup.added) await agentWritten();
    const chatName = allFragments().find((f) => f.kind === "chat" && own(f) && labelOf(f.name) === `${label}-chat`)?.name ??
      (await api("POST", "/api/fragments", { label: `${label}-chat`, template: "chat", title: FIRST_CHAT })).name;
    setupChanged({ chat: chatName });
    await load();
    if (!hasFrame(chatName)) openChat(chatName);

    // These can run together once the chat is on screen. The computer is
    // assigned before it wakes, with the agent's configuration deployed.
    const prepared = await Promise.allSettled([skillsFragment(), computerOf()]);
    // Finish both before offering Retry: no previous create is left in flight.
    for (const result of prepared) if (result.status === "rejected") throw result.reason;
    const computer = prepared[1].value;
    const agent = await agentWritten();
    const assigned = await api("PUT", `/api/computers/${seg(computer.computer)}/agents/${seg(agent)}`, {});
    const id = assigned.agents.find((a) => a.fragment === agent)?.identity;
    if (!id) throw new Error("its computer did not take it");
    await api("POST", `/api/f/${agent}/deploy`, {});
    setupChanged({ made: true });
    // its joining is what lets the chat's page send (and wakes its computer)
    await api("PUT", `/api/f/${chatName}/members/${seg(id)}`, { role: "editor" });
    setupChanged({ joined: true });
    await hello(agent, chatName);
    api("POST", `/api/computers/${seg(computer.computer)}/wake`, {}).then(computerIs).catch(() => {});
    await load();
    return { chat: chatName, id, computer: computer.computer };
  }
  // Its agent fragment's name: made unless a try made it (`setup.agent`, else
  // found by its label), with its job, tier and role written once (an agent
  // already set up keeps its own).
  async function agentWritten() {
    const { label, title, job } = setup;
    let agent = setup.agent ?? allFragments().find((f) => f.kind === "agent" && own(f) && labelOf(f.name) === label)?.name;
    if (!agent) agent = (await api("POST", "/api/fragments", { label, template: "agent", title })).name;
    setupChanged({ agent });
    if (setup.written) return agent;
    const written = await api("GET", `/api/f/${seg(agent)}/file?path=agent.json`).then(() => true, (e) => { if (e.status === 404) return false; throw e; });
    if (!written) {
      const owner = person().email || "your person";
      // what it runs on, as the bridge reads it (docs/bridge.md): Claude
      // Code named, Hermes (none named) with its tier
      const runs = setup.runtime === "claude" ? { runtime: "claude" } : { tier: DEFAULT_AGENT_TIER };
      const files = [{ path: "SOUL.md", text: job(owner) }, { path: "agent.json", text: JSON.stringify(runs, null, 2) + "\n" }];
      // its role, beside its name in its fragment.json (`meta.role`)
      if (setup.role) {
        const manifest = await api("GET", `/api/f/${seg(agent)}/manifest`);
        manifest.meta = { ...(manifest.meta ?? {}), role: setup.role };
        files.push({ path: "fragment.json", text: JSON.stringify(manifest, null, 2) + "\n" });
      }
      await api("POST", `/api/f/${agent}/files`, { key: "agent-default", message: "its job", files });
    }
    setupChanged({ written: true });
    return agent;
  }
  // Ready: its computer awake, and the agent following its chat (its wake
  // subscription, which its guest makes as it follows: the chat's owner sees
  // it). The computer as it answered is shown at once (its phase, in the chat).
  async function agentReady({ chat, id, computer }) {
    const [c, subs] = await Promise.all([
      api("GET", "/api/computers").then((v) => v.computers?.find((x) => x.computer === computer)),
      api("GET", `/api/f/${chat}/subscriptions`),
    ]);
    computerIs(c);
    // a wake refused (its owner's credit) is said, not waited out
    if (c?.why && c.phase !== "awake") throw Object.assign(new Error(c.why), { refused: true });
    return c?.phase === "awake" && (subs.subscriptions ?? []).some((x) => x.wake && x.principal === id && x.channel === "chat");
  }
  // `seat` (`{session?}`): a seat comes first (`seatFirst`), then the agent.
  function creatingAgent(seat = null) {
    if (seat) return seatFirst(seat);
    if (setupRun) return setupRun;
    history.replaceState(null, "", "/");
    showLayout();
    // until its chat is made, the middle column says what is being made
    if (!setup?.chat || !hasFrame(setup.chat)) notice("Making your agent…");
    setupRun = (async () => {
      const made = await defaultAgent();
      const t0 = Date.now();
      for (;;) {
        let ready = false;
        try {
          ready = await agentReady(made);
        } catch (e) {
          if (e.refused) throw e;
        }
        if (ready) break;
        if (Date.now() - t0 > SETUP_WAIT_MS) throw new Error("Its computer is taking longer than usual to start. What you send waits for it; Try again looks again.");
        await new Promise((r) => setTimeout(r, 1500));
      }
      setupChanged({ ready: true });
      setup = null;
      rosterChanged();
      await load();
    })().catch((err) => {
      if (setup) setupChanged({ error: err.message || "It did not answer." });
      if (!setup?.chat || !hasFrame(setup.chat)) {
        const retry = el("button", "primary", "Try again");
        retry.type = "button";
        retry.onclick = () => creatingAgent();
        notice("Your agent could not start", err.message, retry);
      }
    }).finally(() => { setupRun = null; });
    return setupRun;
  }
  // A seat before the first agent (the welcome's: a Checkout's return, or a
  // seat given whose plan is on its way): the welcome's card says it is
  // being set up until the platform says it holds (`seated`; Checkout's
  // return stays in the address until then, so a reload applies it again),
  // then the agent is made in its chat (`creatingAgent`). Refused, it says
  // why, with Try again and Choose a seat (the welcome's seats again).
  function seatFirst(seat) {
    const step = el("li", "doing", "Setting up your seat");
    const list = el("ol", "creating-steps ticket");
    list.append(step);
    const error = el("p", "form-error");
    error.hidden = true;
    const retry = el("button", "primary", "Try again");
    retry.type = "button";
    retry.hidden = true;
    retry.onclick = () => creatingAgent(seat);
    const choose = el("button", "text", "Choose a seat");
    choose.type = "button";
    choose.hidden = true;
    choose.onclick = () => {
      history.replaceState(null, "", WELCOME);
      welcome().catch((e) => notice("This page did not load", e.message));
    };
    firstRun(el("h1", null, "Setting up your seat…"), list, error, retry, choose);
    seated(seat.session).then(
      () => {
        step.className = "done";
        // a seat now, as the ledger said: the shell offers what a seat makes
        setGuest(false);
        return creatingAgent();
      },
      (err) => {
        step.className = "failed";
        error.textContent = err.message;
        error.hidden = false;
        retry.hidden = false;
        choose.hidden = false;
      },
    );
  }
  return { signIn, showLayout, welcome, switchForm, noSeatSaid, setupFor, owesFirstAgent, resumeFirstRun, creatingAgent, addAgent,
    setupChat: () => setup?.chat ?? null,
    setupState: () => setup && [setup.chat, setupFor(setup.chat)],
    setupPending: () => setup && !setup.joined,
    retrySetup: (name) => { if (name === setup?.chat && setup.error) creatingAgent(); },
    putOff: () => firstRunPutOff,
  };
}
