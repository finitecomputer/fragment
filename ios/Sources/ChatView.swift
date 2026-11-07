import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

struct ChatView: View {
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var session: ChatSession
    @State private var followsBottom = true
    @State private var scrollingByHand = false
    /// A jump to the latest under way: scrolling is held off for it, which
    /// stops a fling's momentum. Momentum ran on past a jump made while it
    /// lasted, so the tap only hid the button.
    @State private var jumping = false
    /// The row the chat keeps in place as pages come in above it.
    @State private var anchor: String?
    /// Messages heard (`ChatSession.heard`) when it last left the newest.
    @State private var heardAtLeave = 0
    let library: Library
    /// Opens a fragment of the person's (a chat, or an app at a page).
    let open: (FragmentSummary) -> Void

    init(session: ChatSession, library: Library, open: @escaping (FragmentSummary) -> Void) {
        _session = State(initialValue: session)
        self.library = library
        self.open = open
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                // Each part observes what it draws alone: a draft's next words
                // redraw the draft, a new record the rows (each an equatable
                // `ChatRow`, drawn again only when it changes), and the
                // stack draws only the rows on screen.
                LazyVStack(alignment: .leading, spacing: 24) {
                    // the top reached: the page before it (FIN-415's sentinel)
                    Color.clear.frame(height: 1)
                        // (again once a page is in, if the top is still in view)
                        .task(id: "\(session.oldestMessage ?? "")|\(session.paging)") { if session.read && !session.paging { session.page(.older) } }
                    ChatRows(session: session, library: library, open: open)
                    PendingRows(session: session)
                    WorkingLine(session: session, library: library)
                    // the end of a window paged back reached: the page after it
                    Color.clear.frame(height: 1).id("bottom")
                        .task(id: "\(session.rows.last?.id ?? "")|\(session.paging)") { if session.detached && !session.paging { session.page(.newer) } }
                }
                .scrollTargetLayout()
                .frame(maxWidth: 660, alignment: .leading)
                .padding(.horizontal, 20).padding(.vertical, 24).frame(maxWidth: .infinity)
                // its backlog is laid out unseen, and shown once read, at its end
                .opacity(session.read ? 1 : 0)
            }
            // rows read in above keep the one shown in place
            .scrollPosition(id: $anchor, anchor: .top)
            .scrollDismissesKeyboard(.interactively)
            .accessibilityIdentifier("timeline")
            .defaultScrollAnchor(.bottom, for: .initialOffset)
            .defaultScrollAnchor(.top, for: .alignment)
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentSize.height - geometry.visibleRect.maxY < 100
            } action: { _, nearBottom in
                if scrollingByHand && !jumping { followsBottom = nearBottom }
            }
            // Following the latest, the bottom stays pinned as the content
            // grows: rows the lazy stack drew taller than its guess (a reveal
            // landed short of the newest messages), a reply streaming in.
            .onScrollGeometryChange(for: CGFloat.self) { geometry in
                geometry.contentSize.height
            } action: { old, new in
                guard followsBottom, !scrollingByHand, !jumping, session.read,
                      !session.detached, !session.paging, new > old else { return }
                // Follow measured growth, not each token. Only the scroll
                // offset animates; initial positioning and Latest stay instant.
                let motion: Animation? = !reduceMotion && !session.drafts.isEmpty
                    ? .easeOut(duration: 0.16) : nil
                withAnimation(motion) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onScrollPhaseChange { _, phase in
                scrollingByHand = phase == .tracking || phase == .interacting || phase == .decelerating
            }
            // the newest record, never a count: at the window's bound a new
            // message leaves the count as it was (one in, the oldest out), and
            // an older page read changes the count but not the newest
            .onChange(of: [session.rows.last?.id, session.pending.last?.id]) { _, _ in
                if followsBottom && !scrollingByHand && !jumping && !session.detached && session.read { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onChange(of: session.read) { _, read in
                if read { followsBottom = true; proxy.scrollTo("bottom", anchor: .bottom) }
            }
            // Drafts follow through measured height above: a token that adds
            // no line must not restart (or snap) the in-flight follow scroll.
            .scrollDisabled(jumping)
            // once scrolling is held off (the fling stopped), the jump lands
            .onChange(of: followsBottom) { _, following in
                if !following { heardAtLeave = session.heard }
            }
            .onChange(of: session.detached) { _, detached in
                if detached { followsBottom = false }
            }
            .onChange(of: jumping) { _, now in
                guard now else { return }
                scrollingByHand = false
                proxy.scrollTo("bottom", anchor: .bottom)
                jumping = false
            }
            .animation(reduceMotion ? nil : .easeIn(duration: 0.2), value: session.read)
            .overlay(alignment: .bottomTrailing) {
                if (!followsBottom || session.detached) && session.read {
                    // At once, not animated: an animated scroll across rows the
                    // lazy stack has not drawn lands short of the end. A window
                    // paged back is the chat's newest page again first.
                    let fresh = session.heard - heardAtLeave
                    Button(fresh > 0 ? "\(fresh) new" : "Latest messages", systemImage: "arrow.down") {
                        Task {
                            if session.detached { await session.resetToLatest() }
                            followsBottom = true
                            jumping = true
                        }
                    }
                    .labelStyle(LatestLabel(counted: fresh > 0)).buttonStyle(.borderedProminent).buttonBorderShape(fresh > 0 ? .capsule : .circle).padding()
                    .accessibilityIdentifier("latestMessages")
                }
            }
            .overlay { ChatEmptyState(session: session) }
        }
        // A tapped link: a page of a fragment of theirs opens in the app, as the
        // web's Open opens it beside the chat; any other, in Safari.
        .environment(\.openURL, OpenURLAction { url in
            guard let fragment = library.fragment(linkedBy: url) else { return .systemAction }
            open(fragment)
            return .handled
        })
        .background(FragmentStyle.background)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                if let error = session.error {
                    Text(error).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 20).padding(.top, 8)
                }
                ComposerBar(text: Binding(get: { session.composer }, set: { session.edit($0) }), working: !session.activeTurns.isEmpty,
                    // sends go over HTTP: a reconnect never holds the composer
                    enabled: session.read, busy: session.attaching,
                    placeholder: session.placeholder(name: { library.agent(for: $0)?.displayTitle ?? "The agent" }),
                    attached: session.attachments.map { ($0.id, $0.name) },
                    remove: { id in session.attachments.removeAll { $0.id == id } },
                    // each file from its bytes here, an upload with its progress and Retry
                    files: AnyView(ComposerAttachments(session: session)),
                    // uploaded at once, one attaching at a time (`beginAttaching`)
                    attach: { count, read in
                        guard session.beginAttaching(count) else { return false }
                        Task { await session.attach(read) }
                        return true
                    },
                    send: { await session.send() }, stop: { await session.stop() },
                    steer: { recording in session.steers(recording: recording) ? { await session.steer(recording: recording) } : nil },
                    quote: session.replyTo.map { quoted in (quoted.text, { session.replyTo = nil }) })
            }.background(FragmentStyle.background)
        }
        // Seen only once its newest messages are on screen: read, the window
        // the chat's own (not paged back), following its end; and again on
        // coming back there. Caught up on the socket is not seen.
        .task(id: "\(session.caughtUp)|\(session.chatCursor)|\(scenePhase)|\(session.read)|\(session.detached)|\(followsBottom)") {
            guard scenePhase == .active, session.caughtUp, session.read, !session.detached, followsBottom else { return }
            do { let _: JSONValue = try await session.api.request("api/fragments/\(session.fragment.name)/seen", method: "PUT") }
            catch { /* A later live cursor or foreground visit retries the read marker. */ }
        }
        .onDisappear { session.flushDraft() }
        // the person's own agents: whose menus they may use, whose routines are theirs
        .onChange(of: library.ownedIdentities, initial: true) { _, owned in session.owned = owned }
        // and the words that @mention the agents they know, for whom a message is
        .onChange(of: library.handles, initial: true) { _, handles in session.handles = handles }
        .onChange(of: scenePhase) { _, phase in if phase != .active { session.flushDraft() } }
        .task(id: scenePhase) {
            guard scenePhase == .active else { AudioPlayback.shared.pause(); return }
            await session.run()
        }
        // a sound playing stops with its chat (a voice memo being recorded
        // stops with its ComposerBar)
        .onDisappear { AudioPlayback.shared.stop() }
    }
}

