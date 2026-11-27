// The operators' admin page (docs/billing.md, "The operator's admin";
// decision 59): a glance at who joined and what runs, people, orgs (and an
// org's own acts: its invoiced mark, a comp into it, its credit, budget and
// payer, each through the route the CLI's `fragment operator` uses), trial
// codes, billing's health and the log
// of what operators did, through `/api/admin/*` with the operator's own
// platform session (`x-fragment-shell`, same-origin only). The page is
// served to an operator's session alone (anyone else gets the 404 of a
// missing path), and the API decides again: anyone else is answered 403
// and sees only that. Every value reaches the page as text, never as
// markup.

import { ago, troubleWhat } from "./admin-glance.js";

const $ = (id) => document.getElementById(id);
const status = $("status");

function say(text, bad = false) {
  status.textContent = text;
  status.className = bad ? "bad" : "";
}

async function api(method, path, body) {
  const headers = { "x-fragment-shell": "1" };
  if (body !== undefined) headers["content-type"] = "application/json";
  const r = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), credentials: "same-origin" });
  const v = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(v.message || `${method} ${path}: ${r.status}`);
  return v;
}

/** A table row of cells, each text; `open` runs when it is clicked. */
function row(cells, open) {
  const tr = document.createElement("tr");
  for (const c of cells) {
    const td = document.createElement("td");
    if (c instanceof Node) td.append(c);
    else td.textContent = c ?? "";
    tr.append(td);
  }
  if (open) tr.addEventListener("click", open);
  return tr;
}

function el(tag, text, cls) {
  const e = document.createElement(tag);
  if (text !== undefined) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}

function when(ms) {
  return ms ? new Date(ms).toISOString().slice(0, 16).replace("T", " ") : "";
}

// A seat's kind, shortly: the one sold, or a comp's seat that sleeps.
const KIND = { seat_always_on: "always-on", seat: "sleeping" };

// An always-on seat whose holder lets its computer sleep says so: why its
// computer is not always on.
function seatText(s) {
  if (!s) return "";
  return `${KIND[s.kind]} ${s.comped ? "comped" : "paid"}${s.kind === "seat_always_on" && s.sleeps ? ", lets it sleep" : ""}${s.good ? "" : " (lapsed)"}`;
}

function guard(fn) {
  return async (...args) => {
    try {
      await fn(...args);
    } catch (e) {
      say(e.message, true);
    }
  };
}

let me = null;

// ---- people
let peopleNext = null;
let peopleQ = "";

async function people(more = false) {
  const p = new URLSearchParams();
  if (peopleQ) p.set("q", peopleQ);
  if (more && peopleNext) p.set("after", peopleNext);
  const v = await api("GET", `/api/admin/people?${p}`);
  const rows = $("people-rows");
  if (!more) rows.replaceChildren();
  for (const person of v.people) {
    rows.append(row([person.email || person.npub, seatText(person.seat), person.org?.name, when(person.joinedAt), when(person.lastSignInAt)], guard(() => openPerson(person.npub))));
  }
  peopleNext = v.next;
  $("people-more").hidden = !v.next;
  say(`${rows.children.length} people`);
}

