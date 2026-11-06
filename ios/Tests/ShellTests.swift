import Foundation
import Observation
import UIKit
import ImageIO
import UniformTypeIdentifiers
import Synchronization
import Testing
@testable import Fragment

/// The shell's reads and writes against a stub of the platform's answers
/// (docs/api.md): what the app asks, and what it makes of the answers.
@MainActor
struct ShellTests {
    /// A person's list: two agents, a chat with both, an app, and their
    /// session archive.
    nonisolated private static let list = #"""
    {"fragments":[
      {"name":"moss--a1","kind":"agent","title":"Moss","role":"owner"},
      {"name":"juniper--a2","kind":"agent","title":"Juniper","role":"owner"},
      {"name":"talk--c1","kind":"chat","title":"A calmer morning","agents":["npub1moss","npub1juniper"],"previewAt":5,"role":"owner"},
      {"name":"reading--p1","kind":"app","title":"Reading","picture":"/api/f/reading--p1/icon?v=7","role":"owner","dock":true},
      {"name":"archive--s1","kind":"sessions","role":"owner"}
    ]}
    """#
    nonisolated private static func platform(_ seen: Seen, _ before: [Seen]) -> (Int, String) {
        switch seen.route {
        case "GET /api/fragments": (200, list)
        case "GET /api/f/moss--a1/status": (200, #"{"npub":"npub1moss","urls":{"canonical":"https://moss--a1.finite.test/"}}"#)
        case "GET /api/f/juniper--a2/status": (200, #"{"npub":"npub1juniper","urls":{"canonical":"https://juniper--a2.finite.test/"}}"#)
        case "GET /api/f/archive--s1/file": (200, #"{"sessions":[{"key":"imported-chat","title":"Before Finite","ended_at":"2026-10-01T10:00:00Z"}]}"#)
        default: (404, #"{"error":"not_found","message":"no route \#(seen.route)"}"#)
        }
    }

    @Test func theListResolvesAgentsChatsAppsAndTheArchive() async throws {
        let stub = Stub(Self.platform)
        let api = try stub.api()
        let library = Library()
        await library.load(api)
        #expect(library.error == nil)
        #expect(library.agents.map(\.name) == ["moss--a1", "juniper--a2"])
        let chat = try #require(library.chats.first)
        #expect(library.agents(in: chat).map(\.name) == ["moss--a1", "juniper--a2"])
        #expect(library.agent(for: "npub1juniper")?.displayTitle == "Juniper")
        // a listed picture is the platform's, signed like every other read
        #expect(library.apps.first?.picture == "https://\(stub.host)/api/f/reading--p1/icon?v=7")
        #expect(library.imported.map(\.key) == ["imported-chat"] && library.imported[0].at > 0)
        #expect(stub.seen.first { $0.path == "/api/f/archive--s1/file" }?.query == ["path": "index/chat.json"])
    }

    /// The archive's index is optional: one of other kinds alone (cron,
    /// subagents) has none, and one that fails to read stops nothing.
    @Test func anArchiveWithoutChatsOrFailingNeverStopsAgents() async throws {
        let failing = Stub { seen, before in
            guard seen.route == "GET /api/f/archive--s1/file" else { return Self.platform(seen, before) }
            // the first read finds no chats index; the next fails for a while
            return before.contains { $0.route == seen.route } ? (503, #"{"error":"unavailable","message":"Try again."}"#) : (404, #"{"error":"not_found","message":"no file index/chat.json at main"}"#)
        }
        let api = try failing.api()
        let library = Library()
        await library.load(api)
        #expect(library.error == nil && library.imported.isEmpty)
        #expect(library.agent(for: "npub1juniper")?.name == "juniper--a2")
        await library.load(api)
        #expect(library.error == nil && library.imported.isEmpty)
        #expect(library.agents(in: try #require(library.chats.first)).map(\.name) == ["moss--a1", "juniper--a2"])

        // an index read once is kept through a failed read
        let flaky = Stub { seen, before in
            seen.route == "GET /api/f/archive--s1/file" && before.contains { $0.route == seen.route } ? (503, #"{"message":"Try again."}"#) : Self.platform(seen, before)
        }
        let kept = Library()
        let flakyAPI = try flaky.api()
        await kept.load(flakyAPI)
        await kept.load(flakyAPI)
        #expect(kept.imported.map(\.key) == ["imported-chat"] && kept.error == nil)
    }

    @Test func anExportIsBuiltThenDownloadedSignedToAFile() async throws {
        let id = "0123456789abcdef"
        let stub = Stub { seen, before in
            switch seen.route {
            case "POST /api/export": (200, #"{"id":"\#(id)","state":"building","startedAt":1,"progress":{"done":0,"total":3,"bytes":0}}"#)
            case "GET /api/export":
                before.contains { $0.route == "GET /api/export" }
                    ? (200, #"{"export":{"id":"\#(id)","state":"ready","startedAt":1,"progress":{"done":3,"total":3,"bytes":11},"size":11,"readyAt":2,"expiresAt":3,"download":"/api/export/\#(id)/download"}}"#)
                    : (200, #"{"export":null}"#)
            case "GET /api/export/\(id)/download": (200, "tar-archive")
            default: (404, "{}")
            }
        }
        let api = try stub.api()
        #expect(try await api.export() == nil)
        let building = try await api.startExport()
        #expect(building.state == "building" && building.said == "Building (0 of 3)" && building.download == nil)
        await #expect(throws: APIError.self) { _ = try await api.download(building) }
        let ready = try #require(try await api.export())
        #expect(ready.state == "ready" && ready.download == "/api/export/\(id)/download")
        let file = try await api.download(ready)
        defer { try? FileManager.default.removeItem(at: file) }
        #expect(try String(contentsOf: file, encoding: .utf8) == "tar-archive" && file.lastPathComponent == "fragment-export-\(id).tar")
        let asked = try #require(stub.seen.last)
        #expect(asked.route == "GET /api/export/\(id)/download" && asked.authorization?.hasPrefix("Nostr ") == true)
        // a download path that is not this export's is never asked
        var elsewhere = ready
        elsewhere.download = "https://elsewhere.example/api/export/\(id)/download"
        await #expect(throws: APIError.self) { _ = try await api.download(elsewhere) }
    }

    /// Settings follows a building export to ready on the same screen, then
    /// downloads it; a new export stays at hand beside it and, made, drops
    /// the file of the one it replaced (cell/shell/exports.js).
    @Test func anExportIsFollowedToReadyAndCanBeMadeAgain() async throws {
        let id = "0123456789abcdef"
        let building = #"{"id":"\#(id)","state":"building","startedAt":1,"progress":{"done":1,"total":3,"bytes":5}}"#
        let ready = #"{"id":"\#(id)","state":"ready","startedAt":1,"progress":{"done":3,"total":3,"bytes":11},"size":11,"readyAt":2,"expiresAt":3,"download":"/api/export/\#(id)/download"}"#
        let stub = Stub { seen, before in
            switch seen.route {
            case "POST /api/export": return (200, building)
            // ready on its third look, after the first export was asked for
            case "GET /api/export": return (200, #"{"export":\#(before.filter { $0.route == seen.route }.count >= 2 ? ready : building)}"#)
            case "GET /api/export/\(id)/download": return (200, "tar-archive")
            default: return (404, "{}")
            }
        }
        let model = ExportModel(api: try stub.api(), every: .milliseconds(10))
        await model.start()
        #expect(model.export?.state == "building" && model.startLabel == "Exporting…")
        await model.follow()
        #expect(model.export?.state == "ready" && model.startLabel == "Export again" && model.error == nil)
        await model.download()
        let file = try #require(model.file)
        defer { try? FileManager.default.removeItem(at: file) }
        #expect(try String(contentsOf: file, encoding: .utf8) == "tar-archive")
        await model.start()
        #expect(model.export?.state == "building" && model.file == nil)
        #expect(stub.seen.filter { $0.route == "POST /api/export" }.count == 2)
    }

    /// Asks and makes run one after another: an older read held in flight
    /// never lands after the new export made behind it, and nothing is sent
    /// before it answers.
    @Test func anOlderExportReadNeverLandsAfterANewerExport() async throws {
        let gate = Gate()
        let building = #"{"id":"0123456789abcdef","state":"building","startedAt":1,"progress":{"done":0,"total":3,"bytes":0}}"#
        let stub = Stub { seen, before in
            switch seen.route {
            case "GET /api/export":
                gate.pass()
                return (200, #"{"export":null}"#)
            // a make sent while the older read is still held fails the test
            case "POST /api/export": return gate.isOpen ? (200, building) : (500, #"{"message":"sent before the read answered"}"#)
            default: return (404, "{}")
            }
        }
        let model = ExportModel(api: try stub.api())
        let read = Task { await model.refresh() }
        try await waitUntil("the read held") { stub.seen.contains { $0.route == "GET /api/export" } }
        let make = Task { await model.start() }
        await Task.yield()
        #expect(model.busy && !stub.seen.contains { $0.route == "POST /api/export" })
        gate.open()
        await read.value
        await make.value
        #expect(model.export?.state == "building" && model.error == nil && !model.busy)
        #expect(stub.seen.map(\.route) == ["GET /api/export", "POST /api/export"])
    }

    /// A downloaded archive goes with its export: refreshed to another,
    /// sharing needs that one downloaded, and a download still in flight
    /// for one no longer shown is never kept.
    @Test func aDownloadedArchiveGoesWithItsExport() async throws {
        let gate = Gate()
        let view: @Sendable (String) -> String = { id in #"{"id":"\#(id)","state":"ready","startedAt":1,"progress":{"done":1,"total":1,"bytes":1},"size":1,"download":"/api/export/\#(id)/download"}"# }
        let (a, b) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb")
        let stub = Stub { seen, before in
            let reads = before.filter { $0.route == "GET /api/export" }.count
            switch seen.route {
            // A, then A again, then B (made on the web)
            case "GET /api/export": return (200, #"{"export":\#(view(reads < 2 ? a : b))}"#)
            case "GET /api/export/\(a)/download":
                // its second download is held until B is shown
                if before.contains(where: { $0.route == seen.route }) { gate.pass() }
                return (200, "archive A")
            case "GET /api/export/\(b)/download": return (200, "archive B")
            default: return (404, "{}")
            }
        }
        let model = ExportModel(api: try stub.api())
        await model.refresh()
        await model.download()
        let first = try #require(model.file)
        #expect(try String(contentsOf: first, encoding: .utf8) == "archive A")

        // A's download again, held; meanwhile the export shown becomes B
        await model.refresh()
        let held = Task { await model.download() }
        try await waitUntil("A's download held") { stub.seen.filter { $0.route == "GET /api/export/\(a)/download" }.count == 2 }
        await model.refresh()
        #expect(model.export?.id == b && model.file == nil && !FileManager.default.fileExists(atPath: first.path))
        gate.open()
        await held.value
        #expect(model.file == nil, "a download for an export no longer shown is never kept")
        await model.download()
        let second = try #require(model.file)
        defer { try? FileManager.default.removeItem(at: second) }
        #expect(try String(contentsOf: second, encoding: .utf8) == "archive B")
    }

    @Test func aComputerSaysItsPhase() throws {
        let listed = try JSONDecoder().decode(JSONValue.self, from: Data(#"{"computers":[{"computer":"c1","owner":"npub1me","image":"stub","phase":"wont_wake","generation":3,"agents":[]}],"support":null}"#.utf8))
        #expect(ComputerPhase.said(listed["computers"].array.first?["phase"].string) == "Won’t wake")
        #expect(ComputerPhase.said("awake") == "Awake" && ComputerPhase.said(nil) == "—")
    }

    /// A web sheet's spent redemption is never opened again: each load
    /// drops it before asking for a new one, and a failed ask leaves none.
    @Test func aWebSheetRetryNeverReusesASpentRedemption() async throws {
        let stub = Stub { seen, before in
            switch seen.route {
            case "GET /api/f/reading--p1/status": return (200, #"{"npub":"npub1app","urls":{"canonical":"http://reading--p1.fragment.localhost:8790/"}}"#)
            case "POST /api/f/reading--p1/session":
                let n = before.filter { $0.route == seen.route }.count
                if n == 1 { return (503, #"{"message":"Try again shortly."}"#) }
                return (200, #"{"consentRequired":false,"url":"http://reading--p1.fragment.localhost:8790/__signin?token=t\#(n)"}"#)
            default: return (404, "{}")
            }
        }
        let model = WebSheetModel(fragment: FragmentSummary(name: "reading--p1", kind: "app"), api: try stub.api())
        await model.load()
        #expect(model.url?.query == "token=t0" && model.error == nil)
        // its page failed after the redemption was spent; Try again
        model.error = "The app could not be opened (HTTP 502)."
        await model.load()
        #expect(model.url == nil && model.error == "Try again shortly.")
        await model.load()
        #expect(model.url?.query == "token=t2" && model.error == nil)
    }

    /// A message's links are the URLs its words link, as drawn: Markdown
    /// targets whole (parentheses, a final `!`), `<autolinks>`, and bare
    /// URLs, which end before trailing punctuation.
    @Test func aMessagesLinksAreTheOnesItsWordsLink() {
        let text = "Same link as before: https://sports-schedule--ugzb.finite.place/?view=fe1f4b5067d2339a83ba0f62. See [docs](https://example.com/a) or <https://example.com/b>, and https://example.com/a again."
        #expect(MessageLinks.find(in: text, markdown: true).map(\.absoluteString) == [
            "https://sports-schedule--ugzb.finite.place/?view=fe1f4b5067d2339a83ba0f62", "https://example.com/a", "https://example.com/b"])
        #expect(MessageLinks.find(in: "[Wiki](https://example.com/wiki/Function_(mathematics)) and [Page](https://example.com/report!)", markdown: true).map(\.absoluteString)
                == ["https://example.com/wiki/Function_(mathematics)", "https://example.com/report!"])
        #expect(MessageLinks.find(in: "```\nhttps://example.com/in-code\n```", markdown: true).isEmpty, "code is not a link")
        #expect(MessageLinks.find(in: "mine: https://example.com/x.", markdown: false).map(\.absoluteString) == ["https://example.com/x"])
        // a bare one is linked in the words shown, and every link underlined
        let shown = MessageLinks.linked(AttributedString("go to https://example.com/x."))
        let linked = shown.runs.filter { $0.link != nil }
        #expect(linked.count == 1 && linked.first?.link?.absoluteString == "https://example.com/x" && linked.first?.underlineStyle == .single)
    }

    /// A page of this deployment's fragments is told as the web's
    /// `fragmentPage` tells one: a name, then its own domain, scheme and port.
    @Test func aFragmentsPageIsTheDeploymentsAlone() throws {
        let pages = try #require(FragmentPages(canonical: URL(string: "https://sports-schedule--ugzb.finite.place/")!, name: "sports-schedule--ugzb"))
        func name(_ link: String) -> String? { MessageLinks.fragmentName(URL(string: link)!, pages: pages) }
        #expect(name("https://sports-schedule--ugzb.finite.place/week?view=1") == "sports-schedule--ugzb")
        #expect(name("https://SPORTS-schedule--ugzb.Finite.Place/") == "sports-schedule--ugzb")
        #expect(name("https://sports-schedule--ugzb.example.com/week") == nil, "a lookalike domain")
        #expect(name("https://sports-schedule--ugzb.finite.place:444/week") == nil, "another port")
        #expect(name("http://sports-schedule--ugzb.finite.place/") == nil, "another scheme")
        #expect(name("https://sports-schedule--ugzb.preview.finite.place/") == nil, "another deployment's domain")
        #expect(name("https://a--b1.finite.place/") == nil, "a suffix is four of its letters")
        #expect(MessageLinks.fragmentName(URL(string: "https://sports-schedule--ugzb.finite.place/")!, pages: nil) == nil, "unknown until read")
        #expect(FragmentPages(canonical: URL(string: "https://other--abcd.finite.place/")!, name: "sports-schedule--ugzb") == nil)
        let local = try #require(FragmentPages(canonical: URL(string: "http://todo--rcmf.fragment.localhost:8790/")!, name: "todo--rcmf"))
        #expect(MessageLinks.fragmentName(URL(string: "http://sports--abcd.fragment.localhost:8790/x")!, pages: local) == "sports--abcd")
        #expect(MessageLinks.fragmentName(URL(string: "http://sports--abcd.fragment.localhost/x")!, pages: local) == nil)
    }

    /// A link to a fragment of the person's opens it in the app, an app at
    /// the linked page, path, query and hash; another's, a lookalike, or any
    /// other address is Safari's.
    @Test func aLinkToTheirFragmentOpensItAtItsPage() async throws {
        let list = #"{"fragments":[{"name":"sports-schedule--ugzb","kind":"app","title":"Sports schedule","role":"owner"},{"name":"trip-chat--k3x9","kind":"chat","title":"Trip","role":"owner"}]}"#
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/fragments": (200, list)
            case "GET /api/f/sports-schedule--ugzb/status": (200, #"{"npub":"npub1app","urls":{"canonical":"https://sports-schedule--ugzb.finite.place/"}}"#)
            case "POST /api/f/sports-schedule--ugzb/session": (200, #"{"consentRequired":false,"url":"https://sports-schedule--ugzb.finite.place/__signin?token=t"}"#)
            default: (404, "{}")
            }
        }
        let api = try stub.api()
        let library = Library()
        await library.load(api)
        let link = URL(string: "https://sports-schedule--ugzb.finite.place/week?view=fe1f#today")!
        let app = try #require(library.fragment(linkedBy: link))
        #expect(app.name == "sports-schedule--ugzb" && app.link == link)
        #expect(library.fragment(linkedBy: URL(string: "https://trip-chat--k3x9.finite.place/")!)?.link == nil, "a chat opens as a chat")
        #expect(library.fragment(linkedBy: URL(string: "https://someone-else--abcd.finite.place/")!) == nil)
        #expect(library.fragment(linkedBy: URL(string: "https://sports-schedule--ugzb.example.com/week")!) == nil, "a lookalike opens as typed")
        let model = WebSheetModel(fragment: app, api: api)
        await model.load()
        #expect(model.error == nil && model.url?.path == "/__signin")
        let asked = try #require(stub.seen.last { $0.route == "POST /api/f/sports-schedule--ugzb/session" })
        #expect(asked.json["returnTo"].string == "/week?view=fe1f#today", "its page, the link's own, hash and all")
        // a hash-routed page keeps its route
        let routed = try #require(library.fragment(linkedBy: URL(string: "https://sports-schedule--ugzb.finite.place/#/week/2")!))
        await WebSheetModel(fragment: routed, api: api).load()
        #expect(stub.seen.last { $0.route == "POST /api/f/sports-schedule--ugzb/session" }?.json["returnTo"].string == "/#/week/2")
    }

    /// Pictures attach as the web's chat shows them: PNG, JPEG, WebP and GIF
    /// as they are, any other (a photo's HEIC) as JPEG; files by their
    /// extension's type; nothing empty or past 25 MB.
    @Test func aPictureOrFileAttachesAsTheWebTakesIt() throws {
        let picture = UIGraphicsImageRenderer(size: CGSize(width: 4, height: 4)).image { context in
            UIColor.systemTeal.setFill(); context.fill(CGRect(x: 0, y: 0, width: 4, height: 4))
        }
        let png = try #require(picture.pngData())
        let kept = try AttachmentInput.image(png, type: .png, name: "photo-1")
        #expect(kept == OutgoingFile(data: png, name: "photo-1.png", type: "image/png"))
        // a photo's HEIC, turned (orientation 6), becomes a JPEG that keeps its turn
        let heic = NSMutableData()
        let out = try #require(CGImageDestinationCreateWithData(heic, UTType.heic.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(out, try #require(picture.cgImage), [kCGImagePropertyOrientation: 6] as CFDictionary)
        #expect(CGImageDestinationFinalize(out))
        let made = try AttachmentInput.image(heic as Data, type: .heic, name: "photo-2.heic")
        #expect(made.type == "image/jpeg" && made.name == "photo-2.jpg" && UIImage(data: made.data) != nil)
        let source = try #require(CGImageSourceCreateWithData(made.data as CFData, nil))
        #expect(CGImageSourceGetType(source) as String? == UTType.jpeg.identifier)
        let header = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        #expect(header?[kCGImagePropertyOrientation] as? Int == 6, "its metadata goes with it, as the web's upload keeps a file's")
        #expect(throws: APIError.self) { try AttachmentInput.image(Data("not a picture".utf8), type: .heic, name: "x") }
        #expect(throws: APIError.self) { try AttachmentInput.image(Data(), type: .png, name: "empty") }
        #expect(throws: APIError.self) { try AttachmentInput.file(Data(count: ChatAttachment.bytesMax + 1), name: "big.bin", type: nil) }
        #expect(try AttachmentInput.file(Data("a,b".utf8), name: "t.csv", type: UTType(filenameExtension: "csv")).type == "text/csv")
        #expect(try AttachmentInput.file(Data([1]), name: "x.unknownext", type: UTType(filenameExtension: "unknownext")).type == "application/octet-stream")
    }

    /// A paste of pictures attaches each, as `image` makes it; words on the
    /// clipboard are no picture.
    @Test func pastedPicturesAttachAndWordsDoNot() throws {
        let picture = UIGraphicsImageRenderer(size: CGSize(width: 4, height: 4)).image { context in
            UIColor.systemPink.setFill(); context.fill(CGRect(x: 0, y: 0, width: 4, height: 4))
        }
        let png = try #require(picture.pngData()), heic = try #require(picture.heicData())
        let board = UIPasteboard.withUniqueName()
        defer { UIPasteboard.remove(withName: board.name) }
        board.items = [[UTType.png.identifier: png], [UTType.plainText.identifier: "just words"], [UTType.heic.identifier: heic]]
        let files = AttachmentInput.pasted(AttachmentInput.snapshot(board))
        #expect(files.map(\.name) == ["pasted-image-1.png", "pasted-image-2.jpg"])
        // the pasteboard hands a PNG back as it holds it, a picture all the same
        #expect(files.map(\.type) == ["image/png", "image/jpeg"] && files.allSatisfy { UIImage(data: $0.data) != nil })
        board.items = [[UTType.plainText.identifier: "only words"]]
        #expect(AttachmentInput.snapshot(board).isEmpty)
    }

    /// Attaching is one at a time, reserved before anything is read: while
    /// a chosen photo still loads, nothing sends and nothing more attaches;
    /// once it is put, it goes with the message.
    @Test func attachingHoldsTheComposerUntilItsFilesArePut() async throws {
        let stub = Stub { seen, _ in
            seen.method == "PUT" ? (200, #"{"ok":true,"stored":true}"#) : (404, #"{"error":"not_found","message":"no route"}"#)
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: FragmentSummary(name: "talk--c1", kind: "chat"), api: api)
        session.edit("A caption")
        let loading = Gate()
        #expect(session.beginAttaching(1) && session.attaching)
        let attach = Task {
            await session.attach {
                await Task.detached { loading.pass() }.value
                return [OutgoingFile(data: Data("photo".utf8), name: "photo-1.jpg", type: "image/jpeg")]
            }
        }
        // the photo still loading: Send waits, and another paste is refused
        await session.send()
        #expect(session.pending.isEmpty && session.composer == "A caption" && session.error == "Wait for the files being attached.")
        #expect(!session.beginAttaching(1) && session.error == "Wait for the files being attached, then add more.")
        loading.open()
        await attach.value
        #expect(!session.attaching && session.attachments.map(\.name) == ["photo-1.jpg"])
        #expect(stub.seen.filter { $0.method == "PUT" }.count == 1)
        // a reservation past eight a message is refused before anything is read
        #expect(!session.beginAttaching(ChatAttachment.countMax) && !session.attaching)
    }

    @Test func anAgentJoinsAChatAsAnEditorByItsNpub() async throws {
        let stub = Stub { seen, before in
            seen.route == "PUT /api/f/talk--c9/members/npub1moss" ? (200, #"{"principal":"npub1moss","role":"editor"}"#) : Self.platform(seen, before)
        }
        let api = try stub.api()
        try await api.join(FragmentSummary(name: "moss--a1", kind: "agent"), to: FragmentSummary(name: "talk--c9", kind: "chat"))
        #expect(stub.seen.map(\.route) == ["GET /api/f/moss--a1/status", "PUT /api/f/talk--c9/members/npub1moss"])
        #expect(stub.seen[1].json == .object(["role": .string("editor")]))
    }

    @Test func aNewChatIsMadeFromTheChatTemplate() async throws {
        let stub = Stub { seen, _ in seen.route == "POST /api/fragments" ? (200, #"{"name":"mobile-chat--x1","npub":"npub1c"}"#) : (404, "{}") }
        let chat = try await stub.api().createChat()
        #expect(chat.name == "mobile-chat--x1" && chat.kind == "chat")
        #expect(stub.seen[0].json == .object(["label": .string("mobile-chat"), "template": .string("chat"), "title": .string("New chat")]))
    }

    @Test func cachedDraftsAreNeverEvictedAtCapacity() throws {
        let stub = Stub { _, _ in (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let library = Library()
        for index in 0..<32 {
            try library.session(for: FragmentSummary(name: "draft-\(index)", kind: "chat"), api: api).composer = "A draft to preserve"
        }
        #expect(throws: APIError.self) { try library.session(for: FragmentSummary(name: "one-more", kind: "chat"), api: api) }
        #expect(library.sessions.count == 32)
        #expect(library.sessions.values.allSatisfy { $0.composer == "A draft to preserve" })
        let again = try library.session(for: FragmentSummary(name: "draft-3", kind: "chat"), api: api)
        #expect(again === library.sessions["draft-3"])
    }

    @Test func renameKeepsTheManifestAndDeploysIt() async throws {
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/f/talk--c1/manifest": (200, #"{"name":"talk--c1","meta":{"title":"Old","template":"chat"},"channels":{"chat":{}}}"#)
            case "POST /api/f/talk--c1/files": (200, #"{"commit":"abc"}"#)
            case "POST /api/f/talk--c1/deploy": (200, #"{"live":"abc","canonical":"https://talk--c1.finite.test/"}"#)
            default: (404, "{}")
            }
        }
        let api = try stub.api()
        let chat = FragmentSummary(name: "talk--c1", kind: "chat")
        await #expect(throws: APIError.self) { try await api.rename(chat, title: "   ") }
        #expect(stub.seen.isEmpty)
        try await api.rename(chat, title: "  A name to keep ")
        #expect(stub.seen.map(\.route) == ["GET /api/f/talk--c1/manifest", "POST /api/f/talk--c1/files", "POST /api/f/talk--c1/deploy"])
        let file = stub.seen[1].json["files"].array.first
        #expect(file?["path"].string == "fragment.json")
        let manifest = try JSONDecoder().decode(JSONValue.self, from: Data(try #require(file?["text"].string).utf8))
        #expect(manifest["meta"] == .object(["title": .string("A name to keep"), "template": .string("chat")]))
        #expect(manifest["channels"] == .object(["chat": .object([:])]))
    }

    @Test func archivingIsThePersonsOwnView() async throws {
        let stub = Stub { seen, before in
            seen.route == "PUT /api/fragments/talk--c1/archived" ? (200, #"{"ok":true}"#) : Self.platform(seen, before)
        }
        let api = try stub.api()
        let library = Library()
        await library.archive(FragmentSummary(name: "talk--c1", kind: "chat"), api: api)
        #expect(stub.seen.first?.json == .object(["archived": .bool(true)]))
        await library.archive(FragmentSummary(name: "talk--c1", kind: "chat", archived: true), api: api)
        #expect(stub.seen.last { $0.method == "PUT" }?.json == .object(["archived": .bool(false)]))
    }

    @Test func anAttachmentIsPutByItsHashAndARefusalSaysWhy() async throws {
        let stub = Stub { seen, _ in
            seen.body.count > 4 ? (413, #"{"error":"too_large","message":"a blob is at most 4 bytes here"}"#) : (200, #"{"ok":true,"stored":true}"#)
        }
        let api = try stub.api()
        let file = try await api.upload(Data("abc".utf8), name: "a.txt", type: "text/plain", to: "talk--c1")
        #expect(file.sha256 == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" && file.size == 3)
        #expect(stub.seen[0].route == "PUT /api/f/talk--c1/blobs/\(file.sha256)" && stub.seen[0].body == Data("abc".utf8))
        do {
            _ = try await api.upload(Data("too big".utf8), name: "b.txt", type: "text/plain", to: "talk--c1")
            Issue.record("a refused upload is an error")
        } catch APIError.refused(let message) { #expect(message == "a blob is at most 4 bytes here") }
    }

    @Test func anEndedAuthorizationAsksForTheConnectionAgain() async throws {
        let stub = Stub { _, _ in (401, #"{"error":"unauthenticated","message":"the key was revoked"}"#) }
        let api = try stub.api()
        let heard = Heard()
        let observer = NotificationCenter.default.addObserver(forName: .finiteAuthorizationFailed, object: nil, queue: nil) { note in
            if let key = note.userInfo?["key"] as? String { heard.keys.withLock { $0.append(key) } }
        }
        defer { NotificationCenter.default.removeObserver(observer) }
        await #expect(throws: APIError.self) { let _: JSONValue = try await api.request("api/identities/me") }
        #expect(heard.keys.withLock { $0.contains(api.identity.publicKey) })
    }

    /// A chat of the person's still "New chat" with a message is titled
    /// from it, as the web titles one (`titleFromFirst`): once, and never
    /// one renamed meanwhile.
    @Test func aNewChatIsTitledFromItsFirstMessageOnce() async throws {
        let list = #"""
        {"fragments":[
          {"name":"moss--a1","kind":"agent","title":"Moss","role":"owner"},
          {"name":"trip--c1","kind":"chat","title":"New chat","preview":"Plan","agents":["npub1moss"],"role":"owner"},
          {"name":"named--c2","kind":"chat","title":"New chat","preview":"Hi","role":"owner"},
          {"name":"shared--c3","kind":"chat","title":"New chat","preview":"Hi","role":"editor"},
          {"name":"quiet--c4","kind":"chat","title":"New chat","role":"owner"}
        ]}
        """#
        let first = #"{"records":[\#(record("chat", 1, body: #"{"kind":"stop","turn":"t0"}"#)),\#(record("chat", 2, principal: "npub1me", body: #"{"text":"\n  Plan a weekend trip to the coast with the whole family and the dog too\nand more"}"#))]}"#
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/fragments": (200, list)
            case "GET /api/f/moss--a1/status": (200, #"{"npub":"npub1moss","urls":{"canonical":"https://moss--a1.finite.test/"}}"#)
            case "GET /api/f/trip--c1/channels/chat", "GET /api/f/named--c2/channels/chat": (200, first)
            case "GET /api/f/trip--c1/manifest": (200, #"{"template":"chat","meta":{"title":"New chat"}}"#)
            // renamed on the web after the list was read
            case "GET /api/f/named--c2/manifest": (200, #"{"template":"chat","meta":{"title":"Our plans"}}"#)
            case "POST /api/f/trip--c1/files": (200, #"{"commit":"c1"}"#)
            case "POST /api/f/trip--c1/deploy": (200, #"{"deployed":true}"#)
            default: (404, #"{"error":"not_found","message":"no route \#(seen.route)"}"#)
            }
        }
        let api = try stub.api()
        let library = Library()
        await library.load(api)
        await library.load(api)
        let writes = stub.seen.filter { $0.method == "POST" }
        #expect(writes.map(\.path) == ["/api/f/trip--c1/files", "/api/f/trip--c1/deploy"])
        let files = try #require(writes.first?.json)
        #expect(files["message"].string == "its title, from its first message")
        let manifest = try JSONDecoder().decode(JSONValue.self, from: Data((files["files"].array.first?["text"].string ?? "").utf8))
        #expect(manifest["meta"]["title"].string == "Plan a weekend trip to the coast with the whole…" && manifest["template"].string == "chat")
        #expect(stub.seen.filter { $0.path == "/api/f/trip--c1/channels/chat" }.count == 1, "once a session")
        #expect(stub.seen.first { $0.path == "/api/f/trip--c1/channels/chat" }?.query == ["after": "0", "limit": "20"])
        // another's chat, and one with nothing said, are never read for it
        #expect(!stub.seen.contains { $0.path.hasPrefix("/api/f/shared--c3/") || $0.path.hasPrefix("/api/f/quiet--c4/") })
    }

    /// The title written is the one made, exactly: no second trim of its
    /// own (Foundation's takes NEL, which JavaScript's keeps).
    @Test func aMadeTitleIsWrittenExactly() async throws {
        let list = #"{"fragments":[{"name":"nel--c1","kind":"chat","title":"New chat","preview":"x","role":"owner"}]}"#
        let page = #"{"records":[\#(record("chat", 1, principal: "npub1me", body: #"{"text":"x\u0085"}"#))]}"#
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/fragments": (200, list)
            case "GET /api/f/nel--c1/channels/chat": (200, page)
            case "GET /api/f/nel--c1/manifest": (200, #"{"template":"chat","meta":{"title":"New chat"}}"#)
            case "POST /api/f/nel--c1/files": (200, #"{"commit":"c1"}"#)
            case "POST /api/f/nel--c1/deploy": (200, #"{"deployed":true}"#)
            default: (404, #"{"error":"not_found","message":"no route"}"#)
            }
        }
        await Library().load(try stub.api())
        let files = try #require(stub.seen.first { $0.route == "POST /api/f/nel--c1/files" }?.json)
        let manifest = try JSONDecoder().decode(JSONValue.self, from: Data((files["files"].array.first?["text"].string ?? "").utf8))
        #expect(manifest["meta"]["title"].string == "x\u{85}", "as the web writes it")
    }

    /// The web's semantics, code point for code point: lines split on LF
    /// alone (a CRLF's CR is white space), JavaScript's white space trimmed
    /// and collapsed, 48 code points kept.
    @Test func aTitleIsItsFirstLineWithWordsCutAsTheWebCutsIt() {
        #expect(Library.title(from: "\n \n  Two   words  \nnext") == "Two words")
        #expect(Library.title(from: "First\r\nSecond") == "First", "a CRLF ends the line, as the web splits it")
        #expect(Library.title(from: "\r\n\u{3000}\u{A0}Wide\u{2003}\u{3000}space\u{FEFF}\r\nnext") == "Wide space")
        #expect(Library.title(from: "One\u{2028}two") == "One two", "a line separator is white space, not a line: the web splits on LF alone")
        #expect(Library.title(from: "\u{85}x") == "\u{85}x", "NEL is no JavaScript white space")
        #expect(Library.title(from: String(repeating: "é", count: 60)).unicodeScalars.count == 48)
        #expect(Library.firstSaid([]) == nil)
        #expect(Library.title(from: String(repeating: "a", count: 48)) == String(repeating: "a", count: 48))
        #expect(Library.title(from: String(repeating: "a", count: 49)) == String(repeating: "a", count: 47) + "…")
    }

    /// The person's agents in their computer's order, as the web's sidebar
    /// takes them: its first is their main agent, first everywhere here, and
    /// its identities need no agent's status asked.
    @Test func agentsFollowTheirComputersOrder() async throws {
        let stub = Stub { seen, before in
            if seen.route == "GET /api/computers" {
                return (200, #"{"computers":[{"computer":"c1","phase":"awake","agents":[{"fragment":"juniper--a2","identity":"npub1juniper","label":"juniper"},{"fragment":"moss--a1","identity":"npub1moss","label":"moss"}]}],"support":null}"#)
            }
            return Self.platform(seen, before)
        }
        let api = try stub.api()
        let library = Library()
        await library.load(api)
        #expect(library.agents.map(\.name) == ["juniper--a2", "moss--a1"])
        #expect(library.agent?.name == "juniper--a2", "new chats are with the main agent")
        #expect(library.agentsByIdentity["npub1moss"]?.name == "moss--a1" && library.computerPhase == "awake")
        // no agent's status for its identity; one read alone, the deployment's pages (`loadPages`)
        #expect(stub.seen.filter { $0.path.hasSuffix("/status") }.count <= 1)
        // a computer that cannot be read keeps the list's order
        let plain = Library()
        await plain.load(try Stub(Self.platform).api())
        #expect(plain.agents.map(\.name) == ["moss--a1", "juniper--a2"])
    }

    /// The list read again saying the same tells nothing that draws from
    /// it (a chat's rows, its agents' faces) to draw again.
    @Test func aListReadAgainThatSaysTheSameRedrawsNothing() async throws {
        let api = try Stub(Self.platform).api()
        let library = Library()
        await library.load(api)
        let told = Mutex(false)
        withObservationTracking { _ = library.fragments; _ = library.identities; _ = library.agentOrder; _ = library.computerPhase } onChange: { told.withLock { $0 = true } }
        await library.load(api)
        #expect(!told.withLock { $0 })
    }

    /// A picture is read once a session, however many places show it at once
    /// or after: its URL names its version.
    @Test func aPictureIsReadOnceASession() async throws {
        let png = UIGraphicsImageRenderer(size: CGSize(width: 2, height: 2)).pngData { context in
            UIColor.systemTeal.setFill(); context.fill(CGRect(x: 0, y: 0, width: 2, height: 2))
        }
        let stub = Stub { seen, _ in
            seen.route == "GET /api/f/moss--a1/icon" ? (200, "base64:" + png.base64EncodedString()) : (404, "{}")
        }
        let api = try stub.api()
        let url = URL(string: "https://\(stub.host)/api/f/moss--a1/icon?v=abc")!
        async let one = ImageCache.shared.image(url, api: api)
        async let two = ImageCache.shared.image(url, api: api)
        let (a, b) = await (one, two)
        let three = await ImageCache.shared.image(url, api: api)
        #expect(a != nil && a === b && b === three && ImageCache.shared.cached(url) === a)
        #expect(stub.seen.count == 1 && stub.seen[0].query == ["v": "abc"])
        #expect(stub.seen[0].authorization?.hasPrefix("Nostr ") == true, "the platform's picture is read signed")
    }
}

private final class Heard: Sendable { let keys = Mutex<[String]>([]) }
