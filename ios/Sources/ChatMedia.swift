import SwiftUI
import CryptoKit
import AVFoundation
import UniformTypeIdentifiers
import ImageIO
import Synchronization

struct ChatAttachment: Codable, Identifiable, Equatable, Sendable {
    let sha256: String
    let name: String
    let type: String
    let size: Int
    var id: String { sha256 }
    var json: JSONValue { .object(["sha256": .string(sha256), "name": .string(name), "type": .string(type), "size": .number(Double(size))]) }
}

/// What one message carries, at most: the platform's own (`limits` on
/// `GET /api/fragments`, docs/api.md), which the web's chat reads too.
struct UploadLimits: Decodable, Equatable, Sendable {
    let uploadMaxBytes: Int
    let filesPerMessage: Int
    /// A platform older than `limits` names none: what it took then.
    /// Gone once production names them (docs/technical-debt-ledger.md).
    static let olderPlatform = UploadLimits(uploadMaxBytes: 25 * 1_024 * 1_024, filesPerMessage: 8)
    /// Why a file is refused, its bound in the platform's words ("100 MB").
    var sizeRefusal: String { "Files must be between 1 byte and \(ByteCountFormatter.string(fromByteCount: Int64(uploadMaxBytes), countStyle: .file))." }
    var countRefusal: String { "Attach at most \(filesPerMessage) files per message." }
}

extension ChatAttachment {
    private static let held = Mutex(UploadLimits.olderPlatform)
    /// The platform's limits, as its list last named them (`FragmentAPI.fragments`).
    static var limits: UploadLimits {
        get { held.withLock { $0 } }
        set { held.withLock { $0 = newValue } }
    }
    /// One attachment's bytes, at most: it is read whole into memory.
    static var bytesMax: Int { limits.uploadMaxBytes }
    /// Files a message carries, at most.
    static var countMax: Int { limits.filesPerMessage }
}

/// A file about to be attached, read whole: its bytes, name and type.
struct OutgoingFile: Equatable, Sendable {
    let data: Data
    let name: String
    let type: String
}

