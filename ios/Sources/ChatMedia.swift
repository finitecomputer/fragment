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
