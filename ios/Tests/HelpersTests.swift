import Foundation
import Testing
@testable import Fragment

/// FIN-432: the helpers an agent's turns started (`turn.child`), as the
/// composer's bar reads them. Valid, invalid, replay and an earlier page's
/// order; and the records change nothing else a chat draws.
@MainActor
struct HelpersTests {
    private func work(_ seq: Int64, _ body: String, agent: String = "npub1ada") throws -> ChannelRecord {
        .init(channel: "work", seq: seq, at: seq * 1000, principal: agent, kind: "message", body: try JSONDecoder().decode(JSONValue.self, from: Data(body.utf8)))
    }
    private func start(_ seq: Int64, _ child: String, _ goal: String, turn: String = "t1", agent: String = "npub1ada") throws -> ChannelRecord {
        try work(seq, #"{"kind":"turn.child","turn":"\#(turn)","child":"\#(child)","goal":"\#(goal)"}"#, agent: agent)
    }
    private func end(_ seq: Int64, _ child: String, _ outcome: String, turn: String = "t1", agent: String = "npub1ada") throws -> ChannelRecord {
        try work(seq, #"{"kind":"turn.child","turn":"\#(turn)","child":"\#(child)","outcome":"\#(outcome)"}"#, agent: agent)
    }

    @Test func aHelperWorksFromItsStartUntilAnyEnd() throws {
        var timeline = Timeline()
        timeline.insert(try work(1, #"{"kind":"turn.start","turn":"t1","agent":"npub1ada"}"#))
        let venues = try start(2, "h1", "Find three venues near Lisbon")
        timeline.insert(venues)
        timeline.insert(venues)
        timeline.insert(try start(3, "h2", "Compare the quotes"))
        #expect(timeline.helpers.running.map(\.goal) == ["Find three venues near Lisbon", "Compare the quotes"], "the oldest first, once")
        #expect(timeline.helpers.running.first?.at == 2000)
        // any end ends it: done, lost, an outcome a later bridge adds
        timeline.insert(try end(4, "h1", "completed"))
        #expect(timeline.helpers.running.map(\.child) == ["h2"])
        timeline.insert(try start(5, "h3", "Draft it"))
        timeline.insert(try end(6, "h3", "something-new"))
        timeline.insert(try end(7, "h2", "lost"))
        #expect(timeline.helpers.running.isEmpty)
        #expect(timeline.helpers.open.isEmpty, "each read whole is let go")
    }

    @Test func anEarlierPagesStartAfterItsEndStaysEnded() throws {
        var timeline = Timeline()
        timeline.insert(try end(9, "h1", "failed"))
        #expect(timeline.helpers.running.isEmpty)
        timeline.insert(try start(2, "h1", "Find venues"), advancingCursor: false)
        #expect(timeline.helpers.running.isEmpty && timeline.helpers.open.isEmpty)
    }

    @Test func aHelperIsItsPostersAndAMalformedOneIsNone() throws {
        var timeline = Timeline()
        timeline.insert(try start(1, "h1", "Find venues"))
        // another agent's end of it, under the same turn and child, ends nothing
        timeline.insert(try end(2, "h1", "completed", agent: "npub1bo"))
        timeline.insert(try start(3, "h1", "Same child, another agent", agent: "npub1bo"))
        #expect(timeline.helpers.running.map(\.agent) == ["npub1ada"], "bo's end came first, so its own start is ended")
        for (seq, bad) in [(4, #"{"kind":"turn.child","turn":"t1","child":"","goal":"x"}"#), (5, #"{"kind":"turn.child","turn":"t1","child":7,"goal":"x"}"#), (6, #"{"kind":"turn.child","child":"h9","goal":"x"}"#), (7, #"{"kind":"turn.child","turn":"t1","child":"h9"}"#)] {
            timeline.insert(try work(Int64(seq), bad))
        }
        let chat = ChannelRecord(channel: "chat", seq: 8, at: 8, principal: "npub1ada", kind: "message", body: .object(["kind": .string("turn.child"), "turn": .string("t1"), "child": .string("h8"), "goal": .string("not work")]))
        timeline.insert(chat)
        #expect(timeline.helpers.running.map(\.child) == ["h1"] && timeline.helpers.open.count == 1)
    }

    /// What an older app reads too: the records start no turn, end none,
    /// draw nothing in the chat, and a helper's end after its turn's end
    /// keeps that turn ended.
    @Test func theRecordsChangeNothingElseAChatDraws() throws {
        var timeline = Timeline()
        timeline.insert(try work(1, #"{"kind":"turn.start","turn":"t1","agent":"npub1ada"}"#))
        timeline.insert(try start(2, "h1", "Find venues"))
        timeline.insert(try work(3, #"{"kind":"turn.step","turn":"t1","step":1,"tool":"delegate_task","category":"delegate","args":"Find venues","ok":true}"#))
        timeline.insert(try work(4, #"{"kind":"turn.end","turn":"t1","outcome":"idle"}"#))
        timeline.insert(try start(5, "h9", "A helper of a turn never read", turn: "t9"))
        timeline.insert(try end(6, "h1", "completed"))
        #expect(timeline.activeTurns.isEmpty)
        let activity = WorkingActivity(timeline)
        #expect(activity.hints.isEmpty)
        #expect(!activity.rows.contains { $0.body["kind"].string == "turn.child" })
        #expect(activity.cards.values.allSatisfy { !$0.running })
        #expect(timeline.helpers.running.map(\.child) == ["h9"])
    }

    @Test func heldToItsBound() throws {
        var timeline = Timeline()
        for n in 0...Helpers.keptMax { timeline.insert(try start(Int64(n + 1), "h\(n)", "task \(n)")) }
        #expect(timeline.helpers.open.count == Helpers.keptMax)
        #expect(timeline.helpers.running.first?.child == "h1", "the oldest went")
    }

    @Test func theBarSaysHowManyAndWhose() throws {
        var timeline = Timeline()
        timeline.insert(try start(1, "h1", "a"))
        timeline.insert(try start(2, "h2", "b", agent: "npub1bo"))
        let helpers = timeline.helpers.running
        #expect(HelpersBar.said(Array(helpers.prefix(1)), names: []) == "1 helper working")
        #expect(HelpersBar.said(helpers, names: []) == "2 helpers working")
        #expect(HelpersBar.said(helpers, names: ["Ada", "Bo"]) == "Ada and Bo: 2 helpers working")
        #expect(HelpersBar.said(helpers, names: ["Ada", "Bo", "Cy"]) == "Ada and 2 others: 2 helpers working")
        #expect([0, 59_999, 60_000, 4 * 60_000 + 5, 65 * 60_000].map(HelpersBar.minutes) == ["<1 min", "<1 min", "1 min", "4 min", "1h 5m"])
    }
}
