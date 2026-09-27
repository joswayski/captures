import AppKit
import CCapturesSettings
import QuartzCore

/// Shipping screenshot-editor previews (`ScreenshotEditor.tsx`,
/// `styles/editor-image.css`) for AppKit: the Trim edges hover preview, the
/// Wand colour loupe, the Properties `DrawToolPreview` and the Apply crop
/// `cta-pulse`. Geometry, copy and motion come from captures-app through
/// snapshots (`can_trim`, `trim_preview`), `captures_editor_chrome_v1`,
/// `captures_editor_wand_loupe_v1` and the motion catalog, shared with wgpu.

private func previewNumber(_ value: Any?) -> CGFloat? {
    (value as? NSNumber).map { CGFloat($0.doubleValue) }
}

private func previewPoint(_ value: Any?) -> CGPoint? {
    guard let object = value as? [String: Any], let x = previewNumber(object["x"]),
          let y = previewNumber(object["y"]) else { return nil }
    return CGPoint(x: x, y: y)
}

private func previewRect(_ value: Any?) -> CGRect? {
    guard let object = value as? [String: Any], let x = previewNumber(object["x"]),
          let y = previewNumber(object["y"]), let width = previewNumber(object["width"]),
          let height = previewNumber(object["height"]) else { return nil }
    return CGRect(x: x, y: y, width: width, height: height)
}

private func previewColor(_ value: Any?, alpha: CGFloat = 1) -> NSColor? {
    guard let channels = value as? [NSNumber], channels.count >= 3 else { return nil }
    return NSColor(srgbRed: CGFloat(channels[0].doubleValue) / 255, green: CGFloat(channels[1].doubleValue) / 255,
                   blue: CGFloat(channels[2].doubleValue) / 255, alpha: alpha)
}

/// Paint metrics the editor chrome `copy` carries for these previews.
enum NativeEditorPreviewPaint {
    private static func group(_ key: String) -> [String: Any] {
        EditorChrome.copy[key] as? [String: Any] ?? [:]
    }

    static func trim(_ key: String, _ fallback: CGFloat) -> CGFloat {
        previewNumber(group("trim")[key]) ?? fallback
    }

    static func wandLoupe(_ key: String, _ fallback: CGFloat) -> CGFloat {
        previewNumber(group("wand_loupe")[key]) ?? fallback
    }

    static func drawPreview(_ key: String, _ fallback: CGFloat) -> CGFloat {
        previewNumber(group("draw_preview")[key]) ?? fallback
    }

    /// `rgba(var(--trim-rgb), alpha)`.
    static func trimColor(_ alpha: CGFloat) -> NSColor {
        previewColor(group("trim")["rgb"], alpha: alpha)
            ?? NSColor(srgbRed: 1, green: 92.0 / 255, blue: 106.0 / 255, alpha: alpha)
    }

    /// `trim_bloom` gradient stops (offset from the edge, alpha).
    static var trimBloomStops: [(CGFloat, CGFloat)] {
        let stops = (group("trim")["bloom_stops"] as? [[NSNumber]] ?? []).compactMap { stop -> (CGFloat, CGFloat)? in
            stop.count == 2 ? (CGFloat(stop[0].doubleValue), CGFloat(stop[1].doubleValue)) : nil
        }
        return stops.isEmpty ? [(0, 0.5), (0.38, 0.2), (0.72, 0.06), (1, 0)] : stops
    }

    static func loupeChecker(dark: Bool) -> NSColor {
        previewColor(group("wand_loupe")[dark ? "checker_dark" : "checker_light"])
            ?? (dark ? NSColor(srgbRed: 0.769, green: 0.769, blue: 0.784, alpha: 1)
                : NSColor(srgbRed: 0.925, green: 0.925, blue: 0.933, alpha: 1))
    }
}

/// Snapshot `trim_preview`: what Trim edges would cut, in document coordinates.
struct NativeTrimPreview: Equatable {
    let keep: CGRect
    /// Cut edges in shipping's top/right/bottom/left order.
    let edges: [NativeCanvasExpand.Edge]
    /// Discarded strips, one per cut edge.
    let regions: [CGRect]

    init?(_ value: [String: Any]) {
        guard let keep = previewRect(value["keep"]), let rawEdges = value["edges"] as? [String],
              let rawRegions = value["regions"] as? [[Any]] else { return nil }
        let edges = rawEdges.compactMap { NativeCanvasExpand.Edge(rawValue: $0) }
        let regions = rawRegions.compactMap { pair -> CGRect? in pair.count == 2 ? previewRect(pair[1]) : nil }
        guard !edges.isEmpty, edges.count == rawEdges.count, regions.count == rawRegions.count else { return nil }
        self.keep = keep; self.edges = edges; self.regions = regions
    }
}

