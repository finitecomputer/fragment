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

/// What the person is told, over every page: each notice as the server
/// said it, the most pressing first, and a line when the last read of them
/// failed (what it said before stays under it, with Try again).
struct NoticesBand: View {
    let library: Library
    let api: FragmentAPI
    /// The most of the page it takes (`FragmentRoot`'s share of its
    /// height): past it, it scrolls, so the page and its toolbar stay.
    let maxHeight: CGFloat
    @State private var height: CGFloat = 0
    @Environment(\.openURL) private var openURL

    var body: some View {
        // a countdown says its minutes live, and a notice whose time passed goes
        TimelineView(.periodic(from: .now, by: 15)) { context in
            let now = Int64(context.date.timeIntervalSince1970 * 1000)
            let shown = library.notices.filter { $0.shown(at: now) && !$0.mustAnswer }
            if library.noticesFailed || !shown.isEmpty {
                ScrollView {
                    VStack(spacing: 8) {
                        if library.noticesFailed { refresh }
                        ForEach(shown) { notice in row(notice, now: now) }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 6)
                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
                }
                .scrollBounceBehavior(.basedOnSize)
                .frame(height: min(height, maxHeight))
                // never under the toolbar, however it was scrolled when a notice went
                .clipped()
            }
        }
    }

    private var refresh: some View {
        card(warns: false) { NoticesRefresh(library: library, api: api) }
            .accessibilityIdentifier("noticesRefresh")
    }

    private func row(_ notice: Notice, now: Int64) -> some View {
        card(warns: notice.warns) {
            VStack(alignment: .leading, spacing: 8) {
                // every word it says, however little room the page leaves (the keyboard up)
                VStack(alignment: .leading, spacing: 2) {
                    Text(notice.title).font(.subheadline.weight(.semibold))
                    if let text = notice.text { Text(text).font(.footnote).foregroundStyle(.secondary) }
                    if let until = notice.until { Text(Notice.untilSaid(until, now: now)).font(.footnote).foregroundStyle(.secondary) }
                    if let why = library.failedNotices[notice.id] { Text(why).font(.footnote).foregroundStyle(.secondary) }
                    if let detail = notice.detail { Text(detail).font(.caption2).foregroundStyle(.tertiary).padding(.top, 2) }
                }
                .fixedSize(horizontal: false, vertical: true)
                if !notice.actions.isEmpty {
                    HStack(spacing: 8) {
                        ForEach(Array(notice.actions.enumerated()), id: \.offset) { index, action in
                            button(notice, action, main: index == 0)
                        }
                    }
                }
            }
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
            Button(action: tap) { Text(action.label).foregroundStyle(Color(.systemBackground)) }
                .buttonStyle(.borderedProminent).buttonBorderShape(.capsule).controlSize(.small).disabled(pressed)
        } else {
            Button(action.label, action: tap).buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.small).disabled(pressed)
        }
    }

    private func card(warns: Bool, @ViewBuilder _ content: () -> some View) -> some View {
        content()
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(FragmentStyle.surface, in: RoundedRectangle(cornerRadius: 14))
            .overlay {
                if warns { RoundedRectangle(cornerRadius: 14).strokeBorder(Color.red.opacity(0.45)) }
            }
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
                        if let why = library.failedNotices[notice.id] { Text(why).foregroundStyle(.secondary) }
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
