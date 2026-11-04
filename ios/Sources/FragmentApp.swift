import SwiftUI

@main
struct FragmentApp: App {
    var body: some Scene { WindowGroup { FragmentRoot() } }
}

@MainActor @Observable
final class Library {
    var fragments: [FragmentSummary] = []
    var error: String?
    var loading = false
    var creating = false
    var unfinishedChat: FragmentSummary?
    var selectedAgent: String?
    var imported: [ImportedSession] = []
    var identities: [String: String] = [:]
    var sessions: [String: ChatSession] = [:]
    @ObservationIgnored private var loadID = UUID()

    var chats: [FragmentSummary] { fragments.filter { $0.kind == "chat" && $0.archived != true }.sorted { ($0.previewAt ?? 0) > ($1.previewAt ?? 0) } }
    var allApps: [FragmentSummary] { fragments.filter { ["app", "brain"].contains($0.kind) && $0.name != "add-app" } }
    var apps: [FragmentSummary] { allApps.filter { $0.archived != true } }
    var shownApps: [FragmentSummary] { apps.filter { $0.dock != false } }
    var agents: [FragmentSummary] { fragments.filter { $0.kind == "agent" && $0.archived != true } }
    var agent: FragmentSummary? { agents.first { $0.name == selectedAgent } ?? agents.first }
    func agent(for identity: String) -> FragmentSummary? { agents.first { identities[$0.name] == identity } }
    func agents(in chat: FragmentSummary) -> [FragmentSummary] { (chat.agents ?? []).compactMap { agent(for: $0) } }
    func session(for chat: FragmentSummary, api: FragmentAPI) throws -> ChatSession {
        if let session = sessions[chat.name] { return session }
        // A bounded cache preserves unsent messages and cursors while changing screens.
        if sessions.count >= 32, let unused = sessions.first(where: { $0.value.pending.isEmpty && $0.value.composer.isEmpty && $0.value.attachments.isEmpty })?.key {
            sessions.removeValue(forKey: unused)
        }
        guard sessions.count < 32 else { throw APIError.server("Finish or discard a draft in another chat before opening more conversations.") }
        let session = ChatSession(fragment: chat, api: api)
        sessions[chat.name] = session
        return session
    }
    func load(_ api: FragmentAPI) async {
        let requestID = UUID(); loadID = requestID
        loading = true
        defer { if loadID == requestID { loading = false } }
        do {
            let response = try await api.fragments()
            guard loadID == requestID, !Task.isCancelled else { return }
            fragments = response
            for index in fragments.indices {
                if let picture = fragments[index].picture { fragments[index].picture = URL(string: picture, relativeTo: api.baseURL)?.absoluteString }
            }
            if let archive = fragments.first(where: { $0.kind == "sessions" && $0.role == "owner" }) {
                struct Index: Decodable { let sessions: [ImportedSession] }
                let index: Index = try await api.request("api/f/\(archive.name)/file", query: [URLQueryItem(name: "path", value: "index/chat.json")])
                guard loadID == requestID, !Task.isCancelled else { return }
                imported = Array(index.sessions.prefix(2_000))
            } else { imported = [] }
            for agent in agents.prefix(32) where identities[agent.name] == nil {
                let status: FragmentStatus = try await api.request("api/f/\(agent.name)/status")
                identities[agent.name] = status.npub
            }
            if loadID == requestID { error = nil }
        } catch { if loadID == requestID && !Task.isCancelled { self.error = error.localizedDescription } }
    }
    func watch(_ api: FragmentAPI) async {
        guard api.isSigned else { return }
        var attempt = 0
        // One foreground watcher. Cancellation closes an outstanding receive;
        // reconnect reloads the list so changes during a disconnect are included.
        while !Task.isCancelled {
            let socket: URLSessionWebSocketTask
            do { socket = try api.webSocket("api/fragments/watch") }
            catch { self.error = error.localizedDescription; return }
            socket.resume()
            do {
                try await withTaskCancellationHandler {
                    while !Task.isCancelled {
                        let message = try await socket.receive()
                        let data: Data
                        switch message {
                        case .data(let bytes): data = bytes
                        case .string(let text): data = Data(text.utf8)
                        @unknown default: throw APIError.invalidResponse
                        }
                        let frame = try JSONDecoder().decode(JSONValue.self, from: data)
                        if ["hello", "changed", "invited"].contains(frame["type"].string ?? "") {
                            attempt = 0
                            await load(api)
                        }
                    }
                } onCancel: { socket.cancel(with: .goingAway, reason: nil) }
            } catch {
                let status = (socket.response as? HTTPURLResponse)?.statusCode
                if status == 401 { api.authorizationFailed() }
                if status == 401 || status == 403 { socket.cancel(with: .goingAway, reason: nil); return }
            }
            socket.cancel(with: .goingAway, reason: nil)
            guard !Task.isCancelled else { return }
            attempt = min(attempt + 1, 4)
            do { try await Task.sleep(for: .seconds(min(15, 1 << attempt))) } catch { return }
        }
    }
    func archive(_ fragment: FragmentSummary, api: FragmentAPI) async {
        do {
            let _: JSONValue = try await api.request("api/fragments/\(fragment.name)/archived", method: "PUT", body: .object(["archived": .bool(fragment.archived != true)]))
            await load(api)
        } catch { self.error = error.localizedDescription }
    }
}

