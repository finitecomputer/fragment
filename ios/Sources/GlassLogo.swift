import CoreMotion
import SwiftUI

/// How the logo is turned, read once a frame. Fused device motion rather than
/// the raw accelerometer: gravity without the hand's shake, and no permission.
/// Polled by the frame, so sensor samples never reach SwiftUI's update cycle.
@MainActor
final class GlassTilt {
    private let motion = CMMotionManager()
    private var rest: SIMD2<Double>?
    private var tilt = SIMD2<Double>.zero
    private var center: Double?
    private var last: Date?
    private var drift = 0.0

    func start() {
        guard motion.isDeviceMotionAvailable, !motion.isDeviceMotionActive else { return }
        motion.deviceMotionUpdateInterval = 1 / 60
        motion.startDeviceMotionUpdates()
    }

    func stop() {
        motion.stopDeviceMotionUpdates()
        rest = nil
    }

    /// The side of the logo nearest the viewer, in screen axes (y runs down).
    /// The logo keeps still in the world as the phone turns around it, so the
    /// edge that tips away brings that side of the logo forward.
    nonisolated static func lean(gravityX: Double, gravityY: Double) -> SIMD2<Double> {
        SIMD2(gravityX, -gravityY)
    }

    /// Advances to `date`: the turn in radians and the logo's center, both
    /// eased the same at any frame rate. No place yet, so no center.
    func frame(_ date: Date, place: Double?) -> (tilt: SIMD2<Double>, center: Double?) {
        let step = min(last.map { date.timeIntervalSince($0) } ?? 0, 0.1)
        last = date
        drift += step
        // The float the web's logo has, so it lives on a phone at rest too.
        var target = SIMD2(cos(drift / 2.6), sin(drift / 2.6)) * 0.07
        if let gravity = motion.deviceMotion?.gravity {
            let now = Self.lean(gravityX: gravity.x, gravityY: gravity.y)
            // However the phone is held becomes straight on, after a moment.
            let settled = rest ?? now
            let next = settled + (now - settled) * (1 - exp(-step / 2.5))
            rest = next
            target += ((now - next) * 1.5).clamped(lowerBound: .init(repeating: -0.6), upperBound: .init(repeating: 0.6))
        }
        tilt += (target - tilt) * (1 - exp(-step / 0.07))
        guard let place else { return (tilt, nil) }
        let from = center ?? place
        let eased = from + (place - from) * (1 - exp(-step / 0.09))
        center = eased
        return (tilt, eased)
    }
}

private struct GlassLogoEffect: ViewModifier {
    let size: CGFloat
    let y: CGFloat?
    @State private var tilt = GlassTilt()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase
    private var still: Bool { reduceMotion || scenePhase != .active }

    func body(content: Content) -> some View {
        TimelineView(.animation(paused: still)) { timeline in
            let frame = reduceMotion ? (tilt: SIMD2<Double>.zero, center: y.map(Double.init)) : tilt.frame(timeline.date, place: y.map(Double.init))
            content.visualEffect { [size] effect, proxy in
                effect.layerEffect(
                    ShaderLibrary.glassLogo(
                        .float2(proxy.size),
                        .float2(proxy.size.width / 2, frame.center.map { CGFloat($0) - proxy.frame(in: .global).minY } ?? -size),
                        .float(size),
                        .float2(frame.tilt.x, frame.tilt.y),
                        .image(Image("GlassLogoHeight"))
                    ),
                    maxSampleOffset: CGSize(width: size / 2, height: size / 2)
                )
            }
        }
        .task(id: still) {
            if still { tilt.stop() } else { tilt.start() }
        }
        .onDisappear { tilt.stop() }
    }
}

extension View {
    /// The Finite logo as glass refracting this view, turning with the phone.
    /// `y` places its center in global coordinates; it waits for one.
    func glassLogo(size: CGFloat, y: CGFloat?) -> some View {
        modifier(GlassLogoEffect(size: size, y: y))
    }
}
