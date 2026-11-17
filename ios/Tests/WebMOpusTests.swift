import Foundation
import Testing
@testable import Fragment

/// Opus in WebM rewrapped as Ogg Opus (`WebMOpus`, `OggWriter`): Chrome's
/// recording to exact pages and granules, and a file someone else sent,
/// however it is made, refused rather than ever trapping.
struct WebMOpusTests {
    // MARK: A WebM, built element by element

    private static func size(_ count: Int) -> [UInt8] {
        [0x01] + stride(from: 48, through: 0, by: -8).map { UInt8((count >> $0) & 0xFF) }
    }
    private static let unknown: [UInt8] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    private static func element(_ id: [UInt8], _ body: [UInt8], size: [UInt8]? = nil) -> [UInt8] {
        id + (size ?? Self.size(body.count)) + body
    }
    /// An Opus header of one channel, mapping family 0, pre-skip 312.
    static let head: [UInt8] = Array("OpusHead".utf8) + [1, 1, 0x38, 0x01, 0x80, 0xBB, 0, 0, 0, 0, 0]
    /// A packet of one 20 ms CELT frame.
    static let frame: [UInt8] = [31 << 3, 0, 0, 0]

    /// A WebM as a live recorder writes one: its segment and cluster of
    /// unknown size, one Opus track (its header and delay as given), and
    /// each frame a SimpleBlock with `flags`.
    static func webm(head: [UInt8]? = Self.head, delay: [UInt8]? = nil, frames: [[UInt8]], flags: UInt8 = 0x80, codec: String = "A_OPUS") -> Data {
        let ebml = element([0x1A, 0x45, 0xDF, 0xA3], element([0x42, 0x82], Array("webm".utf8)))
        var entry = element([0xD7], [1]) + element([0x86], Array(codec.utf8)) + element([0x83], [2])
        if let head { entry += element([0x63, 0xA2], head) }
        if let delay { entry += element([0x56, 0xAA], delay) }
        let tracks = element([0x16, 0x54, 0xAE, 0x6B], element([0xAE], entry))
        var cluster = element([0xE7], [0])
        for frame in frames { cluster += element([0xA3], [0x81, 0, 0, flags] + frame) }
        return Data(ebml + [0x18, 0x53, 0x80, 0x67] + unknown + tracks + [0x1F, 0x43, 0xB6, 0x75] + unknown + cluster)
    }