/// `captures_editor_wand_loupe_v1`: magnified natural pixels around the
/// Wand's sample and its label.
struct NativeWandLoupe: Equatable {
    let pixel: [Int]
    /// Straight-alpha RGBA, 0–255.
    let color: [Int]
    /// `extent × extent` tiles row by row; nil outside the image.
    let tiles: [[Int]?]
    let extent: Int
    /// `#rrggbb`, or `empty` when transparent.
    let text: String
    let transparent: Bool
    let accessibleLabel: String

    init?(_ value: [String: Any]) {
        guard let pixel = value["pixel"] as? [NSNumber], let color = value["color"] as? [NSNumber],
              color.count == 4, let rawTiles = value["tiles"] as? [Any],
              let extent = value["extent"] as? NSNumber, extent.intValue > 0,
              rawTiles.count == extent.intValue * extent.intValue,
              let text = value["text"] as? String, let transparent = value["transparent"] as? Bool,
              let label = value["accessible_label"] as? String else { return nil }
        self.pixel = pixel.map { $0.intValue }
        self.color = color.map { $0.intValue }
        tiles = rawTiles.map { tile -> [Int]? in
            guard let channels = tile as? [NSNumber], channels.count == 4 else { return nil }
            return channels.map { $0.intValue }
        }
        self.extent = extent.intValue
        self.text = text; self.transparent = transparent; accessibleLabel = label
    }

    /// Top-left of the loupe circle beside `cursor` (y-down points inside a
    /// viewport of `size`), flipped near the edges as shipping does.
    static func position(cursor: CGPoint, viewport size: CGSize) -> CGPoint? {
        let result = EditorChrome.request(["operation": "wand_loupe_position",
                                           "cursor": [Double(cursor.x), Double(cursor.y)],
                                           "viewport": [Double(size.width), Double(size.height)]])
        return previewPoint(result)
    }
}

/// Shipping `DrawToolPreview` geometry (`editor_chrome::draw_preview`) in the
/// SVG's 160 × 72 view box.
struct NativeDrawToolPreview: Equatable {
    enum Shape: Equatable {
        case roundedRect(CGRect, radius: CGFloat)
        case ellipse(CGRect)
        case path([CGPoint], closed: Bool)
    }

    struct Brush: Equatable {
        let center: CGPoint
        let radius: CGFloat
        /// Opaque out to this fraction of the radius, then clear at the rim.
        let hardStop: CGFloat
    }

    let label: String
    let strokeWidth: CGFloat
    let shapes: [Shape]
    let brush: Brush?

    init?(_ value: [String: Any]) {
        guard let label = value["label"] as? String, let strokeWidth = previewNumber(value["stroke_width"]),
              let rawShapes = value["shapes"] as? [[String: Any]] else { return nil }
        var shapes: [Shape] = []
        for raw in rawShapes {
            switch raw["kind"] as? String ?? "" {
            case "rounded_rect":
                guard let rect = previewRect(raw["rect"]), let radius = previewNumber(raw["radius"]) else { return nil }
                shapes.append(.roundedRect(rect, radius: radius))
            case "ellipse":
                guard let rect = previewRect(raw["rect"]) else { return nil }
                shapes.append(.ellipse(rect))
            case "path":
                guard let rawPoints = raw["points"] as? [Any], let closed = raw["closed"] as? Bool else { return nil }
                let points = rawPoints.compactMap { previewPoint($0) }
                guard points.count == rawPoints.count else { return nil }
                shapes.append(.path(points, closed: closed))
            default:
                return nil
            }
        }
        var brush: Brush?
        if let raw = value["brush"] as? [String: Any] {
            guard let center = previewPoint(raw["center"]), let radius = previewNumber(raw["radius"]),
                  let hardStop = previewNumber(raw["hard_stop"]) else { return nil }
            brush = Brush(center: center, radius: radius, hardStop: hardStop)
        }
        self.label = label; self.strokeWidth = strokeWidth; self.shapes = shapes; self.brush = brush
    }

    /// Stroke sample for a drawing tool key (`rectangle` … `star`, else pen).
    static func stroke(tool: String, strokeWidth: Double, strokeEnabled: Bool) -> NativeDrawToolPreview? {
        (EditorChrome.request(["operation": "draw_tool_preview", "tool": tool,
                               "stroke_width": strokeWidth, "stroke_enabled": strokeEnabled])
            as? [String: Any]).flatMap { NativeDrawToolPreview($0) }
    }

    /// Erase/Restore brush dab.
    static func brush(size: Double, softness: Double) -> NativeDrawToolPreview? {
        (EditorChrome.request(["operation": "brush_preview", "size": size, "softness": softness])
            as? [String: Any]).flatMap { NativeDrawToolPreview($0) }
    }
}

/// `DROP_SNAP_PARTICLES` from the motion catalog (`snap_particles`): sparks
/// streaming outward from a glowing canvas edge.
struct NativeSnapParticles {
    struct Seed: Equatable {
        let along: CGFloat
        let travel: CGFloat
        /// Seconds.
        let delay: Double
        let duration: Double
        let size: CGFloat
    }

