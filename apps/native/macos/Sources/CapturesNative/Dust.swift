import AppKit
import CoreImage
import Metal
import QuartzCore
import CCapturesSettings

struct DustFixture: Decodable {
    let particles: [ThumbnailDustParticle]

    static func load() throws -> DustFixture {
        let url = NativeResources.bundle.url(forResource: "dust", withExtension: "json")!
        return try JSONDecoder().decode(Self.self, from: Data(contentsOf: url))
    }
}

enum NativeError: Error { case noMetal, renderFailed }

// One instance per preview surface, no global/unbounded image cache.
// Compare atlas=false with the original per-chip filtering path on identical input.
final class DustTextures {
    let context: CIContext
    let deviceName: String

    init() throws {
        guard let device = MTLCreateSystemDefaultDevice() else { throw NativeError.noMetal }
        deviceName = device.name
        context = CIContext(mtlDevice: device, options: [
            .workingColorSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
            .cacheIntermediates: false,
        ])
    }

    func render(_ image: CIImage, rect: CGRect) throws -> CGImage {
        guard let result = context.createCGImage(image, from: rect, format: .RGBA8,
            colorSpace: CGColorSpace(name: CGColorSpace.sRGB)) else { throw NativeError.renderFailed }
        return result
    }

    func filter(_ image: CIImage, scale: CGFloat) -> CIImage {
        image.applyingFilter("CIGaussianBlur", parameters: [kCIInputRadiusKey: 2 * scale])
            .applyingFilter("CIColorMatrix", parameters: [
                "inputRVector": CIVector(x: 0.5, y: 0, z: 0, w: 0),
                "inputGVector": CIVector(x: 0, y: 0.5, z: 0, w: 0),
                "inputBVector": CIVector(x: 0, y: 0, z: 0.5, w: 0),
                "inputAVector": CIVector(x: 0, y: 0, z: 0, w: 1),
            ])
    }

    struct Chip {
        let image: CGImage
        let size: CGSize
    }

    func prepare(source: CGImage, particles: [ThumbnailDustParticle], scale: CGFloat,
                 atlas: Bool) throws -> [Chip] {
        let image = CIImage(cgImage: source)
        let pad: CGFloat = 8
        let columns = Int(ceil(sqrt(Double(particles.count))))
        guard columns > 0 else { return [] }
        // 16pt extra gutter keeps neighboring blur kernels outside each chip's
        // 8pt visible fringe. Filter slices, never the intact source image.
        let cellW = ceil((particles.map(\.width).max()! + 2 * pad + 16) * scale)
        let cellH = ceil((particles.map(\.height).max()! + 2 * pad + 16) * scale)
        let extent = CGRect(x: 0, y: 0, width: CGFloat(columns) * cellW,
            height: CGFloat((particles.count + columns - 1) / columns) * cellH)
        var packed = CIImage(color: .clear).cropped(to: extent)
        var rects: [CGRect] = []
        var chips: [Chip] = []
        for (index, p) in particles.enumerated() {
            let slice = CGRect(x: p.sourceLeft * scale,
                y: (160 - p.sourceTop - p.height) * scale,
                width: p.width * scale, height: p.height * scale)
            let padded = slice.insetBy(dx: -pad * scale, dy: -pad * scale)
            if !atlas {
                let clear = CIImage(color: .clear).cropped(to: padded)
                let filtered = filter(image.cropped(to: slice).composited(over: clear), scale: scale)
                chips.append(Chip(image: try render(filtered, rect: padded.integral),
                    size: CGSize(width: p.width + 2 * pad, height: p.height + 2 * pad)))
                continue
            }
            // Preserve the source's fractional sampling phase. Translating the
            // fractional padded origin to an integer atlas slot resamples edges.
            // createCGImage reads integral pixel bounds on the reference path.
            let rasterBounds = padded.integral
            let tile = CGRect(x: CGFloat(index % columns) * cellW + 8 * scale,
                y: CGFloat(index / columns) * cellH + 8 * scale,
                width: rasterBounds.width, height: rasterBounds.height)
            let moved = image.cropped(to: slice).transformed(by:
                CGAffineTransform(translationX: tile.minX - rasterBounds.minX, y: tile.minY - rasterBounds.minY))
            packed = moved.composited(over: packed)
            rects.append(tile)
        }
        guard atlas else { return chips }
        // One GPU evaluation/readback instead of 198 createCGImage calls.
        let texture = try render(filter(packed, scale: scale), rect: extent)
        return try zip(rects, particles).map { rect, particle in
            // Crop in integer CGImage pixels (top-left origin) after the single
            // filter render. CALayer.contentsRect changes interpretation under
            // flipped ancestors; a full-image chip works in either hierarchy.
            let pixels = CGRect(x: rect.minX, y: extent.maxY - rect.maxY,
                width: rect.width, height: rect.height)
            guard let image = texture.cropping(to: pixels) else { throw NativeError.renderFailed }
            return Chip(image: image,
                size: CGSize(width: particle.width + 2 * pad, height: particle.height + 2 * pad))
        }
    }
}

