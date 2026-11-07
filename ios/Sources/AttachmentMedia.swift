import Foundation
import CryptoKit
import UniformTypeIdentifiers

/// What a chat shows an attachment as, from its record's `type` (and, for
/// a voice memo, its `name`): docs/chat-records.md, "Attachments". The
/// record's words choose only how it is drawn; what plays is read from its
/// bytes (`AudioContainer`).
enum AttachmentKind: Equatable, Sendable {
    /// A picture iOS draws: inline at its aspect ratio, full screen on a tap.
    case image
    /// A voice memo: a player and no name.
    case voice
    /// Any other sound: a player, its name small.
    case audio
    /// A PDF: its first page, its page count and name.
    case pdf
    /// Anything else: an icon, name, size and type, opened by Quick Look.
    case file

    init(_ attachment: ChatAttachment) {
        let type = Self.essence(attachment.type)
        if Self.images.contains(type) { self = .image }
        else if type.hasPrefix("audio/") { self = Self.isVoiceMemo(attachment.name) ? .voice : .audio }
        else if type == "application/pdf" { self = .pdf }
        else { self = .file }
    }

    /// A type without its parameters, in lowercase (`audio/webm;codecs=opus`
    /// is `audio/webm`), as the web's `essence`.
    static func essence(_ type: String) -> String {
        String(type.split(separator: ";", maxSplits: 1).first ?? "").trimmingCharacters(in: .whitespaces).lowercased()
    }

    /// The web's shown pictures (media.js `IMAGE`) and the ones iOS's own
    /// camera makes.
    static let images: Set<String> = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/heic", "image/heif"]

    /// Whether a sound's name is a recorder's: this app's (`voice-<id>.m4a`),
    /// the web's (`voice-memo.<ext>`), SimpleX's (`voice_<date>.m4a`), or an
    /// agent's spoken reply (the bridge's `speech.mp3`, Hermes' `tts_<n>.mp3`).
    static func isVoiceMemo(_ name: String) -> Bool {
        let base = (name.lowercased() as NSString).deletingPathExtension
        return base == "speech" || ["voice-", "voice_", "tts_", "tts-"].contains { base.hasPrefix($0) } || base == "voice"
    }
}

/// How the app names a file it hands to Quick Look or Share: the record's
/// name is the server's, so it is only ever a last path component, cleaned,
/// and never chooses a directory. Directories are the blob's own hash
/// (`BlobStore`), which is checked to be one.
enum SafeFileName {
    static let bytesMax = 120

    static func of(_ name: String, type: String) -> String {
        // its last component, as a path or a URL would read it
        let last = name.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last.map(String.init) ?? ""
        var cleaned = String(String.UnicodeScalarView(last.unicodeScalars.filter { scalar in
            !CharacterSet.controlCharacters.contains(scalar) && scalar != ":" && !(0xFFF9...0xFFFB).contains(scalar.value)
                && !(0x202A...0x202E).contains(scalar.value) && !(0x2066...0x2069).contains(scalar.value)
        })).trimmingCharacters(in: .whitespacesAndNewlines)
        while cleaned.hasPrefix(".") { cleaned.removeFirst() }
        let wanted = UTType(mimeType: AttachmentKind.essence(type))?.preferredFilenameExtension
        var ext = (cleaned as NSString).pathExtension
        var base = (cleaned as NSString).deletingPathExtension
        if base.isEmpty { base = "file" }
        // Quick Look reads a file by its extension: one the type names wins
        // over none, or one of another type entirely
        if let wanted, ext.isEmpty || UTType(filenameExtension: ext).map({ !$0.conforms(to: UTType(mimeType: AttachmentKind.essence(type)) ?? .data) }) ?? true {
            ext = wanted
        }
        let suffix = ext.isEmpty ? "" : "." + ext
        while (base + suffix).utf8.count > bytesMax, !base.isEmpty { base.removeLast() }
        return base.isEmpty ? "file" + suffix : base + suffix
    }

