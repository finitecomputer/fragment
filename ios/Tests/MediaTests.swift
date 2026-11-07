import Foundation
import AVFoundation
import Testing
@testable import Fragment

/// A chat's files as the app shows and plays them: what each is shown as,
/// which sounds play (and how a WebM one is made to), how a file is read
/// (bounded by its record, checked by its hash, kept by its key), how it is
/// named on disk, and the one player.
@MainActor
struct MediaTests {
    private static func attachment(_ name: String, _ type: String, sha256: String = String(repeating: "a", count: 64), size: Int = 1) -> ChatAttachment {
        ChatAttachment(sha256: sha256, name: name, type: type, size: size)
    }

    @Test func eachFileShowsAsWhatItIs() {
        #expect(AttachmentKind(Self.attachment("voice-3F2A.m4a", "audio/mp4")) == .voice)
        #expect(AttachmentKind(Self.attachment("voice-memo.webm", "audio/webm;codecs=opus")) == .voice)
        #expect(AttachmentKind(Self.attachment("voice_2026-11-06.m4a", "audio/mp4")) == .voice)
        #expect(AttachmentKind(Self.attachment("speech.mp3", "audio/mpeg")) == .voice)
        #expect(AttachmentKind(Self.attachment("tts_17.mp3", "audio/mpeg")) == .voice)
        #expect(AttachmentKind(Self.attachment("interview.mp3", "audio/mpeg")) == .audio)
        #expect(AttachmentKind(Self.attachment("voiceover-final.wav", "audio/wav")) == .audio)
        #expect(AttachmentKind(Self.attachment("cat.png", " Image/PNG ")) == .image)
        #expect(AttachmentKind(Self.attachment("IMG_1.HEIC", "image/heic")) == .image)
        // a type a page could run is never a picture
        #expect(AttachmentKind(Self.attachment("logo.svg", "image/svg+xml")) == .file)
        #expect(AttachmentKind(Self.attachment("report", "APPLICATION/PDF")) == .pdf)
        #expect(AttachmentKind(Self.attachment("clip.mov", "video/quicktime")) == .file)
        #expect(AttachmentKind(Self.attachment("notes.zip", "application/zip")) == .file)
        #expect(Self.attachment("a.zip", "application/zip", size: 1_258_291).shownSize == "1.3 MB")
        #expect(Self.attachment("a.zip", "application/zip").shownType == "ZIP")
    }

    /// Which sounds iOS plays as they are, which are rewrapped, and which
    /// cannot play, read from their bytes whatever their records say.
    @Test func eachSoundIsReadForWhatItIs() {
        #expect(AudioContainer.of(AudioFixtures.chromeWebM) == .webm)
        #expect(AudioContainer.of(AudioFixtures.webmVorbis) == .webm)
        #expect(AudioContainer.of(AudioFixtures.chromeMP4) == .native)
        #expect(AudioContainer.of(AudioFixtures.oggOpus) == .native)
        #expect(AudioContainer.of(AudioFixtures.oggVorbis) == .unplayable)
        #expect(AudioContainer.of(AudioFixtures.mp3) == .native)
        #expect(AudioContainer.of(AudioFixtures.m4a) == .native)
        #expect(AudioContainer.of(AudioFixtures.wav) == .native)
        #expect(AudioContainer.of(Data("<html>".utf8)) == .unplayable)
        #expect(AudioContainer.of(Data()) == .unplayable)
    }

