import SwiftUI

/// One thing the platform tells the person (`GET /api/notices`, FIN-458),
/// in the server's own words, with what they may do about it. Every kind is
/// drawn the same way (`NoticesBand`): nothing here reads `kind`, so a new
/// kind is the server's alone, as on the web (cell/shell/computer.js
/// `noticeRow`).
struct Notice: Decodable, Identifiable, Equatable, Sendable {
    /// When what it tells ends by the clock alone: shown as `said`, then
    /// how long until `at` (`untilSaid`), and gone once it passes.
    struct Until: Decodable, Equatable, Sendable {
        let at: Int64
        let said: String
    }
    /// A route to post `body` to with this iPhone's key, or a page to open.
    struct Action: Decodable, Equatable, Sendable {
        let label: String
        var post: String?
        var body: JSONValue?
        var open: String?
    }
    let id: String
    let kind: String
    /// `warn`, or anything else (an older app's unknown level) drawn as `info`.
    let level: String
    let title: String
    var text: String?
    var detail: String?
    var until: Until?
    var actions: [Action]
    /// It must be answered: shown alone over the whole app
    /// (`MustAnswerView`), never in the band, until the read after an
    /// answer of it has it no more.
    var mustAnswer: Bool

    var warns: Bool { level == "warn" }

    private enum Keys: String, CodingKey { case id, kind, level, title, text, detail, until, actions, mustAnswer }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decode(String.self, forKey: .kind)
        level = try c.decode(String.self, forKey: .level)
        title = try c.decode(String.self, forKey: .title)
        text = try c.decodeIfPresent(String.self, forKey: .text)
        detail = try c.decodeIfPresent(String.self, forKey: .detail)
        until = try c.decodeIfPresent(Until.self, forKey: .until)
        actions = try c.decodeIfPresent([Action].self, forKey: .actions) ?? []
        mustAnswer = try c.decodeIfPresent(Bool.self, forKey: .mustAnswer) ?? false
    }

    /// Whether it still tells something at `now` (ms): one whose time
    /// passed is gone, though no one told this iPhone yet.
    func shown(at now: Int64) -> Bool { until.map { $0.at > now } ?? true }

    /// A countdown in words at `now` (ms), as the web says it
    /// (cell/shell/computer.js `untilSaid`): whole minutes up; the last
    /// minute, under a minute.
    static func untilSaid(_ until: Until, now: Int64) -> String {
        let left = until.at - now
        guard left > 60_000 else { return "\(until.said) in under a minute." }
        let minutes = (left + 59_999) / 60_000
        guard minutes > 60 else { return "\(until.said) in \(minutes) min." }
        let (h, m) = (minutes / 60, minutes % 60)
        return "\(until.said) in \(h) h\(m > 0 ? " \(m) min" : "")."
    }
}

/// An action of a notice that failed (`Library.act`): why, in the server's
/// words for its refusal (`refused`) or the app's for a call that never
/// reached it, and its notice's title and level, so a refusal can be said
/// after its notice is gone.
struct NoticeFailure: Equatable, Sendable {
    let said: String
    let refused: Bool
    let title: String
    let warns: Bool
}

/// What the person is told, over every page: each notice as the server
/// said it, the most pressing first, and a line when the last read of them
/// failed (what it said before stays under it, with Try again).
///
/// Each is one line, its title and its main action; a tap opens its words
/// and detail. Several stack as iOS stacks notifications: the first, with
/// the edges of the rest behind it, until a tap fans them out.
struct NoticesBand: View {
    let library: Library
    let api: FragmentAPI
    /// The most of the page it takes (`FragmentRoot`'s share of its
    /// height): past it, it scrolls, so the page and its toolbar stay.
    let maxHeight: CGFloat
    @State private var height: CGFloat = 0
    /// Every one shown, not only the first.
    @State private var fanned = false
    /// The ones whose words are open.
    @State private var opened: Set<String> = []
    @Environment(\.openURL) private var openURL
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private enum Item: Identifiable {
        case refresh, notice(Notice), refusal(String, NoticeFailure)
        var id: String {
            switch self { case .refresh: "refresh"; case .notice(let notice): "notice:" + notice.id; case .refusal(let id, _): "refused:" + id }
        }
    }

