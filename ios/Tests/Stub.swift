import Foundation
import Synchronization
@testable import Fragment

/// A request the app made, as a stub saw it.
struct Seen: Sendable {
    let method: String
    let path: String
    let query: [String: String]
    let body: Data
    let authorization: String?
    var json: JSONValue { (try? JSONDecoder().decode(JSONValue.self, from: body)) ?? .null }
    var route: String { "\(method) \(path)" }
}

/// A test's stand-in for the platform, on a host of its own: each request
/// is answered by the test's handler (through `StubProtocol`), each socket
/// is a `ScriptedSocket` the test drives, and both are recorded. Answers
/// are written by hand here; the wire they stand for is docs/api.md.
final class Stub: Sendable {
    /// The answer to a request, given every request this stub saw before it
    /// (a body of `base64:<bytes>` is those bytes).
    typealias Handler = @Sendable (Seen, [Seen]) -> (Int, String)
    private static let live = Mutex<[String: Stub]>([:])
    static func named(_ host: String) -> Stub? { live.withLock { $0[host] } }

    let host = "stub-\(UUID().uuidString.lowercased()).test"
    private let handler: Handler
    private let state = Mutex<(seen: [Seen], sockets: [ScriptedSocket])>(([], []))

    init(_ handler: @escaping Handler) {
        self.handler = handler
        Self.live.withLock { $0[host] = self }
    }
    deinit { Self.live.withLock { _ = $0.removeValue(forKey: host) } }

    var seen: [Seen] { state.withLock { $0.seen } }
    var sockets: [ScriptedSocket] { state.withLock { $0.sockets } }
    func answer(_ request: Seen) -> (Int, String) {
        let before = state.withLock { s in defer { s.seen.append(request) }; return s.seen }
        return handler(request, before)
    }

    /// An API on this stub, signed by a fresh key.
    func api(accountID: String? = nil) throws -> FragmentAPI {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubProtocol.self]
        return FragmentAPI(baseURL: URL(string: "https://\(host)")!, identity: try DeviceIdentity(), accountID: accountID,
                           session: URLSession(configuration: configuration), sockets: { [weak self] request in
            let socket = ScriptedSocket(request: request)
            self?.state.withLock { $0.sockets.append(socket) }
            return socket
        })
    }
}

/// Each request answered on a queue of its own, never on the thread URLSession
/// loads every stubbed request on: a test that holds one answer (`Gate`)
/// holds no other.
final class StubProtocol: URLProtocol, @unchecked Sendable {
    private let stopped = Mutex(false)
    override class func canInit(with request: URLRequest) -> Bool { request.url?.host.flatMap(Stub.named) != nil }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        guard let url = request.url, let stub = url.host.flatMap(Stub.named) else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotFindHost)); return
        }
        var query: [String: String] = [:]
        for item in URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? [] { query[item.name] = item.value ?? "" }
        let seen = Seen(method: request.httpMethod ?? "GET", path: url.path, query: query, body: request.httpBody ?? Self.read(request.httpBodyStream),
                        authorization: request.value(forHTTPHeaderField: "Authorization"))
        DispatchQueue.global().async { [self] in
            let (status, body) = stub.answer(seen)
            guard !stopped.withLock({ $0 }) else { return }
            let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            // `base64:` answers bytes that are not text (a picture)
            let bytes = body.hasPrefix("base64:") ? Data(base64Encoded: String(body.dropFirst(7))) ?? Data() : Data(body.utf8)
            client?.urlProtocol(self, didLoad: bytes)
            client?.urlProtocolDidFinishLoading(self)
        }
    }
    override func stopLoading() { stopped.withLock { $0 = true } }
    private static func read(_ stream: InputStream?) -> Data {
        guard let stream else { return Data() }
        stream.open(); defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 16_384)
        while stream.hasBytesAvailable {
            let n = stream.read(&buffer, maxLength: buffer.count)
            guard n > 0 else { break }
            data.append(buffer, count: n)
        }
        return data
    }
}

/// A `__live` socket the test plays the server's side of: frames it pushes
/// arrive in order, `drop` ends it as a lost connection, and every frame
/// the app sent is kept.
final class ScriptedSocket: LiveSocket {
    let request: URLRequest
    private let state = Mutex<(queue: [Result<URLSessionWebSocketTask.Message, any Error>], waiting: CheckedContinuation<URLSessionWebSocketTask.Message, any Error>?, sent: [JSONValue], cancelled: Bool)>(([], nil, [], false))
    init(request: URLRequest) { self.request = request }

    var response: URLResponse? { nil }
    var closeCode: URLSessionWebSocketTask.CloseCode { .invalid }
    var sent: [JSONValue] { state.withLock { $0.sent } }
    var cancelled: Bool { state.withLock { $0.cancelled } }
    func resume() {}
    func send(_ message: URLSessionWebSocketTask.Message) async throws {
        guard case .string(let text) = message else { throw URLError(.cannotDecodeContentData) }
        let frame = try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
        state.withLock { $0.sent.append(frame) }
    }
    func receive() async throws -> URLSessionWebSocketTask.Message {
        try await withCheckedThrowingContinuation { continuation in
            let next: Result<URLSessionWebSocketTask.Message, any Error>? = state.withLock { s in
                if !s.queue.isEmpty { return s.queue.removeFirst() }
                if s.cancelled { return .failure(URLError(.cancelled)) }
                s.waiting = continuation
                return nil
            }
            if let next { continuation.resume(with: next) }
        }
    }
    func cancel(with closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        let waiting = state.withLock { s in s.cancelled = true; defer { s.waiting = nil }; return s.waiting }
        waiting?.resume(throwing: URLError(.cancelled))
    }
    func push(_ frame: String) { deliver(.success(.string(frame))) }
    func drop() { deliver(.failure(URLError(.networkConnectionLost))) }
    private func deliver(_ result: Result<URLSessionWebSocketTask.Message, any Error>) {
        let waiting = state.withLock { s -> CheckedContinuation<URLSessionWebSocketTask.Message, any Error>? in
            guard let waiting = s.waiting else { s.queue.append(result); return nil }
            s.waiting = nil
            return waiting
        }
        waiting?.resume(with: result)
    }
}

/// Polls `condition` on the main actor until it holds, or records a failure.
@MainActor
func waitUntil(_ what: String, timeout: Duration = .seconds(10), _ condition: @MainActor () -> Bool) async throws {
    let clock = ContinuousClock()
    let deadline = clock.now + timeout
    while !condition() {
        guard clock.now < deadline else { throw StubTimeout(what: what) }
        try await Task.sleep(for: .milliseconds(20))
    }
}

struct StubTimeout: Error, CustomStringConvertible { let what: String; var description: String { "timed out waiting for \(what)" } }

/// A record frame or an answer's record, as the server writes it.
func record(_ channel: String, _ seq: Int64, principal: String = "npub1agent", body: String) -> String {
    #"{"channel":"\#(channel)","seq":\#(seq),"at":\#(seq * 1000),"principal":"\#(principal)","kind":"message","body":\#(body)}"#
}

/// Holds the stub's answers it is asked in, until opened: a test's way to
/// keep one request in flight while others are made.
final class Gate: Sendable {
    private let opened = DispatchSemaphore(value: 0)
    private let state = Mutex(false)
    var isOpen: Bool { state.withLock { $0 } }
    /// On the stub's own thread: waits until the test opens it.
    func pass() { if !isOpen { opened.wait(); opened.signal() } }
    func open() { state.withLock { $0 = true }; opened.signal() }
}
