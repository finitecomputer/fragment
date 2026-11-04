# Finite for iOS

A native SwiftUI client for Finite.Computer, organized like the web shell. iOS 26+,
Swift 6, Apple frameworks and pinned P256K 0.21.1 for CLI-compatible NIP-98 signing.
No Rust bindings, shared runtime, or web chat UI. App fragments and imported conversations open in a native WKWebView sheet.

## Connect to a real account

Launch without `--server`, enter your Finite platform’s HTTPS address, and tap
Connect. Enter the phone’s approval code at that server’s `/cli` page and approve
the Finite-iOS request. Sign-in remains the website’s WorkOS flow. The phone uses
its own approved key in Keychain; never import the CLI’s private key. Settings →
Account connection can revoke/disconnect this phone. If its server is unreachable,
Remove from this iPhone clears the saved connection locally and explains that the
server key still needs revoking on the web.

Signed API calls use the existing live service. Private apps/brains require the
new `POST /api/f/{name}/session` endpoint deployed on that service. See
[the integration contract](../docs/ios.md) for session revocation, consent, local
draft/outbox storage, tests, and remaining push/distribution work. Debug accepts
loopback/local HTTP origins; `--pair-server http://127.0.0.1:8790` pre-fills pairing
for a Mac-forwarded Linux dev stack. Explicit `--server` always selects the fixture.

## Run locally

On the Mac, from the repository root:

```sh
cargo run --locked --manifest-path ios/dev-server/Cargo.toml --target-dir target/ios-fixture
open ios/Fragment.xcodeproj
```

Choose the **Fragment** scheme and an iPhone Simulator, then Run. The
checked-in Xcode project needs no generator or dependency installation.
Choose **Try the local preview** on Connect; the fixture defaults to
`http://localhost:8910`. Settings → This iPhone → Local development
can change it; `--server http://localhost:8910` overrides it for UI tests.
Default launch offers CLI-style account connection. Release permits HTTPS
platforms and cannot connect to the fixture. Xcode resolves the pinned signing package.

To build and launch from a terminal (keep the fixture running separately):

```sh
xcrun simctl list devices available
# Set this to the iOS 26.5 iPhone's UDID from the list above.
FRAGMENT_IOS_SIMULATOR='paste-simulator-udid-here'
xcodebuild -project ios/Fragment.xcodeproj -scheme Fragment -configuration Debug -destination "platform=iOS Simulator,id=$FRAGMENT_IOS_SIMULATOR" -derivedDataPath target/ios build
xcrun simctl boot "$FRAGMENT_IOS_SIMULATOR" # only if not already booted
xcrun simctl bootstatus "$FRAGMENT_IOS_SIMULATOR" -b
xcrun simctl install "$FRAGMENT_IOS_SIMULATOR" target/ios/Build/Products/Debug-iphonesimulator/Fragment.app
xcrun simctl launch "$FRAGMENT_IOS_SIMULATOR" build.finite.fragment --server http://localhost:8910
open -a Simulator
```

For T3 Code on Linux, build and run both the app and fixture on the Mac in
a dedicated build folder. Enable **Settings → Integrations → Devices →
Agent device access**, add the Mac runner, and start a fresh agent session
so it receives device access. Open that Mac's simulator in the thread's
Device panel. The simulator's `localhost:8910` reaches the Mac fixture;
it does not reach a fixture running on Linux.

The Rust server is a separate, memory-only development fixture. It never
calls a model, deployment, account, or Docker. Stop and restart it to reset.
It speaks the existing Fragment API shapes for lists, status, chat creation,
agent membership, channel posts, backwards history, and `__live` WebSockets.
The server is not part of the app binary or the root Cargo workspace.
App settings use the web API shapes too. Icon generation returns a deterministic
local candidate, previews use a local sample image, and invitations stay in memory;
no AI image service, browser capture, or email delivery runs in the fixture.

For a physical phone, explicitly bind the fixture to the LAN and set the
canonical URL to the Mac's Bonjour name (do this only on a trusted network;
the fixture has no authentication):

