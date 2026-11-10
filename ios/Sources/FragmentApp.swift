import SwiftUI

@main
struct FragmentApp: App {
    /// The one account every window shows (an iPad runs several): removing,
    /// pairing or clearing in any window is every window's at once. Made
    /// after `LaunchScope.start`, which a UI test's store is emptied by.
    @State private var connection: AccountConnection
    init() {
        LaunchScope.start()
        _connection = State(initialValue: AccountConnection())
    }
    var body: some Scene { WindowGroup { FragmentRoot(connection: connection) } }
}

@MainActor @Observable
final class Library {
    /// The person's list. Each chat's session follows its agents in it
    /// (`ChatSession.agents`): one added after the chat opened counts in
    /// its cached session at once, its draft and outbox kept.
    var fragments: [FragmentSummary] = [] {
        didSet {
            guard fragments != oldValue else { return }
            for chat in fragments { if let agents = chat.agents, let session = sessions[chat.name] { session.agents = agents } }
        }
    }
    var error: String?
    var loading = false
    var creating = false
    var unfinishedChat: FragmentSummary?
    @ObservationIgnored private var readying: (agent: String, chat: Task<FragmentSummary, Error>)?
    /// What the home screen's prompt holds (`HomeDraft`).
    let home = HomeDraft()
    /// A launch not sent yet: its chat, made for it and still empty, and
    /// the id its message goes with (launcher.js `pendingLaunch`). Sent
    /// again, the same words and files are the same post in the same chat;
    /// changed, a post of their own, still in that chat.
    private(set) var pendingLaunch: (id: String, text: String, files: [UUID], agent: String, chat: FragmentSummary?)?
    var selectedAgent: String?
    var imported: [ImportedSession] = []
    var identities: [String: String] = [:]
    var sessions: [String: ChatSession] = [:]
    /// Their agents' fragments in their computer's order (`loadComputer`).
    var agentOrder: [String] = []
    /// Where this deployment serves fragments' pages, from a fragment of
    /// theirs' canonical URL, read once (`loadPages`): until it is, no link
    /// is one of theirs, and every link opens in Safari.
    var pages: FragmentPages?
    /// Their computer's `phase` as last read (`ComputerView`), or nil.
    var computerPhase: String?
    /// What they are told (`GET /api/notices`, `NoticesBand`), as last read;
    /// whether that read failed (what it said before stays); the notices
    /// whose last action failed, by id (`NoticeFailure`), until one is acted
    /// on again; and those with an action under way, every action of each
    /// off meanwhile (an invite's Accept and Decline are one answer).
    var notices: [Notice] = []
    var noticesFailed = false
    var failedNotices: [String: NoticeFailure] = [:]
    var acting: Set<String> = []
    /// The refusals whose notice is not shown at `now` (the last read no
    /// longer has it, an invite taken back; or its time passed): each a
    /// line of its own until dismissed, as the band draws it.
    func refusals(at now: Int64) -> [(id: String, failure: NoticeFailure)] {
        failedNotices.filter { id, f in f.refused && !notices.contains { $0.id == id && $0.shown(at: now) } }.sorted { $0.key < $1.key }.map { ($0.key, $0.value) }
    }
    /// A fragment of theirs an action's answer named, for the page to open
    /// (`FragmentRoot`): set by the first read begun after the answer if
    /// it lists it (`act`).
    var answered: FragmentSummary?
    @ObservationIgnored private var loadID = UUID()
    /// Reads begun (`load`), and a page an action's answer named with the
    /// number of the first read begun after the answer: the first list
    /// read of those to come back, or to fail, decides it there and then,
    /// once (`decide`), newest or not (`load` lets the newest read begun win
    /// what is shown, which says nothing of when a list was read); a read
    /// after that never opens it, the person maybe elsewhere by then. It
    /// goes with this library, and so with the account it was asked in.
    @ObservationIgnored private var begun = 0
    @ObservationIgnored private var held: (page: URL, after: Int)?
    @ObservationIgnored private var settle: Task<Void, Never>?
    @ObservationIgnored private var titling: Set<String> = []