    let seeds: [Seed]
    let travel: CGFloat
    let easing: CAMediaTimingFunction
    let opacity: [(Double, Double)]
    let scale: (CGFloat, CGFloat)

    static let shipping = NativeSnapParticles(
        (try? SettingsBridge().request(["operation": "motion"]))?["motion"] as? [String: Any])

    init(_ motion: [String: Any]?) {
        let value = motion?["snap_particles"] as? [String: Any] ?? [:]
        seeds = (value["seeds"] as? [[String: Any]] ?? []).compactMap { seed in
            guard let along = previewNumber(seed["along"]), let travel = previewNumber(seed["travel"]),
                  let delay = previewNumber(seed["delay_ms"]), let duration = previewNumber(seed["duration_ms"]),
                  let size = previewNumber(seed["size"]) else { return nil }
            return Seed(along: along, travel: travel, delay: Double(delay) / 1000,
                        duration: Double(duration) / 1000, size: size)
        }
        travel = previewNumber(value["travel"]) ?? 72
        let points = (value["easing"] as? [NSNumber] ?? []).map { Float($0.doubleValue) }
        easing = points.count == 4
            ? CAMediaTimingFunction(controlPoints: points[0], points[1], points[2], points[3])
            : CAMediaTimingFunction(controlPoints: 0.2, 0.65, 0.25, 1)
        let frames = (value["opacity"] as? [[NSNumber]] ?? []).compactMap { frame -> (Double, Double)? in
            frame.count == 2 ? (frame[0].doubleValue, frame[1].doubleValue) : nil
        }
        opacity = frames.count >= 2 ? frames : [(0, 0), (0.12, 1), (0.7, 0.55), (1, 0)]
        let scales = (value["scale"] as? [NSNumber] ?? []).map { CGFloat($0.doubleValue) }
        scale = scales.count == 2 ? (scales[0], scales[1]) : (0.55, 0.2)
    }

    /// One particle `elapsed` seconds into its edge's glow, or nil while it is
    /// invisible (before its delay, and always under reduced motion).
    func pose(_ seed: Seed, at elapsed: Double, reduced: Bool)
        -> (outward: CGFloat, scale: CGFloat, opacity: CGFloat)? {
        guard !reduced, seed.duration > 0, elapsed.isFinite, elapsed >= seed.delay else { return nil }
        let progress = (elapsed - seed.delay).truncatingRemainder(dividingBy: seed.duration) / seed.duration
        let moved = CGFloat(NativeMotion.ease(easing, progress))
        var alpha = 0.0
        for index in 0..<(opacity.count - 1) {
            let (start, from) = opacity[index]
            let (end, to) = opacity[index + 1]
            if progress <= end {
                let local = end > start ? (progress - start) / (end - start) : 1
                alpha = from + (to - from) * NativeMotion.ease(easing, local)
                break
            }
        }
        guard alpha > 0.001 else { return nil }
        return (seed.travel * travel * moved, scale.0 + (scale.1 - scale.0) * moved, CGFloat(alpha))
    }
}

