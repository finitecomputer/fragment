// The shell's Billing (docs/billing.md, "The shell"; docs/api.md, Seats
// and orgs, Billing): a person's seat, their credit, and their org's,
// from `GET /api/seat` (`mine`: their seat, whether they are a guest, and
// what is sold, its prices core's). A guest's seat and trial code are the
// welcome's (`seatOffer`, for shell.js `welcome`), which Billing links to;
// a seat's holder sees it, lets an always-on seat's computer sleep, and
// buys credit; an org's admin offers and removes seats and admins, and
// opens Stripe's portal for card and invoices. One seat is sold, always on
// (Paul, 2026-10-15); a `seat` is an operator's comp. Nobody joins an org
// but by their own yes: what an org offers someone (a seat, an admin's
// place) shows here and on the welcome, with who offered it, to Accept or
// Decline (`offers`), and a seat's holder who is no admin may Leave. An org
// is shown once someone besides its admin is in it (a seat, held or
// offered, or another admin): its admin alone, who pays for themselves,
// sees only Payment and invoices, and asks for the org's seats to offer one
// to someone else. Stripe's Checkout and portal are Stripe's pages: the
// shell goes there; a seat's Checkout returns to the welcome
// (`/welcome?checkout=…`), a credit pack's here (`/settings?pack=…`,
// `returning`). A deployment that sells nothing (`offer` null: no Stripe,
// billing off, its seats an operator's to give) shows no price and nothing
// to buy: a guest is told seats are not open (`CLOSED`). Every value is
// text, never markup.

import { icon } from "./icons.js";