```sh
FRAGMENT_IOS_LISTEN=0.0.0.0:8910 FRAGMENT_IOS_BASE_URL=http://YOUR-MAC.local:8910 cargo run --locked --manifest-path ios/dev-server/Cargo.toml --target-dir target/ios-fixture
```

Set that same URL in the app's development settings. Loopback is the default.

## Scope

- Opens on the fullscreen search/new-chat launcher over the exact web desktop
  wallpaper, with system light/dark variants. System typography and iOS 26 glass
  controls throughout. Product identity is Finite.Computer (Finite for short).
- A left drawer follows horizontal dismissal drags, fading the backdrop with
  the distance and settling closed or back in place on release. Vertical chat
  scrolling stays independent, and settling respects Reduce Motion. It mirrors
  the web: agent menus and recent chats grouped by lead
  agent, group-chat faces, History, Your Apps, Settings. The launcher selects an agent;
  agent hero menus and chat-row context menus can add more agents, and a group composer addresses one or all.
- Your Apps is a native list over the shared photo background, with Add an app. Add opens the built-in app
  prompt sheet; choosing an idea returns a draft to the launcher.
  The top-right Manage Apps button opens a searchable list including hidden and
  archived apps; each visible app also has a settings button. App Settings covers
  naming, personal show/hide/archive, sharing links, member permissions and
  invitations, icon generation/upload/removal, and preview generation.
  No duplicate suggestions/empty state in the app library itself.
- Dismissible app sheets leave the conversation in place; a bounded in-memory
  session cache retains composer drafts, pending post ids and cursors across navigation.
- Native message author identities, Markdown, timestamps, long-press Reply/Copy/
  Share, quoted replies, file import, and AAC voice memos (five minutes maximum).
  Attachments use the existing blob API and chat record shape: eight per message,
  25 MiB per file. Download/share and audio playback are native.
- Connected accounts follow the signed list watcher and mark visible chats read.
- Existing HTTP/WebSocket streaming, Stop, same-id retry, independent cursors,
  backwards paging, reconnect and foreground resumption remain.
- History searches message contents and lists archived chats and read-only imports
  from the Session Archive's actual `index/chat.json` and lexical search contract.
- Native settings categories mirror web organization: account, billing, computer,
  agents, skills, connections, developer setup, data, and this iPhone. Local
  account/seat/computer/provider data, rename, restart, skills and JSON export are
  fixture-backed in local preview and use signed platform calls when connected.
  Account connection enrolls or revokes this iPhone’s own key; billing and OAuth
  setup continue on the web. No simulated purchases.
- Microphone and notification permission are native. Background reply delivery
  still requires production push integration; permission alone does not deliver it.

CLI-key authentication and private web-sheet redemption are implemented. Push delivery,
interactive approval cards, full Markdown tables, imported-chat continuation are separate integration work. The fixture spends nothing
and is memory-only. Connected-account drafts and exact pending sends survive relaunch;
message history is read again from the platform. Unknown records stay
readable; approval cards still direct you to the web. See [the integration contract](../docs/ios.md).

## Sample brain

Your Apps includes **Sample Brain**, a `brain` fragment with linked Markdown notes,
a table, folders, and a plain text checklist. It opens in the ordinary web sheet.
The fixture serves the unchanged `cell/files.mjs`, `cell/files.css`, and
`cell/markdown.mjs` sources; keep those files in a dedicated Mac build copy too.
The Files UI is real; its six files, word search, and index status are local
fixture data. No embedding or agent is called. Tap Files inside the page to
browse the tree on iPhone.

## Icon and composer

