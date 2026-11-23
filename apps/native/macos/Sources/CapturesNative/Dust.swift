import Accelerate
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

// Builds the pre-filtered chips a Delete dissolves into: each chip is a
// padded slice of the card, blurred and darkened. The shipping chips are
// filtered on the CPU with vImage. Core Image's first render in a process
// costs about 66 MB (a 4x4 render took the footprint from 6 to 72 MB, with
// the Metal or the software renderer), which was most of Delete's peak.
// `reference: true` renders each chip through Core Image, the original
// filtering the vImage chips are compared with.
final class DustTextures {
    let deviceName: String
    private let device: MTLDevice
    private lazy var context = CIContext(mtlDevice: device, options: [
        .workingColorSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
        .cacheIntermediates: false,
    ])

    init() throws {
        guard let device = MTLCreateSystemDefaultDevice() else { throw NativeError.noMetal }
        self.device = device
        deviceName = device.name
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
                 reference: Bool) throws -> [Chip] {
        let pad: CGFloat = 8
        // Slices in Core Image's coordinates (origin bottom-left), padded by
        // the chip's 8pt fringe and read at integral pixel bounds.
        let slices = particles.map { p -> (slice: CGRect, bounds: CGRect, size: CGSize) in
            let slice = CGRect(x: p.sourceLeft * scale, y: (160 - p.sourceTop - p.height) * scale,
                width: p.width * scale, height: p.height * scale)
            return (slice, slice.insetBy(dx: -pad * scale, dy: -pad * scale).integral,
                    CGSize(width: p.width + 2 * pad, height: p.height + 2 * pad))
        }
        if reference {
            let image = CIImage(cgImage: source)
            return try slices.map { slice, bounds, size in
                let clear = CIImage(color: .clear).cropped(to: bounds)
                let filtered = filter(image.cropped(to: slice).composited(over: clear), scale: scale)
                return Chip(image: try render(filtered, rect: bounds), size: size)
            }
        }

        let width = source.width, height = source.height
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        let info = CGImageAlphaInfo.premultipliedLast.rawValue
        try pixels.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height,
                bitsPerComponent: 8, bytesPerRow: width * 4, space: space, bitmapInfo: info)
            else { throw NativeError.renderFailed }
            context.draw(source, in: CGRect(x: 0, y: 0, width: width, height: height))
        }
        // `CIGaussianBlur`'s radius is its standard deviation.
        let sigma = Float(2 * scale)
        let reach = Int((3 * sigma).rounded(.up))
        // Two one-dimensional integer passes; vImageSepConvolve needs macOS 14.
        let weights = (-reach...reach).map { exp(-Float($0 * $0) / (2 * sigma * sigma)) }
        let total = weights.reduce(0, +)
        let divisor: Int32 = 1 << 14
        var kernel = weights.map { Int16((Float(divisor) * $0 / total).rounded()) }
        kernel[reach] += Int16(divisor - kernel.reduce(Int32(0)) { $0 + Int32($1) })
        let clear: [UInt8] = [0, 0, 0, 0]

        return try slices.map { slice, bounds, size in
            let chipWidth = Int(bounds.width), chipHeight = Int(bounds.height)
            var chip = [UInt8](repeating: 0, count: chipWidth * chipHeight * 4)
            // Core Image's crop to a fractional rect keeps the part of each
            // edge pixel inside it, so edge pixels are scaled by that part.
            // Chip rows run top-down, as in a CGImage.
            let covered = { (low: CGFloat, high: CGFloat) in
                max(0, min(high, 1) - max(low, 0)) }
            for row in 0..<chipHeight {
                let y = bounds.maxY - CGFloat(row) - 1
                let rowCoverage = covered(slice.minY - y, slice.maxY - y)
                let sourceRow = height - 1 - Int(y)
                guard rowCoverage > 0, sourceRow >= 0, sourceRow < height else { continue }
                for column in 0..<chipWidth {
                    let x = bounds.minX + CGFloat(column)
                    let coverage = rowCoverage * covered(slice.minX - x, slice.maxX - x)
                    let sourceColumn = Int(x)
                    guard coverage > 0, sourceColumn >= 0, sourceColumn < width else { continue }
                    let from = (sourceRow * width + sourceColumn) * 4, to = (row * chipWidth + column) * 4
                    for channel in 0..<4 {
                        chip[to + channel] = UInt8((CGFloat(pixels[from + channel]) * coverage).rounded())
                    }
                }
            }
            var across = [UInt8](repeating: 0, count: chip.count)
            var blurred = [UInt8](repeating: 0, count: chip.count)
            let error = chip.withUnsafeMutableBytes { input in
                across.withUnsafeMutableBytes { middle in
                    blurred.withUnsafeMutableBytes { output in
                        let buffer = { (bytes: UnsafeMutableRawBufferPointer) in
                            vImage_Buffer(data: bytes.baseAddress, height: vImagePixelCount(chipHeight),
                                          width: vImagePixelCount(chipWidth), rowBytes: chipWidth * 4)
                        }
                        var source = buffer(input), between = buffer(middle), destination = buffer(output)
                        let flags = vImage_Flags(kvImageBackgroundColorFill)
                        let first = vImageConvolve_ARGB8888(&source, &between, nil, 0, 0, kernel,
                            1, UInt32(kernel.count), divisor, clear, flags)
                        guard first == kvImageNoError else { return first }
                        return vImageConvolve_ARGB8888(&between, &destination, nil, 0, 0, kernel,
                            UInt32(kernel.count), 1, divisor, clear, flags)
                    }
                }
            }
            guard error == kvImageNoError else { throw NativeError.renderFailed }
            // `CIColorMatrix` halves red, green and blue and keeps alpha.
            for index in stride(from: 0, to: blurred.count, by: 4) {
                blurred[index] = UInt8((Int(blurred[index]) + 1) / 2)
                blurred[index + 1] = UInt8((Int(blurred[index + 1]) + 1) / 2)
                blurred[index + 2] = UInt8((Int(blurred[index + 2]) + 1) / 2)
            }
            guard let provider = CGDataProvider(data: Data(blurred) as CFData),
                  let image = CGImage(width: chipWidth, height: chipHeight, bitsPerComponent: 8,
                      bitsPerPixel: 32, bytesPerRow: chipWidth * 4, space: space,
                      bitmapInfo: CGBitmapInfo(rawValue: info), provider: provider, decode: nil,
                      shouldInterpolate: false, intent: .defaultIntent)
            else { throw NativeError.renderFailed }
            return Chip(image: image, size: size)
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
    private let reference: Bool
    private var source: CGImage?
    private var cache: [DustTextures.Chip]?
    private var cachedScale: CGFloat = 0
    private var running = false
    private var generation = 0

    init(frame: NSRect, reference: Bool) throws {
        self.reference = reference
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
            survivor.transform = CATransform3DMakeTranslation(0, 176, 0)
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
                    scale: cachedScale, reference: reference)
            }
        } catch {
            Metrics.emit("effect-error", milliseconds: 0, detail: String(describing: error))
            return
        }
        Metrics.emit("texture-preparation", milliseconds: (CACurrentMediaTime() - preparation) * 1000,
            detail: "\(reference ? "Core Image" : "vImage"), cacheHit=\(cacheHit), \(textures.deviceName)")
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
        survivor.transform = CATransform3DMakeTranslation(0, 176, 0)
        animate(survivor, key: "transform.translation.y", values: times.map { time in
            NSNumber(value: 176 * cubicBezierProgress(0.4, 0, 0.2, 1, min(1, max(0, (time - 1.8) / 0.58))))
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
