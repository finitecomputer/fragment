import SwiftUI

/// The web chat's activity surfaces. All temporal/record decisions are in
/// WorkingActivity; this view owns only expansion and animation clocks.
struct WorkingIndicator: View {
    struct Agent: Identifiable {
        let id: TurnKey
        let name: String
        var home: String? = nil
    }

    let agents: [Agent]
    var hints: [WorkingHint] = []
    var cards: [ToolCard] = []
    var principal = ""
    var phase: String?
    var present: Set<String> = []
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var colorScheme
    @State private var began = Date()
    @ScaledMetric(relativeTo: .caption) private var textSize = 13.0
    @State private var expanded = false
    @State private var openSteps = Set<String>()
    @State private var advancedAt: Date?
    @State private var fittedArguments: [String: String] = [:]
    private struct ArgumentFit: Equatable {
        let width: CGFloat
        let args: String
        let verb: String
        let clock: String
        let size: CGFloat
    }

    init(timeline: Timeline, library: Library) {
        agents = timeline.turns.filter { $0.value.running }
            .sorted {
                if $0.value.startAt != $1.value.startAt { return $0.value.startAt! < $1.value.startAt! }
                if $0.key.agent != $1.key.agent { return $0.key.agent < $1.key.agent }
                return $0.key.id < $1.key.id
            }
            .map { Agent(id: $0.key, name: library.agent(for: $0.key.agent)?.displayTitle ?? "The agent") }
        hints = WorkingActivity(timeline).hints
    }

    init(agents: [Agent]) {
        self.agents = agents
        hints = agents.map { .init(id: $0.id, kind: .working(asker: nil)) }
    }

    init(card: ToolCard, agent: FragmentSummary?) {
        agents = [Agent(id: card.turn, name: agent?.displayTitle ?? "The agent", home: agent?.name)]
        cards = [card]
    }

    init(hints: [WorkingHint], card: ToolCard? = nil, library: Library, principal: String = "", phase: String? = nil, present: Set<String> = []) {
        self.hints = hints; self.cards = card.map { [$0] } ?? []
        self.principal = principal; self.phase = phase; self.present = present
        let keys = hints.map(\.id) + self.cards.map(\.turn)
        agents = keys.map { Agent(id: $0, name: library.agent(for: $0.agent)?.displayTitle ?? "The agent", home: library.agent(for: $0.agent)?.name) }
    }

