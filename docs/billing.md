# Billing, organizations and the operator's admin

The rules of seats, orgs, Stripe, trials and the operator's admin.
Decisions 51 to 59 and 66 to 71 (docs/cloudflare-v1.md) bind; the wire
contract is docs/api.md (Seats and orgs, Org content, Billing,
Operators); credit and plans are docs/ledger.md. The rules in code:
`fragment_core::org` and `fragment_core::stripe` (pure, host-tested), and
`cell/src/registry/` (`orgs.rs`, `offers.rs`, `seats.rs`, `billing.rs`,
`content.rs`, `trials.rs`, `admin.rs`).

**What is sold**: one seat, `seat_always_on`, and the $25 credit pack
(decisions 25 and 55). Checkout, an admin's offers and trial codes refuse
a `seat` (400, "not sold"); that plan stays for operators' comps and a
deployment's `default_plan`. A deployment whose config has no `stripe`
sells nothing: seats are comped only, `GET /api/seat`'s `offer` is null,
and the shell shows no price and nothing to buy.

## The model

**Nobody joins anything without their own yes** (Paul, 2026-10-17, from
the security audit's H1).

- **An org is the billing account.** Every seat belongs to one; a person
  who pays for themselves is an org of one. An org is made only by a paid
  Checkout or an operator's comp, never before: a Checkout left unpaid
  makes no org and no Stripe customer.
- **Its people**, admins and seat holders, are each in it by their own
  yes: an offer they accepted, their own paid Checkout, or an operator's
  comp (operators are trusted). A person is in at most one org and holds
  at most one seat. An org keeps at least one admin who said yes; an
  admin need not hold a seat.
- **Offers.** While the org pays, an admin offers a seat, an admin's
  place, or both, to an email. Nothing of it is anyone's until whoever
  signs in with that email, verified, accepts it; signing in attaches
  nothing. One of the org's members given a seat has it at once, but an
  admin's place is power over the org, so it is always an offer. A paid
  seat offered bills from the offer (decision 52).
- **Leaving, stepping down and moving** (accepting another org's offer)
  are one rule (`fragment_core::org::departing`): refused for the last
  admin of an org others are in, and for who pays while its subscription
  lives. An org its last person leaves ends, its offers withdrawn.
- **Who pays** is the buyer whose Checkout made the org's subscription
  (`org_payers`). No other admin removes them or takes their seat; only
  they open its billing (Stripe's portal; a default Paul may reverse);
  while its subscription lives they stay (cancel first). So no card pays
  for an org its owner no longer manages. Handing billing to someone else
  is by hand: an operator cancels the subscription, the old payer goes,
  and an admin who accepted the place buys the org again on a customer of
  their own.
- **A seat is paid or comped.** Paid seats are counted on the org's one
  subscription; comped seats are an operator's, outside Stripe, always
  good.
- **Standing is Stripe's status** (decision 53): `trialing`, `active` and
  `past_due` are good (Stripe's retries are the grace); anything else is
  lapsed, both of Stripe's final retry states included, so the shared
  account's choice between them does not matter.

### What a seat does

The registry is the one writer of a seat holder's plan and seat. It
pushes, one queued push a person, each read fresh, to their ledger (their
plan while the seat is good; `canceled` when it lapses or goes) and to
their computer (always-on while their plan is `seat_always_on` and they
have not let it sleep: decision 57). Billing deletes nothing; paying
again restores it.

### An org's own credit

Decisions 66 to 68. An org's ledger (`org:<id>`; docs/ledger.md) pays
for what the org owns, from credit of its own; its seats' included
credit stays each holder's. Orgs pay out of band (a Stripe link, an
invoice): an operator credits it (`fragment operator credit`, under the
payment's own id, so a retry credits it once), sets its monthly budget
(`fragment operator budget`) and names who its credit is invoiced to
(`fragment operator payer`, its governance log's `payer-named`). Each is
logged once by its id, in its governance log and the operators' log,
before its ledger takes it, so a command cut short is finished by
sending it again. Orgs buy no credit and set no budget themselves. Its
billing admins read its ledger; reading or funding it lets no one into
its fragments (decision 69).

### An org's content outlives its billing

Decisions 69 to 71. An org's content admins and its governance log are
the registry's, apart from its members (docs/api.md, Org content). An
org with anyone in its content, or that an operator credited or named a
payer for, is never dropped when its subscription ends and its seats go
(`drop_if_empty`), so its credit is never stranded. A wipe makes its
person no content admin there rather than taking the org with them; an
org left with no content admin waits on an operator's recovery offer,
logged.

### A wipe

A wipe's `billing` step (docs/api.md, "Operators: wiping a person"; the
audit's M4) cancels at once the subscription of every org it ends and of
every org the wiped person pays for; any other org goes on without them.
Wiping the sole admin of an org others hold seats in ends their seats
(the dry run names them). The Stripe customer is kept for its invoices
(tax, refunds by hand), and the registry's link to it dropped.

## Stripe

- **The account is finite-mono's.** Everything fragment makes carries
  `fragment_deployment` in its metadata, and code never changes an object
  without this deployment's (`fragment_core::stripe::ours`). Customers
  are never looked up by email. Fragment has its own webhook endpoint and
  portal configuration; the retry schedule, tax registrations, statement
  descriptor and Stripe's customer mails are the account's. Dev, previews
  and the e2e use fragment's own Stripe Sandbox.
- **A customer per org**, made by its first paid Checkout, the buyer's.
  Only its last payer buys again on it; anyone else buys on a customer of
  their own, and no Checkout names another org's (the audit's H1). A
  customer is never deleted.
- **Prices** are found by lookup key, and their amounts are core's
  constants (`SEAT_CENTS`, `PACK_CENTS`): Checkout refuses a Price of any
  other amount, so a price change is a code change. Automatic tax, prices
  before tax (decision 54).
- **One subscription per org**, one item, its quantity the org's paid
  seats, held or offered. The seats are the registry's and the quantity a
  copy, pushed prorated. No paid seat left cancels the subscription at
  once and deletes its pending prorations; an admin's removal of the last
  paid seat is refused (the portal cancels at the period's end).
- **The registry's copy** of a subscription is written only from one
  fetched from Stripe: at the Checkout's return, on a webhook (`POST
  /api/stripe/webhook`, its signature checked), and by a daily reconcile.
  An event without this deployment's metadata is answered 200 and
  dropped; an event older than the last applied changes nothing.
- **Credit packs** are a Checkout payment, its session id the grant's id
  (docs/ledger.md). An admin may buy one for a member. No usage goes to
  Stripe.
- **Discounts** are Stripe promotion codes, made in Stripe's dashboard.
- **Its keys** are store secrets (docs/secrets.md); the restricted key's
  grants are in deploy/example.jsonc. `cargo xtask stripe check|setup`
  audits and makes the Prices, portal configuration and endpoint; live
  mode is Paul's.

## Trials

Decision 56. A code names the always-on seat, its days (1 to 30), its
places and its expiry. A guest who never had a subscription redeems it
in the welcome (or its mailed link, `/welcome?trial=<code>`): one
transaction checks a place and that they never trialed, and records the
use. Its Checkout asks no card, and its expiry frees the place. At the
trial's end Stripe charges a card added meanwhile in the portal; with
none it cancels the subscription (cancel, not pause: Pay again is already
the way back for any lapsed org). One trial per person ever, for a seat
of one.

## The operator's admin

Decision 59: `/admin`, served to an operator's session alone (to anyone
else, the 404 of a missing path), and `/api/admin/*`. People (comp a seat,
end a comp, grant credit), invites (a comped seat, or a mailed trial),
trial codes (a change is compare-and-set, its capacity never below the
places taken), orgs, billing's health, and an audit log of every write.
The wipe stays a signed CLI call. It is a row of SPECIAL-CASE-INVENTORY.md.

## The shell

- **The welcome** (`/welcome`) is a guest's first run, never through
  settings: a guest with nothing shared and no invite lands there. It
  offers the seat (Checkout) and a trial code, and takes back Checkout's
  return (`?checkout=<session>`, or `canceled`) and a trial's link. A paid
  return applies the Checkout, waits for the plan to reach the ledger,
  then opens the first run in its chat.
- **Who is signed in** is said wherever someone is told they have no
  seat, with Use another account (`POST /auth/switch`): a person signed
  in under the wrong email must see it.
- **Billing**, in Settings: a guest sees the seat's offer; a seat holder
  their seat, credit ("left", or "owed" below zero) and the pack; anyone
  offered something, each offer to accept or decline; an org's admin its
  seats; who pays, the portal; a lapsed person, how to pay again.
- `GET /api/seat` alone says who is a guest and what things cost
  (`offer`), so the welcome, home and Billing never disagree.

## Not in v1

Annual plans, team trials and invoices by bank transfer; WorkOS
Organizations and SSO; several orgs per person; usage sent to Stripe;
refunds, which stay in Stripe's dashboard; paying with a key (decision
50).
