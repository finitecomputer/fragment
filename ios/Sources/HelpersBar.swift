import SwiftUI

/// The helpers (children an agent's runtime delegated work to) its turns
/// started (`turn.child`, docs/chat-records.md), from every `work` record
/// read, in any order and however often (an earlier page, a window paged
/// back to, earlier work read again, `ChatSession.recentHelpers`): a start
/// or note read after its end, or again after it, stays ended. A helper is
/// its poster's, whatever a body names, and works from its start (or a
/// note, its start again with `since`) until any outcome, shown while its
/// newest start or note is at most `readBack` old (`working`).
struct Helpers: Equatable, Sendable {
    struct Helper: Equatable, Identifiable, Sendable {
        let agent: String
        /// The turn that started it (a record's `parent`).
        let turn: String
        let child: String
        var goal: String?
        /// When it started: its start record's time, else a note's `since`.
        var at: Int64 = 0
        var started = false
        /// Its newest start or note's time: it works while this is at most
        /// `readBack` old, its bridge noting it more often than that.
        var seen: Int64 = 0
        var ended = false
        /// The newest of its records learned: what goes first past `keptMax`.
        var seq: Int64 = 0
        var id: String { "\(agent)|\(turn)|\(child)" }
    }
    /// Every helper read, ended ones too: an ended one kept is what keeps
    /// its start, read again, from working once more. Past `keptMax` (as
    /// many as the turns a timeline keeps), those learned longest ago go.
    private(set) var known: [String: Helper] = [:]
    static let keptMax = 4_096
    /// How old a helper's newest start or note may be for it to work, and
    /// how far back a chat opening reads `work` for them: its bridge notes
    /// a running helper every 10 minutes (docs/chat-records.md,
    /// `turn.child`; the contract's `CHILD_READ_BACK_MS`, images/bridge's
    /// limits.rs). The bounded version: a durable listing may come later.
    static let readBack: Int64 = 15 * 60 * 1000

    /// A note's `since`: none on a start; on a note, a whole number of ms
    /// above 0, at or before its record's `at`, else the record is none.
    private enum Since { case none, at(Int64), malformed }
    private static func since(_ record: ChannelRecord) -> Since {
        guard case .object(let body) = record.body, let value = body["since"] else { return .none }
        guard case .number(let ms) = value, ms >= 1, ms <= Double(record.at), ms.rounded() == ms else { return .malformed }
        return .at(Int64(ms))
    }

    /// Whether the record changed what is known.
    mutating func learn(_ record: ChannelRecord) -> Bool {
        guard record.channel == "work", record.body["kind"].string == "turn.child",
              let turn = record.body["parent"].string, let child = record.body["child"].string, !child.isEmpty else { return false }
        let id = "\(record.principal)|\(turn)|\(child)"
        var helper = known[id] ?? Helper(agent: record.principal, turn: turn, child: child)
        if record.body["outcome"].string != nil { helper.ended = true }
        else if let goal = record.body["goal"].string {
            switch Self.since(record) {
            case .malformed: return false
            case .none: helper.at = record.at; helper.started = true
            case .at(let since): if !helper.started { helper.at = since }
            }
            helper.goal = goal
            helper.seen = max(helper.seen, record.at)
        }
        else { return false }
        helper.seq = max(helper.seq, record.seq)
        guard known[id] != helper else { return false }
        known[id] = helper
        if known.count > Self.keptMax, let oldest = known.values.min(by: { ($0.seq, $0.id) < ($1.seq, $1.id) }) { known.removeValue(forKey: oldest.id) }
        return true
    }

    /// The helpers started and not ended, the oldest first.
    var running: [Helper] {
        known.values.filter { $0.goal != nil && !$0.ended }.sorted { ($0.at, $0.id) < ($1.at, $1.id) }
    }

    /// Of `running`, those working at `now` (ms): a start or note of theirs
    /// at most `readBack` old. One no note has come for since (its computer
    /// crashed) is none.
    static func working(_ running: [Helper], at now: Int64) -> [Helper] {
        running.filter { now - $0.seen <= readBack }
    }
}

/// The helpers working now, in one quiet row just above the composer: how
/// many, and tapped, each one's task on a line (cut short) and how long it
/// has run, at most `shownMax`, then how many more. In a chat of several
/// agents, whose they are, by face. None working, nothing shows.
struct HelpersBar: View {
    /// The helpers started and not ended (`Helpers.running`); the bar shows
    /// those working now (`Helpers.working`).
    let helpers: [Helpers.Helper]
    let library: Library
    /// The chat has several agents (or its helpers are several agents').
    let several: Bool
    @State private var open = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    static let shownMax = 4