/// Shipping `.screenshot-canvas-trim-hint`, shown while Trim edges is hovered
/// or focused: red-tinted discarded margins, a dashed outline of the kept
/// area, and pulsing cut edges with blooms and particles. Under reduced
/// motion it is static and particle-free. Never takes pointer events.
final class EditorTrimPreviewView: NSView {
    override var isFlipped: Bool { true }
    var imageRect: () -> NSRect = { .zero }
    var canvasSize = NSSize.zero
    var reducedMotion: () -> Bool = { NativeMotion.reduceMotion }
    private let tokens: Tokens
    private var trimStarted: CFTimeInterval?
    private var trimTimer: Timer?
    /// Non-nil while the preview shows.
    var trimPreview: NativeTrimPreview? {
        didSet {
            isHidden = trimPreview == nil
            if trimPreview == nil {
                stopAnimating()
            } else if oldValue == nil {
                trimStarted = CACurrentMediaTime()
            }
            startAnimatingIfNeeded()
            needsDisplay = true
        }
    }
    /// True while the breathing hint schedules redraws.
    var isAnimating: Bool { trimTimer != nil }

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        isHidden = true
        setAccessibilityElement(false)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    deinit { trimTimer?.invalidate() }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window == nil { stopAnimating() } else { startAnimatingIfNeeded() }
    }

    private func startAnimatingIfNeeded() {
        guard trimPreview != nil, trimTimer == nil, window != nil, !reducedMotion() else { return }
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in
            guard let self, self.trimPreview != nil, self.window != nil, !self.reducedMotion() else {
                self?.stopAnimating(); return
            }
            self.needsDisplay = true
        }
        RunLoop.main.add(timer, forMode: .common)
        trimTimer = timer
    }

    private func stopAnimating() {
        trimTimer?.invalidate(); trimTimer = nil
    }

    func project(_ rect: CGRect) -> CGRect {
        let image = imageRect()
        guard canvasSize.width > 0, canvasSize.height > 0 else { return .zero }
        let scale = image.width / canvasSize.width
        return CGRect(x: image.minX + rect.minX * scale, y: image.minY + rect.minY * scale,
                      width: rect.width * scale, height: rect.height * scale)
    }

    /// The breathing loops' opacities `elapsed` seconds in. They have no fill
    /// mode, so under reduced motion each element rests on its own style (1).
    func breathing(at elapsed: Double, reduced: Bool) -> (region: CGFloat, keep: CGFloat, edge: CGFloat) {
        guard !reduced else { return (1, 1, 1) }
        func opacity(_ name: String) -> CGFloat {
            CGFloat(NativeMotion.poseRepeating(name, at: elapsed, tokens: tokens, reduced: false)?.opacity ?? 1)
        }
        return (opacity("trim_region_breathe"), opacity("trim_keep_breathe"), opacity("trim_edge_pulse"))
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let trimPreview, let context = NSGraphicsContext.current?.cgContext else { return }
        let reduced = reducedMotion()
        let elapsed = CACurrentMediaTime() - (trimStarted ?? CACurrentMediaTime())
        let pose = breathing(at: elapsed, reduced: reduced)
        let paint = NativeEditorPreviewPaint.self
        for region in trimPreview.regions {
            let rect = project(region)
            paint.trimColor(paint.trim("region_alpha", 0.14) * pose.region).setFill()
            NSBezierPath(rect: rect).fill()
            // `inset 0 0 24px rgba(trim, 0.12)`.
            for (width, alpha) in [(CGFloat(12), CGFloat(0.04)), (5, 0.05)] {
                let inset = NSBezierPath(rect: rect.insetBy(dx: min(width / 2, rect.width / 2),
                                                            dy: min(width / 2, rect.height / 2)))
                inset.lineWidth = width
                paint.trimColor(alpha * pose.region).setStroke(); inset.stroke()
            }
        }
        let keep = project(trimPreview.keep)
        let radius = paint.trim("keep_radius", 3)
        for (grow, alpha) in [(CGFloat(11), CGFloat(0.03)), (6, 0.06), (1, 0.1)] {
            let glow = NSBezierPath(roundedRect: keep.insetBy(dx: -grow / 2, dy: -grow / 2),
                                    xRadius: radius + grow / 2, yRadius: radius + grow / 2)
            glow.lineWidth = grow
            paint.trimColor(alpha * pose.keep).setStroke(); glow.stroke()
        }
        let outline = NSBezierPath(roundedRect: keep, xRadius: radius, yRadius: radius)
        outline.lineWidth = paint.trim("keep_width", 1.5)
        outline.setLineDash([4.5, 3], count: 2, phase: 0)
        paint.trimColor(paint.trim("keep_alpha", 0.72) * pose.keep).setStroke(); outline.stroke()

        let bar = paint.trim("edge_bar", 4)
        let depth = paint.trim("bloom", 96)
        for edge in trimPreview.edges {
            drawBloom(context, keep: keep, edge: edge, depth: depth)
            let strip: CGRect
            switch edge {
            case .top: strip = CGRect(x: keep.minX - 1, y: keep.minY - bar / 2, width: keep.width + 2, height: bar)
            case .bottom: strip = CGRect(x: keep.minX - 1, y: keep.maxY - bar / 2, width: keep.width + 2, height: bar)
            case .left: strip = CGRect(x: keep.minX - bar / 2, y: keep.minY - 1, width: bar, height: keep.height + 2)
            case .right: strip = CGRect(x: keep.maxX - bar / 2, y: keep.minY - 1, width: bar, height: keep.height + 2)
            }
            // `0 0 8px .95, 0 0 20px .55, 0 0 32px .32` around the pill.
            for (grow, alpha) in [(CGFloat(14), CGFloat(0.06)), (8, 0.12), (3, 0.3)] {
                paint.trimColor(alpha * pose.edge).setFill()
                NSBezierPath(roundedRect: strip.insetBy(dx: -grow, dy: -grow),
                             xRadius: bar / 2 + grow, yRadius: bar / 2 + grow).fill()
            }
            paint.trimColor(pose.edge).setFill()
            NSBezierPath(roundedRect: strip, xRadius: bar / 2, yRadius: bar / 2).fill()
            let particles = NativeSnapParticles.shipping
            for seed in particles.seeds {
                guard let particle = particles.pose(seed, at: elapsed, reduced: reduced) else { continue }
                let center: CGPoint
                switch edge {
                case .top: center = CGPoint(x: keep.minX + seed.along * keep.width, y: keep.minY - particle.outward)
                case .bottom: center = CGPoint(x: keep.minX + seed.along * keep.width, y: keep.maxY + particle.outward)
                case .left: center = CGPoint(x: keep.minX - particle.outward, y: keep.minY + seed.along * keep.height)
                case .right: center = CGPoint(x: keep.maxX + particle.outward, y: keep.minY + seed.along * keep.height)
                }
                let size = seed.size * particle.scale
                paint.trimColor(0.25 * particle.opacity).setFill()
                NSBezierPath(ovalIn: CGRect(x: center.x - size / 2 - 3, y: center.y - size / 2 - 3,
                                            width: size + 6, height: size + 6)).fill()
                paint.trimColor(particle.opacity).setFill()
                NSBezierPath(ovalIn: CGRect(x: center.x - size / 2, y: center.y - size / 2,
                                            width: size, height: size)).fill()
            }
        }
    }

    /// `.screenshot-canvas-trim-bloom`: a gradient outward from one side of
    /// the kept area, faded along the edge by shipping's 12 %/88 % mask.
    private func drawBloom(_ context: CGContext, keep: CGRect, edge: NativeCanvasExpand.Edge, depth: CGFloat) {
        let band: CGRect
        let start: CGPoint
        let end: CGPoint
        let alongStart: CGPoint
        let alongEnd: CGPoint
        switch edge {
        case .top:
            band = CGRect(x: keep.minX, y: keep.minY - depth, width: keep.width, height: depth)
            start = CGPoint(x: keep.minX, y: keep.minY); end = CGPoint(x: keep.minX, y: keep.minY - depth)
            alongStart = CGPoint(x: keep.minX, y: 0); alongEnd = CGPoint(x: keep.maxX, y: 0)
        case .bottom:
            band = CGRect(x: keep.minX, y: keep.maxY, width: keep.width, height: depth)
            start = CGPoint(x: keep.minX, y: keep.maxY); end = CGPoint(x: keep.minX, y: keep.maxY + depth)
            alongStart = CGPoint(x: keep.minX, y: 0); alongEnd = CGPoint(x: keep.maxX, y: 0)
        case .left:
            band = CGRect(x: keep.minX - depth, y: keep.minY, width: depth, height: keep.height)
            start = CGPoint(x: keep.minX, y: keep.minY); end = CGPoint(x: keep.minX - depth, y: keep.minY)
            alongStart = CGPoint(x: 0, y: keep.minY); alongEnd = CGPoint(x: 0, y: keep.maxY)
        case .right:
            band = CGRect(x: keep.maxX, y: keep.minY, width: depth, height: keep.height)
            start = CGPoint(x: keep.maxX, y: keep.minY); end = CGPoint(x: keep.maxX + depth, y: keep.minY)
            alongStart = CGPoint(x: 0, y: keep.minY); alongEnd = CGPoint(x: 0, y: keep.maxY)
        }
        guard band.width > 0, band.height > 0, let space = CGColorSpace(name: CGColorSpace.sRGB) else { return }
        let stops = NativeEditorPreviewPaint.trimBloomStops
        guard let across = CGGradient(colorsSpace: space,
                                      colors: stops.map { NativeEditorPreviewPaint.trimColor($0.1).cgColor } as CFArray,
                                      locations: stops.map { $0.0 }),
              let mask = CGGradient(colorsSpace: space,
                                    colors: [0.0, 1, 1, 0].map { NSColor(white: 0, alpha: $0).cgColor } as CFArray,
                                    locations: [0, 0.12, 0.88, 1]) else { return }
        context.saveGState()
        context.clip(to: band)
        context.beginTransparencyLayer(auxiliaryInfo: nil)
        context.drawLinearGradient(across, start: start, end: end, options: [])
        context.setBlendMode(.destinationIn)
        context.drawLinearGradient(mask, start: alongStart, end: alongEnd, options: [])
        context.endTransparencyLayer()
        context.restoreGState()
    }
}

