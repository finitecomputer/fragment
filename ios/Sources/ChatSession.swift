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
    /// A page of the window being read (older or newer): one at a time.
    private(set) var paging = false
    /// Whether the server holds messages before the window's oldest (a full
    /// page answered last; unknown is yes).
    private(set) var hasOlder = true
    /// Whether the window has let its newest end go (`Timeline.detached`).
    private(set) var detached = false
    /// The window's oldest message: what the top sentinel pages back from.
    private(set) var oldestMessage: String?
    /// Messages heard live since the chat was read, counted whether or not
    /// they are in the window: what the Latest pill counts from.
    private(set) var heard = 0
    /// The newest message counted toward `heard` (from the chat's first
    /// read on): a catch-up's replay counts as live, and nothing twice.
    @ObservationIgnored private var heardThrough: Int64?
    /// The window's oldest message as last seen: when it moves on (its
    /// oldest end evicted), there are older messages again.
    @ObservationIgnored private var oldestSeq: Int64?
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
        if timeline.detached != detached { detached = timeline.detached }
        let oldest = timeline.oldestChat?.id
        if oldest != oldestMessage { oldestMessage = oldest }
        let oldestNow = timeline.oldestChat?.seq
        if let was = oldestSeq, let now = oldestNow, now > was, !hasOlder { hasOlder = true }
        oldestSeq = oldestNow
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
    /// How long a reconnect goes unsaid: back within it, nothing is shown.
    var reconnectNoticeAfter = Duration.seconds(5)
    static let reconnecting = "Reconnecting…"
    /// The web's chat's words for a socket closed for good (chat.js `closed`).
    static let deleted = "This chat was deleted."
    static let accessChanged = "Your access to this chat changed."
    @ObservationIgnored private var reconnectNotice: Task<Void, Never>?

    /// Why the chat is closed for good (deleted, its access changed, the
    /// key ended), once it is: nothing opens it again, not a return to the
    /// app nor its view coming back. Only a new session, made once the chat
    /// is left and opened again, asks anew (`Library.leftChat`).
    private(set) var closed: String?

    func run() async {
        if let closed { error = closed; return }
        // failures since the last hello: the first reconnects at once (a
        // socket closed as its object sleeps, 4001, or the network dropped),
        // only repeated ones back off
        var attempt = 0
        // a connection closed within this of its hello was no connection: a
        // server opening and closing at once is backed off from, not looped on
        let lastsAtLeast = Duration.seconds(5), clock = ContinuousClock()
        var openedAt: ContinuousClock.Instant?
        defer { reconnectNotice?.cancel(); reconnectNotice = nil; if error == Self.reconnecting { error = nil } }
        while !Task.isCancelled {
            let socket: LiveSocket
            do { socket = try api.socket(fragment.name) }
            catch { self.error = error.localizedDescription; return }
            var refused = false
            var ended: String?
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
                            reconnectNotice?.cancel(); reconnectNotice = nil
                            if error == Self.reconnecting { error = nil }
                            openedAt = clock.now
                            for channel in ["chat", "work"] {
                                try await subscribe(socket, channel: channel, after: timeline.cursors[channel])
                            }
                        }
                        if frame.type == "presence", let id = frame.id {
                            if frame.data == nil || frame.data == .null { presence.removeValue(forKey: id) }
                            else if let principal = frame.principal, presence.count < 256 || presence[id] != nil { presence[id] = principal }
                        }
                        if frame.type == "record", let record = frame.record, record.isMessage, let through = heardThrough, record.seq > through {
                            heard += 1
                            heardThrough = record.seq
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
                                if heardThrough == nil { heardThrough = timeline.cursors["chat"] ?? 0 }
                                if workRead && !read { read = true }
                                if timeline.chatCount < Self.chatLast && !timeline.detached { hasOlder = false }
                            } else if channel == "work" {
                                if !workRead { workRead = true; timelineChanged() }
                                if chatReadNow && !read { read = true }
                                await earlierWork()
                                // drafts kept across the reconnect go once their turn is read to have ended
                                timeline.settleDrafts()
                            }
                        }
                    }
                } onCancel: {
                    socket.cancel(with: .goingAway, reason: nil)
                }
            } catch {
                let status = (socket.response as? HTTPURLResponse)?.statusCode
                let code = socket.closeCode.rawValue
                if status == 401 { api.authorizationFailed() }
                // final, never reconnected: the chat deleted (4004, or an upgrade
                // 404), access to it changed (4003, or 403), the key's ended (401),
                // said as the web's chat says it
                if code == 4004 || status == 404 { ended = Self.deleted }
                else if code == 4003 || status == 403 { ended = Self.accessChanged }
                else if status == 401 { ended = "Your access ended. Reconnect your account or check sharing permissions." }
                refused = ended != nil
                if let ended { closed = ended; self.error = ended }
            }
            socket.cancel(with: .goingAway, reason: nil)
            connected = false
            caughtUp = false
            chatReadNow = false
            // no turn is said to run while none is heard; the next connection
            // says again which do, once its work is read
            workRead = false
            // a reply being written stays on screen until the next connection
            // says what became of it (`settleDrafts`)
            timelineChanged()
            presence = [:]
            if refused {
                // no connection follows to say what became of a reply being written
                reconnectNotice?.cancel(); reconnectNotice = nil
                timeline.dropDrafts()
            }
            guard !Task.isCancelled, !refused else { return }
            // said only if it is not back soon, so the words are true
            if reconnectNotice == nil {
                let after = reconnectNoticeAfter
                reconnectNotice = Task { [weak self] in
                    do { try await Task.sleep(for: after) } catch { return }
                    guard let self, !self.connected, self.error == nil else { return }
                    self.error = Self.reconnecting
                }
            }
            if let opened = openedAt, clock.now - opened >= lastsAtLeast { attempt = 0 }
            openedAt = nil
            attempt += 1
            if attempt > 1 {
                do { try await Task.sleep(for: .seconds(min(15, 1 << min(attempt - 1, 4)))) }
                catch { return }
            }
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
        guard count > 0, count + attachments.count <= ChatAttachment.countMax else { error = ChatAttachment.limits.countRefusal; return false }
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
            guard files.count + attachments.count <= ChatAttachment.countMax else { error = ChatAttachment.limits.countRefusal; return }
            for file in files {
                let put = try await api.upload(file.data, name: file.name, type: file.type, to: fragment.name)
                if !attachments.contains(where: { $0.id == put.id }) { attachments.append(put) }
            }
        } catch { self.error = error.localizedDescription }
    }

    func send() async {
        // what is being attached goes with it, or the message waits
        guard !attaching else { error = "Wait for the files being attached."; return }
        // a send goes at the chat's end: a window paged back returns there first
        if timeline.detached { await resetToLatest() }
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

    enum Paging { case older, newer }
    /// A page read as the chat is scrolled to an end, in a task of the
    /// session's own: the view's sentinel that asks for it is restarted as
    /// paging starts and ends, which must not cancel the read it asked for.
    func page(_ direction: Paging) {
        guard !paging else { return }
        Task { direction == .older ? await loadOlder() : await loadNewer() }
    }

    /// Messages before the window's oldest, a page, as the chat is scrolled
    /// up to its top (the web's sentinel; FIN-415), and the work their turns
    /// show. Past `Timeline.chatMax`, the window's newest end goes.
    func loadOlder() async {
        guard !paging, hasOlder, read, let oldest = timeline.oldestChat else { return }
        paging = true
        defer { paging = false }
        do {
            let page = try await api.history(fragment.name, before: oldest.seq, limit: Self.page)
            hasOlder = page.count == Self.page
            timeline.insertOlder(page)
            if let from = timeline.oldestChat?.at { await readWork(back: from) }
        } catch { self.error = error.localizedDescription }
    }

    /// Messages after the window's newest, a page, as a window paged back is
    /// scrolled down to its end; once it reaches the newest heard, it is the
    /// chat's again.
    func loadNewer() async {
        guard !paging, timeline.detached, let newest = timeline.newestChat else { return }
        paging = true
        defer { paging = false }
        do {
            let page = try await api.historyPage(fragment.name, channel: "chat", after: newest.seq, limit: Self.page).records
            // its work from the newest learned at the window's end, whether or
            // not the window holds any: through the page's last message, or,
            // a last page, through the newest work heard (steps, a prompt, an
            // end after the newest message), so the chat rejoined is whole
            let last = page.count < Self.page ? Int64.max : page.last?.at ?? newest.at
            let from = timeline.workSeq(atOrBefore: newest.at) ?? 0
            let work = await readWork(after: from, upTo: last).filter { $0.at > newest.at }
            // whether it reaches the newest, decided after the reads: a message
            // heard meanwhile waits outside the window, and is read next
            let reaches = page.count < Self.page && (page.last?.seq ?? newest.seq) >= (timeline.cursors["chat"] ?? 0)
            timeline.insertNewer(page + work, reachesNewest: reaches)
        } catch { self.error = error.localizedDescription }
    }

    /// The chat's newest page in place of the window (Latest messages, or a
    /// send from a window paged back): whether turns run stays.
    func resetToLatest() async {
        guard !paging, timeline.detached else { return }
        paging = true
        defer { paging = false }
        do {
            // read again while messages come meanwhile, so none heard is left out
            var page: [ChannelRecord] = []
            var whole = false
            for _ in 0..<3 {
                let cursor = timeline.cursors["chat"] ?? 0
                page = try await api.history(fragment.name, before: cursor + 1, limit: Self.chatLast)
                if (timeline.cursors["chat"] ?? 0) == cursor { whole = true; break }
            }
            // messages still coming after three reads: it stays paged back, and
            // its end's sentinel reads on to the newest, never claiming it
            timeline.reset(page, live: whole)
            hasOlder = page.count == Self.chatLast
            // its work back from the newest heard (steps after its newest message too)
            if let from = timeline.oldestChat?.at { await readWork(back: from, through: whole ? .max : nil) }
        } catch { self.error = error.localizedDescription }
    }

    /// `work` before the window's oldest held (or from the newest learned at
    /// its end, if it holds none), back to `from`: kept in the window between
    /// `from` and its newest message, learned otherwise. Batches as `earlierWork` sizes them; at most
    /// `earlierWorkMax`.
    private func readWork(back from: Int64, through: Int64? = nil) async {
        // from the window's oldest work, else the newest learned at its end (it may hold none)
        let end = through ?? timeline.newestChat?.at ?? .max
        var before = timeline.records.first { $0.channel == "work" }?.seq
            ?? (timeline.workSeq(atOrBefore: end) ?? (timeline.cursors["work"] ?? 0)) + 1
        var read = 0, batch = Self.workBatch
        while read < Self.earlierWorkMax, before > 1 {
            let limit = min(batch, Self.earlierWorkMax - read)
            let page: (records: [ChannelRecord], bytes: Int)
            do { page = try await api.historyPage(fragment.name, channel: "work", before: before, limit: limit) }
            catch APIError.limitExceeded where limit > Self.workBatch { batch = Self.workBatch; continue }
            catch { self.error = "Earlier progress could not be read: " + error.localizedDescription; return }
            // the window's own stretch kept; the rest only says which turns run
            timeline.insertOlder(page.records.filter { $0.at >= from && $0.at <= end })
            for record in page.records where record.at < from || record.at > end { timeline.learn(record) }
            read += page.records.count
            guard let first = page.records.first, page.records.count == limit, first.at >= from else { return }
            before = first.seq
            batch = Self.nextBatch(after: page.records.count, bytes: page.bytes)
        }
    }

    /// `work` after `from`, up to the time `upTo` (a newer page's newest
    /// message): what that page's turns show.
    private func readWork(after from: Int64, upTo: Int64) async -> [ChannelRecord] {
        var after = from, kept: [ChannelRecord] = [], batch = Self.workBatch
        while kept.count < Self.earlierWorkMax {
            let limit = min(batch, Self.earlierWorkMax - kept.count)
            let page: (records: [ChannelRecord], bytes: Int)
            do { page = try await api.historyPage(fragment.name, channel: "work", after: after, limit: limit) }
            catch APIError.limitExceeded where limit > Self.workBatch { batch = Self.workBatch; continue }
            catch { self.error = "Later progress could not be read: " + error.localizedDescription; return kept }
            kept += page.records.filter { $0.at <= upTo }
            guard let last = page.records.last, page.records.count == limit, last.at <= upTo else { return kept }
            after = last.seq
            batch = Self.nextBatch(after: page.records.count, bytes: page.bytes)
        }
        return kept
    }

    /// A page of messages, read older or newer as the chat is scrolled.
    static let page = 50
}
