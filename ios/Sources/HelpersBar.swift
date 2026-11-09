import SwiftUI

/// Each agent's newest helpers set (`turn.helpers`, docs/chat-records.md):
/// every helper (a child its runtime delegated work to) it has working in
/// this chat, as its computer's bridge holds them, shown until `until`. A
/// set is its poster's, whatever a body names, and the newest of each
/// poster wins, however records come (the newest asked for as a chat
/// opens, `ChatSession.newestHelpers`, the live stream, earlier pages).
struct Helpers: Equatable, Sendable {
    struct Helper: Equatable, Identifiable, Sendable {
        let agent: String
        let child: String
        let goal: String
        /// When its bridge saw it start (ms).
        let since: Int64
        var id: String { "\(agent)|\(child)" }
    }
    private struct Said: Equatable, Sendable {
        let seq: Int64
        let until: Int64
        let helpers: [Helper]
    }
    /// One a poster: as many as the chat's agents.
    private var sets: [String: Said] = [:]

    /// A time a set says (ms): 0 to 2^53 (a reader's safe integer), else
    /// none: never a trap converting it, nor an overflow subtracting it.
    private static func ms(_ value: JSONValue) -> Int64? {
        guard case .number(let ms) = value, ms >= 0, ms <= 9_007_199_254_740_991 else { return nil }
        return Int64(ms)
    }

    /// Whether the record changed what is known.
    mutating func learn(_ record: ChannelRecord) -> Bool {
        guard record.channel == "work", record.body["kind"].string == "turn.helpers", let until = Self.ms(record.body["until"]),
              case .array(let list) = record.body["helpers"], (sets[record.principal]?.seq ?? 0) < record.seq else { return false }
        let helpers = list.compactMap { h -> Helper? in
            guard let child = h["child"].string, let goal = h["goal"].string, let since = Self.ms(h["since"]) else { return nil }
            return Helper(agent: record.principal, child: child, goal: goal, since: since)
        }
        sets[record.principal] = Said(seq: record.seq, until: until, helpers: helpers)
        return true
    }

    /// The helpers working at `now` (ms): each agent's newest set until its
    /// `until`, the oldest first. A set that lapsed (its computer crashed)
    /// says nothing.
    func working(at now: Int64) -> [Helper] {
        sets.values.filter { $0.until > now }.flatMap(\.helpers).sorted { ($0.since, $0.id) < ($1.since, $1.id) }
    }

    var isEmpty: Bool { sets.values.allSatisfy(\.helpers.isEmpty) }
}

/// The helpers working now, in one quiet row just above the composer: how
/// many, and tapped, each one's task on a line (cut short) and how long it
/// has run, at most `shownMax`, then how many more. In a chat of several
/// agents, whose they are, by face. None working, nothing shows.
struct HelpersBar: View {
    /// Each agent's newest set; the bar shows those working now
    /// (`Helpers.working`).
    let helpers: Helpers
    let library: Library
    /// The chat has several agents (or its helpers are several agents').
    let several: Bool
    @State private var open = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    static let shownMax = 4

    var body: some View {
        if !helpers.isEmpty {
            // which work, and their minutes: looked at again every 30 s,
            // never per frame (a set whose `until` passed leaves)
            TimelineView(.periodic(from: .now, by: 30)) { context in
                let now = Int64(context.date.timeIntervalSince1970 * 1000)
                let working = helpers.working(at: now)
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
                        let goal = helper.goal.isEmpty ? "A task" : helper.goal
                        let ran = Self.minutes(now - helper.since)
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
