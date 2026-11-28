import AppKit
import QuartzCore
import CCapturesSettings

/// Scale about a layer's centre, whatever its anchor point.
func centredScale(_ s: CGFloat, of layer: CALayer) -> CATransform3D {
    guard s != 1 else { return CATransform3DIdentity }
    let dx = (0.5 - layer.anchorPoint.x) * layer.bounds.width
    let dy = (0.5 - layer.anchorPoint.y) * layer.bounds.height
    return CATransform3DTranslate(CATransform3DScale(CATransform3DMakeTranslation(dx, dy, 0), s, s, 1), -dx, -dy, 0)
}

/// One card's file drag out of the stack (`captures_app::preview_drag_style`):
/// the shared model owns the curves; this owns the clock. The timer runs in
/// the common run loop modes, which include the drag session's tracking.
/// It runs only while the pose changes: a held pose re-applied every frame
/// re-committed the stack for the whole drag.
final class MiniPreviewDragOut {
    let artifactID: String
    private var handle: OpaquePointer?
    private var timer: Timer?
    private var dropped = false
    private var ended = false
    private var applied: CapturesPreviewDragPose?
    /// The pose to show each frame.
    var apply: (CapturesPreviewDragPose) -> Void = { _ in }
    /// The end finished playing: the card is back at rest, or (`true`) gone
    /// with its slot closed.
    var completed: (Bool) -> Void = { _ in }

    init(artifactID: String, style: UInt32, reducedMotion: Bool) {
        self.artifactID = artifactID
        handle = captures_preview_drag_out_new_v1(style, Self.now, reducedMotion)
    }

    deinit {
        timer?.invalidate()
        if let handle { captures_preview_drag_out_free_v1(handle) }
    }

    private static var now: Double { CACurrentMediaTime() * 1000 }

    func start() { tick() }

    /// Show the last pose again, on a rebuilt stack.
    func reapply() {
        if let applied { apply(applied) }
    }

    func end(dropped: Bool) {
        guard let handle, !ended else { return }
        ended = true; self.dropped = dropped
        captures_preview_drag_out_end_v1(handle, dropped, Self.now)
        tick()
    }

    /// The file is back over its own stack (`over`) or left it: the card
    /// eases back into view, or back into the style.
    func hover(_ over: Bool) {
        guard let handle, !ended else { return }
        captures_preview_drag_out_hover_v1(handle, over, Self.now)
        tick()
    }

    /// Stop at once (another drag began): settle as the drag ended.
    func finish() {
        guard handle != nil else { return }
        stop()
        completed(ended && dropped)
    }

    /// Stop without settling (the stack closed).
    func cancel() { stop() }

    private func stop() {
        timer?.invalidate(); timer = nil
        if let handle { captures_preview_drag_out_free_v1(handle) }
        handle = nil
    }

    private func tick() {
        guard let handle else { return }
        let now = Self.now
        var pose = CapturesPreviewDragPose()
        let done = captures_preview_drag_out_pose_v1(handle, now, &pose)
        if applied.map({ !Self.same($0, pose) }) ?? true {
            applied = pose
            apply(pose)
        }
        if done {
            stop()
            completed(dropped)
        } else if captures_preview_drag_out_moving_v1(handle, now) {
            guard timer == nil else { return }
            let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.tick() }
            RunLoop.main.add(timer, forMode: .common)
            self.timer = timer
        } else {
            timer?.invalidate(); timer = nil
        }
    }

    private static func same(_ a: CapturesPreviewDragPose, _ b: CapturesPreviewDragPose) -> Bool {
        withUnsafeBytes(of: a) { a in withUnsafeBytes(of: b) { b in a.elementsEqual(b) } }
    }
}

/// What a dragged card's slot draws over its media: the frosted and well
/// hairline, the well's recessed fill and the glyph, each faded by the pose.
final class MiniPreviewDragOverlay: CALayer {
    private let hairline = CAShapeLayer()
    private let well = CALayer()
    private let glyph = CALayer()
    private let metrics = captures_preview_drag_style_metrics_v1()

