import Foundation
import CryptoKit
import Synchronization
import Observation
import UIKit
import ImageIO

/// A chat's file as this iPhone keeps it: by the account (its server and
/// this iPhone's key for it), the chat it is a blob of (`scope`, a hash of
/// them all: no server text in a path, and no file one account read is
/// another's), and its own hash.
struct BlobKey: Hashable, Sendable {
    let scope: String
    let sha256: String

    init(api: FragmentAPI, fragment: String, sha256: String) {
        let account = "\(api.baseURL.absoluteString)|\(api.identity.publicKey)"
        scope = String(sha256Hex(Data("\(account)|\(fragment)".utf8)).prefix(32))
        self.sha256 = sha256
    }
    /// A key for the image cache, which keeps pictures by URL.
    func imageKey(_ pixels: Int) -> URL { URL(string: "fragment-blob://\(scope)/\(sha256)?px=\(pixels)")! }
}

/// A file's read, observed by what shows it: under way (with how much is
/// in, when its size is known), done (its file), or failed (why, and a retry).
@MainActor @Observable
final class BlobLoad {
    enum State: Equatable { case idle, loading(Double), ready(URL), failed(String) }
    private(set) var state: State
    @ObservationIgnored fileprivate var start: (() -> Void)?

    fileprivate init(_ state: State) { self.state = state }
    fileprivate func set(_ next: State) { if next != state { state = next } }

    var file: URL? { if case .ready(let url) = state { url } else { nil } }
    /// Reads it, if it is not read or being read.
    func begin() {
        switch state {
        case .idle, .failed: start?()
        case .loading, .ready: break
        }
    }
}

/// Every chat file this iPhone has read, on disk under Caches (the system
/// may empty it; then a file is read again): one read per file, shared by
/// every view of it, never held whole in memory. A file read is the size
/// its record names and the hash it is named by, or it is refused: the
/// record names the bound (no bound of the app's own), and a blob is never
/// read past it. Emptied when the account changes, as `ImageCache` is.
@MainActor
final class BlobStore {
    static let shared = BlobStore(root: FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appending(path: "attachments", directoryHint: .isDirectory))
    /// A preview (a picture, a voice, a PDF's page) up to this size reads by
    /// itself as it comes on screen; a larger one waits for a tap.
    static let previewBytesMax = 20 * 1_024 * 1_024
    /// What the files kept take on disk at most, by default: past it, after
    /// each file written, the least recently used go (`trim`).
    nonisolated static let diskMaxDefault: Int64 = 1_024 * 1_024 * 1_024
    /// Reads held for views at most; past it, the least recently used that
    /// are not under way are let go (a view keeps its own).
    static let loadsMax = 512

    let root: URL
    let diskMax: Int64
    /// The most recently used files a trim never takes, whatever the bound:
    /// those likely on screen.
    let keptRecent: Int
    private var loads: [BlobKey: BlobLoad] = [:]
    /// When each file was last asked for, this launch (bounded with `loads`).
    private var used: [BlobKey: Date] = [:]
    private var trimming: Task<Void, Never>?
    private var trimAgain = false
    /// Reads under way, each ended when the store is emptied.
    private var reading: [BlobKey: Task<Void, Never>] = [:]
    /// Reads ended by an emptying, until they have stopped (`settled`).
    private var ending: [Task<Void, Never>] = []
    /// How many times the store was emptied (`removeAll`): work begun
    /// before is never put in place after (another account's).
    private(set) var generation = 0

    init(root: URL, diskMax: Int64 = BlobStore.diskMaxDefault, keptRecent: Int = 16) {
        self.root = root
        self.diskMax = diskMax
        self.keptRecent = keptRecent
    }

    /// Where a kept file is: named by its hash alone, in its scope's folder.
    nonisolated func path(_ key: BlobKey) -> URL? {
        guard SafeFileName.isHash(key.sha256), key.scope.utf8.count == 32, SafeFileName.isHash(key.scope + String(repeating: "0", count: 32)) else { return nil }
        return root.appending(path: key.scope, directoryHint: .isDirectory).appending(path: key.sha256)
    }

    func cached(_ key: BlobKey) -> URL? {
        guard let url = path(key), FileManager.default.fileExists(atPath: url.path) else { return nil }
        return url
    }

