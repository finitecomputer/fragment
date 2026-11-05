import Foundation
import Testing
@testable import Fragment

struct SearchTests {
    @Test func longPromptOnlyBoundsSearchHint() {
        let prompt = "one two three four five six seven eight nine ten"
        #expect(FragmentAPI.searchHint(prompt) == "one two three four five six seven eight")
        let unicode = String(repeating: "🙂", count: 100)
        let hint = FragmentAPI.searchHint(unicode)
        #expect(hint.utf8.count == 256)
        #expect(FragmentAPI.searchHint(" \n\t ").isEmpty)
        #expect(FragmentAPI.searchHint("one\ntwo") == "one two")
    }

    @Test func searchSendsTheHintAndReadsFragmentsAndMessages() async throws {
        let stub = Stub { seen, _ in
            (200, #"{"fragments":[{"name":"reading--p1","kind":"app","title":"Reading"}],"messages":[{"fragment":"talk--c1","channel":"chat","seq":2,"at":2000,"snippet":"a few quiet minutes"}]}"#)
        }
        let api = try stub.api()
        let found = try await api.search("quiet minutes, then a long tail of words past the eighth")
        #expect(stub.seen.map(\.route) == ["GET /api/search"])
        #expect(stub.seen[0].query == ["q": "quiet minutes, then a long tail of words"])
        #expect(found.fragments.map(\.name) == ["reading--p1"])
        #expect(found.messages.map(\.id) == ["talk--c1:chat:2"])
        let empty = try await api.search(" ")
        #expect(empty.fragments.isEmpty && empty.messages.isEmpty && stub.seen.count == 1)
    }
}
