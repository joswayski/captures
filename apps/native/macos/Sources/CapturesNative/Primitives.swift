import AppKit

// Shared shipping UI primitives drawn from design tokens (`styles/base.css`,
// `styles/primitives.css`), used by every AppKit surface.

/// Shipping `--focus-ring-tight`, a 2 pt translucent accent ring, drawn just
/// inside `rect` because AppKit clips a view's drawing to its bounds. Every
/// token-drawn control (buttons, selects, fields, sliders) shares it.
func drawFocusRing(_ tokens: Tokens, in rect: NSRect, radius: CGFloat) {
    let ring = NSBezierPath(roundedRect: rect.insetBy(dx: 1, dy: 1), xRadius: radius, yRadius: radius)
    ring.lineWidth = 2
    tokens.color("theme-accent").withAlphaComponent(0.55).setStroke()
    ring.stroke()
}

/// Shipping scroll bars: a 10 pt bar whose transparent 3 pt border leaves a
/// 4 pt pill thumb in `--border-strong`, lifting to `--text-faint` while the
/// pointer is over it, above a transparent track. It keeps the scroll view's
/// scroller style, so overlay scrollers still fade out when idle.
final class TokenScroller: NSScroller {
    var tokens: Tokens? { didSet { needsDisplay = true } }
    private(set) var thumbHovered = false
    private var thumbTracking: NSTrackingArea?

    override class var isCompatibleWithOverlayScrollers: Bool { true }

    override class func scrollerWidth(for controlSize: NSControl.ControlSize,
                                      scrollerStyle: NSScroller.Style) -> CGFloat { 10 }

    /// The painted thumb inside the knob part.
    var thumbRect: NSRect {
        let knob = rect(for: .knob)
        guard knob.width > 6, knob.height > 6 else { return .zero }
        return knob.insetBy(dx: 3, dy: 3)
    }

    override func draw(_ dirtyRect: NSRect) { drawKnob() }
    override func drawKnobSlot(in slotRect: NSRect, highlight flag: Bool) {}

    override func drawKnob() {
        let thumb = thumbRect
        guard let tokens, thumb.width > 0, thumb.height > 0 else { return }
        let radius = min(thumb.width, thumb.height) / 2
        tokens.color(thumbHovered ? "text-faint" : "border-strong").setFill()
        NSBezierPath(roundedRect: thumb, xRadius: radius, yRadius: radius).fill()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let thumbTracking { removeTrackingArea(thumbTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .mouseMoved, .activeAlways, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(tracking); thumbTracking = tracking
    }

    private func updateThumbHover(_ event: NSEvent?) {
        let inside = event.map { rect(for: .knob).contains(convert($0.locationInWindow, from: nil)) } ?? false
        if inside != thumbHovered { thumbHovered = inside; needsDisplay = true }
    }
    override func mouseEntered(with event: NSEvent) { super.mouseEntered(with: event); updateThumbHover(event) }
    override func mouseMoved(with event: NSEvent) { super.mouseMoved(with: event); updateThumbHover(event) }
    override func mouseExited(with event: NSEvent) { super.mouseExited(with: event); updateThumbHover(nil) }
}

extension NSScrollView {
    /// Replace both scrollers with shipping token scrollers, keeping which
    /// axes show a scroller and the scroller style.
    func useTokenScrollers(_ tokens: Tokens) {
        let showsVertical = hasVerticalScroller, showsHorizontal = hasHorizontalScroller
        let vertical = TokenScroller(frame: NSRect(x: 0, y: 0, width: 10, height: 40))
        vertical.tokens = tokens
        verticalScroller = vertical
        let horizontal = TokenScroller(frame: NSRect(x: 0, y: 0, width: 40, height: 10))
        horizontal.tokens = tokens
        horizontalScroller = horizontal
        hasVerticalScroller = showsVertical; hasHorizontalScroller = showsHorizontal
    }
}

/// Re-theme every token scroller under `view` after an appearance change.
func restyleTokenScrollers(in view: NSView, _ tokens: Tokens) {
    if let scroll = view as? NSScrollView {
        for scroller in [scroll.verticalScroller, scroll.horizontalScroller] {
            (scroller as? TokenScroller)?.tokens = tokens
        }
    }
    view.subviews.forEach { restyleTokenScrollers(in: $0, tokens) }
}