    /// The read of an attachment's file, shared: begun now if `begin`.
    func load(_ attachment: ChatAttachment, api: FragmentAPI, fragment: String, begin: Bool) -> BlobLoad {
        let key = BlobKey(api: api, fragment: fragment, sha256: attachment.sha256)
        let load: BlobLoad
        if let held = loads[key] {
            load = held
            // emptied meanwhile by the system: read again
            if let file = held.file, !FileManager.default.fileExists(atPath: file.path) { held.set(.idle) }
        } else {
            load = BlobLoad(cached(key).map { .ready($0) } ?? .idle)
            loads[key] = load
        }
        load.start = { [weak self, weak load] in
            guard let self, let load else { return }
            load.set(.loading(0))
            self.reading[key] = Task { await self.read(attachment, key: key, api: api, fragment: fragment, into: load) }
        }
        used[key] = Date()
        if begin { load.begin() }
        if loads.count > Self.loadsMax { forgetOldLoads() }
        return load
    }

    /// A file of the person's own, kept as it is put: what they attach shows
    /// from these bytes at once, before and after it is sent. Checked and
    /// written off the main actor (a file may be large).
    @discardableResult
    func keep(_ data: Data, key: BlobKey) async throws -> URL {
        guard let url = path(key) else { throw APIError.invalidResponse }
        let started = generation
        let staged = try await Task.detached(priority: .userInitiated) {
            guard sha256Hex(data) == key.sha256 else { throw APIError.invalidResponse }
            return try Self.stage(data)
        }.value
        try commit(staged, to: url, since: started)
        if let held = loads[key] { held.set(.ready(url)) }
        used[key] = Date()
        scheduleTrim()
        return url
    }

    /// The file as Quick Look, Share and the player take it: named by its
    /// record's name made safe (`SafeFileName`), in a folder of its hash's,
    /// a link to the kept file (no second copy).
    func named(_ attachment: ChatAttachment, file: URL) throws -> URL {
        guard SafeFileName.isHash(attachment.sha256), FileManager.default.fileExists(atPath: file.path) else { throw APIError.invalidResponse }
        let folder = file.deletingLastPathComponent().appending(path: "named", directoryHint: .isDirectory)
            .appending(path: attachment.sha256, directoryHint: .isDirectory)
        let target = folder.appending(path: SafeFileName.of(attachment.name, type: attachment.type))
        guard target.deletingLastPathComponent().standardizedFileURL == folder.standardizedFileURL else { throw APIError.invalidResponse }
        if FileManager.default.fileExists(atPath: target.path) { return target }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        do { try FileManager.default.linkItem(at: file, to: target) }
        catch { try FileManager.default.copyItem(at: file, to: target) }
        return target
    }

    /// A sound as iOS plays it: as it is, or Opus in WebM rewrapped as Ogg
    /// (`WebMOpus`), kept beside it. Nil for one that cannot play here.
    nonisolated func playable(_ attachment: ChatAttachment, file: URL) async -> URL? {
        let head = (try? FileHandle(forReadingFrom: file)).flatMap { handle in defer { try? handle.close() }; return try? handle.read(upToCount: 64) } ?? Data()
        switch AudioContainer.of(head) {
        case .unplayable: return nil
        case .native: return await MainActor.run { try? named(attachment, file: file) }
        case .webm:
            let ogg = file.appendingPathExtension("ogg")
            if FileManager.default.fileExists(atPath: ogg.path) { return ogg }
            let started = await generation
            let staged = await Task.detached(priority: .userInitiated) { () -> URL? in
                guard let data = try? Data(contentsOf: file, options: .mappedIfSafe), let wrapped = try? WebMOpus.ogg(from: data) else { return nil }
                return try? Self.stage(wrapped)
            }.value
            guard let staged else { return nil }
            return await MainActor.run {
                defer { scheduleTrim() }
                return try? commit(staged, to: ogg, since: started)
            }
        }
    }

    /// Emptied (the account changed): reads under way end, and nothing
    /// begun before is put in place after (`commit`).
    func removeAll() {
        generation += 1
        for task in reading.values { task.cancel() }
        ending += reading.values
        reading.removeAll()
        loads.removeAll()
        used.removeAll()
        try? FileManager.default.removeItem(at: root)
    }

    /// Where a file made off the main actor waits to be put in place: never
    /// in the store, so a store emptied meanwhile is never filled again.
    nonisolated static func stage(_ data: Data) throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appending(path: "blob-staging", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appending(path: UUID().uuidString)
        try data.write(to: url, options: .atomic)
        return url
    }