async function openPerson(npub) {
  const v = await api("GET", `/api/admin/people/${encodeURIComponent(npub)}`);
  const box = $("person");
  box.replaceChildren(el("h2", v.person.email || npub));
  const facts = el("dl");
  const fact = (k, val) => facts.append(el("dt", k), el("dd", val ?? ""));
  fact("npub", npub);
  fact("org", v.person.org ? `${v.person.org.name}${v.person.admin ? " (admin)" : ""}` : "none");
  fact("seat", seatText(v.person.seat) || "none");
  fact("plan", v.ledger.plan);
  fact("standing", v.ledger.standing?.why ? `${v.ledger.standing.standing}: ${v.ledger.standing.why}` : v.ledger.standing?.standing);
  fact("balance", `$${(v.ledger.balanceMicros / 1e6).toFixed(2)}`);
  fact("computer", v.computer ? `${v.computer.phase}${v.computer.alwaysOn ? ", always on" : ""}` : "none");
  box.append(facts);
  if (v.person.seat?.comped) {
    const end = el("button", "End this comp");
    end.addEventListener("click", guard(async () => {
      if (!confirm(`End the comped seat of ${v.person.email}? Their agents stop.`)) return;
      await api("DELETE", `/api/admin/seats/${encodeURIComponent(v.person.seat.id)}`);
      await openPerson(npub);
      await people();
      say("comp ended");
    }));
    box.append(end);
  }
  const grant = el("form");
  const usd = el("input");
  usd.type = "number";
  usd.min = "0.01";
  usd.step = "0.01";
  usd.placeholder = "dollars";
  usd.required = true;
  const why = el("input");
  why.placeholder = "why";
  why.required = true;
  grant.append(usd, why, el("button", "Grant credit"));
  grant.addEventListener("submit", guard(async (e) => {
    e.preventDefault();
    const id = `admin-${crypto.randomUUID()}`;
    await api("POST", `/api/ledger/${encodeURIComponent(npub)}/grant`, { id, micros: Math.round(Number(usd.value) * 1e6), by: me.id, why: why.value });
    say(`granted $${usd.value}`);
    await openPerson(npub);
  }));
  box.append(grant);
  box.hidden = false;
}

// ---- orgs
let orgsNext = null;

async function orgs(more = false) {
  const p = new URLSearchParams();
  if (more && orgsNext) p.set("after", orgsNext);
  const v = await api("GET", `/api/admin/orgs?${p}`);
  const rows = $("org-rows");
  if (!more) rows.replaceChildren();
  for (const o of v.orgs) {
    const st = o.invoiced
      ? el("span", "paid by invoice")
      : el("span", o.status || "never paid", o.status && !["trialing", "active", "past_due"].includes(o.status) ? "lapsed" : "");
    rows.append(row([o.name, st, String(o.paidSeats), String(o.compedSeats), String(o.pending), String(o.admins), o.customer || ""], guard(() => openOrg(o.id))));
  }
  orgsNext = v.next;
  $("orgs-more").hidden = !v.next;
  say(`${rows.children.length} orgs`);
}

const dollars = (micros) => `$${(micros / 1e6).toFixed(2)}`;
const fresh = () => `admin-${crypto.randomUUID()}`;

/** A form of `fields` and a button labeled `label`; `done` runs on its submit. */
function form(label, fields, done) {
  const f = el("form");
  f.append(...fields, el("button", label));
  f.addEventListener("submit", guard(async (e) => {
    e.preventDefault();
    await done();
  }));
  return f;
}

function input(placeholder, { type = "text", required = true, min, step } = {}) {
  const i = el("input");
  i.type = type;
  i.placeholder = placeholder;
  i.required = required;
  if (min !== undefined) i.min = min;
  if (step !== undefined) i.step = step;
  return i;
}

