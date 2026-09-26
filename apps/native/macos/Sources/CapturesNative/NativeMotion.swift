import AppKit
import QuartzCore

/// One shipping animation from `captures_app::motion`, as the settings ABI's
/// `motion` operation describes it. Durations and easings name design tokens
/// where the shipping CSS does, and resolve against `Tokens`.
struct MotionKeyframes {
    enum Timing: Equatable { case token(String), millis(Double) }
    enum Easing: Equatable { case token(String), bezier([Double]) }
    struct Frame: Equatable {
        let offset: Double
        let opacity: Double
        /// Points, positive downward as in CSS.
        let translateY: Double
        let scale: Double
    }

    let duration: Timing
    let delayMs: Double
    let easing: Easing
    let frames: [Frame]

    static func timing(_ value: Any?) -> Timing? {
        guard let object = value as? [String: Any] else { return nil }
        if let token = object["token"] as? String { return .token(token) }
        if let millis = object["millis"] as? NSNumber { return .millis(millis.doubleValue) }
        return nil
    }

    static func easing(_ value: Any?) -> Easing? {
        guard let object = value as? [String: Any] else { return nil }
        if let token = object["token"] as? String { return .token(token) }
        if let points = object["bezier"] as? [NSNumber], points.count == 4 {
            return .bezier(points.map(\.doubleValue))
        }
        return nil
    }

    init?(_ value: Any?) {
        guard let object = value as? [String: Any],
              let duration = Self.timing(object["duration"]),
              let easing = Self.easing(object["easing"]),
              let frames = object["frames"] as? [[String: Any]], frames.count >= 2 else { return nil }
        self.duration = duration
        self.easing = easing
        self.delayMs = (object["delay_ms"] as? NSNumber)?.doubleValue ?? 0
        self.frames = frames.map { frame in
            Frame(offset: (frame["offset"] as? NSNumber)?.doubleValue ?? 0,
                  opacity: (frame["opacity"] as? NSNumber)?.doubleValue ?? 1,
                  translateY: (frame["translate_y"] as? NSNumber)?.doubleValue ?? 0,
                  scale: (frame["scale"] as? NSNumber)?.doubleValue ?? 1)
        }
    }

    /// Offsets of the steady middle of a lifecycle (first and last resting frame).
    var restOffsets: (first: Double, last: Double) {
        let rest = frames.filter { $0.opacity == 1 && $0.translateY == 0 && $0.scale == 1 }
        return (rest.first?.offset ?? 0, rest.last?.offset ?? 1)
    }
}

/// Shipping motion for AppKit, played by Core Animation.
///
/// Animations are presentation-only: they are added to a view's layer and
/// never change its model frame, alpha or transform, so code and tests that
/// read geometry immediately see the settled state. Reduced motion follows
/// the shipping global rule: entrances are skipped (they settle at rest), and
/// an exit keeps its delay and then lands on its final keyframe.
enum NativeMotion {
    static let animationKey = "captures.motion"

    struct Catalog {
        let keyframes: [String: MotionKeyframes]
        let transitions: [String: (duration: MotionKeyframes.Timing, easing: MotionKeyframes.Easing)]
    }

    static let catalog: Catalog = load(transport: SettingsBridge())

    static func load(transport: SettingsTransport) -> Catalog {
        guard let response = try? transport.request(["operation": "motion"]),
              let motion = response["motion"] as? [String: Any] else {
            return Catalog(keyframes: [:], transitions: [:])
        }
        var keyframes: [String: MotionKeyframes] = [:]
        for (name, value) in motion["keyframes"] as? [String: Any] ?? [:] {
            keyframes[name] = MotionKeyframes(value)
        }
        var transitions: [String: (duration: MotionKeyframes.Timing, easing: MotionKeyframes.Easing)] = [:]
        for (name, value) in motion["transitions"] as? [String: Any] ?? [:] {
            guard let object = value as? [String: Any],
                  let duration = MotionKeyframes.timing(object["duration"]),
                  let easing = MotionKeyframes.easing(object["easing"]) else { continue }
            transitions[name] = (duration, easing)
        }
        return Catalog(keyframes: keyframes, transitions: transitions)
    }

    static var reduceMotion: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    static func seconds(_ timing: MotionKeyframes.Timing, tokens: Tokens) -> Double {
        switch timing {
        case .token(let name): return Double(tokens.number(name)) / 1000
        case .millis(let millis): return millis / 1000
        }
    }

    static func controlPoints(_ easing: MotionKeyframes.Easing, tokens: Tokens) -> [Double] {
        switch easing {
        case .token(let name): return tokens.easing(name)
        case .bezier(let points): return points
        }
    }

    static func timingFunction(_ easing: MotionKeyframes.Easing, tokens: Tokens) -> CAMediaTimingFunction {
        let p = controlPoints(easing, tokens: tokens).map { Float($0) }
        return CAMediaTimingFunction(controlPoints: p[0], p[1], p[2], p[3])
    }

