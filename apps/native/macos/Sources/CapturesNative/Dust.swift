import AppKit
import CoreImage
import Metal
import QuartzCore

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