final class PreviewView: NSView {
    override var isFlipped: Bool { true }
    private let card = CALayer()
    private let survivor = CALayer()
    private let dust = CALayer()
    private let maskLayer = CAShapeLayer()
    private let fixture: DustFixture
    private let textures: DustTextures
    private let atlas: Bool
    private var source: CGImage?
    private var cache: [DustTextures.Chip]?
    private var cachedScale: CGFloat = 0
    private var running = false
    private var generation = 0

    init(frame: NSRect, atlas: Bool) throws {
        self.atlas = atlas
        fixture = try DustFixture.load()
        textures = try DustTextures()
        super.init(frame: frame)
        wantsLayer = true
        layer!.isGeometryFlipped = true
        layer!.addSublayer(survivor)
        layer!.addSublayer(card)
        dust.frame = CGRect(x: 0, y: 144, width: 524, height: 400)
        dust.mask = maskLayer
        layer!.addSublayer(dust)
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        setAccessibilityLabel("Synthetic mini previews. Use the Dissolve button to test deletion.")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        reset()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { reset() }
    }

    func reset() {
        generation += 1
        running = false
        let scale = window?.backingScaleFactor ?? 2
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        dust.sublayers = nil
        dust.removeAllAnimations()
        maskLayer.removeAllAnimations()
        survivor.removeAllAnimations()
        card.removeAllAnimations()
        card.opacity = 1
        survivor.transform = CATransform3DIdentity
        dust.opacity = 0
        if source == nil || cachedScale != scale {
            let start = CACurrentMediaTime()
            source = Self.fixtureImage(scale: scale)
            cache = nil
            cachedScale = scale
            Metrics.emit("fixture-raster", milliseconds: (CACurrentMediaTime() - start) * 1000)
        }
        card.frame = CGRect(x: 120, y: 264, width: 284, height: 160)
        survivor.frame = CGRect(x: 120, y: 80, width: 284, height: 160)
        for item in [card, survivor] {
            item.contents = source
            item.contentsScale = scale
            item.cornerRadius = 12 // Shipping preview geometry, not a general token.
            item.masksToBounds = true
        }
    }

    // Synthetic asymmetric content; never sample the user's desktop for a benchmark.
    static func fixtureImage(scale: CGFloat) -> CGImage {
        let width = Int(284 * scale), height = Int(160 * scale)
        let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.scaleBy(x: scale, y: scale)
        context.addPath(CGPath(roundedRect: CGRect(x: 0, y: 0, width: 284, height: 160),
            cornerWidth: 12, cornerHeight: 12, transform: nil))
        context.clip()
        context.setFillColor(CGColor(srgbRed: 0.12, green: 0.27, blue: 0.42, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 284, height: 160))
        context.setFillColor(CGColor(srgbRed: 0.2, green: 0.48, blue: 0.4, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 284, height: 45))
        context.setFillColor(CGColor(srgbRed: 0.96, green: 0.74, blue: 0.29, alpha: 1))
        context.fillEllipse(in: CGRect(x: 218, y: 108, width: 30, height: 30))
        context.setFillColor(CGColor(srgbRed: 0.85, green: 0.33, blue: 0.41, alpha: 1))
        context.fill(CGRect(x: 38, y: 50, width: 75, height: 80))
        return context.makeImage()!
    }