    /// A shipping transition's duration (0 under reduced motion) and timing.
    static func transition(_ name: String, tokens: Tokens,
                           reduced: Bool = NativeMotion.reduceMotion) -> (duration: Double, timing: CAMediaTimingFunction) {
        guard let spec = catalog.transitions[name] else {
            return (0, CAMediaTimingFunction(name: .linear))
        }
        return (reduced ? 0 : seconds(spec.duration, tokens: tokens),
                timingFunction(spec.easing, tokens: tokens))
    }

    /// CSS `translateY() scale()` about the layer's centre. `down` is +1 when
    /// the superlayer's y axis points down (a flipped superview), else -1.
    static func transform(_ frame: MotionKeyframes.Frame, layer: CALayer, down: CGFloat) -> CATransform3D {
        let size = layer.bounds.size
        let cx = (0.5 - layer.anchorPoint.x) * size.width
        let cy = (0.5 - layer.anchorPoint.y) * size.height
        var t = CATransform3DMakeTranslation(cx, cy + down * CGFloat(frame.translateY), 0)
        t = CATransform3DScale(t, CGFloat(frame.scale), CGFloat(frame.scale), 1)
        return CATransform3DTranslate(t, -cx, -cy, 0)
    }

    /// Plays `name` (optionally only the `segment` of its keyframe offsets, for
    /// a lifecycle's entrance or exit) on `view`'s layer. `holdEnd` keeps the
    /// final keyframe on screen afterwards, for exits whose window closes
    /// later. Returns the seconds until the last keyframe, or 0 when nothing
    /// plays. The layer's model values are never changed.
    @discardableResult
    static func play(_ name: String, on view: NSView, tokens: Tokens,
                     segment: ClosedRange<Double> = 0...1, holdEnd: Bool = false,
                     delay extraDelay: Double = 0, reduced: Bool = NativeMotion.reduceMotion) -> Double {
        guard let spec = catalog.keyframes[name] else { return 0 }
        if reduced && !holdEnd { return 0 }
        view.wantsLayer = true
        guard let layer = view.layer else { return 0 }
        let span = segment.upperBound - segment.lowerBound
        let frames = spec.frames.filter { segment.contains($0.offset) }
        guard span > 0, frames.count >= 2 else { return 0 }
        let delay = spec.delayMs / 1000 + extraDelay
        // Shipping's reduced-motion rule keeps the delay, then a 0.01 ms run.
        let duration = reduced ? 0.00001 : seconds(spec.duration, tokens: tokens) * span
        let flipped = layer.superlayer?.contentsAreFlipped() ?? view.superview?.isFlipped ?? false
        let down: CGFloat = flipped ? 1 : -1
        let keyTimes = frames.map { NSNumber(value: ($0.offset - segment.lowerBound) / span) }
        let timing = timingFunction(spec.easing, tokens: tokens)
        let segments = Array(repeating: timing, count: frames.count - 1)

        let opacity = CAKeyframeAnimation(keyPath: "opacity")
        opacity.values = frames.map { NSNumber(value: $0.opacity) }
        opacity.keyTimes = keyTimes
        opacity.timingFunctions = segments
        let move = CAKeyframeAnimation(keyPath: "transform")
        move.values = frames.map { NSValue(caTransform3D: transform($0, layer: layer, down: down)) }
        move.keyTimes = keyTimes
        move.timingFunctions = segments

        let group = CAAnimationGroup()
        group.animations = [opacity, move]
        group.duration = duration
        group.beginTime = layer.convertTime(CACurrentMediaTime(), from: nil) + delay
        group.fillMode = holdEnd ? .both : .backwards
        group.isRemovedOnCompletion = !holdEnd
        layer.add(group, forKey: animationKey)
        return delay + duration
    }

    /// Removes any playing or held motion, returning the view to rest.
    static func cancel(on view: NSView) {
        view.layer?.removeAnimation(forKey: animationKey)
    }

    static func isPlaying(on view: NSView) -> Bool {
        view.layer?.animation(forKey: animationKey) != nil
    }

    /// Lifecycle helpers: the entrance runs up to the first resting keyframe,
    /// the exit from the last resting keyframe to the end.
    @discardableResult
    static func playEntrance(_ name: String, on view: NSView, tokens: Tokens,
                             reduced: Bool = NativeMotion.reduceMotion) -> Double {
        guard !reduced, let spec = catalog.keyframes[name] else { return 0 }
        return play(name, on: view, tokens: tokens, segment: 0...spec.restOffsets.first, reduced: false)
    }

    /// Seconds the exit of a lifecycle takes, so a host can start it that long
    /// before the window closes.
    static func exitDuration(_ name: String, tokens: Tokens) -> Double {
        guard let spec = catalog.keyframes[name] else { return 0 }
        return seconds(spec.duration, tokens: tokens) * (1 - spec.restOffsets.last)
    }

    /// Under reduced motion a lifecycle notice stays still and visible until
    /// its window closes (shipping's 0.01 ms rule would hide it at once).
    @discardableResult
    static func playExit(_ name: String, on view: NSView, tokens: Tokens,
                         reduced: Bool = NativeMotion.reduceMotion) -> Double {
        guard !reduced, let spec = catalog.keyframes[name] else { return 0 }
        return play(name, on: view, tokens: tokens, segment: spec.restOffsets.last...1,
                    holdEnd: true, reduced: false)
    }
}
