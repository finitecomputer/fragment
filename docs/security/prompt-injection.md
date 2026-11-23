# Prompt injection (FIN-589)

An agent reads untrusted text: email, web pages, documents, messages
from other people. That text may try to steer it. This page says what
limits an agent today, whatever it is told, and what does not.

Google's Workspace API user data policy asks AI apps for prompt-injection
protection (Model Armor or an equivalent). Finite has no classifier of
that kind today. What it has are limits outside the model, below, and
one instruction inside it.

## What limits an agent, whatever it reads

These hold in the platform's code, not in the model's judgment.

### It holds no credential

- The computer's guest holds no secret. Every credential is a
  per-agent placeholder (`fcx_…`, `fck_…`), a MAC under a key only the
  platform holds (docs/computers.md, "Connections and operator keys").
- The real token is added only at the computer's egress, only toward
  that provider's own hosts. A placeholder sent anywhere else is
  refused before any provider is reached (`swap::Plan`;
  `cell/intercepts.mjs`). So an injected agent cannot send a Google
  token to an attacker: it never has one.
- A connection's token is asked of WorkOS Pipes for each request and
  never held on the computer.
- An operator key shared by every agent is never sent on the calls its
  catalog row denies, which would reach other people's data at that
  vendor (`catalog::Deny`; docs/computers.md, "Calls an operator key
  never goes on").
- An agent cannot read, write or move its owner's Vault secrets. Every
  Vault route refuses an agent (docs/secrets.md, "Only its person writes
  it").

### It can do only what its owner allowed

- **Connection scopes are the person's.** As they connect Google, they
  pick read only or read and write, and untick what they don't want. The
  grant holds only those scopes (docs/pipes-browser-binding.md,
  "Access"). A read-only grant cannot send mail or change files, whatever
  the agent is told.
- **The owner may narrow an agent's connections** to a list (decision 44;
  `fragment_core::grants`). This is specialization, not a security
  boundary: a person's agents are not fenced from each other.
- **An agent acts for its owner alone**, at the lower of its owner's role
  and its own cap, never above `editor` (R17; `access::may_act_for`). It
  cannot claim to act for anyone else.
- **Some acts are never an agent's.** An agent never deletes a fragment
  it shares for its owner or sets a cap (`access::agent_shares`, decision
  36); an org's deletion, full export and transfer are a content admin's
  own action (decision 70); a secret's move into a fragment is its
  person's, never a page's or an agent's (docs/secrets.md); a card is
  answered only by the agent's owner (decision 42); invites wait for
  their person's yes (decision 63).
- **Spending is capped.** A fragment's costs stop at its monthly cap
  (decision 26); agents stop at zero credit (decision 27); a shared key
  has an hourly cap per person (`key_hour`).

### It runs apart from other people

- One computer per person, a container of its own (decision 13). Its
  agents act as themselves, with their grants, never as their owner
  (docs/computers.md, "What a computer is").
- What an agent reaches on the platform is what its owner reaches. An
  injected agent can harm its own person's data; it has no route to
  another person's except what they shared with its owner.
- Apps run in a Durable Object Facet with no network
  (`globalOutbound: null`); a job reaches out only through `job.fetch`,
  which the platform takes (docs/MODEL.md).

## The instruction inside the model

Every turn, our Hermes image tells the agent to ask its owner first, in
plain words and with a card only the owner can answer, before it makes
something of theirs public, shares it with someone new, sends an email
or a message in their name, deletes something of theirs, or spends their
money, unless they asked for exactly that in this chat
(`images/hermes/boot/src/hermes.rs`, `ASKING`; docs/computers.md,
"Approvals in our Hermes image").

This is a prompt. A model can be talked out of it. It is not a control
the platform enforces.

## What does not limit an agent

Stated plainly, so the lab reads it here first:

- **Computers reach the internet** (decision 43). The guest holds no
  secret, so traffic that bypasses the egress carries nothing of ours.
  But data the agent has read (an email's text, a file) can be sent by
  an injected agent to any host. There is no egress allowlist for
  computers.
- **No technical approvals.** Hermes' own approvals are off
  (`approvals.mode: off`, decision 16): commands, files and code run
  without asking. Hermes' user has passwordless `sudo` on its computer
  (docs/computers.md, "Root in our Hermes image").
- **No injection detection.** No classifier, Model Armor or equivalent
  screens what an agent reads or what it is about to do.
- **Read and write grants can act.** An agent with a read-and-write
  Google grant can send mail or change files if it is steered to and
  ignores the `ASKING` instruction. Nothing outside the model asks the
  person before a Gmail send.
- **A person's agents are not fenced from each other** (decision 44). An
  agent steered by one chat can use everything its person has.

## Gaps

- Exfiltration: no egress limit on what an agent can send after reading
  untrusted content, and no prompt-injection screening (Model Armor or an
  equivalent). FIN-630.
- No enforced confirmation before outward acts (sending mail in the
  person's name) on read-and-write grants. FIN-631.

## To confirm

- Paul: whether Google's reviewers will accept these limits plus the
  `ASKING` instruction as "an equivalent" of Model Armor, or whether a
  screening step or an enforced confirmation is needed first.
- Paul: whether a narrower egress for agents holding a Google grant is
  acceptable against decision 43.