/// The Latest pill: its arrow alone, or with how many messages came.
private struct LatestLabel: LabelStyle {
    let counted: Bool
    func makeBody(configuration: Configuration) -> some View {
        if counted { HStack(spacing: 6) { configuration.icon; configuration.title }.font(.footnote.weight(.semibold)) }
        else { configuration.icon }
    }
}

private struct ChatRows: View {
    let session: ChatSession
    let library: Library
    let open: (FragmentSummary) -> Void
    var body: some View {
        let faces = library.agentsByIdentity
        let you = session.principal
        ForEach(session.rows) { item in
            switch item {
            case .record(let record):
                let links = record.isMessage ? MessageLinks.find(in: record.text, markdown: record.principal != you) : []
                ChatRow(record: record, isYou: record.principal == you, agent: faces[record.principal], card: session.activity.cards[record.id],
                        queued: session.activity.queued.contains(record.id),
                        links: links, opens: links.compactMap(library.fragment(linkedBy:)).reduce(into: []) { kept, app in if !kept.contains(where: { $0.name == app.name }) { kept.append(app) } },
                        api: session.api, fragment: session.fragment.name, reply: { session.replyTo = $0 }, open: open)
                    .equatable()
            case .draft(let key):
                DraftRow(session: session, library: library, key: key)
            case .hint(let hint):
                HintRow(session: session, library: library, hint: hint)
            }
        }
    }
}

