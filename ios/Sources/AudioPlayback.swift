import AVFoundation
import Observation

/// A sound being played: AVPlayer's, or a test's.
@MainActor
protocol PlaybackEngine: AnyObject {
    /// Its length in seconds, 0 until known.
    var duration: Double { get }
    var elapsed: Double { get }
    /// Whether it played to its end.
    var ended: Bool { get }
    func play()
    func pause()
    func seek(_ seconds: Double)
}

/// The chat's one player: a sound plays alone, so playing one stops any
/// other, and leaving the chat stops it (`stop`, from ChatView). Each
/// player shows its own sound's place from here, and only the one playing
/// redraws as it plays.
///
/// A sound plays as Messages plays one tapped: aloud on the speaker (never
/// the earpiece a recording's session would route it to), with the
/// silent switch on or off, as a tap asked for it; others' audio pauses
/// meanwhile and resumes after. A call or unplugged headphones pause it.
@MainActor @Observable
final class AudioPlayback {
    static let shared = AudioPlayback(engine: { AVPlayerEngine(url: $0) }, session: .system, pausesForTheSystem: true)

    struct Session {
        let activate: @MainActor () -> Void
        let deactivate: @MainActor () -> Void
        static let system = Session(activate: {
            let session = AVAudioSession.sharedInstance()
            try? session.setCategory(.playback, mode: .spokenAudio)
            try? session.setActive(true)
        }, deactivate: {
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        })
        static let none = Session(activate: {}, deactivate: {})
    }

    /// The sound in the player (playing or paused), by its id.
    private(set) var current: String?
    private(set) var playing = false
    private(set) var elapsed: Double = 0
    private(set) var duration: Double = 0

    @ObservationIgnored private let makeEngine: @MainActor (URL) -> PlaybackEngine
    @ObservationIgnored private let session: Session
    @ObservationIgnored private var engine: PlaybackEngine?
    @ObservationIgnored private var ticking: Task<Void, Never>?
    @ObservationIgnored private var observers: [NSObjectProtocol] = []

    init(engine: @escaping @MainActor (URL) -> PlaybackEngine, session: Session, pausesForTheSystem: Bool = false) {
        makeEngine = engine
        self.session = session
        guard pausesForTheSystem else { return }
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            let began = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt) == AVAudioSession.InterruptionType.began.rawValue
            if began { MainActor.assumeIsolated { self?.pause() } }
        })
        observers.append(center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
            let reason = (note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt) ?? 0
            if reason == AVAudioSession.RouteChangeReason.oldDeviceUnavailable.rawValue { MainActor.assumeIsolated { self?.pause() } }
        })
    }

    /// Plays `id` (from `url`), or pauses it if it is playing; any other stops.
    func toggle(_ id: String, url: URL) {
        if current == id, playing { pause(); return }
        if current != id { load(id, url: url) }
        guard let engine else { return }
        if engine.ended || (duration > 0 && elapsed >= duration) { engine.seek(0); elapsed = 0 }
        session.activate()
        engine.play()
        playing = true
        tick()
    }

    func pause() {
        guard playing else { return }
        engine?.pause()
        playing = false
        ticking?.cancel(); ticking = nil
        if let engine { elapsed = engine.elapsed }
    }

    /// Moves `id`'s place to `fraction` of it (`length` seconds, as read
    /// before it plays: a player just made knows none yet), loading it
    /// (paused) if it is not the one in the player.
    func seek(_ id: String, url: URL, to fraction: Double, of length: Double) {
        if current != id { load(id, url: url) }
        guard let engine else { return }
        if duration == 0 { duration = length }
        let length = engine.duration > 0 ? engine.duration : length
        let seconds = max(0, min(1, fraction)) * length
        engine.seek(seconds)
        elapsed = seconds
    }

    /// Stops whatever plays, and lets the player go.
    func stop() {
        ticking?.cancel(); ticking = nil
        engine?.pause()
        let was = engine != nil
        engine = nil
        current = nil; playing = false; elapsed = 0; duration = 0
        if was { session.deactivate() }
    }

    /// Where `id` is in the player, as its view shows it: its place when it
    /// is the one in the player, else its start.
    func place(_ id: String) -> (elapsed: Double, playing: Bool)? {
        current == id ? (elapsed, playing) : nil
    }

    private func load(_ id: String, url: URL) {
        stop()
        engine = makeEngine(url)
        current = id
        duration = engine?.duration ?? 0
    }

    private func tick() {
        ticking?.cancel()
        ticking = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, let engine = self.engine else { return }
                if engine.duration > 0, engine.duration != self.duration { self.duration = engine.duration }
                let now = engine.elapsed
                if engine.ended {
                    // played to its end: back to its start, paused, as Messages leaves one
                    engine.pause(); engine.seek(0)
                    self.elapsed = 0; self.playing = false
                    self.session.deactivate()
                    return
                }
                if abs(now - self.elapsed) >= 0.03 { self.elapsed = now }
                do { try await Task.sleep(for: .milliseconds(50)) } catch { return }
            }
        }
    }
}

