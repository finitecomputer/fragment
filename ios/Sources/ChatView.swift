import SwiftUI
import UniformTypeIdentifiers

struct ChatView: View {
    @Environment(\.scenePhase) private var scenePhase
    @State private var session: ChatSession
    @State private var followsBottom = true
    @State private var scrollingByHand = false
    let library: Library
    @State private var importer = false
    @State private var uploading = false
    @State private var voice = VoiceMemo()

    init(session: ChatSession, library: Library) {
        _session = State(initialValue: session)
        self.library = library
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 24) {
                    if session.hasEarlier && !session.timeline.records.isEmpty {
                        Button("Earlier messages") { Task { await session.loadEarlier() } }
                            .font(.footnote).frame(maxWidth: .infinity).disabled(session.loadingHistory)
                    }
                    ForEach(session.timeline.records) { record in
                        if record.isMessage {
                            VStack(alignment: .leading, spacing: 8) {
                                MessageView(text: record.text, isYou: record.principal == session.principal, agent: library.agent(for: record.principal))
                                    .contextMenu {
                                        Button("Reply", systemImage: "arrowshape.turn.up.left") { session.replyTo = record }
                                        Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = record.text }
                                        ShareLink(item: record.text) { Label("Share", systemImage: "square.and.arrow.up") }
                                    }
                                ForEach(attachments(record)) { attachment in
                                    AttachmentView(attachment: attachment, api: session.api, fragment: session.fragment.name)
                                }
                                Text(Date(timeIntervalSince1970: Double(record.at) / 1000), style: .time).font(.caption2).foregroundStyle(.tertiary)
                            }.id(record.id)
                        } else if record.channel == "work" {
                            WorkRecordView(record: record)
                        }
                    }
                    ForEach(session.pending) { pending in
                        VStack(alignment: .trailing, spacing: 6) {
                            MessageView(text: pending.text, isYou: true)
                            if pending.failed {
                                Button("Not sent. Tap to retry", systemImage: "arrow.clockwise") {
                                    Task { await session.retry(pending) }
                                }.font(.caption).accessibilityIdentifier("retry")
                            } else { Text("Sending…").font(.caption).foregroundStyle(.secondary) }
                        }.frame(maxWidth: .infinity, alignment: .trailing)
                    }
                    ForEach(session.timeline.drafts.values.sorted { $0.at < $1.at }) { draft in
                        MessageView(text: draft.text, isYou: false, agent: library.agent(for: draft.principal), streaming: true)
                    }
                    if !session.timeline.activeTurns.isEmpty && session.timeline.drafts.isEmpty {
                        Label("Working…", systemImage: "ellipsis").font(.callout).foregroundStyle(.secondary)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .frame(maxWidth: 660, alignment: .leading)
                .padding(.horizontal, 20).padding(.vertical, 24).frame(maxWidth: .infinity)
            }
            .scrollDismissesKeyboard(.interactively)
            .accessibilityIdentifier("timeline")
            .defaultScrollAnchor(.bottom, for: .initialOffset)
            .defaultScrollAnchor(.top, for: .alignment)
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentSize.height - geometry.visibleRect.maxY < 100
            } action: { _, nearBottom in
                if scrollingByHand { followsBottom = nearBottom }
            }
            .onScrollPhaseChange { _, phase in
                scrollingByHand = phase == .interacting || phase == .decelerating
            }
            .onChange(of: session.timeline.records.count + session.pending.count) { _, _ in
                if followsBottom { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onChange(of: session.timeline.drafts) { _, _ in
                if followsBottom { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .overlay(alignment: .bottomTrailing) {
                if !followsBottom {
                    Button("Latest messages", systemImage: "arrow.down") {
                        followsBottom = true
                        withAnimation { proxy.scrollTo("bottom", anchor: .bottom) }
                    }
                    .labelStyle(.iconOnly).buttonStyle(.borderedProminent).buttonBorderShape(.circle).padding()
                }
            }
            .overlay {
                if session.timeline.records.isEmpty && session.pending.isEmpty {
                    ContentUnavailableView("A fresh start", systemImage: "bubble.left.and.bubble.right",
                        description: Text("A new conversation with your agent."))
                }
            }
        }
        .background(FragmentStyle.background)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                if let error = session.error {
                    Text(error).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 20).padding(.top, 8)
                }
                composerAccessories
                ComposerView(text: $session.composer, working: !session.timeline.activeTurns.isEmpty,
                    enabled: session.connected && session.caughtUp, busy: uploading, recording: voice.recording,
                    attach: { importer = true }, record: { Task { await recordVoice() } },
                    send: { await session.send() }, stop: { await session.stop() }, hasAttachments: !session.attachments.isEmpty)
            }.background(FragmentStyle.background)
        }
        .fileImporter(isPresented: $importer, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls): Task { await uploadFiles(urls) }
            case .failure(let error): session.error = error.localizedDescription
            }
        }
        .task(id: "\(session.caughtUp)|\(session.timeline.cursors["chat"] ?? 0)|\(scenePhase)") {
            guard scenePhase == .active, session.caughtUp, session.api.isSigned else { return }
            do { let _: JSONValue = try await session.api.request("api/fragments/\(session.fragment.name)/seen", method: "PUT") }
            catch { /* A later live cursor or foreground visit retries the read marker. */ }
        }
        .onDisappear { session.flushDraft() }
        .onChange(of: scenePhase) { _, phase in if phase != .active { session.flushDraft() } }
        .task(id: scenePhase) {
            guard scenePhase == .active else { voice.cancel(); return }
            await session.run()
        }
        .onDisappear { voice.cancel() }
    }

    private var composerAccessories: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let quoted = session.replyTo {
                HStack { Label(quoted.text, systemImage: "arrowshape.turn.up.left").font(.caption).lineLimit(2); Spacer(); Button("Cancel reply", systemImage: "xmark") { session.replyTo = nil }.labelStyle(.iconOnly) }
            }
            if !session.attachments.isEmpty {
                ScrollView(.horizontal) { HStack {
                    ForEach(session.attachments) { file in
                        Button { session.attachments.removeAll { $0.id == file.id } } label: { Label(file.name, systemImage: "xmark.circle").font(.caption) }.buttonStyle(.bordered)
                    }
                } }
            }
            if let error = voice.error { Text(error).font(.caption).foregroundStyle(.secondary) }
            if voice.recording { Label("Recording voice memo…", systemImage: "waveform").foregroundStyle(.red).font(.caption) }
        }.padding(.horizontal, 20)
    }

    private func attachments(_ record: ChannelRecord) -> [ChatAttachment] {
        record.body["attachments"].array.compactMap { value in
            guard let data = try? JSONEncoder().encode(value) else { return nil }
            return try? JSONDecoder().decode(ChatAttachment.self, from: data)
        }
    }
    private func uploadFiles(_ urls: [URL]) async {
        guard urls.count + session.attachments.count <= 8 else { session.error = "Attach at most eight files per message."; return }
        uploading = true; defer { uploading = false }
        for url in urls {
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            do {
                let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                guard size > 0, size <= 25 * 1_024 * 1_024 else { throw APIError.server("Files must be between 1 byte and 25 MB.") }
                let type = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
                let file = try await session.api.upload(Data(contentsOf: url), name: url.lastPathComponent, type: type, to: session.fragment.name)
                if !session.attachments.contains(where: { $0.id == file.id }) { session.attachments.append(file) }
            } catch { session.error = error.localizedDescription; return }
        }
    }
    private func recordVoice() async {
        if voice.recording {
            if let url = voice.stop() { await uploadFiles([url]); try? FileManager.default.removeItem(at: url) }
        } else {
            guard session.attachments.count < 8 else { session.error = "Attach at most eight files per message."; return }
            await voice.start { url in Task { await uploadFiles([url]); try? FileManager.default.removeItem(at: url) } }
        }
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
                        if streaming { ProgressView().controlSize(.mini).accessibilityLabel("Replying") }
                    }
                }
                if isYou {
                    Text(text).font(.body).lineSpacing(5).textSelection(.enabled)
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