/// One record of a chat as shown: drawn again only when what it shows
/// changes (its record, whose it is, its agent's name and face).
struct ChatRow: View, Equatable {
    let record: ChannelRecord
    let isYou: Bool
    let agent: FragmentSummary?
    var card: ToolCard? = nil
    /// A person's message waiting for its agent's running turn (chat.js `queued`).
    var queued = false
    /// The web addresses in its words, and the fragments of theirs among
    /// them, each with an Open of its own, as the web's chat gives one.
    var links: [URL] = []
    var opens: [FragmentSummary] = []
    let api: FragmentAPI
    let fragment: String
    let reply: (ChannelRecord) -> Void
    var open: (FragmentSummary) -> Void = { _ in }

    nonisolated static func == (a: ChatRow, b: ChatRow) -> Bool {
        a.record == b.record && a.isYou == b.isYou && a.agent == b.agent && a.fragment == b.fragment && a.card == b.card
            && a.queued == b.queued && a.links == b.links && a.opens == b.opens
    }

    var body: some View {
        if record.isMessage {
            let files = attachments
            VStack(alignment: .leading, spacing: 8) {
                // a message of files alone is its files, without an empty bubble
                if !(isYou && files.count > 0 && record.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty) {
                    MessageView(text: record.text, isYou: isYou, agent: agent)
                        .contextMenu {
                            Button("Reply", systemImage: "arrowshape.turn.up.left") { reply(record) }
                            Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = record.text }
                            ShareLink(item: record.text) { Label("Share", systemImage: "square.and.arrow.up") }
                            if links.count == 1, let link = links.first {
                                Button("Copy link", systemImage: "link") { UIPasteboard.general.url = link }
                                ShareLink(item: link) { Label("Share link", systemImage: "square.and.arrow.up.on.square") }
                            } else if !links.isEmpty {
                                Menu("Links", systemImage: "link") {
                                    ForEach(links, id: \.self) { link in
                                        Menu(link.host() ?? link.absoluteString) {
                                            Button("Copy link", systemImage: "link") { UIPasteboard.general.url = link }
                                            ShareLink(item: link) { Label("Share link", systemImage: "square.and.arrow.up.on.square") }
                                        }
                                    }
                                }
                            }
                        }
                }
                if !opens.isEmpty {
                    HStack(spacing: 8) {
                        ForEach(opens) { app in
                            Button { open(app) } label: {
                                Label("Open \(app.displayTitle)", systemImage: "arrow.up.forward.app").font(.footnote.weight(.semibold)).lineLimit(1)
                            }
                            .buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.small)
                            .accessibilityIdentifier("openLinked-\(app.name)")
                        }
                    }
                }
                if !files.isEmpty { MessageAttachments(attachments: files, isYou: isYou, api: api, fragment: fragment) }
                HStack(spacing: 8) {
                    // its agent answers it after the turn it is in now
                    if queued { Label("Queued", systemImage: "clock").font(.caption2).foregroundStyle(.secondary).accessibilityIdentifier("queued") }
                    Text(Date(timeIntervalSince1970: Double(record.at) / 1000), style: .time).font(.caption2).foregroundStyle(.tertiary)
                }
            }
        } else if let command = record.command {
            // a command, as it was typed: its name, then its words (chat.js `userMessage`)
            VStack(alignment: .leading, spacing: 8) {
                CommandView(command: command, args: record.body["args"].string ?? "", isYou: isYou)
                    .frame(maxWidth: .infinity, alignment: isYou ? .trailing : .leading)
                Text(Date(timeIntervalSince1970: Double(record.at) / 1000), style: .time).font(.caption2).foregroundStyle(.tertiary)
            }
        } else if let card {
            VStack(alignment: .leading, spacing: 20) {
                if let words = card.words { MessageView(text: words, isYou: false, agent: agent) }
                WorkingIndicator(card: card, agent: agent)
            }
        } else {
            WorkRecordView(record: record)
        }
    }
    private var attachments: [ChatAttachment] {
        record.body["attachments"].array.compactMap { value in
            guard let data = try? JSONEncoder().encode(value) else { return nil }
            return try? JSONDecoder().decode(ChatAttachment.self, from: data)
        }
    }
}

