import Foundation
import CryptoKit
import P256K
import Testing
@testable import Fragment

struct DeviceIdentityTests {
    private func fixedKey(_ suffix: UInt8 = 3) throws -> DeviceIdentity { try DeviceIdentity(secret: Data(repeating: 0, count: 31) + Data([suffix])) }
    private func event(_ header: String) throws -> [String: Any] {
        let data = try #require(Data(base64Encoded: String(header.dropFirst(6))))
        return try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
    @Test func signatureBindsExactURLMethodBodyAndClock() throws {
        #expect(Bundle.main.url(forResource: "ThirdPartyNotices", withExtension: "txt") != nil)
        let key = try fixedKey()
        #expect(key.publicKey == "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9")
        let url = URL(string: "https://finite.example/api/f/notes/channels/chat?q=one%20two")!
        let body = Data("{\"text\":\"hello ☀️\"}".utf8)
        let signed = try event(key.authorization(method: "POST", url: url, body: body, at: 1_800_000_000))
        let tags = try #require(signed["tags"] as? [[String]])
        #expect(tags == [["u",url.absoluteString],["method","POST"],["payload",Data(SHA256.hash(data: body)).hex]])
        let preimage = try JSONSerialization.data(withJSONObject: [0,key.publicKey,1_800_000_000,27235,tags,""], options: [.withoutEscapingSlashes])
        #expect(signed["id"] as? String == Data(SHA256.hash(data: preimage)).hex)
        let hex = try #require(signed["sig"] as? String)
        let bytes = stride(from: 0, to: hex.count, by: 2).map { index in UInt8(hex[hex.index(hex.startIndex, offsetBy: index)..<hex.index(hex.startIndex, offsetBy: index + 2)], radix: 16)! }
        let signature = try P256K.Schnorr.SchnorrSignature(dataRepresentation: Data(bytes))
        let publicKey = try P256K.Schnorr.PrivateKey(dataRepresentation: key.secret).xonly
        #expect(publicKey.isValidSignature(signature, for: SHA256.hash(data: preimage)))
        #expect(!publicKey.isValidSignature(signature, for: SHA256.hash(data: Data("changed".utf8))))
    }
    @Test func websocketSignsTheHTTPUpgradeAndCredentialsStayOnPlatform() throws {
        let key = try fixedKey()
        let api = FragmentAPI(baseURL: URL(string: "https://finite.example")!, identity: key)
        let request = try api.authorized(URLRequest(url: URL(string: "wss://finite.example/f/chat/__live")!))
        let tags = try event(#require(request.value(forHTTPHeaderField: "Authorization")))["tags"] as? [[String]]
        #expect(tags == [["u","https://finite.example/f/chat/__live"],["method","GET"]])
        #expect(throws: APIError.self) { try api.authorized(URLRequest(url: URL(string: "https://another.example/api/fragments")!)) }
        #expect(throws: APIError.self) { try api.authorized(URLRequest(url: URL(string: "https://finite.example/api/fragments#secret")!)) }
    }
    @Test func originParsingRejectsCredentialsAndUntrustedTransport() throws {
        for raw in ["https://user:password@finite.example", "https://finite.example/path", "https://finite.example?token=x", "https://finite.example#key", "http://finite.example", "javascript:alert(1)"] {
            #expect(throws: APIError.self) { try PlatformOrigin.parse(raw) }
        }
        #expect(try PlatformOrigin.parse("https://finite.example/").absoluteString == "https://finite.example")
    }
    @Test func keychainSurvivesReloadAndDeletesOnlyItsAccount() throws {
        let slot = "tests-" + UUID().uuidString
        defer { try? DeviceKeychain.clear(account: slot) }
        let key = try fixedKey()
        var stored = DeviceAccount(origin: URL(string: "https://finite.example")!, secret: key.secret, approved: false)
        try DeviceKeychain.save(stored, account: slot)
        #expect(try DeviceKeychain.read(account: slot)?.secret == key.secret)
        stored.approved = true; stored.identity = "npub1person"
        try DeviceKeychain.save(stored, account: slot)
        #expect(try DeviceKeychain.read(account: slot)?.approved == true)
        try DeviceKeychain.clear(account: slot)
        #expect(try DeviceKeychain.read(account: slot) == nil)
    }
    @Test func draftAndAmbiguousPostSurviveRestartWithoutChangingIDOrAccount() throws {
        let root = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let api = FragmentAPI(baseURL: URL(string: "https://finite.example")!, identity: try fixedKey())
        let store = try ChatDraftStore(api: api, fragment: "chat", root: root)
        let post = PendingPost(id: "same-id", text: "An ambiguous send")
        let saved = SavedChat(composer: "Keep this draft", attachments: [], recipients: [], pending: [post])
        try store.save(saved)
        let reloaded = try ChatDraftStore(api: api, fragment: "chat", root: root).load()
        #expect(reloaded?.composer == saved.composer)
        #expect(reloaded?.pending.first?.id == post.id)
        let other = FragmentAPI(baseURL: api.baseURL, identity: try fixedKey(4))
        #expect(try ChatDraftStore(api: other, fragment: "chat", root: root).load() == nil)
        try store.save(SavedChat(composer: "", attachments: [], recipients: [], pending: []))
        #expect(try store.load() == nil)
    }
}