    var body: some View {
        // a countdown says its minutes live, and a notice whose time passed goes
        TimelineView(.periodic(from: .now, by: 15)) { context in
            let now = Int64(context.date.timeIntervalSince1970 * 1000)
            let items = (library.noticesFailed ? [Item.refresh] : [])
                + library.notices.filter { $0.shown(at: now) && !$0.mustAnswer }.map(Item.notice)
                + library.refusals(at: now).map { Item.refusal($0.id, $0.failure) }
            if let first = items.first {
                ScrollView {
                    VStack(spacing: 8) {
                        if fanned || items.count == 1 {
                            ForEach(items) { item($0, now: now, stacked: false) }
                            if items.count > 1 {
                                Button { fan(false) } label: {
                                    Text("Show less").font(.footnote.weight(.semibold)).padding(.horizontal, 12).padding(.vertical, 6)
                                }
                                .buttonStyle(.plain).glassEffect(.regular.interactive(), in: .capsule)
                                .accessibilityIdentifier("noticesShowLess")
                            }
                        } else {
                            stacked(item(first, now: now, stacked: true), behind: items.count - 1)
                        }
                    }
                    // nothing below: the toolbar's own margin is the gap above its buttons
                    .padding(.horizontal, 12).padding(.top, 6)
                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
                }
                .scrollBounceBehavior(.basedOnSize)
                // no shade of its own at its edges: the page shows through as it is
                .scrollEdgeEffectHidden(true, for: .all)
                // The glass's shadow falls past the band unless its notices
                // overflow it; overflowing, they never scroll under the toolbar,
                // however it was scrolled when a notice went.
                .scrollClipDisabled(height <= maxHeight)
                .frame(height: min(height, maxHeight))
                .clipShape(Rectangle().inset(by: height > maxHeight ? 0 : -32))
                // one left: nothing to fan
                .onChange(of: items.count) { _, count in if count <= 1 { fanned = false } }
            }
        }
        .sensoryFeedback(.impact(weight: .light), trigger: fanned)
        .sensoryFeedback(.selection, trigger: opened)
    }

    private func fan(_ out: Bool) {
        withAnimation(reduceMotion ? nil : .snappy(duration: 0.3)) { fanned = out }
    }
    private func toggle(_ id: String) {
        withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) {
            if opened.contains(id) { opened.remove(id) } else { opened.insert(id) }
        }
    }

    /// The first, with up to two glass edges of the rest beneath it; a tap
    /// anywhere but its button fans them out.
    private func stacked(_ front: some View, behind: Int) -> some View {
        let edges = min(behind, 2)
        return front
            .background(alignment: .bottom) {
                ZStack(alignment: .bottom) {
                    ForEach((1...edges).reversed(), id: \.self) { depth in
                        Color.clear.frame(height: 36)
                            .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                            .shadow(color: .black.opacity(0.08), radius: 3, y: 1)
                            .padding(.horizontal, CGFloat(depth) * 12)
                            .offset(y: CGFloat(depth) * 8)
                    }
                }
            }
            .padding(.bottom, CGFloat(edges) * 8)
            .contentShape(Rectangle())
            .onTapGesture { fan(true) }
            .accessibilityAction(named: "Show all \(behind + 1)") { fan(true) }
            .accessibilityIdentifier("noticesStack")
    }

    @ViewBuilder private func item(_ item: Item, now: Int64, stacked: Bool) -> some View {
        switch item {
        case .refresh: refresh
        case .notice(let notice): row(notice, now: now, stacked: stacked)
        case .refusal(let id, let failure): refused(id, failure)
        }
    }

    /// A refusal whose notice is gone: its title and the server's words,
    /// until OK (which asks nothing).
    private func refused(_ id: String, _ failure: NoticeFailure) -> some View {
        card {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 10) {
                    Image(systemName: "exclamationmark.circle.fill").font(.system(size: 18)).foregroundStyle(.red)
                    Text(failure.title).font(.subheadline.weight(.semibold)).lineLimit(2)
                    Spacer(minLength: 4)
                    Button { library.failedNotices[id] = nil } label: { Text("OK").foregroundStyle(Color(.systemBackground)) }
                        .buttonStyle(.borderedProminent).buttonBorderShape(.capsule).controlSize(.small)
                }
                Text(failure.said).font(.footnote).foregroundStyle(.secondary).padding(.leading, 28)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("refused:\(id)")
    }

    private var refresh: some View {
        card { NoticesRefresh(library: library, api: api) }
            .accessibilityIdentifier("noticesRefresh")
    }

    /// In a stack, a tap fans the stack out rather than opening it.
    private func row(_ notice: Notice, now: Int64, stacked: Bool) -> some View {
        let failure = library.failedNotices[notice.id]
        // why its last answer failed is never hidden
        let open = opened.contains(notice.id) || failure != nil
        let more = notice.text != nil || notice.until != nil || notice.detail != nil || notice.actions.count > 1
        return card {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 10) {
                    Image(systemName: notice.warns ? "exclamationmark.circle.fill" : "info.circle.fill")
                        .font(.system(size: 18)).foregroundStyle(notice.warns ? Color.red : Color.secondary)
                    Text(notice.title).font(.subheadline.weight(.semibold)).lineLimit(open ? nil : 2)
                    if more {
                        Image(systemName: "chevron.down").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                            .rotationEffect(.degrees(open ? 180 : 0))
                    }
                    Spacer(minLength: 4)
                    if let main = notice.actions.first { button(notice, main, main: true) }
                }
                if open {
                    // every word it says, however little room the page leaves (the keyboard up)
                    VStack(alignment: .leading, spacing: 2) {
                        if let text = notice.text { Text(text).font(.footnote).foregroundStyle(.secondary) }
                        if let until = notice.until { Text(Notice.untilSaid(until, now: now)).font(.footnote).foregroundStyle(.secondary) }
                        if let failure { Text(failure.said).font(.footnote).foregroundStyle(.secondary) }
                        if let detail = notice.detail { Text(detail).font(.caption2).foregroundStyle(.tertiary).padding(.top, 2) }
                    }
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 28)
                    if notice.actions.count > 1 {
                        HStack(spacing: 8) {
                            ForEach(Array(notice.actions.dropFirst().enumerated()), id: \.offset) { _, action in
                                button(notice, action, main: false)
                            }
                        }.padding(.leading, 28)
                    }
                }
            }
            .contentShape(Rectangle())
            .onTapGesture { if stacked { fan(true) } else if more { toggle(notice.id) } }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("notice:\(notice.id)")
    }

    @ViewBuilder private func button(_ notice: Notice, _ action: Notice.Action, main: Bool) -> some View {
        let pressed = library.acting.contains(notice.id)
        let tap = {
            if let open = action.open, let url = URL(string: open) { openURL(url) }
            else { Task { if let outside = await library.act(notice, action, api: api) { openURL(outside) } } }
        }
        if main {
            // its words in the page's own colour, on the app's primary tint (dark: white on white otherwise)
            // on one line: the title gives way first
            Button(action: tap) { Text(action.label).foregroundStyle(Color(.systemBackground)).lineLimit(1).fixedSize() }
                .buttonStyle(.borderedProminent).buttonBorderShape(.capsule).controlSize(.small).disabled(pressed)
        } else {
            Button(action: tap) { Text(action.label).lineLimit(1).fixedSize() }.buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.small).disabled(pressed)
        }
    }

    private func card(@ViewBuilder _ content: () -> some View) -> some View {
        content()
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            // an iOS banner's glass and corners
            .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
}