private struct PendingRows: View {
    let session: ChatSession
    var body: some View {
        ForEach(session.pending) { pending in
            VStack(alignment: .trailing, spacing: 6) {
                if let command = pending.command { CommandView(command: command, args: pending.text, isYou: true) }
                else if !pending.text.isEmpty || pending.attachments.isEmpty { MessageView(text: pending.text, isYou: true) }
                // its files from their bytes on this iPhone, as they will show once sent
                if !pending.attachments.isEmpty {
                    MessageAttachments(attachments: pending.attachments, isYou: true, api: session.api, fragment: session.fragment.name)
                }
                if let refused = pending.refused {
                    Button("Not sent: \(refused) Tap to edit", systemImage: "pencil") {
                        session.reopen(pending)
                    }.font(.caption).accessibilityIdentifier("editRefused")
                } else if pending.failed {
                    Button("Not sent. Tap to retry", systemImage: "arrow.clockwise") {
                        Task { await session.retry(pending) }
                    }.font(.caption).accessibilityIdentifier("retry")
                } else { Text("Sending…").font(.caption).foregroundStyle(.secondary) }
            }.frame(maxWidth: .infinity, alignment: .trailing)
        }
    }
}

/// A reply being written, in its place (after its turn's latest record):
/// only it observes the draft's words, so they redraw it alone.
private struct DraftRow: View {
    let session: ChatSession
    let library: Library
    let key: TurnKey
    var body: some View {
        if let draft = session.drafts.first(where: { $0.turn == key.id && $0.principal == key.agent }) {
            MessageView(text: draft.text, isYou: false, agent: library.agent(for: draft.principal), streaming: true)
        }
    }
}

/// A running turn's working line (or its question's), in its place.
private struct HintRow: View {
    let session: ChatSession
    let library: Library
    let hint: WorkingHint
    var body: some View {
        WorkingIndicator(hints: [hint], library: library, principal: session.principal,
                         phase: library.computerPhase, present: Set(session.presence.values))
    }
}

/// A post awaiting its agent's claim, at the chat's end.
private struct WorkingLine: View {
    let session: ChatSession
    let library: Library
    var body: some View {
        WorkingIndicator(hints: session.workingHints, library: library, principal: session.principal,
                         phase: library.computerPhase, present: Set(session.presence.values))
    }
}

