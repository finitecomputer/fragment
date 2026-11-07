import SwiftUI

/// The helpers (children an agent's runtime delegated work to) its turns
/// started (`turn.child`, docs/chat-records.md), from every `work` record
/// read, in either order: a page of earlier records brings a start after
/// its end, which stays ended. A helper is its poster's, whatever a body
/// names, and works from its start until any outcome.
struct Helpers: Equatable, Sendable {
    struct Helper: Equatable, Identifiable, Sendable {
        let agent: String
        let turn: String
        let child: String
        var goal: String?
        var at: Int64 = 0
        var ended = false
        var id: String { "\(agent)|\(turn)|\(child)" }
    }
    /// Those half read (a start, or an end, alone), by `id`: one read
    /// whole is let go, so this holds the running and what an earlier page
    /// may still finish. Past `keptMax`, the oldest go.
    private(set) var open: [String: Helper] = [:]
    static let keptMax = 1_024

    /// Whether the record changed what is known.
    mutating func learn(_ record: ChannelRecord) -> Bool {
        guard record.channel == "work", record.body["kind"].string == "turn.child",
              let turn = record.turn, let child = record.body["child"].string, !child.isEmpty else { return false }
        let id = "\(record.principal)|\(turn)|\(child)"
        var helper = open[id] ?? Helper(agent: record.principal, turn: turn, child: child)
        if record.body["outcome"].string != nil { helper.ended = true }
        else if let goal = record.body["goal"].string { helper.goal = goal; helper.at = record.at }
        else { return false }
        if helper.ended && helper.goal != nil { open.removeValue(forKey: id) } else { open[id] = helper }
        if open.count > Self.keptMax, let oldest = open.values.min(by: { ($0.at, $0.id) < ($1.at, $1.id) }) { open.removeValue(forKey: oldest.id) }
        return true
    }

    /// The helpers working now, the oldest first.
    var running: [Helper] {
        open.values.filter { $0.goal != nil && !$0.ended }.sorted { ($0.at, $0.id) < ($1.at, $1.id) }
    }
}

/// The helpers working now, in one quiet row just above the composer: how
/// many, and tapped, each one's task on a line (cut short) and how long it
/// has run, at most `shownMax`, then how many more. In a chat of several
/// agents, whose they are, by face. None working, nothing shows.
struct HelpersBar: View {
    let helpers: [Helpers.Helper]
    let library: Library
    /// The chat has several agents (or its helpers are several agents').
    let several: Bool
    @State private var open = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    static let shownMax = 4
    private var group: Bool { several || agents.count > 1 }

    var body: some View {
        if !helpers.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Button { withAnimation(reduceMotion ? nil : .snappy(duration: 0.2)) { open.toggle() } } label: {
                    HStack(spacing: 8) {
                        if group { faces } else { glyph("WorkingStep-users").foregroundStyle(.secondary) }
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
                    // minutes: looked at again every 30 s, never per frame
                    TimelineView(.periodic(from: .now, by: 30)) { context in
                        VStack(alignment: .leading, spacing: 6) {
                            ForEach(helpers.prefix(Self.shownMax)) { helper in
                                HStack(spacing: 8) {
                                    if group { AgentAvatar(agent: library.agent(for: helper.agent), size: 16) }
                                    Text(helper.goal?.isEmpty == false ? helper.goal! : "A task").foregroundStyle(.primary).lineLimit(1).truncationMode(.tail)
                                    Spacer(minLength: 4)
                                    Text(Self.minutes(Int64(context.date.timeIntervalSince1970 * 1000) - helper.at)).foregroundStyle(.tertiary).monospacedDigit()
                                }
                                .accessibilityElement(children: .combine)
                                .accessibilityIdentifier("helper")
                            }
                            if helpers.count > Self.shownMax {
                                Text("and \(helpers.count - Self.shownMax) more").foregroundStyle(.tertiary).accessibilityIdentifier("helpersMore")
                            }
                        }
                        .padding(.leading, group ? 0 : 21).padding(.bottom, 8)
                    }
                }
            }
            .font(.caption).foregroundStyle(.secondary)
            .padding(.horizontal, 12)
            .background(FragmentStyle.surface, in: RoundedRectangle(cornerRadius: 14))
        }
    }

    /// "2 helpers working", and in a group whose: "Ada and Bo: 3 helpers working".
    var said: String { Self.said(helpers, names: group ? agents.map { library.agent(for: $0)?.displayTitle ?? "Agent" } : []) }

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

    private var agents: [String] {
        var seen: [String] = []
        for helper in helpers where !seen.contains(helper.agent) { seen.append(helper.agent) }
        return seen
    }

    private var faces: some View {
        HStack(spacing: -5) {
            ForEach(agents.prefix(5), id: \.self) { AgentAvatar(agent: library.agent(for: $0), size: 16) }
        }
    }

    private func glyph(_ name: String, size: CGFloat = 13) -> some View {
        Image(name).renderingMode(.template).resizable().frame(width: size, height: size).accessibilityHidden(true)
    }
}