    init(tokens: Tokens, bounds: CGRect, radius: CGFloat) {
        super.init()
        frame = bounds
        let m = metrics
        well.frame = bounds; well.cornerRadius = radius; well.masksToBounds = true
        well.backgroundColor = tokens.color("glass-veil").withAlphaComponent(CGFloat(m.well_fill_opacity)).cgColor
        // An inset shadow: a ring outside the edge casting inward.
        let inner = CAShapeLayer()
        let ring = CGMutablePath()
        ring.addRect(bounds.insetBy(dx: -40, dy: -40))
        ring.addPath(BoxShadowLayers.roundedPath(bounds, radius: radius))
        inner.path = ring; inner.fillRule = .evenOdd; inner.fillColor = NSColor.black.cgColor
        inner.shadowColor = NSColor.black.cgColor; inner.shadowOffset = .zero
        inner.shadowRadius = CGFloat(m.well_inner_shadow_blur)
        inner.shadowOpacity = Float(m.well_inner_shadow_opacity)
        well.addSublayer(inner)
        addSublayer(well)
        let line = CGFloat(m.hairline_width)
        hairline.frame = bounds
        hairline.path = BoxShadowLayers.roundedPath(bounds.insetBy(dx: line / 2, dy: line / 2), radius: radius - line / 2)
        hairline.fillColor = nil; hairline.lineWidth = line
        hairline.strokeColor = tokens.color("glass-text").withAlphaComponent(CGFloat(m.hairline_opacity)).cgColor
        addSublayer(hairline)
        let side = CGFloat(m.glyph_diameter)
        glyph.bounds = CGRect(x: 0, y: 0, width: side, height: side)
        glyph.position = CGPoint(x: bounds.midX, y: bounds.midY)
        glyph.contents = Self.glyphImage(metrics: m)
        glyph.contentsGravity = .resizeAspect
        addSublayer(glyph)
        apply(MiniPreviewCardView.restPose)
    }

    override init(layer: Any) { super.init(layer: layer) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    /// The drag-out glyph in its faint disc (`preview_drag_style::GLYPH_*`).
    static func glyphImage(metrics m: CapturesPreviewDragStyleMetrics) -> NSImage {
        let side = CGFloat(m.glyph_diameter)
        return NSImage(size: NSSize(width: side, height: side), flipped: true) { rect in
            NSColor.white.withAlphaComponent(CGFloat(m.glyph_disc_opacity)).setFill()
            NSBezierPath(ovalIn: rect).fill()
            let icon = CGFloat(m.glyph_size)
            NSColor.white.withAlphaComponent(CGFloat(m.glyph_opacity)).setStroke()
            ShippingIcons.stroke("drag-out", in: NSRect(x: (side - icon) / 2, y: (side - icon) / 2,
                                                        width: icon, height: icon),
                                 width: CGFloat(m.glyph_stroke))
            return true
        }
    }

    func apply(_ pose: CapturesPreviewDragPose) {
        hairline.opacity = Float(pose.hairline)
        well.opacity = Float(pose.well)
        glyph.opacity = Float(pose.glyph)
        glyph.transform = CATransform3DMakeScale(CGFloat(pose.glyph_scale), CGFloat(pose.glyph_scale), 1)
    }
}

/// A looping preview of one drag style for Preferences: three small cards,
/// the middle one dragged out and back (`preview_drag_style::demo`). Drawn
/// with layers and updated only while the tile is on screen.
final class MiniPreviewDragDemoView: NSView {
    private let style: UInt32
    private let metrics = captures_preview_drag_style_metrics_v1()
    private var cards: [CALayer] = []
    private var media = CALayer()
    private let overlay: MiniPreviewDragOverlay
    private let ghost = CALayer()
    private var timer: Timer?
    /// Every tile loops in step.
    private static let epoch = CACurrentMediaTime()
    var animates = true { didSet { updateTimer() } }

