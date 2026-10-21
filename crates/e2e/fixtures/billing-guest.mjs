// docs/billing.md, "The shell" and "Code, secrets, config".
// Exercise production renderers and their handlers. Elements and API are
// recorded helpers, not a browser DOM or a real registry/Stripe service.
import assert from "node:assert/strict";
import { test } from "node:test";
import { billingSections, seatOffer } from "../../../cell/shell/billing.js";

function el(tag, classes, text) {
  return {
    tag, classes, textContent: text ?? "", children: [], dataset: {},
    classList: { add() {} },
    append(...children) { this.children.push(...children.filter((c) => c != null)); },
    setAttribute(name, value) { this[name] = value; },
  };
}
const nodes = (roots) => roots.flatMap((n) => typeof n === "object" ? [n, ...nodes(n.children)] : []);
const words = (roots) => roots.map((n) => typeof n === "object" ? `${n.textContent} ${words(n.children)}` : n).join(" ");
const buttons = (roots) => nodes(roots).filter((n) => n.tag === "button");
const links = (roots) => nodes(roots).filter((n) => n.tag === "a");
const offer = { kind: "seat_always_on", seatMicros: 200_000_000, includedMicros: 100_000_000, packMicros: 25_000_000 };
const org = { id: "org-0000000000000001", name: "Test team" };
const guest = (billingEnabled = true) => ({ seat: null, org: null, admin: false, pays: false, offered: [], guest: true, billingEnabled, offer });
const ledger = { plan: "seat_always_on", availableMicros: 100_000_000 };
function holder({ billingEnabled = true, good = true, comped = false, admin = false } = {}) {
  return { ...guest(billingEnabled), guest: false, org, admin, seat: { id: "mem-0000000000000001", org, kind: "seat_always_on", good, comped, admin, sleeps: false } };
}
function helpers(mine, replies = {}) {
  const calls = [];
  const navigated = [];
  globalThis.location = { assign: (url) => navigated.push(url) };
  const group = (label, ...children) => {
    const n = el("section", null, label);
    n.append(...children);
    return n;
  };
  return {
    calls, navigated, el, section: group, line: group, actionLine: group,
    usd: (micros) => `$${micros / 1_000_000}`,
    noSeatSaid: () => el("p", null, "This account has no seat."),
    switchForm: (back) => Object.assign(el("form", null, "Use another account"), { action: "/auth/switch", back }),
    reopen: async () => calls.push(["reopen"]),
    api: async (method, path, body) => {
      calls.push([method, path, body]);
      if (Object.hasOwn(replies, path)) return replies[path];
      if (method === "GET" && path === "/api/org" && mine.admin) {
        return { ...org, members: [{ id: mine.seat?.id, person: "test-person", email: "member@example.test", seat: mine.seat?.kind, comped: mine.seat?.comped, admin: true }] };
      }
      throw new Error(`Unexpected API call: ${method} ${path}`);
    },
  };
}
function noPurchases(roots) {
  assert.equal(buttons(roots).some((b) => b.dataset.plan || /Start a trial|Pay again|Buy .*credit|Payment and invoices|Add a seat|Add seats for others/.test(b.textContent)), false);
  assert.equal(nodes(roots).some((n) => n.classes === "trial-code"), false);
}

test("a guest's Billing explains shared access, links to the welcome and permits account switching", async () => {
  const mine = guest();
  const h = helpers(mine);
  const roots = await billingSections(h, null, mine);
  assert.match(words(roots), /see and edit what is shared with you/);
  assert.match(words(roots), /This account has no seat/);
  assert.match(words(roots), /Use another account/);
  assert.deepEqual(links(roots).map((n) => n.href), ["/welcome"]);
  noPurchases(roots);
  assert.deepEqual(h.calls, []);
});

test("billing-off guests and ended comps have an operator next step without checkout or trial controls", async () => {
  for (const plan of ["guest", "seat", "seat_always_on"]) {
    const mine = guest(false);
    const h = helpers(mine);
    const welcome = seatOffer(h, mine, "TEST-TRIAL");
    const billing = await billingSections(h, { ...ledger, plan, seat: "canceled" }, mine);
    noPurchases([...welcome, ...billing]);
    assert.match(words(welcome), /operator/i);
    assert.match(words(billing), /operator/i);
    assert.match(words(billing), /Use another account/);
    assert.deepEqual(h.calls, []);
  }
});

for (const verb of ["accept", "decline"]) {
  test(`a billing-off comp offer keeps explicit ${verb}, with no purchase`, async () => {
    const mine = guest(false);
    const id = "mem-0000000000000002";
    mine.offered = [{ id, org, kind: "seat_always_on", comped: true, admin: true, from: "operator@example.test" }];
    const path = `/api/seat/offers/${id}/${verb}`;
    const h = helpers(mine, { [path]: verb === "accept" ? holder({ billingEnabled: false, comped: true }) : guest(false) });
    const roots = seatOffer(h, mine, "");
    assert.match(words(roots), /Nothing of it is yours until you accept/);
    assert.deepEqual(buttons(roots).map((b) => b.textContent), ["Accept", "Decline"]);
    noPurchases(roots);
    assert.deepEqual(h.calls, [], "rendering an invitation grants nothing");
    await buttons(roots).find((b) => b.textContent.toLowerCase() === verb).onclick();
    assert.deepEqual(h.calls, [["POST", path, {}]]);
    assert.deepEqual(h.navigated, ["/"]);
  });
}

