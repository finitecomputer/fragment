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

    /// One chat's bounds, which editing, saving and loading all hold it to:
    /// a message's words, its attachments, and the sends waiting.
    static let textMax = 32_768
    static let attachmentsMax = 8
    static let pendingMax = 16
    /// A saved file's size at most: the bounds above, with room for JSON's
    /// escapes.
    static let fileMax = 4 * 1_048_576

    /// `text` cut to `textMax` UTF-8 bytes, at a character, and whether it was.
    static func fitted(_ text: String) -> (String, Bool) {
        guard text.utf8.count > textMax else { return (text, false) }
        var kept = "", bytes = 0
        // whole characters: one that would cross the bound is dropped whole
        for character in text {
            bytes += character.utf8.count
            guard bytes <= textMax else { break }
            kept.append(character)
        }
        return (kept, true)
    }

    /// Within the bounds, and whether anything was cut to get there: the
    /// composer to `textMax`, its attachments to the first eight, and the
    /// sends no send could be (no id, too long) and those past sixteen
    /// dropped.
    func fitted() -> (SavedChat, Bool) {
        var chat = self
        let (composer, long) = Self.fitted(composer)
        chat.composer = composer
        chat.attachments = Array(attachments.prefix(Self.attachmentsMax))
        let sendable = pending.filter { !$0.id.isEmpty && $0.text.utf8.count <= Self.textMax && $0.attachments.count <= Self.attachmentsMax }
        chat.pending = Array(sendable.prefix(Self.pendingMax))
        return (chat, long || chat.attachments.count < attachments.count || chat.pending.count < pending.count)
    }
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
        return support.appending(path: LaunchScope.current.supportFolder + "/Accounts/" + scope, directoryHint: .isDirectory)
    }
    /// What this chat saved, within its bounds (`cut`: some of it was not),
    /// or nothing. A file that is no saved chat (another version, too big,
    /// not JSON) is removed, and said so (`unreadable`): a chat always opens.
    func load() throws -> (chat: SavedChat, cut: Bool)? {
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        let size = try file.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
        guard size <= SavedChat.fileMax, let saved = try? JSONDecoder().decode(SavedChat.self, from: Data(contentsOf: file)), saved.version == 1 else {
            try FileManager.default.removeItem(at: file)
            throw ChatDraftStore.unreadable
        }
        return saved.fitted()
    }
    static let unreadable = APIError.server("This chat’s saved draft could not be read, and was cleared.")

    /// Saves `saved` as `load` takes it back: within its bounds. Answers
    /// whether anything was cut to fit (nothing is when its chat keeps to
    /// them as it is edited: `ChatSession`).
    @discardableResult
    func save(_ saved: SavedChat) throws -> Bool {
        let (saved, cut) = saved.fitted()
        if let accountID {
            // A send finishing after disconnect must not recreate erased files.
            guard let active = try DeviceKeychain.read(), active.origin == origin,
                  active.identity == accountID, try active.signingKey.publicKey == publicKey else { return cut }
        }
        if saved.isEmpty {
            if FileManager.default.fileExists(atPath: file.path) { try FileManager.default.removeItem(at: file) }
            return cut
        }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.complete])
        if !FileManager.default.fileExists(atPath: file.path) {
            let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            guard files.count < 32 else { throw APIError.server("Finish or discard a draft in another chat before saving more conversations.") }
        }
        let data = try JSONEncoder().encode(saved)
        // within its bounds a chat fits, but for text of nothing but JSON escapes
        guard data.count <= SavedChat.fileMax else { throw APIError.server("This chat’s draft is too large to keep on this iPhone.") }
        try data.write(to: file, options: [.atomic, .completeFileProtection])
        var excluded = directory
        var values = URLResourceValues(); values.isExcludedFromBackup = true
        try excluded.setResourceValues(values)
        return cut
    }
    static func removeAccount(api: FragmentAPI) throws {
        let path = try accountDirectory(api: api)
        if FileManager.default.fileExists(atPath: path.path) { try FileManager.default.removeItem(at: path) }
    }
}
