import Foundation

enum APIError: LocalizedError {
    /// `refused`: the server answered 4xx (but a timeout or a rate limit), a
    /// final answer: the same request again gets it again.
    /// `absent`: a 404, which `answer` says apart only to `optional` readers.
    case invalidResponse, limitExceeded, server(String), refused(String), absent(String), unauthorized, keychain(Int32)
    var errorDescription: String? {
        switch self {
        case .unauthorized: "This iPhone is not authorized. Connect your account again."
        case .keychain(let status): "The iPhone key could not be accessed (Keychain \(status))."
        case .invalidResponse: "Finite sent an unreadable response."
        case .limitExceeded: "This response is larger than the app can display."
        case .server(let message), .refused(let message), .absent(let message): message
        }
    }
    /// A non-2xx answer as an error, with the server's own message.
    static func answer(_ data: Data, status: Int) -> APIError {
        let message = (try? JSONDecoder().decode(JSONValue.self, from: data))?["message"].string ?? "Finite returned HTTP \(status)."
        return (400..<500).contains(status) && status != 408 && status != 429 ? .refused(message) : .server(message)
    }
}

/// A live socket a chat or the list watcher reads: a URLSessionWebSocketTask,
/// or a test's scripted one (URLProtocol never sees WebSocket tasks).
protocol LiveSocket: AnyObject, Sendable {
    var response: URLResponse? { get }
    var closeCode: URLSessionWebSocketTask.CloseCode { get }
    func resume()
    func send(_ message: URLSessionWebSocketTask.Message) async throws
    func receive() async throws -> URLSessionWebSocketTask.Message
    func cancel(with closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?)
}

extension URLSessionWebSocketTask: LiveSocket {}

