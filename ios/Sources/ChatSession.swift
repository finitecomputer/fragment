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
    /// What a chat shows (WorkingActivity), in the web's order: its
    /// records, and the replies being written and working lines in their places.
    private(set) var rows: [ChatItem] = []
    private(set) var activity = WorkingActivity(Timeline())
    private(set) var presence: [String: String] = [:]
    private(set) var awaitingReply: WorkingHint?
    @ObservationIgnored private var activityReady = false
    /// The replies being written, by turn, as the activity was last made
    /// with them: their words change nothing of it, their coming and going does.
    @ObservationIgnored private var activityDrafting: Set<TurnKey> = []
    @ObservationIgnored private var activityPrincipal = ""
    /// The identities of the person's own agents (`Library`): whose menus
    /// they may use (chat.js `menuOf`, by `__members`' owner) and whose
    /// routines' turns are theirs (control.js `controllerOf`).
    var owned: Set<String> = [] { didSet { if owned != oldValue { activityOwned = nil; timelineChanged() } } }
    @ObservationIgnored private var activityOwned: Set<String>?
    /// The word that @mentions each agent the person's list names, by its
    /// identity: its fragment's label (chat.js `handleOf`, bridge `answers_to`).
    var handles: [String: String] = [:] { didSet { if handles != oldValue { activityHandles = nil; timelineChanged() } } }
    @ObservationIgnored private var activityHandles: [String: String]?
    /// The chat's agents, its lead first, as the person's list names them
    /// now (`Library.fragments`), not as when it opened: whom a message is
    /// for, so one added through Agents in this chat counts at once.
    var agents: [String] = [] { didSet { if agents != oldValue { activityAgents = nil; timelineChanged() } } }
    @ObservationIgnored private var activityAgents: [String]?
    /// A post awaiting its agent's claim, at the chat's end (the web's
    /// pending line): each running turn's line is in the rows.
    var workingHints: [WorkingHint] {
        guard workRead, let pending = awaitingReply, !activity.hints.contains(where: { $0.id.agent == pending.id.agent }) else { return [] }
        return [pending]
    }
    /// Replies being written, the earliest first.
    private(set) var drafts: [ReplyDraft] = []
    private(set) var activeTurns: [String] = []
    /// Each agent's menu, its commands' names (`Timeline.menus`), as the
    /// composer observes it: a menu that comes or changes alone redraws Steer.
    private(set) var menus: [String: [String]] = [:]
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
    /// Files being put, or that failed to be: each shown in the composer
    /// from its own bytes on this iPhone (`BlobStore.keep`) as it goes, then
    /// held as an attachment once put. A failed one waits to be put again
    /// (`retryUpload`) or removed, and the message waits with it.
    private(set) var uploads: [Upload] = []
    var composer = "" { didSet { scheduleDraftSave() } }
    var attachments: [ChatAttachment] = [] { didSet { scheduleDraftSave() } }
    var replyTo: ChannelRecord? { didSet { scheduleDraftSave() } }
    var recipients: [String] = [] { didSet { scheduleDraftSave() } }

    @ObservationIgnored private var persistence: ChatDraftStore?
    @ObservationIgnored private var draftWrite: Task<Void, Never>?

    init(fragment: FragmentSummary, api: FragmentAPI) {
        self.fragment = fragment
        self.api = api
        agents = fragment.agents ?? []
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
        // a reply being written is at the chat's end, which a window paged back does not show
        let drafting = timeline.detached ? [] : Set(timeline.drafts.values.map { TurnKey(agent: $0.principal, id: $0.turn) })
        if rowsRevision != timeline.revision || activityReady != workRead || activityDrafting != drafting
            || activityPrincipal != principal || activityOwned != owned || activityHandles != handles || activityAgents != agents {
            rowsRevision = timeline.revision
            activityReady = workRead
            activityDrafting = drafting
            activityPrincipal = principal
            activityOwned = owned
            activityHandles = handles
            activityAgents = agents
            let next = WorkingActivity(timeline, ready: workRead, drafting: drafting, me: principal, owned: owned, agents: agents, handles: handles)
            if next != activity { activity = next }
            if next.rows != rows { rows = next.rows }
            let candidate = WorkingActivity.pending(timeline, principal: principal, lead: agents.first)
            if let candidate, case .pending(let since) = candidate.kind,
               candidate.id == awaitingReply?.id || Int64(Date().timeIntervalSince1970 * 1000) - since < 90_000 { awaitingReply = candidate }
            else { awaitingReply = nil }
        }
        let writing = timeline.drafts.values.sorted { ($0.at, $0.turn) < ($1.at, $1.turn) }
        if writing != drafts { drafts = writing }
        let running = workRead ? timeline.activeTurns : []
        if running != activeTurns { activeTurns = running }
        let offered = timeline.menus.mapValues(\.commands)
        if offered != menus { menus = offered }
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
        // nor while files are being attached: their places are held (`beginAttaching`)
        guard composer.isEmpty && attachments.isEmpty && uploads.isEmpty && !attaching else { error = "Send or clear your draft first, then edit this message."; return }
        let refused = pending.remove(at: index)
        // its own words, files and addressees: the draft's were kept until
        // now (a command's words alone: its agent is no addressee of theirs)
        composer = refused.text; attachments = refused.attachments
        if refused.command == nil { recipients = refused.recipients }
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

    /// The message's files: attached, being put, or failed to be put (which
    /// waits to go with it). Each takes one of a message's `countMax` places;
    /// every check of what more it takes counts this, and only this.
    var fileCount: Int { attachments.count + uploads.count }
    /// Places the message has left (the photo picker's most).
    var fileRoom: Int { max(0, ChatAttachment.countMax - fileCount) }
    /// Whether `count` more files may be attached now, saying why not.
    func admits(_ count: Int) -> Bool {
        guard !attaching else { error = "Wait for the files being attached, then add more."; return false }
        guard count > 0, count <= fileRoom else { error = ChatAttachment.limits.countRefusal; return false }
        return true
    }
    /// Reserves the attaching of `count` files, at once, before anything is
    /// read: none while others are attaching, none past `countMax` a message.
    /// Then `attach` reads and uploads them; until it ends, nothing sends.
    /// A voice memo reserves its place before it records (`VoiceMemo.file`),
    /// so one is refused before it starts, never once it is recorded.
    func beginAttaching(_ count: Int) -> Bool {
        guard admits(count) else { return false }
        attaching = true
        return true
    }
    /// The files `prepare` reads (off the main actor, its own), each put by
    /// its hash and held as the message's attachment; ends what
    /// `beginAttaching` began.
    func attach(_ prepare: @escaping @Sendable () async throws -> [OutgoingFile]) async {
        defer { attaching = false }
        let files: [OutgoingFile]
        do { files = try await prepare() }
        // a voice memo cancelled, or never begun (`VoiceMemo.file`): nothing to say
        catch is CancellationError { return }
        catch { self.error = error.localizedDescription; return }
        guard !files.isEmpty else { error = "This could not be attached."; return }
        guard files.count <= fileRoom else { error = ChatAttachment.limits.countRefusal; return }
        var queued: [(OutgoingFile, ChatAttachment)] = []
        for file in files {
            let sha256 = await Task.detached(priority: .userInitiated) { sha256Hex(file.data) }.value
            let attachment = ChatAttachment(sha256: sha256, name: file.name, type: file.type, size: file.data.count)
            guard !attachments.contains(where: { $0.id == attachment.id }), !uploads.contains(where: { $0.id == attachment.id }),
                  !queued.contains(where: { $0.1.id == attachment.id }) else { continue }
            // kept as it is, so it shows at once (and once sent) from these bytes;
            // one not kept shows once it is read back
            do { try await BlobStore.shared.keep(file.data, key: BlobKey(api: api, fragment: fragment.name, sha256: attachment.sha256)) } catch {}
            uploads.append(Upload(attachment: attachment))
            queued.append((file, attachment))
        }
        for (file, attachment) in queued { await put(file.data, attachment) }
    }

    /// A failed upload put again, from its bytes kept on this iPhone.
    func retryUpload(_ id: String) async {
        guard !attaching, let upload = uploads.first(where: { $0.id == id }), upload.failed != nil else { return }
        attaching = true
        defer { attaching = false }
        let attachment = upload.attachment
        let kept = BlobStore.shared.cached(BlobKey(api: api, fragment: fragment.name, sha256: attachment.sha256))
        let read = await Task.detached(priority: .userInitiated) { kept.flatMap { try? Data(contentsOf: $0) } }.value
        // removed meanwhile: nothing to put
        guard let index = uploads.firstIndex(where: { $0.id == id }) else { return }
        guard let data = read, data.count == attachment.size else {
            uploads[index].failed = "This file is no longer on this iPhone. Remove it and attach it again."
            return
        }
        uploads[index].failed = nil
        uploads[index].progress = 0
        await put(data, attachment)
    }

    func removeUpload(_ id: String) {
        guard !attaching || uploads.first(where: { $0.id == id })?.failed != nil else { return }
        uploads.removeAll { $0.id == id }
    }

    private func put(_ data: Data, _ attachment: ChatAttachment) async {
        let id = attachment.id
        do {
            let put = try await api.upload(data, name: attachment.name, type: attachment.type, to: fragment.name) { [weak self] fraction in
                Task { @MainActor in
                    guard let self, let index = self.uploads.firstIndex(where: { $0.id == id }),
                          fraction - self.uploads[index].progress >= 0.02 || fraction >= 1 else { return }
                    self.uploads[index].progress = fraction
                }
            }
            uploads.removeAll { $0.id == id }
            if !attachments.contains(where: { $0.id == put.id }) { attachments.append(put) }
        } catch {
            if let index = uploads.firstIndex(where: { $0.id == id }) { uploads[index].failed = error.localizedDescription }
        }
    }

    func send() async {
        // what is being attached goes with it, or the message waits
        guard !attaching else { error = "Wait for the files being attached."; return }
        guard !uploads.contains(where: { $0.failed != nil }) else { error = "A file did not upload. Retry it or remove it."; return }
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

    /// A new chat's first message from the home screen, as submitted there
    /// (`Library.launch`): its files, held on the phone until this chat was
    /// made, put in this chat's blobs, then its words and them posted as one
    /// message through the outbox, as a message is sent here (saved with
    /// `id` before any request; the web's launcher, `startFromSearch`).
    /// Each upload is checked as it answers: one that fails throws, and
    /// nothing is posted, so the whole message is tried again, never words
    /// alone or some of its files. Files of the same bytes go once, as this
    /// composer attaches them. It neither reads nor clears what is typed
    /// here meanwhile.
    func sendFirst(id: String, text: String, files: [OutgoingFile]) async throws {
        let words = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !words.isEmpty || !files.isEmpty else { return }
        guard persistence != nil else { throw APIError.server("The local draft could not be opened. Reopen this chat before sending.") }
        guard pending.count < SavedChat.pendingMax else { throw APIError.server("Wait for the messages still sending, or edit the ones not sent.") }
        var uploaded: [ChatAttachment] = []
        for file in files {
            let put = try await api.upload(file.data, name: file.name, type: file.type, to: fragment.name)
            if !uploaded.contains(where: { $0.id == put.id }) { uploaded.append(put) }
        }
        let post = PendingPost(id: id, text: words, attachments: uploaded)
        try persistence?.save(SavedChat(composer: composer, attachments: attachments, replyTo: replyTo, recipients: recipients, pending: pending + [post]))
        pending.append(post)
        await deliver(post)
    }

    func retry(_ post: PendingPost) async {
        guard let index = pending.firstIndex(where: { $0.id == post.id }), pending[index].failed, pending[index].refused == nil else { return }
        pending[index].failed = false
        await deliver(post)
    }

    private func deliver(_ post: PendingPost) async {
        do {
            let record = try await api.post(fragment.name, id: post.id, body: Self.body(of: post))
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

    // ---- the composer while turns run (chat.js `refreshSend`, `render`) ----

    /// Who controls a turn (control.js `controllerOf`): its asker, but the
    /// agent's owner for a turn caused on its own `tasks` (a routine's).
    private func controller(of key: TurnKey, _ state: TurnState) -> String? {
        state.cause["channel"].string == "tasks" ? (owned.contains(key.agent) ? principal : nil) : state.asker
    }
    /// The newest running turn this person controls (control.js `controlledBy`).
    var myTurn: TurnKey? {
        guard workRead, !principal.isEmpty else { return nil }
        return timeline.turns.filter { $0.value.running && controller(of: $0.key, $0.value) == principal }
            .max { ($0.value.startAt!, $0.key.id) < ($1.value.startAt!, $1.key.id) }?.key
    }
    /// Whom a command goes to: the agent working for this person, else the lead.
    var commandTarget: String? { myTurn?.agent ?? agents.first }
    /// Whether `agent`'s runtime takes `command` from this person: its
    /// menu's, offered to its owner alone (chat.js `menuOf`).
    func offers(_ command: String, _ agent: String?) -> Bool {
        guard let agent, owned.contains(agent) else { return false }
        return menus[agent]?.contains(command) == true
    }
    /// Whether what is typed may Steer the running turn instead of waiting
    /// its turn (chat.js `refreshSend`): words, no command, no files, its
    /// target working, and its runtime offering `steer`. A file still
    /// uploading, or failed, is one of the composer's files too (#1088).
    func steers(recording: Bool) -> Bool {
        let words = composer.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !words.isEmpty, !words.hasPrefix("/"), attachments.isEmpty, uploads.isEmpty, !recording, workRead, let target = commandTarget,
              timeline.turns.contains(where: { $0.key.agent == target && $0.value.running }) else { return false }
        return offers("steer", target)
    }
    /// A running turn waiting on this person's words: its question
    /// (`turn.asked`), or its card that takes words (chat.js `askedMe`).
    func answering(now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> TurnKey? {
        guard workRead, !principal.isEmpty else { return nil }
        if let asked = activity.asking.filter({ $0.value == principal }).keys.min(by: { ($0.agent, $0.id) < ($1.agent, $1.id) }) { return asked }
        return activity.openCards.first { card in
            card.takesWords && card.asks == principal && (card.expiresAt.map { now <= $0 } ?? true) && timeline.turns[card.turn]?.running == true
        }?.turn
    }
    /// The composer's words, as the web's say what it does now (chat.js
    /// `render`). While their turn runs, the web's whole sentence ("<name>
    /// is working: a message waits its turn, or Steer it now") is cut on a
    /// phone: the chat already says who works, and Steer shows beside Send
    /// once words are typed, so it says what sending does alone.
    func placeholder(name: (String) -> String, now: Int64 = Int64(Date().timeIntervalSince1970 * 1000)) -> String {
        if let asked = answering(now: now) { return "Your answer for \(name(asked.agent))" }
        if workRead, let card = activity.openCards.first(where: { $0.asks == principal && !$0.stopped && ($0.expiresAt.map { now <= $0 } ?? true) }) {
            return "\(name(card.turn.agent)) is waiting for you"
        }
        if myTurn != nil { return "A message waits its turn" }
        return agents.first.map { "Message \(name($0))" } ?? "Message your agent"
    }

    /// The running turn told what is typed, at once (`/steer <words>`, a
    /// command of its runtime's menu, as the web's Steer sends it). It goes
    /// as a message goes: saved with its id before any request, shown
    /// sending, and, its answer lost, retried only by hand with that same
    /// id and body (docs/ios.md), so it never runs twice.
    func steer(recording: Bool = false) async {
        guard steers(recording: recording), let target = commandTarget else { return }
        guard persistence != nil else { error = "The local draft could not be opened. Reopen this chat before sending."; return }
        guard pending.count < SavedChat.pendingMax else { error = "Wait for the messages still sending, or edit the ones not sent."; return }
        let words = composer.trimmingCharacters(in: .whitespacesAndNewlines)
        let post = PendingPost(id: UUID().uuidString, text: words, recipients: [target], command: "steer")
        do {
            try persistence?.save(SavedChat(composer: "", attachments: attachments, replyTo: replyTo, recipients: recipients, pending: pending + [post]))
        } catch { self.error = "The command could not be saved on this iPhone: " + error.localizedDescription; return }
        pending.append(post)
        composer = ""
        await deliver(post)
    }
    /// What a post sends: a command (docs/chat-records.md, "A command"),
    /// or a message.
    static func body(of post: PendingPost) -> JSONValue {
        if let command = post.command, let target = post.recipients.first { return Self.command(command, args: post.text, to: target) }
        var body: [String: JSONValue] = ["text": .string(post.text), "client_id": .string(post.id)]
        if !post.attachments.isEmpty { body["attachments"] = .array(post.attachments.map(\.json)) }
        if let replyTo = post.replyTo { body["reply_to"] = .number(Double(replyTo)) }
        if !post.recipients.isEmpty { body["to"] = .array(post.recipients.map { .string($0) }) }
        return .object(body)
    }
    /// A command's body (docs/chat-records.md, "A command"): its words only if any.
    static func command(_ name: String, args: String, to target: String) -> JSONValue {
        var body: [String: JSONValue] = ["kind": .string("command"), "command": .string(name), "to": .array([.string(target)])]
        if !args.isEmpty { body["args"] = .string(args) }
        return .object(body)
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

/// A file of the composer's being put: how much is sent, or why it failed.
struct Upload: Identifiable, Equatable, Sendable {
    let attachment: ChatAttachment
    var progress: Double = 0
    var failed: String?
    var id: String { attachment.id }
}
