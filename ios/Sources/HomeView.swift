import AVFoundation
import PhotosUI
import SwiftUI
import Synchronization
import UniformTypeIdentifiers

/// What the home screen's prompt holds: its words and the files of the
/// first message, read and kept here until its chat is made (a chat's blobs
/// are its own: launcher.js `launchFiles`), and whether a voice memo is
/// being recorded. Kept by the `Library`, so leaving Home, or a launch that
/// fails, loses none of it; nothing keeps the files past a relaunch, as on
/// the web. While a launch sends what was submitted, it takes no edits,
/// files or removals (`launching`): what is sent is what was submitted.
@MainActor @Observable
final class HomeDraft {
    struct Held: Identifiable, Equatable, Sendable {
        let id: UUID
        let file: OutgoingFile
    }
    /// What is typed; while a launch sends what was submitted, it stays as submitted.
    var text: String {
        get { typed }
        set { if !launching { typed = newValue } }
    }
    private var typed = ""
    private(set) var files: [Held] = []
    private(set) var reading = false
    /// The reading under way, to wait for.
    @ObservationIgnored private(set) var holding: Task<Void, Never>?
    /// A voice memo being recorded in the prompt (`ComposerBar`): nothing
    /// launches until it is finished and attached.
    var recording = false
    var error: String?
    private(set) var launching = false
    var words: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    /// Whether what is held may start a chat now: one rule for every way to
    /// start one (Send, a finished memo).
    var ready: Bool { (!words.isEmpty || !files.isEmpty) && text.utf8.count <= 32_768 && !launching && !reading && !recording }

    /// Places the first message has left for files, of what a message carries.
    var room: Int { max(0, ChatAttachment.countMax - files.count) }
    /// Files for the first message, read now and held: at most what a
    /// message carries, one reading at a time, none while a launch sends.
    /// A voice memo's place is held from before it records (`VoiceMemo.file`).
    func hold(_ count: Int, _ read: @escaping @Sendable () async throws -> [OutgoingFile]) -> Bool {
        guard !launching else { error = "Wait for the message being sent."; return false }
        guard !reading else { error = "Wait for the files being attached, then add more."; return false }
        guard count > 0, count <= room else { error = ChatAttachment.limits.countRefusal; return false }
        reading = true
        holding = Task {
            defer { reading = false }
            do {
                let read = try await read()
                guard !read.isEmpty else { error = "This could not be attached."; return }
                guard read.count <= room else { error = ChatAttachment.limits.countRefusal; return }
                files += read.map { Held(id: UUID(), file: $0) }
                error = nil
            }
            // a voice memo cancelled, or never begun: nothing to say
            catch is CancellationError {}
            catch { self.error = error.localizedDescription }
        }
        return true
    }
    func remove(_ id: UUID) { guard !launching else { return }; files.removeAll { $0.id == id } }

    /// The words and files a launch sends, as they are now; the prompt
    /// locked until it ends (`ended`).
    func begin() -> (text: String, files: [Held]) { launching = true; return (text, files) }
    /// A launch over: once sent, what it sent goes (later edits, had there
    /// been any, stay); not sent, all of it stays to send again.
    func ended(sent: (text: String, files: [Held])?) {
        launching = false
        guard let sent else { return }
        files.removeAll { held in sent.files.contains { $0.id == held.id } }
        if typed == sent.text { typed = "" }
        error = nil
    }
}

private extension View {
    /// The launcher's primary action, the top bar's size. Glass doesn't refract
    /// glass, so on the card the system's goes flat: this one draws what light
    /// does in a glass bead, a tight gloss above and light pooled at the foot
    /// behind the icon, and a rim and shadow around it.
    func launcherGlass(tint: Color = Color(uiColor: .label)) -> some View {
        foregroundStyle(Color(uiColor: .systemBackground)).fontWeight(.semibold)
            .frame(width: 44, height: 44).contentShape(.circle)
            .background {
                ZStack {
                    Circle().fill(RadialGradient(colors: [.white.opacity(0.42), .clear], center: .bottom, startRadius: 0, endRadius: 26))
                    Ellipse().fill(LinearGradient(colors: [.white.opacity(0.5), .white.opacity(0.03)], startPoint: .top, endPoint: .bottom))
                        .frame(width: 30, height: 15).offset(y: -12.5)
                }.clipShape(.circle)
            }
            .glassEffect(.regular.tint(tint.opacity(0.85)).interactive(), in: .circle)
            .overlay {
                Circle().strokeBorder(LinearGradient(colors: [.white.opacity(0.7), .white.opacity(0.05), .white.opacity(0.5)],
                                                     startPoint: .top, endPoint: .bottom), lineWidth: 0.75)
                    .allowsHitTesting(false)
            }
            .shadow(color: .black.opacity(0.28), radius: 8, y: 4)
    }
}