    func dissolve(cold: Bool) {
        guard !running, let source else { return }
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            card.opacity = 0
            survivor.transform = CATransform3DMakeTranslation(0, 184, 0)
            CATransaction.commit()
            Metrics.emit("reduced-motion", milliseconds: 0)
            return
        }
        let totalStart = CACurrentMediaTime()
        let preparation = CACurrentMediaTime()
        let cacheHit = !cold && cache != nil
        do {
            if !cacheHit {
                cache = try textures.prepare(source: source, particles: fixture.particles,
                    scale: cachedScale, atlas: atlas)
            }
        } catch {
            Metrics.emit("effect-error", milliseconds: 0, detail: String(describing: error))
            return
        }
        Metrics.emit("texture-preparation", milliseconds: (CACurrentMediaTime() - preparation) * 1000,
            detail: "\(atlas ? "atlas" : "per-chip"), cacheHit=\(cacheHit), \(textures.deviceName)")
        let construction = CACurrentMediaTime()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        dust.sublayers = nil
        dust.opacity = 0 // Final model state; animation supplies visibility.
        let duration = 2.55
        // Sample the tested scalar curve once, then let Core Animation interpolate.
        // No application display link, recurring timer, or CPU raster loop.
        let times = (0...306).map { Double($0) / 306 * duration }
        for (index, particle) in fixture.particles.enumerated() {
            let chip = cache![index]
            let layer = CALayer()
            layer.bounds = CGRect(origin: .zero, size: chip.size)
            layer.position = CGPoint(x: particle.left + particle.width / 2,
                y: particle.top + particle.height / 2)
            layer.contents = chip.image
            layer.contentsScale = cachedScale
            let poses = times.map { thumbnailDustVisualAt(particle, elapsedMs: $0 * 1000) }
            let transforms: [NSValue] = poses.map { pose in
                var t = CATransform3DMakeTranslation(pose.dx, pose.dy, 0)
                t = CATransform3DRotate(t, pose.rotate * .pi / 180, 0, 0, 1)
                return NSValue(caTransform3D: CATransform3DScale(t, pose.scale, pose.scale, 1))
            }
            animate(layer, key: "transform", values: transforms, duration: duration)
            animate(layer, key: "opacity", values: poses.map { NSNumber(value: $0.opacity) }, duration: duration)
            dust.addSublayer(layer)
        }
        let paths: [CGPath] = times.map { time in
            let linear = time / duration
            let opening = linear <= 0.08 ? 0 : linear >= 0.7 ? 1
                : cubicBezierProgress(0.33, 0, 0.2, 1, (linear - 0.08) / 0.62)
            return CGPath(roundedRect: dust.bounds.insetBy(dx: 120 * (1 - opening), dy: 120 * (1 - opening)),
                cornerWidth: 12 * (1 - opening), cornerHeight: 12 * (1 - opening), transform: nil)
        }
        animate(maskLayer, key: "path", values: paths, duration: duration)
        animate(dust, key: "opacity", values: times.map { time -> NSNumber in
            let linear = time / duration
            let fade = linear <= 0.7 ? 0 : linear >= 0.9 ? 1
                : cubicBezierProgress(0.33, 0, 0.2, 1, (linear - 0.7) / 0.2)
            return NSNumber(value: 1 - fade)
        }, duration: duration)
        card.opacity = 0
        animate(card, key: "opacity", values: times.map { time -> NSNumber in
            let linear = min(1, time / 0.55)
            return NSNumber(value: linear <= 0.2 ? 1
                : 1 - cubicBezierProgress(0.22, 0.1, 0.25, 1, (linear - 0.2) / 0.8))
        }, duration: duration)
        survivor.transform = CATransform3DMakeTranslation(0, 184, 0)
        animate(survivor, key: "transform.translation.y", values: times.map { time in
            NSNumber(value: 184 * cubicBezierProgress(0.4, 0, 0.2, 1, min(1, max(0, (time - 1.8) / 0.58))))
        }, duration: duration)
        CATransaction.commit()
        Metrics.emit("layers-and-animation-submission", milliseconds: (CACurrentMediaTime() - construction) * 1000)
        Metrics.emit("first-action-total", milliseconds: (CACurrentMediaTime() - totalStart) * 1000)
        running = true
        let currentGeneration = generation
        DispatchQueue.main.asyncAfter(deadline: .now() + duration) { [weak self] in
            guard let self, self.generation == currentGeneration else { return }
            self.dust.sublayers = nil
            self.running = false
        }
    }

    private func animate(_ layer: CALayer, key: String, values: [Any], duration: Double) {
        let animation = CAKeyframeAnimation(keyPath: key)
        animation.values = values
        animation.duration = duration
        animation.calculationMode = .linear
        layer.add(animation, forKey: key)
    }
}

