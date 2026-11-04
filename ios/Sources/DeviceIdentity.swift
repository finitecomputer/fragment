import Foundation
import CryptoKit
import Security
import P256K

struct DeviceIdentity: Sendable {
    let secret: Data
    let publicKey: String
    init(secret: Data) throws {
        let key = try P256K.Schnorr.PrivateKey(dataRepresentation: secret)
        self.secret = secret
        publicKey = Data(key.xonly.bytes).hex
    }
    init() throws { try self.init(secret: P256K.Schnorr.PrivateKey().dataRepresentation) }

    func authorization(method: String, url: URL, body: Data = Data(), at: Int64 = Int64(Date().timeIntervalSince1970)) throws -> String {
        // NIP-98 binds the HTTP URL (https/http, including on a WebSocket upgrade),
        // method, exact encoded body, and time. JSONSerializer emits NIP-01's array.
        var target = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        if target.scheme == "wss" { target.scheme = "https" }
        if target.scheme == "ws" { target.scheme = "http" }
        guard let absolute = target.url, ["http", "https"].contains(absolute.scheme) else { throw APIError.invalidResponse }
        var tags = [["u", absolute.absoluteString], ["method", method]]
        if !body.isEmpty { tags.append(["payload", Data(SHA256.hash(data: body)).hex]) }
        let serialized = try JSONSerialization.data(withJSONObject: [0, publicKey, at, 27235, tags, ""], options: [.withoutEscapingSlashes])
        var digest = Array(SHA256.hash(data: serialized))
        let key = try P256K.Schnorr.PrivateKey(dataRepresentation: secret)
        let signature = try key.signature(message: &digest, auxiliaryRand: nil, strict: true)
        let event: [String: Any] = ["id": Data(digest).hex, "pubkey": publicKey, "created_at": at, "kind": 27235,
                                    "tags": tags, "content": "", "sig": signature.dataRepresentation.hex]
        return "Nostr " + (try JSONSerialization.data(withJSONObject: event, options: [.withoutEscapingSlashes])).base64EncodedString()
    }
}

extension Data {
    var hex: String { map { String(format: "%02x", $0) }.joined() }
}

struct DeviceAccount: Codable, Sendable {
    let origin: URL
    let secret: Data
    var identity: String?
    var email: String?
    var approved: Bool
    var signingKey: DeviceIdentity { get throws { try DeviceIdentity(secret: secret) } }
}

enum DeviceKeychain {
    private static let service = "build.finite.fragment.device"
    private static func query(_ account: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
         kSecAttrAccount as String: account, kSecAttrSynchronizable as String: false]
    }
    static func read(account: String = "active-account-v1") throws -> DeviceAccount? {
        var request = query(account)
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw APIError.keychain(status) }
        return try JSONDecoder().decode(DeviceAccount.self, from: data)
    }
    static func save(_ account: DeviceAccount, account slot: String = "active-account-v1") throws {
        let data = try JSONEncoder().encode(account)
        let attributes: [String: Any] = [kSecValueData as String: data, kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly]
        let status = SecItemUpdate(query(slot) as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var insert = query(slot)
            attributes.forEach { insert[$0.key] = $0.value }
            let added = SecItemAdd(insert as CFDictionary, nil)
            guard added == errSecSuccess else { throw APIError.keychain(added) }
        } else if status != errSecSuccess { throw APIError.keychain(status) }
    }
    static func clear(account: String = "active-account-v1") throws {
        let status = SecItemDelete(query(account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw APIError.keychain(status) }
    }
}

extension Notification.Name { static let finiteAuthorizationFailed = Notification.Name("FiniteAuthorizationFailed") }

// A signed request is never silently redirected with stale authorization or to
// another host. Web navigation has its own one-fragment session and policy.
final class NoSignedRedirects: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}

enum PlatformOrigin {
    static func parse(_ raw: String) throws -> URL {
        guard let url = URL(string: raw.trimmingCharacters(in: .whitespacesAndNewlines)), url.host != nil,
              url.user == nil, url.password == nil, url.query == nil, url.fragment == nil,
              url.path.isEmpty || url.path == "/" else { throw APIError.server("Enter the platform’s HTTPS address, without a path.") }
        var allowed = url.scheme == "https"
        #if DEBUG
        let host = url.host!
        allowed = allowed || (url.scheme == "http" && (host == "localhost" || host == "127.0.0.1" || host == "::1" || host.hasSuffix(".localhost") || host.hasSuffix(".local")))
        #endif
        guard allowed else { throw APIError.server("Use HTTPS for your Finite account.") }
        var parts = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        parts.path = ""
        return parts.url!
    }
    static func same(_ a: URL, _ b: URL) -> Bool {
        func scheme(_ url: URL) -> String? { switch url.scheme { case "wss": "https"; case "ws": "http"; default: url.scheme } }
        return scheme(a) == scheme(b) && a.host?.lowercased() == b.host?.lowercased() && (a.port ?? (scheme(a) == "https" ? 443 : 80)) == (b.port ?? (scheme(b) == "https" ? 443 : 80))
    }
}