/// Shipping `WandColorLoupe`: a magnified circle of natural pixels beside the
/// crosshair with a swatch and hex pill below. Never takes pointer events.
final class EditorWandLoupeView: NSView {
    override var isFlipped: Bool { true }
    private let tokens: Tokens
    private(set) var loupe: NativeWandLoupe?
    static var diameter: CGFloat { NativeEditorPreviewPaint.wandLoupe("size", 84) }
    /// Room around the circle for the drop shadow and the pill below.
    static let margin: CGFloat = 28

    init(tokens: Tokens) {
        self.tokens = tokens
        let side = Self.diameter + 2 * Self.margin
        super.init(frame: NSRect(x: 0, y: 0, width: max(side, 150), height: side + 12))
        isHidden = true
        setAccessibilityElement(true)
        setAccessibilityRole(.helpTag)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// The circle's frame in this view.
    var circleRect: CGRect {
        CGRect(x: (bounds.width - Self.diameter) / 2, y: Self.margin / 2,
               width: Self.diameter, height: Self.diameter)
    }

    /// Show `value` with the circle's top-left at `origin` in the superview.
    func show(_ value: NativeWandLoupe, circleOrigin origin: CGPoint) {
        loupe = value
        let circle = circleRect
        setFrameOrigin(NSPoint(x: origin.x - circle.minX, y: origin.y - circle.minY))
        setAccessibilityLabel(value.accessibleLabel)
        isHidden = false
        needsDisplay = true
    }

    func hide() {
        loupe = nil
        isHidden = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let loupe else { return }
        let circle = circleRect
        let oval = NSBezierPath(ovalIn: circle)
        // `drop-shadow(0 8px 18px rgba(0, 0, 0, 0.42))`.
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowColor = NSColor(white: 0, alpha: 0.42)
        shadow.shadowBlurRadius = 18
        shadow.shadowOffset = NSSize(width: 0, height: -8)
        shadow.set()
        NSColor.white.setFill(); oval.fill()
        NSGraphicsContext.restoreGraphicsState()

        NSGraphicsContext.saveGraphicsState()
        oval.addClip()
        let checker = NativeEditorPreviewPaint.wandLoupe("checker_cell", 6)
        let columns = Int(ceil(circle.width / checker))
        for row in 0..<columns {
            for column in 0..<columns {
                NativeEditorPreviewPaint.loupeChecker(dark: (row + column) % 2 == 0).setFill()
                NSRect(x: circle.minX + CGFloat(column) * checker, y: circle.minY + CGFloat(row) * checker,
                       width: checker, height: checker).fill()
            }
        }
        let extent = max(1, loupe.extent)
        let cell = circle.width / CGFloat(extent)
        for (index, tile) in loupe.tiles.enumerated() {
            guard let tile, tile.count == 4, tile[3] > 0 else { continue }
            NSColor(srgbRed: CGFloat(tile[0]) / 255, green: CGFloat(tile[1]) / 255,
                    blue: CGFloat(tile[2]) / 255, alpha: CGFloat(tile[3]) / 255).setFill()
            NSRect(x: circle.minX + CGFloat(index % extent) * cell, y: circle.minY + CGFloat(index / extent) * cell,
                   width: cell, height: cell).fill()
        }
        // Grid between source pixels.
        let hairline = 1 / max(1, window?.backingScaleFactor ?? 1)
        let grid = NSBezierPath()
        for index in 1..<extent {
            let offset = CGFloat(index) * cell
            grid.move(to: NSPoint(x: circle.minX + offset, y: circle.minY))
            grid.line(to: NSPoint(x: circle.minX + offset, y: circle.maxY))
            grid.move(to: NSPoint(x: circle.minX, y: circle.minY + offset))
            grid.line(to: NSPoint(x: circle.maxX, y: circle.minY + offset))
        }
        grid.lineWidth = hairline
        NSColor(white: 0, alpha: NativeEditorPreviewPaint.wandLoupe("grid_alpha", 0.18)).setStroke()
        grid.stroke()
        // The keyed centre sample.
        let center = CGFloat(extent / 2) * cell
        let inner = NSBezierPath(rect: NSRect(x: circle.minX + center + 0.5, y: circle.minY + center + 0.5,
                                              width: cell - 1, height: cell - 1))
        inner.lineWidth = 1.5
        NSColor(white: 1, alpha: 0.95).setStroke(); inner.stroke()
        let outer = NSBezierPath(rect: NSRect(x: circle.minX + center, y: circle.minY + center,
                                              width: cell, height: cell))
        outer.lineWidth = 1
        NSColor(white: 0, alpha: 0.55).setStroke(); outer.stroke()
        NSGraphicsContext.restoreGraphicsState()

        // `.screenshot-wand-loupe-rim`.
        let rim = NSBezierPath(ovalIn: circle.insetBy(dx: 1, dy: 1))
        rim.lineWidth = 2
        NSColor(white: 1, alpha: 0.92).setStroke(); rim.stroke()
        let outline = NSBezierPath(ovalIn: circle.insetBy(dx: -0.5, dy: -0.5))
        outline.lineWidth = 1
        NSColor(white: 0, alpha: 0.55).setStroke(); outline.stroke()
        let inset = NSBezierPath(ovalIn: circle.insetBy(dx: 2.5, dy: 2.5))
        inset.lineWidth = 1
        NSColor(white: 0, alpha: 0.28).setStroke(); inset.stroke()

        // `.screenshot-wand-loupe-meta`: swatch and hex in a glass pill.
        let font = NSFont.monospacedDigitSystemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: tokens.color("glass-text")]
        let text = loupe.text as NSString
        let textSize = text.size(withAttributes: attributes)
        let swatch: CGFloat = 14
        let gap = tokens.number("s-3")
        let pillSize = CGSize(width: 4 + swatch + gap + ceil(textSize.width) + 7,
                              height: max(swatch, ceil(textSize.height)) + 6)
        let pill = CGRect(x: circle.midX - pillSize.width / 2,
                          y: circle.maxY + NativeEditorPreviewPaint.wandLoupe("meta_gap", 6),
                          width: pillSize.width, height: pillSize.height)
        let plate = NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2)
        tokens.color("glass-strong").setFill(); plate.fill()
        plate.lineWidth = 1
        tokens.color("glass-border").setStroke(); plate.stroke()
        let chip = CGRect(x: pill.minX + 4, y: pill.midY - swatch / 2, width: swatch, height: swatch)
        let chipPath = NSBezierPath(ovalIn: chip)
        if loupe.transparent {
            NSGraphicsContext.saveGraphicsState()
            chipPath.addClip()
            for row in 0..<3 {
                for column in 0..<3 {
                    NativeEditorPreviewPaint.loupeChecker(dark: (row + column) % 2 == 0).setFill()
                    NSRect(x: chip.minX + CGFloat(column) * swatch / 3, y: chip.minY + CGFloat(row) * swatch / 3,
                           width: swatch / 3, height: swatch / 3).fill()
                }
            }
            NSGraphicsContext.restoreGraphicsState()
        } else if loupe.color.count == 4 {
            NSColor(srgbRed: CGFloat(loupe.color[0]) / 255, green: CGFloat(loupe.color[1]) / 255,
                    blue: CGFloat(loupe.color[2]) / 255, alpha: 1).setFill()
            chipPath.fill()
        }
        chipPath.lineWidth = 1
        NSColor(white: 1, alpha: 0.55).setStroke(); chipPath.stroke()
        text.draw(at: CGPoint(x: chip.maxX + gap, y: pill.midY - textSize.height / 2), withAttributes: attributes)
    }
}

