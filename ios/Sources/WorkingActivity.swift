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

/// The web's chat.js `items`: pure, agent-scoped and replay-safe. Call it
/// once per record revision (not per animation tick). Draft-only updates
/// only need to filter the hints by their agent/turn at the call site.
struct WorkingActivity: Equatable {
    var rows: [ChannelRecord] = []
    var cards: [String: ToolCard] = [:]
    var hints: [WorkingHint] = []

    init(_ timeline: Timeline, ready: Bool = true) {
        let records = timeline.records
        let messages = records.filter { $0.isMessage }
        let replies = Dictionary(grouping: messages.filter { $0.turn != nil }) { TurnKey(agent: $0.principal, id: $0.turn!) }
        let lastSaid = Dictionary(grouping: messages, by: \.principal).mapValues { $0.last!.at }
        let work = Dictionary(grouping: records.filter { $0.channel == "work" && $0.turn != nil }) {
            TurnKey(agent: $0.principal, id: $0.turn!)
        }
        struct Item {
            let record: ChannelRecord
            let at: Int64
            let order: Int64
            var step: ToolStep?
            var turn: TurnKey?
        }
        var items = messages.map { Item(record: $0, at: $0.at, order: $0.seq, step: nil, turn: nil) }
        var waiting = Set<TurnKey>()
        var asked: [TurnKey: String] = [:]
        var starts: [TurnKey: Int64] = [:]
        for (key, state) in timeline.turns where state.startAt != nil {
            starts[key] = state.startAt
            let own = work[key] ?? []
            let steps = own.filter { $0.body["kind"].string == "turn.step" }
            let calls = own.filter { $0.body["kind"].string == "turn.call" }
            let latestResponse = calls.last?.body["response"].string
            let lastWork = own.last
            let closed = Set(own.filter { $0.body["kind"].string == "turn.prompt.closed" }.compactMap { $0.body["prompt"].string })
            if own.contains(where: { $0.body["kind"].string == "turn.prompt" && !closed.contains($0.body["prompt"].string ?? "") }) { waiting.insert(key) }
            if lastWork?.body["kind"].string == "turn.asked", let asks = lastWork?.body["asks"].string,
               (lastSaid[asks] ?? Int64.min) < lastWork!.at { asked[key] = asks }
            for record in own {
                switch record.body["kind"].string {
                case "turn.step":
                    items.append(Item(record: record, at: record.at, order: record.seq,
                                      step: ToolStep(record: record), turn: key))
                case "turn.prompt", "turn.end":
                    if record.body["kind"].string == "turn.prompt" || record.body["error"].string != nil || record.body["status"].string == "stopped" || record.body["outcome"].string == "stopped" {
                        items.append(Item(record: record, at: record.at, order: record.seq, step: nil, turn: key))
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
            for (callIndex, call) in calls.enumerated() where open.contains(callIndex) {
                let live = ready && state.running && call.body["response"].string == latestResponse
                if live && (waiting.contains(key) || asked[key] != nil) { continue }
                items.append(Item(record: call, at: live ? after : call.at, order: live ? Int64.max - Int64(calls.count) + Int64(callIndex) : call.seq,
                                  step: ToolStep(record: call, running: live, maybe: !live), turn: key))
            }
        }
        items.sort { $0.at == $1.at ? $0.order < $1.order : $0.at < $1.at }
        var lastCard: String?
        for item in items {
            guard let step = item.step, let key = item.turn else {
                rows.append(item.record); lastCard = nil; continue
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
                lastCard = item.record.id
                rows.append(item.record)
                cards[item.record.id] = ToolCard(id: item.record.id, turn: key, steps: [step], since: item.record.at, until: nil, words: preface)
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
        let liveCards = Set(cards.values.filter(\.running).map(\.turn))
        for (key, state) in timeline.turns.sorted(by: { ($0.value.startAt ?? 0, $0.key.agent, $0.key.id) < ($1.value.startAt ?? 0, $1.key.agent, $1.key.id) }) where state.running && ready {
            let closed = Set((work[key] ?? []).filter { $0.body["kind"].string == "turn.prompt.closed" }.compactMap { $0.body["prompt"].string })
            let prompts = (work[key] ?? []).filter { $0.body["kind"].string == "turn.prompt" && !closed.contains($0.body["prompt"].string ?? "") }
            let notBefore = prompts.compactMap { record -> Int64? in
                if case .number(let at) = record.body["expiresAt"], at.isFinite, at > Double(Int64.min), at < Double(Int64.max) { return Int64(at) }
                return nil
            }.max()
            if waiting.contains(key) && notBefore == nil { continue }
            if let asks = asked[key] { hints.append(.init(id: key, kind: .asked(asks))); continue }
            if liveCards.contains(key) { continue }
            let asker = work[key]?.first { $0.body["kind"].string == "turn.start" }?.body["asker"].string
            hints.append(.init(id: key, kind: .working(asker: asker), notBefore: notBefore))
        }
    }

    /// A successful post awaiting its agent's claim, as the web's pending
    /// line. It is not an outbox spinner: refused/unsent posts stay separate.
    static func pending(_ timeline: Timeline, principal: String, lead: String?) -> WorkingHint? {
        guard let message = timeline.records.last(where: { $0.isMessage }), message.principal == principal else { return nil }
        let target = message.body["to"].array.first?.string ?? lead
        guard let target else { return nil }
        if timeline.turns.contains(where: { $0.key.agent == target && $0.value.cause["channel"].string == "chat" && $0.value.cause["seq"] == .number(Double(message.seq)) }) { return nil }
        if timeline.records.contains(where: { $0.channel == "work" && $0.principal == target && $0.body["kind"].string == "turn.start" && $0.body["cause"]["seq"] == .number(Double(message.seq)) }) { return nil }
        return WorkingHint(id: .init(agent: target, id: "pending:\(message.id)"), kind: .pending(since: message.at))
    }
}