    /// A blob's name is its SHA-256 in lowercase hex, and nothing else.
    static func isHash(_ sha256: String) -> Bool {
        sha256.utf8.count == 64 && sha256.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
}

/// A sound's container and codec, read from its first bytes: what iOS 26
/// plays as it is, what the app rewraps first, and what it cannot play.
///
/// Checked on iOS 26.5 (AVURLAsset and AVAudioPlayer, in the Simulator):
/// AAC in MP4 or M4A (this app's, Safari's MediaRecorder, SimpleX's), MP3
/// (an agent's spoken reply), WAV, and Opus in Ogg (Firefox's
/// MediaRecorder, a `.opus` file) all play. Opus in WebM (Chrome's
/// MediaRecorder) does not, so it is rewrapped as Ogg (`WebMOpus`), its
/// packets unchanged. Vorbis in Ogg does not play, and is offered as a file.
enum AudioContainer: Equatable, Sendable {
    case native
    case webm
    case unplayable

    static func of(_ head: Data) -> AudioContainer {
        let bytes = [UInt8](head.prefix(64))
        func at(_ offset: Int, _ ascii: String) -> Bool {
            let want = Array(ascii.utf8)
            return bytes.count >= offset + want.count && Array(bytes[offset..<offset + want.count]) == want
        }
        if bytes.starts(with: [0x1A, 0x45, 0xDF, 0xA3]) { return .webm }
        // an Ogg stream's first page carries its codec's header
        if at(0, "OggS") { return at(28, "OpusHead") ? .native : .unplayable }
        if at(4, "ftyp") || at(0, "ID3") || at(0, "fLaC") || at(0, "caff") { return .native }
        if at(0, "RIFF") && at(8, "WAVE") { return .native }
        if at(0, "FORM") && (at(8, "AIFF") || at(8, "AIFC")) { return .native }
        // an MP3 or ADTS AAC frame's sync word
        if bytes.count >= 2, bytes[0] == 0xFF, bytes[1] & 0xE0 == 0xE0 { return .native }
        return .unplayable
    }
}

/// Opus in WebM, as Chrome's MediaRecorder writes it (a live recording: its
/// segment and clusters of unknown size, no cues), rewrapped as Ogg Opus
/// (RFC 7845), which iOS plays. Nothing is decoded: each packet is copied
/// as it is, and its position counted from its own header (RFC 6716, 3.1).
/// The file is another's, so everything is checked before it is used: each
/// element within the one holding it, the Opus header (RFC 7845, 5.1),
/// each packet's length and header. Anything else (Vorbis, video, laced
/// blocks, a header or packet out of bounds) is refused, never guessed.
enum WebMOpus {
    enum Refusal: Error, Equatable { case notWebM, malformed, noOpusTrack, badHeader, badPacket, laced, truncated, tooLarge }

    /// Elements read at most: a bound on a hostile file's work.
    static let elementsMax = 2_000_000
    /// An Opus packet's bytes at most: 48 frames of 1,275 (RFC 6716, 3.2.5),
    /// so its lacing fits one Ogg page.
    static let packetBytesMax = 48 * 1_275

    static func ogg(from webm: Data) throws -> Data {
        let (head, packets) = try demux(webm)
        return try OggWriter.opus(head: head, packets: packets)
    }