    /// A file made off the main actor (`staged`), put in place here, unless
    /// the store was emptied since its work began (another account's): then
    /// it goes.
    @discardableResult
    private func commit(_ staged: URL, to target: URL, since started: Int) throws -> URL {
        guard generation == started else {
            try? FileManager.default.removeItem(at: staged)
            throw CancellationError()
        }
        try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
        if FileManager.default.fileExists(atPath: target.path) { try? FileManager.default.removeItem(at: staged) }
        else { try FileManager.default.moveItem(at: staged, to: target); Self.seal(target) }
        return target
    }

    private func read(_ attachment: ChatAttachment, key: BlobKey, api: FragmentAPI, fragment: String, into load: BlobLoad) async {
        let started = generation
        var overran: DownloadProgress?
        defer { if generation == started { reading[key] = nil } }
        do {
            guard let target = path(key), attachment.size > 0 else { throw APIError.invalidResponse }
            let progress = DownloadProgress(expected: Int64(attachment.size)) { fraction in
                Task { @MainActor in if case .loading = load.state { load.set(.loading(fraction)) } }
            }
            let request = try api.authorized(URLRequest(url: api.url("api/f/\(fragment)/blobs/\(attachment.sha256)")))
            overran = progress
            let (temporary, response) = try await api.session.download(for: request, delegate: progress)
            defer { try? FileManager.default.removeItem(at: temporary) }
            if (response as? HTTPURLResponse)?.statusCode == 401 { api.authorizationFailed(); throw APIError.unauthorized }
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw APIError.server("This file could not be read.") }
            // the record names its size and hash: a file that is not both is not it
            let sha256 = attachment.sha256
            let size = attachment.size
            let whole = try await Task.detached(priority: .utility) { try Self.check(temporary, size: size, sha256: sha256) }.value
            guard whole else { throw APIError.server("This file did not arrive whole.") }
            try commit(temporary, to: target, since: started)
            load.set(.ready(target))
            scheduleTrim()
        } catch {
            // emptied meanwhile: what it read is not kept, nor said
            guard generation == started else { return }
            if overran?.overran == true { load.set(.failed("This file was larger than its message says.")) }
            else if error is CancellationError || (error as? URLError)?.code == .cancelled { load.set(.idle) }
            else { load.set(.failed(error.localizedDescription)) }
        }
    }

    /// A kept file made read-only: it is the bytes its hash names, and its
    /// named links (`named`) are it.
    nonisolated static func seal(_ url: URL) {
        try? FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: url.path)
    }

    /// Whether a file is `size` bytes whose SHA-256 is `sha256`, read a MiB at a time.
    nonisolated static func check(_ file: URL, size: Int, sha256: String) throws -> Bool {
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        guard (attributes[.size] as? NSNumber)?.intValue == size else { return false }
        let handle = try FileHandle(forReadingFrom: file)
        defer { try? handle.close() }
        var hasher = SHA256()
        while let chunk = try handle.read(upToCount: 1_048_576), !chunk.isEmpty { hasher.update(data: chunk) }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined() == sha256
    }

    /// A trim, after a file is written: one at a time, and once more if
    /// another write came meanwhile.
    private func scheduleTrim() {
        guard trimming == nil else { trimAgain = true; return }
        trimming = Task { [weak self] in
            while let self {
                self.trimAgain = false
                await self.trimNow()
                guard self.trimAgain else { break }
            }
            self?.trimming = nil
        }
    }

    /// Waits for a trim under way, and for reads an emptying ended to stop.
    func settled() async {
        await trimming?.value
        let ended = ending
        ending.removeAll()
        for task in ended { await task.value }
    }

    /// The files past `diskMax` go, the least recently used first (as asked
    /// this launch, else as written), never a file being read nor one of the
    /// `keptRecent` last asked for. A file's group (it, its rewrap, its
    /// named links) goes whole, and is counted once however many names it has.
    func trimNow() async {
        let recent = used.sorted { $0.value > $1.value }
        let kept = Set(recent.prefix(keptRecent).map { "\($0.key.scope)/\($0.key.sha256)" })
            .union(loads.filter { if case .loading = $0.value.state { true } else { false } }.map { "\($0.key.scope)/\($0.key.sha256)" })
        let lastUse = Dictionary(uniqueKeysWithValues: used.map { ("\($0.key.scope)/\($0.key.sha256)", $0.value) })
        let root = root, max = diskMax
        let removed = await Task.detached(priority: .background) { Self.trim(root, max: max, lastUse: lastUse, kept: kept) }.value
        for name in removed {
            let parts = name.split(separator: "/")
            guard parts.count == 2 else { continue }
            let key = loads.keys.first { $0.scope == parts[0] && $0.sha256 == parts[1] }
            if let key { loads[key]?.set(.idle); loads[key] = nil; used[key] = nil }
        }
    }

    /// The reads held past `loadsMax`: the least recently used not under way go.
    private func forgetOldLoads() {
        let idle = loads.filter { if case .loading = $0.value.state { false } else { true } }.keys
        let oldest = idle.sorted { (used[$0] ?? .distantPast) < (used[$1] ?? .distantPast) }
        for key in oldest.prefix(loads.count - Self.loadsMax / 2) { loads[key] = nil; used[key] = nil }
    }

    /// What `trimNow` removes: the groups (`<scope>/<sha256>`) taken, least
    /// recently used first, until what is left is within `max`.
    nonisolated static func trim(_ root: URL, max: Int64, lastUse: [String: Date], kept: Set<String>) -> [String] {
        let keys: Set<URLResourceKey> = [.isRegularFileKey, .totalFileAllocatedSizeKey, .contentModificationDateKey, .fileResourceIdentifierKey]
        struct Group { var bytes: Int64 = 0; var written: Date = .distantPast; var paths: [URL] = [] }
        var groups: [String: Group] = [:]
        var counted = Set<AnyHashable>()
        func add(_ file: URL, to name: String, main: Bool) {
            guard let values = try? file.resourceValues(forKeys: keys), values.isRegularFile == true else { return }
            // a named link is the kept file: its bytes count once
            if let id = values.fileResourceIdentifier as? NSObject {
                guard counted.insert(AnyHashable(id)).inserted else { return }
            }
            groups[name, default: Group()].bytes += Int64(values.totalFileAllocatedSize ?? 0)
            if main { groups[name, default: Group()].written = values.contentModificationDate ?? .distantPast }
        }
        let manager = FileManager.default
        for scope in (try? manager.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? [] {
            let scopeName = scope.lastPathComponent
            guard scopeName.utf8.count == 32, SafeFileName.isHash(scopeName + String(repeating: "0", count: 32)) else { continue }
            for entry in (try? manager.contentsOfDirectory(at: scope, includingPropertiesForKeys: nil)) ?? [] {
                let name = entry.lastPathComponent
                let sha = name.hasSuffix(".ogg") ? String(name.dropLast(4)) : name
                if SafeFileName.isHash(sha) {
                    add(entry, to: "\(scopeName)/\(sha)", main: sha == name)
                    groups["\(scopeName)/\(sha)", default: Group()].paths.append(entry)
                } else if name == "named" {
                    for folder in (try? manager.contentsOfDirectory(at: entry, includingPropertiesForKeys: nil)) ?? [] where SafeFileName.isHash(folder.lastPathComponent) {
                        let group = "\(scopeName)/\(folder.lastPathComponent)"
                        for file in (try? manager.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? [] { add(file, to: group, main: false) }
                        groups[group, default: Group()].paths.append(folder)
                    }
                }
            }
        }
        var total = groups.values.reduce(0) { $0 + $1.bytes }
        guard total > max else { return [] }
        var removed: [String] = []
        let order = groups.sorted { (lastUse[$0.key] ?? $0.value.written) < (lastUse[$1.key] ?? $1.value.written) }
        for (name, group) in order where total > max && !kept.contains(name) {
            for path in group.paths { try? manager.removeItem(at: path) }
            total -= group.bytes
            removed.append(name)
        }
        return removed
    }
}

/// A download's progress, to its read; and its end, past the size its
/// record names (a server's more is never read).
final class DownloadProgress: NSObject, URLSessionDownloadDelegate, Sendable {
    let expected: Int64
    let report: @Sendable (Double) -> Void
    private let past = Mutex(false)
    /// Whether it was ended for running past `expected`.
    var overran: Bool { past.withLock { $0 } }
    init(expected: Int64, report: @escaping @Sendable (Double) -> Void) {
        self.expected = expected
        self.report = report
    }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard totalBytesWritten <= expected, totalBytesExpectedToWrite <= expected || totalBytesExpectedToWrite == NSURLSessionTransferSizeUnknown else {
            past.withLock { $0 = true }
            downloadTask.cancel(); return
        }
        report(Double(totalBytesWritten) / Double(max(1, expected)))
    }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {}
}
