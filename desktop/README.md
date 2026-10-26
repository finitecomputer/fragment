# Finite.Computer desktop

A native desktop client for Fragment, built with [Native Shell](https://nsh.dev/). The workspace, navigation, launcher, dock, dialogs, and settings are TSX; shared naming, identity-color, title, and search rules are Rust. Fragment’s own chat and app pages run in native web views, so their record rendering and editing stay the platform’s implementation rather than a second chat client.

The visual tokens, Funnel Sans files, and landscape artwork come from `cell/shell`. Light and dark appearance follow the system. The code has no platform-specific branches, but this draft has only been exercised on Linux; macOS and Windows still need validation.

## Run

Install NSH 0.3.0. Linux needs GTK 4.14+ and WebKitGTK 6.0 with their development packages. Then, from the repository root:

```sh
nsh run desktop
```

The client defaults to `https://finite.chat`. To use a local `cargo xtask dev` node instead:

```sh
FRAGMENT_HOST=http://127.0.0.1:8790 nsh run desktop
```

Sign-in uses the deployment’s normal WorkOS flow in a persistent native WebKit session. API calls and the fragment-list socket use that same session through a small, visually hidden bridge on the platform origin. Chats and apps use top-level `/auth/fragment` sessions; this avoids third-party frame-cookie restrictions.

## Check

```sh
nsh check desktop
nsh test desktop
nsh test desktop --target linux
```

The tests cover the Rust rules and native workspace, settings, dock, app prompts, chat switching, and launcher. On a real target they also write screenshots under `target/` in the repository. `App.test.tsx` uses a fixture and never calls a deployment.

Current screenshots are in `screenshots/`:

- `live-workspace.png`: a signed-in local Fragment node and its real chat
- `dock.png`: the app desktop and wallpaper
- `app-prompts.png`: app-creation suggestions
- `launcher.png`: the native launcher
- `settings.png`: native settings

## Draft boundaries

This is a functional draft, not full web-shell parity yet. The main remaining work is:

- Native chat alerts currently answer `unsupported`; there is no desktop push registration or notification click handling.
- Top-level fragment views do not yet reproduce every parent-frame integration (notably confirmed add-agent and composer injection). The bridge types are present, but delivery needs a native-to-page origin adapter.
- Settings covers account, billing actions, computer recovery, agents, sharing access, skills, connections, CLI setup, and export. It does not yet include every model/reasoning control, connection authorization flow, profile-picture editor, or live export progress.
- Pane resizing/reordering, preview-card hover cards, and macOS/Windows packaging checks remain.
