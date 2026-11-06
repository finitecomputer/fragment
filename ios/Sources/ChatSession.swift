import Foundation
import Observation

@MainActor @Observable
final class ChatSession {
    let fragment: FragmentSummary
    let api: FragmentAPI
    /// What was read of the chat. Views draw from what is made of it
    /// (`rows`, `drafts`, `activeTurns`), each set only when it changes: a
    /// draft's next words redraw its draft alone, never the chat's rows.
    @ObservationIgnored private(set) var timeline = Timeline() { didSet { timelineChanged() } }
    /// The records a chat shows (WorkingActivity), in order.
    private(set) var rows: [ChannelRecord] = []
    private(set) var activity = WorkingActivity(Timeline())
    private(set) var presence: [String: String] = [:]
    private(set) var awaitingReply: WorkingHint?
    @ObservationIgnored private var activityReady = false
    var workingHints: [WorkingHint] {
        guard workRead else { return [] }
        var hints = activity.hints.filter { hint in !drafts.contains { $0.turn == hint.id.id && $0.principal == hint.id.agent } }
        if let pending = awaitingReply, !hints.contains(where: { $0.id.agent == pending.id.agent }) { hints.append(pending) }
        return hints
    }
    /// Replies being written, the earliest first.
    private(set) var drafts: [ReplyDraft] = []
    private(set) var activeTurns: [String] = []
    private(set) var chatCursor: Int64 = 0
    /// Whether the chat was ever read to its end here, both its channels
    /// (`chat` and the `work` it shows): before, it is loading, never shown
    /// as an empty chat nor its backlog streaming in. Once shown, it stays.
    private(set) var read = false
    /// Whether this connection has read its `work` to its end: until then
    /// (and while none is open) a turn's start may be read without its end,
    /// as a catch-up replays them, so none is said to run (no Working…, no
    /// Stop). Each connection reads it again.
    private(set) var workRead = false
    /// Whether this connection has read its `chat` to its end.
    @ObservationIgnored private var chatReadNow = false
    @ObservationIgnored private var rowsRevision = -1
    private(set) var pending: [PendingPost] = [] { didSet { scheduleDraftSave() } }
    private(set) var connected = false
    private(set) var caughtUp = false
    private(set) var principal = ""
    private(set) var loadingHistory = false
    private(set) var hasEarlier = true
    var error: String?
    /// Files being attached, from their choosing until their upload: the
    /// composer is busy, and one attaching goes at a time (`beginAttaching`).
    private(set) var attaching = false
    var composer = "" { didSet { scheduleDraftSave() } }
    var attachments: [ChatAttachment] = [] { didSet { scheduleDraftSave() } }
    var replyTo: ChannelRecord? { didSet { scheduleDraftSave() } }
    var recipients: [String] = [] { didSet { scheduleDraftSave() } }

    @ObservationIgnored private var persistence: ChatDraftStore?
    @ObservationIgnored private var draftWrite: Task<Void, Never>?

