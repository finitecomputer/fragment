import SwiftUI

// A small native renderer for the chat slice: paragraphs, headings, lists,
// quotes and fenced code. Inline formatting and links are Apple's Markdown.
// Blocks have positional identities so a growing final block stays in place.
struct MarkdownBlock: Identifiable, Equatable {
    enum Kind: Equatable { case paragraph, heading, code(String), quote }
    let id: Int
    let kind: Kind
    let text: String

    static func parse(_ source: String) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        var lines: [String] = []
        var code: String?
        func append(_ kind: Kind) {
            guard !lines.isEmpty else { return }
            blocks.append(MarkdownBlock(id: blocks.count, kind: kind, text: lines.joined(separator: "\n")))
            lines = []
        }
        for line in source.components(separatedBy: "\n") {
            if line.hasPrefix("```") {
                if let language = code { append(.code(language)); code = nil }
                else { append(.paragraph); code = String(line.dropFirst(3)) }
            } else if code != nil { lines.append(line) }
            else if line.trimmingCharacters(in: .whitespaces).isEmpty { append(.paragraph) }
            else if line.hasPrefix("#") {
                let heading = line.drop(while: { $0 == "#" })
                if heading.first == " " {
                    append(.paragraph)
                    lines = [String(heading.dropFirst())]
                    append(.heading)
                } else { lines.append(line) }
            } else if line.hasPrefix("> ") {
                append(.paragraph)
                lines = [String(line.dropFirst(2))]
                append(.quote)
            } else {
                lines.append(line.hasPrefix("- ") || line.hasPrefix("* ") ? "• " + line.dropFirst(2) : line)
            }
        }
        append(code.map(Kind.code) ?? .paragraph)
        return blocks
    }
}

struct MarkdownText: View {
    let text: String
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(MarkdownBlock.parse(text)) { block in
                switch block.kind {
                case .code(let language):
                    VStack(alignment: .leading, spacing: 10) {
                        HStack {
                            Text(language.isEmpty ? "Code" : language).font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Button("Copy code", systemImage: "doc.on.doc") { UIPasteboard.general.string = block.text }
                                .labelStyle(.iconOnly).font(.caption).frame(minWidth: 44, minHeight: 32)
                        }
                        ScrollView(.horizontal) { Text(block.text).font(.callout.monospaced()).textSelection(.enabled) }
                    }
                    .padding(14).background(FragmentStyle.surface, in: RoundedRectangle(cornerRadius: 14))
                case .heading:
                    inline(block.text).font(.title3.weight(.semibold))
                case .quote:
                    HStack(alignment: .top, spacing: 12) {
                        RoundedRectangle(cornerRadius: 2).fill(.secondary.opacity(0.3)).frame(width: 3)
                        inline(block.text).foregroundStyle(.secondary)
                    }.fixedSize(horizontal: false, vertical: true)
                case .paragraph:
                    inline(block.text).font(.body).lineSpacing(5)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
    private func inline(_ source: String) -> some View {
        Text(MessageLinks.linked((try? AttributedString(markdown: source, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(source)))
            .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
    }
}

/// The web addresses in a message's words, as the web's renderer links
/// them (cell/markdown.mjs): a Markdown link's target, an `<autolink>`, or
/// a bare http(s) URL, which ends before trailing punctuation. A tap opens
/// one (`ChatView`'s openURL: a fragment's page in the app, others in
/// Safari); a long press on its message copies or shares it.
enum MessageLinks {
    /// cell/markdown.mjs's bare URL, `https?://[^\s<>]*[^\s<>.,;:!?'")\]*_~]`.
    private static let bare = try! NSRegularExpression(pattern: #"https?://[^\s<>]*[^\s<>.,;:!?'")\]*_~]"#)
    private nonisolated(unsafe) static let nameAt = try! Regex("[a-z0-9](?:-?[a-z0-9])*--[a-km-np-z2-9]{4}")
    /// A message's links at most: past them, it names none more.
    static let max = 8

    /// The links a message shows, the very URLs its words link (a tap opens
    /// the same one a copy or share takes): a reply's Markdown parsed as
    /// `MarkdownText` draws it, code blocks aside; the person's own words
    /// as typed, bare URLs alone.
    static func find(in text: String, markdown: Bool) -> [URL] {
        let shown: [AttributedString] = markdown
            ? MarkdownBlock.parse(text).compactMap { block in
                if case .code = block.kind { return nil }
                return linked((try? AttributedString(markdown: block.text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(block.text))
            }
            : [linked(AttributedString(text))]
        var found: [URL] = []
        for part in shown {
            for run in part.runs {
                guard let url = run.link, !found.contains(url) else { continue }
                found.append(url)
                if found.count == max { return found }
            }
        }
        return found
    }

    /// `text` with its bare URLs linked too, every link underlined: links
    /// stand apart from words in the app's tint, which is the text's own.
    static func linked(_ text: AttributedString) -> AttributedString {
        var text = text
        let plain = String(text.characters)
        for match in bare.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)) {
            guard let range = Range(match.range, in: plain), let url = URL(string: String(plain[range])) else { continue }
            let start = text.characters.index(text.startIndex, offsetBy: plain.distance(from: plain.startIndex, to: range.lowerBound))
            let end = text.characters.index(start, offsetBy: plain.distance(from: range.lowerBound, to: range.upperBound))
            if text[start..<end].runs.allSatisfy({ $0.link == nil }) { text[start..<end].link = url }
        }
        for run in text.runs where run.link != nil { text[run.range].underlineStyle = .single }
        return text
    }

    /// A page of a fragment of this deployment, as the web's `fragmentPage`
    /// (templates/chat/site/chat.js) tells one: its host a name (`NAME_AT`)
    /// then the deployment's own domain for fragments, on its scheme and
    /// port (`pages`); its name. Any other address is none, however alike.
    static func fragmentName(_ url: URL, pages: FragmentPages?) -> String? {
        guard let pages, url.scheme?.lowercased() == pages.scheme, url.user == nil, url.password == nil,
              url.port == pages.port, let host = url.host?.lowercased(), let dot = host.firstIndex(of: ".") else { return nil }
        let label = String(host[..<dot])
        return host[host.index(after: dot)...] == pages.domain && label.wholeMatch(of: nameAt) != nil ? label : nil
    }
}

/// Where this deployment serves fragments' pages: `<name>.<domain>` on a
/// scheme and port, as a fragment's own canonical URL says it
/// (`GET /api/f/{name}/status`, `urls.canonical`).
struct FragmentPages: Equatable, Sendable {
    let scheme: String
    let domain: String
    let port: Int?
    /// From a fragment's canonical URL, which must be its name's.
    init?(canonical: URL, name: String) {
        guard let scheme = canonical.scheme?.lowercased(), ["http", "https"].contains(scheme),
              let host = canonical.host?.lowercased(), host.hasPrefix(name + "."), host.count > name.count + 1 else { return nil }
        self.scheme = scheme
        self.domain = String(host.dropFirst(name.count + 1))
        self.port = canonical.port
    }
}
