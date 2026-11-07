import Foundation

struct ToolStep: Equatable, Identifiable {
    let record: ChannelRecord
    var running = false
    var maybe = false
    var id: String { record.id }
    var category: String { record.body["category"].string ?? "other" }
    var args: String { record.body["args"].string ?? "" }
    var failed: Bool { record.body["ok"] == .bool(false) || ["failed", "timed_out", "cancelled", "blocked"].contains(record.body["status"].string ?? "") }
    var icon: String {
        switch category {
        case "shell": "terminal"
        case "web": "globe"
        case "read": "book-open"
        case "write": "pencil"
        case "browser": "app-window"
        case "image": "image"
        case "delegate": "users"
        case "memory": "brain"
        default: "wrench"
        }
    }
    var verb: String {
        if maybe { return "May have run" }
        let words: (String, String)
        switch category {
        case "shell": words = ("Running", "Ran")
        case "web": words = ("Searching the web", "Searched the web")
        case "read": words = ("Reading", "Read")
        case "write": words = ("Writing", "Wrote")
        case "browser": words = ("Browsing", "Browsed")
        case "image": words = ("Making an image", "Made an image")
        case "delegate": words = ("Delegating", "Delegated")
        case "memory": words = ("Updating memory", "Updated memory")
        default: words = (record.body["tool"].string ?? "a tool", record.body["tool"].string ?? "a tool")
        }
        guard failed else { return running ? words.0 : words.1 }
        let prefix: String
        switch record.body["status"].string {
        case "timed_out": prefix = "Timed out"
        case "cancelled": prefix = "Cancelled"
        case "blocked": prefix = "Blocked"
        default: prefix = "Failed"
        }
        return prefix + " " + (icon == "wrench" ? words.0 : words.0.prefix(1).lowercased() + words.0.dropFirst())
    }
    var label: String {
        guard !args.isEmpty, !maybe else { return verb }
        if category == "web" { return verb + " for" }
        if ["image", "delegate", "memory"].contains(category) || icon == "wrench" { return verb + ":" }
        return verb
    }
}

struct ToolCard: Equatable, Identifiable {
    let id: String
    let turn: TurnKey
    var steps: [ToolStep]
    var since: Int64
    var until: Int64?
    var words: String? = nil
    var running: Bool { until == nil }
}

struct WorkingHint: Equatable, Identifiable {
    enum Kind: Equatable {
        case working(asker: String?)
        case asked(String)
        case pending(since: Int64)
    }
    let id: TurnKey
    let kind: Kind
    var notBefore: Int64?
}

/// One thing a chat shows, in its place: a record (a message, a command, a
/// card's first step, a prompt, an end), a reply being written (its words
/// drawn apart: `ChatSession.drafts`), or a running turn's working line.
enum ChatItem: Equatable, Identifiable {
    case record(ChannelRecord)
    case draft(TurnKey)
    case hint(WorkingHint)
    /// As the web's chat keys its nodes: `d:`, `w:` and `q:` (asked) by turn.
    var id: String {
        switch self {
        case .record(let record): record.id
        case .draft(let key): "d:\(key.agent) \(key.id)"
        case .hint(let hint):
            if case .asked = hint.kind { "q:\(hint.id.agent) \(hint.id.id)" } else { "w:\(hint.id.agent) \(hint.id.id)" }
        }
    }
    var record: ChannelRecord? { if case .record(let record) = self { record } else { nil } }
}

/// A card a turn waits on, still open: whom it asks, whether its words
/// may be a message (chat.js `takesWords`), and until when.
struct OpenCard: Equatable {
    let turn: TurnKey
    let asks: String?
    let takesWords: Bool
    let expiresAt: Int64?
    let stopped: Bool
}

