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
    /// How many of its steps are shown while it opens, one after another;
    /// `.max` once they all are (new steps then show at once).
    @State private var revealed = 0
    @State private var revealing: Task<Void, Never>?
    @State private var closed = 0
    /// The steps that open one by one, each with a tick; past them, the rest at once.
    static let staggered = 10
    @State private var openSteps = Set<String>()
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
        .onChange(of: reduceMotion) { _, _ in began = Date() }
        .onChange(of: cards.first?.steps) { _, _ in began = Date() }
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
            case .working(let asker): return "Thinking" + (asker != nil && asker != principal ? " for \(name(asker!))" : "")
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
            } else if case .pending = hint.kind, !late && phase != "wont_wake" {
                dots(identity: agent.id.agent, elapsed: elapsed)
            }
            // Thinking reads as the working line does; a wait still stands out
            Text(text).font(.system(size: textSize, weight: { if case .working = hint.kind { return .regular } else { return .semibold } }())).foregroundStyle(secondary)
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
        // one line until it's opened: what it does now, never the steps rolling by
        let shown = expanded ? Array(card.steps.prefix(revealed)) : []
        return VStack(alignment: .leading, spacing: 0) {
            Button { toggle(steps: card.steps.count) } label: {
                HStack(spacing: 6) {
                    if card.running {
                        // a word for what it does now, held long enough to read
                        HeldWord(word: card.nowWord) { shimmer($0, elapsed: elapsed, active: true) }
                        Text("· " + Self.duration(Int64(now.timeIntervalSince1970 * 1000) - card.since)).monospacedDigit()
                    } else {
                        Text(card.doneWord + " for " + Self.duration((card.until ?? card.since) - card.since))
                    }
                    glyph("chevron", size: 14).rotationEffect(.degrees(expanded ? 90 : 0)).animation(.easeInOut(duration: 0.14), value: expanded)
                }.frame(minHeight: 24).contentShape(Rectangle())
            }.buttonStyle(.plain).accessibilityIdentifier("workingTools")
            if !shown.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(shown) { step in
                        toolLine(step, agent: card.turn.agent, now: now, elapsed: elapsed, rolling: false)
                            .transition(.asymmetric(insertion: .opacity.combined(with: .offset(y: -6)), removal: .opacity))
                    }
                }.padding(.top, 6)
            }
        }
        .font(.system(size: textSize)).foregroundStyle(stepColor)
        // a tick as each step arrives, and a soft tap as they fold away
        .sensoryFeedback(trigger: revealed) { old, new in new > old && new <= Self.staggered ? .selection : nil }
        .sensoryFeedback(.impact(weight: .light), trigger: closed)
    }

    /// Opens its steps one after another (all at once with Reduce Motion),
    /// or folds them away.
    private func toggle(steps: Int) {
        revealing?.cancel()
        if expanded {
            withAnimation(reduceMotion ? nil : .snappy(duration: 0.2)) { expanded = false; revealed = 0 }
            closed += 1
            return
        }
        expanded = true
        guard !reduceMotion else { revealed = .max; return }
        revealed = 0
        revealing = Task {
            for count in 1...max(1, min(steps, Self.staggered)) {
                withAnimation(.snappy(duration: 0.28)) { revealed = count }
                try? await Task.sleep(for: .milliseconds(55))
                if Task.isCancelled { return }
            }
            withAnimation(.snappy(duration: 0.28)) { revealed = .max }
        }
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
                        if !args.isEmpty { shimmer(fittedArguments[step.id] ?? Self.singleLine(args), elapsed: elapsed, active: step.running && !step.failed, tone: step.failed ? failure : nil).lineLimit(1).truncationMode(.tail) }
                        if step.running && step.record.body["kind"].string == "turn.call" { Text("· " + Self.duration(Int64(now.timeIntervalSince1970 * 1000) - step.record.at)).foregroundStyle(.tertiary).monospacedDigit() }
                    }
                }.frame(maxWidth: .infinity, alignment: .leading).frame(minHeight: 20.15)
                    .onGeometryChange(for: ArgumentFit.self) { geometry in
                        ArgumentFit(width: geometry.size.width, args: args, verb: showsVerb ? step.label : "", clock: clock, size: size)
                    } action: { fit in
                        let font = UIFont.systemFont(ofSize: fit.size)
                        let verb = fit.verb.isEmpty ? 0 : (fit.verb as NSString).size(withAttributes: [.font: font]).width + fit.size * 0.3
                        let clock = fit.clock.isEmpty ? 0 : (fit.clock as NSString).size(withAttributes: [.font: UIFont.monospacedDigitSystemFont(ofSize: fit.size, weight: .regular)]).width + fit.size * 0.3
                        let text = Self.fitPaths(Self.singleLine(fit.args), width: fit.width - 20 - verb - clock, font: font)
                        if fittedArguments[step.id] != text { fittedArguments[step.id] = text }
                    }
            }.buttonStyle(.plain).foregroundStyle(step.failed ? failure : step.running ? secondary : stepColor)
            if detailed {
                VStack(alignment: .leading, spacing: 10) {
                    if !args.isEmpty { detailText(args, tone: secondary) }
                    if let result = step.record.body["excerpt"].string, !result.isEmpty { detailText(result, tone: Color(rgb: colorScheme == .dark ? 0x706d69 : 0x918a82)) }
                }.padding(.vertical, 10).padding(.horizontal, 12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color(rgb: colorScheme == .dark ? 0x2b2b2b : 0xece8df), in: RoundedRectangle(cornerRadius: 8))
                    .padding(.leading, 20).padding(.bottom, 6)
            }
        }
        .padding(.vertical, verticalPadding).padding(.leading, 17)
        .overlay(alignment: .leading) { Rectangle().fill(Color(rgb: colorScheme == .dark ? 0xffffff : 0x141414).opacity(colorScheme == .dark ? 0.16 : 0.18)).frame(width: 1) }
        .padding(.leading, 9)
    }

    private func detailText(_ text: String, tone: Color) -> some View {
        let leading = max(0, 19.2 - UIFont.monospacedSystemFont(ofSize: 12, weight: .regular).lineHeight)
        let words = Text(text).font(.system(size: 12, design: .monospaced)).lineSpacing(leading)
            .fixedSize(horizontal: false, vertical: true).padding(.vertical, leading / 2)
        // Measure the words at their natural height, capped like the web's
        // pre, then scroll in that space. A short result must not stretch.
        return words.hidden().accessibilityHidden(true)
            .frame(maxWidth: .infinity, maxHeight: 240, alignment: .topLeading)
            .fixedSize(horizontal: false, vertical: true)
            .overlay(alignment: .topLeading) {
                ScrollView { words.frame(maxWidth: .infinity, alignment: .leading) }
                    .scrollBounceBehavior(.basedOnSize)
            }.clipped().foregroundStyle(tone).textSelection(.enabled)
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
    // CSS white-space: nowrap collapses command newlines in the list;
    // the expanded detail still shows the original formatting.
    static func singleLine(_ text: String) -> String {
        text.replacingOccurrences(of: #"[\t\n\f\r ]+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: CharacterSet(charactersIn: " "))
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

/// A word that changes as the work does, each held at least a moment so a
/// quick step never flickers past, and changed with a fade; drawn by
/// `content` (the working line's shimmer).
private struct HeldWord<Content: View>: View {
    let word: String
    @ViewBuilder let content: (String) -> Content
    @State private var shown: String?
    @State private var since = Date.distantPast
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    static var hold: TimeInterval { 1.5 }

    var body: some View {
        content(shown ?? word)
            .id(shown ?? word)
            .transition(.opacity)
            .animation(reduceMotion ? nil : .easeInOut(duration: 0.25), value: shown)
            .task(id: word) {
                let wait = Self.hold - Date().timeIntervalSince(since)
                if shown != nil, wait > 0 { try? await Task.sleep(for: .seconds(wait)) }
                guard !Task.isCancelled else { return }
                shown = word; since = Date()
            }
    }
}