/// The launcher's agents in a row: the selected one on a tile with its name,
/// the others their avatars. A long list scrolls rather than squeezes.
private struct AgentSegments: View {
    let library: Library
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Namespace private var pill
    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 2) {
                ForEach(library.agents) { agent in
                    let selected = agent.name == library.agent?.name
                    Button {
                        withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) { library.selectedAgent = agent.name }
                    } label: {
                        HStack(spacing: 7) {
                            AgentAvatar(agent: agent, size: 26)
                            if selected { Text(agent.displayTitle).font(.subheadline.weight(.medium)).lineLimit(1).padding(.trailing, 7) }
                        }
                        .padding(5)
                        .background {
                            if selected {
                                // concentric with the avatar's corners, 5 inside it
                                RoundedRectangle(cornerRadius: 12).fill(Color(uiColor: .systemBackground).opacity(0.8))
                                    .shadow(color: .black.opacity(0.12), radius: 3, y: 1)
                                    .matchedGeometryEffect(id: "pill", in: pill)
                            }
                        }
                        .contentShape(.rect(cornerRadius: 12))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(agent.displayTitle)
                    .accessibilityAddTraits(selected ? .isSelected : [])
                    .accessibilityIdentifier("launcherAgent-\(agent.name)")
                }
            }
            // room for the tile's shadow inside the scroll view's clip
            .padding(.vertical, 3).padding(.horizontal, 16)
        }
        .scrollIndicators(.hidden).scrollBounceBehavior(.basedOnSize)
    }
}

struct HomeView: View {
    let library: Library
    let api: FragmentAPI
    let open: (FragmentSummary) -> Void
    /// A new chat with what Home holds as its first message (`Library.launch`).
    let start: () async -> Void
    let refresh: () async -> Void
    /// Something stands over the launcher, so the keyboard waits for it to go.
    let covered: Bool
    @State private var results = SearchResults()
    @State private var searching = false
    @State private var searchError: String?
    @State private var focused = false
    @State private var logoY: CGFloat?
    @State private var cardY: CGFloat?
    private var home: HomeDraft { library.home }
    private var query: String { home.words }
    private var canStart: Bool { library.canLaunch }
    /// Searching, the card takes all the room there is, the logo behind it.
    private var expanded: Bool { !query.isEmpty }