/// Shipping `.screenshot-draw-preview`: an 88 pt checkerboard card sampling
/// the new stroke/shape or eraser brush with the current colour, fill and
/// opacity.
final class EditorDrawToolPreviewView: NSView {
    override var isFlipped: Bool { true }
    private let tokens: Tokens
    private(set) var sample: NativeDrawToolPreview?
    private(set) var sampleStroke = NSColor.black
    private(set) var sampleFill: NSColor?
    private(set) var sampleOpacity: CGFloat = 1
    static var height: CGFloat { NativeEditorPreviewPaint.drawPreview("height", 88) }

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: NSRect(x: 0, y: 0, width: 252, height: Self.height))
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func update(_ value: NativeDrawToolPreview?, stroke: NSColor, fill: NSColor?, opacity: CGFloat) {
        sample = value; sampleStroke = stroke; sampleFill = fill
        sampleOpacity = min(max(opacity, 0), 1)
        setAccessibilityLabel(value?.label)
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let radius = tokens.number("r-md")
        let card = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        NSGraphicsContext.saveGraphicsState()
        card.addClip()
        tokens.color("canvas-checker-b").setFill(); bounds.fill()
        tokens.color("canvas-checker-a").setFill()
        let square = NativeEditorPreviewPaint.drawPreview("checker", 8)
        let columns = Int(ceil(bounds.width / square)), rows = Int(ceil(bounds.height / square))
        for row in 0..<rows {
            for column in 0..<columns where (row + column) % 2 == 0 {
                NSRect(x: CGFloat(column) * square, y: CGFloat(row) * square, width: square, height: square).fill()
            }
        }
        if let sample, let context = NSGraphicsContext.current?.cgContext {
            let inner = bounds.insetBy(dx: 1, dy: 1)
            let scale = min(inner.width / 160, inner.height / 72)
            let left = inner.minX + (inner.width - 160 * scale) / 2
            let top = inner.minY + (inner.height - 72 * scale) / 2
            func placePoint(_ point: CGPoint) -> CGPoint {
                CGPoint(x: left + point.x * scale, y: top + point.y * scale)
            }
            func placeRect(_ rect: CGRect) -> CGRect {
                CGRect(origin: placePoint(rect.origin),
                       size: CGSize(width: rect.width * scale, height: rect.height * scale))
            }
            // Shipping sets the SVG group's opacity: composite once.
            context.setAlpha(sampleOpacity)
            context.beginTransparencyLayer(auxiliaryInfo: nil)
            let stroking = sample.strokeWidth > 0
            for shape in sample.shapes {
                let path: NSBezierPath
                var fillable = true
                switch shape {
                case .roundedRect(let rect, radius: let corner):
                    path = NSBezierPath(roundedRect: placeRect(rect), xRadius: corner * scale,
                                        yRadius: corner * scale)
                case .ellipse(let rect):
                    path = NSBezierPath(ovalIn: placeRect(rect))
                case .path(let points, closed: let closed):
                    path = NSBezierPath()
                    guard let first = points.first else { continue }
                    path.move(to: placePoint(first))
                    points.dropFirst().forEach { path.line(to: placePoint($0)) }
                    if closed { path.close() }
                    fillable = closed
                }
                if fillable, let sampleFill { sampleFill.setFill(); path.fill() }
                if stroking {
                    path.lineWidth = sample.strokeWidth * scale
                    path.lineCapStyle = .round; path.lineJoinStyle = .round
                    sampleStroke.setStroke(); path.stroke()
                }
            }
            if let brush = sample.brush, let space = CGColorSpace(name: CGColorSpace.sRGB) {
                // Radial gradient in `--solid`: opaque to the hard stop, clear at the rim.
                let solid = tokens.color("solid")
                let colors = [solid.withAlphaComponent(1).cgColor, solid.withAlphaComponent(1).cgColor,
                              solid.withAlphaComponent(0).cgColor] as CFArray
                if let gradient = CGGradient(colorsSpace: space, colors: colors,
                                             locations: [0, min(max(brush.hardStop, 0), 1), 1]) {
                    let center = placePoint(brush.center)
                    context.drawRadialGradient(gradient, startCenter: center, startRadius: 0, endCenter: center,
                                               endRadius: brush.radius * scale, options: [])
                }
            }
            context.endTransparencyLayer()
        }
        NSGraphicsContext.restoreGraphicsState()
        card.lineWidth = 1
        tokens.color("border-subtle").setStroke(); card.stroke()
    }
}