test("billing-off comp holders and admins keep credit and sleep controls without paid actions", async () => {
  for (const admin of [false, true]) {
    const mine = holder({ billingEnabled: false, comped: true, admin });
    const h = helpers(mine, { "/api/seat": {} });
    const roots = await billingSections(h, ledger, mine);
    assert.match(words(roots), /Always-on seat \(given\)/);
    assert.match(words(roots), /Credit this month/);
    noPurchases(roots);
    if (admin) assert.ok(buttons(roots).some((b) => b.textContent === "Manage organization"));
    const sleep = nodes(roots).find((n) => n.type === "checkbox");
    assert.ok(sleep);
    sleep.checked = true;
    await sleep.onchange();
    assert.deepEqual(h.calls.at(-1), ["PUT", "/api/seat", { sleeps: true }]);
  }
});

test("a deployment's default seat shows its credit without a purchase", async () => {
  const mine = { ...guest(false), guest: false };
  const h = helpers(mine);
  const roots = await billingSections(h, ledger, mine);
  assert.match(words(roots), /given to everyone here/);
  assert.match(words(roots), /Credit this month/);
  noPurchases(roots);
  assert.deepEqual(h.calls, []);
});

test("billing-off org tables retain members and admin offers, with no paid seat or portal actions", async () => {
  const mine = { ...holder({ billingEnabled: false, comped: true, admin: true }), pays: true };
  const members = [
    { id: "mem-0000000000000001", person: "test-person", email: "admin@example.test", seat: "seat_always_on", comped: true, admin: true },
    { id: "mem-0000000000000002", person: "test-member", email: "member@example.test", seat: "seat_always_on", comped: false, admin: false },
  ];
  const h = helpers(mine, { "/api/org": { ...org, payer: "test-person", members } });
  const roots = await billingSections(h, ledger, mine);
  assert.match(words(roots), /member@example.test/);
  assert.match(words(roots), /Add an admin/);
  assert.match(words(roots), /operator/i);
  assert.ok(buttons(roots).some((n) => n.textContent === "Remove seat"), "existing seat management is unchanged");
  noPurchases(roots);
  assert.deepEqual(h.calls, [["GET", "/api/org", undefined]]);
});

test("lapsed seats name restoration, with renewal only while billing is enabled", async () => {
  for (const billingEnabled of [false, true]) {
    for (const admin of [false, true]) {
      const mine = holder({ billingEnabled, good: false, admin });
      const h = helpers(mine, { "/api/billing/checkout": { url: "/test-checkout" } });
      const roots = await billingSections(h, ledger, mine);
      assert.match(words(roots), /Your seat lapsed: your agents are stopped. Nothing of yours is gone/);
      assert.equal(nodes(roots).some((n) => n.type === "checkbox"), false);
      const pay = buttons(roots).find((n) => n.textContent === "Pay again");
      assert.equal(!!pay, billingEnabled && admin);
      if (!billingEnabled) {
        noPurchases(roots);
        assert.match(words(roots), /operator/i);
      } else if (admin) {
        await pay.onclick();
        assert.deepEqual(h.calls.at(-1), ["POST", "/api/billing/checkout", {}]);
        assert.deepEqual(h.navigated, ["/test-checkout"]);
      } else {
        assert.match(words(roots), /Ask your org's admin/);
      }
    }
  }
});

test("billing-enabled welcome still checks out the offered seat", async () => {
  const mine = guest();
  const h = helpers(mine, { "/api/billing/checkout": { url: "/test-checkout" } });
  const roots = seatOffer(h, mine, "TEST-TRIAL");
  const plan = buttons(roots).find((b) => b.dataset.plan === offer.kind);
  assert.ok(plan);
  assert.equal(nodes(roots).find((n) => n.placeholder === "Trial code").value, "TEST-TRIAL");
  await plan.onclick();
  assert.deepEqual(h.calls, [["POST", "/api/billing/checkout", { kind: offer.kind }]]);
  assert.deepEqual(h.navigated, ["/test-checkout"]);
  assert.equal(plan.disabled, false);
});

test("billing-enabled holders still buy credit and payers still open their portal", async () => {
  const mine = { ...holder({ admin: true }), pays: true };
  const h = helpers(mine, {
    "/api/billing/packs": { url: "/test-pack" },
    "/api/billing/portal": { url: "/test-portal" },
  });
  const roots = await billingSections(h, ledger, mine);
  await buttons(roots).find((b) => /^Buy .*credit$/.test(b.textContent)).onclick();
  await buttons(roots).find((b) => b.textContent === "Payment and invoices").onclick();
  assert.deepEqual(h.calls.slice(1), [["POST", "/api/billing/packs", {}], ["POST", "/api/billing/portal", {}]]);
  assert.deepEqual(h.navigated, ["/test-pack", "/test-portal"]);
});

test("an unreadable seat offers a reload and no purchase", async () => {
  const h = helpers(null);
  const roots = seatOffer(h, null, "");
  assert.match(words(roots), /reload to try again/);
  noPurchases(roots);
  assert.deepEqual(h.calls, []);
});