    var chats: [FragmentSummary] { fragments.filter { $0.kind == "chat" && $0.archived != true }.sorted { ($0.previewAt ?? 0) > ($1.previewAt ?? 0) } }
    var allApps: [FragmentSummary] { fragments.filter { ["app", "brain"].contains($0.kind) && $0.name != "add-app" } }
    var apps: [FragmentSummary] { allApps.filter { $0.archived != true } }
    var shownApps: [FragmentSummary] { apps.filter { $0.dock != false } }
    /// Their own agents first, in their computer's order (its first their
    /// main agent), then any other, in the list's.
    var agents: [FragmentSummary] {
        let listed = fragments.filter { $0.kind == "agent" && $0.archived != true }
        let rank = Dictionary(agentOrder.enumerated().map { ($1, $0) }, uniquingKeysWith: { first, _ in first })
        return listed.enumerated().sorted { a, b in
            let ra = a.element.role == "owner" ? rank[a.element.name] : nil, rb = b.element.role == "owner" ? rank[b.element.name] : nil
            switch (ra, rb) {
            case let (x?, y?): return x < y
            case (_?, nil): return true
            case (nil, _?): return false
            default: return a.offset < b.offset
            }
        }.map(\.element)
    }
    /// The sidebar's agents, as the web's (cell/shell/chat-list.js
    /// `pinnedAgentsFirst`; Paul, 2026-11-07: ordering them is a sidebar
    /// matter): the ones they pinned first, then the rest, each in `agents`'
    /// order. Their main agent stays `agents`' first, wherever it is drawn.
    var sidebarAgents: [FragmentSummary] {
        agents.enumerated().sorted { a, b in
            let pa = a.element.pinned == true, pb = b.element.pinned == true
            return pa != pb ? pa : a.offset < b.offset
        }.map(\.element)
    }
    var agent: FragmentSummary? { agents.first { $0.name == selectedAgent } ?? agents.first }
    func agent(for identity: String) -> FragmentSummary? { agents.first { identities[$0.name] == identity } }
    /// The fragment of the person's list a link is a page of (the web's
    /// "Open" for one: templates/chat/site/chat.js `openBeside`), opening at
    /// that page; none for another's or any other address, which Safari opens.
    func fragment(linkedBy url: URL) -> FragmentSummary? { fragment(linkedBy: url, in: fragments) }
    /// The same, in `list` (a list read, not yet shown: `decide`).
    func fragment(linkedBy url: URL, in list: [FragmentSummary]) -> FragmentSummary? {
        guard let name = MessageLinks.fragmentName(url, pages: pages), var found = list.first(where: { $0.name == name }) else { return nil }
        if found.kind != "chat" { found.link = url }
        return found
    }
    /// A chat left: a session of it closed for good goes, so opening it
    /// again asks anew what it is now (deleted, or open to them again).
    func leftChat(_ name: String) {
        if sessions[name]?.closed != nil { sessions.removeValue(forKey: name) }
    }
    /// Whether Home may start a chat now: one rule for every way to.
    var canLaunch: Bool { home.ready && agent != nil && !creating }

    /// A new chat with `agent` in it, begun ahead of its first message (a
    /// memo's, as it records) so that message need not wait. Asked again it is
    /// the same one; a chat made outlasts a failed join, for the next try.
    @discardableResult
    func readyChat(with agent: FragmentSummary, api: FragmentAPI) -> Task<FragmentSummary, Error> {
        if let readying, readying.agent == agent.name { return readying.chat }
        let earlier = readying?.chat
        let chat = Task { [self] in
            do {
                // one readied for another agent has them in it: not this one's
                if let earlier { _ = try? await earlier.value; unfinishedChat = nil }
                var chat: FragmentSummary
                if let unfinishedChat { chat = unfinishedChat } else { chat = try await api.createChat(); unfinishedChat = chat }
                chat.agents = [try await api.join(agent, to: chat)]
                return chat
            } catch { readying = nil; throw error }
        }
        readying = (agent.name, chat)
        return chat
    }