/// Every request signs with this iPhone's approved key (NIP-98), to its own
/// platform origin only.
struct FragmentAPI: Sendable {
    typealias Sockets = @Sendable (URLRequest) -> LiveSocket
    let baseURL: URL
    let session: URLSession
    let identity: DeviceIdentity
    let accountID: String?
    private let sockets: Sockets?
    init(baseURL: URL, identity: DeviceIdentity, accountID: String? = nil, session: URLSession? = nil, sockets: Sockets? = nil) {
        self.baseURL = baseURL
        self.identity = identity
        self.accountID = accountID
        self.sockets = sockets
        if let session { self.session = session }
        else {
            let configuration = URLSessionConfiguration.ephemeral
            configuration.httpShouldSetCookies = false
            configuration.httpCookieStorage = nil
            configuration.urlCredentialStorage = nil
            configuration.urlCache = nil
            configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
            self.session = URLSession(configuration: configuration, delegate: NoSignedRedirects(), delegateQueue: nil)
        }
    }
    func authorized(_ original: URLRequest) throws -> URLRequest {
        guard let url = original.url, PlatformOrigin.same(baseURL, url), url.user == nil, url.password == nil,
              url.fragment == nil, original.httpBodyStream == nil else { throw APIError.server("The request does not belong to your Finite server.") }
        guard LaunchScope.current.allows(url) else { throw LaunchScope.refused }
        var request = original
        request.setValue(try identity.authorization(method: request.httpMethod ?? "GET", url: url, body: request.httpBody ?? Data()), forHTTPHeaderField: "Authorization")
        request.setValue("Finite-iOS/0.3", forHTTPHeaderField: "User-Agent")
        return request
    }
    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        let result = try await session.data(for: authorized(request))
        if (result.1 as? HTTPURLResponse)?.statusCode == 401 {
            authorizationFailed()
            throw APIError.unauthorized
        }
        return result
    }
    func authorizationFailed() {
        NotificationCenter.default.post(name: .finiteAuthorizationFailed, object: nil,
                                        userInfo: ["origin": baseURL.absoluteString, "key": identity.publicKey])
    }

    func url(_ path: String) -> URL { baseURL.appending(path: path) }

    func request<T: Decodable>(_ path: String, method: String = "GET", body: JSONValue? = nil,
                              query: [URLQueryItem] = [], timeout: TimeInterval = 30) async throws -> T {
        var components = URLComponents(url: url(path), resolvingAgainstBaseURL: false)!
        components.queryItems = query.isEmpty ? nil : query
        var request = URLRequest(url: components.url!)
        request.httpMethod = method
        request.timeoutInterval = timeout
        if let body {
            request.httpBody = try JSONEncoder().encode(body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await self.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else {
            let refusal = APIError.answer(data, status: http.statusCode)
            if http.statusCode == 404, case .refused(let message) = refusal { throw APIError.absent(message) }
            throw refusal
        }
        guard data.count <= 4_194_304 else { throw APIError.limitExceeded }
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// A read of what may be absent: `nil` for a 404, and else as `request`.
    func optional<T: Decodable>(_ type: T.Type, _ path: String, query: [URLQueryItem] = []) async throws -> T? {
        do { return try await request(path, query: query) as T }
        catch APIError.absent { return nil }
    }

    func fragments() async throws -> [FragmentSummary] {
        struct Response: Decodable { let fragments: [FragmentSummary] }
        let response: Response = try await request("api/fragments")
        return response.fragments
    }

    // Search is only a hint: the complete prompt, including words beyond the
    // API's eight-word / 256-byte query limit, remains the new chat's message.
    static func searchHint(_ text: String) -> String {
        let words = text.split(whereSeparator: { $0.isWhitespace || $0.isNewline }).prefix(8)
        var hint = ""
        for character in words.joined(separator: " ") {
            guard hint.utf8.count + String(character).utf8.count <= 256 else { break }
            hint.append(character)
        }
        return hint
    }

    func search(_ text: String) async throws -> SearchResults {
        let hint = Self.searchHint(text)
        guard !hint.isEmpty else { return SearchResults() }
        return try await request("api/search", query: [URLQueryItem(name: "q", value: hint)])
    }

    func createChat() async throws -> FragmentSummary {
        struct Created: Decodable { let name: String }
        let created: Created = try await request("api/fragments", method: "POST", body: .object([
            "label": .string("mobile-chat"), "template": .string("chat"), "title": .string("New chat")
        ]))
        return FragmentSummary(name: created.name, kind: "chat", title: "New chat")
    }

    func join(_ agent: FragmentSummary, to chat: FragmentSummary) async throws {
        struct Status: Decodable { let npub: String }
        let status: Status = try await request("api/f/\(agent.name)/status")
        let _: JSONValue = try await request("api/f/\(chat.name)/members/\(status.npub)",
            method: "PUT", body: .object(["role": .string("editor")]))
    }

    /// A chat's or an app's name, in its manifest. An agent's name is its
    /// job's too, which only the shell and the CLI rewrite today. With
    /// `from`, only a manifest still titled so is renamed (one renamed
    /// meanwhile keeps its name); whether it was.
    @discardableResult
    func rename(_ fragment: FragmentSummary, title: String, from: String? = nil, message: String = "Rename from Finite for iOS") async throws -> Bool {
        precondition(fragment.kind != "agent", "an agent is renamed where its job is rewritten with it")
        let clean = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty else { throw APIError.server("Enter a name.") }
        let manifest: JSONValue = try await request("api/f/\(fragment.name)/manifest")
        guard case .object(var fields) = manifest else { throw APIError.invalidResponse }
        var meta: [String: JSONValue] = [:]
        if case .object(let existing) = fields["meta"] { meta = existing }
        if let from, meta["title"]?.string != from { return false }
        meta["title"] = .string(clean); fields["meta"] = .object(meta)
        let text = String(decoding: try JSONEncoder().encode(JSONValue.object(fields)), as: UTF8.self)
        let _: JSONValue = try await request("api/f/\(fragment.name)/files", method: "POST", body: .object([
            "message": .string(message), "files": .array([.object(["path": .string("fragment.json"), "text": .string(text)])])
        ]))
        let _: JSONValue = try await request("api/f/\(fragment.name)/deploy", method: "POST", body: .object([:]))
        return true
    }

    func post(_ fragment: String, id: String, body: JSONValue) async throws -> ChannelRecord {
        struct Posted: Decodable { let record: ChannelRecord }
        let result: Posted = try await request("api/f/\(fragment)/channels/chat", method: "POST",
            body: .object(["id": .string(id), "body": body]))
        return result.record
    }

    /// A page of a channel's records just below `before` (at most
    /// `CHANNEL_PAGE`, 1000, and a MiB: docs/api.md).
    func history(_ fragment: String, channel: String = "chat", before: Int64, limit: Int = 50) async throws -> [ChannelRecord] {
        struct Page: Decodable { let records: [ChannelRecord] }
        var components = URLComponents(url: url("api/f/\(fragment)/channels/\(channel)"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "before", value: String(before)), URLQueryItem(name: "limit", value: String(limit))]
        let (data, response) = try await self.data(for: URLRequest(url: components.url!))
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard http.statusCode == 200 else { throw APIError.answer(data, status: http.statusCode) }
        guard data.count <= 4_194_304 else { throw APIError.limitExceeded }
        return try JSONDecoder().decode(Page.self, from: data).records
    }

    func socket(_ fragment: String) throws -> LiveSocket { try webSocket("f/\(fragment)/__live") }
    func webSocket(_ path: String) throws -> LiveSocket {
        var components = URLComponents(url: url(path), resolvingAgainstBaseURL: false)!
        components.scheme = baseURL.scheme == "https" ? "wss" : "ws"
        let request = try authorized(URLRequest(url: components.url!))
        if let sockets { return sockets(request) }
        let socket = session.webSocketTask(with: request)
        socket.maximumMessageSize = 1_048_576
        return socket
    }
}
