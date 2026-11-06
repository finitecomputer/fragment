import Foundation
import Testing
@testable import Fragment

struct TimelineTests {
    private func record(_ seq: Int64, channel: String = "chat", principal: String = "agent", body: JSONValue) -> ChannelRecord {
        ChannelRecord(channel: channel, seq: seq, at: 100, principal: principal, kind: "message", body: body)
    }
    private func frame(_ json: String) throws -> LiveFrame { try JSONDecoder().decode(LiveFrame.self, from: Data(json.utf8)) }

    @Test func replayAndOlderPagesNeverRegressLiveCursor() {
        var timeline = Timeline()
        let latest = record(20, body: .string("latest"))
        timeline.insert(latest)
        timeline.insert(latest)
        timeline.insert(record(2, body: .string("earlier")), advancingCursor: false)
        timeline.insert(record(10, body: .string("middle")), advancingCursor: false)
        #expect(timeline.records.map(\.seq) == [2, 10, 20])
        #expect(timeline.cursors["chat"] == 20)
    }

    @Test func draftIsWholeTextAndReplacedOnlyByItsAuthorsCommit() throws {
        var timeline = Timeline()
        try timeline.apply(frame(#"{"type":"draft","channel":"chat","turn":"a","principal":"agent","text":"One","at":100}"#))
        try timeline.apply(frame(#"{"type":"draft","channel":"chat","turn":"a","principal":"agent","text":"One two","at":101}"#))
        #expect(timeline.drafts["a"]?.text == "One two")
        timeline.insert(record(1, principal: "someone-else", body: .object(["turn": .string("a"), "text": .string("forged")])))
        #expect(timeline.drafts["a"] != nil)
        timeline.insert(record(2, body: .object(["turn": .string("a"), "text": .string("One two")])))
        #expect(timeline.drafts.isEmpty)
        #expect(timeline.records.count == 2)
    }

    @Test func replayedRecordDoesNotEraseNextPartOfTurn() throws {
        var timeline = Timeline()
        let commit = record(1, body: .object(["turn": .string("a"), "text": .string("First part")]))
        timeline.insert(commit)
        try timeline.apply(frame(#"{"type":"draft","channel":"chat","turn":"a","principal":"agent","text":"Next part","at":110}"#))
        timeline.insert(commit)
        #expect(timeline.drafts["a"]?.text == "Next part")
        timeline.disconnected()
        #expect(timeline.drafts.isEmpty)
        #expect(timeline.records.count == 1)
    }

    @Test func channelsHaveIndependentCursorsAndUnknownKindsDecode() throws {
        var timeline = Timeline()
        try timeline.apply(frame(#"{"type":"record","channel":"chat","seq":8,"at":1,"principal":"person","kind":"message","body":"plain text"}"#))
        try timeline.apply(frame(#"{"type":"record","channel":"work","seq":2,"at":2,"principal":"agent","kind":"message","body":{"kind":"future.kind","newField":true}}"#))
        #expect(timeline.cursors == ["chat": 8, "work": 2])
        #expect(timeline.records.first?.text == "plain text")
        #expect(timeline.records.last?.isMessage == false)
        #expect(throws: (any Error).self) {
            try timeline.apply(frame(#"{"type":"record","channel":"chat","seq":-1}"#))
        }
    }

    @Test func turnEndClearsWorkingAndDraft() throws {
        var timeline = Timeline()
        timeline.insert(record(1, channel: "work", body: .object(["kind": .string("turn.start"), "turn": .string("a")])))
        #expect(timeline.activeTurns == ["a"])
        try timeline.apply(frame(#"{"type":"draft","channel":"chat","turn":"a","principal":"agent","text":"unfinished","at":100}"#))
        timeline.insert(record(2, channel: "work", body: .object(["kind": .string("turn.end"), "turn": .string("a")])))
        #expect(timeline.activeTurns.isEmpty)
        #expect(timeline.drafts.isEmpty)
    }

    // ---- which turns run, as the web's chat page decides it (chat.js
    // `turnOf`, control.js `running`), records read in any order ----
    private func work(_ seq: Int64, _ kind: String, _ turn: String, by agent: String = "npub1a") -> ChannelRecord {
        ChannelRecord(channel: "work", seq: seq, at: seq * 10, principal: agent, kind: "message", body: .object(["kind": .string(kind), "turn": .string(turn)]))
    }

    @Test func anEndBeforeItsStartCountsOnlyFromTheAgentThatBeganIt() {
        // its own agent's end, read before its start (paging back): it ended
        var same = Timeline()
        same.learn(work(9, "turn.end", "t"))
        #expect(!same.awaitsAStart, "an ended turn never runs: no start is read back for it")
        same.learn(work(1, "turn.start", "t"))
        #expect(same.activeTurns.isEmpty && !same.awaitsAStart)
        // another agent's end naming it: A's turn still runs once its start is read
        var other = Timeline()
        other.learn(work(9, "turn.end", "t", by: "npub1b"))
        other.learn(work(5, "turn.step", "t"))
        other.learn(work(1, "turn.start", "t"))
        #expect(other.activeTurns == ["t"])
        other.learn(work(12, "turn.end", "t"))
        #expect(other.activeTurns.isEmpty)
    }

    /// Codex's probe: a 1000-record backlog that holds a boundary turn's end
    /// (its start older), 499 whole turns and a busy turn's step, then the
    /// page before it: the ended turn never runs again; the busy one does.
    @Test func pagingBackNeverRevivesAnEndedTurn() {
        var timeline = Timeline()
        timeline.learn(work(1001, "turn.end", "boundary"))
        for i in Int64(0)..<499 {
            timeline.learn(work(1002 + 2 * i, "turn.start", "done\(i)"))
            timeline.learn(work(1003 + 2 * i, "turn.end", "done\(i)"))
        }
        timeline.learn(work(2000, "turn.step", "busy"))
        #expect(timeline.activeTurns.isEmpty && timeline.awaitsAStart)
        for (seq, turn) in [(Int64(500), "boundary"), (Int64(990), "busy")] { timeline.learn(work(seq, "turn.start", turn)) }
        #expect(timeline.activeTurns == ["busy"] && !timeline.awaitsAStart)
    }

    @Test func pastItsBoundOnlyTurnsBegunAndEndedGo() {
        var timeline = Timeline()
        timeline.learn(work(1_000_000, "turn.end", "late"))
        for i in Int64(0)..<Int64(Timeline.maximumTurns + 10) {
            timeline.learn(work(10 + 2 * i, "turn.start", "w\(i)"))
            timeline.learn(work(11 + 2 * i, "turn.end", "w\(i)"))
        }
        #expect(timeline.turns.count == Timeline.maximumTurns)
        // the end read without its start is kept: its start, read later, never runs
        timeline.learn(work(1, "turn.start", "late"))
        #expect(timeline.activeTurns.isEmpty)
    }

    /// Records are placed in order as they come, from either end or the
    /// middle, each once: as sorting all of them would leave them.
    @Test func recordsArePlacedInOrderEachOnce() {
        var generator = SystemRandomNumberGenerator()
        var made: [ChannelRecord] = []
        for seq in 1...300 {
            let channel = seq % 3 == 0 ? "work" : "chat"
            made.append(ChannelRecord(channel: channel, seq: Int64(seq), at: Int64.random(in: 0...50, using: &generator),
                                      principal: "agent", kind: "message", body: .string("r\(seq)")))
        }
        var timeline = Timeline()
        for record in (made + made.prefix(40)).shuffled() { timeline.insert(record, advancingCursor: false) }
        #expect(timeline.records.map(\.id) == made.sorted(by: Timeline.precedes).map(\.id))
        #expect(timeline.revision == made.count, "a record read again changes nothing")
        // past its bound the oldest go, and one of them read again is let go again
        var long = Timeline()
        for seq in 1...Int64(Timeline.chatMax + 10) { long.insert(record(seq, body: .string("m"))) }
        long.insert(record(1, body: .string("m")), advancingCursor: false)
        #expect(long.records.count == Timeline.chatMax && long.records.first?.seq == 11)
    }

    /// The window counts `chat` records alone: `work` never takes a
    /// message's place, and the `work` before the oldest message kept goes
    /// with it. At the window's bound a new message keeps the count, but is
    /// the newest record: what the chat follows (`ChatView`), never a count.
    @Test func theWindowCountsMessagesAloneAndItsNewestMoves() {
        var timeline = Timeline()
        func at(_ channel: String, _ seq: Int64, _ time: Int64) -> ChannelRecord {
            ChannelRecord(channel: channel, seq: seq, at: time, principal: "agent", kind: "message", body: .object(["text": .string("r"), "kind": .string(channel == "work" ? "turn.step" : "message"), "turn": .string("t")]))
        }
        // each message with three steps before it
        for n in Int64(1)...Int64(Timeline.chatMax) {
            for k in Int64(0)..<3 { timeline.insert(at("work", n * 3 + k, n * 10 + k)) }
            timeline.insert(at("chat", n, n * 10 + 5))
        }
        #expect(timeline.chatCount == Timeline.chatMax && timeline.records.count == Timeline.chatMax * 4, "steps never count toward the window")
        let newest = timeline.records.last?.id
        let next = Int64(Timeline.chatMax + 1)
        timeline.insert(at("chat", next, next * 10 + 5))
        #expect(timeline.chatCount == Timeline.chatMax, "the count is the same at the bound")
        #expect(timeline.records.last?.id != newest && timeline.records.last?.id == "chat:\(next)", "its newest is not")
        // the oldest message went, and the steps before the oldest kept went with it
        #expect(timeline.records.first?.id == "chat:2")
        #expect(!timeline.records.contains { $0.id == "work:6" } && timeline.records.contains { $0.id == "work:7" } == false && timeline.records.contains { $0.id == "chat:2" })
    }

    /// A turn that ended never runs, so it never asks for its start to be read
    /// back; one read without a start or an end may run, and does ask.
    @Test func onlyATurnThatMayRunAwaitsItsStart() {
        var timeline = Timeline()
        timeline.learn(work(9, "turn.end", "done"))
        #expect(!timeline.awaitsAStart)
        timeline.learn(work(10, "turn.step", "busy"))
        #expect(timeline.awaitsAStart)
    }

    /// Paged back past the bound, the window lets its newest end go (and the
    /// steps after its newest kept message); a live record then says which
    /// turns run but enters no window, until a newer page reaches the newest.
    @Test func theWindowIsTwoSided() {
        func chat(_ seq: Int64) -> ChannelRecord { ChannelRecord(channel: "chat", seq: seq, at: seq * 10, principal: "a", kind: "message", body: .object(["text": .string("m")])) }
        func step(_ seq: Int64, at: Int64, _ kind: String = "turn.step") -> ChannelRecord {
            ChannelRecord(channel: "work", seq: seq, at: at, principal: "a", kind: "message", body: .object(["kind": .string(kind), "turn": .string("t")]))
        }
        var timeline = Timeline()
        for seq in Int64(551)...1000 { timeline.insert(chat(seq)) }
        timeline.insert(step(1, at: 10_005))
        timeline.insertOlder((Int64(401)...550).map(chat))
        #expect(timeline.chatCount == 600 && !timeline.detached)
        timeline.insertOlder((Int64(351)...400).map(chat))
        #expect(timeline.detached && timeline.chatCount == 600 && timeline.newestChat?.seq == 950 && timeline.oldestChat?.seq == 351)
        #expect(!timeline.records.contains { $0.id == "work:1" }, "the steps after its newest kept message went too")
        // live, detached: it runs, but waits outside
        timeline.insert(step(2, at: 10_020, "turn.start"))
        timeline.insert(chat(1001))
        #expect(timeline.activeTurns == ["t"] && timeline.newestChat?.seq == 950 && timeline.cursors["chat"] == 1001)
        timeline.insertNewer((Int64(951)...1001).map(chat), reachesNewest: true)
        #expect(!timeline.detached && timeline.newestChat?.seq == 1001 && timeline.oldestChat?.seq == 402 && timeline.chatCount == 600)
        // the newest page in place of the window: turns stay
        timeline.reset((Int64(952)...1001).map(chat))
        #expect(timeline.chatCount == 50 && timeline.activeTurns == ["t"] && timeline.cursors["chat"] == 1001)
    }

    /// A turn that ended still needs its start while the window shows its
    /// steps: its card is drawn from it.
    @Test func anEndedTurnWhoseStepsAreShownAwaitsItsStart() {
        var timeline = Timeline()
        timeline.insert(work(8, "turn.step", "shown"))
        timeline.insert(work(9, "turn.end", "shown"))
        #expect(timeline.awaitsAStart)
        timeline.learn(work(2, "turn.start", "shown"))
        #expect(!timeline.awaitsAStart && timeline.activeTurns.isEmpty)
    }
}