/// The web's chat.js `timeline()`: pure, agent-scoped and replay-safe. Call
/// it once per record revision, and as replies begin or stop being written
/// (`drafting`), never per word of one nor per animation tick.
///
/// The order is the web's, exactly, so both show a chat alike: every item
/// by its time (`at`), then its `n`, a record's own `seq` (chat.js `n`, as
/// `onChat` and `onWork` set it: a `chat` and a `work` seq compare as
/// numbers, though their channels count apart). A reply being written, and
/// a running turn's working (or asked) line, go after its turn's latest
/// record (`touch`: by time, then `n`), at `lastN + 0.5`; a live call's
/// line after all its turn said, at `Number.MAX_SAFE_INTEGER - live +
/// index`. Ties keep the web's insertion order (its `sort` is stable):
/// messages, then turns' records, then drafts, then working lines; then
/// by turn, so no two draws of the same records differ.
struct WorkingActivity: Equatable {
    var rows: [ChatItem] = []
    var cards: [String: ToolCard] = [:]
    var hints: [WorkingHint] = []
    /// Person's messages that wait for their agent's running turn (chat.js
    /// `queued`), by record id.
    var queued: Set<String> = []
    /// Running turns waiting on someone's words (`turn.asked`): whose.
    var asking: [TurnKey: String] = [:]
    /// Cards not yet closed, of turns read to have started.
    var openCards: [OpenCard] = []

    /// JavaScript's `Number.MAX_SAFE_INTEGER`, a live call's `n` base.
    static let safeMax = 9_007_199_254_740_991.0

