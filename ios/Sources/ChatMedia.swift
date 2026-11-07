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
    /// A file put as a blob of `fragment`, by its hash; `progress` hears
    /// the share of it sent.
    func upload(_ data: Data, name: String, type: String, to fragment: String, progress: (@Sendable (Double) -> Void)? = nil) async throws -> ChatAttachment {
        let hash = sha256Hex(data)
        var request = URLRequest(url: url("api/f/\(fragment)/blobs/\(hash)"))
        request.httpMethod = "PUT"; request.httpBody = data; request.timeoutInterval = 60
        request.setValue(type, forHTTPHeaderField: "Content-Type")
        request.setValue(String(data.count), forHTTPHeaderField: "Content-Length")
        // signed over its body, then sent as an upload task's, which reports its progress
        var signed = try authorized(request)
        signed.httpBody = nil
        let (body, response) = try await session.upload(for: signed, from: data, delegate: progress.map(UploadProgress.init))
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        if http.statusCode == 401 { authorizationFailed(); throw APIError.unauthorized }
        guard (200..<300).contains(http.statusCode) else { throw APIError.answer(body, status: http.statusCode) }
        return ChatAttachment(sha256: hash, name: name, type: type, size: data.count)
    }
}

/// An upload's progress, as a share of its body sent.
final class UploadProgress: NSObject, URLSessionTaskDelegate, Sendable {
    let report: @Sendable (Double) -> Void
    init(_ report: @escaping @Sendable (Double) -> Void) { self.report = report }
    func urlSession(_ session: URLSession, task: URLSessionTask, didSendBodyData bytesSent: Int64, totalBytesSent: Int64, totalBytesExpectedToSend: Int64) {
        guard totalBytesExpectedToSend > 0 else { return }
        report(Double(totalBytesSent) / Double(totalBytesExpectedToSend))
    }
}

/// What a voice memo records with: this iPhone's microphone, or a test's.
@MainActor protocol Microphone: AnyObject {
    /// Whether the person lets it record (iOS asks them the first time).
    func allowed() async -> Bool
    /// Records into the file at `url`, until `stop`.
    func start(into url: URL) throws
    func stop() throws
}

/// This iPhone's microphone, AAC into its file. The audio session is the
/// recording's from start to stop: what plays stops, and nothing plays
/// until it ends (`AudioPlayback`).
@MainActor final class DeviceMicrophone: Microphone {
    private var recorder: AVAudioRecorder?
    func allowed() async -> Bool { await AVAudioApplication.requestRecordPermission() }
    func start(into url: URL) throws {
        AudioPlayback.shared.recordingBegan()
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker])
            try session.setActive(true)
            let recorder = try AVAudioRecorder(url: url, settings: [AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 44_100, AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 96_000])
            guard recorder.record() else { throw APIError.server("The microphone could not start recording.") }
            self.recorder = recorder
        } catch { AudioPlayback.shared.recordingEnded(); throw error }
    }
    func stop() throws {
        recorder?.stop(); recorder = nil
        AudioPlayback.shared.recordingEnded()
        try AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

/// A voice memo: recorded from `record` until `finish` or its deadline,
/// then its file is the caller's. `cancel` stops and removes it.
@MainActor @Observable
final class VoiceMemo {
    /// The web's five-minute bound, an unattended recording's too.
    static let longest: Duration = .seconds(300)
    @ObservationIgnored private let microphone: any Microphone
    @ObservationIgnored private let longest: Duration
    @ObservationIgnored private var deadline: Task<Void, Never>?
    @ObservationIgnored private var url: URL?
    @ObservationIgnored private var ended: CheckedContinuation<URL?, Never>?
    /// Asked to record, not yet recording (iOS may be asking the person):
    /// a cancel meanwhile keeps it from starting.
    @ObservationIgnored private var starting = false
    private(set) var recording = false
    var error: String?

    init(microphone: any Microphone = DeviceMicrophone(), longest: Duration = VoiceMemo.longest) {
        self.microphone = microphone
        self.longest = longest
    }

    /// Records until `finish` or the deadline, and returns its file; nil if
    /// it never records (saying why in `error`) or is cancelled.
    func record() async -> URL? {
        guard !starting, !recording else { return nil }
        starting = true
        error = nil
        guard await microphone.allowed() else { starting = false; error = "Allow microphone access in iOS Settings to record a voice memo."; return nil }
        guard starting else { return nil }
        starting = false
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("voice-\(UUID().uuidString).m4a")
        do { try microphone.start(into: url) } catch { self.error = error.localizedDescription; return nil }
        self.url = url; recording = true
        let longest = longest
        deadline = Task { [weak self] in
            do { try await Task.sleep(for: longest) } catch { return }
            self?.finish()
        }
        return await withCheckedContinuation { ended = $0 }
    }
    /// Finished, by hand or at its deadline: `record` returns its file.
    func finish() { end(keeping: true) }
    /// Stopped and removed: `record` returns none.
    func cancel() { starting = false; end(keeping: false) }

    private func end(keeping: Bool) {
        guard recording, let url else { return }
        deadline?.cancel(); deadline = nil
        recording = false; self.url = nil
        do { try microphone.stop() } catch { self.error = error.localizedDescription }
        if !keeping { do { try FileManager.default.removeItem(at: url) } catch { self.error = error.localizedDescription } }
        ended?.resume(returning: keeping ? url : nil); ended = nil
    }

    /// The memo, as the file a message attaches, once recorded: its place
    /// reserved first (`ChatSession.beginAttaching(1)`), so a message with
    /// no room refuses it before it records, never after. Its recording
    /// goes once read. Cancelled or never begun, it is no file
    /// (`CancellationError`, which `attach` leaves unsaid).
    func file() async throws -> [OutgoingFile] {
        guard let url = await record() else { throw CancellationError() }
        defer { try? FileManager.default.removeItem(at: url) }
        return [try await Task.detached(priority: .userInitiated) {
            try AttachmentInput.file(Data(contentsOf: url), name: url.lastPathComponent, type: UTType(filenameExtension: url.pathExtension))
        }.value]
    }
}
