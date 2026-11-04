import Foundation
import CryptoKit

struct SavedChat: Codable {
    var version = 1
    var composer: String
    var attachments: [ChatAttachment]
    var replyTo: ChannelRecord?
    var recipients: [String]
    var pending: [PendingPost]
    var isEmpty: Bool { composer.isEmpty && attachments.isEmpty && pending.isEmpty && replyTo == nil }
}

struct ChatDraftStore {
    let directory: URL
    let file: URL
    private let origin: URL
    private let publicKey: String
    private let accountID: String?
    init(api: FragmentAPI, fragment: String, root: URL? = nil) throws {
        origin = api.baseURL; publicKey = api.identity.publicKey; accountID = api.accountID
        directory = try Self.accountDirectory(api: api, root: root).appending(path: "Chats", directoryHint: .isDirectory)
        file = directory.appending(path: Data(SHA256.hash(data: Data(fragment.utf8))).hex + ".json")
    }
    private static func accountDirectory(api: FragmentAPI, root: URL? = nil) throws -> URL {
        let support = try root ?? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let scope = Data(SHA256.hash(data: Data((api.baseURL.absoluteString + "|" + (api.accountID ?? api.identity.publicKey)).utf8))).hex
        return support.appending(path: "Finite/Accounts/" + scope, directoryHint: .isDirectory)
    }
    func load() throws -> SavedChat? {
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        guard (try file.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0) <= 1_048_576 else { throw APIError.limitExceeded }
        let saved = try JSONDecoder().decode(SavedChat.self, from: Data(contentsOf: file))
        guard saved.version == 1, saved.composer.utf8.count <= 32_768, saved.pending.count <= 16,
              saved.attachments.count <= 8, saved.pending.allSatisfy({ !$0.id.isEmpty && $0.text.utf8.count <= 32_768 && $0.attachments.count <= 8 }) else { throw APIError.invalidResponse }
        return saved
    }
    func save(_ saved: SavedChat) throws {
        if let accountID {
            // A send finishing after disconnect must not recreate erased files.
            guard let active = try DeviceKeychain.read(), active.origin == origin,
                  active.identity == accountID, try active.signingKey.publicKey == publicKey else { return }
        }
        if saved.isEmpty {
            if FileManager.default.fileExists(atPath: file.path) { try FileManager.default.removeItem(at: file) }
            return
        }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.complete])
        if !FileManager.default.fileExists(atPath: file.path) {
            let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            guard files.count < 32 else { throw APIError.server("Finish or discard a draft in another chat before saving more conversations.") }
        }
        let data = try JSONEncoder().encode(saved)
        guard data.count <= 1_048_576 else { throw APIError.limitExceeded }
        try data.write(to: file, options: [.atomic, .completeFileProtection])
        var excluded = directory
        var values = URLResourceValues(); values.isExcludedFromBackup = true
        try excluded.setResourceValues(values)
    }
    static func removeAccount(api: FragmentAPI) throws {
        let path = try accountDirectory(api: api)
        if FileManager.default.fileExists(atPath: path.path) { try FileManager.default.removeItem(at: path) }
    }
}
