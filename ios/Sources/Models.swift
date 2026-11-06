import Foundation

struct FragmentSummary: Codable, Identifiable, Hashable, Sendable {
    let name: String
    var kind: String
    var title: String?
    var agents: [String]?
    var preview: String?
    var previewAt: Int64?
    var archived: Bool?
    var dock: Bool?
    var role: String?
    var picture: String?
    // A navigation selection in the session archive, never an authentication token.
    var sessionKey: String?
    /// A page of it a link named, to open there (`Library.fragment(linkedBy:)`):
    /// a navigation selection, never from the wire.
    var link: URL?
    var id: String { name }
    var displayTitle: String { title ?? name }
}

struct SearchResults: Decodable, Sendable {
    var fragments: [FragmentSummary] = []
    var messages: [MessageHit] = []
}

struct MessageHit: Decodable, Identifiable, Sendable {
    let fragment: String
    let channel: String
    let seq: Int64
    let at: Int64
    let snippet: String
    var id: String { "\(fragment):\(channel):\(seq)" }
}

// Channels accept arbitrary JSON; unknown fields and record kinds must survive
// decoding without making an older app unable to read a newer chat.
enum JSONValue: Codable, Equatable, Sendable {
    case string(String), number(Double), bool(Bool), object([String: JSONValue]), array([JSONValue]), null

    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if value.decodeNil() { self = .null }
        else if let v = try? value.decode(String.self) { self = .string(v) }
        else if let v = try? value.decode(Bool.self) { self = .bool(v) }
        else if let v = try? value.decode(Double.self) { self = .number(v) }
        else if let v = try? value.decode([String: JSONValue].self) { self = .object(v) }
        else { self = .array(try value.decode([JSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var value = encoder.singleValueContainer()
        switch self {
        case .string(let v): try value.encode(v)
        case .number(let v): try value.encode(v)
        case .bool(let v): try value.encode(v)
        case .object(let v): try value.encode(v)
        case .array(let v): try value.encode(v)
        case .null: try value.encodeNil()
        }
    }

    subscript(_ key: String) -> JSONValue {
        if case .object(let value) = self { return value[key] ?? .null }
        return .null
    }
    var string: String? { if case .string(let v) = self { return v }; return nil }
    var array: [JSONValue] { if case .array(let v) = self { return v }; return [] }
}

struct ChannelRecord: Codable, Identifiable, Equatable, Sendable {
    let channel: String
    let seq: Int64
    let at: Int64
    let principal: String
    let kind: String
    let body: JSONValue
    var id: String { "\(channel):\(seq)" }
    var text: String { body["text"].string ?? body.string ?? "" }
    var turn: String? { body["turn"].string }
    var isMessage: Bool {
        channel == "chat" && (body["kind"].string == nil || body["kind"].string == "message")
    }
}

struct LiveFrame: Decodable, Sendable {
    let type: String
    var channel: String?
    var seq: Int64?
    var at: Int64?
    var principal: String?
    var kind: String?
    var body: JSONValue?
    var turn: String?
    var text: String?
    var next: Int64?
    var more: Bool?
    var message: String?
    struct Presence: Decodable, Sendable {
        let id: String
        let principal: String
        let data: JSONValue?
    }
    var presence: [Presence]?
    var id: String?
    var data: JSONValue?

    var record: ChannelRecord? {
        guard let channel, let seq, seq > 0, let at, let principal, let kind, let body else { return nil }
        return ChannelRecord(channel: channel, seq: seq, at: at, principal: principal, kind: kind, body: body)
    }
}

struct ReplyDraft: Identifiable, Equatable {
    let turn: String
    let principal: String
    var text: String
    var at: Int64
    var id: String { turn }
}

/// A turn by its agent and its id, as the web's chat page keys one
/// (templates/chat/site/chat.js `turnOf`): a `work` record counts toward
/// the turn of its id that its own poster began, so one agent's records
/// (an end among them) never touch another's turn (docs/chat-records.md).
struct TurnKey: Hashable, Sendable {
    let agent: String
    let id: String
}

/// What a turn's records said: when its own `turn.start` was read, and
/// whether its `turn.end` was. It runs once started and until ended
/// (control.js `running`), in whichever order its records were read.
struct TurnState: Equatable, Sendable {
    var startAt: Int64?
    var ended = false
    var cause: JSONValue = .null
    var running: Bool { startAt != nil && !ended }
}

struct Timeline {
    private(set) var records: [ChannelRecord] = []
    /// Bumped by each change to records or remembered turns: what a view draws from them is
    /// made again only then (`ChatSession.rows`), never for a draft's words.
    private(set) var revision = 0
    private var ids: Set<String> = []
    private(set) var drafts: [String: ReplyDraft] = [:]
    private(set) var cursors: [String: Int64] = [:]
    /// Each turn read, kept apart from the record window, which a long
    /// turn's steps outgrow: whether a turn runs (and Stop shows) never
    /// depends on how much of it is on screen.
    private(set) var turns: [TurnKey: TurnState] = [:]
    /// The oldest `work` record read (its seq and time): where reading
    /// earlier work goes on from, and how far back it reached.
    private(set) var oldestWork: (seq: Int64, at: Int64)?
    // Bound the in-memory window independently of the server's retention.
    static let maximumRecords = 2_000
    /// Turns kept, past which the oldest ones both begun and ended go: their
    /// start was read, so no earlier page holds it to begin one again.
    static let maximumTurns = 4_096

    /// What a `work` record says of its poster's turn, wherever it falls:
    /// in the window, or read back for this alone (`ChatSession`).
    mutating func learn(_ record: ChannelRecord) {
        guard record.channel == "work" else { return }
        if oldestWork.map({ record.seq < $0.seq }) ?? true { oldestWork = (record.seq, record.at) }
        guard let id = record.turn else { return }
        let key = TurnKey(agent: record.principal, id: id)
        var state = turns[key] ?? TurnState()
        switch record.body["kind"].string {
        case "turn.start": state.startAt = record.at; state.cause = record.body["cause"]
        case "turn.end": state.ended = true
        default: break
        }
        if turns[key] != state { revision &+= 1 }
        turns[key] = state
        if turns.count > Self.maximumTurns {
            let done = turns.filter { $0.value.startAt != nil && $0.value.ended && $0.key != key }.sorted { $0.value.startAt! < $1.value.startAt! }
            for (gone, _) in done.prefix(turns.count - Self.maximumTurns) { turns.removeValue(forKey: gone) }
        }
    }

    /// Whether a turn was read without its start: earlier work holds it.
    var awaitsAStart: Bool { turns.values.contains { $0.startAt == nil } }

    mutating func insert(_ record: ChannelRecord, advancingCursor: Bool = true) {
        learn(record)
        if advancingCursor { cursors[record.channel] = max(cursors[record.channel] ?? 0, record.seq) }
        guard ids.insert(record.id).inserted else { return }
        // in order of time, then channel, then seq: placed where it goes (a
        // live record at the end, an older page's at the start), not sorted
        // again with every record
        let index = Self.place(record, in: records)
        records.insert(record, at: index)
        if records.count > Self.maximumRecords {
            let gone = records.count - Self.maximumRecords
            for old in records.prefix(gone) { ids.remove(old.id) }
            records.removeFirst(gone)
        }
        revision &+= 1
        if advancingCursor, record.channel == "chat", let turn = record.turn,
           drafts[turn]?.principal == record.principal {
            drafts.removeValue(forKey: turn)
        }
        if record.channel == "work", record.body["kind"].string == "turn.end", let turn = record.turn {
            drafts.removeValue(forKey: turn)
        }
    }

    static func precedes(_ a: ChannelRecord, _ b: ChannelRecord) -> Bool {
        if a.at != b.at { return a.at < b.at }
        if a.channel != b.channel { return a.channel < b.channel }
        return a.seq < b.seq
    }
    /// Where `record` goes in `records`, kept in `precedes` order: after
    /// every record before it.
    static func place(_ record: ChannelRecord, in records: [ChannelRecord]) -> Int {
        if let last = records.last, precedes(last, record) { return records.count }
        var low = 0, high = records.count
        while low < high {
            let mid = (low + high) / 2
            if precedes(records[mid], record) { low = mid + 1 } else { high = mid }
        }
        return low
    }

    mutating func apply(_ frame: LiveFrame) throws {
        switch frame.type {
        case "record":
            guard let record = frame.record else { throw APIError.invalidResponse }
            insert(record)
        case "draft":
            guard frame.channel == "chat", let turn = frame.turn, let principal = frame.principal else { return }
            if let text = frame.text {
                guard text.utf8.count <= 65_536, drafts.count < 32 || drafts[turn] != nil else {
                    throw APIError.limitExceeded
                }
                drafts[turn] = ReplyDraft(turn: turn, principal: principal, text: text, at: frame.at ?? 0)
            } else { drafts.removeValue(forKey: turn) }
        case "subscribed":
            if let channel = frame.channel, let next = frame.next {
                cursors[channel] = max(cursors[channel] ?? 0, next)
            }
        case "error": throw APIError.server(frame.message ?? "The live connection was refused.")
        default: break
        }
    }

    mutating func disconnected() { drafts.removeAll() }

    /// The turns running, the earliest begun first: each id once (a Stop
    /// names a turn by its id).
    var activeTurns: [String] {
        var ids: [String] = []
        for (key, _) in turns.filter({ $0.value.running }).sorted(by: { ($0.value.startAt!, $0.key.id) < ($1.value.startAt!, $1.key.id) }) where !ids.contains(key.id) {
            ids.append(key.id)
        }
        return ids
    }
}

struct PendingPost: Codable, Identifiable, Sendable {
    let id: String
    let text: String
    var failed = false
    /// Why the server refused it: final, so it is edited, never retried.
    var refused: String?
    var attachments: [ChatAttachment] = []
    var replyTo: Int64?
    var recipients: [String] = []
}

struct ImportedSession: Decodable, Identifiable, Sendable {
    let key: String
    var title: String?
    var preview: String?
    var started_at: String?
    var ended_at: String?
    var id: String { key }
    var at: Int64 { Int64((ISO8601DateFormatter().date(from: ended_at ?? started_at ?? "")?.timeIntervalSince1970 ?? 0) * 1000) }
}