    /// Every sound that occurs plays here, with its length and level bars:
    /// Chrome's WebM once rewrapped as Ogg, the others as they are.
    @Test func everySoundThatOccursPlays() async throws {
        let folder = FileManager.default.temporaryDirectory.appending(path: "media-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let ogg = try WebMOpus.ogg(from: AudioFixtures.chromeWebM)
        #expect(AudioContainer.of(ogg) == .native)
        let sounds: [(String, Data, ClosedRange<Double>)] = [
            ("chrome-rewrapped.ogg", ogg, 2.7...3.3), ("chrome.mp4", AudioFixtures.chromeMP4, 2.5...3.3),
            ("opus.ogg", AudioFixtures.oggOpus, 0.9...1.1), ("tone.mp3", AudioFixtures.mp3, 0.9...1.2),
            ("tone.m4a", AudioFixtures.m4a, 0.9...1.1), ("tone.wav", AudioFixtures.wav, 0.2...0.3),
        ]
        for (name, data, length) in sounds {
            let url = folder.appending(path: name)
            try data.write(to: url)
            let facts = try #require(await SoundFacts.read(url), "\(name) reads")
            #expect(length.contains(facts.duration), "\(name): \(facts.duration) s")
            #expect(facts.levels.count == SoundFacts.bars && facts.levels.contains { $0 > 0.5 }, "\(name) has levels")
            #expect(try await AVURLAsset(url: url).load(.isPlayable), "\(name) plays")
        }
        // the recording's louder half seconds (its first, third, fifth) are its louder bars
        let rewrapped = try #require(await SoundFacts.read(folder.appending(path: "chrome-rewrapped.ogg")))
        let loud = rewrapped.levels[2], quiet = rewrapped.levels[7]
        #expect(loud > quiet, "\(rewrapped.levels)")
    }

    /// A rewrap copies Chrome's packets as they are, positioned by their own
    /// headers, in pages Ogg checks; anything else is refused, never guessed.
    @Test func aWebMIsRewrappedOrRefused() throws {
        let (head, packets) = try WebMOpus.demux(AudioFixtures.chromeWebM)
        #expect(head.starts(with: Array("OpusHead".utf8)))
        // three seconds, in Chrome's 60 ms packets, give or take the recorder's start and end
        let samples = packets.reduce(0) { $0 + OggWriter.samples($1) }
        #expect(packets.allSatisfy { OggWriter.samples($0) == 2_880 } && (2.7...3.2).contains(Double(samples) / 48_000), "\(packets.count) packets, \(samples) samples")
        let ogg = try WebMOpus.ogg(from: AudioFixtures.chromeWebM)
        // each page's checksum is Ogg's, the first begins the stream and the last ends it
        var offset = 0
        var flags: [UInt8] = []
        let bytes = [UInt8](ogg)
        while offset < bytes.count {
            #expect(Array(bytes[offset..<offset + 4]) == Array("OggS".utf8))
            let lacing = Int(bytes[offset + 26])
            let length = 27 + lacing + bytes[(offset + 27)..<(offset + 27 + lacing)].reduce(0) { $0 + Int($1) }
            var page = Data(bytes[offset..<offset + length])
            let stored = page[22..<26].withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }
            page.replaceSubrange(22..<26, with: [0, 0, 0, 0])
            #expect(OggWriter.crc32(page) == UInt32(littleEndian: stored))
            flags.append(bytes[offset + 5])
            offset += length
        }
        #expect(flags.first == 0x02 && flags.last == 0x04 && flags.dropFirst().dropLast().allSatisfy { $0 == 0 })
        // Ogg's own checksum on ffmpeg's page agrees with ours
        var first = Data(AudioFixtures.oggOpus.prefix(27 + Int(AudioFixtures.oggOpus[26]) + 19))
        let theirs = first[22..<26].withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }
        first.replaceSubrange(22..<26, with: [0, 0, 0, 0])
        #expect(OggWriter.crc32(first) == UInt32(littleEndian: theirs))
        // a packet's samples, from its TOC byte (RFC 6716, 3.1)
        #expect(OggWriter.samples(Data([31 << 3])) == 960)
        #expect(OggWriter.samples(Data([15 << 3 | 1])) == 1_920)
        #expect(OggWriter.samples(Data([3 << 3 | 3, 2])) == 5_760)
        #expect(OggWriter.samples(Data([16 << 3])) == 120)
        #expect(throws: WebMOpus.Refusal.notWebM) { try WebMOpus.ogg(from: AudioFixtures.oggOpus) }
        #expect(throws: WebMOpus.Refusal.noOpusTrack) { try WebMOpus.ogg(from: AudioFixtures.webmVorbis) }
        #expect(throws: WebMOpus.Refusal.truncated) { try WebMOpus.ogg(from: AudioFixtures.chromeWebM.prefix(200)) }
        #expect(throws: (any Error).self) { try WebMOpus.ogg(from: Data([0x1A, 0x45, 0xDF, 0xA3, 0x80])) }
    }

    /// The server's name never chooses a path: a last component, cleaned,
    /// with the extension its type names, in a folder of its hash.
    @Test func aFilesNameIsMadeSafe() async throws {
        #expect(SafeFileName.of("../../Library/Preferences/x.plist", type: "application/x-plist") == "x.plist")
        #expect(SafeFileName.of("a/b\\c.pdf", type: "application/pdf") == "c.pdf")
        #expect(SafeFileName.of("..", type: "application/pdf") == "file.pdf")
        #expect(SafeFileName.of(".hidden", type: "text/plain") == "hidden.txt")
        #expect(SafeFileName.of("report", type: "application/pdf") == "report.pdf")
        #expect(SafeFileName.of("photo.JPG", type: "image/jpeg") == "photo.JPG")
        #expect(SafeFileName.of("invoice.pdf.exe", type: "application/pdf") == "invoice.pdf.pdf")
        #expect(SafeFileName.of("cv\u{202E}fdp.exe", type: "application/octet-stream") == "cvfdp.exe")
        #expect(SafeFileName.of("a:b\u{0}\nc.txt", type: "text/plain") == "abc.txt")
        #expect(SafeFileName.of("", type: "audio/mpeg") == "file.mp3")
        let long = SafeFileName.of(String(repeating: "é", count: 300) + ".m4a", type: "audio/mp4")
        #expect(long.utf8.count <= SafeFileName.bytesMax && long.hasSuffix(".m4a"))
        #expect(SafeFileName.isHash(String(repeating: "0f", count: 32)))
        #expect(!SafeFileName.isHash(String(repeating: "0F", count: 32)) && !SafeFileName.isHash("../" + String(repeating: "a", count: 61)))

        let root = FileManager.default.temporaryDirectory.appending(path: "blobs-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let store = BlobStore(root: root)
        let data = Data("a page".utf8)
        let api = try Stub { _, _ in (404, "{}") }.api()
        let file = try await store.keep(data, key: BlobKey(api: api, fragment: "talk--c1", sha256: sha256Hex(data)))
        let named = try store.named(Self.attachment("../../../evil", "application/pdf", sha256: sha256Hex(data)), file: file)
        #expect(named.lastPathComponent == "evil.pdf" && named.path.hasPrefix(root.path) && named.deletingLastPathComponent().lastPathComponent == sha256Hex(data))
        #expect(try Data(contentsOf: named) == data)
        #expect(throws: (any Error).self) { try store.named(Self.attachment("x", "text/plain", sha256: "../../x"), file: file) }
    }

    /// A file is kept by its server, its chat and its hash, and nothing else
    /// the server says.
    @Test func aFileIsKeptByItsChatAndHash() throws {
        let one = try Stub { _, _ in (404, "{}") }.api(), other = try Stub { _, _ in (404, "{}") }.api()
        let sha = String(repeating: "b", count: 64)
        let key = BlobKey(api: one, fragment: "talk--c1", sha256: sha)
        #expect(key == BlobKey(api: one, fragment: "talk--c1", sha256: sha))
        #expect(key != BlobKey(api: one, fragment: "talk--c2", sha256: sha))
        #expect(key != BlobKey(api: other, fragment: "talk--c1", sha256: sha))
        #expect(key.scope.count == 32 && !key.scope.contains("talk"))
        #expect(key.imageKey(720) != key.imageKey(192))
        let store = BlobStore(root: URL(fileURLWithPath: "/tmp/blobs"))
        #expect(store.path(key)?.path == "/tmp/blobs/\(key.scope)/\(sha)")
        #expect(store.path(BlobKey(api: one, fragment: "talk--c1", sha256: "../" + sha.dropFirst(3))) == nil)
    }

    /// A file is read once, signed, to disk: the size its record names and
    /// the hash it is named by, or refused, never read past that size.
    @Test func aFileIsReadOnceBoundedByItsRecord() async throws {
        let bytes = Data("hello, a voice".utf8)
        let sha = sha256Hex(bytes)
        let stub = Stub { seen, _ in
            switch seen.path {
            case "/api/f/talk--c1/blobs/\(sha)": (200, "base64:" + bytes.base64EncodedString())
            // more than its record names
            case "/api/f/talk--c2/blobs/\(sha)": (200, "base64:" + (bytes + bytes).base64EncodedString())
            // other bytes than its hash names
            case "/api/f/talk--c3/blobs/\(sha)": (200, "base64:" + Data("hello, a vowel".utf8).base64EncodedString())
            default: (404, #"{"error":"not_found","message":"no such blob"}"#)
            }
        }
        let api = try stub.api()
        let root = FileManager.default.temporaryDirectory.appending(path: "blobs-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let store = BlobStore(root: root)
        let memo = Self.attachment("voice-1.m4a", "audio/mp4", sha256: sha, size: bytes.count)

        let load = store.load(memo, api: api, fragment: "talk--c1", begin: false)
        #expect(load.state == .idle && stub.seen.isEmpty)
        load.begin()
        // shown again (scrolled past and back) while reading: the same read
        #expect(store.load(memo, api: api, fragment: "talk--c1", begin: true) === load)
        let file = try #require(await Self.settled(load).file)
        #expect(try Data(contentsOf: file) == bytes && stub.seen.count == 1 && stub.seen[0].authorization?.hasPrefix("Nostr ") == true)
        // kept: a later view reads nothing
        #expect(store.load(memo, api: api, fragment: "talk--c1", begin: true).file == file && stub.seen.count == 1)
        #expect(BlobStore(root: root).load(memo, api: api, fragment: "talk--c1", begin: false).file == file)

        for (fragment, why) in [("talk--c2", "past its size"), ("talk--c3", "another hash"), ("talk--c4", "absent")] {
            let refused = store.load(memo, api: api, fragment: fragment, begin: true)
            guard case .failed = await Self.settled(refused).state else { Issue.record("\(why) is refused"); continue }
            #expect(store.cached(BlobKey(api: api, fragment: fragment, sha256: sha)) == nil, "\(why): nothing kept")
        }
        // a failed one is read again on a retry
        let retried = store.load(memo, api: api, fragment: "talk--c4", begin: false)
        retried.begin()
        #expect(retried.state == .loading(0))
        _ = await Self.settled(retried)
        #expect(stub.seen.filter { $0.path.hasPrefix("/api/f/talk--c4/") }.count == 2)
    }

    private static func settled(_ load: BlobLoad) async -> BlobLoad {
        for _ in 0..<400 {
            switch load.state {
            case .ready, .failed, .idle: return load
            case .loading: try? await Task.sleep(for: .milliseconds(10))
            }
        }
        return load
    }

    /// One sound plays at a time: playing another stops the first, and the
    /// chat's leaving stops either.
    @Test func oneSoundPlaysAtATime() async throws {
        final class Engine: PlaybackEngine {
            let url: URL
            var duration: Double = 10
            var elapsed: Double = 0
            var ended = false
            var playing = false
            init(url: URL) { self.url = url }
            func play() { playing = true }
            func pause() { playing = false }
            func seek(_ seconds: Double) { elapsed = seconds; ended = false }
        }
        var engines: [Engine] = []
        var active = 0
        let playback = AudioPlayback(engine: { url in let engine = Engine(url: url); engines.append(engine); return engine },
                                     session: .init(activate: { active += 1 }, deactivate: { active -= 1 }))
        let a = URL(fileURLWithPath: "/a.m4a"), b = URL(fileURLWithPath: "/b.ogg")
        playback.toggle("a", url: a)
        #expect(playback.current == "a" && playback.playing && engines[0].playing)
        playback.toggle("b", url: b)
        #expect(playback.current == "b" && playback.playing && !engines[0].playing && engines[1].playing && engines.count == 2)
        #expect(playback.place("a") == nil && playback.place("b")?.playing == true)
        playback.toggle("b", url: b)
        #expect(!playback.playing && !engines[1].playing && playback.current == "b")
        // moving another's place loads it, paused, by the length read before its player knows one
        playback.seek("a", url: a, to: 0.5, of: 8)
        #expect(playback.current == "a" && !playback.playing && engines.count == 3 && engines[2].elapsed == 5)
        engines[2].duration = 0
        playback.seek("a", url: a, to: 0.25, of: 8)
        #expect(engines[2].elapsed == 2 && playback.place("a")?.elapsed == 2)
        engines[2].duration = 10; engines[2].elapsed = 5
        playback.toggle("a", url: a)
        #expect(engines[2].playing && engines[2].elapsed == 5)
        // played to its end: back to its start, paused
        engines[2].elapsed = 10; engines[2].ended = true
        try await Task.sleep(for: .milliseconds(150))
        #expect(!playback.playing && playback.place("a")?.elapsed == 0)
        playback.toggle("a", url: a)
        #expect(playback.playing && engines[2].elapsed == 0)
        playback.stop()
        #expect(playback.current == nil && !playback.playing && !engines[2].playing)
        #expect(active <= 0, "its session let go")
    }

    /// The person's own file shows from its bytes as it is put; one that
    /// fails to be put says so, holds the message, and goes on a retry.
    @Test func aFailedUploadSaysSoAndRetries() async throws {
        let stub = Stub { seen, before in
            guard seen.method == "PUT" else { return (404, #"{"error":"not_found","message":"no route"}"#) }
            return before.contains { $0.method == "PUT" } ? (200, #"{"ok":true,"stored":true}"#) : (503, #"{"error":"unavailable","message":"Try again soon."}"#)
        }
        let api = try stub.api()
        defer { try? ChatDraftStore.removeAccount(api: api) }
        let session = ChatSession(fragment: FragmentSummary(name: "talk--c1", kind: "chat"), api: api)
        let data = Data("a photo's bytes".utf8)
        #expect(session.beginAttaching(1))
        await session.attach { [OutgoingFile(data: data, name: "photo-1.jpg", type: "image/jpeg")] }
        let failed = try #require(session.uploads.first)
        #expect(failed.failed == "Try again soon." && session.attachments.isEmpty && !session.attaching)
        // shown from its own bytes, never read back
        let kept = try #require(BlobStore.shared.cached(BlobKey(api: api, fragment: "talk--c1", sha256: sha256Hex(data))))
        #expect(try Data(contentsOf: kept) == data)
        session.edit("Look")
        await session.send()
        #expect(session.pending.isEmpty && session.error == "A file did not upload. Retry it or remove it.")
        await session.retryUpload(failed.id)
        #expect(session.uploads.isEmpty && session.attachments.map(\.sha256) == [sha256Hex(data)])
        #expect(stub.seen.filter { $0.method == "PUT" }.map(\.body) == [data, data])
        BlobStore.shared.removeAll()
    }
}
