import Foundation

enum APIError: LocalizedError {
    case invalidResponse, limitExceeded, server(String), unauthorized, keychain(Int32)
    var errorDescription: String? {
        switch self {
        case .unauthorized: "This iPhone is not authorized. Connect your account again."
        case .keychain(let status): "The iPhone key could not be accessed (Keychain \(status))."
        case .invalidResponse: "Finite sent an unreadable response."
        case .limitExceeded: "This response is larger than the app can display."
        case .server(let message): message
        }
    }
}

struct FragmentAPI: Sendable {
    let baseURL: URL
    let session: URLSession

    let identity: DeviceIdentity?
    let accountID: String?
    var isSigned: Bool { identity != nil }
    init(baseURL: URL, session: URLSession? = nil, identity: DeviceIdentity? = nil, accountID: String? = nil) {
        self.baseURL = baseURL
        self.identity = identity
        self.accountID = accountID
        if let session { self.session = session }
        else if identity != nil {
            let configuration = URLSessionConfiguration.ephemeral
            configuration.httpShouldSetCookies = false
            configuration.httpCookieStorage = nil
            configuration.urlCredentialStorage = nil
            configuration.urlCache = nil
            configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
            self.session = URLSession(configuration: configuration, delegate: NoSignedRedirects(), delegateQueue: nil)
        } else { self.session = .shared }
    }
    func authorized(_ original: URLRequest) throws -> URLRequest {
        guard let identity else { return original }
        guard let url = original.url, PlatformOrigin.same(baseURL, url), url.user == nil, url.password == nil,
              url.fragment == nil, original.httpBodyStream == nil else { throw APIError.server("The request does not belong to your Finite server.") }
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
        guard let identity else { return }
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
            let problem = try? JSONDecoder().decode(JSONValue.self, from: data)
            throw APIError.server(problem?["message"].string ?? "Finite returned HTTP \(http.statusCode).")
        }
        guard data.count <= 4_194_304 else { throw APIError.limitExceeded }
        return try JSONDecoder().decode(T.self, from: data)
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

    func rename(_ fragment: FragmentSummary, title: String) async throws {
        let clean = title.trimmingCharacters(in: .whitespacesAndNewlines)
        let limit = ["app", "brain"].contains(fragment.kind) ? 120 : 48
        guard !clean.isEmpty, clean.count <= limit else { throw APIError.server("Use a name between 1 and \(limit) characters.") }
        let manifest: JSONValue = try await request("api/f/\(fragment.name)/manifest")
        guard case .object(var fields) = manifest else { throw APIError.invalidResponse }
        var meta: [String: JSONValue] = [:]
        if case .object(let existing) = fields["meta"] { meta = existing }
        meta["title"] = .string(clean); fields["meta"] = .object(meta)
        let text = String(decoding: try JSONEncoder().encode(JSONValue.object(fields)), as: UTF8.self)
        let _: JSONValue = try await request("api/f/\(fragment.name)/files", method: "POST", body: .object([
            "message": .string("Rename from Finite for iOS"), "files": .array([.object(["path": .string("fragment.json"), "text": .string(text)])])
        ]))
        let _: JSONValue = try await request("api/f/\(fragment.name)/deploy", method: "POST", body: .object([:]))
    }

    func post(_ fragment: String, id: String, body: JSONValue) async throws -> ChannelRecord {
        struct Posted: Decodable { let record: ChannelRecord }
        let result: Posted = try await request("api/f/\(fragment)/channels/chat", method: "POST",
            body: .object(["id": .string(id), "body": body]))
        return result.record
    }

    func history(_ fragment: String, before: Int64) async throws -> [ChannelRecord] {
        struct Page: Decodable { let records: [ChannelRecord] }
        var components = URLComponents(url: url("api/f/\(fragment)/channels/chat"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "before", value: String(before)), URLQueryItem(name: "limit", value: "50")]
        let (data, response) = try await self.data(for: URLRequest(url: components.url!))
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw APIError.server("Earlier messages could not be loaded.") }
        guard data.count <= 4_194_304 else { throw APIError.limitExceeded }
        return try JSONDecoder().decode(Page.self, from: data).records
    }

    func socket(_ fragment: String) throws -> URLSessionWebSocketTask { try webSocket("f/\(fragment)/__live") }
    func webSocket(_ path: String) throws -> URLSessionWebSocketTask {
        var components = URLComponents(url: url(path), resolvingAgainstBaseURL: false)!
        components.scheme = baseURL.scheme == "https" ? "wss" : "ws"
        let socket = session.webSocketTask(with: try authorized(URLRequest(url: components.url!)))
        socket.maximumMessageSize = 1_048_576
        return socket
    }
}
