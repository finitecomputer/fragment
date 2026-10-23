---
name: open-slide-finite
description: Create, edit, and publish browser slide decks with open-slide as Fragment apps. Use for presentations, pitch decks, talks, or slide revisions delivered at a web link, or when the person names open-slide. For PowerPoint-only deliverables use Hermes' powerpoint skill. Google Slides API editing is outside this workflow.
---

# Presentations with open-slide

Use [open-slide/open-slide](https://github.com/open-slide/open-slide) for
React slide authoring and `apps-finite` for Fragment publishing and sharing.
The current agent writes the deck; no separate coding agent or provider key
is needed. This skill owns workspace setup, hosting and recovery. Upstream
skills own slide structure, design, assets, notes and transitions.

## Start or resume

For an existing deck, find its fragment and working copy first. Reuse
installed dependencies when the lockfile is unchanged. After pulling into
a fresh folder, run `npm ci` and `npm run sync:skills` from its saved
lockfile. Keep its versions and edit the same fragment; do not scaffold
over it or make a second link. See [publishing and recovery](references/publishing.md).

For a new deck, use a new folder under `~/apps`. The computer already has
Node and npm. These tested versions are deliberate; do not substitute
`@latest` or upgrade an existing deck as part of a content edit.

```sh
mkdir -p ~/apps && cd ~/apps
npm exec --yes --package=@open-slide/cli@2.0.0 -- open-slide init my-talk --use-npm --no-install --no-git
cd my-talk
npm install --save-exact @open-slide/core@2.3.1 react@19.2.8 react-dom@19.2.8
npm run sync:skills
```

Choose a meaningful folder and deck id instead of `my-talk`. Replace the
new scaffold's `getting-started` example with the requested deck; preserve
existing user decks. Keep the standard `slides/`, `themes/` and `assets/`
directories. In `open-slide.config.ts`, set `base: '/'` and
`agent: { enabled: false }`; keep the slide browser and slide UI enabled.
Fragment serves the build at its own host root.

## Read upstream guidance on demand

`sync:skills` supplies the pinned package's skills under the workspace's
`.agents/skills/`. Read these files with the terminal/file reader, even if
Hermes does not list them in its skill catalog. Resolve their linked
references relative to each upstream skill's directory, not this one.

| Task | Read in the workspace |
|---|---|
| Create a deck | `.agents/skills/create-slide/SKILL.md`, then `.agents/skills/slide-authoring/SKILL.md` |
| Revise a deck | `.agents/skills/slide-authoring/SKILL.md`; locate and read the affected page first |
| Create a reusable theme | `.agents/skills/create-theme/SKILL.md` |
| Resolve a selected page or inspector comments in a running dev UI | `.agents/skills/current-slide/SKILL.md` or `.agents/skills/apply-comments/SKILL.md` |

Load only the relevant authoring references. Do not copy these skills into
the agent's global skills tree or install another agent to follow them.

Apply `apps-finite`'s fast first reveal: infer audience, length, theme and
tone from the request and existing context; ask only for blocking missing
information. Upstream's `AskUserQuestion` means an ordinary question here,
not a tool to install or a requirement to repeat answered questions.
Its restriction to editing slide files applies to authoring; setup and
publishing use the configuration and commands in this skill. Deliver a
working Fragment link, replacing upstream's localhost-only handoff.

## Build, check and publish

Follow [publishing and recovery](references/publishing.md) to build into
`site/`, add the static route entries and publish source alongside output.
Show a coherent first draft early. Before calling the deck finished,
inspect every page for clipping, unreadable type and missing assets; check
navigation, a direct-link reload and the presenter view at the served URL.

The published build is a presentation viewer. The inspector, design edits
and file-backed comments need open-slide's dev server; they do not persist
through the static site. Take revision requests in chat, edit source,
rebuild and redeploy the same fragment. Only start `npm run dev` when a
local authoring session or its preview tools are needed. Use the published
viewer for ordinary QA. If PDF/PPTX export is requested, check the actual
export before promising layout fidelity or editability.