    init(fragment: FragmentSummary, api: FragmentAPI) {
        self.fragment = fragment
        self.api = api
        do { persistence = try ChatDraftStore(api: api, fragment: fragment.name) }
        catch { self.error = "The local draft could not be opened: " + error.localizedDescription; return }
        // What was saved, within its bounds; a file that is none is cleared.
        // Either way the chat opens and sends.
        do {
            if let (saved, cut) = try persistence?.load() {
                composer = saved.composer; attachments = saved.attachments; replyTo = saved.replyTo; recipients = saved.recipients
                // A terminated send may already be committed. Retry remains an
                // explicit action, with exactly the saved id and body.
                pending = saved.pending.map { var post = $0; post.failed = true; return post }
                if cut { self.error = "Part of this chat’s saved draft was longer than a message can be, and was cut." }
            }
        } catch { self.error = error.localizedDescription }
    }
    private func timelineChanged() {
        if rowsRevision != timeline.revision || activityReady != workRead {
            rowsRevision = timeline.revision
            activityReady = workRead
            let next = WorkingActivity(timeline, ready: workRead)
            if next != activity { activity = next }
            if !next.rows.elementsEqual(rows, by: { $0.id == $1.id }) { rows = next.rows }
            let candidate = WorkingActivity.pending(timeline, principal: principal, lead: fragment.agents?.first)
            if let candidate, case .pending(let since) = candidate.kind,
               candidate.id == awaitingReply?.id || Int64(Date().timeIntervalSince1970 * 1000) - since < 90_000 { awaitingReply = candidate }
            else { awaitingReply = nil }
        }
        let writing = timeline.drafts.values.sorted { ($0.at, $0.turn) < ($1.at, $1.turn) }
        if writing != drafts { drafts = writing }
        let running = workRead ? timeline.activeTurns : []
        if running != activeTurns { activeTurns = running }
        let cursor = timeline.cursors["chat"] ?? 0
        if cursor != chatCursor { chatCursor = cursor }
        // What else a view draws from the timeline is made here the same
        // way: projected from it, and set only when it changes.
    }
    private var saved: SavedChat { SavedChat(composer: composer, attachments: attachments, replyTo: replyTo, recipients: recipients, pending: pending) }
    private func scheduleDraftSave() {
        guard persistence != nil else { return }
        draftWrite?.cancel()
        draftWrite = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(300)) } catch { return }
            self?.flushDraft()
        }
    }
    func flushDraft() {
        draftWrite?.cancel(); draftWrite = nil
        do { try persistence?.save(saved) }
        catch { self.error = "The local draft could not be saved: " + error.localizedDescription }
    }

    /// The composer as typed or pasted, kept to what a message (and its
    /// saved draft) can hold: past it, cut at a character, and said so.
    func edit(_ text: String) {
        let (kept, cut) = SavedChat.fitted(text)
        composer = kept
        if cut { error = "A message is at most 32 KB: the rest was cut." }
    }

    /// A refused send's words back in the composer, to change and send
    /// anew, once the composer is empty (`deliver`).
    func reopen(_ post: PendingPost) {
        guard let index = pending.firstIndex(where: { $0.id == post.id }), pending[index].refused != nil else { return }
        guard composer.isEmpty && attachments.isEmpty else { error = "Send or clear your draft first, then edit this message."; return }
        let refused = pending.remove(at: index)
        // its own words, files and addressees: the draft's were kept until now
        composer = refused.text; attachments = refused.attachments; recipients = refused.recipients
        replyTo = timeline.records.first { $0.channel == "chat" && $0.seq == refused.replyTo }
        error = nil
    }

    // Owned by the view's .task: backgrounding or leaving the chat closes the
    // socket, including a suspended receive. Each foreground visit catches up.
    func run() async {
        var attempt = 0
        while !Task.isCancelled {
            let socket: LiveSocket
            do { socket = try api.socket(fragment.name) }
            catch { self.error = error.localizedDescription; return }
            var refused = false
            socket.resume()
            do {
                try await withTaskCancellationHandler {
                    let heartbeat = Task {
                        while !Task.isCancelled {
                            try await Task.sleep(for: .seconds(20))
                            try await socket.send(.string("{\"type\":\"ping\"}"))
                        }
                    }
                    defer { heartbeat.cancel() }
                    while !Task.isCancelled {
                        let message = try await socket.receive()
                        let data: Data
                        switch message {
                        case .string(let text): data = Data(text.utf8)
                        case .data(let bytes): data = bytes
                        @unknown default: throw APIError.invalidResponse
                        }
                        let frame = try JSONDecoder().decode(LiveFrame.self, from: data)
                        if frame.type == "hello" {
                            chatReadNow = false
                            if workRead { workRead = false; timelineChanged() }
                            presence = Dictionary((frame.presence ?? []).prefix(256).filter { $0.data != nil && $0.data != .null }.map { ($0.id, $0.principal) }, uniquingKeysWith: { _, last in last })
                            principal = frame.principal ?? ""
                            connected = true
                            error = nil
                            attempt = 0
                            for channel in ["chat", "work"] {
                                try await subscribe(socket, channel: channel, after: timeline.cursors[channel])
                            }
                        }
                        if frame.type == "presence", let id = frame.id {
                            if frame.data == nil || frame.data == .null { presence.removeValue(forKey: id) }
                            else if let principal = frame.principal, presence.count < 256 || presence[id] != nil { presence[id] = principal }
                        }
                        try timeline.apply(frame)
                        if let clientID = frame.body?["client_id"].string, frame.principal == principal {
                            pending.removeAll { $0.id == clientID }
                        }
                        if frame.type == "subscribed", let channel = frame.channel {
                            if frame.more == true {
                                try await subscribe(socket, channel: channel, after: frame.next)
                            } else if channel == "chat" {
                                caughtUp = true
                                chatReadNow = true
                                if workRead && !read { read = true }
                                if timeline.records.filter({ $0.channel == "chat" }).count < Self.chatLast { hasEarlier = false }
                            } else if channel == "work" {
                                if !workRead { workRead = true; timelineChanged() }
                                if chatReadNow && !read { read = true }
                                await earlierWork()
                            }
                        }
                    }
                } onCancel: {
                    socket.cancel(with: .goingAway, reason: nil)
                }
            } catch {
                let status = (socket.response as? HTTPURLResponse)?.statusCode
                if status == 401 { api.authorizationFailed() }
                refused = status == 401 || status == 403
                if socket.closeCode.rawValue == 4003 {
                    do { let _: JSONValue = try await api.request("api/identities/me") }
                    catch APIError.unauthorized { refused = true }
                    catch { /* A transient check failure can retry with the usual backoff. */ }
                }
                if !Task.isCancelled { self.error = refused ? "Your access ended. Reconnect your account or check sharing permissions." : "Connection interrupted. Reconnecting…" }
            }
            socket.cancel(with: .goingAway, reason: nil)
            connected = false
            caughtUp = false
            chatReadNow = false
            // no turn is said to run while none is heard; the next connection
            // says again which do, once its work is read
            workRead = false
            timeline.disconnected()
            presence = [:]
            guard !Task.isCancelled, !refused else { return }
            attempt = min(attempt + 1, 4)
            do { try await Task.sleep(for: .seconds(min(15, 1 << attempt))) }
            catch { return }
        }
    }

    /// The backlog a chat first reads: its last messages, and a page of
    /// `work` (`CHANNEL_PAGE`), which holds a long turn's steps (up to 200
    /// calls and 200 steps) and, mostly, its start (`earlierWork`).
    static let chatLast = 50
    static let workPage = 1_000
    /// Earlier `work` is read first in batches of this many: each record at
    /// most `RECORD_BODY_MAX_BYTES` (64 KiB), so a batch stays within the
    /// 4 MiB an answer may be here (`FragmentAPI.historyBytesMax`), which the
    /// HTTP route, bounding its pages by count alone, does not promise.
    static let workBatch = 50
    /// Earlier `work` read back at most, a visit.
    static let earlierWorkMax = 1_000

    /// The next batch, sized by what the last one's records took: as many
    /// as half the answer's bound holds at twice their mean, between
    /// `workBatch` and `earlierWorkMax`. One that still overflows is read
    /// again at `workBatch`.
    static func nextBatch(after count: Int, bytes: Int) -> Int {
        guard count > 0 else { return workBatch }
        let mean = max(bytes / count, 256)
        return min(earlierWorkMax, max(workBatch, FragmentAPI.historyBytesMax / 2 / (2 * mean)))
    }

    private func subscribe(_ socket: LiveSocket, channel: String, after: Int64?) async throws {
        var body: [String: JSONValue] = ["type": .string("subscribe"), "channel": .string(channel)]
        if let after { body["after"] = .number(Double(after)) }
        else { body["last"] = .number(Double(channel == "work" ? Self.workPage : Self.chatLast)) }
        let data = try JSONEncoder().encode(JSONValue.object(body))
        try await socket.send(.string(String(decoding: data, as: UTF8.self)))
    }

    /// Earlier `work`, read back as the web's chat page reads it
    /// (templates/chat/site/chat.js `earlierWork`), only as far as the turns
    /// in the window need: to the oldest message shown, so those turns have
    /// their steps (inserted), and while a turn that may run was read
    /// without its start, so it shows running, and Stop, however the chat
    /// was opened (learned). A turn whose end was read needs nothing more.
    /// Batches grow with how little their records take (`nextBatch`), so a
    /// visit is a call or two, not twenty; bounded by `earlierWorkMax`. A
    /// failure leaves it for the next visit, and says so.
    private func earlierWork() async {
        var read = 0
        var batch = Self.workBatch
        while read < Self.earlierWorkMax, let oldest = timeline.oldestWork, oldest.seq > 1 {
            let shownFrom = timeline.records.first { $0.channel == "chat" }?.at
            guard timeline.awaitsAStart || shownFrom.map({ oldest.at >= $0 }) == true else { return }
            let limit = min(batch, Self.earlierWorkMax - read)
            let page: (records: [ChannelRecord], bytes: Int)
            do { page = try await api.historyPage(fragment.name, channel: "work", before: oldest.seq, limit: limit) }
            catch APIError.limitExceeded where limit > Self.workBatch { batch = Self.workBatch; continue }
            catch { self.error = "Earlier progress could not be read: " + error.localizedDescription; return }
            for record in page.records {
                if let shownFrom, record.at >= shownFrom { timeline.insert(record, advancingCursor: false) } else { timeline.learn(record) }
            }
            read += page.records.count
            if page.records.count < limit { return }
            batch = Self.nextBatch(after: page.records.count, bytes: page.bytes)
        }
    }

    /// Reserves the attaching of `count` files, at once, before anything is
    /// read: none while others are attaching, none past `countMax` a message.
    /// Then `attach` reads and uploads them; until it ends, nothing sends.
    func beginAttaching(_ count: Int) -> Bool {
        guard !attaching else { error = "Wait for the files being attached, then add more."; return false }
        guard count > 0, count + attachments.count <= ChatAttachment.countMax else { error = "Attach at most eight files per message."; return false }
        attaching = true
        return true
    }
    /// The files `prepare` reads (off the main actor, its own), each put by
    /// its hash and held as the message's attachment; ends what
    /// `beginAttaching` began.
    func attach(_ prepare: @escaping @Sendable () async throws -> [OutgoingFile]) async {
        defer { attaching = false }
        do {
            let files = try await prepare()
            guard !files.isEmpty else { error = "This could not be attached."; return }
            guard files.count + attachments.count <= ChatAttachment.countMax else { error = "Attach at most eight files per message."; return }
            for file in files {
                let put = try await api.upload(file.data, name: file.name, type: file.type, to: fragment.name)
                if !attachments.contains(where: { $0.id == put.id }) { attachments.append(put) }
            }
        } catch { self.error = error.localizedDescription }
    }

    func send() async {
        // what is being attached goes with it, or the message waits
        guard !attaching else { error = "Wait for the files being attached."; return }
        guard persistence != nil else {
            error = "The local draft could not be opened. Reopen this chat before sending."
            return
        }
        let text = composer.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !attachments.isEmpty else { return }
        guard pending.count < SavedChat.pendingMax else { error = "Wait for the messages still sending, or edit the ones not sent."; return }
        let post = PendingPost(id: UUID().uuidString, text: text, attachments: attachments, replyTo: replyTo?.seq, recipients: recipients)
        do {
            // Persist the exact send before any request leaves the phone.
            let next = SavedChat(composer: "", attachments: [], replyTo: nil, recipients: recipients, pending: pending + [post])
            try persistence?.save(next)
        } catch { self.error = "The message could not be saved on this iPhone: " + error.localizedDescription; return }
        pending.append(post)
        composer = ""; attachments = []; replyTo = nil
        await deliver(post)
    }

    func retry(_ post: PendingPost) async {
        guard let index = pending.firstIndex(where: { $0.id == post.id }), pending[index].failed, pending[index].refused == nil else { return }
        pending[index].failed = false
        await deliver(post)
    }

    private func deliver(_ post: PendingPost) async {
        do {
            var body: [String: JSONValue] = ["text": .string(post.text), "client_id": .string(post.id)]
            if !post.attachments.isEmpty { body["attachments"] = .array(post.attachments.map(\.json)) }
            if let replyTo = post.replyTo { body["reply_to"] = .number(Double(replyTo)) }
            if !post.recipients.isEmpty { body["to"] = .array(post.recipients.map { .string($0) }) }
            let record = try await api.post(fragment.name, id: post.id, body: .object(body))
            timeline.insert(record, advancingCursor: false)
            pending.removeAll { $0.id == post.id }
            error = nil
        } catch APIError.refused(let message), APIError.absent(let message) {
            // Refused, not lost: sending it again gets the same answer, so
            // it is never retried. Its words go back to an empty composer;
            // beside a draft, it waits to be edited (`reopen`), as a draft
            // and it joined might be more than a message holds.
            guard let index = pending.firstIndex(where: { $0.id == post.id }) else { return }
            pending[index].failed = true
            pending[index].refused = message
            reopen(pending[index])
            self.error = message
        } catch {
            // A matching live record also acknowledges delivery. The HTTP
            // acknowledgement may have been lost after the append committed.
            guard let index = pending.firstIndex(where: { $0.id == post.id }) else { return }
            pending[index].failed = true
            self.error = error.localizedDescription
        }
    }

    func stop() async {
        for turn in timeline.activeTurns.prefix(16) {
            do {
                let record = try await api.post(fragment.name, id: "stop:\(turn)", body: .object([
                    "kind": .string("stop"), "turn": .string(turn)
                ]))
                timeline.insert(record, advancingCursor: false)
            } catch { self.error = error.localizedDescription }
        }
    }

    func loadEarlier() async {
        guard !loadingHistory, let first = timeline.records.filter({ $0.channel == "chat" }).map(\.seq).min() else {
            hasEarlier = false
            return
        }
        loadingHistory = true
        defer { loadingHistory = false }
        do {
            let page = try await api.history(fragment.name, before: first)
            for record in page { timeline.insert(record, advancingCursor: false) }
            hasEarlier = page.count == 50 && timeline.chatCount < Timeline.chatMax
        } catch { self.error = error.localizedDescription }
    }
}