/// What the composer attaches, as the web's chat takes it
/// (templates/chat/site/chat.js `addFiles`): any file, from 1 byte to
/// `ChatAttachment.bytesMax`, at most `countMax` a message. A picture the
/// web shows as one (media.js `IMAGE`: PNG, JPEG, WebP, GIF) goes as it is;
/// any other picture (a photo's HEIC) as JPEG, which the web and agents read,
/// its metadata (orientation and all) carried over as the web's upload
/// carries a file's. Pictures are converted off the main actor (callers run
/// these in a task of their own), within `pixelsMax` read before decoding.
nonisolated enum AttachmentInput {
    static let shownImages: [UTType] = [.png, .jpeg, .webP, .gif]
    static let jpegQuality = 0.9
    /// Pixels a picture converted holds at most, read from its header before
    /// any decoding: a bound on the memory a conversion takes.
    static let pixelsMax = 100_000_000

    /// A file as picked, its type from its name's extension.
    static func file(_ data: Data, name: String, type: UTType?) throws -> OutgoingFile {
        try bounded(OutgoingFile(data: data, name: name, type: type?.preferredMIMEType ?? "application/octet-stream"))
    }

    /// A picture (from Photos, or pasted): as it is if the web shows its
    /// type, else as JPEG, its name's extension to match.
    static func image(_ data: Data, type: UTType, name: String) throws -> OutgoingFile {
        if let shown = shownImages.first(where: { type.conforms(to: $0) }), let mime = shown.preferredMIMEType {
            return try bounded(OutgoingFile(data: data, name: named(name, shown), type: mime))
        }
        guard type.conforms(to: .image), !data.isEmpty, data.count <= 4 * ChatAttachment.bytesMax,
              let source = CGImageSourceCreateWithData(data as CFData, nil), CGImageSourceGetCount(source) > 0,
              let header = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = header[kCGImagePropertyPixelWidth] as? Int, let height = header[kCGImagePropertyPixelHeight] as? Int else {
            throw APIError.server("This picture could not be read.")
        }
        guard width * height <= pixelsMax else { throw APIError.server("This picture is too large to attach.") }
        let jpeg = NSMutableData()
        guard let out = CGImageDestinationCreateWithData(jpeg, UTType.jpeg.identifier as CFString, 1, nil) else {
            throw APIError.server("This picture could not be read.")
        }
        CGImageDestinationAddImageFromSource(out, source, 0, [kCGImageDestinationLossyCompressionQuality: jpegQuality] as CFDictionary)
        guard CGImageDestinationFinalize(out) else { throw APIError.server("This picture could not be read.") }
        return try bounded(OutgoingFile(data: jpeg as Data, name: named(name, .jpeg), type: "image/jpeg"))
    }

    /// A pasteboard's pictures, read on the main actor (where a pasteboard
    /// is read) as they are held: for each item its best representation, a
    /// type the web shows first, then any picture's data, then an image
    /// object. At most `countMax`. `pasted` converts them, off it.
    @MainActor static func snapshot(_ pasteboard: UIPasteboard) -> [PastedPicture] {
        pasteboard.items.prefix(ChatAttachment.countMax).compactMap { item in
            let types = item.keys.compactMap { UTType($0) }.filter { $0.conforms(to: .image) }.sorted { rank($0) < rank($1) }
            for type in types {
                if let data = item[type.identifier] as? Data { return PastedPicture(data: data, type: type, image: nil) }
            }
            for type in types {
                if let image = item[type.identifier] as? UIImage { return PastedPicture(data: nil, type: nil, image: image) }
            }
            return nil
        }
    }

    /// The pictures pasted, each as `image` makes it (an image object as
    /// PNG, or JPEG past the bound); one that cannot be made is left out.
    static func pasted(_ pictures: [PastedPicture]) -> [OutgoingFile] {
        pictures.enumerated().compactMap { index, picture in
            let name = "pasted-image-\(index + 1)"
            if let data = picture.data, let type = picture.type { return try? image(data, type: type, name: name) }
            if let object = picture.image { return try? fromImage(object, name: name) }
            return nil
        }
    }

    static func fromImage(_ picture: UIImage, name: String) throws -> OutgoingFile {
        guard picture.size.width * picture.scale * picture.size.height * picture.scale <= CGFloat(pixelsMax) else {
            throw APIError.server("This picture is too large to attach.")
        }
        if let png = picture.pngData(), png.count <= ChatAttachment.bytesMax { return try image(png, type: .png, name: name) }
        guard let jpeg = picture.jpegData(compressionQuality: jpegQuality) else { throw APIError.server("This picture could not be read.") }
        return try image(jpeg, type: .jpeg, name: name)
    }

    private static func rank(_ type: UTType) -> Int { shownImages.firstIndex { type.conforms(to: $0) } ?? shownImages.count }
    private static func named(_ name: String, _ type: UTType) -> String {
        // a photo's usual name, not UTType's "jpeg"
        guard let ext = type == .jpeg ? "jpg" : type.preferredFilenameExtension else { return name }
        let base = (name as NSString).deletingPathExtension
        return (base.isEmpty ? name : base) + "." + ext
    }
    private static func bounded(_ file: OutgoingFile) throws -> OutgoingFile {
        let limits = ChatAttachment.limits
        guard !file.data.isEmpty, file.data.count <= limits.uploadMaxBytes else { throw APIError.server(limits.sizeRefusal) }
        return file
    }
}

/// A picture on the pasteboard, as it was held: its data and type, or an
/// image object.
struct PastedPicture: @unchecked Sendable {
    let data: Data?
    let type: UTType?
    /// UIImage is immutable, and read here only to be encoded.
    let image: UIImage?
}

