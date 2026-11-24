import AppKit
import QuartzCore
import CCapturesSettings

/// The wake a mini preview leaves while its file is dragged out of the stack
/// (`captures_app::preview_wake`): the whole card leaves its slot, the slot
/// keeps a glowing imprint, and a ribbon of light runs from it to the drag
/// image. An accepted drop snaps the ribbon away with a bloom; anything else
/// draws it back into the slot as the card re-forms.
///
/// The shared model owns the motion and geometry. This owns the clock, the
/// pointer and a click-through overlay window over the stack's display, so
/// the ribbon reaches wherever the pointer goes. The timer runs in the common
/// run loop modes, which include the drag session's event tracking.
final class MiniPreviewDragWake {
    let artifactID: String
    let window: NSWindow
    private let view: MiniPreviewWakeView
    private var handle: OpaquePointer?
    private var timer: Timer?
    private let screenFrame: NSRect
    /// From the pointer to the drag image's centre, in screen points.
    private let tipOffset: NSPoint
    private let radius: CGFloat
    /// The dragged card now: panels rebuild under a drag, and a replaced or
    /// removed card answers nil.
    private let lookup: () -> MiniPreviewCardView?
    /// The pointer in screen points.
    private let pointer: () -> NSPoint
    private weak var styled: MiniPreviewCardView?
    var finished: () -> Void = {}

    init?(artifactID: String, card: MiniPreviewCardView, tipOffset: NSPoint, tokens: Tokens,
          includeInCaptures: Bool, pointer: @escaping () -> NSPoint = { NSEvent.mouseLocation },
          lookup: @escaping () -> MiniPreviewCardView?) {
        guard let cardWindow = card.window, let screen = cardWindow.screen ?? NSScreen.main else { return nil }
        self.artifactID = artifactID; self.tipOffset = tipOffset; self.lookup = lookup
        self.pointer = pointer
        screenFrame = screen.frame
        radius = tokens.number("thumbnail-card-radius")
        window = NSWindow(contentRect: screen.frame, styleMask: .borderless, backing: .buffered, defer: false)
        view = MiniPreviewWakeView(frame: NSRect(origin: .zero, size: screen.frame.size), tokens: tokens)
        window.isReleasedWhenClosed = false; window.isOpaque = false; window.backgroundColor = .clear
        window.hasShadow = false; window.ignoresMouseEvents = true; window.animationBehavior = .none
        // Above the stack, which rebuilds its panel under a drag.
        window.level = NSWindow.Level(rawValue: cardWindow.level.rawValue + 1)
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        window.sharingType = includeInCaptures ? .readOnly : .none
        window.contentView = view
        guard let slot = slot(of: card) else { return nil }
        let tip = self.tip()
        handle = captures_preview_wake_new_v1(slot, tip.x, tip.y, Self.now, NativeMotion.reduceMotion)
        window.orderFrontRegardless()
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.tick() }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
        tick()
    }

    deinit {
        timer?.invalidate()
        if let handle { captures_preview_wake_free_v1(handle) }
    }

    private static var now: Double { CACurrentMediaTime() * 1000 }

    /// The drag ended: `dropped` when another app took the file.
    func end(dropped: Bool) {
        guard let handle else { return }
        captures_preview_wake_end_v1(handle, dropped, Self.now)
        // Same run loop turn as the drag's own cleanup: no frame of the card
        // at full opacity before the wake takes it back.
        tick()
    }

    /// Stop at once (a new drag, the stack closing), restoring the card.
    func finish() {
        timer?.invalidate(); timer = nil
        if let card = styled {
            CATransaction.begin(); CATransaction.setDisableActions(true)
            card.alphaValue = card.fileDragStarted ? 0 : 1
            card.layer?.transform = CATransform3DIdentity
            CATransaction.commit()
        }
        styled = nil
        window.orderOut(nil); window.close()
        if let handle { captures_preview_wake_free_v1(handle) }
        handle = nil
        finished()
    }

    private func tick() {
        guard let handle else { return }
        let card = lookup()
        if let card, let slot = slot(of: card) { captures_preview_wake_set_slot_v1(handle, slot) }
        let tip = self.tip()
        captures_preview_wake_pointer_v1(handle, tip.x, tip.y)
        var frame = CapturesPreviewWakeFrame()
        guard captures_preview_wake_frame_v1(handle, Self.now, &frame) else { finish(); return }
        if let card, let layer = card.layer {
            CATransaction.begin(); CATransaction.setDisableActions(true)
            card.alphaValue = CGFloat(frame.card_opacity)
            layer.transform = Self.scale(CGFloat(frame.card_scale), about: layer)
            CATransaction.commit()
            styled = card
        }
        view.show(handle: handle, frame: frame)
    }

    /// Screen points (y up) to overlay points (y down).
    private func local(_ point: NSPoint) -> NSPoint {
        NSPoint(x: point.x - screenFrame.minX, y: screenFrame.maxY - point.y)
    }

    private func tip() -> NSPoint {
        let mouse = pointer()
        return local(NSPoint(x: mouse.x + tipOffset.x, y: mouse.y + tipOffset.y))
    }

    private func slot(of card: MiniPreviewCardView) -> CapturesPreviewWakeRect? {
        guard let window = card.window else { return nil }
        let rect = window.convertToScreen(card.convert(card.bounds, to: nil))
        let origin = local(NSPoint(x: rect.minX, y: rect.maxY))
        return CapturesPreviewWakeRect(x: origin.x, y: origin.y, width: rect.width, height: rect.height,
                                       radius: radius)
    }

    /// Scale about the layer's centre, whatever its anchor point.
    static func scale(_ s: CGFloat, about layer: CALayer) -> CATransform3D {
        guard s != 1 else { return CATransform3DIdentity }
        let dx = (0.5 - layer.anchorPoint.x) * layer.bounds.width
        let dy = (0.5 - layer.anchorPoint.y) * layer.bounds.height
        return CATransform3DTranslate(CATransform3DScale(CATransform3DMakeTranslation(dx, dy, 0), s, s, 1),
                                      -dx, -dy, 0)
    }
}