    init(frame: NSRect, style: UInt32, tokens: Tokens) {
        self.style = style
        let card = CGRect(x: 0, y: 0, width: metrics.demo_card_width, height: metrics.demo_card_height)
        overlay = MiniPreviewDragOverlay(tokens: tokens, bounds: card, radius: 4)
        super.init(frame: frame)
        wantsLayer = true
        layerUsesCoreImageFilters = true
        layer?.masksToBounds = true
        let shots = Self.screenshots()
        for index in 0..<3 {
            let layer = CALayer()
            layer.bounds = card
            layer.cornerRadius = 4; layer.masksToBounds = true
            layer.backgroundColor = tokens.color("glass-strong-solid").cgColor
            layer.borderWidth = 0.5; layer.borderColor = tokens.color("glass-border").cgColor
            let content = CALayer()
            content.frame = card; content.contents = shots[index]; content.contentsGravity = .resize
            if index == 1 {
                if let blur = CIFilter(name: "CIGaussianBlur") {
                    blur.setDefaults(); blur.setValue(0, forKey: kCIInputRadiusKey); blur.name = "blur"
                    content.filters = [blur]
                }
                media = content
            }
            layer.addSublayer(content)
            if index == 1 { layer.addSublayer(overlay) }
            self.layer?.addSublayer(layer)
            cards.append(layer)
        }
        ghost.bounds = card; ghost.cornerRadius = 4; ghost.masksToBounds = false
        ghost.contents = shots[1]; ghost.contentsGravity = .resize
        ghost.shadowColor = NSColor.black.cgColor; ghost.shadowOpacity = 0.3
        ghost.shadowRadius = 4; ghost.shadowOffset = CGSize(width: 0, height: -2)
        layer?.addSublayer(ghost)
        render()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    /// The tile and its cards are decorative; the tile button takes clicks.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        updateTimer()
    }

    private func updateTimer() {
        let run = animates && window != nil && !NativeMotion.reduceMotion
        guard run != (timer != nil) else { return }
        timer?.invalidate(); timer = nil
        if run {
            let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in
                guard let self, self.window?.occlusionState.contains(.visible) == true else { return }
                self.render()
            }
            RunLoop.main.add(timer, forMode: .common)
            self.timer = timer
        }
        render()
    }

    /// Tile units are points, y down; layers here are y up.
    private func render() {
        let elapsed = (CACurrentMediaTime() - Self.epoch) * 1000
        let demo = captures_preview_drag_demo_v1(style, animates ? elapsed : 0,
                                                 NativeMotion.reduceMotion || !animates)
        let height = bounds.height, w = metrics.demo_card_width, h = metrics.demo_card_height
        let tops = [demo.card_tops.0, demo.card_tops.1, demo.card_tops.2]
        let p = demo.pose
        CATransaction.begin(); CATransaction.setDisableActions(true)
        for (index, card) in cards.enumerated() {
            card.position = CGPoint(x: demo.card_left + w / 2, y: height - tops[index] - h / 2)
        }
        let middle = cards[1]
        middle.opacity = Float(p.opacity)
        middle.transform = CATransform3DMakeScale(CGFloat(p.scale), CGFloat(p.scale), 1)
        middle.backgroundColor = middle.backgroundColor?.copy(alpha: CGFloat(p.fill_opacity))
        middle.borderWidth = 0.5 * CGFloat(p.chrome_opacity)
        media.opacity = Float(p.media_opacity)
        media.transform = CATransform3DMakeScale(CGFloat(p.media_scale), CGFloat(p.media_scale), 1)
        media.setValue(p.media_blur, forKeyPath: "filters.blur.inputRadius")
        overlay.apply(p)
        // The glyph keeps a legible size on the small card.
        overlay.sublayers?.last?.transform = CATransform3DMakeScale(0.4 * CGFloat(p.glyph_scale),
                                                                    0.4 * CGFloat(p.glyph_scale), 1)
        ghost.position = CGPoint(x: demo.ghost_x + w / 2, y: height - demo.ghost_y - h / 2)
        ghost.opacity = Float(demo.ghost_opacity)
        CATransaction.commit()
    }