// An org, and what an operator does for it: its invoiced mark (paid to
// Finite directly, outside Stripe: its admins' seats are comps), a seat
// comped into it (someone in another org is offered it), and its own
// credit, budget and payer (docs/billing.md, "An org's own credit").
async function openOrg(id) {
  const org = encodeURIComponent(id);
  const [v, ledger] = await Promise.all([api("GET", `/api/admin/orgs/${org}`), api("GET", `/api/admin/orgs/${org}/ledger`).catch((e) => ({ unread: e.message }))]);
  const box = $("org");
  const table = el("table");
  const body = el("tbody");
  for (const m of v.members) {
    body.append(row([m.email, m.person ? "" : "waiting", m.admin ? "admin" : "", m.seat ? `${KIND[m.seat]} ${m.comped ? "comped" : "paid"}` : "", m.id]));
  }
  table.append(body);
  const seats = v.members.filter((m) => m.seat).length;
  const facts = el("dl");
  const fact = (k, val) => facts.append(el("dt", k), el("dd", val ?? ""));
  fact("id", v.id);
  fact("paid by invoice", v.invoiced ? (v.invoiced.seats != null ? `yes: ${seats} of ${v.invoiced.seats} prepaid seats taken` : `yes, no seat cap: ${seats} seats`) : "no");
  fact("credit", ledger.unread ? `unread: ${ledger.unread}` : `${dollars(ledger.balanceMicros)} left, ${dollars(ledger.spentMicros)} of its ${dollars(ledger.budgetMicros)} budget spent this month`);
  const again = async (said) => {
    await openOrg(id);
    await orgs();
    say(said);
  };
  // the mark: set, its prepaid seats changed, or cleared (its seats stay)
  const prepaid = input("prepaid seats (none: no cap)", { type: "number", required: false, min: "1", step: "1" });
  if (v.invoiced?.seats != null) prepaid.value = String(v.invoiced.seats);
  const mark = form(v.invoiced ? "Change prepaid seats" : "Mark paid by invoice", [prepaid], async () => {
    await api("PUT", `/api/admin/orgs/${org}/invoiced`, { id: fresh(), invoiced: true, seats: prepaid.value ? Number(prepaid.value) : null });
    await again(`${v.name} is paid by invoice${prepaid.value ? `, ${prepaid.value} prepaid seats` : ""}: its admins add seats, each a comp`);
  });
  if (v.invoiced) {
    const clear = el("button", "Clear the mark");
    clear.type = "button";
    clear.addEventListener("click", guard(async () => {
      if (!confirm(`Clear ${v.name}'s invoiced mark? Its seats stay; its admins can't add more.`)) return;
      await api("PUT", `/api/admin/orgs/${org}/invoiced`, { id: fresh(), invoiced: false });
      await again(`${v.name} is no longer paid by invoice: its seats stay; its admins can't add more`);
    }));
    mark.append(clear);
  }
  // a seat comped into this org: theirs at once, or offered (someone in another org moves by their own yes)
  const email = input("email", { type: "email" });
  const kind = el("select");
  kind.append(new Option("always-on seat", "seat_always_on"), new Option("seat that sleeps (comps only)", "seat"));
  const comp = form("Comp a seat here", [email, kind], async () => {
    const c = await api("POST", "/api/admin/seats", { email: email.value.trim(), kind: kind.value, org: id });
    const how = c.seat.person ? "theirs now" : "offered: theirs once they accept it";
    await again(`${c.created ? "comped" : "already comped"}: ${c.seat.email} in ${c.org.name}, ${how}${c.mailed ? ", mailed" : ""}`);
  });
  // its own credit, under the payment's own id (so a retry credits it once)
  const usd = input("dollars", { type: "number", min: "0.01", step: "0.01" });
  const why = input("why");
  const payment = input("payment's id (an invoice's number)");
  const credit = form("Credit the org", [usd, why, payment], async () => {
    await api("POST", `/api/ledger/org:${org}/grant`, { id: payment.value.trim(), micros: Math.round(Number(usd.value) * 1e6), by: me.id, why: why.value });
    await again(`credited ${v.name} $${usd.value}`);
  });
  const budget = input("monthly budget, dollars (0: none)", { type: "number", min: "0", step: "0.01" });
  const setBudget = form("Set its budget", [budget], async () => {
    await api("POST", `/api/ledger/org:${org}/budget`, { id: fresh(), micros: Math.round(Number(budget.value) * 1e6) });
    await again(`${v.name}'s budget is $${budget.value} a month`);
  });
  const person = input("email or npub");
  const payer = form("Name who its credit is invoiced to", [person], async () => {
    const p = await api("POST", `/api/admin/orgs/${org}/payer`, { id: fresh(), person: person.value.trim() });
    await again(`${v.name}'s credit is invoiced to ${p.email || p.person}`);
  });
  box.replaceChildren(el("h2", v.name), facts, table, mark, comp, credit, setBudget, payer);
  box.hidden = false;
}

