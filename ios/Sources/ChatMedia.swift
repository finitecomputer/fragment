import SwiftUI
import CryptoKit
import AVFoundation
import UniformTypeIdentifiers

struct ChatAttachment: Codable, Identifiable, Equatable, Sendable {
    let sha256: String
    let name: String
    let type: String
    let size: Int
    var id: String { sha256 }
    var json: JSONValue { .object(["sha256": .string(sha256), "name": .string(name), "type": .string(type), "size": .number(Double(size))]) }
}

extension ChatAttachment {
    /// The app's own bound on one attachment: it is read whole into memory,
    /// and sent over a phone's network. The server takes up to
    /// `limits::BLOB_MAX_BYTES` (crates/proto).
    static let bytesMax = 25 * 1_024 * 1_024
    /// Files a message carries at most, as the web's chat (`ATTACHMENTS_MAX`).
    static let countMax = 8
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
/// any other picture (a photo's HEIC) as JPEG, which the web and agents read.
enum AttachmentInput {
    static let shownImages: [UTType] = [.png, .jpeg, .webP, .gif]
    static let jpegQuality = 0.9

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
        guard type.conforms(to: .image), let image = UIImage(data: data), let jpeg = image.jpegData(compressionQuality: jpegQuality) else {
            throw APIError.server("This picture could not be read.")
        }
        return try bounded(OutgoingFile(data: jpeg, name: named(name, .jpeg), type: "image/jpeg"))
    }

    /// The pictures on a pasteboard, each as `image` makes it: a type the
    /// web shows first, then any picture's data, then an image object (as
    /// PNG, or JPEG past the bound). At most `countMax`.
    static func pasted(_ pasteboard: UIPasteboard) -> [OutgoingFile] {
        var files: [OutgoingFile] = []
        for (index, item) in pasteboard.items.prefix(ChatAttachment.countMax).enumerated() {
            let name = "pasted-image-\(index + 1)"
            let types = item.keys.compactMap { UTType($0) }.filter { $0.conforms(to: .image) }
            let ordered = types.sorted { a, b in rank(a) < rank(b) }
            var made: OutgoingFile?
            for type in ordered {
                if let data = item[type.identifier] as? Data, let file = try? image(data, type: type, name: name) { made = file; break }
                if let picture = item[type.identifier] as? UIImage { made = try? fromImage(picture, name: name); if made != nil { break } }
            }
            if let made { files.append(made) }
        }
        return files
    }

    static func fromImage(_ picture: UIImage, name: String) throws -> OutgoingFile {
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
        guard !file.data.isEmpty, file.data.count <= ChatAttachment.bytesMax else { throw APIError.server("Files must be between 1 byte and 25 MB.") }
        return file
    }
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
            guard (response as? HTTPURLResponse)?.statusCode == 200, data.count <= 25 * 1_024 * 1_024 else { throw APIError.invalidResponse }
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
