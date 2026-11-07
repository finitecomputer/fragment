import Foundation
import Observation
import Synchronization
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
        #expect(session.timeline.drafts["t1"]?.text == "Hal" && session.error == nil, "a reply being written stays, and nothing is said")
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

    /// Until both its channels are first read to their end a chat is
    /// loading, never an empty chat nor its backlog streaming in; it shows
    /// its messages and the work it tells of, and a reply's words as they
    /// come never redraw the chat's rows.
    @Test func aChatLoadsUntilBothChannelsAreReadAndADraftRedrawsOnlyItself() async throws {
        let stub = Stub { _, _ in (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("the socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        try await waitUntil("its subscriptions") { socket.sent.count == 2 }
        socket.push(frame("chat", 1, body: #"{"text":"Hello"}"#))
        socket.push(#"{"type":"subscribed","channel":"chat","next":1,"more":false}"#)
        try await waitUntil("its chat read") { session.caughtUp }
        #expect(!session.read, "its work is still to come: loading, not shown in part")
        // the work backlog comes after the chat's, and shows records of its own
        socket.push(frame("work", 1, body: #"{"kind":"turn.start","turn":"t1"}"#))
        socket.push(frame("work", 2, body: #"{"kind":"turn.step","turn":"t1","tool":"search"}"#))
        try await waitUntil("its work backlog") { session.timeline.records.count == 3 }
        #expect(!session.read && session.activeTurns.isEmpty, "until its work is read, no turn is said to run")
        socket.push(#"{"type":"subscribed","channel":"work","next":2,"more":false}"#)
        try await waitUntil("read") { session.read }
        #expect(session.rows.map(\.id) == ["chat:1", "work:2"], "a turn's start is not shown")
        #expect(session.activeTurns == ["t1"])

        // a reply begun takes its place, after its turn's latest record
        socket.push(#"{"type":"draft","channel":"chat","principal":"npub1agent","turn":"t1","text":"Hal","at":2500}"#)
        try await waitUntil("a reply begun") { session.drafts.map(\.text) == ["Hal"] }
        #expect(session.rows.map(\.id) == ["chat:1", "work:2", "d:npub1agent t1"])
        let told = Mutex(false)
        withObservationTracking { _ = session.rows } onChange: { told.withLock { $0 = true } }
        socket.push(#"{"type":"draft","channel":"chat","principal":"npub1agent","turn":"t1","text":"Hallo","at":2600}"#)
        try await waitUntil("the reply's words") { session.drafts.map(\.text) == ["Hallo"] }
        #expect(!told.withLock { $0 }, "a draft's words never redraw the rows")
    }

    /// Which turns run is said again by each connection, once its work is
    /// read: while none is open no turn runs, and a catch-up that replays a
    /// turn's start before its end never shows it running meanwhile. What
    /// was shown stays shown.
    @Test func eachConnectionReadsItsWorkBeforeATurnRuns() async throws {
        let stub = Stub { _, _ in (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("the socket") { stub.sockets.count == 1 }
        let first = stub.sockets[0]
        first.push(hello)
        try await waitUntil("its subscriptions") { first.sent.count == 2 }
        first.push(frame("chat", 1, body: #"{"text":"Hello"}"#))
        first.push(frame("work", 1, body: #"{"kind":"turn.start","turn":"t1"}"#))
        first.push(#"{"type":"subscribed","channel":"chat","next":1,"more":false}"#)
        first.push(#"{"type":"subscribed","channel":"work","next":1,"more":false}"#)
        try await waitUntil("read, a turn running") { session.read && session.activeTurns == ["t1"] }

        first.drop()
        try await waitUntil("the socket dropped") { !session.connected }
        #expect(session.activeTurns.isEmpty && !session.workRead, "none heard, none said to run")
        #expect(session.read && session.rows.map(\.id) == ["chat:1"], "what was shown stays shown")

        try await waitUntil("the second socket") { stub.sockets.count == 2 }
        let second = stub.sockets[1]
        second.push(hello)
        try await waitUntil("its subscriptions") { second.sent.count == 2 }
        #expect(second.sent[1]["channel"].string == "work" && second.sent[1]["after"] == .number(1))
        // meanwhile t1 ended and another turn began and ended: its start is replayed first
        second.push(frame("work", 2, body: #"{"kind":"turn.end","turn":"t1"}"#))
        second.push(frame("work", 3, body: #"{"kind":"turn.start","turn":"t2"}"#))
        try await waitUntil("the replay's start") { session.timeline.records.contains { $0.id == "work:3" } }
        #expect(session.activeTurns.isEmpty, "a replayed start is not a running turn")
        second.push(frame("work", 4, body: #"{"kind":"turn.end","turn":"t2"}"#))
        second.push(#"{"type":"subscribed","channel":"work","next":4,"more":false}"#)
        second.push(#"{"type":"subscribed","channel":"chat","next":1,"more":false}"#)
        try await waitUntil("its work read again") { session.workRead && session.caughtUp }
        #expect(session.activeTurns.isEmpty)
    }

    /// A socket closed as its object sleeps (4001) is opened again at once,
    /// with nothing said, the composer open (sends go over HTTP) and the
    /// reply being written kept; "Reconnecting…" only once it is not back
    /// soon, and gone once it is. Only a repeated failure waits.
    @Test func aSocketClosedAsItsObjectSleepsReconnectsQuietly() async throws {
        let stub = Stub { _, _ in (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        session.reconnectNoticeAfter = .milliseconds(300)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let first = stub.sockets[0]
        first.push(hello)
        try await waitUntil("its subscriptions") { first.sent.count == 2 }
        first.push(frame("chat", 1, body: #"{"text":"Hi"}"#))
        first.push(frame("work", 1, body: #"{"kind":"turn.start","turn":"t1"}"#))
        first.push(#"{"type":"subscribed","channel":"chat","next":1,"more":false}"#)
        first.push(#"{"type":"subscribed","channel":"work","next":1,"more":false}"#)
        first.push(#"{"type":"draft","channel":"chat","principal":"npub1agent","turn":"t1","text":"Half a reply","at":2500}"#)
        try await waitUntil("a reply being written") { session.read && session.drafts.map(\.text) == ["Half a reply"] }

        let clock = ContinuousClock(), closed = clock.now
        first.close(URLSessionWebSocketTask.CloseCode(rawValue: 4001)!)
        try await waitUntil("opened again") { stub.sockets.count == 2 }
        #expect(clock.now - closed < .seconds(1), "at once, no wait")
        #expect(session.error == nil && session.read && session.drafts.map(\.text) == ["Half a reply"], "nothing said, the composer open, the draft kept")
        let second = stub.sockets[1]
        second.push(hello)
        try await waitUntil("its subscriptions") { second.sent.count == 2 }
        second.push(#"{"type":"subscribed","channel":"chat","next":1,"more":false}"#)
        second.push(#"{"type":"subscribed","channel":"work","next":1,"more":false}"#)
        try await waitUntil("caught up again") { session.caughtUp && session.workRead }
        #expect(session.drafts.map(\.text) == ["Half a reply"] && session.error == nil, "its turn still runs: its draft stays")
        try await Task.sleep(for: .milliseconds(500))
        #expect(session.error == nil, "back in time: never said")

        // failing again and again: the second waits, and past the notice's time it is said
        second.drop()
        try await waitUntil("opened again") { stub.sockets.count == 3 }
        stub.sockets[2].drop()
        try await waitUntil("said") { session.error == ChatSession.reconnecting }
        #expect(stub.sockets.count == 3, "a repeated failure backs off")
        try await waitUntil("opened again, after a while", timeout: .seconds(8)) { stub.sockets.count == 4 }
        stub.sockets[3].push(hello)
        try await waitUntil("unsaid once back") { session.error == nil && session.connected }
    }

    /// A chat deleted (4004, or an upgrade answered 404) or one whose access
    /// changed (4003, 403) is closed for good: said as the web's chat says
    /// it, never reconnected, and a reply being written goes with it.
    @Test func aChatClosedForGoodIsNeverReconnected() async throws {
        for (end, said) in [("4004", ChatSession.deleted), ("4003", ChatSession.accessChanged), ("404", ChatSession.deleted), ("403", ChatSession.accessChanged)] {
            let stub = Stub { _, _ in (404, "{}") }
            let api = try stub.api()
            defer { try? ChatDraftStore.removeAccount(api: api) }
            let session = ChatSession(fragment: chat, api: api)
            let run = Task { await session.run() }
            try await waitUntil("a socket") { stub.sockets.count == 1 }
            let first = stub.sockets[0]
            if end.hasPrefix("40") && end.count == 3 {
                // its upgrade refused, after a connection that had been open
                first.push(hello)
                try await waitUntil("its subscriptions") { first.sent.count == 2 }
                first.close(URLSessionWebSocketTask.CloseCode(rawValue: 4001)!)
                try await waitUntil("opened again") { stub.sockets.count == 2 }
                stub.sockets[1].refuse(Int(end)!)
            } else {
                first.push(hello)
                try await waitUntil("its subscriptions") { first.sent.count == 2 }
                first.push(frame("work", 1, body: #"{"kind":"turn.start","turn":"t1"}"#))
                first.push(#"{"type":"subscribed","channel":"chat","next":0,"more":false}"#)
                first.push(#"{"type":"subscribed","channel":"work","next":1,"more":false}"#)
                first.push(#"{"type":"draft","channel":"chat","principal":"npub1agent","turn":"t1","text":"Half","at":2500}"#)
                try await waitUntil("a reply being written") { session.drafts.count == 1 }
                first.close(URLSessionWebSocketTask.CloseCode(rawValue: Int(end)!)!)
            }
            await run.value
            let sockets = stub.sockets.count
            #expect(session.error == said, "\(end): \(session.error ?? "nothing said")")
            #expect(session.drafts.isEmpty && !session.connected, "\(end): nothing left writing")
            try await Task.sleep(for: .milliseconds(300))
            #expect(stub.sockets.count == sockets, "\(end): no socket after")
            // back to the app (its view's task runs it again): still closed, no socket
            await session.run()
            #expect(stub.sockets.count == sockets && session.error == said && session.closed == said, "\(end): a return to the app opens nothing")

            // left and opened again: a new session asks anew
            let library = Library()
            library.sessions[chat.name] = session
            library.leftChat(chat.name)
            #expect(library.sessions[chat.name] == nil, "\(end): a closed session goes once the chat is left")
        }
        // one still open stays across leaving (its drafts and cursors)
        let open = ChatSession(fragment: chat, api: try Stub { _, _ in (404, "{}") }.api())
        let library = Library()
        library.sessions[chat.name] = open
        library.leftChat(chat.name)
        #expect(library.sessions[chat.name] === open)
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

    /// Steer, as the web's composer offers it (chat.js `refreshSend`): to
    /// the owner of an agent whose menu has `steer`, while it works, for
    /// words alone; it sends `/steer <words>` as a command of that menu,
    /// and the words stay until it is sent.
    @Test func steerIsOfferedByTheAgentsMenuAndSendsItsCommand() async throws {
        let route = posts
        let stub = Stub { seen, _ in seen.route == route ? posted(seen, seq: 9, replayed: false) : (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: FragmentSummary(name: chat.name, kind: "chat", agents: ["npub1agent"]), api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        socket.push(frame("work", 1, body: #"{"kind":"commands","agent":"npub1agent","commands":[{"name":"usage","description":"Say"},{"name":"steer","description":"Tell it","args":"What"}]}"#))
        socket.push(frame("work", 2, body: #"{"kind":"turn.start","turn":"t2","asker":"npub1me","cause":{"channel":"chat","seq":1}}"#))
        socket.push(#"{"type":"subscribed","channel":"work","next":2,"more":false}"#)
        socket.push(#"{"type":"subscribed","channel":"chat","next":0,"more":false}"#)
        try await waitUntil("the turn running") { session.activeTurns == ["t2"] && session.workRead }
        session.composer = "  use the blue one "
        #expect(!session.steers(recording: false), "an agent not theirs offers them no menu")
        session.owned = ["npub1agent"]
        #expect(session.myTurn == TurnKey(agent: "npub1agent", id: "t2"))
        #expect(session.steers(recording: false))
        #expect(!session.steers(recording: true), "a voice memo being recorded goes with a message alone")
        #expect(session.placeholder(name: { _ in "Juniper" }) == "A message waits its turn")
        session.attachments = [ChatAttachment(sha256: "x", name: "a.txt", type: "text/plain", size: 1)]
        #expect(!session.steers(recording: false), "files go with a message alone")
        session.attachments = []
        session.composer = "/usage"
        #expect(!session.steers(recording: false), "a command typed is no steer")
        session.composer = "  use the blue one "
        await session.steer()
        let sent = try #require(stub.seen.first { $0.route == route })
        #expect(sent.json["body"] == .object(["kind": .string("command"), "command": .string("steer"), "args": .string("use the blue one"), "to": .array([.string("npub1agent")])]))
        #expect(session.composer.isEmpty && session.error == nil)
        // shown as typed, a person's command (chat.js `userMessage`)
        #expect(session.rows.contains { $0.record?.command == "steer" })
    }

    /// A menu without `steer` (another runtime's) offers none.
    @Test func noSteerWhereTheRuntimeOffersNone() async throws {
        let stub = Stub { _, _ in (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: FragmentSummary(name: chat.name, kind: "chat", agents: ["npub1agent"]), api: api)
        session.owned = ["npub1agent"]
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        socket.push(frame("work", 1, body: #"{"kind":"commands","agent":"npub1agent","commands":[{"name":"usage","description":"Say"}]}"#))
        socket.push(frame("work", 2, body: #"{"kind":"turn.start","turn":"t2","asker":"npub1me"}"#))
        socket.push(#"{"type":"subscribed","channel":"work","next":2,"more":false}"#)
        try await waitUntil("the turn running") { session.activeTurns == ["t2"] && session.workRead }
        session.composer = "use the blue one"
        #expect(!session.steers(recording: false))
        #expect(session.placeholder(name: { _ in "Juniper" }) == "A message waits its turn")
    }

    /// A turn asking its asker something (`turn.asked`): the composer is the
    /// answer, and what it sends is the web's answer, a message of theirs
    /// to the chat (docs/chat-records.md: "The asker's next message to that
    /// agent is then the answer"), no command, no card's answer. Once said,
    /// it waits on no one.
    @Test func anAnswerToATurnsQuestionIsTheirNextMessage() async throws {
        let route = posts
        let stub = Stub { seen, _ in seen.route == route ? posted(seen, seq: 3, replayed: false) : (404, "{}") }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: FragmentSummary(name: chat.name, kind: "chat", agents: ["npub1agent"]), api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        socket.push(#"{"type":"record",\#(record("chat", 1, principal: "npub1me", body: #"{"text":"ask-me"}"#).dropFirst())"#)
        socket.push(frame("work", 1, body: #"{"kind":"turn.start","turn":"t1","asker":"npub1me","cause":{"channel":"chat","seq":1}}"#))
        socket.push(frame("chat", 2, body: #"{"text":"What should I call it?","turn":"t1"}"#))
        socket.push(frame("work", 3, body: #"{"kind":"turn.asked","turn":"t1","asks":"npub1me"}"#))
        socket.push(#"{"type":"subscribed","channel":"work","next":3,"more":false}"#)
        socket.push(#"{"type":"subscribed","channel":"chat","next":2,"more":false}"#)
        try await waitUntil("the question asked") { session.workRead && session.answering() != nil }
        #expect(session.answering() == TurnKey(agent: "npub1agent", id: "t1"))
        #expect(session.placeholder(name: { _ in "Juniper" }) == "Your answer for Juniper")
        // its line says so, after the question
        let ids = session.rows.map(\.id)
        #expect(ids.suffix(2) == ["chat:2", "q:npub1agent t1"])
        session.composer = "Basil"
        await session.send()
        let sent = try #require(stub.seen.first { $0.route == route })
        let body = sent.json["body"]
        #expect(body["text"] == .string("Basil") && body["kind"] == .null && body["to"] == .null && body["command"] == .null)
        // the platform's record of it (`posted`: npub1me's, at 3000) ends the wait
        try await waitUntil("answered") { session.answering() == nil }
        #expect(!session.rows.contains { $0.id == "q:npub1agent t1" })
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

    /// Earlier work is read in batches that grow with how little their
    /// records take: a call or two, not twenty. What falls in the window
    /// (from its oldest message on) is kept, for its turns' steps; what is
    /// older only says which turns run. A batch that overflows the answer's
    /// bound is read again at the first batch's size.
    @Test func earlierWorkIsACallOrTwoAndKeepsWhatTheWindowShows() async throws {
        // work 1 to 2000: t7's start at 500, then its steps; 871 to 940 are
        // large (60 KB): a batch of them all is past the 4 MiB an answer holds
        let large = String(repeating: "x", count: 60_000)
        let work: @Sendable (Int64) -> String = { seq in
            record("work", seq, body: seq == 500 ? #"{"kind":"turn.start","turn":"t7"}"# : #"{"kind":"turn.step","turn":"t7","text":"\#((871...940).contains(seq) ? large : "s")"}"#)
        }
        let stub = Stub { seen, _ in
            guard seen.route == "GET /api/f/chat--abc123/channels/work" else { return (404, "{}") }
            let before = Int64(seen.query["before"] ?? "") ?? 0, limit = Int64(seen.query["limit"] ?? "") ?? 0
            let page = (max(1, before - limit)..<before).map(work).joined(separator: ",")
            return (200, #"{"channel":"work","records":[\#(page)],"next":null}"#)
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        try await waitUntil("its subscriptions") { socket.sent.count == 2 }
        // the window's oldest message is at work 960's time; the backlog is the last 1000
        socket.push(frame("chat", 960, body: #"{"text":"Look this up"}"#))
        for seq: Int64 in 1001...2000 { socket.push(#"{"type":"record",\#(work(seq).dropFirst())"#) }
        socket.push(#"{"type":"subscribed","channel":"chat","next":960,"more":false}"#)
        socket.push(#"{"type":"subscribed","channel":"work","next":2000,"more":false}"#)
        try await waitUntil("the turn running") { session.activeTurns == ["t7"] }
        let asked = stub.seen.filter { $0.route == "GET /api/f/chat--abc123/channels/work" }.map(\.query)
        #expect(Array(asked.prefix(3)) == [["before": "1001", "limit": "50"], ["before": "951", "limit": "950"], ["before": "951", "limit": "50"]],
                "a small batch grows; one past the bound is read again small")
        #expect(asked.count <= 6, "\(asked.count) calls")
        #expect(session.error == nil)
        #expect(session.timeline.records.contains { $0.id == "work:960" }, "its steps in the window are kept")
        #expect(!session.timeline.records.contains { $0.id == "work:500" }, "older work only says which turns run")
    }

    /// A completed turn straddling the backlog: its steps and end are held,
    /// its start older than both the backlog and the oldest message shown.
    /// Earlier work goes on past that message until the start, so its card
    /// (drawn from its start) shows.
    @Test func aCompletedTurnStraddlingTheBacklogGetsItsStart() async throws {
        let work: @Sendable (Int64) -> String = { seq in
            record("work", seq, body: seq == 50 ? #"{"kind":"turn.start","turn":"t"}"# : seq == 110 ? #"{"kind":"turn.end","turn":"t"}"# : #"{"kind":"turn.step","turn":"t","tool":"search"}"#)
        }
        let stub = Stub { seen, _ in
            guard seen.route == "GET /api/f/chat--abc123/channels/work" else { return (404, "{}") }
            let before = Int64(seen.query["before"] ?? "") ?? 0, limit = Int64(seen.query["limit"] ?? "") ?? 0
            let page = (max(1, before - limit)..<before).map(work).joined(separator: ",")
            return (200, #"{"channel":"work","records":[\#(page)],"next":null}"#)
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        try await waitUntil("its subscriptions") { socket.sent.count == 2 }
        // its intermediate reply is the oldest message shown, after its start
        socket.push(frame("chat", 95, body: #"{"text":"Still looking","turn":"t"}"#))
        for seq: Int64 in 101...110 { socket.push(#"{"type":"record",\#(work(seq).dropFirst())"#) }
        socket.push(#"{"type":"subscribed","channel":"chat","next":95,"more":false}"#)
        socket.push(#"{"type":"subscribed","channel":"work","next":110,"more":false}"#)
        try await waitUntil("its start read back") { session.timeline.turns[TurnKey(agent: "npub1agent", id: "t")]?.startAt != nil }
        #expect(session.activeTurns.isEmpty && session.error == nil)
        #expect(!session.activity.cards.isEmpty, "its steps show as a card")
    }

    /// A chat of 1,000 messages, opened on its newest 50: scrolled up, a
    /// page at a time before the oldest, never moving the live cursor; past
    /// the window's 600 its newest end goes. A live message then waits
    /// outside the window, counted; scrolled down, the window pages newer
    /// until it reaches the newest; Latest reads the newest page at once.
    /// Tool work k, between messages 990 + k and 991 + k.
    nonisolated private static func toolWork(_ k: Int64) -> String {
        #"{"channel":"work","seq":\#(k),"at":\#((990 + k) * 1000 + 500),"principal":"npub1agent","kind":"message","body":{"kind":"turn.step","turn":"t\#(k)","tool":"search"}}"#
    }

    @Test func aLongChatPagesBothWaysInAWindowOf600() async throws {
        let stub = Stub { seen, _ in
            let limit = Int64(seen.query["limit"] ?? "") ?? 0
            switch seen.route {
            case "GET /api/f/chat--abc123/channels/chat":
                let seqs: [Int64]
                if let before = seen.query["before"].flatMap({ Int64($0) }) { seqs = Array(max(1, before - limit)..<min(before, 1002)) }
                else { let after = Int64(seen.query["after"] ?? "") ?? 0; seqs = Array((after + 1)...min(after + limit, 1001)).filter { $0 <= 1001 } }
                let page = seqs.map { record("chat", $0, body: #"{"text":"m"}"#) }.joined(separator: ",")
                return (200, #"{"channel":"chat","records":[\#(page)],"next":null}"#)
            case "GET /api/f/chat--abc123/channels/work":
                // tool work only between messages 990 and 1000: work 1...10
                let seqs: [Int64]
                if let before = seen.query["before"].flatMap({ Int64($0) }) { seqs = Array(max(1, before - limit)..<min(before, 11)) }
                else { let after = Int64(seen.query["after"] ?? "") ?? 0; seqs = after >= 10 ? [] : Array((after + 1)...min(after + limit, 10)) }
                return (200, #"{"channel":"work","records":[\#(seqs.map(Self.toolWork).joined(separator: ","))],"next":null}"#)
            default: return (404, "{}")
            }
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        try await waitUntil("its subscriptions") { socket.sent.count == 2 }
        for seq: Int64 in 951...1000 { socket.push(frame("chat", seq, body: #"{"text":"m"}"#)) }
        for k: Int64 in 1...10 { socket.push(#"{"type":"record",\#(Self.toolWork(k).dropFirst())"#) }
        socket.push(#"{"type":"subscribed","channel":"chat","next":1000,"more":false}"#)
        socket.push(#"{"type":"subscribed","channel":"work","next":10,"more":false}"#)
        try await waitUntil("read") { session.read }
        #expect(session.hasOlder, "a full first page: there may be more")

        await session.loadOlder()
        let asked = stub.seen.filter { $0.route == "GET /api/f/chat--abc123/channels/chat" }.map(\.query)
        #expect(asked == [["before": "951", "limit": "50"]])
        #expect(session.rows.first?.id == "chat:901" && session.rows.last?.id == "chat:1000" && session.timeline.cursors["chat"] == 1000)
        for _ in 0..<10 { await session.loadOlder() }
        #expect(session.timeline.chatCount == 600 && !session.detached, "600 held, still the newest")
        await session.loadOlder()
        #expect(session.timeline.chatCount == 600 && session.detached, "past 600, the newest end goes")
        #expect(session.rows.first?.id == "chat:351" && session.rows.last?.id == "chat:950")
        #expect(!session.timeline.records.contains { $0.channel == "work" }, "a stretch with no tool work")

        // a live message while paged back: counted, not in the window
        let heard = session.heard
        socket.push(frame("chat", 1001, body: #"{"text":"new"}"#))
        try await waitUntil("heard") { session.heard == heard + 1 }
        #expect(session.rows.last?.id == "chat:950" && session.timeline.cursors["chat"] == 1001)

        // scrolled down: the page after the window's newest
        await session.loadNewer()
        #expect(stub.seen.last { $0.route == "GET /api/f/chat--abc123/channels/chat" }?.query == ["after": "950", "limit": "50"])
        #expect(session.rows.last?.id == "chat:1000" && session.detached && session.timeline.chatCount == 600)
        #expect(session.timeline.records.contains { $0.id == "work:5" }, "a newer page gets its tool steps though the window held none")
        await session.loadNewer()
        #expect(session.rows.last?.id == "chat:1001" && !session.detached, "the newest reached: the chat's again")

        // Latest from far back: the newest page at once
        for _ in 0..<13 { await session.loadOlder() }
        #expect(session.detached)
        await session.resetToLatest()
        #expect(stub.seen.last { $0.route == "GET /api/f/chat--abc123/channels/chat" }?.query == ["before": "1002", "limit": "50"])
        #expect(!session.detached && session.rows.first?.id == "chat:952" && session.rows.last?.id == "chat:1001")
    }

    /// Paged back to the chat's first message and forward again, the window
    /// pages back once more (its oldest end went); a stretch holding no work
    /// still gets its newer pages' tool steps (`workIndex`); and messages
    /// replayed on reconnect count on the pill as live ones.
    @Test func aWindowPagedBackComesBackWhole() async throws {
        // 300 messages; tool work only for the last 20 (work 1...20, between messages 281 and 300)
        let stub = Stub { seen, _ in
            let limit = Int64(seen.query["limit"] ?? "") ?? 0
            let before = seen.query["before"].flatMap { Int64($0) }, after = Int64(seen.query["after"] ?? "") ?? 0
            func span(_ top: Int64) -> [Int64] { before.map { Array(max(1, $0 - limit)..<min($0, top + 1)) } ?? (after >= top ? [] : Array((after + 1)...min(after + limit, top))) }
            switch seen.route {
            case "GET /api/f/chat--abc123/channels/chat":
                return (200, #"{"channel":"chat","records":[\#(span(300).map { record("chat", $0, body: #"{"text":"m"}"#) }.joined(separator: ","))],"next":null}"#)
            case "GET /api/f/chat--abc123/channels/work":
                let page = span(20).map { k in #"{"channel":"work","seq":\#(k),"at":\#((280 + k) * 1000 + 500),"principal":"npub1agent","kind":"message","body":{"kind":"turn.step","turn":"t\#(k)","tool":"search"}}"# }
                return (200, #"{"channel":"work","records":[\#(page.joined(separator: ","))],"next":null}"#)
            default: return (404, "{}")
            }
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let first = stub.sockets[0]
        first.push(hello)
        try await waitUntil("its subscriptions") { first.sent.count == 2 }
        for seq: Int64 in 251...300 { first.push(frame("chat", seq, body: #"{"text":"m"}"#)) }
        for k: Int64 in 1...20 { first.push(#"{"type":"record","channel":"work","seq":\#(k),"at":\#((280 + k) * 1000 + 500),"principal":"npub1agent","kind":"message","body":{"kind":"turn.step","turn":"t\#(k)","tool":"search"}}"#) }
        first.push(#"{"type":"subscribed","channel":"chat","next":300,"more":false}"#)
        first.push(#"{"type":"subscribed","channel":"work","next":20,"more":false}"#)
        try await waitUntil("read") { session.read }
        // to the first message, then on, a window of 600 never detaches here: back at its first, hasOlder is no
        for _ in 0..<6 { await session.loadOlder() }
        #expect(session.rows.first?.id == "chat:1" && !session.hasOlder)

        // a reconnect replays two messages: both count
        let heard = session.heard
        first.drop()
        try await waitUntil("the second socket") { stub.sockets.count == 2 }
        let second = stub.sockets[1]
        second.push(hello)
        try await waitUntil("its subscriptions") { second.sent.count == 2 }
        // (no new messages on the server here: these are the live ones it replays)
        second.push(frame("chat", 301, body: #"{"text":"while away"}"#))
        second.push(frame("chat", 302, body: #"{"text":"while away too"}"#))
        second.push(#"{"type":"subscribed","channel":"chat","next":302,"more":false}"#)
        second.push(#"{"type":"subscribed","channel":"work","next":20,"more":false}"#)
        try await waitUntil("caught up") { session.caughtUp && session.workRead }
        #expect(session.heard == heard + 2, "a catch-up's replay counts")
        #expect(session.rows.last?.id == "chat:302")
        // its oldest end goes as the window passes its bound: there are older messages again
        for seq: Int64 in 303...602 { second.push(frame("chat", seq, body: #"{"text":"m"}"#)) }
        try await waitUntil("its oldest end gone") { session.rows.first?.id != "chat:1" }
        #expect(session.hasOlder, "its oldest end evicted, older messages are there again")
    }

    /// Steps and a prompt after the newest message, heard while paged back,
    /// are the chat's when it rejoins the newest: by Latest, and by paging
    /// forward to its end.
    @Test func rejoiningTheNewestBringsTheWorkAfterItsLastMessage() async throws {
        // 700 messages; work 1...6 after message 700: five steps, then a prompt
        let work: @Sendable (Int64) -> String = { k in
            #"{"channel":"work","seq":\#(k),"at":\#(700_000 + 100 * k),"principal":"npub1agent","kind":"message","body":{"kind":"\#(k == 6 ? "turn.prompt" : "turn.step")","turn":"t","tool":"search","text":"Go on?"}}"#
        }
        let stub = Stub { seen, _ in
            let limit = Int64(seen.query["limit"] ?? "") ?? 0
            let before = seen.query["before"].flatMap { Int64($0) }, after = Int64(seen.query["after"] ?? "") ?? 0
            func span(_ top: Int64) -> [Int64] { before.map { Array(max(1, $0 - limit)..<min($0, top + 1)) } ?? (after >= top ? [] : Array((after + 1)...min(after + limit, top))) }
            switch seen.route {
            case "GET /api/f/chat--abc123/channels/chat":
                return (200, #"{"channel":"chat","records":[\#(span(700).map { record("chat", $0, body: #"{"text":"m"}"#) }.joined(separator: ","))],"next":null}"#)
            case "GET /api/f/chat--abc123/channels/work":
                return (200, #"{"channel":"work","records":[\#(span(6).map(work).joined(separator: ","))],"next":null}"#)
            default: return (404, "{}")
            }
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: chat, api: api)
        let run = Task { await session.run() }
        defer { run.cancel() }
        try await waitUntil("a socket") { stub.sockets.count == 1 }
        let socket = stub.sockets[0]
        socket.push(hello)
        try await waitUntil("its subscriptions") { socket.sent.count == 2 }
        for seq: Int64 in 651...700 { socket.push(frame("chat", seq, body: #"{"text":"m"}"#)) }
        socket.push(#"{"type":"subscribed","channel":"chat","next":700,"more":false}"#)
        socket.push(#"{"type":"subscribed","channel":"work","next":0,"more":false}"#)
        try await waitUntil("read") { session.read }
        func pageBack() async { while !session.detached { await session.loadOlder() } }
        await pageBack()
        // heard while paged back: the steps and the prompt after the newest message
        for k: Int64 in 1...6 { socket.push(#"{"type":"record",\#(work(k).dropFirst())"#) }
        try await waitUntil("heard") { session.timeline.cursors["work"] == 6 }
        #expect(!session.timeline.records.contains { $0.channel == "work" }, "paged back, outside the window")

        await session.resetToLatest()
        #expect(!session.detached && (1...6).allSatisfy { k in session.timeline.records.contains { $0.id == "work:\(k)" } }, "Latest: the chat whole, its prompt too")

        await pageBack()
        #expect(!session.timeline.records.contains { $0.channel == "work" })
        while session.detached { await session.loadNewer() }
        #expect((1...6).allSatisfy { k in session.timeline.records.contains { $0.id == "work:\(k)" } }, "paged forward to its end: whole too")
    }
}

/// The answer to a post: the record it appended, its body the post's.
private func posted(_ seen: Seen, seq: Int64, replayed: Bool) -> (Int, String) {
    let body = String(decoding: (try? JSONEncoder().encode(seen.json["body"])) ?? Data("null".utf8), as: UTF8.self)
    return (200, #"{"record":\#(record("chat", seq, principal: "npub1me", body: body)),"replayed":\#(replayed)}"#)
}
