import Foundation
import SwiftUI
import Synchronization
import Testing
import UIKit
@testable import Fragment

/// The chat's timeline laid out for real (FIN-419): `ChatView` in a window of
/// the test host, its chat played by a `ScriptedSocket` as a turn streams. A
/// layout that never settles holds the main thread inside one render, where
/// no test can see it, so a watchdog thread ends the process saying where.
@MainActor
struct TimelineLayoutTests {
    private let chat = FragmentSummary(name: "chat--layout", kind: "chat")
    private let hello = #"{"type":"hello","id":"s1","principal":"npub1me","role":"editor","platform":"https://finite.test","presence":[]}"#

    /// A chat of one message, then two turns: each reply streamed and said, a
    /// tool called, and the rest of the reply. Scrolled by hand once, and followed
    /// at its end, the timeline settles each time and never moves sideways (a
    /// row held in place since the scroll fought the follow, and moved it 20
    /// points).
    @Test(.timeLimit(.minutes(1))) func aShortChatFollowsATurnWithoutLoopingOrMovingSideways() async throws {
        let watchdog = LayoutWatchdog(limit: 5)
        defer { watchdog.stop() }
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/f/chat--layout/channels/chat": (200, #"{"channel":"chat","records":[],"next":null}"#)
            case "GET /api/f/chat--layout/channels/work": (200, #"{"channel":"work","records":[],"next":null}"#)
            default: (200, "{}")
            }
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let scene = try #require(UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first)
        let window = UIWindow(windowScene: scene)
        // as the app shows a chat: under its title, in a navigation stack
        let host = UIHostingController(rootView: NavigationStack {
            ChatView(session: session, library: Library(), open: { _ in })
                .navigationTitle("A chat").navigationBarTitleDisplayMode(.inline)
        }.environment(\.scenePhase, .active))
        window.rootViewController = host
        window.makeKeyAndVisible()
        defer { window.isHidden = true }

        // each record a second after the one before, as a chat's are
        var clock = Int64(Date().timeIntervalSince1970 * 1000) - 60_000
        func frame(_ channel: String, _ seq: Int64, principal: String = "npub1agent", body: [String: Any]) -> String {
            clock += 1000
            return #"{"type":"record","channel":"\#(channel)","seq":\#(seq),"at":\#(clock),"principal":"\#(principal)","kind":"message","body":\#(Self.json(body))}"#
        }
        func draft(_ turn: String, _ text: String) -> String {
            #"{"type":"draft","channel":"chat","principal":"npub1agent","turn":"\#(turn)","text":\#(Self.json(text)),"at":\#(clock + 500)}"#
        }
        var sideways: CGFloat = 0
        func pace(_ step: String, _ milliseconds: Int) async throws {
            watchdog.at(step)
            try await Task.sleep(for: .milliseconds(milliseconds))
            if let timeline = Self.timeline(in: host.view) { sideways = max(sideways, abs(timeline.contentOffset.x)) }
        }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        try await waitUntil("its subscriptions") { socket.sent.count == 2 }
        socket.push(frame("chat", 1, principal: "npub1me", body: ["text": "Plan my week"]))
        socket.push(#"{"type":"subscribed","channel":"chat","next":1,"more":false}"#)
        socket.push(#"{"type":"subscribed","channel":"work","next":0,"more":false}"#)
        try await waitUntil("read") { session.read }
        try await pace("read", 1000)

        // two turns, each asked by a message of the person's: four records
        // on `chat` and four on `work` a turn
        for turn in 1...2 {
            let (asked, said, work) = (Int64(turn * 3 - 1), Int64(turn * 3), Int64(turn * 4 - 3))
            let id = "t\(turn)"
            socket.push(frame("chat", asked, principal: "npub1me", body: ["text": "Use a tool for it"]))
            socket.push(frame("work", work, body: ["kind": "turn.start", "turn": id, "asker": "npub1me", "cause": ["channel": "chat", "seq": asked]]))
            try await pace("turn \(turn) begun", 800)
            var text = ""
            for n in 0..<25 {
                text += Self.words(n)
                socket.push(draft(id, text))
                try await pace("turn \(turn)'s reply, words \(n)", 60)
                // the person scrolls by hand as the reply starts, and the chat
                // stays at its end: it follows on, a row now held in place
                if n == 1 { try await Self.drag(try #require(Self.timeline(in: host.view)), by: -0.65) }
            }
            // its words said as a reply of their own, and a tool called
            socket.push(frame("work", work + 1, body: ["kind": "turn.call", "turn": id, "tool": "search", "category": "web", "args": "a week", "tool_call_id": "c\(turn)", "text": text]))
            socket.push(frame("chat", said, body: ["text": text, "turn": id]))
            try await pace("turn \(turn)'s tool called", 700)
            socket.push(frame("work", work + 2, body: ["kind": "turn.step", "turn": id, "tool": "search", "category": "web", "args": "a week", "tool_call_id": "c\(turn)", "ok": true]))
            text = ""
            for n in 0..<20 {
                text += Self.words(n + 25)
                socket.push(draft(id, text))
                try await pace("turn \(turn)'s reply, the rest \(n)", 60)
            }
            socket.push(frame("chat", said + 1, body: ["text": text, "turn": id]))
            socket.push(frame("work", work + 3, body: ["kind": "turn.end", "turn": id, "outcome": "completed"]))
            try await pace("turn \(turn) ended", 1000)
        }
        #expect(session.rows.map(\.id) == ["chat:1", "chat:2", "chat:3", "work:3", "chat:4", "chat:5", "chat:6", "work:7", "chat:7"])
        #expect(sideways == 0, "the timeline keeps its margins")
    }

    private static func json(_ value: Any) -> String {
        String(decoding: try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .fragmentsAllowed]), as: UTF8.self)
    }
    /// Eight words of a reply, a new paragraph every third time.
    private static func words(_ n: Int) -> String {
        let pool = ["the", "small", "river", "moves", "gently", "past", "the", "old", "stone", "bridge", "while", "morning", "light", "follows"]
        return (n % 3 == 2 ? "\n\n" : " ") + (0..<8).map { pool[($0 * 5 + n * 3) % pool.count] }.joined(separator: " ")
    }
    /// A drag by hand of `fraction` of the timeline's height (up, below
    /// zero), as its scroll view's delegate hears one: begun, moved frame by
    /// frame, let go. SwiftUI's scroll phases, and the row it holds, follow
    /// the delegate.
    private static func drag(_ timeline: UIScrollView, by fraction: CGFloat) async throws {
        let end = max(0, timeline.contentSize.height - timeline.bounds.height + timeline.adjustedContentInset.bottom)
        timeline.delegate?.scrollViewWillBeginDragging?(timeline)
        for _ in 0..<12 {
            timeline.contentOffset.y = min(max(0, timeline.contentOffset.y + timeline.bounds.height * fraction / 12), end)
            try await Task.sleep(for: .milliseconds(16))
        }
        var target = timeline.contentOffset
        timeline.delegate?.scrollViewWillEndDragging?(timeline, withVelocity: .zero, targetContentOffset: &target)
        timeline.delegate?.scrollViewDidEndDragging?(timeline, willDecelerate: false)
        timeline.delegate?.scrollViewDidEndDecelerating?(timeline)
    }
    /// The timeline's scroll view: the tallest in the window.
    private static func timeline(in view: UIView) -> UIScrollView? {
        var tallest = view as? UIScrollView
        for sub in view.subviews {
            if let found = timeline(in: sub), found.contentSize.height > (tallest?.contentSize.height ?? -1) { tallest = found }
        }
        return tallest
    }
}

/// Ends the process when the main thread answers nothing for `limit`
/// seconds, saying the step it was at: a render that never ends can't be
/// failed, or left, from the main thread.
final class LayoutWatchdog: Sendable {
    private let state = Mutex<(answered: ContinuousClock.Instant, step: String, stopped: Bool)>((ContinuousClock.now, "start", false))
    init(limit: Double) {
        Thread.detachNewThread { [self] in
            while true {
                let (answered, step, stopped) = state.withLock { ($0.answered, $0.step, $0.stopped) }
                if stopped { return }
                if ContinuousClock.now - answered > .seconds(limit) {
                    fatalError("The chat's layout never settled, at \(step): the main thread held for \(limit) s (FIN-419)")
                }
                DispatchQueue.main.async { [self] in state.withLock { $0.answered = ContinuousClock.now } }
                Thread.sleep(forTimeInterval: 0.25)
            }
        }
    }
    func at(_ step: String) { state.withLock { $0.step = step } }
    func stop() { state.withLock { $0.stopped = true } }
}