    /// The Opus track's header (`OpusHead`, its CodecPrivate) and packets.
    static func demux(_ data: Data) throws -> (head: Data, packets: [Data]) {
        let bytes = [UInt8](data)
        guard bytes.starts(with: [0x1A, 0x45, 0xDF, 0xA3]) else { throw Refusal.notWebM }
        var reader = EBMLReader(bytes: bytes)
        guard let first = try reader.element(within: bytes.count), first.id == 0x1A45DFA3 else { throw Refusal.notWebM }
        try reader.skip(first)
        struct Track { var number: UInt64 = 0; var codec = ""; var head: Data? }
        var tracks: [Track] = []
        var blocks: [(track: UInt64, frame: Data)] = []
        var count = 0
        // Read flat: a master element of interest is entered, any other
        // skipped by its own size. Each open master's end is held, and every
        // element must end within the innermost one (a live recording's
        // segment and clusters, of unknown size, end where their parent does).
        let masters: Set<UInt64> = [0x18538067, 0x1654AE6B, 0xAE, 0x1F43B675, 0xA0]
        // a segment's own children: one ends a cluster of unknown size (Matroska's rule)
        let segmentLevel: Set<UInt64> = [0x1F43B675, 0x1C53BB6B, 0x1254C367, 0x1654AE6B, 0x1549A966, 0x114D9B74, 0x1043A770, 0x1941A469]
        var open: [(id: UInt64, end: Int, unknown: Bool)] = []
        while true {
            while let last = open.last, reader.offset >= last.end { open.removeLast() }
            let at = reader.offset
            var peek = reader
            if let id = peek.vint(keepMarker: true, lengthMax: 4), segmentLevel.contains(id) {
                while let last = open.last, last.unknown, last.id == 0x1F43B675 { open.removeLast() }
            }
            reader.offset = at
            let limit = open.last?.end ?? bytes.count
            guard let element = try reader.element(within: limit) else { break }
            count += 1
            guard count <= elementsMax else { throw Refusal.tooLarge }
            if masters.contains(element.id) {
                open.append((element.id, element.size.map { element.start + Int($0) } ?? limit, element.size == nil))
                if element.id == 0xAE { tracks.append(Track()) }
                continue
            }
            // a track's fields count only inside its own TrackEntry
            let inTrack = open.last?.id == 0xAE && !tracks.isEmpty
            switch element.id {
            case 0xD7 where inTrack: tracks[tracks.count - 1].number = try reader.uint(element)
            case 0x86 where inTrack: tracks[tracks.count - 1].codec = String(decoding: try reader.body(element), as: UTF8.self)
            case 0x63A2 where inTrack: tracks[tracks.count - 1].head = Data(try reader.body(element))
            case 0xA3, 0xA1:  // SimpleBlock, Block (inside a BlockGroup)
                let body = Array(try reader.body(element))
                var inner = EBMLReader(bytes: body)
                guard let track = inner.vint(keepMarker: false), inner.offset + 3 <= body.count else { throw Refusal.truncated }
                guard body[inner.offset + 2] & 0x06 == 0 else { throw Refusal.laced }
                blocks.append((track, Data(body[(inner.offset + 3)...])))
            default: break
            }
            try reader.skip(element)
        }
        guard let opus = tracks.first(where: { $0.codec == "A_OPUS" }) else { throw Refusal.noOpusTrack }
        guard let head = opus.head, OggWriter.validHead(head) else { throw Refusal.badHeader }
        // an empty packet carries nothing (and is no frame to count)
        let packets = blocks.filter { $0.track == opus.number && !$0.frame.isEmpty }.map(\.frame)
        guard !packets.isEmpty else { throw Refusal.noOpusTrack }
        for packet in packets where packet.count > packetBytesMax || OggWriter.samples(packet) == nil { throw Refusal.badPacket }
        return (head, packets)
    }
}

/// A reader of EBML (Matroska's encoding), bounded by its bytes.
struct EBMLReader {
    struct Element { let id: UInt64; let size: UInt64?; let start: Int }
    let bytes: [UInt8]
    var offset = 0

    /// A variable-length integer: an element's ID keeps its marker bit, a
    /// size drops it (all ones is unknown).
    mutating func vint(keepMarker: Bool, lengthMax: Int = 8) -> UInt64? {
        guard offset < bytes.count else { return nil }
        let first = bytes[offset]
        guard first != 0 else { return nil }
        let length = first.leadingZeroBitCount + 1
        guard offset + length <= bytes.count, length <= lengthMax else { return nil }
        var value = UInt64(keepMarker ? first : first & (0xFF >> length))
        for index in 1..<length { value = value << 8 | UInt64(bytes[offset + index]) }
        offset += length
        return value
    }