    var body: some View {
        if !helpers.isEmpty {
            // which work, and their minutes: looked at again every 30 s,
            // never per frame (one no note came for in `Helpers.readBack`
            // leaves)
            TimelineView(.periodic(from: .now, by: 30)) { context in
                let now = Int64(context.date.timeIntervalSince1970 * 1000)
                let working = Helpers.working(helpers, at: now)
                if !working.isEmpty { bar(working, now: now) }
            }
        }
    }

    private func bar(_ working: [Helpers.Helper], now: Int64) -> some View {
        let agents = Self.agents(of: working)
        let group = several || agents.count > 1
        let said = Self.said(working, names: group ? agents.map(name) : [])
        return VStack(alignment: .leading, spacing: 6) {
            Button { withAnimation(reduceMotion ? nil : .snappy(duration: 0.2)) { open.toggle() } } label: {
                HStack(spacing: 8) {
                    if group { faces(agents) } else { glyph("WorkingStep-users").foregroundStyle(.secondary) }
                    Text(said).lineLimit(1).truncationMode(.tail)
                    Spacer(minLength: 4)
                    glyph("Working-chevron", size: 12).rotationEffect(.degrees(open ? 90 : -90)).foregroundStyle(.tertiary)
                }
                .frame(minHeight: 32).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(said)
            .accessibilityValue(open ? "Shown" : "Hidden")
            .accessibilityHint(open ? "Hides their tasks" : "Shows their tasks")
            .accessibilityIdentifier("helpersBar")
            if open {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(working.prefix(Self.shownMax)) { helper in
                        let goal = helper.goal?.isEmpty == false ? helper.goal! : "A task"
                        let ran = Self.minutes(now - helper.at)
                        HStack(spacing: 8) {
                            if group { AgentAvatar(agent: library.agent(for: helper.agent), size: 16) }
                            Text(goal).foregroundStyle(.primary).lineLimit(1).truncationMode(.tail)
                            Spacer(minLength: 4)
                            Text(ran).foregroundStyle(.tertiary).monospacedDigit()
                        }
                        .accessibilityElement(children: .combine)
                        // (its face is hidden: in a group, whose it is, said)
                        .accessibilityLabel(group ? "\(name(helper.agent)): \(goal), \(ran)" : "\(goal), \(ran)")
                        .accessibilityIdentifier("helper")
                    }
                    if working.count > Self.shownMax {
                        Text("and \(working.count - Self.shownMax) more").foregroundStyle(.tertiary).accessibilityIdentifier("helpersMore")
                    }
                }
                .padding(.leading, group ? 0 : 21).padding(.bottom, 8)
            }
        }
        .font(.caption).foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .background(FragmentStyle.surface, in: RoundedRectangle(cornerRadius: 14))
        // none working, it closes, as the web's does: the next ones come closed
        .onDisappear { open = false }
    }

    /// "2 helpers working", and in a group whose: "Ada and Bo: 3 helpers working".
    private func name(_ agent: String) -> String { library.agent(for: agent)?.displayTitle ?? "Agent" }

    static func said(_ helpers: [Helpers.Helper], names: [String]) -> String {
        let count = "\(helpers.count) \(helpers.count == 1 ? "helper" : "helpers") working"
        switch names.count {
        case 0: return count
        case 1: return "\(names[0]): \(count)"
        case 2: return "\(names[0]) and \(names[1]): \(count)"
        default: return "\(names[0]) and \(names.count - 1) others: \(count)"
        }
    }

    /// How long a helper has run, as the web's bar says it: "<1 min", "4 min", "1h 5m".
    static func minutes(_ milliseconds: Int64) -> String {
        let minutes = Int(max(0, milliseconds) / 60_000)
        if minutes < 1 { return "<1 min" }
        if minutes < 60 { return "\(minutes) min" }
        return "\(minutes / 60)h \(minutes % 60)m"
    }

    /// Whose helpers they are, in their order.
    static func agents(of helpers: [Helpers.Helper]) -> [String] {
        var seen: [String] = []
        for helper in helpers where !seen.contains(helper.agent) { seen.append(helper.agent) }
        return seen
    }

    private func faces(_ agents: [String]) -> some View {
        HStack(spacing: -5) {
            ForEach(agents.prefix(5), id: \.self) { AgentAvatar(agent: library.agent(for: $0), size: 16) }
        }
    }

    private func glyph(_ name: String, size: CGFloat = 13) -> some View {
        Image(name).renderingMode(.template).resizable().frame(width: size, height: size).accessibilityHidden(true)
    }
}