    var body: some View {
        ZStack {
            DesktopWallpaper().glassLogo(size: 136, y: expanded ? cardY : logoY)
            VStack(spacing: 0) {
                // The logo sits midway between the top bar and the card.
                // 7 past the bar's own 9 under its buttons: the sides' 16 by eye.
                if expanded { Color.clear.frame(height: 7) }
                else { Spacer(minLength: 16).onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).midY } action: { logoY = $0 } }
                VStack(spacing: 0) {
                    // the chat's own composer: Return is a new line, Send starts the chat
                    ComposerBar(text: Binding(get: { home.text }, set: { home.text = $0 }), enabled: canStart,
                                busy: library.creating || home.reading,
                                placeholder: "Ask, or find a chat or app",
                                attached: home.files.map { ($0.id.uuidString, $0.file.name) },
                                remove: { id in if let id = UUID(uuidString: id) { home.remove(id) } },
                                attach: { home.hold($0, $1) }, room: home.room, memo: { launchMemo($0) },
                                send: { await launch() }, editable: !home.launching, recordingChanged: recordingChanged,
                                chrome: false, recordFirst: true, canRecord: library.agent != nil, focused: $focused)
                        .padding(.horizontal, 4).padding(.top, 10).padding(.bottom, library.agent == nil ? 4 : -1) // 8 with the agents' own 3
                    if let error = home.error { Text(error).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 18).padding(.bottom, 6) }
                    if library.agent != nil {
                        AgentSegments(library: library).padding(.bottom, 13)
                    }
                    if !query.isEmpty {
                        Divider()
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 4) {
                                foundFragments("Chats", kinds: ["chat"])
                                foundFragments("Apps", kinds: ["app", "brain"])
                                if !results.messages.isEmpty {
                                    heading("Messages")
                                    ForEach(results.messages) { hit in
                                        if let fragment = library.fragments.first(where: { $0.name == hit.fragment }) {
                                            Button { dismissKeyboard(); open(fragment) } label: {
                                                VStack(alignment: .leading, spacing: 5) {
                                                    Text(fragment.displayTitle).font(.subheadline.weight(.medium))
                                                    Text(hit.snippet).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                                                }.frame(maxWidth: .infinity, alignment: .leading).padding(12)
                                            }.accessibilityIdentifier("messageResult-\(hit.id)")
                                        }
                                    }
                                }
                                if let searchError { Text(searchError).font(.footnote).foregroundStyle(.secondary).padding() }
                            }.padding(6).buttonStyle(.plain)
                        }
                        .scrollDismissesKeyboard(.interactively)
                        .overlay { if searching { ProgressView("Searching…").font(.subheadline).foregroundStyle(.secondary) } }
                    }
                    if library.agent == nil && !library.loading {
                        VStack(spacing: 8) {
                            Text("Your account has no agent yet.").foregroundStyle(.secondary)
                            Link("Set up your agent on the web", destination: api.baseURL.appending(path: "welcome"))
                        }.font(.footnote).padding()
                    }
                    if let error = library.error {
                        Text(error).font(.footnote).foregroundStyle(.secondary).padding()
                        Button("Try again") { Task { await refresh() } }.padding(.bottom)
                    }
                }
                .glassEffect(.regular, in: .rect(cornerRadius: 26))
                .frame(maxWidth: 620).padding(.horizontal, 16)
                .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).midY } action: { cardY = $0 }
                // Centered until the keyboard rises, then resting just above it.
                if focused || expanded { Color.clear.frame(height: 16) } else { Spacer(minLength: 16) }
            }
            .animation(.snappy(duration: 0.25), value: expanded)
        }
        .onAppear { focused = !covered }
        .onChange(of: covered) { _, covered in focused = !covered }
        .task(id: query) {
            results = SearchResults(); searchError = nil; searching = false
            guard !query.isEmpty else { return }
            let asked = query
            do {
                try await Task.sleep(for: .milliseconds(250)); searching = true
                let found = try await api.search(asked)
                try Task.checkCancellation()
                guard asked == query else { return }
                results = found; searching = false
            } catch { guard !Task.isCancelled, asked == query else { return }; searchError = error.localizedDescription; searching = false }
        }
    }
    private func heading(_ text: String) -> some View { Text(text).font(.caption.weight(.semibold)).foregroundStyle(.secondary).padding(.horizontal, 12).padding(.top, 10) }
    @ViewBuilder private func foundFragments(_ title: String, kinds: Set<String>) -> some View {
        let found = results.fragments.filter { kinds.contains($0.kind) }
        if !found.isEmpty {
            heading(title)
            ForEach(found) { fragment in
                Button { dismissKeyboard(); open(fragment) } label: {
                    HStack(spacing: 12) {
                        if fragment.kind == "chat" { AgentAvatar(agent: library.agents(in: fragment).first, size: 28) }
                        else { Image(systemName: "app").frame(width: 28) }
                        VStack(alignment: .leading, spacing: 4) {
                            Text(fragment.displayTitle).font(.subheadline.weight(.medium))
                            if let preview = fragment.preview { Text(preview).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                        }
                        Spacer(minLength: 0)
                        if fragment.archived == true { Image(systemName: "archivebox").font(.caption).foregroundStyle(.secondary) }
                    }.padding(12)
                }.accessibilityIdentifier("fragmentResult-\(fragment.name)")
            }
        }
    }
    private func launch() async {
        guard canStart else { return }
        dismissKeyboard()
        await start()
    }
    /// Recording, its chat is made: a finished memo need not wait for one.
    private func recordingChanged(_ recording: Bool) {
        home.recording = recording
        if recording, let agent = library.agent { library.readyChat(with: agent, api: api) }
    }
    /// A voice memo, held as any file is, its place taken before it records
    /// (`HomeDraft.hold`), and launched once recorded, with anything typed
    /// meanwhile. One not held (cancelled, unreadable) launches nothing:
    /// words never go without the memo they were written beside. Not sent,
    /// Home keeps it with why.
    private func launchMemo(_ read: @escaping @Sendable () async throws -> [OutgoingFile]) -> Bool {
        let before = Set(home.files.map(\.id))
        guard home.hold(1, read), let holding = home.holding else { return false }
        Task {
            await holding.value
            // its recording is over: the launch needn't wait for the view to say so
            home.recording = false
            guard home.files.contains(where: { !before.contains($0.id) }) else { return }
            await launch()
        }
        return true
    }
    private func dismissKeyboard() { UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil) }
}

