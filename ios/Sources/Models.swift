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
    /// A person's command for an agent's runtime (`{kind: "command"}`,
    /// docs/chat-records.md): shown as typed, `/<command> <args>`, as the
    /// web's chat shows one (chat.js `onChat`).
    var command: String? {
        guard channel == "chat", body["kind"].string == "command" else { return nil }
        return Self.commandName(body["command"].string)
    }
    /// A command's name as the wire takes one, `^[a-z0-9_-]{1,32}$` (chat.js `COMMAND`).
    static func commandName(_ name: String?) -> String? {
        guard let name, (1...32).contains(name.utf8.count),
              name.utf8.allSatisfy({ (97...122).contains($0) || (48...57).contains($0) || $0 == 95 || $0 == 45 }) else { return nil }
        return name
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
    /// Whom its `turn.start` names as its asker (control.js `controllerOf`).
    var asker: String?
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
    /// The window: at most this many `chat` records, whatever the server
    /// keeps (100,000 a channel), as the web's chat holds about as many:
    /// its newest while it follows the chat, else the stretch scrolled to
    /// (two-sided: `insertOlder` lets the newest end go, `insertNewer` the
    /// oldest). `work` records never count toward it: they say which turns
    /// run (`turns`, kept apart, so Stop survives any window) and give the
    /// turns in the window their steps.
    static let chatMax = 600
    /// Whether the window's newest end was let go (paged back past
    /// `chatMax`): live records then say which turns run, but enter no
    /// window until it reaches the newest again (`insertNewer`, `reset`).
    private(set) var detached = false
    /// `work` records kept at most, the oldest let go first: a bound on
    /// memory alone, past which the oldest turns' steps go.
    static let workMax = 20_000
    private(set) var chatCount = 0
    private var workCount = 0
    /// Turns kept, past which the oldest ones both begun and ended go: their
    /// start was read, so no earlier page holds it to begin one again.
    static let maximumTurns = 4_096

    /// What a `work` record says of its poster's turn, wherever it falls:
    /// in the window, or read back for this alone (`ChatSession`).
    mutating func learn(_ record: ChannelRecord) {
        guard record.channel == "work" else { return }
        if oldestWork.map({ record.seq < $0.seq }) ?? true { oldestWork = (record.seq, record.at) }
        indexWork(record)
        learnMenu(record)
        guard let id = record.turn else { return }
        let key = TurnKey(agent: record.principal, id: id)
        var state = turns[key] ?? TurnState()
        switch record.body["kind"].string {
        case "turn.start": state.startAt = record.at; state.cause = record.body["cause"]; state.asker = record.body["asker"].string
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

    /// Each agent's menu, its latest (`{kind: "commands"}` on `work`, posted
    /// by the agent it names): the names of the commands its runtime takes
    /// from its owner, as the web's chat reads one (chat.js `onMenu`).
    private(set) var menus: [String: (seq: Int64, commands: [String])] = [:]
    static let menuMax = 64
    private mutating func learnMenu(_ record: ChannelRecord) {
        guard record.body["kind"].string == "commands", record.body["agent"].string == record.principal,
              case .array(let listed) = record.body["commands"], (menus[record.principal]?.seq ?? -1) < record.seq else { return }
        let names = listed.compactMap { ChannelRecord.commandName($0["name"].string) }.prefix(Self.menuMax)
        menus[record.principal] = (record.seq, Array(names))
        revision &+= 1
    }

    /// Whether a turn read without its start needs it, from earlier work:
    /// one that may be running (for Stop), or one whose steps the window
    /// shows (its card is drawn from its start). One that ended with
    /// nothing shown needs nothing more.
    var awaitsAStart: Bool {
        turns.values.contains { $0.startAt == nil && !$0.ended }
            || records.contains { record in
                record.channel == "work" && record.turn.map { turns[TurnKey(agent: record.principal, id: $0)]?.startAt == nil } == true
            }
    }

    /// Past `chatMax`, the oldest `chat` records go, and the `work` before
    /// the oldest one kept with them (it belongs to no turn in the window);
    /// past `workMax`, the oldest `work` goes. Whether a turn runs stays.
    private mutating func trimOldest() {
        guard chatCount > Self.chatMax || workCount > Self.workMax else { return }
        var cut = 0
        if chatCount > Self.chatMax {
            var extra = chatCount - Self.chatMax
            while extra > 0 || (cut < records.count && records[cut].channel != "chat") {
                if records[cut].channel == "chat" { extra -= 1 }
                cut += 1
            }
        }
        for old in records.prefix(cut) { ids.remove(old.id); if old.channel == "chat" { chatCount -= 1 } else { workCount -= 1 } }
        records.removeFirst(cut)
        if workCount > Self.workMax {
            var extra = workCount - Self.workMax
            var kept: [ChannelRecord] = []
            kept.reserveCapacity(records.count - extra)
            for old in records {
                if extra > 0 && old.channel != "chat" { extra -= 1; ids.remove(old.id); workCount -= 1 } else { kept.append(old) }
            }
            records = kept
        }
    }

    mutating func insert(_ record: ChannelRecord, advancingCursor: Bool = true) {
        learn(record)
        if advancingCursor { cursors[record.channel] = max(cursors[record.channel] ?? 0, record.seq) }
        guard ids.insert(record.id).inserted else { return }
        // in order of time, then channel, then seq: placed where it goes (a
        // live record at the end, an older page's at the start), not sorted
        // again with every record
        // detached, a live record says which turns run, and waits outside the window
        // (a record the phone posts, newer than the window, waits likewise)
        if detached && (advancingCursor || newestChat.map({ Self.precedes($0, record) }) ?? true) { ids.remove(record.id); return }
        let index = Self.place(record, in: records)
        records.insert(record, at: index)
        if record.channel == "chat" { chatCount += 1 } else { workCount += 1 }
        trimOldest()
        revision &+= 1
        if advancingCursor, record.channel == "chat", let turn = record.turn,
           drafts[turn]?.principal == record.principal {
            drafts.removeValue(forKey: turn)
        }
        if record.channel == "work", record.body["kind"].string == "turn.end", let turn = record.turn {
            drafts.removeValue(forKey: turn)
        }
    }

    /// Each `work` record learned, its seq and time, in seq order: where a
    /// page of messages' work begins, whatever the window holds (a stretch
    /// paged back to may hold none). At most `workIndexMax`, the oldest let go.
    private(set) var workIndex: [(seq: Int64, at: Int64)] = []
    static let workIndexMax = 100_000
    private mutating func indexWork(_ record: ChannelRecord) {
        var low = 0, high = workIndex.count
        if let last = workIndex.last, last.seq < record.seq { low = high }
        while low < high {
            let mid = (low + high) / 2
            if workIndex[mid].seq < record.seq { low = mid + 1 } else { high = mid }
        }
        if low < workIndex.count, workIndex[low].seq == record.seq { return }
        workIndex.insert((record.seq, record.at), at: low)
        if workIndex.count > Self.workIndexMax { workIndex.removeFirst(workIndex.count - Self.workIndexMax) }
    }
    /// The newest `work` seq learned at or before `time`, to read on from.
    func workSeq(atOrBefore time: Int64) -> Int64? {
        workIndex.last { $0.at <= time }?.seq
    }

    /// An older page, scrolled up to: kept whole, and past `chatMax` the
    /// window's newest end goes instead (the chat no longer reaches its
    /// newest record: `detached`).
    mutating func insertOlder(_ page: [ChannelRecord]) {
        for record in page { place(record) }
        guard chatCount > Self.chatMax else { revision &+= 1; return }
        var extra = chatCount - Self.chatMax
        var cut = records.count
        // its newest messages beyond the bound, and the work after the newest one kept
        while extra > 0 || (cut > 0 && records[cut - 1].channel != "chat") {
            cut -= 1
            if records[cut].channel == "chat" { extra -= 1 }
        }
        for old in records[cut...] { ids.remove(old.id); if old.channel == "chat" { chatCount -= 1 } else { workCount -= 1 } }
        records.removeSubrange(cut...)
        detached = true
        drafts.removeAll()
        revision &+= 1
    }
    /// A newer page, scrolled down to: past `chatMax` the oldest end goes;
    /// once it reaches the newest record heard, the window is the chat's again.
    mutating func insertNewer(_ page: [ChannelRecord], reachesNewest: Bool) {
        for record in page { place(record) }
        trimOldest()
        if reachesNewest { detached = false }
        revision &+= 1
    }
    /// The chat's newest page, and the work it shows, in place of the window
    /// (Latest messages): whether turns run, and the cursors, stay.
    mutating func reset(_ page: [ChannelRecord], live: Bool = true) {
        records.removeAll(); ids.removeAll(); chatCount = 0; workCount = 0
        detached = !live
        for record in page { place(record) }
        trimOldest()
        revision &+= 1
    }
    /// A record read back into the window, never a live one: what it says of
    /// its turn is learned, and it is placed, if not held already.
    private mutating func place(_ record: ChannelRecord) {
        learn(record)
        guard ids.insert(record.id).inserted else { return }
        records.insert(record, at: Self.place(record, in: records))
        if record.channel == "chat" { chatCount += 1 } else { workCount += 1 }
    }
    var oldestChat: ChannelRecord? { records.first { $0.channel == "chat" } }
    var newestChat: ChannelRecord? { records.last { $0.channel == "chat" } }

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

    /// Drafts of turns read to have ended (or begun nowhere): a reconnect
    /// keeps drafts until the new connection's work says this.
    /// Every draft: a chat closed for good hears no more of them.
    mutating func dropDrafts() { drafts.removeAll() }

    mutating func settleDrafts() {
        let running = Set(activeTurns)
        drafts = drafts.filter { running.contains($0.key) }
    }

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
    /// A command of its agent's menu (`steer`), its words `text`, its agent
    /// `recipients`' one: posted as `{kind: "command"}`, saved and retried
    /// with the same id as a message is (docs/ios.md).
    var command: String? = nil
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
