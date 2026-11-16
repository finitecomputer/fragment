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
/// One is one line, its title and its main action; a tap opens its words.
/// Several stack as iOS stacks notifications, the first with the edges of
/// the rest behind it, and a tap opens them all, full screen, as
/// Notification Center does (`NoticesScreen`): its X folds them back.
struct NoticesBand: View {
    let library: Library
    let api: FragmentAPI
    /// The most of the page it takes (`FragmentRoot`'s share of its
    /// height): past it, it scrolls, so the page and its toolbar stay.
    let maxHeight: CGFloat
    @State private var height: CGFloat = 0
    /// The one notice whose words are open, alone in the band.
    @State private var opened = false
    /// All of them, full screen.
    @State private var all = false
    @Namespace private var zoom
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        // a countdown says its minutes live, and a notice whose time passed goes
        TimelineView(.periodic(from: .now, by: 15)) { context in
            let now = Int64(context.date.timeIntervalSince1970 * 1000)
            let items = NoticeItem.all(library, now: now)
            if let first = items.first {
                ScrollView {
                    Group {
                        if items.count == 1 {
                            NoticeItemView(item: first, library: library, api: api, now: now, open: opened) {
                                withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) { opened.toggle() }
                            }
                        } else {
                            stacked(NoticeItemView(item: first, library: library, api: api, now: now, open: false) { all = true },
                                    behind: items.count - 1)
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
            }
        }
        .fullScreenCover(isPresented: $all) {
            NoticesScreen(library: library, api: api) { all = false }
                // it grows out of the stack and folds back into it
                .navigationTransition(.zoom(sourceID: "notices", in: zoom))
        }
        .sensoryFeedback(.impact(weight: .light), trigger: all)
        .sensoryFeedback(.selection, trigger: opened)
    }

    /// The first, with up to two glass edges of the rest beneath it; a tap
    /// anywhere but its button opens them all.
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
            .onTapGesture { all = true }
            .matchedTransitionSource(id: "notices", in: zoom)
            .accessibilityAction(named: "Show all \(behind + 1)") { all = true }
            .accessibilityIdentifier("noticesStack")
    }
}

/// Every notice, full screen, as Notification Center shows them: each with
/// all its words and actions, scrolling; the X folds them back into the
/// band. It closes itself once none is left.
private struct NoticesScreen: View {
    let library: Library
    let api: FragmentAPI
    let dismiss: () -> Void

    var body: some View {
        TimelineView(.periodic(from: .now, by: 15)) { context in
            let now = Int64(context.date.timeIntervalSince1970 * 1000)
            let items = NoticeItem.all(library, now: now)
            ScrollView {
                VStack(spacing: 10) {
                    ForEach(items) { item in
                        NoticeItemView(item: item, library: library, api: api, now: now, open: true, full: true) {}
                    }
                }
                .padding(.horizontal, 12).padding(.bottom, 24)
            }
            .safeAreaInset(edge: .top, spacing: 12) {
                HStack {
                    Text("Notices").font(.largeTitle)
                    Spacer()
                    Button(action: dismiss) {
                        Image(systemName: "xmark").font(.system(size: 17, weight: .semibold)).frame(width: 32, height: 32)
                    }
                    .buttonStyle(.glass).buttonBorderShape(.circle)
                    .accessibilityLabel("Close").accessibilityIdentifier("noticesClose")
                }
                .padding(.horizontal, 20).padding(.top, 8)
            }
            .onChange(of: items.isEmpty) { _, empty in if empty { dismiss() } }
        }
        // the wallpaper frosted behind them, as Notification Center is over the Lock Screen
        .presentationBackground { DesktopWallpaper().overlay(Rectangle().fill(.regularMaterial)) }
    }
}

/// One thing the band tells: a failed read, a notice, or the refusal of an
/// answer whose notice is gone.
private enum NoticeItem: Identifiable {
    case refresh, notice(Notice), refusal(String, NoticeFailure)
    var id: String {
        switch self { case .refresh: "refresh"; case .notice(let notice): "notice:" + notice.id; case .refusal(let id, _): "refused:" + id }
    }
    @MainActor static func all(_ library: Library, now: Int64) -> [NoticeItem] {
        (library.noticesFailed ? [.refresh] : [])
            + library.notices.filter { $0.shown(at: now) && !$0.mustAnswer }.map(NoticeItem.notice)
            + library.refusals(at: now).map { .refusal($0.id, $0.failure) }
    }
}

/// An item's card. In the band, a notice is one line with its main action,
/// its words once `open`; `full` (the full screen), all its words and every
/// action under them.
private struct NoticeItemView: View {
    let item: NoticeItem
    let library: Library
    let api: FragmentAPI
    let now: Int64
    let open: Bool
    var full = false
    /// The card's tap (anywhere but a button).
    let tap: () -> Void
    @Environment(\.openURL) private var openURL

    var body: some View {
        switch item {
        case .refresh:
            card { NoticesRefresh(library: library, api: api) }.accessibilityIdentifier("noticesRefresh")
        case .notice(let notice): row(notice)
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
                    Text(failure.title).font(.subheadline.weight(.semibold)).lineLimit(full ? nil : 2)
                    Spacer(minLength: 4)
                    Button { library.failedNotices[id] = nil } label: { Text("OK").foregroundStyle(Color(.systemBackground)) }
                        .buttonStyle(.borderedProminent).buttonBorderShape(.capsule).controlSize(.small)
                }
                Text(failure.said).font(.footnote).foregroundStyle(.secondary).padding(.leading, 28)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .contentShape(Rectangle()).onTapGesture(perform: tap)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("refused:\(id)")
    }

    private func row(_ notice: Notice) -> some View {
        let failure = library.failedNotices[notice.id]
        // why its last answer failed is never hidden
        let open = open || failure != nil
        let more = notice.text != nil || notice.until != nil || notice.detail != nil || notice.actions.count > 1
        return card {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 10) {
                    Image(systemName: notice.warns ? "exclamationmark.circle.fill" : "info.circle.fill")
                        .font(.system(size: 18)).foregroundStyle(notice.warns ? Color.red : Color.secondary)
                    Text(notice.title).font(.subheadline.weight(.semibold)).lineLimit(open ? nil : 2)
                    if more && !full {
                        Image(systemName: "chevron.down").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                            .rotationEffect(.degrees(open ? 180 : 0))
                    }
                    Spacer(minLength: 4)
                    if !full, let main = notice.actions.first { button(notice, main, main: true) }
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
                    // the full screen: every action under its words; the band: the rest
                    let shown = full ? Array(notice.actions) : Array(notice.actions.dropFirst())
                    if !shown.isEmpty {
                        HStack(spacing: 8) {
                            ForEach(Array(shown.enumerated()), id: \.offset) { index, action in
                                button(notice, action, main: full && index == 0)
                            }
                        }.padding(.leading, 28)
                    }
                }
            }
            .contentShape(Rectangle())
            .onTapGesture { if !full && (more || !open) { tap() } }
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
            // its words in the page's own colour, on the app's primary tint (dark: white on white otherwise);
            // on one line: the title gives way first
            Button(action: tap) { Text(action.label).foregroundStyle(Color(.systemBackground)).lineLimit(1).fixedSize() }
                .buttonStyle(.borderedProminent).buttonBorderShape(.capsule).controlSize(.small).disabled(pressed)
        } else {
            Button(action: tap) { Text(action.label).lineLimit(1).fixedSize() }
                .buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.small).disabled(pressed)
        }
    }

    private func card(@ViewBuilder _ content: () -> some View) -> some View {
        content()
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(full ? 14 : 12)
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