const KIND = { seat: "Seat whose computer sleeps when idle", seat_always_on: "Always-on seat" };
// an offer's amount, in whole dollars where it is whole ($200, not $200.00)
const price = (micros) => (micros / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", minimumFractionDigits: 0 });
const CLOSED = "Finite isn't open for new seats yet.";

/** What a seat gives a guest, at its price (`offer`: `GET /api/seat`'s; null, nothing sold). */
export function seatGives(offer) {
  if (!offer) return CLOSED;
  return `A seat, ${price(offer.seatMicros)} a month, gives you agents of your own, the always-on computer they run on, apps of your own, and ${price(offer.includedMicros)} of credit a month.`;
}

/**
 * The shell's helpers this module uses: `{api, el, step}` for the welcome
 * (`step(from, to)`: its card from one step to another), and
 * `{section, line, actionLine, usd, reopen}` besides for settings, with
 * `noSeatSaid` (who is signed in, said to a guest), `switchForm` (Use
 * another account) and `support` (the deployment's help link, or null).
 */
let h = null;

/**
 * A billing failure, said plainly: what happened and what the person can
 * do, never the platform's words, which go to the console for us. `known`:
 * what to say for a refusal by the reason the platform names
 * (`ErrorBody.reason`: one status means several causes, so a status never
 * picks the words); `otherwise`: what to say for anything else (a reason
 * not known here or none, a fault, a dropped connection), safe whatever
 * the cause, with Try again only where its control stays to retry.
 */
function plainly(e, known, otherwise) {
  console.warn("A billing call failed:", e);
  if (e?.reason && known?.[e.reason]) return known[e.reason];
  if (e?.status === 401) return "You're signed out. Reload the page to sign in again.";
  return otherwise;
}
// What a Checkout refused for the person's own state says; Stripe's own
// refusal (`stripe_refused`) and a fault are `otherwise`'s.
const CHECKOUT = {
  not_sold: "Seats aren't on sale right now.",
  holds_seat: "You have a seat already. Open Settings to see it.",
  org_pays: "Your org pays for its seats: an admin adds yours in Settings, under your org.",
};
const CHECKOUT_FAILED = "Checkout didn't open. Try again in a moment.";
// whoever pays for an org stays in it until its subscription has ended
const PAYS = (org) => `You pay for ${org}, so you stay until its subscription has ended. Cancel it in Payment and invoices; once it has ended, try again.`;

// `failed(e)`: what its failure says (`plainly`)
function button(text, onclick, failed) {
  const b = h.el("button", "quiet", text);
  b.type = "button";
  b.onclick = async () => {
    b.disabled = true;
    try {
      await onclick();
    } catch (e) {
      // said beside it: under its offer, or at the end of its section
      note(b.closest(".billing-offer, section"), failed(e));
    } finally {
      b.disabled = false;
    }
  };
  return b;
}

/** A line of warning at the end of `box`, replacing the last one. */
function note(box, text) {
  if (!box) return;
  box.querySelector(":scope > .billing-note")?.remove();
  box.append(h.el("p", "settings-warning billing-note", text));
}

async function goCheckout(body) {
  const v = await h.api("POST", "/api/billing/checkout", body);
  location.assign(v.url);
}

/** What an offer gives, in words (`OfferedSeat`). */
function offered(o) {
  const seat = o.kind ? (o.kind === "seat_always_on" ? "an always-on seat" : "a seat") + (o.comped ? ", given" : "") : null;
  return [seat, o.admin ? "a place as its admin" : null].filter(Boolean).join(" and ");
}

/**
 * The offers made to the person (`mine.offered`), each with who made it,
 * what accepting does, and Accept and Decline; `done(seat)` runs once one
 * is answered (`seat`: their `MySeat` then). A refusal (the last admin of
 * an org others are in, one whose card pays for it) is said under it.
 */
function offers(mine, done) {
  const { el } = h;
  return mine.offered.map((o) => {
    const box = el("div", "billing-offer");
    box.dataset.offer = o.id;
    const from = o.from ? ` (${o.from})` : "";
    box.append(el("p", null, `${o.org.name}${from} offers you ${offered(o)}.`));
    const away = mine.org && mine.org.id !== o.org.id;
    const alone = mine.admin ? "; if no one else is in it, it ends too (who pays cancels its billing first)" : "";
    const said = away
      ? `Accepting moves you: your place in ${mine.org.name} ends${mine.seat ? ", with its seat" : ""}${alone}.`
      : "Nothing of it is yours until you accept it.";
    box.append(el("p", "muted", said));
    const answer = (verb) => async () => {
      if (verb === "accept" && away && !confirm(`Leave ${mine.org.name} for ${o.org.name}?`)) return;
      const seat = await h.api("POST", `/api/seat/offers/${encodeURIComponent(o.id)}/${verb}`, {});
      await done(seat);
    };
    const gone = "This offer isn't open any more: it was withdrawn or already answered.";
    const accepted = (e) =>
      plainly(
        e,
        {
          gone,
          org_not_paying: `${o.org.name} isn't paying for seats right now, so its offer can't be taken yet. Ask its admins.`,
          holds_seat: `You already have a seat in ${o.org.name}: decline this offer.`,
          pays: PAYS(mine.org?.name ?? "your org"),
          last_admin: `You're the last admin of ${mine.org?.name ?? "your org"}: make someone else its admin first, then accept.`,
        },
        "The offer wasn't accepted. Try again in a moment.",
      );
    const declined = (e) => plainly(e, { gone }, "The offer wasn't declined. Try again in a moment.");
    const actions = el("div", "settings-actions");
    actions.append(button("Accept", answer("accept"), accepted), button("Decline", answer("decline"), declined));
    box.append(actions);
    return box;
  });
}

/**
 * A credit pack's Checkout returning (`/settings?pack=`): the credit
 * applied at once, whatever Stripe's webhook's timing. It is the page's,
 * settled once for every load of settings (a second press while it
 * settles finds the query gone): each awaits it, so the one that renders
 * reads the ledger after it and says how it went, and then no load says
 * it again (`returnSaid`). Answers what to say.
 */
let settling = null;
export function returning(helpers) {
  h = helpers;
  const pack = new URLSearchParams(location.search).get("pack");
  if (pack) {
    history.replaceState(null, "", location.pathname);
    settling = settle(pack);
  }
  return settling ?? Promise.resolve({ said: null });
}
async function settle(pack) {
  if (pack === "canceled") return { said: "Checkout was canceled: nothing was bought." };
  try {
    await h.api("POST", `/api/billing/sessions/${encodeURIComponent(pack)}`, {});
    return { said: "Paid: the credit is yours." };
  } catch (e) {
    // said plainly: what happened, and what to do (the platform's words
    // are for us, in the console)
    console.warn("A credit pack's return was not applied:", e);
    return { said: NOT_HERE, help: true };
  }
}
// A pack's return that was not applied: Stripe's webhook applies a paid
// pack within minutes whatever the return did, so Settings show it later.
const NOT_HERE = "Your purchase isn't here yet. If you were charged, the credit arrives within a few minutes: open Settings again to see it.";
/** A return (`returning`'s) a load of settings has rendered: said once. */
export function returnSaid(back) {
  if (back.said) settling = null;
}

/**
 * The welcome's card for a guest (`mine`: `GET /api/seat`'s), under its
 * heading (`top`), in two steps (Skyler's onboarding design, 2026-10-22):
 * the seat, a ticket of what it gives at its price, whose Continue goes to
 * Stripe's Checkout; and a trial code (`trial`: one a mailed link named,
 * which opens on it, filled in), its own step, whose Continue goes there
 * too. What Checkout or a code refuses is said in the step that asked,
 * then what more credit costs. What an org offers them comes first, to
 * accept (then on to their first run, at `/`) or decline. Someone in an
 * org whose admins give its seats is told so instead of the seat. Answers
 * the card's steps, the seat's first.
 */
export function seatOffer(helpers, mine, trial, top) {
  h = helpers;
  const { el } = h;
  const seat = el("div", "welcome-seat");
  seat.append(...top);
  if (!mine) {
    seat.append(el("p", "form-error", "The seat could not be read: reload to try again."));
    return [seat];
  }
  if (mine.offered.length) {
    const boxes = offers(mine, async () => location.assign("/"));
    for (const box of boxes) box.classList.add("ticket");
    seat.append(el("h2", null, "Offered to you"), ...boxes);
  }
  const { offer } = mine;
  if (!offer) {
    seat.append(el("p", null, CLOSED));
    return [seat];
  }
  if (mine.org && !mine.admin) {
    seat.append(el("p", null, `You are in ${mine.org.name}: its admins give you a seat.`));
    return [seat];
  }
  const said = el("p", "form-error");
  said.setAttribute("role", "alert");
  said.hidden = true;
  // the one seat sold (decision 25; docs/ledger.md, Plans), as a guest takes it
  const ticket = el("div", "ticket ticket-plan");
  const head = el("div", "ticket-head");
  const amount = el("div", "price", price(offer.seatMicros));
  amount.append(el("span", null, " / month"));
  head.append(amount, el("span", "pill", "Always on"));
  const gives = el("ul", "checks");
  for (const what of ["Your personal AI agent", "Your own always-on computer", "Apps you can build and share", `${price(offer.includedMicros)} in monthly usage credit`]) gives.append(el("li", null, what));
  // where Continue goes: Stripe's own page
  const stripe = el("li", "lock");
  stripe.append(icon("lock"), el("span", null, "Secure Stripe checkout"));
  gives.append(stripe);
  ticket.append(head, gives);
  const b = el("button", "primary", "Continue");
  b.type = "button";
  b.dataset.plan = offer.kind;
  const toTrial = el("button", "text", "Start a trial");
  toTrial.type = "button";
  seat.append(ticket, b, toTrial, el("p", "fine", `Monthly, before tax. With a seat, more credit is ${price(offer.packMicros)} a pack, kept until spent.`));
  // a trial code, a step of its own
  const form = el("form", "trial-code");
  const code = el("input");
  code.placeholder = "Trial code";
  code.value = trial || "";
  code.autocomplete = "off";
  code.required = true;
  const field = el("label", null, "Trial code");
  field.append(code);
  const begin = el("button", "primary", "Continue");
  begin.type = "submit";
  const back = el("button", "text", "Back");
  back.type = "button";
  form.append(el("h1", null, "Start a trial"), el("p", null, "Enter your trial code."), field, begin, back, el("p", "fine", "No card needed. To keep your seat after the trial, add one in Settings before it ends."));
  // the step shown, and what is said put in it, above its Continue
  const show = (step) => {
    (step === seat ? b : begin).before(said);
    if (step.hidden) h.step(step === seat ? form : seat, step);
  };
  toTrial.onclick = () => {
    said.hidden = true;
    show(form);
  };
  back.onclick = () => {
    said.hidden = true;
    show(seat);
  };
  // one Checkout at a time; a refusal is said; every choice is open again
  // after, as Billing's buttons are (a page Back brings back is usable)
  const go = async (body, step) => {
    show(step);
    const choices = [b, toTrial, begin, back];
    for (const c of choices) c.disabled = true;
    said.hidden = true;
    try {
      await goCheckout(body);
    } catch (e) {
      said.textContent = plainly(
        e,
        {
          ...CHECKOUT,
          trial_unknown: "That trial code isn't one we know. Check it and try again.",
          trial_ended: "That trial code has ended.",
          trial_used: "You've had a trial already: a trial is once a person.",
          trial_taken: "That trial code's places are all taken.",
          trial_not_here: "A trial is for someone starting a seat of their own, so this code can't be used here.",
        },
        CHECKOUT_FAILED,
      );
      said.hidden = false;
    } finally {
      for (const c of choices) c.disabled = false;
    }
  };
  b.onclick = () => go({ kind: offer.kind }, seat);
  form.onsubmit = (e) => {
    e.preventDefault();
    go({ trialCode: code.value }, form);
  };
  b.before(said);
  // a mailed trial's link opens on its code
  if (trial) {
    seat.hidden = true;
    begin.before(said);
  } else form.hidden = true;
  return [seat, form];
}

/** The Billing section, and an admin's Org section (`mine`: `GET /api/seat`'s). */
export async function billingSections(helpers, ledger, mine, back) {
  h = helpers;
  const { el, section, line, actionLine, usd } = h;
  if (!mine) return [section("Billing", el("p", "muted", "Your seat could not be read."))];
  const { offer } = mine;
  const billing = section("Billing");
  if (back?.said) {
    // one sentence (the line lays its children out apart), and, where the
    // deployment names one, where to get help
    const words = el("span", null, back.said);
    if (back.help && h.support) {
      const help = el("a", null, "get help");
      help.href = h.support;
      help.target = "_blank";
      help.rel = "noopener";
      words.append(" If it doesn't, ", help, ".");
    }
    const said = el("p", "settings-line");
    said.append(words);
    billing.append(said);
  }
  // an admin's org, and whether they are alone in it
  const org = mine.admin
    ? await h.api("GET", "/api/org").catch((e) => ({ error: plainly(e, { not_admin: "Only your org's admins see its seats." }, "Your org's seats couldn't be read. Open Settings again to try again.") }))
    : null;
  const alone = org?.members?.length === 1;
  const seat = mine.seat;
  if (seat) {
    const kind = KIND[seat.kind] + (seat.comped ? " (given)" : seat.kind === offer?.kind ? `, ${price(offer.seatMicros)} a month` : "");
    billing.append(line("Seat", kind));
    // a seat's holder who is no admin leaves by their own yes, as they joined
    const leave = seat.admin
      ? null
      : button(
          `Leave ${seat.org.name}`,
          async () => {
            if (!confirm(`Leave ${seat.org.name}? Your seat there ends, and your agents stop. Nothing of yours is deleted.`)) return;
            await h.api("POST", "/api/seat/leave", {});
            await h.reopen();
          },
          (e) =>
            plainly(
              e,
              {
                pays: PAYS(seat.org.name),
                step_down_first: `You admin ${seat.org.name}: step down as its admin first, then leave.`,
                gone: `You're not in ${seat.org.name} any more. Open Settings again to see where you are.`,
              },
              `You're still in ${seat.org.name}. Try again in a moment.`,
            ),
        );
    if (!alone) billing.append(line("Org", seat.org.name + (seat.admin ? " (you admin it)" : ""), leave));
    if (seat.trialEnds) billing.append(line("Trial ends", new Date(seat.trialEnds * 1000).toLocaleDateString()));
    // a trial asks no card: one added before its end keeps the seat, else it lapses then
    if (seat.trialEnds && mine.pays) billing.append(el("p", "muted", "To keep your seat after the trial, add a card in Payment and invoices. Without one, your seat lapses when the trial ends; nothing of yours is deleted, and Pay again here brings it back."));
    if (!seat.good) {
      billing.append(el("p", "settings-warning", "Your seat lapsed: your agents are stopped. Nothing of yours is gone."));
      // paid again at Checkout, where something is sold
      if (offer) billing.append(seat.admin ? actionLine("Renewal", button("Pay again", () => goCheckout({}), (e) => plainly(e, CHECKOUT, CHECKOUT_FAILED))) : el("p", "muted", "Ask your org's admin to pay for it again."));
    }
    if (seat.good && seat.kind === "seat_always_on") {
      const sleeps = el("input");
      sleeps.type = "checkbox";
      sleeps.checked = seat.sleeps;
      sleeps.onchange = async () => {
        try {
          await h.api("PUT", "/api/seat", { sleeps: sleeps.checked });
        } catch (e) {
          sleeps.checked = !sleeps.checked;
          note(billing, plainly(e, { no_seat: "You don't hold a seat any more. Open Settings again to see where it went." }, "That wasn't changed. Try again in a moment."));
        }
      };
      const label = el("label", "settings-line");
      label.append(sleeps, " Let my computer sleep when nothing is open");
      billing.append(label);
    }
  } else if (mine.guest) {
    billing.append(el("p", "muted", `You are a guest: you see and edit what is shared with you. ${seatGives(offer)}`));
    // the seats and a trial code are the welcome's, as a new person's are;
    // or the account a seat of theirs waits under (who is signed in, said)
    const get = el("a", "quiet", "Get a seat");
    get.href = "/welcome";
    const ways = actionLine("Seat", ...(offer ? [get] : []), h.switchForm("/settings"));
    ways.classList.add("settings-stack");
    billing.append(h.noSeatSaid(), ways);
  } else {
    // no seat of theirs: the deployment's plan gives everyone one
    billing.append(line("Seat", `${KIND[ledger?.plan] ?? "A seat"}, given to everyone here`));
    // A default plan is no org seat: someone in no org may buy their own,
    // but the welcome sends this non-guest home before offering Checkout.
    if (offer && !mine.org) {
      // its failure's words by the refusal's reason, as every Checkout's
      const buy = button(
        `Buy an always-on seat, ${price(offer.seatMicros)} a month`,
        () => goCheckout({ kind: offer.kind }),
        (e) => plainly(e, CHECKOUT, CHECKOUT_FAILED),
      );
      buy.dataset.plan = offer.kind;
      billing.append(actionLine("Subscription", buy));
    }
  }
  // what orgs offer them: theirs to accept (a move, from an org they are in) or decline
  for (const box of offers(mine, () => h.reopen())) billing.append(box);
  // credit is a seat's (or a plan the deployment gives): a guest pays for
  // nothing, so has none to show
  if (ledger && !mine.guest) {
    const topUp =
      offer && seat?.good
        ? button(`Buy ${price(offer.packMicros)} of credit`, async () => location.assign((await h.api("POST", "/api/billing/packs", {})).url), (e) =>
            plainly(e, { no_seat: "Credit is for a seat's holder, and you don't hold one now. Open Settings again to see why." }, CHECKOUT_FAILED),
          )
        : null;
    // the balance: below zero, what is owed (a paid call is charged after it, so one can go past zero)
    const balance = ledger.balanceMicros;
    billing.append(line("Credit this month", balance < 0 ? `${usd(-balance)} owed` : `${usd(balance)} left`, topUp));
  }
  if (!alone) return org ? [billing, orgSection(org, offer, mine.pays)] : [billing];
  // an admin alone buys seats for others, where something is sold
  if (!offer) return [billing];
  const more = el("div", "settings-actions");
  const others = el("button", "quiet", "Add seats for others");
  others.type = "button";
  others.onclick = () => {
    more.closest(".settings-action-line")?.remove();
    billing.after(orgSection(org, offer, mine.pays));
  };
  // only who pays opens its billing (Stripe's portal renews it, and changes its card)
  more.append(...(mine.pays ? [portal()] : []), others);
  billing.append(actionLine("Organization", more));
  return [billing];
}

/** Stripe's portal: the org's card, invoices, and cancelling. */
function portal() {
  return button(
    "Payment and invoices",
    async () => location.assign((await h.api("POST", "/api/billing/portal", {})).url),
    (e) => plainly(e, { not_payer: "Only the person who pays for your org opens its payments." }, "Payment and invoices didn't open. Try again in a moment."),
  );
}

/**
 * An admin's org (`GET /api/org`'s): its seats and admins, and, to who pays
 * (`pays`), Stripe's portal; `offer`, what a seat added costs (null:
 * nothing is sold, the table alone). Who pays's own seat and place are
 * theirs alone to give up, and while its subscription lives they stay
 * (the platform says to cancel it first).
 */
function orgSection(org, offer, pays) {
  const { el, section, actionLine } = h;
  const box = section(org.name ? `Org: ${org.name}` : "Org");
  if (org.error) {
    box.append(el("p", "muted", org.error));
    return box;
  }
  const table = el("table", "billing-seats");
  for (const m of org.members) {
    const tr = el("tr");
    const who = el("td", null, m.email + (m.person ? "" : " (invited)"));
    // who pays's seat and place are theirs alone; offers (no person) are withdrawn
    const payer = !!m.person && m.person === org.payer;
    const role = el("td", null, [m.admin ? "admin" : "", m.seat ? KIND[m.seat] + (m.comped ? " (given)" : "") : "", payer ? "pays" : ""].filter(Boolean).join(", "));
    const actions = el("td");
    const now = "Open Settings again to see your org as it is now.";
    if (m.seat && !m.comped && (!payer || pays)) {
      const removed = (e) =>
        plainly(
          e,
          {
            last_paid_seat: "This is your org's last paid seat: to end it, cancel the subscription in Payment and invoices.",
            payer_only: `${m.email} pays for this org: their seat is theirs alone to give up.`,
            gone: `That seat is gone already. ${now}`,
            not_admin: "Only your org's admins remove seats.",
          },
          `${m.email}'s seat wasn't removed. Try again in a moment.`,
        );
      actions.append(
        button(
          "Remove seat",
          async () => {
            if (!confirm(`Remove ${m.email}'s seat? Their agents stop.`)) return;
            await h.api("DELETE", `/api/org/seats/${encodeURIComponent(m.id)}`);
            await h.reopen();
          },
          removed,
        ),
      );
    }
    if (m.admin && org.members.filter((x) => x.admin && x.person).length > 1 && (!payer || pays)) {
      const stepped = (e) =>
        plainly(
          e,
          {
            pays: PAYS("this org"),
            last_admin: "That would leave the org with no admin: make someone else an admin first.",
            payer_only: `${m.email} pays for this org: only they step down as its admin.`,
            gone: `${m.email} isn't an admin any more. ${now}`,
            not_admin: "Only your org's admins change who admins it.",
          },
          `${m.email} is still an admin. Try again in a moment.`,
        );
      actions.append(
        button(
          "Not admin",
          async () => {
            await h.api("DELETE", `/api/org/admins/${encodeURIComponent(m.id)}`);
            await h.reopen();
          },
          stepped,
        ),
      );
    }
    tr.append(who, role, actions);
    table.append(tr);
  }
  box.append(table);
  // its seats and admins are added, and paid for, through Stripe
  if (!offer) return box;
  const add = el("form", "settings-actions");
  const email = el("input");
  email.type = "email";
  email.placeholder = "email";
  email.required = true;
  const go = el("button", "quiet", "Add a seat");
  go.type = "submit";
  add.append(email, go);
  // what an offer of a seat or an admin's place that failed says
  const offerFailed = (err, who, what) =>
    plainly(
      err,
      {
        org_not_paying: `No ${what} was offered: your org isn't paying for seats right now. Pay for yours first (Pay again, under Billing).`,
        not_an_email: `That isn't an email address. Check it and try again.`,
        already_offered: `${who} already has a seat here, or an offer waiting.`,
        org_full: "Your org has as many seats as it can hold.",
        admins_full: "Your org has 10 admins already, offered ones included.",
        too_many_offers: `${who} has as many offers waiting as they can: they accept or decline one first.`,
        offers_per_day: "Your org has sent as many offers as it can today. Try again tomorrow.",
        not_admin: `Only your org's admins offer ${what}s.`,
      },
      `No ${what} was offered to ${who}. Try again in a moment.`,
    );
  add.onsubmit = async (e) => {
    e.preventDefault();
    try {
      await h.api("POST", "/api/org/seats", { email: email.value.trim() });
      await h.reopen();
    } catch (err) {
      note(box, offerFailed(err, email.value.trim(), "seat"));
    }
  };
  const admin = el("form", "settings-actions");
  const adminEmail = el("input");
  adminEmail.type = "email";
  adminEmail.placeholder = "email";
  adminEmail.required = true;
  const makeAdmin = el("button", "quiet", "Add an admin");
  makeAdmin.type = "submit";
  admin.append(adminEmail, makeAdmin);
  admin.onsubmit = async (e) => {
    e.preventDefault();
    try {
      await h.api("POST", "/api/org/admins", { email: adminEmail.value.trim() });
      await h.reopen();
    } catch (err) {
      note(box, offerFailed(err, adminEmail.value.trim(), "admin's place"));
    }
  };
  box.append(
    actionLine("Add a seat", add),
    actionLine("Add an admin", admin),
    el("p", "muted", `A seat or an admin's place is offered by mail, and is theirs once they accept it (someone in the org is given a seat at once). A seat, ${price(offer.seatMicros)} a month and always on, bills from its offer, prorated; one removed is credited on the next invoice. ${pays ? "Cancel in Payment and invoices." : "Its billing is who pays's."}`),
  );
  if (pays) {
    const pay = el("div", "settings-actions");
    pay.append(portal());
    box.append(actionLine("Payment", pay));
  }
  return box;
}