/// The line a failed read of the notices leaves (`Library.noticesFailed`):
/// what the last read said stays, and Try again reads again, posting no
/// answer. The band's, and the cover's too, as the band can't be reached
/// under it.
struct NoticesRefresh: View {
    let library: Library
    let api: FragmentAPI

    var body: some View {
        HStack {
            Text("Couldn't refresh your computer").font(.subheadline.weight(.semibold))
            Spacer()
            Button { Task { await library.load(api) } } label: { Text("Try again").foregroundStyle(Color(.systemBackground)) }
                .buttonStyle(.borderedProminent).buttonBorderShape(.capsule).controlSize(.small)
        }
    }
}

/// A notice that must be answered (`mustAnswer`), alone over the whole app
/// (`FragmentRoot`'s full-screen cover), as the web's (cell/shell/
/// computer.js `drawAsked`): its words, why its last answer failed, and
/// its answers, the first the main one, until the read after one of them
/// has it no more (`Library.asked`). It has no other way out. A read that
/// failed says so over it, with Try again (`NoticesRefresh`).
struct MustAnswerView: View {
    let library: Library
    let api: FragmentAPI
    let notice: Notice
    @Environment(\.openURL) private var openURL

    var body: some View {
        GeometryReader { page in
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    if library.noticesFailed { NoticesRefresh(library: library, api: api).accessibilityIdentifier("mustAnswerRefresh") }
                    VStack(alignment: .leading, spacing: 8) {
                        Text(notice.title).font(.title2.weight(.semibold))
                        if let text = notice.text { Text(text).foregroundStyle(.secondary) }
                        if let why = library.failedNotices[notice.id] { Text(why.said).foregroundStyle(.secondary) }
                        if let detail = notice.detail { Text(detail).font(.caption).foregroundStyle(.tertiary) }
                    }
                    .fixedSize(horizontal: false, vertical: true)
                    VStack(spacing: 10) {
                        ForEach(Array(notice.actions.enumerated()), id: \.offset) { index, action in
                            answer(action, main: index == 0)
                        }
                    }
                }
                .frame(maxWidth: 440, alignment: .leading)
                .padding(24)
                .frame(maxWidth: .infinity, minHeight: page.size.height)
            }
            .scrollBounceBehavior(.basedOnSize)
        }
        .background(FragmentStyle.background.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mustAnswer:\(notice.id)")
    }

    @ViewBuilder private func answer(_ action: Notice.Action, main: Bool) -> some View {
        let pressed = library.acting.contains(notice.id)
        let tap = {
            if let open = action.open, let url = URL(string: open) { openURL(url) }
            else { Task { if let outside = await library.act(notice, action, api: api) { openURL(outside) } } }
        }
        if main {
            Button(action: tap) { Text(action.label).foregroundStyle(Color(.systemBackground)).frame(maxWidth: .infinity) }
                .buttonStyle(.borderedProminent).buttonBorderShape(.capsule).controlSize(.large).disabled(pressed)
        } else {
            Button(action: tap) { Text(action.label).frame(maxWidth: .infinity) }
                .buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.large).disabled(pressed)
        }
    }
}
