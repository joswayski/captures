import AppKit

/// Shipping `CompressionPreview` copy, badges and split bounds from
/// `captures_app::compression_compare` (through the recording editor UI ABI),
/// shared by both AppKit editors.
enum CompressionCompareCopy {
    struct Badges: Equatable {
        let before: String
        let after: String
        let savings: String?
    }

    struct Copy {
        let groupLabel: String
        let handleLabel: String
        let rangeLabel: String
        let handleGlyph: String
        let dismiss: String
        let dismissLabel: String
        let processing: String
        let showCaption: String
        let show: String
        let afterHint: String
        let minSplit: CGFloat
        let maxSplit: CGFloat
        /// The recording editor's refresh delay before a sample encodes.
        let refreshDelay: TimeInterval
    }

    /// The screenshot editor waits this long after the last change.
    static let screenshotRefreshDelay: TimeInterval = 0.28

    static let copy: Copy = {
        let value = RecordingEditorCopy.request(["operation": "compression_compare"])
        let strings = value?["copy"] as? [String: Any] ?? [:]
        func text(_ key: String, _ fallback: String) -> String { strings[key] as? String ?? fallback }
        return Copy(
            groupLabel: text("group_label", "Compression comparison"),
            handleLabel: text("handle_label", "Drag to compare before and after"),
            rangeLabel: text("range_label", "Before and after comparison"),
            handleGlyph: text("handle_glyph", "‹ ›"),
            dismiss: text("dismiss", "Hide"),
            dismissLabel: text("dismiss_label", "Hide compression comparison"),
            processing: text("processing", "Processing"),
            showCaption: text("show_caption", "Comparison"),
            show: text("show", "Show before / after"),
            afterHint: text("after_hint", "Edits apply to the original. This side updates after you finish."),
            minSplit: CGFloat((value?["min_split"] as? NSNumber)?.doubleValue ?? 0.06),
            maxSplit: CGFloat((value?["max_split"] as? NSNumber)?.doubleValue ?? 0.94),
            refreshDelay: ((value?["refresh_delay_ms"] as? NSNumber)?.doubleValue ?? 350) / 1000)
    }()

    /// "Before · 1.2 MB", "After · 480 KB" and " · 61% smaller", or
    /// "After · Processing…" while a new encode runs.
    static func badges(before: UInt64?, after: UInt64?, processing: Bool) -> Badges {
        var request: [String: Any] = ["operation": "compression_compare", "processing": processing]
        request["before_bytes"] = before.map { NSNumber(value: $0) } ?? NSNull()
        request["after_bytes"] = after.map { NSNumber(value: $0) } ?? NSNull()
        let badges = RecordingEditorCopy.request(request)?["badges"] as? [String: Any]
        return Badges(before: badges?["before"] as? String ?? "Before",
                      after: badges?["after"] as? String ?? (processing ? "After · Processing…" : "After"),
                      savings: badges?["savings"] as? String)
    }

    static func clampSplit(_ split: CGFloat) -> CGFloat {
        guard !split.isNaN else { return 0.5 }
        return min(copy.maxSplit, max(copy.minSplit, split))
    }
}

/// Shipping `.compression-preview-frame.is-cover` over an editor's media: the
/// encoded After side clipped right of a draggable divider, the size badges,
/// Hide, the Processing veil and in-frame errors. The editor draws its own
/// Before underneath; only the handle, the bottom strip and Hide take the
/// pointer, so canvas gestures pass through everywhere else.
final class CompressionCompareView: NSView {
    private let tokens: Tokens
    /// The media box in this view's coordinates; the split is measured on it.
    var mediaRect: NSRect = .zero { didSet { needsDisplay = true } }
    var afterImage: CGImage? { didSet { needsDisplay = true } }
    var split: CGFloat = 0.5 { didSet { needsDisplay = true; updateAccessibility() } }
    var badges = CompressionCompareCopy.Badges(before: "Before", after: "After", savings: nil) {
        didSet { needsDisplay = true }
    }
    var processing = false { didSet { needsDisplay = true } }
    var failureMessage: String? { didSet { needsDisplay = true } }
    /// The bottom strip also moves the split (off while a drawing tool is selected).
    var stripEnabled = true
    /// Cursor-following hint on the After side while a drawing tool is selected.
    var afterHint: String? { didSet { needsDisplay = true } }
    var onDismiss: (() -> Void)?
    var onSplitChanged: ((CGFloat) -> Void)?

