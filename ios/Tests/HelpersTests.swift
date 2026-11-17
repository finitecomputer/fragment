import Foundation
import Testing
@testable import Fragment

/// FIN-432: each agent's newest helpers set (`turn.helpers`), as the
/// composer's bar reads it. Valid, invalid, replay, a lapsed set, and the
/// records change nothing else a chat draws.
@MainActor
struct HelpersTests {
    private func work(_ seq: Int64, _ body: String, agent: String = "npub1ada") throws -> ChannelRecord {
        .init(channel: "work", seq: seq, at: seq * 1000, principal: agent, kind: "message", body: try JSONDecoder().decode(JSONValue.self, from: Data(body.utf8)))
    }
    /// A set naming `helpers` (child, goal, since), held until `until`.
    private func set(_ seq: Int64, _ helpers: [(String, String, Int64)], until: Int64 = 9_000_000, agent: String = "npub1ada") throws -> ChannelRecord {
        let named = helpers.map { #"{"child":"\#($0.0)","parent":"t1","goal":"\#($0.1)","since":\#($0.2)}"# }.joined(separator: ",")
        return try work(seq, #"{"kind":"turn.helpers","helpers":[\#(named)],"until":\#(until)}"#, agent: agent)
    }

    /// Valid: a set's helpers work, the oldest first, until a newer set
    /// names them no more; an empty set, none. Replay: a set read again, or
    /// an older one read after (an earlier page, the newest asked for and
    /// the live stream both), changes nothing.
    @Test func eachAgentsNewestSetWins() throws {
        var timeline = Timeline()
        let both = try set(2, [("h2", "Compare the quotes", 2000), ("h1", "Find three venues near Lisbon", 1000)])
        timeline.insert(both)
        timeline.insert(both)
        #expect(timeline.helpers.working(at: 5000).map(\.goal) == ["Find three venues near Lisbon", "Compare the quotes"], "the oldest first, once")
        timeline.insert(try set(3, [("h2", "Compare the quotes", 2000)]))
        #expect(timeline.helpers.working(at: 5000).map(\.child) == ["h2"])
        timeline.insert(both, advancingCursor: false)
        timeline.learnHelper(both)
        timeline.insertOlder([both])
        #expect(timeline.helpers.working(at: 5000).map(\.child) == ["h2"], "an older set read after changes nothing")
        timeline.insert(try set(4, []))
        #expect(timeline.helpers.working(at: 5000).isEmpty && timeline.helpers.isEmpty)
    }

    /// A set is its poster's, whatever a body names: another agent's set
    /// ends none of the first's. Invalid: a set with no `until`, or whose
    /// list is none, is none; a helper with no child, goal or `since` in a
    /// set is none; a set on `chat` is none.
    @Test func aSetIsItsPostersAndAMalformedOneIsNone() throws {
        var timeline = Timeline()
        timeline.insert(try set(1, [("h1", "Find venues", 100)]))
        timeline.insert(try set(2, [], agent: "npub1bo"))
        #expect(timeline.helpers.working(at: 5000).map(\.agent) == ["npub1ada"])
        timeline.insert(try work(3, #"{"kind":"turn.helpers","helpers":[]}"#))
        timeline.insert(try work(4, #"{"kind":"turn.helpers","helpers":"none","until":9000000}"#))
        #expect(timeline.helpers.working(at: 5000).map(\.child) == ["h1"])
        timeline.insert(try work(5, #"{"kind":"turn.helpers","until":9000000,"helpers":[{"child":"h2","goal":"x"},{"goal":"y","since":1},{"child":"h3","since":1},{"child":"h4","goal":"z","since":7}]}"#))
        #expect(timeline.helpers.working(at: 5000).map(\.child) == ["h4"])
        timeline.insert(ChannelRecord(channel: "chat", seq: 9, at: 9, principal: "npub1ada", kind: "message", body: try JSONDecoder().decode(JSONValue.self, from: Data(#"{"kind":"turn.helpers","helpers":[],"until":9000000}"#.utf8))))
        #expect(timeline.helpers.working(at: 5000).map(\.child) == ["h4"])
        // Codex on this PR: a time out of range is none, never a trap
        timeline.insert(try work(10, #"{"kind":"turn.helpers","helpers":[],"until":1e20}"#))
        timeline.insert(try work(11, #"{"kind":"turn.helpers","helpers":[],"until":-1}"#))
        #expect(timeline.helpers.working(at: 5000).map(\.child) == ["h4"])
        timeline.insert(try work(12, #"{"kind":"turn.helpers","until":9000000,"helpers":[{"child":"h5","goal":"x","since":1e300},{"child":"h6","goal":"y","since":-1e300},{"child":"h7","goal":"z","since":8}]}"#))
        #expect(timeline.helpers.working(at: 5000).map(\.child) == ["h7"])
        #expect(HelpersBar.minutes(5000 - timeline.helpers.working(at: 5000)[0].since) == "<1 min")
    }

    /// Restart: a set whose `until` passed (its computer crashed and
    /// sleeps) says nothing, though no newer set came.
    @Test func aLapsedSetSaysNothing() throws {
        var timeline = Timeline()
        timeline.insert(try set(1, [("h1", "Find venues", 1000)], until: 900_000))
        timeline.insert(try set(2, [("b1", "Draft", 1500)], until: 2_000_000, agent: "npub1bo"))
        #expect(timeline.helpers.working(at: 899_999).map(\.child) == ["h1", "b1"])
        #expect(timeline.helpers.working(at: 900_000).map(\.child) == ["b1"], "ada's lapsed")
        #expect(timeline.helpers.working(at: 2_000_000).isEmpty)
    }

    /// What an older app reads too: a set starts no turn, ends none, draws
    /// nothing in the chat, and keeps an ended turn ended. It names no
    /// `turn`, so no app before it makes a turn of it at all.
    @Test func theRecordsChangeNothingElseAChatDraws() throws {
        var timeline = Timeline()
        timeline.insert(try work(1, #"{"kind":"turn.start","turn":"t1","agent":"npub1ada"}"#))
        timeline.insert(try set(2, [("h1", "Find venues", 2000)]))
        timeline.insert(try work(3, #"{"kind":"turn.step","turn":"t1","step":1,"tool":"delegate_task","category":"delegate","args":"Find venues","ok":true}"#))
        timeline.insert(try work(4, #"{"kind":"turn.end","turn":"t1","outcome":"idle"}"#))
        timeline.insert(try set(5, []))
        #expect(timeline.activeTurns.isEmpty)
        let activity = WorkingActivity(timeline)
        #expect(activity.hints.isEmpty)
        #expect(!activity.rows.contains { $0.record?.body["kind"].string == "turn.helpers" })
        #expect(activity.cards.values.allSatisfy { !$0.running })
        #expect(timeline.turns.count == 1, "a set makes no turn")
    }

    /// Codex on #1086: a turn asking its owner in words keeps asking while
    /// records that do not say it went on come (its agent's helpers sets, a
    /// kind this app does not know); the owner's answer in the chat, or a
    /// step, ends it.
    @Test func aQuestionOutlastsHelpersAndUnknownKinds() throws {
        let key = TurnKey(agent: "npub1ada", id: "t1")
        var timeline = Timeline()
        timeline.insert(try work(1, #"{"kind":"turn.start","turn":"t1","agent":"npub1ada"}"#))
        timeline.insert(try work(2, #"{"kind":"turn.asked","turn":"t1","asks":"npub1paul"}"#))
        let asks = { (t: Timeline) in WorkingActivity(t).hints.first { $0.id == key }?.kind }
        #expect(asks(timeline) == .asked("npub1paul"))
        timeline.insert(try set(3, [("h1", "Check the calendars", 3000)]))
        timeline.insert(try set(4, []))
        timeline.insert(try work(5, #"{"kind":"turn.later-kind","turn":"t1"}"#))
        #expect(asks(timeline) == .asked("npub1paul"), "still asking")
        var answered = timeline
        answered.insert(ChannelRecord(channel: "chat", seq: 1, at: 6000, principal: "npub1paul", kind: "message", body: .object(["text": .string("March")])))
        #expect(asks(answered) != .asked("npub1paul"), "the owner's answer ends it")
        var went = timeline
        went.insert(try work(7, #"{"kind":"turn.step","turn":"t1","step":1,"tool":"terminal","category":"shell","args":"ls","ok":true}"#))
        #expect(asks(went) != .asked("npub1paul"), "a step says it went on")
    }

    @Test func theBarSaysHowManyAndWhose() throws {
        var timeline = Timeline()
        timeline.insert(try set(1, [("h1", "a", 1)]))
        timeline.insert(try set(2, [("h2", "b", 2)], agent: "npub1bo"))
        let helpers = timeline.helpers.working(at: 5000)
        #expect(HelpersBar.said(Array(helpers.prefix(1)), names: []) == "1 helper working")
        #expect(HelpersBar.said(helpers, names: []) == "2 helpers working")
        #expect(HelpersBar.said(helpers, names: ["Ada", "Bo"]) == "Ada and Bo: 2 helpers working")
        #expect(HelpersBar.said(helpers, names: ["Ada", "Bo", "Cy"]) == "Ada and 2 others: 2 helpers working")
        #expect([0, 59_999, 60_000, 4 * 60_000 + 5, 65 * 60_000].map(HelpersBar.minutes) == ["<1 min", "<1 min", "1 min", "4 min", "1h 5m"])
    }
}