extension FragmentAPI {
    func upload(_ data: Data, name: String, type: String, to fragment: String) async throws -> ChatAttachment {
        let hash = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        var request = URLRequest(url: url("api/f/\(fragment)/blobs/\(hash)"))
        request.httpMethod = "PUT"; request.httpBody = data; request.timeoutInterval = 60
        request.setValue(type, forHTTPHeaderField: "Content-Type")
        request.setValue(String(data.count), forHTTPHeaderField: "Content-Length")
        let (body, response) = try await self.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else { throw APIError.answer(body, status: http.statusCode) }
        return ChatAttachment(sha256: hash, name: name, type: type, size: data.count)
    }
}

@MainActor @Observable
final class VoiceMemo {
    private var recorder: AVAudioRecorder?
    private var deadline: Task<Void, Never>?
    private var url: URL?
    var recording = false
    var error: String?
    func start(finished: @escaping @MainActor (URL) -> Void) async {
        guard await AVAudioApplication.requestRecordPermission() else { error = "Allow microphone access in iOS Settings to record a voice memo."; return }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker])
            try session.setActive(true)
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("voice-\(UUID().uuidString).m4a")
            let recorder = try AVAudioRecorder(url: url, settings: [AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 44_100, AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 96_000])
            guard recorder.record() else { throw APIError.server("The microphone could not start recording.") }
            self.recorder = recorder; self.url = url; recording = true
            // Voice memos share the web's five-minute bound, including an unattended recording.
            deadline = Task { do { try await Task.sleep(for: .seconds(300)); if let url = stop() { finished(url) } } catch {} }
        } catch { self.error = error.localizedDescription }
    }
    func stop() -> URL? {
        deadline?.cancel(); deadline = nil
        recorder?.stop(); recorder = nil; recording = false
        do { try AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation) }
        catch { self.error = error.localizedDescription }
        defer { url = nil }
        return url
    }
    func cancel() {
        if let url = stop() { do { try FileManager.default.removeItem(at: url) } catch { self.error = error.localizedDescription } }
    }
}

struct AttachmentView: View {
    let attachment: ChatAttachment
    let api: FragmentAPI
    let fragment: String
    @State private var downloaded: URL?
    @State private var loading = false
    @State private var error: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let downloaded {
                if attachment.type.hasPrefix("audio/") { AudioMemoPlayer(url: downloaded) }
                ShareLink(item: downloaded) { Label(attachment.name, systemImage: "doc") }
            } else {
                Button { Task { await download() } } label: { Label(attachment.name, systemImage: attachment.type.hasPrefix("audio/") ? "waveform" : "arrow.down.doc") }.disabled(loading)
            }
            if loading { ProgressView() }
            if let error { Text(error).font(.caption).foregroundStyle(.secondary) }
        }.padding(12).background(.quaternary, in: RoundedRectangle(cornerRadius: 12))
    }
    private func download() async {
        loading = true; defer { loading = false }
        do {
            let (data, response) = try await api.data(for: URLRequest(url: api.url("api/f/\(fragment)/blobs/\(attachment.sha256)")))
            // a blob is as large as its record names it (a computer's up to
            // `BLOB_MAX_BYTES`), read whole by then: no bound of the app's here
            guard (response as? HTTPURLResponse)?.statusCode == 200, data.count == attachment.size else { throw APIError.invalidResponse }
            // The server's name is display-only; never let it choose a filesystem path.
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(attachment.sha256, isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let target = directory.appendingPathComponent(URL(fileURLWithPath: attachment.name).lastPathComponent)
            try data.write(to: target, options: .atomic); downloaded = target
        } catch { self.error = error.localizedDescription }
    }
}

private struct AudioMemoPlayer: View {
    let url: URL
    @State private var player: AVAudioPlayer?
    @State private var error: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
        HStack {
            Button("Play voice memo", systemImage: "play.fill") {
                do { player = try AVAudioPlayer(contentsOf: url); player?.play() } catch { self.error = error.localizedDescription; player = nil }
            }
            Button("Stop", systemImage: "stop.fill") { player?.stop() }
        }
        if let error { Text(error).font(.caption).foregroundStyle(.secondary) }
        }.onDisappear { player?.stop() }
    }
}