/// Preview stack exit timings, the dust pad and Delete's origin, and the pile
/// sparkle tables from `captures_app::preview_motion` (the settings ABI's
/// `captures_preview_motion_tables_v1`). Times are seconds.
struct PreviewMotionTables {
    struct Exit { let hold: Double; let settleDelay: Double }
    struct Dot { let x, y, core, fade, alpha: CGFloat; let accent: Bool }

    let dismiss, dust, fallback: Exit
    let dustPad: CGFloat
    let deleteOriginFirstX, deleteOriginAfterCloseX, deleteOriginY: CGFloat
    let reach, side, near: CGFloat
    let early, late: [Dot]

    static let shared = load()

    static func load() -> PreviewMotionTables {
        var root: [String: Any] = [:]
        if let raw = captures_preview_motion_tables_v1() {
            defer { captures_settings_free_v1(raw) }
            let text = String(cString: raw)
            root = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
        }
        let exits = root["exits"] as? [String: Any] ?? [:]
        let sparkles = root["sparkles"] as? [String: Any] ?? [:]
        let origin = exits["delete_origin"] as? [String: Any] ?? [:]
        func number(_ object: [String: Any], _ key: String) -> CGFloat {
            CGFloat((object[key] as? NSNumber)?.doubleValue ?? 0)
        }
        func exit(_ key: String) -> Exit {
            let value = exits[key] as? [String: Any] ?? [:]
            return Exit(hold: Double(number(value, "hold_ms")) / 1000,
                        settleDelay: Double(number(value, "settle_delay_ms")) / 1000)
        }
        func dots(_ key: String) -> [Dot] {
            (sparkles[key] as? [[String: Any]] ?? []).map { dot in
                Dot(x: number(dot, "x"), y: number(dot, "y"), core: number(dot, "core"),
                    fade: number(dot, "fade"), alpha: number(dot, "alpha"),
                    accent: dot["accent"] as? Bool ?? false)
            }
        }
        return PreviewMotionTables(dismiss: exit("dismiss"), dust: exit("dust"),
            fallback: exit("delete_fallback"), dustPad: number(exits, "dust_pad"),
            deleteOriginFirstX: number(origin, "first_x"),
            deleteOriginAfterCloseX: number(origin, "after_close_x"),
            deleteOriginY: number(origin, "y"),
            reach: number(sparkles, "reach"), side: number(sparkles, "side"),
            near: number(sparkles, "near"), early: dots("early"), late: dots("late"))
    }

    /// Shipping dust chips for a card (seeded, so a card always dissolves the
    /// same way).
    static func dustParticles(card: CGSize, image: CGSize, origin: CGPoint, seed: UInt32)
        -> [ThumbnailDustParticle] {
        guard let raw = captures_preview_dust_particles_v1(Double(card.width), Double(card.height),
                Double(image.width), Double(image.height), Double(origin.x), Double(origin.y), seed)
        else { return [] }
        defer { captures_settings_free_v1(raw) }
        let text = String(cString: raw)
        return (try? JSONDecoder().decode([ThumbnailDustParticle].self, from: Data(text.utf8))) ?? []
    }
}