    /// A new chat with what Home holds as its first message, sent as its
    /// chat sends one (`ChatSession.sendFirst`), as the web's launcher sends
    /// it (launcher.js `startFromSearch`): what was submitted, locked while
    /// it goes; its chat opened (`opened`) once made. Not sent, all of it
    /// stays on Home with why, and its chat is kept for it: sent again, it
    /// goes there. Throws what kept it.
    func launch(_ api: FragmentAPI, opened: (FragmentSummary) -> Void) async throws {
        guard canLaunch, let agent else { return }
        creating = true
        let submitted = home.begin()
        var sent = false
        defer { creating = false; home.ended(sent: sent ? submitted : nil) }
        let ids = submitted.files.map(\.id)
        let same = pendingLaunch.map { $0.text == submitted.text && $0.files == ids && $0.agent == agent.name } == true
        // the chat made for the last try, if it was for this agent: nothing was posted to it
        let earlier = pendingLaunch?.agent == agent.name ? pendingLaunch?.chat : nil
        pendingLaunch = (same ? pendingLaunch!.id : UUID().uuidString, submitted.text, ids, agent.name, earlier)
        do {
            var chat = earlier
            if chat == nil {
                let made = try await readyChat(with: agent, api: api).value
                readying = nil; unfinishedChat = nil
                pendingLaunch?.chat = made
                chat = made
            }
            guard let chat, let id = pendingLaunch?.id else { return }
            // listed at once: the list's own read comes after the chat has opened
            if !fragments.contains(where: { $0.name == chat.name }) { fragments.insert(chat, at: 0) }
            let resolved = fragments.first { $0.name == chat.name } ?? chat
            let session = try session(for: resolved, api: api)
            if earlier == nil { session.beginEmpty() }
            opened(resolved)
            try await session.sendFirst(id: id, text: submitted.text, files: submitted.files.map(\.file))
            pendingLaunch = nil
            sent = true
            await load(api)
        } catch {
            home.error = "Your message wasn't sent. " + error.localizedDescription
            throw error
        }
    }