    /// Each Ogg page of `ogg`: its flags, granule and packets, its checksum checked.
    static func pages(_ ogg: Data) -> [(flags: UInt8, granule: UInt64, packets: [Data])] {
        let bytes = [UInt8](ogg)
        var offset = 0
        var pages: [(UInt8, UInt64, [Data])] = []
        while offset + 27 <= bytes.count {
            #expect(Array(bytes[offset..<offset + 4]) == Array("OggS".utf8))
            let lacing = Array(bytes[(offset + 27)..<(offset + 27 + Int(bytes[offset + 26]))])
            let length = 27 + lacing.count + lacing.reduce(0) { $0 + Int($1) }
            var page = Data(bytes[offset..<offset + length])
            let stored = page[22..<26].withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }
            page.replaceSubrange(22..<26, with: [0, 0, 0, 0])
            #expect(OggWriter.crc32(page) == UInt32(littleEndian: stored))
            let granule = page[6..<14].withUnsafeBytes { $0.loadUnaligned(as: UInt64.self) }
            var packets: [Data] = []
            var at = offset + 27 + lacing.count, run = 0
            for value in lacing {
                run += Int(value)
                if value < 255 { packets.append(Data(bytes[at..<at + run])); at += run; run = 0 }
            }
            pages.append((bytes[offset + 5], UInt64(littleEndian: granule), packets))
            offset += length
        }
        #expect(offset == bytes.count)
        return pages
    }

    // MARK: Chrome's recording

    /// Chrome's recording: its header on a page of its own, then its tags,
    /// both at granule 0; then its packets unchanged, each page's granule
    /// the samples decoded through it, counted from the stream's start with
    /// pre-skip among them (RFC 7845, 4), as ffmpeg writes them.
    @Test func chromesRecordingIsRewrappedExactly() throws {
        let (head, packets) = try WebMOpus.demux(AudioFixtures.chromeWebM)
        #expect(head.count == 19 && OggWriter.validHead(head))
        let preSkip = Int(head[10]) | Int(head[11]) << 8
        // three seconds, in Chrome's 60 ms packets, give or take the recorder's start and end
        let samples = packets.map { OggWriter.samples($0) ?? 0 }
        #expect(samples.allSatisfy { $0 == 2_880 } && (2.7...3.2).contains(Double(samples.reduce(0, +)) / 48_000))
        let pages = Self.pages(try WebMOpus.ogg(from: AudioFixtures.chromeWebM))
        #expect(pages[0].flags == 0x02 && pages[0].granule == 0 && pages[0].packets == [head])
        #expect(pages[1].flags == 0 && pages[1].granule == 0 && pages[1].packets.first?.starts(with: Array("OpusTags".utf8)) == true)
        let audio = pages.dropFirst(2)
        #expect(Array(audio.flatMap(\.packets)) == packets)
        var decoded: UInt64 = 0
        for page in audio {
            decoded += page.packets.reduce(0) { $0 + UInt64(OggWriter.samples($1) ?? 0) }
            #expect(page.granule == decoded)
        }
        #expect(audio.last?.flags == 0x04 && audio.dropLast().allSatisfy { $0.flags == 0 })
        #expect(audio.last?.granule == UInt64(samples.reduce(0, +)) && Int(audio.first?.granule ?? 0) > preSkip)
        // Ogg's own checksum on ffmpeg's first page agrees with ours
        var first = Data(AudioFixtures.oggOpus.prefix(27 + Int(AudioFixtures.oggOpus[26]) + 19))
        let theirs = first[22..<26].withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }
        first.replaceSubrange(22..<26, with: [0, 0, 0, 0])
        #expect(OggWriter.crc32(first) == UInt32(littleEndian: theirs))
    }

    /// Many packets fill pages of at most fifty, each page's granule exact.
    @Test func pagesHoldFiftyPacketsAndCountTheirSamples() throws {
        let pages = Self.pages(try WebMOpus.ogg(from: Self.webm(frames: Array(repeating: Self.frame, count: 120))))
        #expect(pages.map(\.packets.count) == [1, 1, 50, 50, 20])
        #expect(pages.map(\.granule) == [0, 0, 48_000, 96_000, 115_200])
        #expect(pages.map(\.flags) == [0x02, 0, 0, 0, 0x04])
    }

    /// A packet's samples from its TOC byte (RFC 6716, 3.1–3.2), and none for
    /// one that names no frame or more than 120 ms.
    @Test func aPacketsSamplesComeFromItsHeader() {
        #expect(OggWriter.samples(Data([31 << 3])) == 960)
        #expect(OggWriter.samples(Data([15 << 3 | 1])) == 1_920)
        #expect(OggWriter.samples(Data([3 << 3 | 3, 2])) == 5_760)
        #expect(OggWriter.samples(Data([16 << 3])) == 120)
        #expect(OggWriter.samples(Data([16 << 3 | 3, 48])) == 5_760)
        #expect(OggWriter.samples(Data()) == nil)
        #expect(OggWriter.samples(Data([16 << 3 | 3])) == nil, "code 3 without its count")
        #expect(OggWriter.samples(Data([16 << 3 | 3, 0])) == nil, "no frames")
        #expect(OggWriter.samples(Data([16 << 3 | 3, 49])) == nil, "past 120 ms")
        #expect(OggWriter.samples(Data([3 << 3 | 3, 3])) == nil, "180 ms")
    }

    // MARK: What someone else's file may be

    /// A packet past Opus's largest (48 frames of 1,275 bytes) is refused
    /// before any page is written, at and just past its bound and past a
    /// page's 255 lacing values; the largest is taken.
    @Test func anOversizedPacketIsRefusedNeverTrapped() throws {
        let largest = [16 << 3 | 3, 48] + [UInt8](repeating: 0, count: WebMOpus.packetBytesMax - 2)
        let pages = Self.pages(try WebMOpus.ogg(from: Self.webm(frames: [largest, Self.frame])))
        // its 241 lacing values and the next packet's one fit one page
        #expect(pages.count == 3 && pages[2].packets == [Data(largest), Data(Self.frame)])
        for count in [WebMOpus.packetBytesMax + 1, 255 * 255 - 1, 255 * 255, 255 * 255 + 1, 200_000] {
            let packet = [16 << 3 | 3, 48] + [UInt8](repeating: 0, count: count - 2)
            #expect(throws: WebMOpus.Refusal.badPacket, "\(count) bytes") { try WebMOpus.ogg(from: Self.webm(frames: [Self.frame, packet])) }
            // and the writer refuses it alone, whoever calls it
            #expect(throws: WebMOpus.Refusal.badPacket, "\(count) bytes") { try OggWriter.opus(head: Data(Self.head), packets: [Data(Self.frame), Data(packet)]) }
        }
    }

    /// A header that is no Opus header this writer passes on is refused:
    /// one padded to a page's whole lacing (65,025 bytes) or just short of
    /// it, a later major version, no channels, a family it cannot describe.
    /// None is ever replaced by a guess.
    @Test func aHeaderThatIsNoOpusHeaderIsRefused() throws {
        var heads: [[UInt8]] = [
            Self.head + [UInt8](repeating: 0, count: 255 * 255 - Self.head.count),
            Self.head + [UInt8](repeating: 0, count: 255 * 255 - 1 - Self.head.count),
            Self.head + [0],
            Array(Self.head.prefix(18)),
        ]
        var later = Self.head; later[8] = 0x10; heads.append(later)
        var none = Self.head; none[9] = 0; heads.append(none)
        var three = Self.head; three[9] = 3; heads.append(three)
        var ambisonic = Self.head; ambisonic[18] = 2; heads.append(ambisonic)
        // family 1: a table too short, no streams, more coupled than streams, an entry past them
        let surround: [UInt8] = Array(Self.head.prefix(9)) + [2] + Array(Self.head[10..<18]) + [1, 1, 1, 0, 1]
        #expect(OggWriter.validHead(Data(surround)))
        heads += [Array(surround.dropLast()), surround.enumerated().map { $0 == 19 ? 0 : $1 },
                  surround.enumerated().map { $0 == 20 ? 2 : $1 }, surround.enumerated().map { $0 == 22 ? 7 : $1 }]
        for head in heads {
            #expect(throws: WebMOpus.Refusal.badHeader, "\(head.count) bytes") { try WebMOpus.ogg(from: Self.webm(head: head, frames: [Self.frame])) }
            #expect(throws: WebMOpus.Refusal.badHeader) { try OggWriter.opus(head: Data(head), packets: [Data(Self.frame)]) }
        }
        #expect(throws: WebMOpus.Refusal.badHeader) { try WebMOpus.ogg(from: Self.webm(head: nil, frames: [Self.frame])) }
        #expect(try WebMOpus.ogg(from: Self.webm(head: surround, frames: [Self.frame])).count > 0)
    }

    /// A track's CodecDelay is no header: at its largest it traps nothing,
    /// with no header beside it the file is refused, and with one it is
    /// passed over (pre-skip is the header's).
    @Test func codecDelayAtItsLargestTrapsNothing() throws {
        for delay: [UInt8] in [[UInt8](repeating: 0xFF, count: 8), [0x00, 0x00, 0x15, 0xD8, 0x6E, 0x7B, 0x1B, 0xB2], [0x01]] {
            #expect(throws: WebMOpus.Refusal.badHeader) { try WebMOpus.ogg(from: Self.webm(head: nil, delay: delay, frames: [Self.frame])) }
            let pages = Self.pages(try WebMOpus.ogg(from: Self.webm(delay: delay, frames: [Self.frame])))
            #expect(pages[0].packets == [Data(Self.head)])
        }
    }

    /// Each lacing a block may name is refused (it is never decoded).
    @Test func lacedBlocksAreRefused() {
        for flags: UInt8 in [0x82, 0x84, 0x86] {
            #expect(throws: WebMOpus.Refusal.laced) { try WebMOpus.ogg(from: Self.webm(frames: [Self.frame], flags: flags)) }
        }
    }

    /// Sizes that are no element's: a leaf past its parent or the file, a
    /// leaf of unknown size, a parent of zero size, an ID longer than EBML's.
    @Test func sizesThatAreNoElementsAreRefused() throws {
        let ebml = Self.element([0x1A, 0x45, 0xDF, 0xA3], [])
        // a leaf whose size runs past the file
        #expect(throws: WebMOpus.Refusal.truncated) { try WebMOpus.demux(Data(ebml + [0x18, 0x53, 0x80, 0x67] + Self.unknown + [0xE7] + Self.size(1_000) + [0])) }
        // a leaf whose size runs past its finite parent
        let tracks = Self.element([0x16, 0x54, 0xAE, 0x6B], [0x86] + Self.size(40) + Array("A_OPUS".utf8), size: Self.size(15))
        #expect(throws: WebMOpus.Refusal.truncated) { try WebMOpus.demux(Data(ebml + [0x18, 0x53, 0x80, 0x67] + Self.unknown + tracks + Array(repeating: 0, count: 64))) }
        // a leaf of unknown size
        #expect(throws: WebMOpus.Refusal.malformed) { try WebMOpus.demux(Data(ebml + [0x18, 0x53, 0x80, 0x67] + Self.unknown + [0xE7] + Self.unknown)) }
        // a segment of zero size: nothing in it
        #expect(throws: WebMOpus.Refusal.noOpusTrack) { try WebMOpus.demux(Data(ebml + Self.element([0x18, 0x53, 0x80, 0x67], []))) }
        // a track number of nine bytes is no unsigned integer
        let wide = Self.element([0x16, 0x54, 0xAE, 0x6B], Self.element([0xAE], Self.element([0xD7], [UInt8](repeating: 0xFF, count: 9))))
        #expect(throws: WebMOpus.Refusal.malformed) { try WebMOpus.demux(Data(ebml + [0x18, 0x53, 0x80, 0x67] + Self.unknown + wide)) }
        // an ID of five bytes
        #expect(throws: WebMOpus.Refusal.malformed) { try WebMOpus.demux(Data(ebml + [0x08, 0, 0, 0, 0, 0x80])) }
        // a track's fields outside its TrackEntry count for nothing
        let loose = Self.element([0x16, 0x54, 0xAE, 0x6B], Self.element([0x86], Array("A_OPUS".utf8)) + Self.element([0x63, 0xA2], Self.head))
        #expect(throws: WebMOpus.Refusal.noOpusTrack) { try WebMOpus.demux(Data(ebml + [0x18, 0x53, 0x80, 0x67] + Self.unknown + loose)) }
    }

    /// Masters nested deep, of unknown size, and clusters one after another
    /// (each ends where the next begins) are read with bounded work.
    @Test func nestedMastersAndManyClustersAreBounded() throws {
        let ebml = Self.element([0x1A, 0x45, 0xDF, 0xA3], [])
        let deep = Array(repeating: [0xAE] + Self.unknown, count: 20_000).flatMap { $0 }
        #expect(throws: WebMOpus.Refusal.noOpusTrack) { try WebMOpus.demux(Data(ebml + [0x18, 0x53, 0x80, 0x67] + Self.unknown + deep)) }
        var clusters = Self.webm(frames: [Self.frame])
        for _ in 0..<2_000 { clusters += [0x1F, 0x43, 0xB6, 0x75] + Self.unknown + Self.element([0xA3], [0x81, 0, 0, 0x80] + Self.frame) }
        let pages = Self.pages(try WebMOpus.ogg(from: clusters))
        #expect(pages.dropFirst(2).reduce(0) { $0 + $1.packets.count } == 2_001)
    }

    /// Chrome's recording cut at every length, and mutated and made up at
    /// random (a fixed seed): each is refused or rewrapped, never a trap,
    /// and what is rewrapped is pages Ogg checks.
    @Test func anyCutMutationOrGarbageIsRefusedOrRewrapped() {
        let whole = AudioFixtures.chromeWebM
        for length in 0..<whole.count where length < 4_096 || length % 7 == 0 {
            if let ogg = try? WebMOpus.ogg(from: whole.prefix(length)) { _ = Self.pages(ogg) }
        }
        var seed: UInt64 = 0x5EED_F1A6
        func next() -> Int { seed = seed &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407; return Int(seed >> 33) }
        var rewrapped = 0
        for _ in 0..<2_000 {
            var bytes = [UInt8](whole)
            for _ in 0..<(1 + next() % 4) { bytes[next() % bytes.count] = UInt8(next() % 256) }
            if let ogg = try? WebMOpus.ogg(from: Data(bytes)) { _ = Self.pages(ogg); rewrapped += 1 }
        }
        #expect(rewrapped > 0, "most single mutations land in audio, which is copied as it is")
        for _ in 0..<500 {
            let bytes = [0x1A, 0x45, 0xDF, 0xA3] + (0..<(next() % 600)).map { _ in UInt8(next() % 256) }
            if let ogg = try? WebMOpus.ogg(from: Data(bytes)) { _ = Self.pages(ogg) }
        }
        #expect(throws: WebMOpus.Refusal.notWebM) { try WebMOpus.ogg(from: AudioFixtures.oggOpus) }
        #expect(throws: WebMOpus.Refusal.noOpusTrack) { try WebMOpus.ogg(from: AudioFixtures.webmVorbis) }
        #expect(throws: WebMOpus.Refusal.noOpusTrack) { try WebMOpus.ogg(from: Self.webm(frames: [Self.frame], codec: "A_VORBIS")) }
    }
}
