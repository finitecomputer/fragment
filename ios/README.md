# Finite for iOS

A native SwiftUI client for Finite.Computer, organized like the web shell. iOS 26+,
Swift 6, Apple frameworks and pinned P256K 0.21.1 for CLI-compatible NIP-98 signing.
No Rust bindings, shared runtime, or web chat UI. App fragments and imported
conversations open in a native WKWebView sheet. Every request is signed by this
iPhone's own approved key: there is no unsigned or offline mode.

## Connect an account

Enter your Finite platform’s HTTPS address and tap Connect. Enter the phone’s
approval code at that server’s `/cli` page and approve the Finite-iOS request:
sign-in stays the website’s WorkOS flow. The phone keeps its own approved key in
Keychain; never import the CLI’s private key. Settings → Account connection
revokes and disconnects this phone. If its server is unreachable, Remove from
this iPhone clears the saved connection locally; the key still needs revoking on
the web. Private apps and brains open through `POST /api/f/{name}/session`. See
[the integration contract](../docs/ios.md).

## Run against a local platform

The app talks to a real platform, never a stand-in. For development that is
`cargo xtask dev` (the repository's dev stack, with its vendor fakes) on the Linux
machine at `:8790`. Forward the port to the Mac, so the Simulator's loopback
reaches it; macOS resolves `<name>.fragment.localhost` to loopback too, so app
sheets load:

```sh
ssh -N -L 8790:127.0.0.1:8790 linux   # on the Mac, while you work
open ios/Fragment.xcodeproj
```

Choose the **Fragment** scheme and an iPhone Simulator, then Run. Debug builds
take HTTP loopback and `.localhost` origins; `--pair-server http://127.0.0.1:8790`
fills in Connect. Approve the code at `http://127.0.0.1:8790/cli`, signed in
through the dev stack's WorkOS fake. The checked-in Xcode project needs no
generator; Xcode resolves the pinned signing package.

From a terminal:

```sh
xcrun simctl list devices available
FRAGMENT_IOS_SIMULATOR='paste-an-iOS-26.5-iPhone-udid-here'
xcodebuild -project ios/Fragment.xcodeproj -scheme Fragment -configuration Debug -destination "platform=iOS Simulator,id=$FRAGMENT_IOS_SIMULATOR" -derivedDataPath target/ios build
xcrun simctl boot "$FRAGMENT_IOS_SIMULATOR"   # if it is not booted
xcrun simctl install "$FRAGMENT_IOS_SIMULATOR" target/ios/Build/Products/Debug-iphonesimulator/Fragment.app
xcrun simctl launch "$FRAGMENT_IOS_SIMULATOR" build.finite.fragment --pair-server http://127.0.0.1:8790
open -a Simulator
```

A physical phone reaches a platform over HTTPS; Release builds take nothing else.

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
  25 MiB per file. Pictures show inline and full screen, sounds as players (every
  format that occurs: Chrome's WebM rewrapped as Ogg on the phone), PDFs as their
  first page, other files as rows opened in Quick Look; see docs/ios.md.
- Connected accounts follow the signed list watcher and mark visible chats read.
- Existing HTTP/WebSocket streaming, Stop, same-id retry, independent cursors,
  backwards paging, reconnect and foreground resumption remain.
- History searches message contents and lists archived chats and read-only imports
  from the Session Archive's actual `index/chat.json` and lexical search contract.
- Native settings categories mirror web organization: account, billing, computer,
  agents, skills, connections, developer setup, data, and this iPhone, all read
  and written through signed platform calls. Account connection enrolls or
  revokes this iPhone’s own key; billing, OAuth setup and renaming an agent
  continue on the web (an agent's name is its job's too). No simulated purchases.
- Microphone and notification permission are native. Background reply delivery
  still requires production push integration; permission alone does not deliver it.

Push delivery, interactive approval cards, full Markdown tables and imported-chat
continuation are separate work. Drafts and exact pending sends survive relaunch;
message history is read again from the platform. Unknown records stay readable;
approval cards still direct you to the web.

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

```sh
xcodebuild -project ios/Fragment.xcodeproj -scheme Fragment -destination 'platform=iOS Simulator,name=iPhone 17,OS=26.5' -derivedDataPath target/ios test
```

The unit tests need no server and spend nothing. `TransportTests`,
`ShellTests`, `SearchTests` and `AppSettingsTests` run the app's real HTTP and
live-socket code against a stub of the platform's answers (`Tests/Stub.swift`:
a `URLProtocol` for HTTP, and a scripted `LiveSocket`, since `URLProtocol` never
sees a WebSocket). They cover what a phone meets: an acknowledgement lost after
the post committed (retried with the same id and body), a refusal (final: the
post leaves the outbox and its words return to the composer), a rate limit
(kept to retry), a dropped stream (resubscribed from its cursor, nothing twice),
Stop, a long turn whose start is past the backlog (read back, so Stop shows),
earlier pages that never move the live cursor, an ended authorization, a web
sheet that never reopens a spent redemption, an archive with no chats index or
a failing read, an export built then downloaded signed, and the requests behind
lists, joins, renames, archiving, attachments, search, sharing and icons. `TimelineTests` cover ordering, cursors, drafts and turns;
`DeviceIdentityTests` NIP-98 binding, origins and Keychain; `DraftTests` the
saved draft and outbox across restarts (a paste past a message, a refused send
beside a draft, a saved file past its bounds or unreadable);
`MarkdownTests` a code fence arriving in parts. The stubs' answers are written
by hand from docs/api.md.

`FragmentUITests` drive what needs no server: Connect, an unreachable server,
and a server past loopback refused. Each launches with `--ui-test` (Debug only):
a Keychain service and Application Support folder of its own, emptied as it
starts, and loopback alone, so a simulator's own account is never read and no
other server is ever asked (`Sources/LaunchScope.swift`;
`--ui-test-saved-account <origin>` seeds one, to show it unused). Flows against
a paired account on the dev stack are their own suite.

The source of the generated project is `project.yml`. If changing project
structure, use XcodeGen 2.42+ and commit its result:

```sh
xcodegen generate --spec ios/project.yml
```

## Design references

The signed-in home follows Fragment's own fullscreen Command-K launcher
(`cell/shell/shell.js`): search everything and default to starting a new chat,
replacing an older three-recents home (`finitechat/ios/Sources/FocusedAppShell.swift`
at `finite-mono` commit `a1d5089c^`).

[Enchanted](https://github.com/gluonfield/enchanted) supplies a native iOS
chat reference for conversation history, Markdown and a configurable server.
[Claude's iOS app](https://claude.com/resources/articles/team-plan-and-ios)
is a reference for a focused conversation-first mobile experience. No source
code was copied. SwiftUI's native scrolling follows
[Apple's scroll geometry API](https://developer.apple.com/documentation/swiftui/view/onscrollgeometrychange(for:of:action:)).
