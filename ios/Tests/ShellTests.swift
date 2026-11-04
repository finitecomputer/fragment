import Foundation
import UIKit
import Testing
@testable import Fragment

@MainActor
struct ShellTests {
    private let api = FragmentAPI(baseURL: URL(string: "http://localhost:8910")!)

    @Test func multipleAgentsResolveAndJoinWithoutDuplicates() async throws {
        let library = Library()
        await library.load(api)
        #expect(library.agents.count >= 2)
        let moss = try #require(library.agents.first { $0.name == "moss" })
        let juniper = try #require(library.agents.first { $0.name == "juniper" })
        library.selectedAgent = moss.name
        #expect(library.agent?.name == "moss")
        let chat = try await api.createChat()
        try await api.join(moss, to: chat)
        try await api.join(juniper, to: chat)
        try await api.join(moss, to: chat)
        await library.load(api)
        let loaded = try #require(library.fragments.first { $0.name == chat.name })
        #expect(loaded.agents?.count == 2)
        #expect(library.agents(in: loaded).map(\.name) == ["moss", "juniper"])
        #expect(library.agent(for: "npub1localmoss")?.displayTitle == "Moss")
        let session = try library.session(for: loaded, api: api)
        session.composer = "Keep this draft"
        session.replyTo = ChannelRecord(channel: "chat", seq: 1, at: 1, principal: "someone", kind: "message", body: .object(["text": .string("Quoted")]))
        let resumed = try library.session(for: loaded, api: api)
        #expect(resumed === session)
        #expect(resumed.composer == "Keep this draft")
        #expect(resumed.replyTo?.seq == 1)
    }

    @Test func cachedDraftsAreNeverEvictedAtCapacity() throws {
        let library = Library()
        for index in 0..<32 {
            let chat = FragmentSummary(name: "draft-\(index)", kind: "chat")
            try library.session(for: chat, api: api).composer = "A draft to preserve"
        }
        do {
            _ = try library.session(for: FragmentSummary(name: "one-more", kind: "chat"), api: api)
            Issue.record("Exceeded the session bound")
        } catch {}
        #expect(library.sessions.count == 32)
        #expect(library.sessions.values.allSatisfy { $0.composer == "A draft to preserve" })
    }

    @Test func fragmentIconsUseTheListedPictureURL() async throws {
        let library = Library()
        await library.load(api)
        let app = try #require(library.apps.first { $0.name == "reading" })
        let url = try #require(app.picture.flatMap(URL.init(string:)))
        #expect(url.host == api.baseURL.host)
        #expect(url.path == "/api/f/reading/icon")
        #expect(url.query == "v=fixture-reading-v1")
        let (bytes, response) = try await api.session.data(from: url)
        #expect((response as? HTTPURLResponse)?.statusCode == 200)
        #expect(UIImage(data: bytes) != nil)
    }

    @Test func importedHistoryUsesTheSessionArchiveIndex() async throws {
        let library = Library()
        await library.load(api)
        #expect(library.error == nil)
        #expect(library.imported.contains { $0.key == "imported-chat" && $0.at > 0 })
        let archive = try #require(library.fragments.first { $0.kind == "sessions" })
        let result: JSONValue = try await api.request("api/f/\(archive.name)/search", method: "POST", body: .object(["q": .string("quiet minutes"), "limit": .number(50), "mode": .string("lexical"), "fields": .object(["kind": .string("chat")])]))
        #expect(result["results"].array.first?["source"].string == "sessions/imported-chat.json")
    }

    @Test func archiveIsIdempotentAndInvalidValueIsRejected() async throws {
        let chat = try await api.createChat()
        for _ in 0..<2 {
            let _: JSONValue = try await api.request("api/fragments/\(chat.name)/archived", method: "PUT", body: .object(["archived": .bool(true)]))
        }
        #expect(try await api.fragments().first { $0.name == chat.name }?.archived == true)
        do {
            let _: JSONValue = try await api.request("api/fragments/\(chat.name)/archived", method: "PUT", body: .object(["archived": .string("yes")]))
            Issue.record("Invalid archive value accepted")
        } catch {}
        #expect(try await api.fragments().first { $0.name == chat.name }?.archived == true)
    }

    @Test func attachmentUploadAndPostReplayKeepOneRecord() async throws {
        let chat = try await api.createChat()
        let bytes = Data("A small attachment".utf8)
        let attachment = try await api.upload(bytes, name: "note.txt", type: "text/plain", to: chat.name)
        let replay = try await api.upload(bytes, name: "note.txt", type: "text/plain", to: chat.name)
        #expect(attachment == replay)
        let body = JSONValue.object(["text": .string(""), "attachments": .array([attachment.json]), "reply_to": .number(1)])
        let first = try await api.post(chat.name, id: "attachment-test", body: body)
        let second = try await api.post(chat.name, id: "attachment-test", body: body)
        #expect(first.seq == second.seq)
        let (downloaded, response) = try await URLSession.shared.data(from: api.url("api/f/\(chat.name)/blobs/\(attachment.sha256)"))
        #expect((response as? HTTPURLResponse)?.statusCode == 200)
        #expect(downloaded == bytes)
        do { _ = try await api.upload(Data(), name: "empty.txt", type: "text/plain", to: chat.name); Issue.record("Empty attachment accepted") } catch {}
    }

    @Test func renameValidatesAndDeploysTitle() async throws {
        let chat = try await api.createChat()
        try await api.rename(chat, title: "A name to keep")
        #expect(try await api.fragments().first { $0.name == chat.name }?.title == "A name to keep")
        do { try await api.rename(chat, title: "   "); Issue.record("Empty title accepted") } catch {}
        #expect(try await api.fragments().first { $0.name == chat.name }?.title == "A name to keep")
    }
}