/// A person's command for an agent's runtime, as typed: `/name words`.
struct CommandView: View {
    let command: String
    let args: String
    let isYou: Bool
    var body: some View {
        Text("\(Text("/\(command)").font(.body.monospaced()).foregroundStyle(.secondary))\(args.isEmpty ? "" : " " + args)")
            .lineSpacing(5).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            .padding(14)
            .background(FragmentStyle.bubble, in: RoundedRectangle(cornerRadius: 20))
            .padding(.leading, isYou ? 44 : 0)
            .accessibilityIdentifier("command")
    }
}

/// Until the chat is first read to its end, it is loading: never shown as a
/// chat with nothing in it, which it may not be, nor its backlog streaming
/// in. Read and empty, it is a fresh start.
private struct ChatEmptyState: View {
    let session: ChatSession
    var body: some View {
        let empty = session.rows.isEmpty && session.pending.isEmpty && session.drafts.isEmpty && session.activeTurns.isEmpty
        Group {
            if !session.read { ChatLoadingView() }
            else if empty {
                ContentUnavailableView("A fresh start", systemImage: "bubble.left.and.bubble.right",
                    description: Text("A new conversation with your agent."))
            }
        }
        .animation(.easeInOut(duration: 0.25), value: session.read)
    }
}

/// A chat on its way: a quiet spinner, faded in.
struct ChatLoadingView: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false
    var body: some View {
        ProgressView()
            .controlSize(.large)
            .opacity(shown ? 1 : 0)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .onAppear { withAnimation(reduceMotion ? nil : .easeIn(duration: 0.4).delay(0.15)) { shown = true } }
            .accessibilityLabel("Loading this chat")
            .accessibilityIdentifier("chatLoading")
    }
}

struct MessageView: View {
    let text: String
    let isYou: Bool
    var agent: FragmentSummary? = nil
    var streaming = false

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            if isYou { Spacer(minLength: 44) }
            VStack(alignment: .leading, spacing: 8) {
                if !isYou {
                    HStack(spacing: 7) {
                        AgentAvatar(agent: agent, size: 22)
                        Text(agent?.displayTitle ?? "Agent").font(.caption.weight(.medium)).foregroundStyle(.secondary)
                        // drawn by SwiftUI, never a UIKit spinner: a platform view
                        // in the lazy stack's rows was in each layout pass of a
                        // stuck one (100% CPU) seen while scrolling up mid-reply
                        if streaming {
                            Image(systemName: "ellipsis").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                                .symbolEffect(.variableColor.iterative, options: .repeating)
                                .accessibilityLabel("Replying")
                        }
                    }
                }
                if isYou {
                    Text(MessageLinks.linked(AttributedString(text))).font(.body).lineSpacing(5).textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                } else { MarkdownText(text: text) }
            }
            .padding(isYou ? 14 : 0)
            .background(isYou ? FragmentStyle.bubble : .clear, in: RoundedRectangle(cornerRadius: 20))
            if !isYou { Spacer(minLength: 8) }
        }
        .frame(maxWidth: .infinity, alignment: isYou ? .trailing : .leading)
        .accessibilityElement(children: .combine)
    }
}

struct WorkRecordView: View {
    let record: ChannelRecord
    var body: some View {
        if record.body["kind"].string == "turn.step" {
            DisclosureGroup {
                Text(record.body["args"].string ?? "").font(.caption.monospaced()).textSelection(.enabled)
                Text(record.body["excerpt"].string ?? "").font(.caption).textSelection(.enabled)
            } label: {
                Label(record.body["tool"].string ?? "Working", systemImage: "checkmark.circle")
                    .font(.callout).foregroundStyle(.secondary)
            }
            .padding(14).background(FragmentStyle.surface, in: RoundedRectangle(cornerRadius: 14))
        } else if record.body["kind"].string == "turn.end", let error = record.body["error"].string {
            Label(error, systemImage: "exclamationmark.circle").font(.callout).foregroundStyle(.secondary)
        } else if record.body["kind"].string == "turn.end", record.body["status"].string == "stopped" {
            Text("Response stopped").font(.footnote).foregroundStyle(.secondary)
        } else if record.body["kind"].string == "turn.prompt" {
            Label("\(record.text)\nOpen this chat on the web to answer this request.", systemImage: "questionmark.bubble")
                .font(.callout).foregroundStyle(.secondary)
        }
    }
}
