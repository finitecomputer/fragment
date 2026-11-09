import Foundation
import Testing
@testable import Fragment

/// FIN-458: what the platform tells the person (`GET /api/notices`), read
/// as it answers and drawn the same way whatever its kind. Valid answers
/// (one recorded from a dev stack), what an older or newer server may send,
/// and the countdown's words, which are the web's (cell/shell/computer.js
/// `untilSaid`).
struct NoticesTests {
    private struct Read: Decodable { let notices: [Notice] }
    private func read(_ json: String) throws -> [Notice] { try JSONDecoder().decode(Read.self, from: Data(json.utf8)).notices }

    /// An update ready, as a dev stack answered it (`PUT …/image stub-next`
    /// while awake): its words, and its two actions with their routes.
    @Test func anAnswerAsTheServerWritesIt() throws {
        let told = try read(#"{"notices":[{"actions":[{"body":{"generation":1},"label":"Restart","post":"/api/computers/computer:1e85013ca1cabe00b09631d5/restart"},{"body":{"version":"845a1b0c1a02"},"label":"Later","post":"/api/computers/computer:1e85013ca1cabe00b09631d5/notices/later"}],"id":"update:845a1b0c1a02","kind":"update","level":"info","text":"Restart to get it; your chats and work are kept. It also updates by itself the next time your computer sleeps.","title":"An update is ready for your computer"}]}"#)
        #expect(told.count == 1)
        let update = try #require(told.first)
        #expect(update.id == "update:845a1b0c1a02" && update.title == "An update is ready for your computer" && !update.warns)
        #expect(update.actions.map(\.label) == ["Restart", "Later"])
        #expect(update.actions[1].post == "/api/computers/computer:1e85013ca1cabe00b09631d5/notices/later")
        #expect(update.actions[1].body == .object(["version": .string("845a1b0c1a02")]))
        #expect(update.until == nil && update.detail == nil && update.shown(at: .max))
    }

    /// A warning with a countdown and a page to open; a notice with no
    /// actions; a level this app does not know (a newer server's) is drawn
    /// as info, never refused.
    @Test func whatEachMaySay() throws {
        let told = try read(#"{"notices":[{"id":"key_paused:elevenlabs","kind":"key_paused","level":"warn","title":"ElevenLabs calls paused: $5.03 this hour","until":{"at":120000,"said":"Calls resume"},"actions":[{"label":"Get help","open":"mailto:help@example.com"}]},{"id":"brand_new","kind":"brand_new","level":"urgent","title":"Something new"}]}"#)
        #expect(told.map(\.kind) == ["key_paused", "brand_new"])
        #expect(told[0].warns && told[0].until == Notice.Until(at: 120_000, said: "Calls resume"))
        #expect(told[0].actions[0].open == "mailto:help@example.com" && told[0].actions[0].post == nil)
        #expect(told[1].actions.isEmpty && !told[1].warns)
        // past its time a notice is gone, though no one told the app yet
        #expect(told[0].shown(at: 119_999) && !told[0].shown(at: 120_000))
        // a notice with no title is no notice: the read fails, and says so
        #expect(throws: (any Error).self) { try read(#"{"notices":[{"id":"a","kind":"a","level":"info"}]}"#) }
    }

    @Test func aCountdownInWords() {
        let until = Notice.Until(at: 0, said: "Calls resume")
        #expect(Notice.untilSaid(until, now: -59_000) == "Calls resume in under a minute.")
        #expect(Notice.untilSaid(until, now: -60_000) == "Calls resume in under a minute.")
        #expect(Notice.untilSaid(until, now: -61_000) == "Calls resume in 2 min.")
        #expect(Notice.untilSaid(until, now: -3_600_000) == "Calls resume in 60 min.")
        #expect(Notice.untilSaid(until, now: -3_660_000) == "Calls resume in 1 h 1 min.")
        #expect(Notice.untilSaid(until, now: -7_200_000) == "Calls resume in 2 h.")
    }
}
