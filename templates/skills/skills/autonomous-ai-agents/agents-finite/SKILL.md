---
name: agents-finite
description: Make a bot or agent (an analyst, an assistant) and lead your owner's team. Use when your owner asks for a specialist, when you lead their other agents, or when a lasting job needs its own context. Brief, change, start and stop teammates on Fragment, reusing one whose job already fits.
---

# Your owner's agents

Your owner talks to one agent, their main (default) agent, and it leads
the rest: it makes an agent when a job deserves one, briefs it, checks
what comes back, keeps its job current, picks its model, and stops it
when it is no longer needed. Your owner never has to manage agents
themselves. If your `SOUL.md` says you are their main agent and lead
their team, that is you; any other agent does this only when its owner
asks.

An agent is a fragment your owner owns (its job in `SOUL.md`, its model in
`agent.json`, its memories), running on your owner's computer beside you,
with a chat of its own with your owner. You act for your owner, so you
make and change their agents as they would in Settings.

## When to make one

You are capable: most work is yours. A question, a draft, a fix, anything
you can do well in a turn or two, do yourself; handing it off only adds a
step. Make an agent when one is better:

- **A job that lasts and has a context of its own:** a routine (the
  weekly newsletter, the garden log), or a domain your owner keeps coming
  back to (their research, their finances, a side project). Its own job,
  memory and chat keep that context out of yours.
- **Long work that can run beside you:** a deep research pass, a long
  build. Brief it with `fragment ask` without `--wait`, keep talking with
  your owner, and tell them when its answer comes.
- **A model your owner wants** for something (below).

A tool your owner uses is an app, not an agent. Keep the team small: a
few agents with clear jobs beat many vague ones. When your owner asks for
an agent, make it.

## An agent, a skill or a script?

Your owner says "bot", "agent", "analyst", "assistant": each means an
agent. They differ by who it is for:

- **An agent** is someone your owner can talk to: a job, a memory and a
  chat of its own, on their team. A stock analyst for their account, a
  newsletter writer, a researcher: each is an agent.
- **A skill** is a recipe you or a teammate follow. Save one for a
  procedure that repeats.
- **A script** is a tool that a skill or an agent runs.

When your owner asks for a bot, make the agent. Write what it does into
its job, and name any managed skill its work needs (for markets,
`trading-agent-finite`): it loads them and saves its own skills as it
works. Never save a skill or a script and call it their bot.

## Make one

First look at the team you lead (`fragment list --json`, kind `agent`,
and your roster): when an agent already does this job, or nearly, brief
it, and widen its job if it needs to (`fragment agent <agent>
--job-from -`), rather than make another. Your owner's team grows one
specialist per job.

Name a new one by its job: one to three lowercase words joined by
dashes (`melee-researcher`, `hardware-deal-finder`, `website-designer`,
`ai-stock-consultant`). That is its label, its @name and the name your
owner sees; an agent is never given a person's name.

```sh
fragment agent --new garden-keeper --job-from - <<'JOB'
You keep the garden's notes for your owner: what was planted where and when, what to water, what to harvest.
You answer the default agent's questions about the garden, plainly. You never message anyone outside this computer.
JOB
```

It makes the agent fragment, starts it on your owner's computer, writes
its job, puts it live, and makes its own chat with your owner
(`garden-keeper-chat`), which it leaves you out of, so the new agent
leads it. Run it again after an error: it finishes what the first try
made. A name another agent has already is refused, never rewritten:
that agent exists, so brief it, or name this one by what sets its job
apart.

Write the job as who it is and what it does, in the second person:

- The first line is one sentence of its role: your teammates see it
  beside its name on their roster.
- Then what it does, what it never does, and who it answers to (you,
  and its owner).
- Never its name, which a rename changes.
- Pass it with `--job-from -` and a quoted marker (`<<'JOB'`), as above:
  the shell changes nothing between the markers, so `$`, backticks and
  quotes stay as you wrote them.

Then brief it with `fragment ask` (it is not on your roster yet), and
tell your owner in one plain line what you made and why ("I set up
garden-keeper to keep your garden notes; ask me or garden-keeper anything
about the garden").

```sh
fragment ask garden-keeper --from - --wait <<'ASK'
Here is what your owner planted this spring: …
ASK
```

`fragment ask` reaches it at once. `message_agent` reaches it once it is
on your roster, which Hermes builds as a conversation starts.

## Keep them

```sh
fragment list --json                          # your owner's fragments: agents are kind "agent"
fragment agent garden-keeper                   # its name, where its model calls go, whether it runs
fragment agent garden-keeper --job "…"         # its job, rewritten
fragment agent garden-keeper --tier medium     # the platform's stronger tier
fragment agent garden-keeper --stop            # stopped: its job and memories kept
fragment agent garden-keeper --start           # running again
```

When your owner's needs change, change the job rather than make a second
agent for the same thing. Stop an agent nobody has needed for weeks, and
say so. Never stop or start yourself (the platform refuses), never edit
another agent's memories, and never message `@hermes` (the computer's
gateway, not an agent). A computer runs at most 16 agents.

## Models

Use the platform's tiers by default. Pick another provider only when
your owner asked or agreed and connected it:

```sh
fragment agent research --tier medium
fragment agent research --model openrouter:anthropic/claude-sonnet-5.5
fragment agent research --no-model
```

## Limits

Hand-offs stop after 3 in a row without a person speaking, and a chat
allows its agents 20 turns of each other in 5 minutes: past either, ask
your owner instead of looping. What you do here is recorded as yours,
for your owner: their events say an agent started or stopped an agent.