/// The composer with what it attaches, one view for a chat and the home
/// screen's prompt: words over several lines (Return is a new line, Send
/// sends), Photos or Files from its +, a pasted picture, and a voice memo.
/// What is attached is read here, off the main actor, and handed to
/// `attach`, whose place says where it goes: a chat uploads it at once, the
/// home screen keeps it until its chat is made (cell/shell/launcher.js).
struct ComposerBar: View {
    @Environment(\.scenePhase) private var scenePhase
    @Binding var text: String
    var working = false
    let enabled: Bool
    var busy = false
    var placeholder: String
    /// What is attached now, by id and name: each a chip that removes it.
    let attached: [(id: String, name: String)]
    let remove: (String) -> Void
    /// What is attached, as its owner shows it instead of those chips: a
    /// chat's tiles (`ComposerAttachments`), each upload's progress and Retry.
    var files: AnyView? = nil
    /// Takes `count` files that `read` will make, or refuses them (saying
    /// why itself): the composer is busy from the choosing on.
    let attach: (_ count: Int, _ read: @escaping @Sendable () async throws -> [OutgoingFile]) -> Bool
    /// Places the message has left for files, as its owner counts them (a
    /// chat's uploads, failed ones too, `ChatSession.fileRoom`).
    let room: Int
    /// A voice memo, its place taken before it records as `attach(1)`
    /// takes one, and sent once recorded (its owner's to send), refused
    /// as `attach` refuses: `read` ends with the recording.
    let memo: (_ read: @escaping @Sendable () async throws -> [OutgoingFile]) -> Bool
    let send: () async -> Void
    var stop: () async -> Void = {}
    /// Steer, given whether a voice memo is being recorded: none, no Steer.
    var steer: (_ recording: Bool) -> (() async -> Void)? = { _ in nil }
    /// Quoted above the words, with its cancel (a chat's reply).
    var quote: (text: String, cancel: () -> Void)? = nil
    /// Whether its words and files may change now (none while what was
    /// submitted is being sent from Home).
    var editable = true
    /// Told as a voice memo starts and stops being recorded.
    var recordingChanged: (Bool) -> Void = { _ in }
    /// The composer's own glass: none inside a surface that has its own.
    var chrome = true
    /// One action, the launcher's: record until something is typed, then Send.
    var recordFirst = false
    var canRecord = true
    /// Whether its field has the keyboard, read and set: none, its own affair.
    var focused: Binding<Bool>? = nil
    @State private var importer = false
    @State private var photos = false
    @State private var picked: [PhotosPickerItem] = []
    @State private var voice = VoiceMemo()

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                if let quote {
                    HStack { Label(quote.text, systemImage: "arrowshape.turn.up.left").font(.caption).lineLimit(2); Spacer(); Button("Cancel reply", systemImage: "xmark", action: quote.cancel).labelStyle(.iconOnly) }
                }
                if let files { files }
                else if !attached.isEmpty {
                    ScrollView(.horizontal) { HStack {
                        ForEach(attached, id: \.id) { file in
                            let shown = ComposerMemos.label(file.name)
                            Button { remove(file.id) } label: { Label(shown, systemImage: "xmark.circle").font(.caption) }.buttonStyle(.bordered).disabled(!editable)
                                .accessibilityLabel("Remove \(shown)")
                                .accessibilityIdentifier("attached")
                        }
                    } }
                }
                if let error = voice.error { Text(error).font(.caption).foregroundStyle(.secondary) }
                if let since = voice.startedAt { RecordingLabel(since: since) }
            }.padding(.horizontal, chrome ? 20 : 12)
            // a memo being recorded holds its place as attaching, and its Finish stays
            ComposerView(text: $text, working: working, enabled: enabled, busy: busy && !voice.recording, recording: voice.recording, placeholder: placeholder,
                         attachPhotos: { if roomy() { photos = true } }, attachFiles: { if roomy() { importer = true } },
                         pasteImages: { paste($0) }, record: { recordVoice() },
                         send: send, stop: stop, steer: steer(voice.recording), hasAttachments: !attached.isEmpty, editable: editable, chrome: chrome,
                         recordFirst: recordFirst, canRecord: canRecord, focused: focused)
        }
        .fileImporter(isPresented: $importer, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls): _ = attach(urls.count) { try await ComposerFiles.read(urls) }
            case .failure(let error): voice.error = error.localizedDescription
            }
        }
        // Photos as the system's picker gives them, its most compatible
        // encoding: a picture the web shows goes as it is, any other as JPEG
        .photosPicker(isPresented: $photos, selection: $picked, maxSelectionCount: max(1, room),
                      matching: .any(of: [.images, .videos]), preferredItemEncoding: .compatible)
        .onChange(of: picked) { _, items in
            guard !items.isEmpty else { return }
            picked = []
            // busy from the choosing on, before any photo is read (an iCloud one takes a while)
            _ = attach(items.count) { try await ComposerFiles.read(items) }
        }
        .onChange(of: voice.recording, initial: true) { _, now in recordingChanged(now) }
        .onChange(of: scenePhase) { _, phase in if phase != .active { voice.cancel() } }
        .onDisappear { voice.cancel() }
    }

    /// Pictures pasted: read from the pasteboard here, made off the main actor.
    private func paste(_ pasteboard: UIPasteboard) {
        let pictures = AttachmentInput.snapshot(pasteboard)
        guard !pictures.isEmpty else { voice.error = "This picture could not be attached."; return }
        _ = attach(pictures.count) { await Task.detached(priority: .userInitiated) { AttachmentInput.pasted(pictures) }.value }
    }
    /// Whether a file has a place left, saying so if not.
    private func roomy() -> Bool {
        guard room > 0 else { voice.error = ChatAttachment.limits.countRefusal; return false }
        return true
    }
    /// A voice memo: its place taken before it records (`memo`), so a
    /// message with no room refuses it before it starts, never once it is
    /// recorded; handed on once finished, by hand or at its deadline,
    /// which sends it.
    private func recordVoice() {
        if voice.recording { voice.finish(); return }
        _ = memo { [voice] in try await voice.file() }
    }
}

