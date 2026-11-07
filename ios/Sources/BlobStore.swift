import Foundation
import CryptoKit
import Observation
import UIKit
import ImageIO

/// A chat's file as this iPhone keeps it: by the server's origin and the
/// chat it is a blob of (`scope`, a hash of both: no server text in a path),
/// and its own hash.
struct BlobKey: Hashable, Sendable {
    let scope: String
    let sha256: String

    init(api: FragmentAPI, fragment: String, sha256: String) {
        let origin = api.baseURL.absoluteString
        scope = String(sha256Hex(Data("\(origin)|\(fragment)".utf8)).prefix(32))
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
    /// What the files kept take on disk at most, past which the least
    /// recently used go (checked once a launch).
    nonisolated static let diskMax: Int64 = 1_024 * 1_024 * 1_024

    let root: URL
    private var loads: [BlobKey: BlobLoad] = [:]
    private var trimmed = false

    init(root: URL) { self.root = root }

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
            Task { await self.read(attachment, key: key, api: api, fragment: fragment, into: load) }
        }
        if begin { load.begin() }
        trimOnce()
        return load
    }

    /// A file of the person's own, kept as it is put: what they attach shows
    /// from these bytes at once, before and after it is sent. Checked and
    /// written off the main actor (a file may be large).
    @discardableResult
    func keep(_ data: Data, key: BlobKey) async throws -> URL {
        guard let url = path(key) else { throw APIError.invalidResponse }
        try await Task.detached(priority: .userInitiated) {
            guard sha256Hex(data) == key.sha256 else { throw APIError.invalidResponse }
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            if !FileManager.default.fileExists(atPath: url.path) { try data.write(to: url, options: .atomic); Self.seal(url) }
        }.value
        if let held = loads[key] { held.set(.ready(url)) }
        return url
    }

    /// The file as Quick Look, Share and the player take it: named by its
    /// record's name made safe (`SafeFileName`), in a folder of its hash's,
    /// a link to the kept file (no second copy).
    func named(_ attachment: ChatAttachment, file: URL) throws -> URL {
        guard SafeFileName.isHash(attachment.sha256) else { throw APIError.invalidResponse }
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
            return await Task.detached(priority: .userInitiated) {
                guard let data = try? Data(contentsOf: file, options: .mappedIfSafe), let rewrapped = try? WebMOpus.ogg(from: data) else { return nil }
                do { try rewrapped.write(to: ogg, options: .atomic); BlobStore.seal(ogg); return ogg } catch { return nil }
            }.value
        }
    }

    func removeAll() {
        loads.removeAll()
        try? FileManager.default.removeItem(at: root)
    }

    private func read(_ attachment: ChatAttachment, key: BlobKey, api: FragmentAPI, fragment: String, into load: BlobLoad) async {
        do {
            guard let target = path(key), attachment.size > 0 else { throw APIError.invalidResponse }
            let progress = DownloadProgress(expected: Int64(attachment.size)) { fraction in
                Task { @MainActor in if case .loading = load.state { load.set(.loading(fraction)) } }
            }
            let request = try api.authorized(URLRequest(url: api.url("api/f/\(fragment)/blobs/\(attachment.sha256)")))
            let (temporary, response) = try await api.session.download(for: request, delegate: progress)
            defer { try? FileManager.default.removeItem(at: temporary) }
            if (response as? HTTPURLResponse)?.statusCode == 401 { api.authorizationFailed(); throw APIError.unauthorized }
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw APIError.server("This file could not be read.") }
            // the record names its size and hash: a file that is not both is not it
            let sha256 = attachment.sha256
            let size = attachment.size
            let whole = try await Task.detached(priority: .utility) { try Self.check(temporary, size: size, sha256: sha256) }.value
            guard whole else { throw APIError.server("This file did not arrive whole.") }
            try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
            if FileManager.default.fileExists(atPath: target.path) { try FileManager.default.removeItem(at: temporary) }
            else { try FileManager.default.moveItem(at: temporary, to: target); Self.seal(target) }
            load.set(.ready(target))
        } catch is CancellationError {
            load.set(.idle)
        } catch let error as URLError where error.code == .cancelled {
            load.set(.failed("This file was larger than its message says."))
        } catch {
            load.set(.failed(error.localizedDescription))
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

    /// Past `diskMax`, the files least recently used go, off the main actor.
    private func trimOnce() {
        guard !trimmed else { return }
        trimmed = true
        let root = root
        Task.detached(priority: .background) { Self.trim(root, max: Self.diskMax) }
    }
    nonisolated static func trim(_ root: URL, max: Int64) {
        let keys: [URLResourceKey] = [.isRegularFileKey, .totalFileAllocatedSizeKey, .contentAccessDateKey]
        guard let walk = FileManager.default.enumerator(at: root, includingPropertiesForKeys: keys) else { return }
        var files: [(url: URL, size: Int64, used: Date)] = []
        for case let url as URL in walk {
            guard let values = try? url.resourceValues(forKeys: Set(keys)), values.isRegularFile == true else { continue }
            files.append((url, Int64(values.totalFileAllocatedSize ?? 0), values.contentAccessDate ?? .distantPast))
        }
        var total = files.reduce(0) { $0 + $1.size }
        for file in files.sorted(by: { $0.used < $1.used }) where total > max {
            try? FileManager.default.removeItem(at: file.url)
            total -= file.size
        }
    }
}

/// A download's progress, to its read; and its end, past the size its
/// record names (a server's more is never read).
final class DownloadProgress: NSObject, URLSessionDownloadDelegate, Sendable {
    let expected: Int64
    let report: @Sendable (Double) -> Void
    init(expected: Int64, report: @escaping @Sendable (Double) -> Void) {
        self.expected = expected
        self.report = report
    }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard totalBytesWritten <= expected, totalBytesExpectedToWrite <= expected || totalBytesExpectedToWrite == NSURLSessionTransferSizeUnknown else {
            downloadTask.cancel(); return
        }
        report(Double(totalBytesWritten) / Double(max(1, expected)))
    }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {}
}
