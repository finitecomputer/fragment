import Foundation
import Testing
@testable import Fragment

/// A chat's saved draft and outbox (ChatDraftStore) across restarts: what
/// is saved is always what loads, and no saved file ever stops a chat.
@MainActor
struct DraftTests {
    private let chat = FragmentSummary(name: "chat--d1", kind: "chat")
    private var posts: String { "POST /api/f/\(chat.name)/channels/chat" }

    @Test func anOversizedPasteIsCutVisiblyAndTheDraftSurvivesARestart() async throws {
        let route = posts
        let stub = Stub { seen, _ in seen.route == route ? (200, #"{"record":\#(record("chat", 1, principal: "npub1me", body: #"{"text":"ok"}"#)),"replayed":false}"#) : (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        session.edit(String(repeating: "é", count: 20_000))  // 40,000 bytes
        #expect(session.composer.utf8.count == SavedChat.textMax && session.error?.contains("cut") == true)
        session.flushDraft()
        let reopened = ChatSession(fragment: chat, api: api)
        #expect(reopened.composer == session.composer && reopened.error == nil)
        await reopened.send()
        #expect(stub.seen.filter { $0.route == route }.count == 1 && reopened.pending.isEmpty)
    }

    @Test func aRefusedSendBesideADraftWaitsToBeEditedAcrossARestart() async throws {
        let route = posts
        let stub = Stub { seen, before in
            guard seen.route == route else { return (404, "{}") }
            // lost the first time, refused the second
            return before.contains { $0.route == route } ? (409, #"{"error":"conflicting_body","message":"Not that one."}"#) : (503, #"{"message":"Try again."}"#)
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        session.edit("First words")
        await session.send()
        let sent = try #require(session.pending.first)
        session.edit("A draft of more words")
        await session.retry(sent)
        // the draft stays as it was; the refused send waits, never retried
        #expect(session.composer == "A draft of more words")
        #expect(session.pending.count == 1 && session.pending[0].refused == "Not that one.")
        session.flushDraft()

        let reopened = ChatSession(fragment: chat, api: api)
        #expect(reopened.error == nil && reopened.composer == "A draft of more words")
        let refused = try #require(reopened.pending.first)
        #expect(refused.refused == "Not that one." && refused.text == "First words")
        await reopened.retry(refused)
        #expect(stub.seen.filter { $0.route == route }.count == 2, "a refused send is never sent again")
        reopened.reopen(refused)
        #expect(reopened.pending.count == 1 && reopened.composer == "A draft of more words", "a draft is never joined past a message's size")
        reopened.edit("")
        reopened.reopen(refused)
        #expect(reopened.pending.isEmpty && reopened.composer == "First words")
    }

    @Test func aSavedDraftPastItsBoundsOrUnreadableNeverStopsTheChat() async throws {
        let route = posts
        let stub = Stub { seen, _ in seen.route == route ? (200, #"{"record":\#(record("chat", 1, principal: "npub1me", body: #"{"text":"ok"}"#)),"replayed":false}"#) : (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let store = try ChatDraftStore(api: api, fragment: chat.name)
        try FileManager.default.createDirectory(at: store.directory, withIntermediateDirectories: true)

        // written by something else: a composer past a message, a send no
        // send could be, beside one that can
        let over = SavedChat(composer: String(repeating: "x", count: 40_000), attachments: [], recipients: [],
                             pending: [PendingPost(id: "", text: "no id"), PendingPost(id: "p1", text: "kept")])
        try JSONEncoder().encode(over).write(to: store.file)
        let cut = ChatSession(fragment: chat, api: api)
        #expect(cut.composer.utf8.count == SavedChat.textMax && cut.pending.map(\.id) == ["p1"])
        #expect(cut.error?.contains("was cut") == true)
        await cut.send()
        #expect(stub.seen.filter { $0.route == route }.count == 1, "it still sends")

        try Data("not a saved chat".utf8).write(to: store.file)
        let cleared = ChatSession(fragment: chat, api: api)
        #expect(cleared.composer.isEmpty && cleared.pending.isEmpty && cleared.error?.contains("cleared") == true)
        #expect(!FileManager.default.fileExists(atPath: store.file.path))
        cleared.edit("Fresh")
        await cleared.send()
        #expect(stub.seen.filter { $0.route == route }.count == 2, "and so does this one")
    }
}