// ---- trials
async function trials() {
  const v = await api("GET", "/api/admin/trials");
  const rows = $("trial-rows");
  rows.replaceChildren();
  for (const t of v.codes) {
    const actions = el("span");
    const raise = el("button", "Places…");
    raise.addEventListener("click", guard(async (e) => {
      e.stopPropagation();
      const taken = Math.max(1, t.subscribed + t.open);
      const n = Number(prompt(`Places for ${t.code} (now ${t.capacity}; at least ${taken}, those taken)`, String(t.capacity)));
      if (!n || n === t.capacity) return;
      await api("PATCH", `/api/admin/trials/${t.id}`, { revision: t.revision, capacity: n });
      await trials();
    }));
    const toggle = el("button", t.active ? "Turn off" : "Turn on");
    toggle.addEventListener("click", guard(async (e) => {
      e.stopPropagation();
      await api("PATCH", `/api/admin/trials/${t.id}`, { revision: t.revision, active: !t.active });
      await trials();
    }));
    const send = el("button", "Mail…");
    send.addEventListener("click", guard(async (e) => {
      e.stopPropagation();
      const to = prompt(`Mail ${t.code} to`);
      if (!to) return;
      await api("POST", `/api/admin/trials/${t.id}/send`, { email: to });
      say(`mailed ${to}`);
    }));
    actions.append(raise, toggle, send);
    rows.append(row([el("code", t.code), t.name, `${t.days} days, ${KIND[t.kind]}`, `${t.subscribed}/${t.capacity}`, String(t.open), t.active ? "on" : "off", actions]));
  }
  say(`${v.codes.length} codes`);
}

// ---- the glance: who joined, from the registry; what runs, from the
// deployment's logs (`GET /api/admin/glance`). Asked each minute while it
// is shown; the logs' numbers say when they were read (the admin keeps a
// read two minutes, and longer only when a newer one fails, saying why).
const GLANCE_EVERY_MS = 60_000;
let glanceTimer = null;

function tile(label, big, small) {
  const t = el("div", undefined, "tile");
  t.append(el("div", label, "label"), el("div", String(big), "big"), el("div", small, "small"));
  return t;
}

// a trouble's kind, and under it, small, why a save failed, when read
function troubleCell(t) {
  const { what, why } = troubleWhat(t, when);
  if (!why) return what;
  const cell = el("div", what);
  cell.append(el("div", why, "quiet why"));
  return cell;
}

// a person in the glance opens their page, as the People tab does
function openFromGlance(npub) {
  return guard(async () => {
    await show("people");
    await openPerson(npub);
  });
}

async function glance() {
  const midnight = new Date();
  midnight.setHours(0, 0, 0, 0);
  const [v, h] = await Promise.all([api("GET", `/api/admin/glance?dayStart=${midnight.getTime()}`), api("GET", "/api/admin/health")]);
  const logs = v.logs;
  const read = logs.state === "read";
  $("tiles").replaceChildren(
    tile("Signups today", v.signups.today, `${v.signups.hour} in the last hour · ${v.signups.week} this week · ${v.signups.total} in all`),
    tile("Computers awake now", read ? logs.computersAwakeNow : "–", read ? `${logs.computersAwakeToday} today · ${logs.computersAwakeWeek} this week` : "from the logs"),
    tile("Fragments awake now", read ? logs.fragmentsAwakeNow : "–", read ? `${logs.fragmentsAwakeToday} today` : "from the logs"),
    tile("Seats paid", h.seatsPaid, `${h.seatsComped} comped · ${h.orgsPaying} orgs paying`),
    tile("Signed in, last 24 hours", v.signedIn.day, `${v.signedIn.week} in the last 7 days`),
  );
  $("glance-logs").textContent =
    logs.state === "notConfigured"
      ? "Computers and fragments come from the deployment's logs, which aren't configured here (observability_read)."
      : logs.state === "failed"
        ? `The logs couldn't be read: ${logs.why}`
        : [logs.whyKept && `Newer numbers couldn't be read: ${logs.whyKept}`, logs.whyUnread && `Why saves failed couldn't be read: ${logs.whyUnread}`].filter(Boolean).join(" ");
  const trouble = read ? logs.trouble : [];
  $("trouble-head").hidden = $("trouble").hidden = trouble.length === 0;
  $("trouble-rows").replaceChildren(...trouble.map((t) => row([t.email || t.npub || "no one's", el("code", t.computer), troubleCell(t), String(t.lines)], t.npub ? openFromGlance(t.npub) : undefined)));
  const up = (p) => (p.computerUp == null ? "–" : p.computerUp ? "yes" : "not yet");
  $("new-rows").replaceChildren(...v.newPeople.map((p) => row([p.email || p.npub, when(p.joinedAt), seatText(p.seat) || "none", p.agentAt ? when(p.agentAt) : "none", up(p)], openFromGlance(p.npub))));
  $("glance-at").textContent = `Updated ${new Date(v.at).toLocaleTimeString()}${read ? ` · logs read ${ago(logs.at, v.at)}` : ""}; again each minute.`;
  say("");
}