/// Draws one `CapturesPreviewWakeFrame`, redrawing only what it covers.
final class MiniPreviewWakeView: NSView {
    private struct Ribbon {
        let accent: Bool
        let path: CGPath
        let from: CGPoint
        let to: CGPoint
        let gradient: CGGradient
    }

    private let accent: NSColor
    private let light: NSColor
    private let shade: NSColor
    private let style = captures_preview_wake_style_v1()
    private var frameValue: CapturesPreviewWakeFrame?
    private var ribbons: [Ribbon] = []
    private var motes: [CapturesPreviewWakeMote] = []
    private var drawn = NSRect.zero
    private let space = CGColorSpace(name: CGColorSpace.sRGB)!

    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens) {
        accent = tokens.color("theme-accent"); light = tokens.color("glass-text")
        shade = tokens.color("glass-veil").withAlphaComponent(1)
        super.init(frame: frame)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private func cg(_ color: NSColor, _ alpha: Double) -> CGColor {
        color.withAlphaComponent(CGFloat(max(0, min(1, alpha)))).cgColor
    }

    func show(handle: OpaquePointer, frame: CapturesPreviewWakeFrame) {
        frameValue = frame
        ribbons = (0..<frame.ribbon_count).compactMap { index in
            var ribbon = CapturesPreviewWakeRibbon()
            guard captures_preview_wake_ribbon_v1(handle, index, &ribbon, nil, 0) else { return nil }
            var points = [Double](repeating: 0, count: ribbon.point_count * 2)
            guard captures_preview_wake_ribbon_v1(handle, index, &ribbon, &points, ribbon.point_count),
                  ribbon.point_count > 2 else { return nil }
            let path = CGMutablePath()
            path.addLines(between: stride(from: 0, to: points.count, by: 2).map {
                CGPoint(x: points[$0], y: points[$0 + 1])
            })
            path.closeSubpath()
            let ink = ribbon.accent ? accent : light
            let offsets = [ribbon.stop_offsets.0, ribbon.stop_offsets.1, ribbon.stop_offsets.2].map { CGFloat($0) }
            let opacities = [ribbon.stop_opacities.0, ribbon.stop_opacities.1, ribbon.stop_opacities.2]
            guard let gradient = CGGradient(colorsSpace: space, colors: opacities.map { cg(ink, $0) } as CFArray,
                                            locations: offsets) else { return nil }
            return Ribbon(accent: ribbon.accent, path: path, from: CGPoint(x: ribbon.from_x, y: ribbon.from_y),
                          to: CGPoint(x: ribbon.to_x, y: ribbon.to_y), gradient: gradient)
        }
        motes = (0..<frame.mote_count).compactMap { index in
            var mote = CapturesPreviewWakeMote()
            return captures_preview_wake_mote_v1(handle, index, &mote) ? mote : nil
        }
        let bounds = NSRect(x: frame.bounds_x, y: frame.bounds_y, width: frame.bounds_width,
                            height: frame.bounds_height).insetBy(dx: -2, dy: -2).integral
        setNeedsDisplay(drawn.isEmpty ? bounds : drawn.union(bounds))
        drawn = bounds
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let frame = frameValue, let context = NSGraphicsContext.current?.cgContext else { return }
        drawSlot(frame, in: context)
        for ribbon in ribbons where ribbon.from != ribbon.to {
            context.saveGState()
            context.addPath(ribbon.path); context.clip()
            context.drawLinearGradient(ribbon.gradient, start: ribbon.from, end: ribbon.to,
                                       options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
            context.restoreGState()
        }
        for mote in motes {
            let center = CGPoint(x: mote.x, y: mote.y)
            glow(at: center, radius: CGFloat(mote.radius * style.mote_halo_scale),
                 opacity: mote.opacity * style.mote_halo_opacity, in: context)
            context.setFillColor(cg(light, mote.opacity))
            context.fillEllipse(in: CGRect(x: mote.x - mote.radius, y: mote.y - mote.radius,
                                           width: mote.radius * 2, height: mote.radius * 2))
        }
        if frame.has_bloom {
            let bloom = frame.bloom
            glow(at: CGPoint(x: bloom.x, y: bloom.y), radius: CGFloat(bloom.radius), opacity: bloom.opacity,
                 in: context)
        }
    }

    /// An accent radial glow from `opacity` at the centre to clear.
    private func glow(at center: CGPoint, radius: CGFloat, opacity: Double, in context: CGContext) {
        guard radius > 0, opacity > 0.001,
              let gradient = CGGradient(colorsSpace: space, colors: [cg(accent, opacity), cg(accent, 0)] as CFArray,
                                        locations: [0, 1]) else { return }
        context.drawRadialGradient(gradient, startCenter: center, startRadius: 0, endCenter: center,
                                   endRadius: radius, options: [])
    }

    private func drawSlot(_ frame: CapturesPreviewWakeFrame, in context: CGContext) {
        let opacity = frame.slot_opacity
        guard opacity > 0.001 else { return }
        let rect = CGRect(x: frame.slot.x, y: frame.slot.y, width: frame.slot.width, height: frame.slot.height)
        guard rect.width > 2, rect.height > 2 else { return }
        func rounded(_ r: CGRect, _ radius: CGFloat) -> CGPath {
            let radius = max(0, min(radius, r.width / 2, r.height / 2))
            return CGPath(roundedRect: r, cornerWidth: radius, cornerHeight: radius, transform: nil)
        }
        let radius = CGFloat(frame.slot.radius)
        let path = rounded(rect, radius)
        // The accent glow, outside the edge only (like CSS box-shadow).
        let blur = CGFloat(style.slot_glow_blur)
        context.saveGState()
        let outside = CGMutablePath()
        outside.addRect(rect.insetBy(dx: -blur * 3, dy: -blur * 3)); outside.addPath(path)
        context.addPath(outside); context.clip(using: .evenOdd)
        context.setShadow(offset: .zero, blur: blur, color: cg(accent, style.slot_glow_opacity * opacity))
        context.addPath(path); context.setFillColor(.black); context.fillPath()
        context.restoreGState()
        context.addPath(path)
        context.setFillColor(cg(shade, style.slot_shade_opacity * opacity)); context.fillPath()
        let line = CGFloat(style.slot_line_width)
        context.addPath(rounded(rect.insetBy(dx: line / 2, dy: line / 2), radius - line / 2))
        context.setLineWidth(line)
        context.setStrokeColor(cg(light, style.slot_line_opacity * opacity)); context.strokePath()
    }
}
