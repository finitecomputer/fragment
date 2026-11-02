// What each of the person's agents' runtime offers beyond its chats.
//
// Each runtime offers what it supports, and the shell shows only what an
// agent's runtime can do (AGENTS.md, "Rules and approvals"): Hermes, the
// agent's profile (its own page) and its screen (its computer's desktop);
// Claude Code, neither (Paul, 2026-11-02: "different harness, different
// capabilities"). An agent's runtime is its agent.json's `runtime` (none:
// Hermes), as Settings reads it (agent-settings.js `agentJson`), chosen at
// its making and never changed: read once a page. Until it is read, or when
// its read fails, an agent is offered what it was before; a runtime this
// shell does not know is offered nothing beyond its chats.

export const RUNTIME_OFFERS = { hermes: ["profile", "screen"], claude: [] };

export function createRuntimes({ agentJson, agents, own, byName }) {
  const runtimes = new Map();
  const reading = new Set();
  // Reads the runtime of each of the person's agents not read yet; each read
  // that lands shows or hides what it offers (`renderOffers`). Answers once
  // the reads it began have landed (none begun: at once).
  function loadRuntimes() {
    const reads = [];
    for (const a of agents().values()) {
      const agent = a.fragment;
      if (!agent || runtimes.has(agent) || reading.has(agent) || !own(byName(agent) ?? {})) continue;
      reading.add(agent);
      reads.push(agentJson(agent)
        .catch((e) => (e.status === 404 ? {} : null))
        .then((j) => {
          reading.delete(agent);
          if (!j) return;
          runtimes.set(agent, typeof j.runtime === "string" ? j.runtime : "hermes");
          renderOffers();
        }));
    }
    return Promise.all(reads);
  }
  // Whether `agent` offers `what` ("profile", "screen").
  function offers(agent, what) {
    const runtime = runtimes.get(agent);
    return runtime === undefined || (RUNTIME_OFFERS[runtime] ?? []).includes(what);
  }
  // A control of what `agent` offers, drawn now and shown or hidden as its
  // runtime is read (`renderOffers`).
  function markOffer(node, agent, what) {
    node.dataset.offer = what;
    node.dataset.offerFor = agent;
    node.hidden = !offers(agent, what);
    return node;
  }
  // Each drawn control as its agent's runtime says; a line of them alone
  // (`data-offers`: Settings' Screens) with none shown goes too.
  function renderOffers() {
    if (typeof document === "undefined") return;
    for (const node of document.querySelectorAll("[data-offer]")) node.hidden = !offers(node.dataset.offerFor, node.dataset.offer);
    for (const line of document.querySelectorAll("[data-offers]")) line.hidden = ![...line.querySelectorAll("[data-offer]")].some((n) => !n.hidden);
  }
  return { loadRuntimes, offers, markOffer, renderOffers };
}