/// Shipping dust delete on a live preview card, built like the Workbench
/// dissolve: pre-filtered chips sampled from the tested scalar pose, the clip
/// opening from the card's rounded rect, then the layer fading
/// (`thumbnail-dust-clip`). No display link or CPU raster loop.
enum DustDissolve {
    /// Adds chip layers to `layer` (geometry-flipped, covering the card padded
    /// by `pad` on every side) and returns the seconds the dissolve plays.
    @discardableResult
    static func play(in layer: CALayer, chips: [DustTextures.Chip], particles: [ThumbnailDustParticle],
                     pad: CGFloat, radius: CGFloat, scale: CGFloat) -> Double {
        let duration = 2.55
        let times = (0...306).map { Double($0) / 306 * duration }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        layer.sublayers = nil
        layer.opacity = 0 // Final model state; the animation supplies visibility.
        for (particle, chip) in zip(particles, chips) {
            let sublayer = CALayer()
            sublayer.bounds = CGRect(origin: .zero, size: chip.size)
            sublayer.position = CGPoint(x: particle.left + particle.width / 2,
                                        y: particle.top + particle.height / 2)
            sublayer.contents = chip.image
            sublayer.contentsScale = scale
            let poses = times.map { thumbnailDustVisualAt(particle, elapsedMs: $0 * 1000) }
            animate(sublayer, key: "transform", values: poses.map { pose -> NSValue in
                var t = CATransform3DMakeTranslation(pose.dx, pose.dy, 0)
                t = CATransform3DRotate(t, pose.rotate * .pi / 180, 0, 0, 1)
                return NSValue(caTransform3D: CATransform3DScale(t, pose.scale, pose.scale, 1))
            }, duration: duration)
            animate(sublayer, key: "opacity", values: poses.map { NSNumber(value: $0.opacity) },
                    duration: duration)
            layer.addSublayer(sublayer)
        }
        let mask = CAShapeLayer()
        mask.frame = layer.bounds
        layer.mask = mask
        let paths: [CGPath] = times.map { time in
            let linear = time / duration
            let opening = linear <= 0.08 ? 0 : linear >= 0.7 ? 1
                : cubicBezierProgress(0.33, 0, 0.2, 1, (linear - 0.08) / 0.62)
            let inset = pad * CGFloat(1 - opening)
            let corner = radius * CGFloat(1 - opening)
            return CGPath(roundedRect: layer.bounds.insetBy(dx: inset, dy: inset),
                          cornerWidth: corner, cornerHeight: corner, transform: nil)
        }
        animate(mask, key: "path", values: paths, duration: duration)
        animate(layer, key: "opacity", values: times.map { time -> NSNumber in
            let linear = time / duration
            let fade = linear <= 0.7 ? 0 : linear >= 0.9 ? 1
                : cubicBezierProgress(0.33, 0, 0.2, 1, (linear - 0.7) / 0.2)
            return NSNumber(value: 1 - fade)
        }, duration: duration)
        return duration
    }

    private static func animate(_ layer: CALayer, key: String, values: [Any], duration: Double) {
        let animation = CAKeyframeAnimation(keyPath: key)
        animation.values = values
        animation.duration = duration
        animation.calculationMode = .linear
        animation.fillMode = .forwards
        animation.isRemovedOnCompletion = false
        layer.add(animation, forKey: key)
    }
}

/// A decorative, input-transparent overlay whose layer uses y-down geometry
/// (dust chips and pile sparkles).
final class PreviewMotionOverlay: NSView {
    override var isFlipped: Bool { true }
    let content = CALayer()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer!.isGeometryFlipped = true
        content.frame = bounds
        layer!.addSublayer(content)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