    /// Three tiny stand-in screenshots: a window with a title bar, a
    /// sidebar, two lines of text and a coloured block.
    private static func screenshots() -> [NSImage] {
        let schemes: [(NSColor, NSColor, NSColor, NSColor)] = [
            (NSColor(srgbRed: 0.15, green: 0.16, blue: 0.21, alpha: 1), NSColor(srgbRed: 0.11, green: 0.12, blue: 0.15, alpha: 1),
             NSColor(srgbRed: 0.24, green: 0.51, blue: 0.96, alpha: 1), NSColor(white: 1, alpha: 0.22)),
            (NSColor(white: 1, alpha: 1), NSColor(srgbRed: 0.91, green: 0.91, blue: 0.93, alpha: 1),
             NSColor(srgbRed: 0.96, green: 0.55, blue: 0.2, alpha: 1), NSColor(white: 0, alpha: 0.14)),
            (NSColor(srgbRed: 0.12, green: 0.18, blue: 0.15, alpha: 1), NSColor(srgbRed: 0.09, green: 0.13, blue: 0.11, alpha: 1),
             NSColor(srgbRed: 0.06, green: 0.72, blue: 0.6, alpha: 1), NSColor(white: 1, alpha: 0.2)),
        ]
        return schemes.map { background, chrome, accent, ink in
            NSImage(size: NSSize(width: 104, height: 52), flipped: true) { rect in
                background.setFill(); rect.fill()
                chrome.setFill()
                NSRect(x: 0, y: 0, width: rect.width, height: 7).fill()
                NSRect(x: 0, y: 7, width: 22, height: rect.height - 7).fill()
                ink.setFill()
                NSBezierPath(roundedRect: NSRect(x: 28, y: 12, width: 46, height: 4), xRadius: 2, yRadius: 2).fill()
                NSBezierPath(roundedRect: NSRect(x: 28, y: 20, width: 62, height: 4), xRadius: 2, yRadius: 2).fill()
                accent.setFill()
                NSBezierPath(roundedRect: NSRect(x: 28, y: 29, width: 70, height: 17), xRadius: 3, yRadius: 3).fill()
                return true
            }
        }
    }
}

/// One option of Preferences' "When you drag a preview out" picker: the
/// style's looping preview with its name below.
final class PreferenceDragStyleTile: PreferenceHoverButton {
    private let tokens: Tokens
    private let name: String
    let demo: MiniPreviewDragDemoView
    var active = false { didSet { setAccessibilityValue(active); needsDisplay = true } }
    /// Arrow keys move the selection to the previous (-1) or next (1) style.
    var step: (Int) -> Void = { _ in }
    static let labelHeight: CGFloat = 22

    init(frame: NSRect, tokens: Tokens, style: UInt32, label: String, onPress: @escaping () -> Void) {
        self.tokens = tokens; name = label
        let inset: CGFloat = 4
        demo = MiniPreviewDragDemoView(frame: NSRect(x: inset, y: inset, width: frame.width - inset * 2,
            height: frame.height - inset - Self.labelHeight), style: style, tokens: tokens)
        super.init(frame: frame)
        title = ""; actionBlock = onPress
        configure(label: label, role: .radioButton)
        demo.wantsLayer = true
        demo.layer?.cornerRadius = tokens.number("r-md") - 2
        // Previews always float on the fixed glass palette, so the demo's
        // desktop is the same dark glass in either appearance.
        demo.layer?.backgroundColor = tokens.color("glass-raised-solid").cgColor
        addSubview(demo)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override var isEnabled: Bool { didSet { demo.animates = isEnabled } }

    override var acceptsFirstResponder: Bool { isEnabled }

    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 123, 126: step(-1)
        case 124, 125: step(1)
        default: super.keyDown(with: event)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-md")
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: radius, yRadius: radius)
        tokens.color(isEnabled && hovered && !active ? "control-hover" : "surface-raised").setFill(); shape.fill()
        let edge = active ? tokens.color("theme-accent") : tokens.color(isEnabled && hovered ? "border-strong" : "border-subtle")
        edge.setStroke(); shape.lineWidth = active ? 2 : 1; shape.stroke()
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.capturesUI(ofSize: tokens.number("text-sm"), weight: .medium),
            .foregroundColor: tokens.color(active || hovered ? "text" : "text-muted"),
        ]
        let size = (name as NSString).size(withAttributes: attributes)
        (name as NSString).draw(at: NSPoint(x: (bounds.width - size.width) / 2,
            y: bounds.height - Self.labelHeight + (Self.labelHeight - size.height) / 2 - 1),
            withAttributes: attributes)
        if isFocused { drawFocusRing(tokens, in: bounds.insetBy(dx: -2, dy: -2), radius: radius + 2) }
    }
}