struct FragmentStatus: Decodable { struct URLs: Decodable { let canonical: URL }; let npub: String; let urls: URLs }
enum ShellPage: Equatable { case launcher, chat(String), history, settings, apps }

struct FragmentRoot: View {
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var library = Library()
    @State private var connection = AccountConnection()
    @State private var showingConnection = false
    @State private var page = ShellPage.launcher
    @State private var sidebar = false
    @State private var showingApps = false
    @State private var openedApp: FragmentSummary?
    @State private var homeDraft = ""
    @AppStorage("localServer") private var localServer = "http://localhost:8910"

    private var previewURL: URL? {
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        let explicit = args.firstIndex(of: "--server").flatMap { args.indices.contains($0 + 1) ? args[$0 + 1] : nil }
        guard let raw = explicit ?? (connection.preview ? localServer : nil) else { return nil }
        return try? PlatformOrigin.parse(raw)
        #else
        return nil
        #endif
    }
    private var activeAPI: FragmentAPI? { previewURL.map { FragmentAPI(baseURL: $0) } ?? connection.api }
    var body: some View {
        Group {
            if let api = activeAPI { shell(api).environment(\.fragmentAPI, api) }
            else { AccountConnectionView(connection: connection) }
        }
        .tint(.primary)
        .sheet(isPresented: $showingConnection) { AccountConnectionView(connection: connection, close: { showingConnection = false }) }
        .onChange(of: connection.account?.identity) { _, _ in resetNavigation() }
        .onChange(of: connection.account?.approved) { _, _ in resetNavigation() }
        .onReceive(NotificationCenter.default.publisher(for: .finiteAuthorizationFailed).receive(on: RunLoop.main)) { notification in
            guard let api = connection.api, notification.userInfo?["origin"] as? String == api.baseURL.absoluteString,
                  notification.userInfo?["key"] as? String == api.identity?.publicKey else { return }
            Task { await connection.validate() }
        }
        .task(id: scenePhase) { if scenePhase == .active { await connection.validate() } }
    }
    private func resetNavigation() {
        library = Library(); page = .launcher; sidebar = false; showingApps = false; openedApp = nil
    }
    private func shell(_ api: FragmentAPI) -> some View {
        ZStack(alignment: .leading) {
            Group {
                if showingApps {
                    NavigationStack {
                        AppsLibraryView(library: library, api: api, open: { openedApp = $0 })
                            .navigationTitle("Your Apps").navigationBarTitleDisplayMode(.inline)
                            .toolbar {
                                ToolbarItem(placement: .topBarLeading) {
                                    Button("Show sidebar", systemImage: "sidebar.left") { toggleSidebar() }.accessibilityIdentifier("sidebar")
                                }
                            }
                    }
                } else {
                    NavigationStack {
                        content(api)
                            .navigationTitle(title)
                            .navigationBarTitleDisplayMode(.inline)
                            .toolbar {
                                ToolbarItem(placement: .topBarLeading) {
                                    Button("Show sidebar", systemImage: "sidebar.left") { toggleSidebar() }.accessibilityIdentifier("sidebar")
                                }
                                ToolbarItemGroup(placement: .topBarTrailing) {
                                    if page != .launcher && page != .apps {
                                        Button("Search or start a new chat", systemImage: "square.and.pencil") { page = .launcher }.accessibilityIdentifier("launcher")
                                    }
                                    if page != .apps {
                                        Button("Your Apps", systemImage: "list.bullet.rectangle") { showingApps = true }.accessibilityIdentifier("yourApps")
                                    }
                                }
                            }
                    }
                }
            }
            .accessibilityHidden(sidebar)
            SidebarDrawer(isPresented: $sidebar, library: library, page: page, api: api) { destination in
                page = destination; showingApps = false; toggleSidebar()
            }
        }
        .sheet(item: $openedApp) { app in
            if app.name == "add-app" {
                AppCreationSheet { prompt in
                    openedApp = nil; showingApps = false; homeDraft = prompt; page = .launcher
                }
            } else { FragmentWebSheet(fragment: app, api: api) }
        }
        .task(id: "\(api.baseURL)|\(api.identity?.publicKey ?? "")|\(scenePhase)") {
            if scenePhase == .active { await library.load(api); await library.watch(api) }
        }
        .onChange(of: showingApps) { _, presented in
            if presented { dismissKeyboard() }
        }
        .onChange(of: localServer) { _, _ in library = Library(); page = .launcher; showingApps = false }
    }
    @ViewBuilder private func content(_ api: FragmentAPI) -> some View {
        switch page {
        case .launcher:
            HomeView(library: library, api: api, draft: $homeDraft, open: { open($0) }, start: { await start(api) }, refresh: { await library.load(api) })
        case .chat(let name):
            if let chat = library.fragments.first(where: { $0.name == name }) {
                if let session = try? library.session(for: chat, api: api) {
                    ChatView(session: session, library: library).id(chat.name)
                } else {
                    ContentUnavailableView("Your drafts are safe", systemImage: "square.and.pencil", description: Text("Finish or discard a draft in another chat before opening more conversations."))
                }
            }
        case .history: HistoryView(library: library, api: api, open: { open($0) })
        case .apps: AppsLibraryView(library: library, api: api, open: { openedApp = $0 })
        case .settings: SettingsView(localServer: $localServer, library: library, api: api, connect: { showingConnection = true }, open: { open($0) })
        }
    }
    private var title: String {
        if showingApps { return "Your Apps" }
        switch page {
        case .launcher: return "Finite.Computer"
        case .chat(let name): return library.fragments.first { $0.name == name }?.displayTitle ?? "Chat"
        case .history: return "History"
        case .apps: return "Your Apps"
        case .settings: return "Settings"
        }
    }
    private func dismissKeyboard() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
    private func toggleSidebar() {
        if !sidebar { dismissKeyboard() }
        withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) { sidebar.toggle() }
    }
    private func open(_ fragment: FragmentSummary) {
        if fragment.kind == "chat" { page = .chat(fragment.name); showingApps = false }
        else if fragment.kind == "launcher" { page = .launcher }
        else { openedApp = fragment }
    }
    private func start(_ api: FragmentAPI) async {
        guard let agent = library.agent, !library.creating else { return }
        library.creating = true
        defer { library.creating = false }
        do {
            if library.unfinishedChat == nil { library.unfinishedChat = try await api.createChat() }
            guard let chat = library.unfinishedChat else { return }
            try await api.join(agent, to: chat)
            library.unfinishedChat = nil
            await library.load(api)
            let resolved = library.fragments.first { $0.name == chat.name } ?? chat
            let session = try library.session(for: resolved, api: api)
            session.composer = homeDraft
            homeDraft = ""
            page = .chat(chat.name)
            await session.send()
            await library.load(api)
        } catch { library.error = error.localizedDescription }
    }
}

enum FragmentStyle {
    static let background = Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: 24 / 255, alpha: 1) : UIColor(red: 247 / 255, green: 246 / 255, blue: 243 / 255, alpha: 1) })
    static let surface = Color(uiColor: .secondarySystemGroupedBackground)
    static let bubble = Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: 57 / 255, alpha: 1) : UIColor(red: 233 / 255, green: 230 / 255, blue: 223 / 255, alpha: 1) })
    static let lilac = Color(red: 168 / 255, green: 139 / 255, blue: 234 / 255)
}