/// The composer's voice memos, by the name each is sent with (`VoiceMemo`'s
/// `voice-<UUID>.m4a`, which the web and agents see): its chip says "Voice
/// memo · 0:07", never that name. Lengths are kept while the app runs; a
/// draft's memo from before a relaunch says "Voice memo".
enum ComposerMemos {
    private static let lengths = Mutex<[String: Int]>([:])
    /// Lengths kept at most: a bound on memory alone.
    static let keptMax = 64
    /// A memo's length in seconds, read from its file.
    static func remember(_ url: URL) {
        guard let player = try? AVAudioPlayer(contentsOf: url), player.duration.isFinite else { return }
        remember(url.lastPathComponent, seconds: Int(player.duration.rounded()))
    }
    static func remember(_ name: String, seconds: Int) {
        lengths.withLock { kept in
            if kept.count >= keptMax { kept.removeAll() }
            kept[name] = max(0, seconds)
        }
    }
    /// What a chip says for an attached file of this name.
    static func label(_ name: String) -> String {
        guard name.wholeMatch(of: /voice-[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\.m4a/) != nil else { return name }
        guard let seconds = lengths.withLock({ $0[name] }) else { return "Voice memo" }
        return "Voice memo · \(seconds / 60):" + String(format: "%02d", seconds % 60)
    }
}

/// What the composer attaches, read as the chat's page takes files: each
/// within the bound a file may be, a picture made one the web shows.
enum ComposerFiles {
    /// Files chosen (in Files, or a voice memo just recorded), off the main actor.
    nonisolated static func read(_ urls: [URL]) async throws -> [OutgoingFile] {
        try await Task.detached(priority: .userInitiated) {
            try urls.map { url in
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                let limits = ChatAttachment.limits
                guard size > 0, size <= limits.uploadMaxBytes else { throw APIError.server(limits.sizeRefusal) }
                return try AttachmentInput.file(Data(contentsOf: url), name: url.lastPathComponent, type: UTType(filenameExtension: url.pathExtension))
            }
        }.value
    }
    /// Photos picked: each read in the picker's most compatible encoding,
    /// a picture made one the web shows (`AttachmentInput.image`), off the
    /// main actor.
    nonisolated static func read(_ items: [PhotosPickerItem]) async throws -> [OutgoingFile] {
        var files: [OutgoingFile] = []
        for (index, item) in items.enumerated() {
            guard let data = try await item.loadTransferable(type: Data.self) else { throw APIError.server("This item could not be read.") }
            let type = item.supportedContentTypes.first { $0.conforms(to: .image) || $0.conforms(to: .movie) } ?? .data
            files.append(try await Task.detached(priority: .userInitiated) {
                type.conforms(to: .image)
                    ? try AttachmentInput.image(data, type: type, name: "photo-\(index + 1)")
                    : try AttachmentInput.file(data, name: "video-\(index + 1).\(type.preferredFilenameExtension ?? "mov")", type: type)
            }.value)
        }
        return files
    }
}

struct ComposerView: View {
    @Binding var text: String
    /// Whether a turn runs that Stop stops: the circle is Stop until
    /// something is typed, then Send (chat.js `refreshSend`).
    let working: Bool
    let enabled: Bool
    var busy = false
    var recording = false
    /// What the field says while empty: what sending does now.
    var placeholder = "Message your agent"
    /// The + menu's two ways to attach: the photo library, or Files.
    let attachPhotos: () -> Void
    let attachFiles: () -> Void
    /// Pictures pasted into the field (`ComposerField`).
    let pasteImages: (UIPasteboard) -> Void
    let record: () -> Void
    let send: () async -> Void
    let stop: () async -> Void
    /// Tells the running turn what is typed now, when its runtime offers
    /// it (`ChatSession.steers`); none, no Steer.
    var steer: (() async -> Void)? = nil
    @State private var submitting = false
    var hasAttachments = false
    /// Whether its words may change now.
    var editable = true
    /// Its own glass and margins: none inside a surface that has its own.
    var chrome = true
    /// One action in its circle's place (`ComposerBar`), in the launcher's glass.
    var recordFirst = false
    var canRecord = true
    var focused: Binding<Bool>? = nil
    private var typed: Bool { !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || hasAttachments || recording }
    private var stops: Bool { working && !typed }
    private var valid: Bool { (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || hasAttachments) && text.utf8.count <= 32_768 }
    private var canSend: Bool { !submitting && !busy && !recording && enabled && (stops || valid) }
    var body: some View {
        // A single glass surface, with equal hit targets and no nested glass
        // button padding, keeps the single-line and growing field aligned.
        HStack(alignment: .bottom, spacing: 0) {
            Menu {
                Button("Photos", systemImage: "photo.on.rectangle", action: attachPhotos)
                Button("Files", systemImage: "folder", action: attachFiles)
            } label: {
                Image(systemName: "plus").font(.system(size: 20))
                    .frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .accessibilityLabel("Attach").accessibilityIdentifier("attach").disabled(busy || recording || !editable)
            if !recordFirst {
                Button(action: record) {
                    Image(systemName: recording ? "stop.circle.fill" : "mic")
                        .font(.system(size: 20))
                        .foregroundStyle(recording ? Color.red : Color.primary)
                        .frame(width: 44, height: 44).contentShape(Rectangle())
                }
                .accessibilityLabel(recording ? "Finish voice memo" : "Record a voice memo")
                .disabled(busy)
            }
            ComposerField(text: $text, placeholder: placeholder, pasteImages: pasteImages, focused: focused)
                .frame(minHeight: 44).disabled(!editable)
            if let steer, !stops {
                // Send queues the words for the turn after; Steer tells this one now
                Button {
                    submitting = true
                    Task { await steer(); submitting = false }
                } label: {
                    Text("Steer").font(.subheadline.weight(.semibold))
                        .padding(.horizontal, 12).frame(height: 34)
                        .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                        .frame(height: 44).contentShape(Capsule())
                }
                .disabled(submitting || busy || !enabled)
                .accessibilityLabel("Steer it now")
                .accessibilityIdentifier("steer")
            }
            if recordFirst { launcherAction } else { Button {
                submitting = true
                Task { await (stops ? stop() : send()); submitting = false }
            } label: {
                ZStack {
                    // The adaptive label color keeps the filled control high-contrast;
                    // the glass foreground style is softened by vibrancy.
                    Circle().fill(canSend ? Color(uiColor: .label) : Color(uiColor: .tertiarySystemFill))
                    if busy || submitting {
                        ProgressView().tint(.secondary)
                    } else {
                        Image(systemName: stops ? "stop.fill" : "arrow.up")
                            .font(.system(size: 17, weight: .semibold))
                            .foregroundStyle(canSend ? Color(uiColor: .systemBackground) : Color.secondary)
                    }
                }
                .frame(width: 34, height: 34)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
            }
            .disabled(!canSend)
            .accessibilityLabel(stops ? "Stop response" : "Send message")
            .accessibilityIdentifier(stops ? "stop" : "send") }
        }
        .buttonStyle(.plain)
        .padding(6)
        .modifier(ComposerChrome(shown: chrome))
    }
    @ViewBuilder private var launcherAction: some View {
        if recording {
            Button(action: record) { Image(systemName: "stop.fill").foregroundStyle(.white).launcherGlass(tint: .red) }
                .accessibilityLabel("Finish voice memo").accessibilityIdentifier("finishMemo")
        } else if !typed && !busy && !submitting {
            // not `disabled`: never dimmed while the list loads
            Button(action: record) { Image(systemName: "mic").launcherGlass() }.allowsHitTesting(canRecord)
                .accessibilityLabel("Record a voice memo").accessibilityIdentifier("recordMemo")
        } else {
            Button {
                submitting = true
                Task { await send(); submitting = false }
            } label: {
                if busy || submitting { ProgressView().launcherGlass() } else { Image(systemName: "arrow.up").launcherGlass() }
            }
            .disabled(!canSend).opacity(canSend || busy || submitting ? 1 : 0.4)
            .accessibilityLabel("Send message").accessibilityIdentifier("send")
        }
    }
}

/// The composer's own glass surface and margins, when it is not inside one.
private struct ComposerChrome: ViewModifier {
    let shown: Bool
    func body(content: Content) -> some View {
        if shown {
            content.glassEffect(.regular, in: .rect(cornerRadius: 28))
                .frame(maxWidth: 660)
                .padding(.horizontal, 12).padding(.top, 8).padding(.bottom, 8)
                .frame(maxWidth: .infinity)
        } else { content }
    }
}

/// The composer's words: a text view of UIKit's, as a SwiftUI field takes no
/// pasted picture. Long-pressing with a picture on the clipboard offers
/// Paste, which hands the pictures to `pasteImages` (and pastes no text, as
/// the web's chat takes a paste of files); text pastes as before. It grows
/// to six lines, then scrolls; Return is a new line; dictation is the
/// system's.
struct ComposerField: UIViewRepresentable {
    @Binding var text: String
    let placeholder: String
    let pasteImages: (UIPasteboard) -> Void
    var focused: Binding<Bool>? = nil
    static let linesMax = 6

