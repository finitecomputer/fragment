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
    /// A voice memo being recorded in the prompt (`ComposerBar`): nothing
    /// launches until it is finished and attached.
    var recording = false
    var error: String?
    private(set) var launching = false
    var words: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    /// Whether what is held may start a chat now: one rule for every way to
    /// start one (Send, the Start a new chat row).
    var ready: Bool { (!words.isEmpty || !files.isEmpty) && text.utf8.count <= 32_768 && !launching && !reading && !recording }

    /// Files for the first message, read now and held: at most what a
    /// message carries, one reading at a time, none while a launch sends.
    func hold(_ count: Int, _ read: @escaping @Sendable () async throws -> [OutgoingFile]) -> Bool {
        guard !launching else { error = "Wait for the message being sent."; return false }
        guard !reading else { error = "Wait for the files being attached, then add more."; return false }
        guard count > 0, count + files.count <= ChatAttachment.countMax else { error = ChatAttachment.limits.countRefusal; return false }
        reading = true
        Task {
            defer { reading = false }
            do {
                let read = try await read()
                guard !read.isEmpty else { error = "This could not be attached."; return }
                guard read.count + files.count <= ChatAttachment.countMax else { error = ChatAttachment.limits.countRefusal; return }
                files += read.map { Held(id: UUID(), file: $0) }
                error = nil
            } catch { self.error = error.localizedDescription }
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

struct HomeView: View {
    let library: Library
    let api: FragmentAPI
    let open: (FragmentSummary) -> Void
    /// A new chat with what Home holds as its first message (`Library.launch`).
    let start: () async -> Void
    let refresh: () async -> Void
    @State private var results = SearchResults()
    @State private var searching = false
    @State private var searchError: String?
    private var home: HomeDraft { library.home }
    private var query: String { home.words }
    private var canStart: Bool { library.canLaunch }

    var body: some View {
        ZStack {
            DesktopWallpaper()
            VStack(spacing: 0) {
                Spacer(minLength: 24)
                VStack(spacing: 0) {
                    // the chat's own composer: Return is a new line, Send starts the chat
                    ComposerBar(text: Binding(get: { home.text }, set: { home.text = $0 }), enabled: canStart,
                                busy: library.creating || home.reading,
                                placeholder: "Ask, or find a chat or app",
                                attached: home.files.map { ($0.id.uuidString, $0.file.name) },
                                remove: { id in if let id = UUID(uuidString: id) { home.remove(id) } },
                                attach: { home.hold($0, $1) }, send: { await launch() },
                                editable: !home.launching, recordingChanged: { home.recording = $0 }, chrome: false)
                        .padding(.horizontal, 4).padding(.top, 10).padding(.bottom, 4)
                    if let error = home.error { Text(error).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 18).padding(.bottom, 6) }
                    if let agent = library.agent {
                        HStack {
                            Menu {
                                ForEach(library.agents) { agent in
                                    Button { library.selectedAgent = agent.name } label: {
                                        Label(agent.displayTitle, systemImage: agent.name == library.agent?.name ? "checkmark" : "person")
                                    }
                                }
                            } label: {
                                HStack(spacing: 8) { AgentAvatar(agent: agent, size: 22); Text(agent.displayTitle); Image(systemName: "chevron.down").font(.caption2) }
                                    .font(.caption.weight(.medium))
                            }.accessibilityIdentifier("launcherAgent")
                            Spacer()
                        }.padding(.horizontal, 18).padding(.bottom, 14)
                    }
                    if !query.isEmpty {
                        Divider()
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 4) {
                                if library.agent != nil {
                                    Button { Task { await launch() } } label: {
                                        Label {
                                            VStack(alignment: .leading, spacing: 5) {
                                                Text("Start a new chat with \(library.agent?.displayTitle ?? "your agent")").font(.subheadline.weight(.medium))
                                                Text(query).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                                            }
                                        } icon: { Image(systemName: "square.and.pencil").frame(width: 30) }
                                            .frame(maxWidth: .infinity, alignment: .leading).padding(12)
                                    }.disabled(!canStart).accessibilityIdentifier("newChatResult")
                                }
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
                                if searching { ProgressView("Searching…").padding() }
                                if let searchError { Text(searchError).font(.footnote).foregroundStyle(.secondary).padding() }
                            }.padding(6).buttonStyle(.plain)
                        }.frame(maxHeight: 330).scrollDismissesKeyboard(.interactively)
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
                Spacer(minLength: 24)
            }
        }
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
                if voice.recording { Label("Recording voice memo…", systemImage: "waveform").foregroundStyle(.red).font(.caption) }
            }.padding(.horizontal, chrome ? 20 : 12)
            ComposerView(text: $text, working: working, enabled: enabled, busy: busy, recording: voice.recording, placeholder: placeholder,
                         attachPhotos: { photos = true }, attachFiles: { importer = true },
                         pasteImages: { paste($0) }, record: { Task { await recordVoice() } },
                         send: send, stop: stop, steer: steer(voice.recording), hasAttachments: !attached.isEmpty, editable: editable, chrome: chrome)
        }
        .fileImporter(isPresented: $importer, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls): _ = attach(urls.count) { try await ComposerFiles.read(urls) }
            case .failure(let error): voice.error = error.localizedDescription
            }
        }
        // Photos as the system's picker gives them, its most compatible
        // encoding: a picture the web shows goes as it is, any other as JPEG
        .photosPicker(isPresented: $photos, selection: $picked, maxSelectionCount: max(1, ChatAttachment.countMax - attached.count),
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
    /// A voice memo, attached as a file once finished; its recording goes
    /// once read (or refused).
    private func recordVoice() async {
        if voice.recording {
            if let url = voice.stop() { attachMemo(url) }
        } else {
            guard attached.count < ChatAttachment.countMax else { voice.error = ChatAttachment.limits.countRefusal; return }
            await voice.start { url in attachMemo(url) }
        }
    }
    private func attachMemo(_ url: URL) {
        // its length, read before its file goes
        ComposerMemos.remember(url)
        let taken = attach(1) {
            defer { try? FileManager.default.removeItem(at: url) }
            return try await ComposerFiles.read([url])
        }
        if !taken { try? FileManager.default.removeItem(at: url) }
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
            Button(action: record) {
                Image(systemName: recording ? "stop.circle.fill" : "mic")
                    .font(.system(size: 20))
                    .foregroundStyle(recording ? Color.red : Color.primary)
                    .frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .accessibilityLabel(recording ? "Finish voice memo" : "Record a voice memo")
            .disabled(busy)
            ComposerField(text: $text, placeholder: placeholder, pasteImages: pasteImages)
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
            Button {
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
            .accessibilityIdentifier(stops ? "stop" : "send")
        }
        .buttonStyle(.plain)
        .padding(6)
        .modifier(ComposerChrome(shown: chrome))
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
        view.pasteImages = pasteImages
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
        init(text: Binding<String>) { self.text = text }
        func textViewDidChange(_ view: UITextView) {
            text.wrappedValue = view.text
            (view as? PastingTextView)?.placeholder.isHidden = !view.text.isEmpty
        }
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
