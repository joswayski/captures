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

/// How a token select's trigger is drawn.
enum TokenSelectStyle {
    /// `.custom-select-trigger`: a field with the selected label and a chevron.
    case field
    /// The capture menu's media-palette trigger.
    case glass
    /// `.filename-format-select`: borderless inside another field.
    case inline
}

/// Shipping `CustomSelect` on AppKit: a token-drawn trigger over
/// `NSPopUpButton`, whose native menu keeps arrow, Return, Escape and
/// type-select keyboard handling and VoiceOver semantics. An item's tool tip
/// is shown as its description (shipping `<small>`) while the menu is open.
final class ClosurePopUpButton: NSPopUpButton {
    var tokens: Tokens! { didSet { needsDisplay = true } }
    var selectStyle: TokenSelectStyle = .field { didSet { needsDisplay = true } }
    var change: ((Int) -> Void)?
    private var pointerInside = false
    private var pointerTracking: NSTrackingArea?
    private var popUpObserver: NSObjectProtocol?
    private var menuEndObserver: NSObjectProtocol?
    /// Plain titles of described items while the menu is open.
    private var plainTitles: [(NSMenuItem, String)] = []

    @objc func selectedValue() { change?(indexOfSelectedItem) }

    func bindChange(_ callback: @escaping (Int) -> Void) {
        change = callback; target = self; action = #selector(selectedValue)
    }

    deinit {
        for observer in [popUpObserver, menuEndObserver].compactMap({ $0 }) {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        focusRingType = .none
        guard popUpObserver == nil else { return }
        popUpObserver = NotificationCenter.default.addObserver(
            forName: NSPopUpButton.willPopUpNotification, object: self, queue: .main
        ) { [weak self] _ in self?.describeItems() }
        menuEndObserver = NotificationCenter.default.addObserver(
            forName: NSMenu.didEndTrackingNotification, object: nil, queue: .main
        ) { [weak self] notification in
            guard let self, notification.object as? NSMenu === self.menu else { return }
            self.restoreItems()
        }
    }

    /// Two-line items while the menu is open: the label, then its description
    /// in secondary text. `restoreItems` puts the plain titles back.
    func describeItems() {
        restoreItems()
        let font = menu?.font ?? NSFont.menuFont(ofSize: 0)
        for item in itemArray {
            guard let detail = item.toolTip, !detail.isEmpty else { continue }
            plainTitles.append((item, item.title))
            let title = NSMutableAttributedString(string: item.title, attributes: [.font: font])
            title.append(NSAttributedString(string: "\n" + detail, attributes: [
                .font: NSFont.systemFont(ofSize: max(10, font.pointSize - 2)),
                .foregroundColor: NSColor.secondaryLabelColor,
            ]))
            item.attributedTitle = title
        }
    }

    func restoreItems() {
        for (item, title) in plainTitles {
            item.attributedTitle = nil
            item.title = title
        }
        plainTitles = []
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let pointerTracking { removeTrackingArea(pointerTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking); pointerTracking = tracking
    }
    override func mouseEntered(with event: NSEvent) { pointerInside = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { pointerInside = false; needsDisplay = true }
    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder(); needsDisplay = true; return accepted
    }
    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder(); needsDisplay = true; return accepted
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let tokens else { return }
        let radius = tokens.number("r-md")
        let alpha: CGFloat = isEnabled ? 1 : 0.5
        let focused = window?.firstResponder === self
        let hovered = isEnabled && pointerInside
        let fill: NSColor, border: NSColor, ink: NSColor, glyph: NSColor
        switch selectStyle {
        case .field:
            fill = tokens.color("surface-field")
            border = tokens.color(focused ? "theme-accent" : hovered ? "border-strong" : "control-border")
            ink = tokens.color("text"); glyph = tokens.color("text-subtle")
        case .glass:
            fill = NSColor.black.withAlphaComponent(0.3)
            border = tokens.color(focused ? "theme-accent" : hovered ? "glass-border-strong" : "glass-border")
            ink = tokens.color("glass-text"); glyph = tokens.color("glass-text-subtle")
        case .inline:
            fill = hovered ? tokens.color("surface-hover") : .clear
            border = .clear
            ink = tokens.color(hovered || focused ? "text" : "text-subtle"); glyph = ink
        }
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        fill.withAlphaComponent(fill.alphaComponent * alpha).setFill(); outline.fill()
        border.withAlphaComponent(border.alphaComponent * alpha).setStroke()
        outline.lineWidth = 1; outline.stroke()
        if focused && selectStyle != .inline { drawFocusRing(tokens, in: bounds, radius: radius) }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
            .foregroundColor: ink.withAlphaComponent(alpha),
        ]
        let padding = tokens.number(selectStyle == .inline ? "s-3" : "s-4")
        let text = titleOfSelectedItem ?? title
        let size = (text as NSString).size(withAttributes: attributes)
        let textX = selectStyle == .inline ? tokens.number("s-4") : padding
        let textRect = NSRect(x: textX, y: (bounds.height - size.height) / 2,
            width: max(0, bounds.width - textX - padding - 14 - tokens.number("s-3")), height: size.height)
        (text as NSString).draw(with: textRect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
            attributes: attributes)
        // The shipping 16-unit chevron `m4 6 4 4 4-4`, independent of flipping.
        let glyphRect = NSRect(x: bounds.width - padding - 14, y: (bounds.height - 14) / 2, width: 14, height: 14)
        let scale = glyphRect.width / 16
        func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            let down = glyphRect.minY + y * scale
            return NSPoint(x: glyphRect.minX + x * scale, y: isFlipped ? down : bounds.height - down)
        }
        let chevron = NSBezierPath()
        chevron.move(to: point(4, 6)); chevron.line(to: point(8, 10)); chevron.line(to: point(12, 6))
        chevron.lineWidth = 1.7 * scale; chevron.lineCapStyle = .round; chevron.lineJoinStyle = .round
        glyph.withAlphaComponent(alpha).setStroke(); chevron.stroke()
    }
}