    func makeUIView(context: Context) -> PastingTextView {
        let view = PastingTextView()
        view.delegate = context.coordinator
        view.font = .preferredFont(forTextStyle: .body)
        view.adjustsFontForContentSizeCategory = true
        view.backgroundColor = .clear
        view.tintColor = .label
        view.isScrollEnabled = false
        view.textContainerInset = UIEdgeInsets(top: 11, left: 2, bottom: 11, right: 2)
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.accessibilityIdentifier = "composer"
        view.placeholder.isAccessibilityElement = false
        return view
    }
    func updateUIView(_ view: PastingTextView, context: Context) {
        context.coordinator.text = $text
        context.coordinator.focused = focused
        view.pasteImages = pasteImages
        if let focused, focused.wrappedValue != view.isFirstResponder {
            // after this update: a view just made has no window to focus in yet
            let wanted = focused.wrappedValue, coordinator = context.coordinator
            DispatchQueue.main.async { coordinator.focus(view, wanted) }
        }
        // disabled (what was submitted being sent): no typing, no pasting
        view.isEditable = context.environment.isEnabled
        // it says what sending does now (an answer, a message that waits its turn)
        if view.placeholder.text != placeholder { view.placeholder.text = placeholder; view.accessibilityLabel = placeholder }
        if view.text != text { view.text = text }
        view.placeholder.isHidden = !text.isEmpty
    }
    func sizeThatFits(_ proposal: ProposedViewSize, uiView view: PastingTextView, context: Context) -> CGSize? {
        let width = proposal.width ?? 240
        let fitting = view.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        let most = (view.font?.lineHeight ?? 22) * CGFloat(Self.linesMax) + view.textContainerInset.top + view.textContainerInset.bottom
        let scrolls = fitting > most
        if view.isScrollEnabled != scrolls { view.isScrollEnabled = scrolls }
        return CGSize(width: width, height: min(fitting, most))
    }
    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, UITextViewDelegate {
        var text: Binding<String>
        var focused: Binding<Bool>?
        init(text: Binding<String>) { self.text = text }
        /// Takes the keyboard, or gives it up. Taken as its page arrives
        /// (Home on Back from Your Apps, whose `onAppear` asks for it), it
        /// waits for that transition to end: a keyboard raised during it
        /// never reaches the arriving page's layout, which leaves the
        /// composer under the keyboard. A transition cancelled (a swipe
        /// back let go) takes nothing, and asking again takes it.
        func focus(_ view: UITextView, _ wanted: Bool) {
            guard wanted else { view.resignFirstResponder(); return }
            guard let transition = view.owningController?.transitionCoordinator else { view.becomeFirstResponder(); return }
            transition.animate(alongsideTransition: nil) { [weak self, weak view] context in
                guard let self, let view, self.focused?.wrappedValue == true else { return }
                if context.isCancelled { self.focused?.wrappedValue = false } else { view.becomeFirstResponder() }
            }
        }
        func textViewDidBeginEditing(_ view: UITextView) { focused?.wrappedValue = true }
        func textViewDidEndEditing(_ view: UITextView) { focused?.wrappedValue = false }
        func textViewDidChange(_ view: UITextView) {
            text.wrappedValue = view.text
            (view as? PastingTextView)?.placeholder.isHidden = !view.text.isEmpty
        }
    }
}

private extension UIView {
    /// The view controller this view is in: the one whose transitions move it.
    var owningController: UIViewController? {
        var next = self.next
        while let responder = next {
            if let controller = responder as? UIViewController { return controller }
            next = responder.next
        }
        return nil
    }
}

final class PastingTextView: UITextView {
    var pasteImages: ((UIPasteboard) -> Void)?
    let placeholder = UILabel()

    override init(frame: CGRect, textContainer: NSTextContainer?) {
        super.init(frame: frame, textContainer: textContainer)
        placeholder.font = .preferredFont(forTextStyle: .body)
        placeholder.adjustsFontForContentSizeCategory = true
        placeholder.textColor = .placeholderText
        addSubview(placeholder)
    }
    required init?(coder: NSCoder) { fatalError("made in code") }
    override func layoutSubviews() {
        super.layoutSubviews()
        let left = textContainerInset.left + textContainer.lineFragmentPadding
        placeholder.frame = CGRect(x: left, y: textContainerInset.top, width: bounds.width - left - textContainerInset.right, height: placeholder.font.lineHeight)
    }
    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(paste(_:)), pasteImages != nil, UIPasteboard.general.hasImages { return true }
        return super.canPerformAction(action, withSender: sender)
    }
    override func paste(_ sender: Any?) {
        if let pasteImages, UIPasteboard.general.hasImages { pasteImages(UIPasteboard.general); return }
        super.paste(sender)
    }
}
