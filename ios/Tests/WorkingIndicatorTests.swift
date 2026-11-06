import SwiftUI
import Testing
@testable import Fragment

@MainActor
struct WorkingIndicatorTests {
    private func work(_ seq: Int64, _ body: String, agent: String = "npub1preview") throws -> ChannelRecord {
        .init(channel: "work", seq: seq, at: seq * 1000, principal: agent, kind: "message", body: try JSONDecoder().decode(JSONValue.self, from: Data(body.utf8)))
    }

    /// chat_polish.rs calls-card fixture: same provider id in two responses,
    /// then a renamed result paired by tool/arguments, replayed out of order.
    @Test func callCardsReplaceTheRightResponseAndKeepAgentOwnership() throws {
        var timeline = Timeline()
        let start = try work(1, #"{"kind":"turn.start","turn":"calls","agent":"npub1preview"}"#)
        let first = try work(2, #"{"kind":"turn.call","turn":"calls","response":"r1","tool_call_id":"call_0","tool":"terminal","category":"shell","args":"make build"}"#)
        let second = try work(3, #"{"kind":"turn.call","turn":"calls","response":"r2","tool_call_id":"call_0","tool":"terminal","category":"shell","args":"npm test"}"#)
        for record in [start, first, second] { timeline.insert(record) }
        let open = try #require(WorkingActivity(timeline).cards[first.id])
        let backlog = WorkingActivity(timeline, ready: false)
        #expect(backlog.hints.isEmpty)
        #expect(backlog.cards.values.allSatisfy { !$0.running && $0.steps.allSatisfy { !$0.running } })
        #expect(open.steps.count == 2 && open.running)
        #expect(open.steps[0].maybe && !open.steps[0].running)
        #expect(open.steps[1].running && !open.steps[1].maybe)
        #expect(WorkingActivity(timeline).hints.isEmpty)
        let ended = try work(4, #"{"kind":"turn.step","turn":"calls","step":1,"tool":"terminal","category":"shell","args":"npm test","ok":true,"excerpt":"Exit code: 0","tool_call_id":"call_0_d2","status":"completed","duration_ms":900,"response":"r2"}"#)
        timeline.insert(ended)
        timeline.insert(ended)
        timeline.insert(first, advancingCursor: false)
        timeline.insert(try work(5, #"{"kind":"turn.end","turn":"calls","outcome":"idle"}"#, agent: "npub1other"))
        let replaced = try #require(WorkingActivity(timeline).cards[first.id])
        #expect(replaced.steps.map(\.record.id) == [first.id, ended.id])
        #expect(replaced.running && replaced.steps.last?.running == false)
        timeline.insert(try work(6, #"{"kind":"turn.end","turn":"calls","outcome":"idle"}"#))
        #expect(WorkingActivity(timeline).cards[first.id]?.until == 6000)
    }

    @Test func resultsConsumeCallsOnceWithIDsBeforeArguments() throws {
        struct Pairing {
            let calls: [(id: String, args: String, response: String)]
            let ends: [(id: String, args: String, response: String?)]
            let open: [Int]
        }
        let cases: [Pairing] = [
            .init(calls: [("c1", "ls", "r1"), ("c2", "ls", "r1")], ends: [("c1", "ls", "r1")], open: [1]),
            .init(calls: [("c1", "ls", "r1"), ("c2", "ls", "r1")], ends: [("c2", "ls", "r1")], open: [0]),
            .init(calls: [("c0", "ls", "r1"), ("c0", "ls", "r1")], ends: [("c0", "ls", "r1")], open: [1]),
            .init(calls: [("c0", "first", "r1"), ("c0", "second", "r1")], ends: [("c0", "second", "r1")], open: [1]),
            .init(calls: [("c1", "ls", "r1"), ("c2", "ls", "r1")], ends: [("renamed", "ls", "r1")], open: [1]),
            // Reserve the later exact ID before the earlier fallback result.
            .init(calls: [("c1", "first", "r1"), ("c2", "second", "r1")], ends: [("renamed", "first", "r1"), ("c1", "second", "r1")], open: [1]),
            .init(calls: [("c0", "ls", "r1"), ("c0", "ls", "r2")], ends: [("c0", "ls", "r2")], open: [0]),
            .init(calls: [("c0", "ls", "r1"), ("c0", "ls", "r2")], ends: [("c0", "ls", nil)], open: [1]),
            .init(calls: [("c0", "ls", "r1"), ("c0", "ls", "r2")], ends: [("renamed", "ls", nil)], open: [1]),
            .init(calls: [("c0", "ls", "r1")], ends: [("c0", "ls", "another-response")], open: [0])
        ]
        for fixture in cases {
            var records = [try work(1, #"{"kind":"turn.start","turn":"paired"}"#)]
            for (index, call) in fixture.calls.enumerated() {
                records.append(activityRecord(Int64(index + 2), kind: "turn.call", id: call.id, args: call.args, response: .string(call.response)))
            }
            for (index, end) in fixture.ends.enumerated() {
                records.append(activityRecord(Int64(index + 10), kind: "turn.step", id: end.id, args: end.args, response: end.response.map(JSONValue.string)))
            }
            // Neither another agent nor another turn can consume our calls.
            records.append(activityRecord(30, kind: "turn.step", id: fixture.calls[0].id, args: fixture.calls[0].args, response: .string(fixture.calls[0].response), agent: "npub1other"))
            records.append(activityRecord(31, kind: "turn.step", id: fixture.calls[0].id, args: fixture.calls[0].args, response: .string(fixture.calls[0].response), turn: "other"))
            var timeline = Timeline()
            for record in records { timeline.insert(record) }
            let expected = fixture.open.map { records[$0 + 1].id }.sorted()
            func calls(_ activity: WorkingActivity) -> [ToolStep] {
                activity.cards.values.filter { $0.turn == TurnKey(agent: "npub1preview", id: "paired") }
                    .flatMap(\.steps).filter { $0.record.body["kind"].string == "turn.call" }
            }
            let initial = WorkingActivity(timeline)
            #expect(calls(initial).map(\.id).sorted() == expected)
            var replay = Timeline()
            for record in records.reversed() { replay.insert(record); replay.insert(record) }
            #expect(WorkingActivity(replay) == initial)
            // Interruption retains exactly the unended calls as uncertain.
            let stop = try work(40, #"{"kind":"turn.end","turn":"paired","outcome":"stopped"}"#)
            timeline.insert(stop); replay.insert(stop); replay.insert(stop)
            let interrupted = WorkingActivity(timeline)
            #expect(calls(interrupted).map(\.id).sorted() == expected)
            #expect(calls(interrupted).allSatisfy { $0.maybe && !$0.running })
            #expect(WorkingActivity(replay) == interrupted)
        }
        // Explicit null is not the legacy omitted field.
        var nullResponse = Timeline()
        nullResponse.insert(try work(1, #"{"kind":"turn.start","turn":"paired"}"#))
        nullResponse.insert(activityRecord(2, kind: "turn.call", id: "c1", args: "ls", response: .string("r1")))
        nullResponse.insert(activityRecord(3, kind: "turn.step", id: "c1", args: "ls", response: .null))
        #expect(WorkingActivity(nullResponse).cards.values.flatMap(\.steps).contains { $0.id == "work:2" && $0.running })
    }

    private func activityRecord(_ seq: Int64, kind: String, id: String, args: String, response: JSONValue?, agent: String = "npub1preview", turn: String = "paired") -> ChannelRecord {
        var body: [String: JSONValue] = ["kind": .string(kind), "turn": .string(turn), "tool_call_id": .string(id), "tool": .string("terminal"), "category": .string("shell"), "args": .string(args)]
        if let response { body["response"] = response }
        if kind == "turn.step" { body["step"] = .number(Double(seq)); body["status"] = .string("completed"); body["ok"] = .bool(true) }
        return .init(channel: "work", seq: seq, at: seq * 1000, principal: agent, kind: "message", body: .object(body))
    }

    @Test func anIntermediateReplyClosesTheCardButNotAnUnendedCall() throws {
        let start = try work(1, #"{"kind":"turn.start","turn":"paired"}"#)
        let step = activityRecord(2, kind: "turn.step", id: "done", args: "ls", response: .string("r1"))
        let reply = ChannelRecord(channel: "chat", seq: 1, at: 3000, principal: "npub1preview", kind: "message", body: .object(["turn": .string("paired"), "text": .string("Checking the next thing")]))
        var timeline = Timeline()
        timeline.insert(start); timeline.insert(step)
        #expect(WorkingActivity(timeline).cards[step.id]?.running == true)
        timeline.insert(reply); timeline.insert(reply)
        let afterReply = WorkingActivity(timeline)
        #expect(afterReply.cards[step.id]?.running == false)
        #expect(afterReply.cards[step.id]?.until == 3000)
        #expect(afterReply.rows.map(\.id) == [step.id, reply.id])
        #expect(afterReply.hints.map(\.kind) == [.working(asker: nil)])
        var replay = Timeline()
        for record in [reply, step, reply, start, step] { replay.insert(record) }
        #expect(WorkingActivity(replay) == afterReply)
        // A later unended call remains live after the reply, in its own card.
        let call = activityRecord(4, kind: "turn.call", id: "next", args: "pwd", response: .string("r2"))
        timeline.insert(call); replay.insert(call)
        let next = WorkingActivity(timeline)
        #expect(next.cards[step.id]?.until == 3000)
        #expect(next.cards[call.id]?.running == true)
        #expect(next.hints.isEmpty)
        #expect(WorkingActivity(replay) == next)
        // A replayed reply later than an open call does not end that call.
        let laterReply = ChannelRecord(channel: "chat", seq: 2, at: 5000, principal: reply.principal, kind: reply.kind, body: reply.body)
        timeline.insert(laterReply); replay.insert(laterReply); replay.insert(laterReply)
        #expect(WorkingActivity(timeline).cards[call.id]?.running == true)
        #expect(WorkingActivity(timeline) == WorkingActivity(replay))
        let end = try work(6, #"{"kind":"turn.end","turn":"paired","outcome":"idle"}"#)
        timeline.insert(end); replay.insert(end)
        #expect(WorkingActivity(timeline).cards[step.id]?.until == 3000)
        #expect(WorkingActivity(timeline).cards.values.allSatisfy { !$0.running })
        #expect(WorkingActivity(timeline) == WorkingActivity(replay))
    }

    @Test func completedCallsCountFromTheTurnStartAndDoNotEndAnEarlierCard() throws {
        var timeline = Timeline()
        for (seq, body) in [
            (1, #"{"kind":"turn.start","turn":"timed"}"#),
            (2, #"{"kind":"turn.call","turn":"timed","response":"r1","tool_call_id":"c1","tool":"terminal","category":"shell","args":"first"}"#),
            (3, #"{"kind":"turn.step","turn":"timed","step":1,"response":"r1","tool_call_id":"c1","tool":"terminal","category":"shell","args":"first","ok":true,"status":"completed"}"#),
            (4, #"{"kind":"turn.call","turn":"timed","response":"r2","tool_call_id":"c2","tool":"terminal","category":"shell","args":"second"}"#),
            (5, #"{"kind":"turn.step","turn":"timed","step":2,"response":"r2","tool_call_id":"c2","tool":"terminal","category":"shell","args":"second","ok":true,"status":"completed"}"#),
            (6, #"{"kind":"turn.end","turn":"timed","outcome":"idle"}"#)
        ] { timeline.insert(try work(Int64(seq), body)) }
        timeline.insert(.init(channel: "chat", seq: 1, at: 4500, principal: "npub1preview", kind: "message", body: .object(["turn": .string("timed"), "text": .string("Next tool")])))
        let activity = WorkingActivity(timeline)
        #expect(activity.cards["work:3"]?.since == 1000)
        #expect(activity.cards["work:3"]?.until == 4500)
        #expect(activity.cards["work:5"]?.since == 5000)
        #expect(activity.cards["work:5"]?.until == 6000)
    }

    @Test func asksResumeAfterTheAskersWordsAndAnExpiredCardCanShowWork() throws {
        var timeline = Timeline()
        timeline.insert(try work(1, #"{"kind":"turn.start","turn":"asked","asker":"npub1person"}"#))
        timeline.insert(try work(2, #"{"kind":"turn.asked","turn":"asked","asks":"npub1person"}"#))
        #expect(WorkingActivity(timeline).hints.first?.kind == .asked("npub1person"))
        timeline.insert(.init(channel: "chat", seq: 1, at: 2500, principal: "npub1person", kind: "message", body: .object(["text": .string("Yes")])))
        #expect(WorkingActivity(timeline).hints.first?.kind == .working(asker: "npub1person"))
        timeline.insert(try work(3, #"{"kind":"turn.prompt","turn":"asked","prompt":"p1","asks":"npub1person","text":"Continue?","options":[{"id":"yes","label":"Yes"}],"expiresAt":10000}"#))
        #expect(WorkingActivity(timeline).hints.first?.notBefore == 10000)
        timeline.insert(try work(4, #"{"kind":"turn.prompt.closed","turn":"asked","prompt":"p1","outcome":"answered","option":"yes"}"#))
        #expect(WorkingActivity(timeline).hints.first?.notBefore == nil)
    }

    @Test func aClaimRememberedBeyondTheWindowClearsPending() throws {
        var timeline = Timeline()
        timeline.insert(.init(channel: "chat", seq: 1, at: 500, principal: "npub1person", kind: "message", body: .object(["text": .string("Hello")])))
        #expect(WorkingActivity.pending(timeline, principal: "npub1person", lead: "npub1preview") != nil)
        let revision = timeline.revision
        timeline.learn(try work(1, #"{"kind":"turn.start","turn":"claimed","cause":{"channel":"chat","seq":1}}"#))
        #expect(timeline.revision > revision)
        #expect(WorkingActivity.pending(timeline, principal: "npub1person", lead: "npub1preview") == nil)
        #expect(WorkingIndicator.duration(59_500) == "1m 00s")
        #expect(WorkingIndicator.duration(3_601_000) == "1h 0m")
    }
    @Test func olderStepWordsSplitCardsAndRepliesDeduplicateThem() throws {
        var timeline = Timeline()
        timeline.insert(try work(1, #"{"kind":"turn.start","turn":"working-card"}"#))
        timeline.insert(try work(2, #"{"kind":"turn.step","turn":"working-card","step":1,"category":"read","tool":"read_file","args":"README.md","ok":true,"text":"I’ll read it."}"#))
        timeline.insert(try work(3, #"{"kind":"turn.step","turn":"working-card","step":2,"category":"write","tool":"patch","args":"WorkingCard.tsx","ok":true}"#))
        timeline.insert(try work(4, #"{"kind":"turn.step","turn":"working-card","step":3,"tool":"terminal","args":"ls","ok":true,"text":"Installed cleanly. Now listing the app:"}"#))
        let activity = WorkingActivity(timeline)
        #expect(activity.cards.count == 2)
        #expect(activity.cards["work:2"]?.words == "I’ll read it.")
        #expect(activity.cards["work:2"]?.steps.count == 2)
        #expect(activity.cards["work:4"]?.words == "Installed cleanly. Now listing the app:")
        timeline.insert(.init(channel: "chat", seq: 1, at: 1500, principal: "npub1preview", kind: "message", body: .object(["turn": .string("working-card"), "text": .string("I’ll read it.")])))
        #expect(WorkingActivity(timeline).cards["work:2"]?.words == nil)
    }

    @Test func narrowPathsKeepTheirFilenameAndShellPrefix() {
        #expect(WorkingIndicator.singleLine("cat > make.py <<'EOF'\nimport math\n\tfrom PIL import Image") == "cat > make.py <<'EOF' import math from PIL import Image")
        #expect(WorkingIndicator.middlePaths("ls ~/a/b/c/file.swift", room: 14) == "ls ~/…/file.swift")
        #expect(WorkingIndicator.middlePaths("/data/work/another-agent/home/SOUL.md", room: 15).hasSuffix("/SOUL.md"))
        #expect(WorkingIndicator.middlePaths("cargo check", room: 1) == "cargo check")
        #expect(WorkingIndicator.middlePaths("~/README.md", room: 100) == "~/README.md")
    }

    @Test func bounceKeepsTheWebsRestPeakAndStaggerAcrossCycles() {
        for dot in 0..<3 {
            let delay = Double(dot) * 0.13
            #expect(WorkingIndicator.offset(elapsed: delay, dot: dot) == 0)
            for cycle in 0..<5 {
                let start = delay + Double(cycle) * 1.15
                #expect(abs(WorkingIndicator.offset(elapsed: start + 0.345, dot: dot) + 3) < 0.000001)
                #expect(abs(WorkingIndicator.offset(elapsed: start + 0.1725, dot: dot) + 1.5) < 0.000001)
                #expect(abs(WorkingIndicator.offset(elapsed: start + 0.5175, dot: dot) + 1.5) < 0.000001)
                #expect(abs(WorkingIndicator.offset(elapsed: start + 0.8, dot: dot)) < 0.000001)
            }
        }
        #expect(WorkingIndicator.offset(elapsed: -1, dot: 0) == 0)
    }

    @Test func colorsUseTheWebsIdentityAndUTF16RatherThanTheDisplayName() {
        #expect(WorkingIndicator.colorRGB(nil) == 0xa88bea)
        #expect(WorkingIndicator.colorRGB("") == 0xa88bea)
        #expect(WorkingIndicator.colorRGB("npub1preview") == 0x62c8af)
        #expect(WorkingIndicator.colorRGB("agent") == 0xa88bea)
        // An astral character is two UTF-16 units, as in agent-color.js.
        #expect(WorkingIndicator.colorRGB("🙂") == 0xdc91b6)
    }

    @Test func onlyRunningTurnsShowAndNamesFollowTheirOwnAgent() {
        let library = Library()
        library.fragments = [.init(name: "moss", kind: "agent", title: "Moss")]
        library.identities = ["moss": "npub1moss"]
        var timeline = Timeline()
        func work(_ seq: Int64, _ agent: String, _ turn: String, _ kind: String) -> ChannelRecord {
            .init(channel: "work", seq: seq, at: seq, principal: agent, kind: "message",
                  body: .object(["kind": .string(kind), "turn": .string(turn)]))
        }
        timeline.insert(work(1, "npub1moss", "old", "turn.start"))
        timeline.insert(work(2, "npub1moss", "old", "turn.end"))
        timeline.insert(work(3, "npub1unknown", "new", "turn.start"))
        timeline.insert(work(4, "npub1moss", "new", "turn.start"))
        let rows = WorkingIndicator(timeline: timeline, library: library).agents
        #expect(rows.map(\.name) == ["The agent", "Moss"])
        #expect(rows.map(\.id.agent) == ["npub1unknown", "npub1moss"])
        #expect(rows.allSatisfy { $0.id.id == "new" })
    }
}