    private var dragging = false
    private var pressedDismiss = false
    private var hoverPoint: NSPoint?
    private var hoverTracking: NSTrackingArea?

    static let handleSide: CGFloat = 36
    static let stripHeight: CGFloat = 28

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { afterImage != nil && !processing }

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.slider)
        setAccessibilityLabel(CompressionCompareCopy.copy.rangeLabel)
        updateAccessibility()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    /// The media inside the clip view (the recording preview scrolls at 100%).
    private var visibleMedia: NSRect { mediaRect.intersection(visibleRect) }
    private var dividerX: CGFloat { mediaRect.minX + mediaRect.width * CompressionCompareCopy.clampSplit(split) }

    var handleRect: NSRect {
        let visible = visibleMedia
        let side = Self.handleSide
        return NSRect(x: dividerX - side / 2, y: visible.midY - side / 2, width: side, height: side)
    }

    var stripRect: NSRect {
        let visible = visibleMedia
        return NSRect(x: visible.minX, y: visible.maxY - Self.stripHeight,
                      width: visible.width, height: min(Self.stripHeight, visible.height))
    }

    private var pad: NSSize { NSSize(width: tokens.number("s-4"), height: tokens.number("s-2")) }
    private var inset: CGFloat { tokens.number("s-5") }

    private func pillAttributes(_ color: String = "glass-text") -> [NSAttributedString.Key: Any] {
        [.font: NSFont.monospacedDigitSystemFont(ofSize: tokens.number("text-xs"), weight: .medium),
         .foregroundColor: tokens.color(color)]
    }

    var dismissRect: NSRect {
        let text = CompressionCompareCopy.copy.dismiss as NSString
        let size = text.size(withAttributes: pillAttributes())
        let visible = visibleMedia
        let width = ceil(size.width) + pad.width * 2, height = ceil(size.height) + pad.height * 2
        return NSRect(x: visible.maxX - inset - width, y: visible.minY + inset, width: width, height: height)
    }

    private var splitActive: Bool { afterImage != nil && !processing }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard !isHidden, let superview else { return nil }
        let local = convert(point, from: superview)
        guard visibleMedia.contains(local) else { return nil }
        if dismissRect.contains(local) { return self }
        if splitActive && (handleRect.contains(local) || (stripEnabled && stripRect.contains(local))) {
            return self
        }
        return nil
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if dismissRect.contains(point) { pressedDismiss = true; return }
        guard splitActive else { return }
        window?.makeFirstResponder(self)
        dragging = true
        setSplit(at: point.x)
    }

    override func mouseDragged(with event: NSEvent) {
        guard dragging else { return }
        setSplit(at: convert(event.locationInWindow, from: nil).x)
    }

    override func mouseUp(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if pressedDismiss {
            pressedDismiss = false
            if dismissRect.contains(point) { onDismiss?() }
            return
        }
        if dragging { setSplit(at: point.x) }
        dragging = false
    }

    override func keyDown(with event: NSEvent) {
        // The range input's keyboard steps, one percent at a time.
        switch event.keyCode {
        case 123: stepSplit(-0.01)
        case 124: stepSplit(0.01)
        default: super.keyDown(with: event)
        }
    }

    func stepSplit(_ delta: CGFloat) {
        guard splitActive else { return }
        split = CompressionCompareCopy.clampSplit(split + delta)
        onSplitChanged?(split)
    }

    private func setSplit(at x: CGFloat) {
        split = CompressionCompareCopy.clampSplit((x - mediaRect.minX) / max(1, mediaRect.width))
        onSplitChanged?(split)
    }

    private func updateAccessibility() {
        setAccessibilityValue(NSNumber(value: Double(split * 100).rounded()))
    }

    override func accessibilityPerformIncrement() -> Bool { stepSplit(0.01); return true }
    override func accessibilityPerformDecrement() -> Bool { stepSplit(-0.01); return true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(tracking); hoverTracking = tracking
    }
    override func mouseMoved(with event: NSEvent) {
        hoverPoint = convert(event.locationInWindow, from: nil)
        if afterHint != nil { needsDisplay = true }
    }
    override func mouseExited(with event: NSEvent) {
        hoverPoint = nil
        if afterHint != nil { needsDisplay = true }
    }

    private func pill(_ rect: NSRect, hovered: Bool = false) {
        let path = NSBezierPath(roundedRect: rect, xRadius: rect.height / 2, yRadius: rect.height / 2)
        tokens.color(hovered ? "glass-raised" : "glass-strong").setFill(); path.fill()
        tokens.color(hovered ? "glass-border-strong" : "glass-border").setStroke()
        path.lineWidth = 1; path.stroke()
    }

    override func draw(_ dirtyRect: NSRect) {
        let visible = visibleMedia
        guard visible.width >= 1, visible.height >= 1 else { return }
        NSGraphicsContext.saveGraphicsState()
        NSBezierPath(rect: visible).addClip()
        let strings = CompressionCompareCopy.copy
        if let afterImage {
            // `object-fit: fill`: the file's pixels stretch over the Before box.
            let side = NSRect(x: dividerX, y: visible.minY, width: max(0, visible.maxX - dividerX),
                              height: visible.height)
            NSGraphicsContext.saveGraphicsState()
            NSBezierPath(rect: side).addClip()
            NSImage(cgImage: afterImage, size: NSSize(width: afterImage.width, height: afterImage.height))
                .draw(in: mediaRect, from: .zero, operation: .sourceOver, fraction: 1,
                      respectFlipped: true, hints: nil)
            NSGraphicsContext.restoreGraphicsState()
            // `.compression-preview-divider`: 2 pt glass text with a dark edge.
            NSColor.black.withAlphaComponent(0.35).setFill()
            NSRect(x: dividerX - 2, y: visible.minY, width: 4, height: visible.height).fill()
            tokens.color("glass-text").setFill()
            NSRect(x: dividerX - 1, y: visible.minY, width: 2, height: visible.height).fill()
            let handle = handleRect
            let circle = NSBezierPath(ovalIn: handle.insetBy(dx: 0.5, dy: 0.5))
            tokens.color("glass-strong").setFill(); circle.fill()
            tokens.color("glass-border-strong").setStroke(); circle.lineWidth = 1; circle.stroke()
            let glyph = strings.handleGlyph as NSString
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
                .foregroundColor: tokens.color("glass-text"),
            ]
            let size = glyph.size(withAttributes: attributes)
            glyph.draw(at: NSPoint(x: handle.midX - size.width / 2, y: handle.midY - size.height / 2),
                       withAttributes: attributes)
            if window?.firstResponder === self {
                drawFocusRing(tokens, in: handle, radius: handle.width / 2)
            }
        }
        if processing {
            // `.compression-preview-veil` and the centred status pill.
            tokens.color("glass-veil").setFill(); visible.fill()
            let text = strings.processing as NSString
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium),
                .foregroundColor: tokens.color("glass-text"),
            ]
            let size = text.size(withAttributes: attributes)
            let spinner: CGFloat = 16, gap = tokens.number("s-4")
            let padding = NSSize(width: tokens.number("s-6"), height: tokens.number("s-4"))
            let width = spinner + gap + ceil(size.width) + padding.width * 2
            let height = max(spinner, ceil(size.height)) + padding.height * 2
            let box = NSRect(x: visible.midX - width / 2, y: visible.midY - height / 2,
                             width: width, height: height)
            pill(box)
            let ring = NSBezierPath(ovalIn: NSRect(x: box.minX + padding.width + 1, y: box.midY - spinner / 2 + 1,
                                                   width: spinner - 2, height: spinner - 2))
            ring.lineWidth = 2
            // Reduced motion (and this still frame) keep a full ring.
            tokens.color("glass-text").setStroke(); ring.stroke()
            text.draw(at: NSPoint(x: box.minX + padding.width + spinner + gap, y: box.midY - size.height / 2),
                      withAttributes: attributes)
        }
        // `.compression-preview-badge`: the bottom corners.
        let before = NSAttributedString(string: badges.before, attributes: pillAttributes())
        let after = NSMutableAttributedString(string: badges.after, attributes: pillAttributes())
        if let savings = badges.savings {
            after.append(NSAttributedString(string: savings, attributes: pillAttributes("positive-text")))
        }
        for (text, left) in [(before, true), (after as NSAttributedString, false)] {
            let size = text.size()
            let width = ceil(size.width) + pad.width * 2, height = ceil(size.height) + pad.height * 2
            let box = NSRect(x: left ? visible.minX + inset : visible.maxX - inset - width,
                             y: visible.maxY - inset - height, width: width, height: height)
            pill(box)
            text.draw(at: NSPoint(x: box.minX + pad.width, y: box.minY + pad.height))
        }
        // `.compression-preview-dismiss`: Hide, top right.
        let dismiss = dismissRect
        let hovered = hoverPoint.map { dismiss.contains($0) } ?? false
        pill(dismiss, hovered: hovered)
        (strings.dismiss as NSString).draw(at: NSPoint(x: dismiss.minX + pad.width, y: dismiss.minY + pad.height),
                                        withAttributes: pillAttributes())
        if let failureMessage {
            // `.compression-preview-error`: a strip across the top.
            let margin = tokens.number("s-4")
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
                .foregroundColor: tokens.color("danger-text"),
            ]
            let width = visible.width - margin * 2
            let text = failureMessage as NSString
            let bounding = text.boundingRect(with: NSSize(width: width - tokens.number("s-4") * 2, height: 200),
                                             options: [.usesLineFragmentOrigin], attributes: attributes)
            let strip = NSRect(x: visible.minX + margin, y: visible.minY + margin, width: width,
                               height: ceil(bounding.height) + tokens.number("s-3") * 2)
            tokens.color("danger-surface").setFill()
            NSBezierPath(roundedRect: strip, xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md")).fill()
            text.draw(with: strip.insetBy(dx: tokens.number("s-4"), dy: tokens.number("s-3")),
                      options: [.usesLineFragmentOrigin], attributes: attributes)
        }
        if let afterHint, afterImage != nil, !processing, let point = hoverPoint,
           visible.contains(point), point.x > dividerX {
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: tokens.number("text-xs"), weight: .medium),
                .foregroundColor: tokens.color("glass-text"),
            ]
            let text = afterHint as NSString
            let bounding = text.boundingRect(with: NSSize(width: 220 - 16, height: 200),
                                             options: [.usesLineFragmentOrigin], attributes: attributes)
            let size = NSSize(width: ceil(bounding.width) + 16, height: ceil(bounding.height) + 10)
            let origin = CompressionCompareView.afterHintOrigin(pointer: point, area: visible, hint: size)
            let box = NSRect(origin: origin, size: size)
            let path = NSBezierPath(roundedRect: box, xRadius: tokens.number("r-sm"), yRadius: tokens.number("r-sm"))
            tokens.color("glass-strong").setFill(); path.fill()
            tokens.color("glass-border").setStroke(); path.lineWidth = 1; path.stroke()
            text.draw(with: box.insetBy(dx: 8, dy: 5), options: [.usesLineFragmentOrigin], attributes: attributes)
        }
        NSGraphicsContext.restoreGraphicsState()
    }

    /// Shipping `compressionAfterHintPosition`: keep the hint inside the frame.
    static func afterHintOrigin(pointer: NSPoint, area: NSRect, hint: NSSize) -> NSPoint {
        let edge: CGFloat = 8
        let width = min(hint.width, max(1, area.width - edge * 2))
        let height = min(hint.height, max(1, area.height - edge * 2))
        var x = pointer.x - area.minX + 12
        var y = pointer.y - area.minY + 14
        if x + width + edge > area.width { x = pointer.x - area.minX - width - edge }
        if y + height + edge > area.height { y = pointer.y - area.minY - height - edge }
        return NSPoint(x: area.minX + min(area.width - width - edge, max(edge, x)),
                       y: area.minY + min(area.height - height - edge, max(edge, y)))
    }
}