    /// The identities of the person's own agents: whose menus they may use
    /// in a chat (chat.js `menuOf`, an agent's owner by `__members`).
    var ownedIdentities: Set<String> { Set(agents.filter { $0.role == "owner" }.compactMap { identities[$0.name] }) }
    /// The word that @mentions each agent listed, by its identity: its
    /// fragment's label (chat.js `handleOf`), lowercased.
    var handles: [String: String] {
        var words: [String: String] = [:]
        for agent in agents { if let id = identities[agent.name], let label = agent.name.components(separatedBy: "--").first, !label.isEmpty { words[id] = words[id] ?? label.lowercased() } }
        return words
    }
    /// Each agent by its identity, the first of an identity's (`agents`' order).
    var agentsByIdentity: [String: FragmentSummary] {
        var faces: [String: FragmentSummary] = [:]
        for agent in agents { if let id = identities[agent.name], faces[id] == nil { faces[id] = agent } }
        return faces
    }
    func agents(in chat: FragmentSummary) -> [FragmentSummary] { (chat.agents ?? []).compactMap { agent(for: $0) } }
    func session(for chat: FragmentSummary, api: FragmentAPI) throws -> ChatSession {
        if let session = sessions[chat.name] { return session }
        // A bounded cache preserves unsent messages and cursors while changing screens.
        // A session sending Home's first message is its one owner (`launch`):
        // replaced, its chat would send around that message and save over it.
        if sessions.count >= 32, let unused = sessions.first(where: { $0.value.pending.isEmpty && $0.value.composer.isEmpty && $0.value.attachments.isEmpty && $0.value.sendingFirst == nil })?.key {
            sessions.removeValue(forKey: unused)
        }
        guard sessions.count < 32 else { throw APIError.server("Finish or discard a draft in another chat before opening more conversations.") }
        let session = ChatSession(fragment: chat, api: api)
        // its agents as the list names them, which may be newer than `chat` (a chat just made)
        if let agents = fragments.first(where: { $0.name == chat.name })?.agents { session.agents = agents }
        sessions[chat.name] = session
        return session
    }
    func load(_ api: FragmentAPI) async {
        let requestID = UUID(); loadID = requestID
        begun += 1
        let number = begun
        loading = true
        defer { if loadID == requestID { loading = false } }
        do {
            var response = try await api.fragments()
            for index in response.indices {
                if let picture = response[index].picture { response[index].picture = URL(string: picture, relativeTo: api.baseURL)?.absoluteString }
            }
            // the first list after an answer decides its page, there and then,
            // newest or not (a newer read only reads later); cancelled, as none
            decide(number, in: Task.isCancelled ? nil : response)
            guard loadID == requestID, !Task.isCancelled else { return }
            // A list read again that says the same changes nothing: what a
            // chat's view draws from it (its agents, their faces) is not
            // drawn again for each message the list is told of.
            if fragments != response { fragments = response }
            await loadComputer(api, requestID)
            for agent in agents.prefix(32) where identities[agent.name] == nil {
                let status: FragmentStatus = try await api.request("api/f/\(agent.name)/status")
                identities[agent.name] = status.npub
            }
            if loadID == requestID { error = nil }
        } catch {
            if loadID == requestID && !Task.isCancelled { self.error = error.localizedDescription }
            // its list not read, newest or not: a page held for it opens never
            decide(number, in: nil)
        }
        await loadNotices(api, requestID)
        await loadImported(api, requestID)
        await loadPages(api)
        await titleUntitled(api)
    }
    /// A page held for the read numbered `number` or an older one
    /// (`held`), decided once by the first list read after its answer
    /// (`list`; none: that read failed or was cancelled): listed there, the
    /// page is `answered`; else nothing opens, now or after.
    private func decide(_ number: Int, in list: [FragmentSummary]?) {
        guard let page = held, number >= page.after else { return }
        held = nil
        if let list { answered = fragment(linkedBy: page.page, in: list) }
    }
    /// The person's agents in their computer's order (`ComputerView.agents`,
    /// docs/api.md), as the web's sidebar takes them (cell/shell/chat-list.js
    /// `ownAgents`): its first is their main agent, first in every list of
    /// agents here, and new chats are with it. Its identities too, so only
    /// an agent not on it is asked its status. A computer that cannot be
    /// read leaves the order as it was, the list's before it was ever read.
    private func loadComputer(_ api: FragmentAPI, _ requestID: UUID) async {
        struct Computers: Decodable {
            struct Computer: Decodable { struct Agent: Decodable { let fragment: String; let identity: String }; let agents: [Agent]; let phase: String? }
            let computers: [Computer]
        }
        let read: Computers? = try? await api.request("api/computers")
        guard let read, loadID == requestID, !Task.isCancelled else { return }
        let taken = Array((read.computers.first?.agents ?? []).prefix(256))
        let order = taken.map(\.fragment)
        if agentOrder != order { agentOrder = order }
        let phase = read.computers.first?.phase
        if computerPhase != phase { computerPhase = phase }
        for agent in taken where identities[agent.fragment] != agent.identity { identities[agent.fragment] = agent.identity }
    }
    /// What they are told now, each owner's notices in its own words. A
    /// platform from before notices (404) tells none; any other failure
    /// keeps what the last read said, and the band says it couldn't refresh.
    private func loadNotices(_ api: FragmentAPI, _ requestID: UUID) async {
        struct Read: Decodable { let notices: [Notice] }
        let read: Read??
        do { read = try await api.optional(Read.self, "api/notices") } catch { read = nil }
        guard loadID == requestID, !Task.isCancelled else { return }
        noticesFailed = read == nil
        guard let read else { return }
        let told = read?.notices ?? []
        if notices != told { notices = told }
        // a refusal outlives its notice; a call that never reached the server goes with it
        failedNotices = failedNotices.filter { id, f in f.refused || told.contains { $0.id == id } }
    }
    /// Why an action failed, as its notice says it (Paul, 2026-11-09): the
    /// server's own words for its refusal, else this page's for a call that
    /// never reached it (the network).
    static func said(_ error: Error) -> String {
        switch error {
        case APIError.refused(let words), APIError.server(let words), APIError.absent(let words): words
        default: "Couldn't do that. Try again."
        }
    }
    /// Whether the server answered it, refusing (its words are `said`'s).
    static func refused(_ error: Error) -> Bool {
        switch error {
        case APIError.refused, APIError.server, APIError.absent: true
        default: false
        }
    }
    /// A notice's action: its route posted with its body, then everything
    /// read again (`load`, whose newest read wins), which decides what
    /// shows whatever the post answered. Failed, the notice says why, as
    /// that read finds it, to tap again. Its answer may name a page to open
    /// (an invite's app, a checkout), an http(s) address alone: one of
    /// theirs opens here if the first read begun after the answer lists it
    /// (`held`, `answered`); any other, it answers, for Safari.
    func act(_ notice: Notice, _ action: Notice.Action, api: FragmentAPI) async -> URL? {
        guard let post = action.post, !acting.contains(notice.id) else { return nil }
        acting.insert(notice.id)
        defer { acting.remove(notice.id) }
        var answer: JSONValue = .null
        failedNotices[notice.id] = nil
        do {
            answer = try await api.request(String(post.drop { $0 == "/" }), method: "POST", body: action.body ?? .object([:]))
        } catch {
            failedNotices[notice.id] = NoticeFailure(said: Self.said(error), refused: Self.refused(error), title: notice.title, warns: notice.warns)
        }
        let page = answer["open"].string.flatMap { URL(string: $0) }.flatMap { ["https", "http"].contains($0.scheme?.lowercased()) ? $0 : nil }
        let outside = page.flatMap { MessageLinks.fragmentName($0, pages: pages) == nil ? $0 : nil }
        if let page, outside == nil { held = (page, begun + 1) }
        await load(api)
        return outside
    }
    private func loadPages(_ api: FragmentAPI) async {
        guard pages == nil, let mine = fragments.first(where: { $0.role == "owner" }) else { return }
        let status: FragmentStatus? = try? await api.request("api/f/\(mine.name)/status")
        if let status, let found = FragmentPages(canonical: status.urls.canonical, name: mine.name) { pages = found }
    }
    /// Told the list changed: read again a moment after, so a burst (each
    /// message of a reply streaming in) is one read, as the web's
    /// `listChanged` (cell/shell/shell.js, `LIST_SETTLE_MS`).
    static let listSettle = Duration.milliseconds(200)
    func listChanged(_ api: FragmentAPI) {
        settle?.cancel()
        settle = Task { [weak self] in
            do { try await Task.sleep(for: Self.listSettle) } catch { return }
            await self?.load(api)
        }
    }
    /// A chat of the person's still titled `untitled` that has a message is
    /// titled from its first message with words, as the web titles it
    /// (cell/shell/shell.js `titleFromFirst`): its first line's words, cut
    /// to `titleMax` characters; one its manifest no longer titles so keeps
    /// its name. Once per chat a session, unless it had no words to take yet.
    /// As the web's, the check is of the manifest read, not the write: a
    /// rename landing between them is overwritten (last writer wins), as
    /// `POST files` takes no expectation of what main holds.
    static let untitled = "New chat"
    static let titleMax = 48
    private func titleUntitled(_ api: FragmentAPI) async {
        for chat in fragments where chat.kind == "chat" && chat.role == "owner" && chat.title == Self.untitled && chat.preview != nil && !titling.contains(chat.name) {
            titling.insert(chat.name)
            do {
                struct Page: Decodable { let records: [ChannelRecord] }
                let page: Page = try await api.request("api/f/\(chat.name)/channels/chat", query: [URLQueryItem(name: "after", value: "0"), URLQueryItem(name: "limit", value: "20")])
                guard let said = Self.firstSaid(page.records) else { titling.remove(chat.name); continue }
                if try await api.retitle(chat, exactly: Self.title(from: said), from: Self.untitled, message: "its title, from its first message") {
                    listChanged(api)
                }
            } catch { titling.remove(chat.name) }
        }
    }
    /// A chat's first message with words (docs/chat-records.md: a body with
    /// text and no kind but "message"), as the web's `firstSaid`.
    static func firstSaid(_ records: [ChannelRecord]) -> String? {
        records.lazy.compactMap { record -> String? in
            if let text = record.body.string { return text }
            guard (record.body["kind"].string ?? "message") == "message" else { return nil }
            return record.body["text"].string
        }.first { !jsTrimmed(Array($0.unicodeScalars)).isEmpty }
    }
    /// Its title as the web's `titleFromFirst` makes it, code point for code
    /// point: the words trimmed, split on LF alone (a CR stays, a space to
    /// the trim), its first line with words, each run of JavaScript's `\s`
    /// made one space, cut to `titleMax` code points (`[...words]`) with an
    /// ellipsis. Never Swift's Characters, which hold CRLF as one.
    static func title(from said: String) -> String {
        let lines = jsTrimmed(Array(said.unicodeScalars)).split(separator: "\n", omittingEmptySubsequences: false)
        let line = lines.first { !jsTrimmed(Array($0)).isEmpty }.map(Array.init) ?? []
        var words: [Unicode.Scalar] = []
        for scalar in line {
            if jsSpace(scalar) { if words.last != " " { words.append(" ") } } else { words.append(scalar) }
        }
        words = jsTrimmed(words)
        guard words.count > titleMax else { return String(String.UnicodeScalarView(words)) }
        let cut = jsTrimmed(Array(words.prefix(titleMax - 1)))
        return String(String.UnicodeScalarView(cut)) + "…"
    }
    /// JavaScript's white space and line terminators (`\s`, `trim()`):
    /// ECMA-262's WhiteSpace (tab, VT, FF, ZWNBSP, every Zs) and
    /// LineTerminator (LF, CR, LS, PS).
    static func jsSpace(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x09...0x0D, 0x20, 0xA0, 0x1680, 0x2000...0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF: true
        default: false
        }
    }
    static func jsTrimmed(_ scalars: [Unicode.Scalar]) -> [Unicode.Scalar] {
        guard let first = scalars.firstIndex(where: { !jsSpace($0) }), let last = scalars.lastIndex(where: { !jsSpace($0) }) else { return [] }
        return Array(scalars[first...last])
    }
    /// The person's imported chats: their session archive's `index/chat.json`
    /// (templates/sessions/README.md), apart from the rest of the list. An
    /// archive of other kinds alone (cron, subagents) has no such index:
    /// none, as the web's History reads it. A read that fails keeps what was
    /// read before, and nothing else waits on it.
    private func loadImported(_ api: FragmentAPI, _ requestID: UUID) async {
        struct Index: Decodable { let sessions: [ImportedSession] }
        guard let archive = fragments.first(where: { $0.kind == "sessions" && $0.role == "owner" }) else {
            if loadID == requestID { imported = [] }
            return
        }
        let index: Index?
        do { index = try await api.optional(Index.self, "api/f/\(archive.name)/file", query: [URLQueryItem(name: "path", value: "index/chat.json")]) }
        catch { return }
        guard loadID == requestID, !Task.isCancelled else { return }
        imported = Array((index?.sessions ?? []).prefix(2_000))
    }
    func watch(_ api: FragmentAPI) async {
        var attempt = 0
        // One foreground watcher. Cancellation closes an outstanding receive;
        // reconnect reloads the list so changes during a disconnect are included.
        while !Task.isCancelled {
            let socket: LiveSocket
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
                            listChanged(api)
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
    /// Pinned or not, as the person chose (`PUT …/pinned`), then the list again.
    func pin(_ fragment: FragmentSummary, api: FragmentAPI) async {
        do {
            let _: JSONValue = try await api.request("api/fragments/\(fragment.name)/pinned", method: "PUT", body: .object(["pinned": .bool(fragment.pinned != true)]))
            await load(api)
        } catch { self.error = error.localizedDescription }
    }
    func archive(_ fragment: FragmentSummary, api: FragmentAPI) async {
        do {
            let _: JSONValue = try await api.request("api/fragments/\(fragment.name)/archived", method: "PUT", body: .object(["archived": .bool(fragment.archived != true)]))
            await load(api)
        } catch { self.error = error.localizedDescription }
    }
}

struct FragmentStatus: Decodable { struct URLs: Decodable { let canonical: URL }; let npub: String; let urls: URLs }
enum ShellPage: Equatable { case launcher, startingChat, chat(String), history, settings, apps }

struct FragmentRoot: View {
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.openURL) private var openURL
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var library = Library()
    /// The app's one account (`FragmentApp`), shared with every other window.
    let connection: AccountConnection
    @State private var showingConnection = false
    @State private var page = ShellPage.launcher
    @State private var sidebar = false
    @State private var showingApps = false
    @State private var openedApp: FragmentSummary?
    /// The page's height, of which the notices take at most a share.
    @State private var pageHeight: CGFloat = 800

    var body: some View {
        Group {
            if let api = connection.api { shell(api).environment(\.fragmentAPI, api) }
            else { AccountConnectionView(connection: connection) }
        }
        .tint(.primary)
        .sheet(isPresented: $showingConnection) { AccountConnectionView(connection: connection, close: { showingConnection = false }) }
        .onChange(of: connection.account?.identity) { _, _ in resetNavigation() }
        .onChange(of: connection.account?.approved) { _, _ in resetNavigation() }
        .onReceive(NotificationCenter.default.publisher(for: .finiteAuthorizationFailed).receive(on: RunLoop.main)) { notification in
            guard let api = connection.api, notification.userInfo?["origin"] as? String == api.baseURL.absoluteString,
                  notification.userInfo?["key"] as? String == api.identity.publicKey else { return }
            Task { await connection.validate() }
        }
        .task(id: scenePhase) { if scenePhase == .active { await connection.validate() } }
    }
    private func resetNavigation() {
        ImageCache.shared.removeAll()
        BlobStore.shared.removeAll()
        SoundFacts.shared.removeAll()
        ShownFacts.shared.removeAll()
        AudioPlayback.shared.stop()
        library = Library(); page = .launcher; sidebar = false; showingApps = false; openedApp = nil
    }
    private func shell(_ api: FragmentAPI) -> some View {
        ZStack(alignment: .leading) {
            // What they are told sits above the whole stack, so every page
            // and every page pushed on it (Your Apps, Settings' own) has it,
            // as the web's band is over every chat and Settings: one band,
            // never one per page.
            VStack(spacing: 0) {
                NoticesBand(library: library, api: api, maxHeight: pageHeight * 0.4)
                    .background(FragmentStyle.background.ignoresSafeArea(edges: .top))
                    // Your Apps is pushed onto the page it was opened from, so
                    // Back (and swiping back) returns to that chat or page.
                    NavigationStack {
                        content(api)
                            .navigationTitle(title)
                            .navigationBarTitleDisplayMode(.inline)
                            .toolbar {
                                ToolbarItem(placement: .topBarLeading) {
                                    Button("Show sidebar", systemImage: "sidebar.left") { toggleSidebar() }.accessibilityIdentifier("sidebar")
                                }
                                ToolbarItemGroup(placement: .topBarTrailing) {
                                    // a chat has no New chat of its own: the sidebar's starts one
                                    if page == .history || page == .settings {
                                        Button("Search or start a new chat", systemImage: "square.and.pencil") { page = .launcher }.accessibilityIdentifier("launcher")
                                    }
                                    if page != .apps {
                                        Button("Your Apps", systemImage: "square.grid.2x2") { showingApps = true }.accessibilityIdentifier("yourApps")
                                    }
                                }
                            }
                            .navigationDestination(isPresented: $showingApps) {
                                AppsLibraryView(library: library, api: api, open: { openedApp = $0 })
                                    .navigationTitle("Your Apps").navigationBarTitleDisplayMode(.inline)
                            }
                    }
            }
            .accessibilityHidden(sidebar)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { pageHeight = $0 }
            SidebarDrawer(isPresented: $sidebar, library: library, page: page, api: api) { destination in
                page = destination; showingApps = false; toggleSidebar()
            }
        }
        .sheet(item: $openedApp) { app in
            if app.name == "add-app" {
                AppCreationSheet { prompt in
                    openedApp = nil; showingApps = false; library.home.text = prompt; page = .launcher
                }
            } else {
                // a link there to another fragment of theirs opens it, as a chat's link does
                FragmentWebSheet(fragment: app, api: api) { url in
                    guard let found = library.fragment(linkedBy: url) else { return false }
                    openedApp = nil; open(found)
                    return true
                }
            }
        }
        // the library's reads and its watch, for the account as it is now: a
        // library made anew (`resetNavigation`: its identity or approval
        // changed, as a check of the key finds them) reads and watches anew
        .task(id: "\(api.baseURL)|\(api.identity.publicKey)|\(connection.account?.identity ?? "")|\(connection.account?.approved == true)|\(scenePhase)") {
            if scenePhase == .active { await library.load(api); await library.watch(api) }
        }
        .onChange(of: page) { left, _ in
            if case .chat(let name) = left { library.leftChat(name) }
        }
        // a fragment an action's answer named, the read after it listing it
        .onChange(of: library.answered) { _, fragment in
            guard let fragment else { return }
            library.answered = nil
            open(fragment)
        }
        .onChange(of: showingApps) { _, presented in
            if presented { dismissKeyboard() }
        }
    }
    @ViewBuilder private func content(_ api: FragmentAPI) -> some View {
        switch page {
        case .launcher:
            HomeView(library: library, api: api, open: { open($0) }, start: { await start(api) }, refresh: { await library.load(api) }, covered: sidebar || openedApp != nil)
        // a message on its way to a chat not made yet: the launcher is already left
        case .startingChat: ChatLoadingView()
        case .chat(let name):
            if let chat = library.fragments.first(where: { $0.name == name }) {
                if let session = try? library.session(for: chat, api: api) {
                    ChatView(session: session, library: library, open: { open($0) }).id(chat.name)
                } else {
                    ContentUnavailableView("Your drafts are safe", systemImage: "square.and.pencil", description: Text("Finish or discard a draft in another chat before opening more conversations."))
                }
            } else if library.fragments.isEmpty || library.loading {
                // the list not read yet: the chat is on its way
                ChatLoadingView()
            } else {
                ContentUnavailableView("This chat isn’t in your list", systemImage: "bubble.left.and.bubble.right")
            }
        case .history: HistoryView(library: library, api: api, open: { open($0) })
        case .apps: AppsLibraryView(library: library, api: api, open: { openedApp = $0 })
        case .settings: SettingsView(library: library, api: api, connect: { showingConnection = true }, open: { open($0) })
        }
    }
    private var title: String {
        switch page {
        case .launcher: return "Finite.Computer"
        case .startingChat: return "New chat"
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
    /// Home's launch (`Library.launch`): its chat shown once made; not
    /// sent, Home again with its words, files and why, if that chat is still
    /// what is shown (moved on, Home has them when next opened).
    private func start(_ api: FragmentAPI) async {
        var shown: String?
        page = .startingChat
        do { try await library.launch(api) { chat in shown = chat.name; page = .chat(chat.name) } }
        catch { if let shown, page == .chat(shown) { page = .launcher } }
        if page == .startingChat { page = .launcher }
    }
}

enum FragmentStyle {
    static let background = Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: 24 / 255, alpha: 1) : UIColor(red: 247 / 255, green: 246 / 255, blue: 243 / 255, alpha: 1) })
    static let surface = Color(uiColor: .secondarySystemGroupedBackground)
    static let bubble = Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: 57 / 255, alpha: 1) : UIColor(red: 233 / 255, green: 230 / 255, blue: 223 / 255, alpha: 1) })
    static let lilac = Color(red: 168 / 255, green: 139 / 255, blue: 234 / 255)
}
