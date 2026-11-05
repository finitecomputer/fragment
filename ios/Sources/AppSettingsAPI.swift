import Foundation
import ImageIO
import UIKit
import UniformTypeIdentifiers

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
        var request = URLRequest(url: url("api/f/\(name)/icon"))
        request.httpMethod = "PUT"; request.httpBody = data; request.timeoutInterval = 30
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        let (body, response) = try await self.data(for: request)
        guard let response = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200..<300).contains(response.statusCode) else { throw APIError.answer(body, status: response.statusCode) }
    }

    func makeAppIcons(_ name: String, hint: String) async throws -> AppIconOptions {
        var fields: [String: JSONValue] = [:]
        let clean = hint.trimmingCharacters(in: .whitespacesAndNewlines)
        if !clean.isEmpty { fields["hint"] = .string(clean) }
        return try await request("api/f/\(name)/icon/make", method: "POST", body: .object(fields), timeout: 90)
    }
}

enum AppIconImage {
    /// The server fits any icon it is sent inside 256×256 (docs/api.md,
    /// Icons). Fitting a picked photo here first, without decoding it whole,
    /// only spares the phone sending megabytes the server would shrink.
    static let side = 256
    static func fitted(_ file: URL) throws -> Data {
        let options = [kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceCreateThumbnailWithTransform: true,
                       kCGImageSourceThumbnailMaxPixelSize: side] as CFDictionary
        guard let source = CGImageSourceCreateWithURL(file as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options) else { throw APIError.server("Choose a PNG, JPEG, WebP or GIF image.") }
        let png = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(png, UTType.png.identifier as CFString, 1, nil) else { throw APIError.invalidResponse }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { throw APIError.invalidResponse }
        return png as Data
    }
}

/// One export, as its person sees it (`ExportView`, docs/api.md, Export).
struct ExportView: Decodable, Sendable {
    struct Progress: Decodable, Sendable { let done: Int; let total: Int }
    struct Problem: Decodable, Sendable { let message: String }
    let id: String
    let state: String
    var progress: Progress?
    var size: Int64?
    var error: Problem?
    /// While it is ready: a path on the platform's origin, for its person alone.
    var download: String?
    var said: String {
        switch state {
        case "building": progress.map { "Building (\($0.done) of \($0.total))" } ?? "Building"
        case "ready": size.map { "Ready, " + ByteCountFormatter.string(fromByteCount: $0, countStyle: .file) } ?? "Ready"
        case "failed": "Failed" + (error.map { ": " + $0.message } ?? "")
        case "expired": "Expired"
        default: state.capitalized
        }
    }
}

extension FragmentAPI {
    func export() async throws -> ExportView? {
        struct Status: Decodable { let export: ExportView? }
        let status: Status = try await request("api/export")
        return status.export
    }

    func startExport() async throws -> ExportView { try await request("api/export", method: "POST", body: .object([:])) }

    /// A ready export's archive, downloaded with this iPhone's signature to a
    /// file of its own (`Caches/Exports`), streamed to disk: its path must be
    /// the platform's own export download.
    func download(_ export: ExportView) async throws -> URL {
        guard export.state == "ready", let path = export.download, let url = URL(string: path, relativeTo: baseURL)?.absoluteURL,
              PlatformOrigin.same(url, baseURL), url.path == "/api/export/\(export.id)/download" else { throw APIError.invalidResponse }
        let (temporary, response) = try await session.download(for: authorized(URLRequest(url: url)))
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        if http.statusCode == 401 { authorizationFailed(); throw APIError.unauthorized }
        guard http.statusCode == 200 else { throw APIError.answer(Data(), status: http.statusCode) }
        let folder = try FileManager.default.url(for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: true).appending(path: "Exports", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let file = folder.appending(path: "fragment-export-\(export.id).tar")
        if FileManager.default.fileExists(atPath: file.path) { try FileManager.default.removeItem(at: file) }
        try FileManager.default.moveItem(at: temporary, to: file)
        return file
    }
}

/// The person's export, as Settings follows it (cell/shell/exports.js): its
/// latest view, asked again every `every` while it builds, its archive once
/// downloaded, and a new one made at any time (`POST /api/export` replaces
/// the last). Asks and makes run one after another, each answer applied in
/// the order they were sent, so an older read never lands after a newer
/// one. A downloaded archive is kept with the export it is of: once the
/// export shown is another, it goes (deleted), and one still downloading
/// for an export no longer shown is never kept.
@MainActor @Observable
final class ExportModel {
    let api: FragmentAPI
    let every: Duration
    private(set) var export: ExportView?
    private var archive: (id: String, file: URL)?
    private var pending = 0
    private var queue: Task<Void, Never>?
    var error: String?
    init(api: FragmentAPI, every: Duration = .seconds(2)) { self.api = api; self.every = every }

    /// The archive downloaded for the export shown, while it is ready.
    var file: URL? { archive.flatMap { export?.state == "ready" && $0.id == export?.id ? $0.file : nil } }
    var busy: Bool { pending > 0 }
    var startLabel: String {
        switch export?.state {
        case "building": "Exporting…"
        case "ready": "Export again"
        default: "Export everything"
        }
    }

    /// `step` after every ask or make sent before it.
    private func serially(_ step: @escaping @MainActor () async -> Void) async {
        pending += 1
        let before = queue
        let next = Task { @MainActor in
            await before?.value
            await step()
        }
        queue = next
        await next.value
        pending -= 1
    }
    private func shown(_ view: ExportView?) {
        export = view
        if let kept = archive, kept.id != view?.id || view?.state != "ready" { drop(kept.file) }
    }
    private func drop(_ file: URL) {
        try? FileManager.default.removeItem(at: file)
        if archive?.file == file { archive = nil }
    }

    func refresh() async {
        await serially { [self] in
            do { shown(try await api.export()); error = nil } catch { self.error = error.localizedDescription }
        }
    }
    func start() async {
        await serially { [self] in
            do { shown(try await api.startExport()); error = nil } catch { self.error = error.localizedDescription }
        }
    }
    func download() async {
        guard let export, export.state == "ready" else { return }
        pending += 1; defer { pending -= 1 }
        do {
            let file = try await api.download(export)
            // shown no longer (refreshed to another, or made again): never kept
            guard self.export?.id == export.id, self.export?.state == "ready" else { return drop(file) }
            if let old = archive?.file, old != file { drop(old) }
            archive = (export.id, file)
            error = nil
        } catch { self.error = error.localizedDescription }
    }
    /// While it builds, its view again every `every`, until it is ready (or
    /// failed, or gone), or the screen goes.
    func follow() async {
        while export?.state == "building" {
            do { try await Task.sleep(for: every) } catch { return }
            await refresh()
        }
    }
}