/// Shipping `.screenshot-property-actions button.primary.cta-pulse`: an accent
/// halo behind Apply crop that swells and fades every 2.4 s while a crop is
/// staged, resting invisible under reduced motion. Sits behind its button in
/// the same superview and never takes pointer events.
final class EditorCtaHalo: NSView {
    override var isFlipped: Bool { true }
    private let tokens: Tokens
    private var haloStarted: CFTimeInterval?
    private var haloTimer: Timer?
    var reducedMotion: () -> Bool = { NativeMotion.reduceMotion }
    /// Room around the button for the full-strength ring.
    static let outset: CGFloat = 8
    static var spread: CGFloat { ctaMetric("spread", 5) }
    static var alpha: CGFloat { ctaMetric("alpha", 0.22) }

    private static let catalog = (try? SettingsBridge().request(["operation": "motion"]))?["motion"] as? [String: Any]
    private static func ctaMetric(_ key: String, _ fallback: CGFloat) -> CGFloat {
        previewNumber((catalog?["editor_cta"] as? [String: Any])?[key]) ?? fallback
    }

    var pulsing = false {
        didSet {
            guard pulsing != oldValue else { return }
            if pulsing { haloStarted = CACurrentMediaTime() } else { stopPulse() }
            startPulseIfNeeded()
            needsDisplay = true
        }
    }
    /// True while the halo schedules redraws.
    var isAnimating: Bool { haloTimer != nil }

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(false)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    deinit { haloTimer?.invalidate() }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window == nil { haloTimer?.invalidate(); haloTimer = nil } else { startPulseIfNeeded() }
    }

    /// Place the halo around `button` (same superview).
    func surround(_ button: NSView) {
        frame = button.frame.insetBy(dx: -Self.outset, dy: -Self.outset)
    }

    /// The ring's spread and strength `elapsed` seconds into the loop.
    func halo(at elapsed: Double, reduced: Bool) -> (spread: CGFloat, opacity: CGFloat) {
        guard pulsing else { return (0, 0) }
        let pose = NativeMotion.poseRepeating("editor_cta_pulse", at: elapsed, tokens: tokens, reduced: reduced)
        let opacity = CGFloat(pose?.opacity ?? 0)
        return (Self.spread * opacity, opacity)
    }

    private func startPulseIfNeeded() {
        guard pulsing, haloTimer == nil, window != nil, !reducedMotion() else { return }
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in
            guard let self, self.pulsing, self.window != nil, !self.reducedMotion() else {
                self?.haloTimer?.invalidate(); self?.haloTimer = nil; return
            }
            self.needsDisplay = true
        }
        RunLoop.main.add(timer, forMode: .common)
        haloTimer = timer
    }

    private func stopPulse() {
        haloTimer?.invalidate(); haloTimer = nil; haloStarted = nil
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let elapsed = CACurrentMediaTime() - (haloStarted ?? CACurrentMediaTime())
        let ring = halo(at: elapsed, reduced: reducedMotion())
        guard ring.spread > 0.01 else { return }
        // `box-shadow: 0 0 0 <spread> rgba(accent, 0.22)` flush outside the button.
        let button = bounds.insetBy(dx: Self.outset, dy: Self.outset)
        let radius = tokens.number("r-md") + ring.spread / 2
        let path = NSBezierPath(roundedRect: button.insetBy(dx: -ring.spread / 2, dy: -ring.spread / 2),
                                xRadius: radius, yRadius: radius)
        path.lineWidth = ring.spread
        tokens.color("theme-accent").withAlphaComponent(Self.alpha * ring.opacity).setStroke()
        path.stroke()
    }
}
