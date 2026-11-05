import Foundation
import Testing
@testable import Fragment

/// A chat's HTTP and live socket against a stub of the platform's answers
/// (docs/api.md: channels, `__live`; docs/chat-records.md): the faults a
/// phone meets, a lost acknowledgement, a refusal and a dropped stream.
@MainActor
struct TransportTests {
    private let chat = FragmentSummary(name: "chat--abc123", kind: "chat")
    private var posts: String { "POST /api/f/\(chat.name)/channels/chat" }
    private let hello = #"{"type":"hello","id":"s1","principal":"npub1me","role":"editor","platform":"https://finite.test","presence":[]}"#

    /// A record as a `__live` frame.
    private func frame(_ channel: String, _ seq: Int64, body: String) -> String {
        #"{"type":"record",\#(record(channel, seq, body: body).dropFirst())"#
    }

    @Test func aLostAcknowledgementIsRetriedWithTheSameIDAndBody() async throws {
        let route = posts
        let stub = Stub { seen, before in
            guard seen.route == route else { return (404, #"{"error":"not_found","message":"no route"}"#) }
            // the first try's append committed, and its answer was lost
            if !before.contains(where: { $0.route == route }) { return (503, #"{"error":"unavailable","message":"Try again shortly."}"#) }
            return posted(seen, seq: 7, replayed: true)
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        session.composer = "Hello there"
        await session.send()
        let failed = try #require(session.pending.first)
        #expect(failed.failed && session.composer.isEmpty && session.error == "Try again shortly.")
        await session.retry(failed)
        #expect(session.pending.isEmpty)
        #expect(session.timeline.records.map(\.seq) == [7])
        let sent = stub.seen.filter { $0.route == route }
        #expect(sent.count == 2)
        #expect(sent[0].json == sent[1].json && sent[0].json["id"].string == failed.id)
        #expect(sent[0].json["body"]["client_id"].string == failed.id && sent[0].json["body"]["text"].string == "Hello there")
        #expect(stub.seen.allSatisfy { $0.authorization?.hasPrefix("Nostr ") == true })
    }

    @Test func aRefusedPostLeavesTheOutboxAndItsWordsReturnToTheComposer() async throws {
        let route = posts
        let stub = Stub { seen, _ in
            seen.route == route ? (409, #"{"error":"conflicting_body","message":"this post id was already used with a different body"}"#) : (404, "{}")
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        session.composer = "Words to keep"
        await session.send()
        #expect(session.pending.isEmpty)
        #expect(session.composer == "Words to keep")
        #expect(session.error == "this post id was already used with a different body")
        #expect(stub.seen.filter { $0.route == route }.count == 1)
    }

    @Test func aRateLimitedPostStaysInTheOutboxToRetry() async throws {
        let route = posts
        let stub = Stub { seen, _ in seen.route == route ? (429, #"{"error":"rate_limited","message":"Slow down."}"#) : (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        session.composer = "Again later"
        await session.send()
        #expect(session.pending.first?.failed == true && session.composer.isEmpty)
    }

    @Test func aDroppedStreamCatchesUpFromItsCursorWithoutDuplicates() async throws {
        let stub = Stub { _, _ in (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("the first socket") { stub.sockets.count == 1 }
        let first = stub.sockets[0]
        #expect(first.request.url?.absoluteString == "wss://\(stub.host)/f/\(chat.name)/__live")
        #expect(first.request.value(forHTTPHeaderField: "Authorization")?.hasPrefix("Nostr ") == true)
        first.push(hello)
        try await waitUntil("its subscriptions") { first.sent.count == 2 }
        #expect(first.sent.map { $0["channel"].string } == ["chat", "work"])
        // the chat's last messages, and a page of work (a long turn's steps, and its start)
        #expect(first.sent.allSatisfy { $0["type"].string == "subscribe" && $0["after"] == .null })
        #expect(first.sent.map { $0["last"] } == [.number(50), .number(1000)])
        for seq: Int64 in [1, 2] { first.push(frame("chat", seq, body: #"{"text":"m\#(seq)"}"#)) }
        first.push(#"{"type":"subscribed","channel":"chat","next":2,"more":false}"#)
        first.push(#"{"type":"subscribed","channel":"work","next":0,"more":false}"#)
        first.push(#"{"type":"draft","channel":"chat","principal":"npub1agent","turn":"t1","text":"Hal","at":2500}"#)
        try await waitUntil("caught up, a reply being written") { session.caughtUp && session.timeline.drafts["t1"]?.text == "Hal" }

        first.drop()
        try await waitUntil("the socket dropped") { !session.connected }
        #expect(session.timeline.drafts.isEmpty)
        try await waitUntil("the second socket") { stub.sockets.count == 2 }
        let second = stub.sockets[1]
        second.push(hello)
        try await waitUntil("its subscriptions") { second.sent.count == 2 }
        #expect(second.sent[0]["channel"].string == "chat" && second.sent[0]["after"] == .number(2) && second.sent[0]["last"] == .null)
        // the page from the cursor may repeat what was seen: it is kept once
        second.push(frame("chat", 2, body: #"{"text":"m2"}"#))
        second.push(frame("chat", 3, body: #"{"text":"Hallo","turn":"t1"}"#))
        second.push(#"{"type":"subscribed","channel":"chat","next":3,"more":false}"#)
        try await waitUntil("caught up again") { session.caughtUp && session.timeline.records.count == 3 }
        #expect(session.timeline.records.map(\.seq) == [1, 2, 3])
        #expect(session.timeline.cursors["chat"] == 3)
        #expect(session.timeline.drafts.isEmpty)
    }

    @Test func stopPostsAStopForEachActiveTurn() async throws {
        let route = posts
        let stub = Stub { seen, _ in seen.route == route ? posted(seen, seq: 9, replayed: false) : (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        socket.push(frame("work", 4, body: #"{"kind":"turn.start","turn":"t4"}"#))
        try await waitUntil("a turn running") { session.timeline.activeTurns == ["t4"] }
        await session.stop()
        let stop = try #require(stub.seen.first { $0.route == route })
        #expect(stop.json == .object(["id": .string("stop:t4"), "body": .object(["kind": .string("stop"), "turn": .string("t4")])]))
    }

    /// A turn begun before the backlog (a start, then more steps than one
    /// page holds) still runs, and Stop stops it, opening the chat and
    /// opening it again: its start is read back from earlier work.
    @Test func aBusyTurnKeepsItsStopPastTheBacklog() async throws {
        let route = posts
        // work 1 to 1100: turn t9's start, then its steps; the backlog is the
        // last 1000, and the 100 before it are large ones (about 60 KB each,
        // within a record's 64 KiB): a page of them is 3 MB
        let large = String(repeating: "x", count: 60_000)
        let work: @Sendable (Int64) -> String = { seq in
            record("work", seq, body: seq == 1 ? #"{"kind":"turn.start","turn":"t9"}"# : #"{"kind":"turn.step","turn":"t9","text":"\#(seq > 100 ? "step" : large)"}"#)
        }
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/f/chat--abc123/channels/work":
                let before = Int64(seen.query["before"] ?? "") ?? 0, limit = Int64(seen.query["limit"] ?? "") ?? 0
                let page = (max(1, before - limit)..<before).map(work).joined(separator: ",")
                return (200, #"{"channel":"work","records":[\#(page)],"next":null}"#)
            case route: return posted(seen, seq: 2, replayed: false)
            default: return (404, "{}")
            }
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        for visit in 0..<2 {
            let session = ChatSession(fragment: chat, api: api)
            let run = Task { await session.run() }
            try await waitUntil("a socket") { stub.sockets.count == visit + 1 }
            let socket = stub.sockets[visit]
            socket.push(hello)
            try await waitUntil("its subscriptions") { socket.sent.count == 2 }
            #expect(socket.sent[1]["channel"].string == "work" && socket.sent[1]["last"] == .number(1000))
            for seq: Int64 in 101...1100 { socket.push(#"{"type":"record",\#(work(seq).dropFirst())"#) }
            socket.push(#"{"type":"subscribed","channel":"work","next":1100,"more":false}"#)
            socket.push(#"{"type":"subscribed","channel":"chat","next":0,"more":false}"#)
            try await waitUntil("the turn running") { session.timeline.activeTurns == ["t9"] }
            // read back in batches a response holds, until its start
            let asked = stub.seen.filter { $0.route == "GET /api/f/chat--abc123/channels/work" }.suffix(2).map(\.query)
            #expect(asked == [["before": "101", "limit": "50"], ["before": "51", "limit": "50"]])
            #expect(session.error == nil)
            await session.stop()
            #expect(stub.seen.last { $0.route == route }?.json["id"].string == "stop:t9")
            run.cancel()
            await run.value
        }
    }

    @Test func earlierMessagesAreAPageBeforeTheOldestAndNeverMoveTheCursor() async throws {
        let stub = Stub { seen, _ in
            guard seen.route == "GET /api/f/chat--abc123/channels/chat" else { return (404, "{}") }
            let before = Int64(seen.query["before"] ?? "") ?? 0
            let page = (1..<before).map { record("chat", $0, body: #"{"text":"old"}"#) }.joined(separator: ",")
            return (200, #"{"channel":"chat","records":[\#(page)],"next":null}"#)
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        for seq: Int64 in [31, 32] { socket.push(frame("chat", seq, body: #"{"text":"new"}"#)) }
        socket.push(#"{"type":"subscribed","channel":"chat","next":32,"more":false}"#)
        try await waitUntil("caught up") { session.caughtUp }
        await session.loadEarlier()
        let asked = try #require(stub.seen.first { $0.method == "GET" })
        #expect(asked.query == ["before": "31", "limit": "50"])
        #expect(session.timeline.records.count == 32)
        #expect(session.timeline.cursors["chat"] == 32)
        #expect(!session.hasEarlier)
    }
}

/// The answer to a post: the record it appended, its body the post's.
private func posted(_ seen: Seen, seq: Int64, replayed: Bool) -> (Int, String) {
    let body = String(decoding: (try? JSONEncoder().encode(seen.json["body"])) ?? Data("null".utf8), as: UTF8.self)
    return (200, #"{"record":\#(record("chat", seq, principal: "npub1me", body: body)),"replayed":\#(replayed)}"#)
}
