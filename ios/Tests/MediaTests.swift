import Foundation
import AVFoundation
import PDFKit
import UIKit
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

    /// A recording owns the audio session: starting one stops what plays,
    /// and nothing plays until it ends, whichever came first.
    @Test func aRecordingAndASoundNeverShareTheSession() {
        final class Engine: PlaybackEngine {
            var duration: Double = 10, elapsed: Double = 0, ended = false, playing = false
            func play() { playing = true }
            func pause() { playing = false }
            func seek(_ seconds: Double) { elapsed = seconds }
        }
        var engines: [Engine] = []
        var active = 0
        let playback = AudioPlayback(engine: { _ in let engine = Engine(); engines.append(engine); return engine },
                                     session: .init(activate: { active += 1 }, deactivate: { active -= 1 }))
        let a = URL(fileURLWithPath: "/a.m4a")
        // playing, then a recording starts: the sound stops and lets the session go
        playback.toggle("a", url: a)
        #expect(playback.playing && active == 1)
        playback.recordingBegan()
        #expect(playback.recording && !playback.playing && playback.current == nil && !engines[0].playing && active == 0)
        // recording, then Play or a drag: nothing plays, no player is made
        playback.toggle("a", url: a)
        playback.seek("a", url: a, to: 0.5, of: 10)
        #expect(!playback.playing && playback.current == nil && engines.count == 1 && active == 0)
        // the recording ends: Play plays again
        playback.recordingEnded()
        playback.toggle("a", url: a)
        #expect(playback.playing && engines.count == 2 && active == 1)
        playback.stop()
    }

    /// A PDF's first page is drawn into its card's pixels alone, its top in
    /// view and its width filling them, whatever its shape; a page of no
    /// real size is not drawn.
    @Test func aPDFsFirstPageIsDrawnWithinItsCard() throws {
        let card = CGSize(width: 260, height: 170)
        #expect(PDFFirstPage.scale(page: CGSize(width: 1, height: 14_400), canvasWidth: 780) == 780)
        #expect(abs((PDFFirstPage.scale(page: CGSize(width: 612, height: 792), canvasWidth: 780) ?? 0) - 780.0 / 612.0) < 1e-9)
        for page in [CGSize(width: 0, height: 10), CGSize(width: 10, height: 0), CGSize(width: -5, height: 10), CGSize(width: CGFloat.nan, height: 10),
                     CGSize(width: 10, height: CGFloat.infinity), CGSize(width: 14_401, height: 10), CGSize(width: 10, height: 1e12)] {
            #expect(PDFFirstPage.scale(page: page, canvasWidth: 780) == nil, "\(page)")
        }
        func pdf(_ size: CGSize) throws -> PDFPage {
            let data = UIGraphicsPDFRenderer(bounds: CGRect(origin: .zero, size: size)).pdfData { context in
                context.beginPage()
                // its top band black, the rest white
                UIColor.black.setFill()
                context.fill(CGRect(x: 0, y: 0, width: size.width, height: min(size.height, 40)))
            }
            return try #require(PDFDocument(data: data)?.page(at: 0))
        }
        func pixel(_ image: UIImage, _ x: Int, _ y: Int) throws -> UInt8 {
            let cg = try #require(image.cgImage)
            var value: UInt8 = 0
            let context = try #require(CGContext(data: &value, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 1, space: CGColorSpaceCreateDeviceGray(), bitmapInfo: 0))
            context.draw(cg, in: CGRect(x: -x, y: -(cg.height - 1 - y), width: cg.width, height: cg.height))
            return value
        }
        // a letter page: its top band at the card's top, white below
        let letter = try #require(PDFFirstPage.image(try pdf(CGSize(width: 612, height: 792)), card: card))
        #expect(letter.cgImage?.width == 780 && letter.cgImage?.height == 510)
        #expect(try pixel(letter, 390, 10) < 40 && pixel(letter, 390, 400) > 215)
        // a receipt a point wide and 14,400 tall: still the card's pixels, not 780 by 11 million
        let receipt = try #require(PDFFirstPage.image(try pdf(CGSize(width: 1, height: 14_400)), card: card))
        #expect(receipt.cgImage?.width == 780 && receipt.cgImage?.height == 510)
        // a page as wide as PDF allows and a point tall: the card's pixels too
        let strip = try #require(PDFFirstPage.image(try pdf(CGSize(width: 14_400, height: 1)), card: card))
        #expect(strip.cgImage?.width == 780 && strip.cgImage?.height == 510)
    }

    /// The disk the files take stays within its bound through one store's
    /// life: each file written trims the least recently used, a file's named
    /// links and its rewrap counted with it (a link once), the files asked
    /// for last kept.
    @Test func theFilesKeptStayWithinTheirBound() async throws {
        let root = FileManager.default.temporaryDirectory.appending(path: "blobs-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let api = try Stub { _, _ in (404, "{}") }.api()
        let block = 64 * 1_024
        let store = BlobStore(root: root, diskMax: Int64(4 * block), keptRecent: 2)
        var keys: [BlobKey] = []
        for index in 0..<10 {
            let data = Data(repeating: UInt8(index), count: block)
            let key = BlobKey(api: api, fragment: "talk--c1", sha256: sha256Hex(data))
            let file = try await store.keep(data, key: key)
            // a named link is the same bytes, and counts once
            _ = try store.named(ChatAttachment(sha256: key.sha256, name: "memo-\(index).m4a", type: "audio/mp4", size: block), file: file)
            keys.append(key)
            await store.settled()
            #expect(Self.diskBytes(root) <= Int64(4 * block), "after \(index + 1) files: \(Self.diskBytes(root))")
        }
        #expect(store.cached(keys[9]) != nil && store.cached(keys[8]) != nil, "the last asked for are kept")
        #expect(store.cached(keys[0]) == nil && store.cached(keys[1]) == nil, "the least recently used went")
        #expect(!FileManager.default.fileExists(atPath: root.appending(path: "\(keys[0].scope)/named/\(keys[0].sha256)").path), "with its named links")
        // one asked for again stays past newer ones
        _ = store.load(ChatAttachment(sha256: keys[6].sha256, name: "x", type: "audio/mp4", size: block), api: api, fragment: "talk--c1", begin: false)
        for index in 10..<13 {
            let data = Data(repeating: UInt8(index), count: block)
            _ = try await store.keep(data, key: BlobKey(api: api, fragment: "talk--c1", sha256: sha256Hex(data)))
            await store.settled()
        }
        #expect(Self.diskBytes(root) <= Int64(4 * block))
        // 7, 8 and 9 went, the least recently used; 6, asked for again, stayed
        #expect(store.cached(keys[6]) != nil && [7, 8, 9].allSatisfy { store.cached(keys[$0]) == nil })
    }

    /// Bytes a folder's files take on disk, a file of several names once.
    private static func diskBytes(_ root: URL) -> Int64 {
        var seen = Set<AnyHashable>(), total: Int64 = 0
        let keys: [URLResourceKey] = [.isRegularFileKey, .totalFileAllocatedSizeKey, .fileResourceIdentifierKey]
        guard let walk = FileManager.default.enumerator(at: root, includingPropertiesForKeys: keys) else { return 0 }
        for case let url as URL in walk {
            guard let values = try? url.resourceValues(forKeys: Set(keys)), values.isRegularFile == true,
                  let id = values.fileResourceIdentifier as? NSObject, seen.insert(AnyHashable(id)).inserted else { continue }
            total += Int64(values.totalFileAllocatedSize ?? 0)
        }
        return total
    }
}
