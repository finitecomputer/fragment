import Foundation
import Observation

@MainActor @Observable
final class ChatSession {
    let fragment: FragmentSummary
    let api: FragmentAPI
    private(set) var timeline = Timeline()
    private(set) var pending: [PendingPost] = [] { didSet { scheduleDraftSave() } }
    private(set) var connected = false
    private(set) var caughtUp = false
    private(set) var principal = ""
    private(set) var loadingHistory = false
    private(set) var hasEarlier = true
    var error: String?
    var composer = "" { didSet { scheduleDraftSave() } }
    var attachments: [ChatAttachment] = [] { didSet { scheduleDraftSave() } }
    var replyTo: ChannelRecord? { didSet { scheduleDraftSave() } }
    var recipients: [String] = [] { didSet { scheduleDraftSave() } }

    @ObservationIgnored private var persistence: ChatDraftStore?
    @ObservationIgnored private var draftWrite: Task<Void, Never>?

    init(fragment: FragmentSummary, api: FragmentAPI) {
        self.fragment = fragment
        self.api = api
        if api.isSigned {
            do {
                persistence = try ChatDraftStore(api: api, fragment: fragment.name)
                if let saved = try persistence?.load() {
                    composer = saved.composer; attachments = saved.attachments; replyTo = saved.replyTo; recipients = saved.recipients
                    // A terminated send may already be committed. Retry remains an
                    // explicit action, with exactly the saved id and body.
                    pending = saved.pending.map { var post = $0; post.failed = true; return post }
                }
            } catch { self.error = "The local draft could not be opened: " + error.localizedDescription; persistence = nil }
        }
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

    // Owned by the view's .task: backgrounding or leaving the chat closes the
    // socket, including a suspended receive. Each foreground visit catches up.
    func run() async {
        var attempt = 0
        while !Task.isCancelled {
            let socket: URLSessionWebSocketTask
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
                            principal = frame.principal ?? ""
                            connected = true
                            error = nil
                            attempt = 0
                            for channel in ["chat", "work"] {
                                try await subscribe(socket, channel: channel, after: timeline.cursors[channel])
                            }
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
                                if timeline.records.filter({ $0.channel == "chat" }).count < 50 { hasEarlier = false }
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
                if api.isSigned && socket.closeCode.rawValue == 4003 {
                    do { let _: JSONValue = try await api.request("api/identities/me") }
                    catch APIError.unauthorized { refused = true }
                    catch { /* A transient check failure can retry with the usual backoff. */ }
                }
                if !Task.isCancelled { self.error = refused ? "Your access ended. Reconnect your account or check sharing permissions." : "Connection interrupted. Reconnecting…" }
            }
            socket.cancel(with: .goingAway, reason: nil)
            connected = false
            caughtUp = false
            timeline.disconnected()
            guard !Task.isCancelled, !refused else { return }
            attempt = min(attempt + 1, 4)
            do { try await Task.sleep(for: .seconds(min(15, 1 << attempt))) }
            catch { return }
        }
    }

    private func subscribe(_ socket: URLSessionWebSocketTask, channel: String, after: Int64?) async throws {
        var body: [String: JSONValue] = ["type": .string("subscribe"), "channel": .string(channel)]
        if let after { body["after"] = .number(Double(after)) }
        else { body["last"] = .number(50) }
        let data = try JSONEncoder().encode(JSONValue.object(body))
        try await socket.send(.string(String(decoding: data, as: UTF8.self)))
    }

    func send() async {
        guard !api.isSigned || persistence != nil else {
            error = "The local draft could not be opened. Reopen this chat before sending."
            return
        }
        let text = composer.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (!text.isEmpty || !attachments.isEmpty), text.utf8.count <= 32_768, pending.count < 16 else { return }
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
        guard let index = pending.firstIndex(where: { $0.id == post.id }), pending[index].failed else { return }
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
            hasEarlier = page.count == 50 && timeline.records.count < Timeline.maximumRecords
        } catch { self.error = error.localizedDescription }
    }
}