// ---- health and the log
async function health() {
  const v = await api("GET", "/api/admin/health");
  const facts = $("health-facts");
  facts.replaceChildren();
  const fact = (k, val) => facts.append(el("dt", k), el("dd", String(val ?? "")));
  fact("orgs paying", v.orgsPaying);
  fact("orgs lapsed", v.orgsLapsed);
  fact("seats paid", v.seatsPaid);
  fact("seats comped", v.seatsComped);
  fact("plan pushes queued", v.planPushesQueued);
  fact("quantity pushes queued", v.quantityPushesQueued);
  fact("last Stripe event", v.lastEventAt ? when(v.lastEventAt * 1000) : "none");
  fact("oldest subscription copy", v.oldestCopyAt ? when(v.oldestCopyAt) : "none");
  const rows = $("failing-rows");
  rows.replaceChildren(...v.failing.map((f) => row([f.queue, f.target, String(f.tries), when(f.due)])));
  say(v.failing.length ? `${v.failing.length} pushes failing` : "nothing failing");
}

let logNext = null;

async function log(more = false) {
  const p = new URLSearchParams();
  if (more && logNext) p.set("before", String(logNext));
  const v = await api("GET", `/api/admin/log?${p}`);
  const rows = $("log-rows");
  if (!more) rows.replaceChildren();
  for (const e of v.entries) rows.append(row([String(e.n), when(e.at), e.operator, e.action, e.target, e.detail]));
  logNext = v.next;
  $("log-more").hidden = !v.next;
}

// ---- tabs
const load = { glance, people: () => people(), orgs: () => orgs(), trials, health, log: () => log() };

function show(tab) {
  for (const b of document.querySelectorAll("#tabs button")) b.classList.toggle("on", b.dataset.tab === tab);
  for (const s of document.querySelectorAll("main > section")) s.hidden = s.id !== tab;
  history.replaceState(null, "", `/admin#${tab}`);
  // the glance reads again each minute while it is shown, and the page is seen
  clearInterval(glanceTimer);
  glanceTimer = tab === "glance" ? setInterval(() => document.hidden || guard(glance)(), GLANCE_EVERY_MS) : null;
  return guard(load[tab])();
}

document.addEventListener("DOMContentLoaded", guard(async () => {
  me = await api("GET", "/api/identities/me");
  $("me").textContent = me.subjects?.[0]?.email || me.id;
  for (const b of document.querySelectorAll("#tabs button")) b.addEventListener("click", () => show(b.dataset.tab));
  $("search").addEventListener("submit", guard(async (e) => {
    e.preventDefault();
    peopleQ = e.target.q.value.trim();
    await people();
  }));
  $("comp").addEventListener("submit", guard(async (e) => {
    e.preventDefault();
    const v = await api("POST", "/api/admin/seats", { email: e.target.email.value.trim(), kind: e.target.kind.value });
    e.target.reset();
    await people();
    say(`${v.created ? "comped" : "already comped"}: ${v.seat.email} in ${v.org.name}${v.mailed ? ", mailed" : ""}`);
  }));
  $("trial-new").addEventListener("submit", guard(async (e) => {
    e.preventDefault();
    // a form's own `name` shadows its field of that name: read them by `elements`
    const f = e.target.elements;
    // of the one seat sold, the always-on: the platform's own default
    const body = { name: f.name.value.trim(), days: Number(f.days.value), capacity: Number(f.capacity.value) };
    if (f.code.value.trim()) body.code = f.code.value.trim();
    const t = await api("POST", "/api/admin/trials", body);
    e.target.reset();
    await trials();
    say(`made ${t.code}`);
  }));
  $("people-more").addEventListener("click", guard(() => people(true)));
  $("orgs-more").addEventListener("click", guard(() => orgs(true)));
  $("log-more").addEventListener("click", guard(() => log(true)));
  await show(location.hash.slice(1) in load ? location.hash.slice(1) : "glance");
}));