    var body: some View {
        Group {
            if hints.isEmpty && cards.isEmpty {
                EmptyView()
            } else if cards.allSatisfy({ !$0.running }) && hints.allSatisfy({ if case .asked = $0.kind { true } else { false } }) {
                content(now: Date(), elapsed: 0)
            } else if reduceMotion {
                TimelineView(.periodic(from: .now, by: 1)) { frame in content(now: frame.date, elapsed: 0) }
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 60)) { frame in
                    content(now: frame.date, elapsed: frame.date.timeIntervalSince(began))
                }
            }
        }
        // A newly created web activity node starts its CSS clocks at zero.
        .onChange(of: hints) { _, _ in began = Date() }
        .onChange(of: cards.map(\.id)) { _, _ in began = Date(); expanded = false; openSteps = []; fittedArguments = [:] }
        .onChange(of: reduceMotion) { _, _ in began = Date(); advancedAt = nil }
        .onChange(of: cards.first?.steps) { old, new in
            began = Date()
            if (new?.count ?? 0) > (old?.count ?? 0) { advancedAt = Date() }
        }
    }

    private func content(now: Date, elapsed: TimeInterval) -> some View {
        VStack(alignment: .leading, spacing: 20) {
            ForEach(cards) { card in toolCard(card, now: now, elapsed: elapsed) }
            ForEach(hints.filter { ($0.notBefore ?? Int64.min) <= Int64(now.timeIntervalSince1970 * 1000) }) { hint in
                let agent = agents.first { $0.id == hint.id } ?? Agent(id: hint.id, name: "The agent")
                hintLine(agent, hint: hint, now: now, elapsed: elapsed)
            }
        }
    }

    private var secondary: Color { Color(rgb: colorScheme == .dark ? 0xa6a19d : 0x66615b) }
    private var stepColor: Color { Color(rgb: colorScheme == .dark ? 0x85817d : 0x726c66) }
    private func name(_ identity: String) -> String { agents.first { $0.id.agent == identity }?.name ?? "npub…" + identity.suffix(6) }

    private func hintLine(_ agent: Agent, hint: WorkingHint, now: Date, elapsed: TimeInterval) -> some View {
        let milliseconds = Int64(now.timeIntervalSince1970 * 1000)
        let late: Bool = { if case .pending(let since) = hint.kind { return phase != "starting" && phase != "asleep" && milliseconds - since >= 90_000 }; return false }()
        let text: String = {
            switch hint.kind {
            case .working(let asker): return "\(agent.name) is working" + (asker != nil && asker != principal ? " for \(name(asker!))" : "")
            case .asked(let asks): return asks == principal ? "Waiting for your answer" : "Waiting for \(name(asks))'s answer"
            case .pending:
                if phase == "wont_wake" { return "\(agent.name)'s computer could not start. Open the computer settings to restart it." }
                if late { return "\(agent.name) has not started a reply yet." }
                if phase == "starting" || phase == "asleep" { return "\(agent.name) is starting up…" }
                if phase == "sleeping" { return "\(agent.name)'s computer is going to sleep…" }
                return present.contains(agent.id.agent) ? "Waiting for \(agent.name) to start a reply…" : "Waiting for \(agent.name)…"
            }
        }()
        return HStack(spacing: 8) {
            if case .asked = hint.kind {
                glyph("asked", size: 15).foregroundStyle(Color(rgb: Self.colorRGB(agent.id.agent)))
            } else if !late && phase != "wont_wake" {
                dots(identity: agent.id.agent, elapsed: elapsed)
            }
            Text(text).font(.system(size: textSize, weight: .semibold)).foregroundStyle(secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(minHeight: textSize * 1.5)
        .accessibilityElement(children: .combine).accessibilityIdentifier("workingIndicator")
    }

    private func dots(identity: String, elapsed: TimeInterval) -> some View {
        HStack(spacing: 3) {
            ForEach(0..<3) { dot in
                Circle().fill(Color(rgb: Self.colorRGB(identity)))
                    .frame(width: 5, height: 5)
                    .offset(y: Self.offset(elapsed: elapsed, dot: dot))
            }
        }.accessibilityHidden(true)
    }

    private func glyph(_ icon: String, size: CGFloat = 13) -> some View {
        Image((size == 13 ? "WorkingStep-" : "Working-") + icon).renderingMode(.template).resizable().frame(width: size, height: size)
    }

    private func shimmer(_ text: String, elapsed: TimeInterval, active: Bool, tone: Color? = nil) -> some View {
        Text(text).foregroundStyle(tone ?? (active ? secondary : stepColor))
            .overlay {
                if active && !reduceMotion {
                    GeometryReader { geometry in
                        let progress = elapsed.truncatingRemainder(dividingBy: 2.4) / 2.4
                        LinearGradient(stops: [.init(color: secondary, location: 0), .init(color: secondary, location: 0.4), .init(color: Color(rgb: colorScheme == .dark ? 0xececec : 0x171717), location: 0.5), .init(color: secondary, location: 0.6), .init(color: secondary, location: 1)], startPoint: .leading, endPoint: .trailing)
                            .frame(width: geometry.size.width * 2.5)
                            .offset(x: geometry.size.width * (-1.5 + 3.75 * progress))
                    }.mask(Text(text))
                }
            }
    }

    private func toolCard(_ card: ToolCard, now: Date, elapsed: TimeInterval) -> some View {
        let rolling = card.running && !expanded
        let advance = advancedAt.map { min(1, max(0, now.timeIntervalSince($0) / 0.32)) } ?? 1
        let eased = reduceMotion ? 1 : UnitCurve.bezier(startControlPoint: UnitPoint(x: 0.25, y: 0.1), endControlPoint: UnitPoint(x: 0.25, y: 1)).value(at: advance)
        let shown = expanded ? card.steps : Array(card.steps.suffix(card.running ? (eased < 1 ? 4 : 3) : 0))
        return VStack(alignment: .leading, spacing: 0) {
            Button { expanded.toggle(); advancedAt = nil } label: {
                HStack(spacing: 8) {
                    if card.running {
                        let progress = reduceMotion ? 0 : UnitCurve.easeOut.value(at: elapsed.truncatingRemainder(dividingBy: 1.8) / 1.8)
                        Circle().fill(Color(rgb: Self.colorRGB(card.turn.agent)))
                            .background {
                                if !reduceMotion {
                                    Circle().fill(Color(rgb: Self.colorRGB(card.turn.agent)).opacity(0.55 * (1 - progress)))
                                        .frame(width: 7 + 14 * progress, height: 7 + 14 * progress)
                                }
                            }
                            .frame(width: 7, height: 7).padding(.leading, 6).padding(.trailing, 3)
                        shimmer("Working for " + Self.duration(Int64(now.timeIntervalSince1970 * 1000) - card.since), elapsed: elapsed, active: true)
                    } else {
                        HStack(spacing: -6) {
                            ForEach(Self.icons(card.steps), id: \.self) { icon in
                                glyph(icon, size: 11).frame(width: 20, height: 20)
                                    .background(Color(rgb: colorScheme == .dark ? 0x262625 : 0xf0eee9), in: Circle())
                                    .overlay(Circle().strokeBorder(FragmentStyle.background, lineWidth: 1.5))
                            }
                        }
                        Text("Worked for " + Self.duration((card.until ?? card.since) - card.since))
                    }
                    Text("· \(card.steps.count) \(card.steps.count == 1 ? "step" : "steps")")
                    glyph("chevron", size: 14).rotationEffect(.degrees(expanded ? 90 : 0)).animation(.easeInOut(duration: 0.14), value: expanded)
                }.frame(minHeight: 24).contentShape(Rectangle())
            }.buttonStyle(.plain).accessibilityIdentifier("workingTools")
            if !shown.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(shown) { step in
                        let age = card.steps.count - 1 - (card.steps.firstIndex { $0.id == step.id } ?? 0)
                        let opacity = !rolling || age == 0 ? 1.0 : age == 1 ? 0.6 : age == 2 ? 0.3 : 0
                        toolLine(step, agent: card.turn.agent, now: now, elapsed: elapsed, rolling: rolling, verticalPadding: rolling && eased < 1 && age == 3 ? 3 * (1 - eased) : 3)
                            .opacity(rolling && eased < 1 ? (age == 0 ? eased : age == 1 ? 1 - 0.4 * eased : age == 2 ? 0.6 - 0.3 * eased : 0.3 * (1 - eased)) : opacity)
                            .frame(height: rolling && eased < 1 && age == 0 ? max(6, min(textSize * 1.55 + 6, 3 * textSize * eased)) : rolling && eased < 1 && age == 3 ? min(textSize * 1.55 + 6 * (1 - eased), 3 * textSize * (1 - eased)) : nil, alignment: .top).clipped()
                    }
                }.padding(.top, 6)
            }
        }
        .font(.system(size: textSize)).foregroundStyle(stepColor)
    }

    private func toolLine(_ step: ToolStep, agent: String, now: Date, elapsed: TimeInterval, rolling: Bool, verticalPadding: Double = 3) -> some View {
        let size = textSize
        let detailed = openSteps.contains(step.id) && !rolling
        let home = agents.first { $0.id.agent == agent }?.home ?? agent
        let args = step.args.replacingOccurrences(of: "/data/work/\(home)/home/", with: "~/")
            .replacingOccurrences(of: "/data/work/\(home)/", with: "work/")
            .replacingOccurrences(of: "/data/hermes/managed-skills/", with: "skills/")
        let showsVerb = args.isEmpty || step.icon == "wrench" || step.failed || step.maybe
        let clock = step.running && step.record.body["kind"].string == "turn.call" ? "· " + Self.duration(Int64(now.timeIntervalSince1970 * 1000) - step.record.at) : ""
        let failure = Color(rgb: colorScheme == .dark ? 0xf3b3b3 : 0xa8324f)
        return VStack(alignment: .leading, spacing: 4) {
            Button { if !rolling { if detailed { openSteps.remove(step.id) } else { openSteps.insert(step.id) } } else { expanded = true } } label: {
                HStack(spacing: 7) {
                    glyph(step.icon).accessibilityLabel(step.verb)
                    HStack(spacing: textSize * 0.3) {
                        if showsVerb { shimmer(step.label, elapsed: elapsed, active: step.running && !step.failed, tone: step.failed ? failure : nil) }
                        if !args.isEmpty { shimmer(fittedArguments[step.id] ?? args, elapsed: elapsed, active: step.running && !step.failed, tone: step.failed ? failure : nil).lineLimit(1).truncationMode(.tail) }
                        if step.running && step.record.body["kind"].string == "turn.call" { Text("· " + Self.duration(Int64(now.timeIntervalSince1970 * 1000) - step.record.at)).foregroundStyle(.tertiary).monospacedDigit() }
                    }
                }.frame(maxWidth: .infinity, alignment: .leading).frame(minHeight: 20.15)
                    .onGeometryChange(for: ArgumentFit.self) { geometry in
                        ArgumentFit(width: geometry.size.width, args: args, verb: showsVerb ? step.label : "", clock: clock, size: size)
                    } action: { fit in
                        let font = UIFont.systemFont(ofSize: fit.size)
                        let verb = fit.verb.isEmpty ? 0 : (fit.verb as NSString).size(withAttributes: [.font: font]).width + fit.size * 0.3
                        let clock = fit.clock.isEmpty ? 0 : (fit.clock as NSString).size(withAttributes: [.font: UIFont.monospacedDigitSystemFont(ofSize: fit.size, weight: .regular)]).width + fit.size * 0.3
                        let text = Self.fitPaths(fit.args, width: fit.width - 20 - verb - clock, font: font)
                        if fittedArguments[step.id] != text { fittedArguments[step.id] = text }
                    }
            }.buttonStyle(.plain).foregroundStyle(step.failed ? failure : step.running ? secondary : stepColor)
            if detailed {
                VStack(alignment: .leading, spacing: 10) {
                    if !args.isEmpty { Text(args) }
                    if let result = step.record.body["excerpt"].string, !result.isEmpty { Text(result) }
                }.font(.system(size: 12, design: .monospaced)).textSelection(.enabled).padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading).background(FragmentStyle.bubble, in: RoundedRectangle(cornerRadius: 8))
            }
        }
        .padding(.vertical, verticalPadding).padding(.leading, 16)
        .overlay(alignment: .leading) { Rectangle().fill(Color.primary.opacity(colorScheme == .dark ? 0.16 : 0.18)).frame(width: 1) }
        .padding(.leading, 9)
    }

    // chat.js middlePaths/fitStepArgs: shorten directories before applying
    // the line's normal ellipsis, so a filename survives on a narrow phone.
    // Geometry changes (and a clock changing width) fit once, not every tick.
    private static let paths = try! NSRegularExpression(pattern: #"(^|[\s=("'`])((?:~/|/|work/|skills/)[^\s"'`<>]+)"#)
    static func middlePaths(_ text: String, room: Int) -> String {
        let result = NSMutableString(string: text)
        for match in paths.matches(in: text, range: NSRange(location: 0, length: (text as NSString).length)).reversed() {
            let range = match.range(at: 2)
            let path = (text as NSString).substring(with: range) as NSString
            let slash = path.range(of: "/", options: .backwards).location
            guard path.length > room, slash > 0, slash != NSNotFound, path.length - slash + 2 < path.length else { continue }
            let tail = path.substring(from: slash)
            let head = path.range(of: "/").location + 1
            result.replaceCharacters(in: range, with: path.substring(to: max(head, room - (tail as NSString).length - 1)) + "…" + tail)
        }
        return result as String
    }
    private static func fitPaths(_ text: String, width: CGFloat, font: UIFont) -> String {
        guard width > 0, (text as NSString).size(withAttributes: [.font: font]).width > width else { return text }
        var low = 1, high = (text as NSString).length
        while low < high {
            let room = (low + high + 1) / 2
            let trial = middlePaths(text, room: room)
            if (trial as NSString).size(withAttributes: [.font: font]).width <= width { low = room }
            else { high = room - 1 }
        }
        return middlePaths(text, room: low)
    }

    private static func icons(_ steps: [ToolStep]) -> [String] {
        var result: [String] = []
        for step in steps where !result.contains(step.icon) { if result.count < 4 { result.append(step.icon) } }
        return result
    }

    static func duration(_ milliseconds: Int64) -> String {
        let seconds = max(1, Int((Double(milliseconds) / 1000).rounded()))
        if seconds < 60 { return "\(seconds)s" }
        let minutes = seconds / 60
        if minutes < 60 { return "\(minutes)m " + String(format: "%02ds", seconds % 60) }
        return "\(minutes / 60)h \(minutes % 60)m"
    }

    // CSS bounce: 0/60/100% at rest, 30% at -3px. Each segment uses
    // cubic-bezier(.42, 0, .58, 1), including the initial positive delays.
    // A shared clock avoids phase drift across the three dots and rerenders.
    static func offset(elapsed: TimeInterval, dot: Int) -> Double {
        let local = elapsed - Double(dot) * 0.13
        guard local > 0 else { return 0 }
        let phase = local.truncatingRemainder(dividingBy: 1.15) / 1.15
        if phase < 0.3 { return -3 * UnitCurve.easeInOut.value(at: phase / 0.3) }
        if phase < 0.6 { return -3 * (1 - UnitCurve.easeInOut.value(at: (phase - 0.3) / 0.3)) }
        return 0
    }

    /// `agent-color.js`: FNV-1a over UTF-16, not the avatar's name hash.
    static func colorRGB(_ identity: String?) -> UInt32 {
        let colors: [UInt32] = [0xa88bea, 0x62c8af, 0xeda978, 0x80afe9, 0xdc91b6, 0xb7c878]
        guard let identity, !identity.isEmpty else { return colors[0] }
        let hash = identity.utf16.reduce(UInt32(0x811c9dc5)) { ($0 ^ UInt32($1)) &* 0x01000193 }
        return colors[Int(hash % UInt32(colors.count))]
    }
}

private extension Color {
    init(rgb: UInt32) {
        self.init(.sRGB, red: Double((rgb >> 16) & 255) / 255,
                  green: Double((rgb >> 8) & 255) / 255, blue: Double(rgb & 255) / 255, opacity: 1)
    }
}

#Preview {
    WorkingIndicator(agents: [.init(id: TurnKey(agent: "npub1preview", id: "preview"), name: "Juniper")])
        .padding(20)
}
