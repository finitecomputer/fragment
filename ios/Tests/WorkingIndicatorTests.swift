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
