# An org admin's page: a design

Exploration, 2026-10-28, read against master at `3f7cb107`. A design
proposal, not a build: `index.html` is a clickable mockup on sample
data, in the shell's colors and Funnel Sans, styled after
finitecomputer/finite-mono#961 (pill tabs, a sortable full-width table,
a row that opens a dialog with a details rail). Today an org admin
manages their org in one card in Settings (`cell/shell/billing.js`,
`orgSection`); this proposes a page of its own, Organization, beside
Apps and Settings.

## Seeing it

Open `index.html` in a browser. Served from the repo root (any static
server), it loads the shell's fonts from `cell/shell/`; opened as a file,
the browser falls back to its own. Its query string:

- `?n=70`: the org filled to that many people (generated), to see the
  page at size; without it, fourteen.
- `?invites=0`: no one waiting, for the Invites tab's empty state.
- `?view=member|confirm|invites|invite|billing`: opens that screen.

Nothing it does is saved: every action changes the sample data in the
page and says so in a toast.

## What it shows

- **Members.** Each person, admins first: name (a crown for an admin),
  email, role, seat (always on, sleeps, none; given), status, credit
  this month as a bar, last active. Sorted by any column, searched,
  paged at 25, 50 or 100. The bar and the column headers stay on screen
  while it scrolls; the columns keep their widths whatever is shown.
- **A person.** Their credit (and buying $25 more for them), their role
  (make admin; remove as admin, or step down), their seat (remove; give
  one to a seatless admin), and a rail of details. An invite shows its
  link, Resend and Withdraw instead. Anything that takes something away
  asks first, in one confirm of the page's own.
- **Invites.** Who is offered what, by whom, when; Copy link and Resend.
  The tab stays when no one is waiting (nothing moves), and says so.
- **Invite.** An email and a toggle, Make them an admin, and the price:
  $200 a month, billed from today and prorated on the next invoice.
- **Billing.** The subscription: its monthly total, seats (paid and
  given), renewal, status, who pays, a contact for billing questions,
  and Payment and invoices (Stripe's portal). Credit across the org
  this month.
- **A phone.** Each person a card of name, email and usage; the rest is
  a tap away, in a sheet from the bottom.

## What it needs that the API does not give

Built today, `GET /api/org` gives each member's email, admin, seat,
comped and when they were added, and who pays (docs/api.md). The page
also shows, and a build would need:

- **A name** for each person, and for the org (an org is named by its
  first email today).
- **Each member's credit this month** and **last active**, to their
  org's admins: the ledger is each person's own (docs/ledger.md).
- **An invite's link**, and **resending** one. An offer is answered
  today by signing in with its email and accepting in Settings; a link
  would only open that, never let anyone else take it (the audit's H1,
  docs/billing.md, Offers).
- **Who invited** someone, and when.

## Not decided here

- **Orgs paid by invoice** (bank transfer, kept by hand): to the
  platform they are all comped seats, so the Billing card's Stripe
  lines would not hold, "Given" would read as free, and their admins
  could not invite (adding a seat needs the org to pay through Stripe,
  `org_pays` in `cell/src/registry/seats.rs`). docs/billing.md lists
  invoices paid by bank transfer as not in v1. A product call.
- **Sorting on a phone**, where the column headers are hidden.
- Whether the Role column stays now that the crown says it.