    /// The next element, which must end by `limit` (its parent's end): nil
    /// at `limit`. An ID is at most 4 bytes (EBML's), a size at most 8.
    mutating func element(within limit: Int) throws -> Element? {
        guard offset < limit else { return nil }
        guard let id = vint(keepMarker: true, lengthMax: 4) else { throw WebMOpus.Refusal.malformed }
        let sizeStart = offset
        guard let raw = vint(keepMarker: false) else { throw WebMOpus.Refusal.truncated }
        let length = offset - sizeStart
        let unknown = raw == (UInt64(1) << (7 * length)) - 1
        guard offset <= limit, unknown || raw <= UInt64(limit - offset) else { throw WebMOpus.Refusal.truncated }
        return Element(id: id, size: unknown ? nil : raw, start: offset)
    }

    /// Past an element's body (a leaf: it must name its size).
    mutating func skip(_ element: Element) throws {
        guard let size = element.size, size <= UInt64(bytes.count - element.start) else { throw WebMOpus.Refusal.malformed }
        offset = element.start + Int(size)
    }

    func body(_ element: Element) throws -> ArraySlice<UInt8> {
        guard let size = element.size, size <= UInt64(bytes.count - element.start) else { throw WebMOpus.Refusal.malformed }
        return bytes[element.start..<element.start + Int(size)]
    }

    /// An unsigned integer element: at most 8 bytes.
    func uint(_ element: Element) throws -> UInt64 {
        let body = try body(element)
        guard body.count <= 8 else { throw WebMOpus.Refusal.malformed }
        return body.reduce(0) { $0 << 8 | UInt64($1) }
    }
}

/// An Ogg Opus stream (RFC 3533, RFC 7845) from Opus packets.
enum OggWriter {
    static let serial: UInt32 = 0x46524147
    /// Lacing values a page holds at most (one byte counts them).
    static let segmentsMax = 255

    /// Whether `head` is an Opus header this writer passes on (RFC 7845,
    /// 5.1): its magic, a version of major 0, and a channel mapping it
    /// describes whole. Family 0: one or two channels, 19 bytes. Family 1:
    /// one to eight channels, its streams (at least one, coupled ones no
    /// more than them) and a table of one entry per channel, each a stream
    /// channel or silence (255). Any other family is refused.
    static func validHead(_ head: Data) -> Bool {
        let bytes = [UInt8](head)
        guard bytes.count >= 19, bytes.starts(with: Array("OpusHead".utf8)), bytes[8] >> 4 == 0 else { return false }
        let channels = Int(bytes[9])
        switch bytes[18] {
        case 0: return (1...2).contains(channels) && bytes.count == 19
        case 1:
            guard (1...8).contains(channels), bytes.count == 21 + channels else { return false }
            let streams = Int(bytes[19]), coupled = Int(bytes[20])
            guard streams >= 1, coupled <= streams, streams + coupled <= 255 else { return false }
            return bytes[21...].allSatisfy { Int($0) < streams + coupled || $0 == 255 }
        default: return false
        }
    }

    /// Ogg pages of the header, the tags and the packets, each packet whole
    /// on one page (none is larger than `WebMOpus.packetBytesMax`). The
    /// granule of a page is the samples decoded through its last packet,
    /// counted from the stream's start, pre-skip included (RFC 7845, 4).
    static func opus(head: Data, packets: [Data]) throws -> Data {
        guard validHead(head) else { throw WebMOpus.Refusal.badHeader }
        var tags = Data("OpusTags".utf8)
        let vendor = Data("Finite".utf8)
        tags.append(contentsOf: withUnsafeBytes(of: UInt32(vendor.count).littleEndian, Array.init))
        tags.append(vendor)
        tags.append(contentsOf: [0, 0, 0, 0])
        var out = Data()
        var sequence: UInt32 = 0
        func page(_ packets: [Data], granule: UInt64, flags: UInt8) throws {
            var lacing: [UInt8] = []
            for packet in packets {
                lacing.append(contentsOf: Array(repeating: 255, count: packet.count / 255))
                lacing.append(UInt8(packet.count % 255))
            }
            guard lacing.count <= segmentsMax else { throw WebMOpus.Refusal.badPacket }
            var page = Data("OggS".utf8)
            page.append(contentsOf: [0, flags])
            page.append(contentsOf: withUnsafeBytes(of: granule.littleEndian, Array.init))
            page.append(contentsOf: withUnsafeBytes(of: serial.littleEndian, Array.init))
            page.append(contentsOf: withUnsafeBytes(of: sequence.littleEndian, Array.init))
            page.append(contentsOf: [0, 0, 0, 0])
            page.append(UInt8(lacing.count))
            page.append(contentsOf: lacing)
            for packet in packets { page.append(packet) }
            let crc = crc32(page)
            page.replaceSubrange(22..<26, with: withUnsafeBytes(of: crc.littleEndian, Array.init))
            out.append(page)
            sequence += 1
        }
        try page([head], granule: 0, flags: 0x02)
        try page([tags], granule: 0, flags: 0)
        var granule: UInt64 = 0
        var held: [Data] = []
        var segments = 0
        for (index, packet) in packets.enumerated() {
            guard packet.count <= WebMOpus.packetBytesMax, let count = samples(packet) else { throw WebMOpus.Refusal.badPacket }
            let needs = packet.count / 255 + 1
            if !held.isEmpty, segments + needs > segmentsMax || held.count >= 50 {
                try page(held, granule: granule, flags: 0)
                held = []; segments = 0
            }
            held.append(packet); segments += needs
            granule += UInt64(count)
            if index == packets.count - 1 { try page(held, granule: granule, flags: 0x04) }
        }
        return out
    }