The installed icon is `Sources/AppIcon.icon`, a layered Icon Composer document.
Its transparent vector foreground preserves the hand paths in
`cell/shell/finite-logo.svg`, centered at 680 points on a 1024-point canvas over
an independent charcoal background. Corners, lighting and appearance variants
are compiled by Xcode rather than painted into the artwork, following
[Apple’s app icon guidance](https://developer.apple.com/design/human-interface-guidelines/app-icons).
XcodeGen 2.42 needs the `icon` file-type mapping in `project.yml` to keep the
package intact. Edit the document in Icon Composer and regenerate the project.

The chat composer has a single glass surface, 44-point controls and a 34-point
send/stop circle with semantic foreground/background colors. Chat addressing is
in the sidebar agent menu’s Agents in this chat → Messages setting, rather than
a caption above the composer. Your Apps displays each fragment’s original
`picture` (its versioned icon URL), preserving its transparency and aspect ratio.

## Verification

With the fixture running:

```sh
xcodebuild -project ios/Fragment.xcodeproj -scheme Fragment -destination 'platform=iOS Simulator,name=iPhone 17,OS=26.5' -derivedDataPath target/ios -parallel-testing-enabled NO test
cargo clippy --locked --manifest-path ios/dev-server/Cargo.toml --target-dir target/ios-fixture -- -D warnings
```

`ShellTests` cover multi-agent membership order/idempotence, session draft retention,
archive validation/replay, rename validation/deploy, and attachment upload/post replay.
`AppSettingsTests` cover personal hiding, sharing/link rotation, invitation renewal
and revocation, owner protection, viewer permissions, validated icon uploads and
selection, and preview completion/restrictions. The nine UI tests cover composer alignment/growth, the wallpaper launcher, search/new-chat/Stop, multi-agent
sidebar and membership, archived/imported history, app list and swipe-to-dismiss viewing,
retained composer drafts, settings, the Add prompt sheet’s launcher handoff, and app
management with hide/restore, icons, previews, invitations and viewer restrictions,
a brain’s web Files reader with folder navigation and linked notes, and sidebar
short-drag cancellation, full dismissal and reopening.

`TimelineTests` cover duplicate delivery, numeric ordering, independent
cursors, historical insertion, whole-text drafts, replacing only an author's
draft, multipart replay and turn completion. `TransportTests` use real HTTP
and WebSockets: stream, Stop, forced disconnection/catch-up, same-id replay
after a lost acknowledgement, conflicting body rejection, and exclusive
history cursors. `FragmentUITests` drive launcher → search/new chat → Stop, sidebar → multi-agent
chat/history/settings, and Your Apps → swipe-dismiss app sheet → retained chat draft in Simulator. `MarkdownTests` exercise a code
fence arriving incrementally. These tests require no paid calls.

Fixture prompts: `/disconnect` cuts the live socket once mid-reply;
`/fail-once` returns HTTP 503 **after committing** the message, so replay
must not append or run the agent twice; ask for `code` to exercise Markdown.
A matching live record acknowledges a post even when its HTTP response is
lost. To try the visible retry UI, disconnect the server while a send is in
flight, restart it and tap the failed post (the fixture resets on restart).

The source of the generated project is `project.yml`. If changing project
structure, use XcodeGen 2.42+ and commit its result:

```sh
xcodegen generate --spec ios/project.yml
```

## Design references

The second Finite iOS design is the retired `finitechat/ios/Sources/FocusedAppShell.swift`
at `finite-mono` commit `a1d5089c^`: focused home, three recents, one agent,
and chat selection. Its original brief is the Mac Codex archive for
`/Users/futurepaul/dev/finite/finite-mono`, session
`019f9006-c9e6-7f90-831e-7c6ae6f3ace7`, source JSONL line **9**, July 23,
2026 (shared Linux history archive; existing index, not refreshed).
The signed-in home now follows Fragment's own fullscreen Command-K launcher
(`cell/shell/shell.js`): search everything and default to starting a new chat,
per Paul's direction on 2026-10-14, replacing the older three-recents home.

[Enchanted](https://github.com/gluonfield/enchanted) supplies a native iOS
chat reference for conversation history, Markdown and a configurable server.
[Claude's iOS app](https://claude.com/resources/articles/team-plan-and-ios)
is a reference for a focused conversation-first mobile experience. No source
code was copied. SwiftUI's native scrolling follows
[Apple's scroll geometry API](https://developer.apple.com/documentation/swiftui/view/onscrollgeometrychange(for:of:action:)).
