import Foundation
import UIKit

struct AppDetails: Decodable, Sendable {
    struct URLs: Decodable, Sendable { let canonical: URL }
    struct Page: Decodable, Sendable {
        struct Problem: Decodable, Sendable { let kind: String; let text: String }
        let at: Int64
        var errors: [Problem] = []
    }
    let owner: String
    let role: String
    let visibility: String
    let urls: URLs
    var viewToken: String?
    var page: Page?
    var canEdit: Bool { role == "owner" || role == "editor" }
    var shareURL: URL? {
        if visibility == "public" { return urls.canonical }
        guard visibility == "link", role == "owner", let viewToken else { return nil }
        var components = URLComponents(url: urls.canonical, resolvingAgainstBaseURL: false)
        components?.queryItems = [URLQueryItem(name: "view", value: viewToken)]
        return components?.url
    }
}

struct AppMember: Decodable, Identifiable, Sendable {
    let principal: String
    let role: String
    var kind: String?
    var owner: String?
    var email: String?
    var title: String?
    var id: String { principal }
}

struct AppInvite: Decodable, Identifiable, Sendable {
    let email: String
    let role: String
    let expiresAt: Int64
    var id: String { email }
}

struct AppIconOptions: Decodable, Sendable {
    struct Candidate: Decodable, Identifiable, Sendable {
        let png: String
        let score: Double
        var id: String { png }
        var data: Data? { Data(base64Encoded: png) }
    }
    let subject: String
    let candidates: [Candidate]
}

enum AppAccess: String, CaseIterable {
    case members, link, `public`
    var title: String {
        switch self { case .members: "Restricted"; case .link: "Anyone with the link"; case .public: "Public" }
    }
    var explanation: String {
        switch self {
        case .members: "Only people with access can open this app."
        case .link: "Anyone with its share link can open this app."
        case .public: "Anyone can find and open this app."
        }
    }
}

enum AppPermission: String, CaseIterable {
    case viewer, editor
    var title: String { self == .viewer ? "Can view" : "Can edit" }
}

extension FragmentAPI {
    func appDetails(_ name: String) async throws -> AppDetails { try await request("api/f/\(name)/status") }

    func appMembers(_ name: String) async throws -> [AppMember] {
        struct Response: Decodable { let members: [AppMember] }
        let result: Response = try await request("api/f/\(name)/members")
        return result.members
    }

    func appInvites(_ name: String) async throws -> [AppInvite] {
        struct Response: Decodable { let invites: [AppInvite] }
        let result: Response = try await request("api/f/\(name)/invites")
        return result.invites
    }

    func appImage(_ name: String, kind: String) async throws -> Data? {
        precondition(kind == "icon" || kind == "card")
        var request = URLRequest(url: url("api/f/\(name)/\(kind)"))
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 30
        let (data, response) = try await self.data(for: request)
        guard let response = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        if response.statusCode == 404 { return nil }
        guard response.statusCode == 200 else { throw APIError.server("The image could not be loaded (HTTP \(response.statusCode)).") }
        guard data.count <= 2 * 1_024 * 1_024, UIImage(data: data) != nil else { throw APIError.invalidResponse }
        return data
    }

    func putAppIcon(_ data: Data, name: String) async throws {
        guard !data.isEmpty, data.count <= 2 * 1_024 * 1_024 else { throw APIError.server("Choose an image smaller than 2 MB.") }
        var request = URLRequest(url: url("api/f/\(name)/icon"))
        request.httpMethod = "PUT"; request.httpBody = data; request.timeoutInterval = 30
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        let (body, response) = try await self.data(for: request)
        guard let response = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200..<300).contains(response.statusCode) else {
            let problem = try? JSONDecoder().decode(JSONValue.self, from: body)
            throw APIError.server(problem?["message"].string ?? "The icon could not be saved (HTTP \(response.statusCode)).")
        }
    }

    func makeAppIcons(_ name: String, hint: String) async throws -> AppIconOptions {
        guard hint.count <= 200 else { throw APIError.server("Describe the icon in 200 characters or fewer.") }
        var fields: [String: JSONValue] = [:]
        let clean = hint.trimmingCharacters(in: .whitespacesAndNewlines)
        if !clean.isEmpty { fields["hint"] = .string(clean) }
        return try await request("api/f/\(name)/icon/make", method: "POST", body: .object(fields), timeout: 90)
    }
}
