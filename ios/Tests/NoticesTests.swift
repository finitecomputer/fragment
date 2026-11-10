import Foundation
import Synchronization
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

    /// A failed action says why (Paul, 2026-11-09): the server's own words
    /// for its refusal; a call that never reached it, the app's.
    @MainActor @Test func aFailedActionSaysWhy() {
        #expect(Library.said(APIError.refused("You already have a seat in Acme: decline this offer.")) == "You already have a seat in Acme: decline this offer.")
        #expect(Library.said(APIError.absent("No invite to garden--k3x9 waits on you.")) == "No invite to garden--k3x9 waits on you.")
        #expect(Library.said(URLError(.notConnectedToInternet)) == "Couldn't do that. Try again.")
    }

    /// An action's answer that names a page (Paul, 2026-11-09): one of
    /// theirs opens here if the first read begun after the answer lists it,
    /// and never after it (the person may be elsewhere by then); any other
    /// http(s) address goes to Safari; any other scheme, nowhere.
    @MainActor @Test func anAnswersPageOpensByTheReadAfterItAlone() async throws {
        final class Listed: Sendable { let now = Mutex(false); let open = Mutex("") }
        let listed = Listed()
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/fragments":
                let garden = listed.now.withLock { $0 } ? #",{"name":"garden--k3x9","kind":"app","role":"editor","title":"Garden plans"}"# : ""
                return (200, #"{"fragments":[{"name":"juniper--a2","kind":"agent","role":"owner","title":"Juniper"}"# + garden + "]}")
            case "GET /api/f/juniper--a2/status": return (200, #"{"npub":"npub1juniper","urls":{"canonical":"https://juniper--a2.finite.test/"}}"#)
            case "POST /api/invites/garden--k3x9/accept":
                return (200, #"{"fragment":"garden--k3x9","role":"editor","open":""# + listed.open.withLock { $0 } + #""}"#)
            default: return (404, #"{"error":"not_found","message":"no route"}"#)
            }
        }
        let api = try stub.api()
        let notice = try #require(try read(#"{"notices":[{"id":"invite:garden--k3x9","kind":"invite","level":"info","title":"Garden plans","actions":[{"label":"Accept","post":"/api/invites/garden--k3x9/accept","body":{}}]}]}"#).first)
        let library = Library()
        await library.load(api)
        #expect(library.pages != nil, "this deployment's pages, from their own fragment")

        // the read after it lists it: it opens here, Safari given nothing
        listed.open.withLock { $0 = "https://garden--k3x9.finite.test/" }
        listed.now.withLock { $0 = true }
        #expect(await library.act(notice, notice.actions[0], api: api) == nil)
        #expect(library.answered?.name == "garden--k3x9")
        library.answered = nil

        // the read after it does not: nothing opens, then or when a later read lists it
        listed.now.withLock { $0 = false }
        #expect(await library.act(notice, notice.actions[0], api: api) == nil)
        #expect(library.answered == nil)
        listed.now.withLock { $0 = true }
        await library.load(api)
        #expect(library.answered == nil, "a later read never opens it")

        // an address not theirs is answered for Safari; another scheme, for nothing
        listed.open.withLock { $0 = "https://checkout.example.test/c/1" }
        #expect(await library.act(notice, notice.actions[0], api: api) == URL(string: "https://checkout.example.test/c/1"))
        listed.open.withLock { $0 = "tel:5550100" }
        #expect(await library.act(notice, notice.actions[0], api: api) == nil)
        #expect(library.answered == nil)
    }

    /// One that must be answered (FIN-458): read as said (none said: not),
    /// asked alone, the first of them first, and never in the band.
    @MainActor @Test func oneThatMustBeAnsweredIsAskedAloneTheFirstFirst() throws {
        let told = try read(#"{"notices":[{"id":"key","kind":"key_paused","level":"warn","title":"Calls paused"},{"id":"asked:move","kind":"asked","level":"info","title":"Move to finite.chat?","mustAnswer":true,"actions":[{"label":"Yes","post":"/api/notices/asked/move","body":{"answer":"Yes"}}]},{"id":"asked:terms","kind":"asked","level":"info","title":"New terms","mustAnswer":true}]}"#)
        #expect(told.map(\.mustAnswer) == [false, true, true])
        let library = Library()
        library.notices = told
        #expect(library.asked?.id == "asked:move")
        library.notices = [told[0], told[2]]
        #expect(library.asked?.id == "asked:terms")
        library.notices = [told[0]]
        #expect(library.asked == nil)
    }

    /// The read after an answer decides its page as it takes its list, and
    /// nothing after (Codex on #1162): that read not listing it, a newer one
    /// that lists it while the first still reads the rest opens nothing.
    @MainActor @Test func anAnswersPageIsDecidedAsItsReadTakesItsList() async throws {
        final class Shared: Sendable { let listed = Mutex(false); let gate = Mutex<Gate?>(nil) }
        let shared = Shared()
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/fragments":
                let garden = shared.listed.withLock { $0 } ? #",{"name":"garden--k3x9","kind":"app","role":"editor","title":"Garden plans"}"# : ""
                return (200, #"{"fragments":[{"name":"juniper--a2","kind":"agent","role":"owner","title":"Juniper"}"# + garden + "]}")
            case "GET /api/f/juniper--a2/status": return (200, #"{"npub":"npub1juniper","urls":{"canonical":"https://juniper--a2.finite.test/"}}"#)
            case "GET /api/computers":
                shared.gate.withLock { $0 }?.pass()
                return (200, #"{"computers":[]}"#)
            case "POST /api/invites/garden--k3x9/accept": return (200, #"{"fragment":"garden--k3x9","role":"editor","open":"https://garden--k3x9.finite.test/"}"#)
            default: return (404, #"{"error":"not_found","message":"no route"}"#)
            }
        }
        let api = try stub.api()
        let notice = try #require(try read(#"{"notices":[{"id":"invite:garden--k3x9","kind":"invite","level":"info","title":"Garden plans","actions":[{"label":"Accept","post":"/api/invites/garden--k3x9/accept","body":{}}]}]}"#).first)
        let library = Library()
        await library.load(api)
        let lists = { stub.seen.filter { $0.route == "GET /api/fragments" }.count }
        let computers = { stub.seen.filter { $0.route == "GET /api/computers" }.count }
        // the read after the answer takes a list without it, then waits on the computer
        let gate = Gate()
        shared.gate.withLock { $0 = gate }
        let acting = Task { await library.act(notice, notice.actions[0], api: api) }
        try await waitUntil("the read after the answer took its list and asks for the computer") { computers() == 2 }
        // a newer read, begun meanwhile, lists it
        shared.listed.withLock { $0 = true }
        let newer = Task { await library.load(api) }
        try await waitUntil("the newer read has its list") { lists() == 3 }
        gate.open()
        await newer.value
        #expect(await acting.value == nil)
        #expect(library.fragments.contains { $0.name == "garden--k3x9" }, "the newer read's list is shown")
        #expect(library.answered == nil, "decided by the read after the answer, which did not list it: nothing opens")
    }

    /// The stale-navigation case (Alex on #1162): invite A accepted, the read
    /// after it loses to a newer one or fails, the person goes elsewhere, and
    /// a later read lists A: A must not open over them. The first list read
    /// after the answer decides it, newest or not; a failed one, as not listed.
    @MainActor @Test func aLaterReadNeverOpensAnAnswersPage() async throws {
        final class Shared: Sendable {
            let listed = Mutex(false); let failing = Mutex(false)
            let gates = Mutex<[Int: Gate]>([:]); let lists = Mutex(0)
        }
        let shared = Shared()
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/fragments":
                let n = shared.lists.withLock { $0 += 1; return $0 }
                let answer: (Int, String) = shared.failing.withLock { $0 } ? (503, #"{"error":"unavailable","message":"Try again shortly."}"#) : {
                    let garden = shared.listed.withLock { $0 } ? #",{"name":"garden--k3x9","kind":"app","role":"editor","title":"Garden plans"}"# : ""
                    return (200, #"{"fragments":[{"name":"juniper--a2","kind":"agent","role":"owner","title":"Juniper"}"# + garden + "]}")
                }()
                shared.gates.withLock { $0[n] }?.pass()
                return answer
            case "GET /api/f/juniper--a2/status": return (200, #"{"npub":"npub1juniper","urls":{"canonical":"https://juniper--a2.finite.test/"}}"#)
            case "POST /api/invites/garden--k3x9/accept": return (200, #"{"fragment":"garden--k3x9","role":"editor","open":"https://garden--k3x9.finite.test/"}"#)
            default: return (404, #"{"error":"not_found","message":"no route"}"#)
            }
        }
        let api = try stub.api()
        let notice = try #require(try read(#"{"notices":[{"id":"invite:garden--k3x9","kind":"invite","level":"info","title":"Garden plans","actions":[{"label":"Accept","post":"/api/invites/garden--k3x9/accept","body":{}}]}]}"#).first)
        let lists = { shared.lists.withLock { $0 } }

        // loses: the read after the answer (2) is still out when a newer one
        // (3) begins; 2's list, behind, lacks A; the person goes elsewhere;
        // then 3 lists A
        let library = Library()
        await library.load(api)
        let (behind, later) = (Gate(), Gate())
        shared.gates.withLock { $0 = [2: behind, 3: later] }
        let acting = Task { await library.act(notice, notice.actions[0], api: api) }
        try await waitUntil("the read after the answer is out") { lists() == 2 }
        shared.listed.withLock { $0 = true }
        let newer = Task { await library.load(api) }
        try await waitUntil("a newer read is out") { lists() == 3 }
        behind.open()
        #expect(await acting.value == nil)
        later.open()
        await newer.value
        #expect(library.fragments.contains { $0.name == "garden--k3x9" }, "A is listed now")
        #expect(library.answered == nil, "a later read never opens it")

        // fails: the read after the answer fails; a later read lists A
        let other = Library()
        shared.listed.withLock { $0 = false }
        await other.load(api)
        shared.failing.withLock { $0 = true }
        #expect(await other.act(notice, notice.actions[0], api: api) == nil)
        shared.failing.withLock { $0 = false }
        shared.listed.withLock { $0 = true }
        await other.load(api)
        #expect(other.fragments.contains { $0.name == "garden--k3x9" })
        #expect(other.answered == nil, "nor after a failed one")
    }

    /// An answer's page waits while a notice must be answered (Codex on the
    /// must-answer cover): answering A opens a page, B must be answered
    /// next; the page is taken once nothing is asked, never lost.
    @MainActor @Test func anAnswersPageWaitsWhileANoticeMustBeAnswered() throws {
        let told = try read(#"{"notices":[{"id":"asked:terms","kind":"asked","level":"info","title":"New terms","mustAnswer":true}]}"#)
        let library = Library()
        library.fragments = [try JSONDecoder().decode(FragmentSummary.self, from: Data(#"{"name":"garden--k3x9","kind":"app","role":"editor","title":"Garden plans"}"#.utf8))]
        library.answered = library.fragments.first
        library.notices = told
        #expect(library.takeAnswered() == nil, "not while B must be answered")
        #expect(library.answered?.name == "garden--k3x9", "kept, not lost")
        library.notices = []
        #expect(library.takeAnswered()?.name == "garden--k3x9", "taken once nothing is asked")
        #expect(library.takeAnswered() == nil, "and once")
    }
}
