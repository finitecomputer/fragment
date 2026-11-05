import Foundation
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
}

private final class Heard: Sendable { let keys = Mutex<[String]>([]) }