    init(_ timeline: Timeline, ready: Bool = true, drafting: Set<TurnKey> = [], me: String = "", owned: Set<String> = [], agents chatAgents: [String] = []) {
        let lead = chatAgents.first
        let records = timeline.records
        let messages = records.filter { $0.isMessage }
        let commands = records.filter { $0.command != nil }
        let replies = Dictionary(grouping: messages.filter { $0.turn != nil }) { TurnKey(agent: $0.principal, id: $0.turn!) }
        let work = Dictionary(grouping: records.filter { $0.channel == "work" && $0.turn != nil }) {
            TurnKey(agent: $0.principal, id: $0.turn!)
        }
        enum Place: Int { case message, turn, draft, hint }
        struct Item {
            let at: Int64
            let n: Double
            let place: Place
            let tie: String
            let item: ChatItem
            var step: ToolStep?
            var turn: TurnKey?
        }
        func item(_ record: ChannelRecord, place: Place, at: Int64? = nil, n: Double? = nil, step: ToolStep? = nil, turn: TurnKey? = nil) -> Item {
            Item(at: at ?? record.at, n: n ?? Double(record.seq), place: place, tie: turn.map { "\($0.agent) \($0.id)" } ?? "", item: .record(record), step: step, turn: turn)
        }
        var items = (messages + commands).map { item($0, place: .message) }
        // each turn's latest record, by time then `n` (chat.js `touch`): its
        // own on `work`, and its replies; a turn whose records are outside
        // the window, from its start as learned
        var last: [TurnKey: (at: Int64, n: Double)] = [:]
        for (key, own) in work.merging(replies, uniquingKeysWith: +) {
            for record in own {
                let n = Double(record.seq)
                if let was = last[key], record.at < was.at || (record.at == was.at && n <= was.n) { continue }
                last[key] = (record.at, n)
            }
        }
        // a turn waiting on someone's words: until they say something to the
        // chat at or after its question (chat.js `waitsOnWords`, its
        // `speaker`: commands count, as the web's `state.chat` holds them)
        let lastSaid = Dictionary(grouping: (messages + commands).filter { $0.turn == nil }, by: \.principal).mapValues { $0.map(\.at).max()! }
        var waiting = Set<TurnKey>()
        var asked: [TurnKey: String] = [:]
        var starts: [TurnKey: Int64] = [:]
        // each turn's live calls, however many (chat.js `open`)
        var liveCalls: [TurnKey: Int] = [:]
        for (key, state) in timeline.turns where state.startAt != nil {
            starts[key] = state.startAt
            let own = work[key] ?? []
            let steps = own.filter { $0.body["kind"].string == "turn.step" }
            let calls = own.filter { $0.body["kind"].string == "turn.call" }
            let latestResponse = calls.last?.body["response"].string
            let lastWork = own.last
            let closed = Dictionary(own.filter { $0.body["kind"].string == "turn.prompt.closed" }.compactMap { r in r.body["prompt"].string.map { ($0, r) } }, uniquingKeysWith: { first, _ in first })
            let stopped = own.contains { $0.body["kind"].string == "turn.end" && ($0.body["outcome"].string == "stopped" || $0.body["status"].string == "stopped") }
            for prompt in own where prompt.body["kind"].string == "turn.prompt" {
                guard let id = prompt.body["prompt"].string, closed[id] == nil else { continue }
                waiting.insert(key)
                let takes = prompt.body["options"].array.contains { $0["words"] == .bool(true) && $0["computer"] != .bool(true) }
                var expires: Int64?
                if case .number(let at) = prompt.body["expiresAt"], at.isFinite, abs(at) < 9e15 { expires = Int64(at) }
                openCards.append(.init(turn: key, asks: prompt.body["asks"].string, takesWords: takes, expiresAt: expires, stopped: stopped))
            }
            if lastWork?.body["kind"].string == "turn.asked", let asks = lastWork?.body["asks"].string,
               (lastSaid[asks] ?? Int64.min) < lastWork!.at { asked[key] = asks }
            for record in own {
                switch record.body["kind"].string {
                case "turn.step":
                    items.append(item(record, place: .turn, step: ToolStep(record: record), turn: key))
                case "turn.prompt", "turn.end":
                    if record.body["kind"].string == "turn.prompt" || record.body["error"].string != nil || record.body["status"].string == "stopped" || record.body["outcome"].string == "stopped" {
                        items.append(item(record, place: .turn, turn: key))
                    }
                default: break
                }
            }
            // Match one result to one call, reserving all exact IDs before
            // falling back to tool/arguments (chat.js unended / bridge note.rs).
            // Only an omitted response permits legacy cross-response matching.
            var open = Set(calls.indices)
            var paired = Set<Int>()
            for byID in [true, false] {
                for index in steps.indices where !paired.contains(index) {
                    let step = steps[index]
                    if let match = calls.indices.first(where: { candidate in
                        guard open.contains(candidate) else { return false }
                        let call = calls[candidate]
                        if case .object(let body) = step.body, let response = body["response"], response != call.body["response"] { return false }
                        if byID { return step.body["tool_call_id"].string != nil && step.body["tool_call_id"] == call.body["tool_call_id"] }
                        return step.body["tool"] == call.body["tool"] && step.body["args"] == call.body["args"]
                    }) {
                        open.remove(match)
                        paired.insert(index)
                    }
                }
            }
            let after = max(own.last?.at ?? 0, replies[key]?.last?.at ?? 0)
            let live = calls.indices.filter { open.contains($0) && ready && state.running && calls[$0].body["response"].string == latestResponse }
            liveCalls[key] = live.count
            for (callIndex, call) in calls.enumerated() where open.contains(callIndex) {
                let runs = live.contains(callIndex)
                if runs && (waiting.contains(key) || asked[key] != nil) { continue }
                let n = runs ? Self.safeMax - Double(live.count) + Double(live.firstIndex(of: callIndex)!) : Double(call.seq)
                items.append(item(call, place: .turn, at: runs ? after : call.at, n: n,
                                  step: ToolStep(record: call, running: runs, maybe: !runs), turn: key))
            }
        }
        // where a turn's draft or working line goes: after its latest record
        func placed(_ key: TurnKey) -> (at: Int64, n: Double) {
            if let latest = last[key] { return (latest.at, latest.n + 0.5) }
            if let start = timeline.turns[key]?.startAt { return (start, 0.5) }
            return (Int64.max, 0.5)
        }
        let tie = { (key: TurnKey) in "\(key.agent) \(key.id)" }
        for key in drafting {
            let place = placed(key)
            items.append(Item(at: place.at, n: place.n, place: .draft, tie: tie(key), item: .draft(key)))
        }
        // the working lines, decided before the cards are grouped (below)
        var pendingHints: [WorkingHint] = []
        for (key, state) in timeline.turns.sorted(by: { ($0.value.startAt ?? 0, $0.key.agent, $0.key.id) < ($1.value.startAt ?? 0, $1.key.agent, $1.key.id) }) where state.running && ready {
            // a reply being written says its turn works
            if drafting.contains(key) { continue }
            let closed = Set((work[key] ?? []).filter { $0.body["kind"].string == "turn.prompt.closed" }.compactMap { $0.body["prompt"].string })
            let prompts = (work[key] ?? []).filter { $0.body["kind"].string == "turn.prompt" && !closed.contains($0.body["prompt"].string ?? "") }
            let notBefore = prompts.compactMap { record -> Int64? in
                if case .number(let at) = record.body["expiresAt"], at.isFinite, at > Double(Int64.min), at < Double(Int64.max) { return Int64(at) }
                return nil
            }.max()
            if waiting.contains(key) && notBefore == nil { continue }
            if let asks = asked[key] { pendingHints.append(.init(id: key, kind: .asked(asks))); continue }
            // a live call's line says it works
            if (liveCalls[key] ?? 0) > 0 { continue }
            let asker = work[key]?.first { $0.body["kind"].string == "turn.start" }?.body["asker"].string
            pendingHints.append(.init(id: key, kind: .working(asker: asker), notBefore: notBefore))
        }
        for hint in pendingHints {
            let place = placed(hint.id)
            items.append(Item(at: place.at, n: place.n, place: .hint, tie: tie(hint.id), item: .hint(hint)))
        }
        items.sort { a, b in
            if a.at != b.at { return a.at < b.at }
            if a.n != b.n { return a.n < b.n }
            if a.place != b.place { return a.place.rawValue < b.place.rawValue }
            return a.tie < b.tie
        }
        var lastCard: String?
        for item in items {
            guard let step = item.step, let key = item.turn, let record = item.item.record else {
                rows.append(item.item); lastCard = nil; continue
            }
            let said = step.record.body["text"].string?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            let cut = said.hasSuffix("…")
            let words = cut ? String(said.dropLast()).trimmingCharacters(in: .whitespacesAndNewlines) : said
            let echoed = (replies[key] ?? []).contains { reply in
                let text = reply.text.trimmingCharacters(in: .whitespacesAndNewlines)
                return cut ? text.hasPrefix(words) : text == words
            }
            let preface = !said.isEmpty && !echoed ? said : nil
            if preface != nil { lastCard = nil }
            if let id = lastCard, cards[id]?.turn == key {
                cards[id]?.steps.append(step)
            } else {
                lastCard = record.id
                rows.append(item.item)
                cards[record.id] = ToolCard(id: record.id, turn: key, steps: [step], since: record.at, until: nil, words: preface)
            }
        }
        for id in cards.keys.sorted() {
            var card = cards[id]!
            let key = card.turn
            let own = work[key] ?? []
            let last = card.steps.last!.record
            // chat.js cardEnd ignores call announcements: their results,
            // replies, prompts/notices and the turn's end delimit a card.
            let subsequent = records.first {
                guard $0.principal == key.agent && $0.turn == key.id else { return false }
                if $0.isMessage { return $0.at > last.at }
                return $0.channel == "work" && ["turn.step", "turn.prompt", "turn.notice"].contains($0.body["kind"].string ?? "")
                    && ($0.at > last.at || ($0.at == last.at && $0.seq > last.seq))
            }
            let replyAfter = (replies[key]?.last?.at ?? Int64.min) > last.at
            let runs = ready && timeline.turns[key]?.running == true
                && ((last.seq == own.last?.seq && !replyAfter) || (last.body["kind"].string == "turn.call" && !card.steps.last!.maybe))
            if !runs {
                let end = own.last { $0.body["kind"].string == "turn.end" }?.at
                card.until = [subsequent?.at, end].compactMap { $0 }.min() ?? own.last?.at ?? last.at
            }
            if runs, !card.steps.contains(where: { $0.running }), let index = card.steps.indices.last,
               card.steps[index].record.body["status"].string == nil, !card.steps[index].failed { card.steps[index].running = true }
            // A matched call is absent from the rendered steps. The first
            // result still counts from turn.start, as the web does.
            let first = (own.filter { $0.body["kind"].string == "turn.step" }.map(\.at) + card.steps.map { $0.record.at }).min()
            if card.steps.first?.record.at == first { card.since = starts[key] ?? card.since }
            cards[id] = card
        }
        // a running card's head says its turn works: no working line beside
        // it (chat.js: none while its turn has a live call, nor right after
        // its running card)
        let liveCards = Set(cards.values.filter(\.running).map(\.turn))
        rows.removeAll { row in
            if case .hint(let hint) = row, case .working = hint.kind { return liveCards.contains(hint.id) }
            return false
        }
        hints = pendingHints.filter { hint in if case .working = hint.kind { !liveCards.contains(hint.id) } else { true } }
        asking = asked.filter { timeline.turns[$0.key]?.running == true && ready }
        // a person's message that waits its turn (chat.js `queued`)
        let agents = Set(timeline.turns.keys.map(\.agent)).union(chatAgents)
        for message in messages where message.turn == nil && !agents.contains(message.principal) {
            if timeline.turns.values.contains(where: { $0.cause["channel"].string == "chat" && $0.cause["seq"] == .number(Double(message.seq)) }) { continue }
            let to = message.body["to"].array.compactMap(\.string)
            let targets = to.isEmpty ? (lead.map { [$0] } ?? []) : to
            let waits = ready && timeline.turns.contains { key, state in
                guard state.running, let start = state.startAt, targets.contains(key.agent), start <= message.at else { return false }
                let own = work[key] ?? []
                // it answered the turn's card that takes words: it waits for nothing
                let closings = Dictionary(own.filter { $0.body["kind"].string == "turn.prompt.closed" }.compactMap { r in r.body["prompt"].string.map { ($0, r) } }, uniquingKeysWith: { first, _ in first })
                let answered = own.contains { p in
                    guard p.body["kind"].string == "turn.prompt", p.body["asks"].string == message.principal, p.at <= message.at,
                          p.body["options"].array.contains(where: { $0["words"] == .bool(true) && $0["computer"] != .bool(true) }),
                          let close = closings[p.body["prompt"].string ?? ""] else { return false }
                    return close.body["outcome"].string == "answered" && close.at >= message.at
                }
                if answered { return false }
                let controller = state.cause["channel"].string == "tasks" ? (owned.contains(key.agent) ? me : nil) : state.asker
                if controller != message.principal { return true }
                // chat and work count apart: their times say whether a reply waits
                let said = (replies[key] ?? []).filter { $0.seq < message.seq }.map(\.at).max() ?? -1
                let did = own.filter { ["turn.step", "turn.prompt"].contains($0.body["kind"].string ?? "") && $0.at <= message.at }.map(\.at).max() ?? -1
                return !(said >= 0 && said > did)
            }
            if waits { queued.insert(message.id) }
        }
    }

    /// A successful post awaiting its agent's claim, as the web's pending
    /// line. It is not an outbox spinner: refused/unsent posts stay separate.
    static func pending(_ timeline: Timeline, principal: String, lead: String?) -> WorkingHint? {
        guard let message = timeline.records.last(where: { $0.isMessage }), message.principal == principal else { return nil }
        let target = message.body["to"].array.first?.string ?? lead
        guard let target else { return nil }
        if timeline.turns.contains(where: { $0.key.agent == target && $0.value.cause["channel"].string == "chat" && $0.value.cause["seq"] == .number(Double(message.seq)) }) { return nil }
        // one sent while its agent works waits its turn, which that turn's own line says (chat.js `expectTurn`)
        if timeline.turns.contains(where: { $0.key.agent == target && $0.value.running }) { return nil }
        if timeline.records.contains(where: { $0.channel == "work" && $0.principal == target && $0.body["kind"].string == "turn.start" && $0.body["cause"]["seq"] == .number(Double(message.seq)) }) { return nil }
        return WorkingHint(id: .init(agent: target, id: "pending:\(message.id)"), kind: .pending(since: message.at))
    }
}
