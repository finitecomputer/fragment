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
        try work(seq, #"{"kind":"turn.child","parent":"\#(turn)","child":"\#(child)","goal":"\#(goal)"}"#, agent: agent)
    }
    private func end(_ seq: Int64, _ child: String, _ outcome: String, turn: String = "t1", agent: String = "npub1ada") throws -> ChannelRecord {
        try work(seq, #"{"kind":"turn.child","parent":"\#(turn)","child":"\#(child)","outcome":"\#(outcome)"}"#, agent: agent)
    }
    /// A note that a helper still works: its start again, with `since`.
    private func note(_ seq: Int64, _ child: String, _ goal: String, since: Int64, turn: String = "t1", agent: String = "npub1ada") throws -> ChannelRecord {
        try work(seq, #"{"kind":"turn.child","parent":"\#(turn)","child":"\#(child)","goal":"\#(goal)","since":\#(since)}"#, agent: agent)
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
        #expect(timeline.helpers.known.count == 3, "each kept, ended")
    }

    @Test func anEarlierPagesStartAfterItsEndStaysEnded() throws {
        var timeline = Timeline()
        timeline.insert(try end(9, "h1", "failed"))
        #expect(timeline.helpers.running.isEmpty)
        timeline.insert(try start(2, "h1", "Find venues"), advancingCursor: false)
        #expect(timeline.helpers.running.isEmpty)
        // and read again (earlier work read once more): still ended
        timeline.learn(try start(2, "h1", "Find venues"))
        #expect(timeline.helpers.running.isEmpty)
    }

    /// Alex on #1089: a helper read whole, then its start read again (a
    /// window paged back to after its start was let go, or earlier work
    /// read again) stays ended, however the start comes back.
    @Test func aStartReadAgainAfterItsEndStaysEnded() throws {
        var timeline = Timeline()
        let venues = try start(2, "h1", "Find venues")
        timeline.insert(venues)
        timeline.insert(try end(3, "h1", "completed"))
        timeline.learn(venues)
        timeline.insert(venues, advancingCursor: false)
        timeline.insertOlder([venues])
        #expect(timeline.helpers.running.isEmpty)
        // past the window's bound, its start long gone from it, then paged back to
        var long = Timeline()
        long.insert(venues)
        long.insert(try end(3, "h1", "completed"))
        for n in 0...Timeline.chatMax { long.insert(ChannelRecord(channel: "chat", seq: Int64(10 + n), at: Int64(10_000 + n), principal: "npub1paul", kind: "message", body: .string("message \(n)"))) }
        #expect(!long.records.contains { $0.id == venues.id }, "its start was let go from the window")
        long.insertOlder([venues])
        #expect(long.helpers.running.isEmpty)
    }

    @Test func aHelperIsItsPostersAndAMalformedOneIsNone() throws {
        var timeline = Timeline()
        timeline.insert(try start(1, "h1", "Find venues"))
        // another agent's end of it, under the same turn and child, ends nothing
        timeline.insert(try end(2, "h1", "completed", agent: "npub1bo"))
        timeline.insert(try start(3, "h1", "Same child, another agent", agent: "npub1bo"))
        #expect(timeline.helpers.running.map(\.agent) == ["npub1ada"], "bo's end came first, so its own start is ended")
        for (seq, bad) in [(4, #"{"kind":"turn.child","parent":"t1","child":"","goal":"x"}"#), (5, #"{"kind":"turn.child","parent":"t1","child":7,"goal":"x"}"#), (6, #"{"kind":"turn.child","child":"h9","goal":"x"}"#), (7, #"{"kind":"turn.child","parent":"t1","child":"h9"}"#), (8, #"{"kind":"turn.child","turn":"t1","child":"h9","goal":"a reader before helpers' name for its turn"}"#)] {
            timeline.insert(try work(Int64(seq), bad))
        }
        let chat = ChannelRecord(channel: "chat", seq: 9, at: 9, principal: "npub1ada", kind: "message", body: .object(["kind": .string("turn.child"), "parent": .string("t1"), "child": .string("h8"), "goal": .string("not work")]))
        timeline.insert(chat)
        #expect(timeline.helpers.running.map(\.child) == ["h1"] && timeline.helpers.known.count == 2)
    }

    /// What an older app reads too: the records start no turn, end none,
    /// draw nothing in the chat, and a helper's end after its turn's end
    /// keeps that turn ended. They name their turn `parent`, never `turn`,
    /// so no app before them makes a turn of them at all.
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
        #expect(!activity.rows.contains { $0.record?.body["kind"].string == "turn.child" })
        #expect(activity.cards.values.allSatisfy { !$0.running })
        #expect(timeline.helpers.running.map(\.child) == ["h9"])
        #expect(timeline.turns[TurnKey(agent: "npub1ada", id: "t9")] == nil, "a helper's records make no turn")
    }

    /// FIN-432, Paul's (b) of 11-07. Valid: a helper whose start is long
    /// out of what was read works from its note alone, from its `since`;
    /// its start read later times it from the start's record. Replay: a
    /// note read again, or a later one, is the same helper, and a note
    /// after its end, in any order, starts nothing.
    @Test func aLongHelperWorksFromItsNote() throws {
        var timeline = Timeline()
        let read = 2_000_000_000
        let since: Int64 = 2_000_000_000 - 40 * 60_000
        timeline.insert(try note(Int64(read / 1000), "h1", "Read every review", since: since))
        let h = timeline.helpers.running
        #expect(h.map(\.goal) == ["Read every review"] && h.first?.at == since && h.first?.seen == Int64(read))
        #expect(Helpers.working(h, at: Int64(read) + 60_000).count == 1)
        #expect(HelpersBar.minutes(Int64(read) + 60_000 - h[0].at) == "41 min", "how long since its start")
        // replay: the same note, and a later one: one helper, its start kept
        timeline.learn(try note(Int64(read / 1000), "h1", "Read every review", since: since))
        timeline.insert(try note(Int64(read / 1000) + 600, "h1", "Read every review", since: since))
        #expect(timeline.helpers.running.count == 1 && timeline.helpers.running[0].seen == Int64(read) + 600_000 && timeline.helpers.known.count == 1)
        // its start read later (an earlier page): timed from it
        timeline.insert(try start(1_997_599, "h1", "Read every review"), advancingCursor: false)
        #expect(timeline.helpers.running.first?.at == 1_997_599_000 && timeline.helpers.running.first?.seen == Int64(read) + 600_000)
        // a note after its end, either order: ended
        timeline.insert(try end(Int64(read / 1000) + 700, "h1", "completed"))
        timeline.learn(try note(Int64(read / 1000) + 600, "h1", "Read every review", since: since))
        var earlier = Timeline()
        earlier.insert(try end(9, "h2", "completed"))
        earlier.insert(try note(8, "h2", "Draft", since: 1), advancingCursor: false)
        #expect(timeline.helpers.running.isEmpty && earlier.helpers.running.isEmpty)
    }

    /// Invalid: a note with no task, or a `since` that is no whole number
    /// of ms above 0 at or before its record's `at`, is none.
    @Test func aMalformedNoteIsNone() throws {
        var timeline = Timeline()
        let bad = [#""soon""#, "-5", "0", "1.5", "null", "9000", "[1]"]
        for (n, since) in bad.enumerated() {
            timeline.insert(try work(Int64(n + 1), #"{"kind":"turn.child","parent":"t1","child":"b\#(n)","goal":"x","since":\#(since)}"#))
        }
        timeline.insert(try work(20, #"{"kind":"turn.child","parent":"t1","child":"b9","since":5}"#))
        #expect(timeline.helpers.known.isEmpty, "\(timeline.helpers.known)")
        // at its record's `at` exactly is a note
        timeline.insert(try work(21, #"{"kind":"turn.child","parent":"t1","child":"ok","goal":"x","since":21000}"#))
        #expect(timeline.helpers.running.map(\.child) == ["ok"])
    }

    /// Restart: a helper no note has come for in `Helpers.readBack` (its
    /// computer crashed, its notes stopped) works no more; a new life's
    /// `lost` end ends it.
    @Test func aHelperNoNoteCameForWorksNoMore() throws {
        var timeline = Timeline()
        timeline.insert(try start(1_000_000, "h1", "Find venues"))
        timeline.insert(try note(1_000_600, "h2", "Compare quotes", since: 1_000_000_000))
        let running = timeline.helpers.running
        let noted = Int64(1_000_600_000)
        #expect(Helpers.working(running, at: noted + Helpers.readBack).map(\.child) == ["h2"], "h1's start is older than that")
        #expect(Helpers.working(running, at: noted + Helpers.readBack + 1).isEmpty)
        #expect(Helpers.working(running, at: 1_000_000_000 + Helpers.readBack).map(\.child) == ["h1", "h2"])
        timeline.insert(try end(1_000_700, "h2", "lost"))
        #expect(timeline.helpers.running.map(\.child) == ["h1"])
    }

    /// Codex on #1086: a turn asking its owner in words keeps asking while
    /// records that do not say it went on come (a helper's start and end,
    /// a kind this app does not know); the owner's answer in the chat, or
    /// a step, ends it.
    @Test func aQuestionOutlastsHelpersAndUnknownKinds() throws {
        let key = TurnKey(agent: "npub1ada", id: "t1")
        var timeline = Timeline()
        timeline.insert(try work(1, #"{"kind":"turn.start","turn":"t1","agent":"npub1ada"}"#))
        timeline.insert(try work(2, #"{"kind":"turn.asked","turn":"t1","asks":"npub1paul"}"#))
        let asks = { (t: Timeline) in WorkingActivity(t).hints.first { $0.id == key }?.kind }
        #expect(asks(timeline) == .asked("npub1paul"))
        timeline.insert(try start(3, "h1", "Check the calendars"))
        timeline.insert(try end(4, "h1", "completed"))
        timeline.insert(try work(5, #"{"kind":"turn.later-kind","turn":"t1"}"#))
        #expect(asks(timeline) == .asked("npub1paul"), "still asking")
        var answered = timeline
        answered.insert(ChannelRecord(channel: "chat", seq: 1, at: 6000, principal: "npub1paul", kind: "message", body: .object(["text": .string("March")])))
        #expect(asks(answered) != .asked("npub1paul"), "the owner's answer ends it")
        var went = timeline
        went.insert(try work(7, #"{"kind":"turn.step","turn":"t1","step":1,"tool":"terminal","category":"shell","args":"ls","ok":true}"#))
        #expect(asks(went) != .asked("npub1paul"), "a step says it went on")
    }

    @Test func heldToItsBound() throws {
        var timeline = Timeline()
        for n in 0...Helpers.keptMax { timeline.insert(try start(Int64(n + 1), "h\(n)", "task \(n)")) }
        #expect(timeline.helpers.known.count == Helpers.keptMax)
        #expect(timeline.helpers.running.first?.child == "h1", "the one learned longest ago went")
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