    /// An Opus packet's samples at 48 kHz, from its TOC byte (RFC 6716,
    /// 3.1–3.2): nil for a packet that names no frames, or more than 120 ms.
    static func samples(_ packet: Data) -> Int? {
        guard let toc = packet.first else { return nil }
        let config = Int(toc >> 3)
        let frame: Int
        switch config {
        case 0...11: frame = [480, 960, 1920, 2880][config % 4]
        case 12...15: frame = [480, 960][config % 2]
        default: frame = [120, 240, 480, 960][config % 4]
        }
        let frames: Int
        switch toc & 3 {
        case 0: frames = 1
        case 1, 2: frames = 2
        default:
            guard packet.count > 1 else { return nil }
            frames = Int(packet[packet.startIndex + 1] & 0x3F)
        }
        let total = frame * frames
        return frames >= 1 && total <= 5_760 ? total : nil
    }

    private static let table: [UInt32] = (0..<256).map { index in
        var value = UInt32(index) << 24
        for _ in 0..<8 { value = value & 0x8000_0000 != 0 ? value << 1 ^ 0x04C1_1DB7 : value << 1 }
        return value
    }
    /// Ogg's CRC-32: polynomial 0x04C11DB7, not reflected, from 0.
    static func crc32(_ data: Data) -> UInt32 {
        data.reduce(UInt32(0)) { crc, byte in crc << 8 ^ table[Int((crc >> 24) ^ UInt32(byte))] }
    }
}

extension ChatAttachment {
    /// Its size, as a person reads it ("1.2 MB").
    var shownSize: String { ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file) }
    /// Its type, as a person reads it ("PDF", "ZIP"), or nothing.
    var shownType: String? {
        let type = UTType(mimeType: AttachmentKind.essence(self.type)) ?? UTType(filenameExtension: (name as NSString).pathExtension)
        return type?.preferredFilenameExtension?.uppercased()
    }
    /// The symbol a file's row shows.
    var symbol: String {
        let type = UTType(mimeType: AttachmentKind.essence(self.type)) ?? UTType(filenameExtension: (name as NSString).pathExtension) ?? .data
        if type.conforms(to: .movie) { return "film" }
        if type.conforms(to: .audio) { return "waveform" }
        if type.conforms(to: .image) { return "photo" }
        if type.conforms(to: .archive) { return "doc.zipper" }
        if type.conforms(to: .spreadsheet) { return "tablecells" }
        if type.conforms(to: .presentation) { return "rectangle.on.rectangle" }
        if type.conforms(to: .sourceCode) || type.conforms(to: .json) { return "chevron.left.forwardslash.chevron.right" }
        if type.conforms(to: .text) { return "doc.text" }
        return "doc"
    }
}

/// A blob's hash, as a blob is named.
func sha256Hex(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