/// AVPlayer, which plays every container `AudioContainer` calls native
/// (fragmented MP4 among them, whose length AVAudioPlayer misreads).
@MainActor
final class AVPlayerEngine: PlaybackEngine {
    private let player: AVPlayer
    private(set) var ended = false
    private var observer: NSObjectProtocol?

    init(url: URL) {
        let item = AVPlayerItem(url: url)
        player = AVPlayer(playerItem: item)
        player.automaticallyWaitsToMinimizeStalling = false
        observer = NotificationCenter.default.addObserver(forName: AVPlayerItem.didPlayToEndTimeNotification, object: item, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.ended = true }
        }
    }
    isolated deinit { if let observer { NotificationCenter.default.removeObserver(observer) } }

    var duration: Double {
        let seconds = player.currentItem?.duration.seconds ?? 0
        return seconds.isFinite && seconds > 0 ? seconds : 0
    }
    var elapsed: Double { max(0, player.currentTime().seconds.isFinite ? player.currentTime().seconds : 0) }
    func play() { ended = false; player.play() }
    func pause() { player.pause() }
    func seek(_ seconds: Double) {
        ended = false
        player.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
    }
}

/// What a sound's player shows before it plays: its length and its level
/// bars, read once from its file and kept by its id (a few numbers each).
@MainActor
final class SoundFacts {
    static let shared = SoundFacts()
    nonisolated static let bars = 32
    /// A sound longer than this shows even bars: its levels are not read.
    nonisolated static let levelsSecondsMax: Double = 20 * 60
    /// Sounds whose facts are kept at most; past it, all go and are read again.
    static let keptMax = 2_000

    struct Facts: Equatable, Sendable { let duration: Double; let levels: [Float] }
    private var kept: [String: Facts] = [:]
    private var reading: [String: Task<Facts?, Never>] = [:]

    func cached(_ id: String) -> Facts? { kept[id] }

    func facts(_ id: String, url: URL) async -> Facts? {
        if let facts = kept[id] { return facts }
        if let task = reading[id] { return await task.value }
        let task = Task.detached(priority: .utility) { await Self.read(url) }
        reading[id] = task
        let facts = await task.value
        reading[id] = nil
        if let facts {
            if kept.count >= Self.keptMax { kept.removeAll() }
            kept[id] = facts
        }
        return facts
    }

    func removeAll() { kept.removeAll() }

    /// Its length, and its loudness in `bars` even slices (0 to 1).
    nonisolated static func read(_ url: URL) async -> Facts? {
        let asset = AVURLAsset(url: url)
        guard let duration = try? await asset.load(.duration).seconds, duration.isFinite, duration > 0 else { return nil }
        guard duration <= levelsSecondsMax, let track = try? await asset.loadTracks(withMediaType: .audio).first,
              let reader = try? AVAssetReader(asset: asset) else { return Facts(duration: duration, levels: []) }
        let output = AVAssetReaderTrackOutput(track: track, outputSettings: [
            AVFormatIDKey: kAudioFormatLinearPCM, AVLinearPCMBitDepthKey: 16, AVLinearPCMIsFloatKey: false,
            AVLinearPCMIsBigEndianKey: false, AVLinearPCMIsNonInterleaved: false, AVNumberOfChannelsKey: 1, AVSampleRateKey: 8_000,
        ])
        guard reader.canAdd(output) else { return Facts(duration: duration, levels: []) }
        reader.add(output)
        guard reader.startReading() else { return Facts(duration: duration, levels: []) }
        // the mean square of each 50 ms (400 samples at 8 kHz), then slices of those
        var windows: [Float] = []
        var sum: Float = 0
        var count = 0
        while let buffer = output.copyNextSampleBuffer(), let block = CMSampleBufferGetDataBuffer(buffer) {
            var length = 0
            var pointer: UnsafeMutablePointer<CChar>?
            guard CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length, dataPointerOut: &pointer) == noErr,
                  let pointer else { continue }
            pointer.withMemoryRebound(to: Int16.self, capacity: length / 2) { samples in
                for index in 0..<(length / 2) {
                    let value = Float(samples[index]) / 32_768
                    sum += value * value; count += 1
                    if count == 400 { windows.append(sum / 400); sum = 0; count = 0 }
                }
            }
        }
        if count > 0 { windows.append(sum / Float(count)) }
        return Facts(duration: duration, levels: levels(windows, bars: bars))
    }

    /// Mean squares made `bars` levels: each slice's loudest, as a share of
    /// the loudest of all, on a root scale (quiet speech still shows).
    nonisolated static func levels(_ windows: [Float], bars: Int) -> [Float] {
        guard !windows.isEmpty, bars > 0 else { return [] }
        let slices = (0..<bars).map { bar -> Float in
            let from = bar * windows.count / bars
            let to = max(from + 1, (bar + 1) * windows.count / bars)
            return windows[min(from, windows.count - 1)..<min(to, windows.count)].max() ?? 0
        }
        let loudest = slices.max() ?? 0
        guard loudest > 0 else { return Array(repeating: 0, count: bars) }
        return slices.map { sqrt(sqrt($0 / loudest)) }
    }
}
