import AppKit
import CCapturesSettings

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

    /// The painted thumb inside the knob part: shipping's 4 pt pill centred
    /// across the 10 pt bar, whatever cross size AppKit gives the knob part.
    var thumbRect: NSRect {
        let knob = rect(for: .knob)
        guard knob.width > 6, knob.height > 6 else { return .zero }
        let thickness: CGFloat = 4
        if bounds.height > bounds.width {
            return NSRect(x: bounds.midX - thickness / 2, y: knob.minY + 3,
                          width: thickness, height: knob.height - 6)
        }
        return NSRect(x: knob.minX + 3, y: bounds.midY - thickness / 2,
                      width: knob.width - 6, height: thickness)
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

/// Shipping `NumberInput` behaviour from `captures_app::controls::number`.
enum ControlsBridge {
    static func request(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_controls_v1($0)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
    }

    private static func bounded(_ object: [String: Any], min: Double?, max: Double?) -> [String: Any] {
        var object = object
        if let min { object["min"] = min }
        if let max { object["max"] = max }
        return object
    }

    /// The field text one step up or down, clamped to the bounds.
    static func step(_ text: String, up: Bool, min: Double?, max: Double?) -> String? {
        let result = try? request(bounded(["operation": "number_step", "text": text, "up": up],
                                          min: min, max: max))
        return result?["text"] as? String
    }

    /// Whether Decrease and Increase are disabled at the bounds.
    static func bounds(_ text: String, min: Double?, max: Double?) -> (atMin: Bool, atMax: Bool) {
        let result = try? request(bounded(["operation": "number_bounds", "text": text],
                                          min: min, max: max))
        return (result?["at_min"] as? Bool ?? false, result?["at_max"] as? Bool ?? false)
    }
}

/// Text inset for `TokenNumberField`: shipping padding, vertically centred,
/// with room for the 26 pt stepper column.
final class TokenNumberFieldCell: NSTextFieldCell {
    var trailingInset: CGFloat = 0
    var leadingInset: CGFloat = 12

    override func drawingRect(forBounds rect: NSRect) -> NSRect {
        let base = super.drawingRect(forBounds: rect)
        let height = cellSize(forBounds: rect).height
        let inset = NSRect(x: rect.minX + leadingInset, y: rect.minY, width: max(0,
            rect.width - leadingInset - trailingInset), height: rect.height)
        let y = inset.minY + max(0, (inset.height - height) / 2)
        return NSRect(x: inset.minX, y: y, width: inset.width, height: min(base.height, height))
    }

    override func edit(withFrame rect: NSRect, in controlView: NSView, editor textObj: NSText,
                       delegate: Any?, event: NSEvent?) {
        super.edit(withFrame: drawingRect(forBounds: rect), in: controlView, editor: textObj,
                   delegate: delegate, event: event)
    }

    override func select(withFrame rect: NSRect, in controlView: NSView, editor textObj: NSText,
                         delegate: Any?, start selStart: Int, length selLength: Int) {
        super.select(withFrame: drawingRect(forBounds: rect), in: controlView, editor: textObj,
                     delegate: delegate, start: selStart, length: selLength)
    }
}

/// One half of a `TokenNumberField` stepper column. It is a plain view, not a
/// control, so it stays out of the key-view loop like shipping `tabIndex={-1}`,
/// while VoiceOver still presses it as a named button.
final class TokenStepperHalf: NSView {
    let up: Bool
    var tokens: Tokens? { didSet { needsDisplay = true } }
    var available = true { didSet { needsDisplay = true } }
    var press: (() -> Void)?
    private var pointerInside = false
    private var pointerTracking: NSTrackingArea?

    init(up: Bool) {
        self.up = up
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override var isFlipped: Bool { true }
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .button }
    override func isAccessibilityEnabled() -> Bool { available }
    override func accessibilityPerformPress() -> Bool {
        guard available else { return false }
        press?(); return true
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
    override func mouseDown(with event: NSEvent) { if available { press?() } }

    override func draw(_ dirtyRect: NSRect) {
        guard let tokens else { return }
        let hot = available && pointerInside
        if hot { tokens.color("surface-active").setFill(); bounds.fill() }
        let color = tokens.color(hot ? "text" : "text-subtle").withAlphaComponent(available ? 1 : 0.3)
        // Shipping 12-unit chevrons `M3 7.5 6 4.5 9 7.5` and `M3 4.5 6 7.5 9 4.5`.
        let glyph = NSRect(x: bounds.midX - 5.5, y: bounds.midY - 5.5, width: 11, height: 11)
        let scale = glyph.width / 12
        func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            NSPoint(x: glyph.minX + x * scale, y: glyph.minY + y * scale)
        }
        let chevron = NSBezierPath()
        if up {
            chevron.move(to: point(3, 7.5)); chevron.line(to: point(6, 4.5)); chevron.line(to: point(9, 7.5))
        } else {
            chevron.move(to: point(3, 4.5)); chevron.line(to: point(6, 7.5)); chevron.line(to: point(9, 4.5))
        }
        chevron.lineWidth = 2 * scale; chevron.lineCapStyle = .round; chevron.lineJoinStyle = .round
        color.setStroke(); chevron.stroke()
    }
}

/// Shipping `NumberInput` on AppKit: a token field with mono digits, the
/// focus ring while editing, and Increase/Decrease steppers (hidden while
/// disabled). `stepped` runs after a stepper or ArrowUp/ArrowDown changes the
/// text, so the owner can treat it like typing.
final class TokenNumberField: NSTextField {
    var tokens: Tokens? {
        didSet {
            increaseHalf.tokens = tokens; decreaseHalf.tokens = tokens
            updateTextColor(); needsDisplay = true
        }
    }
    var minimum: (() -> Double?) = { nil }
    var maximum: (() -> Double?) = { nil }
    var stepped: ((TokenNumberField) -> Void)?
    let increaseHalf = TokenStepperHalf(up: true)
    let decreaseHalf = TokenStepperHalf(up: false)
    static let stepperWidth: CGFloat = 26

    override class var cellClass: AnyClass? {
        get { TokenNumberFieldCell.self }
        set { super.cellClass = newValue }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        setUpNumberField()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private func setUpNumberField() {
        isBezeled = false; isBordered = false; drawsBackground = false
        focusRingType = .none
        increaseHalf.press = { [weak self] in self?.step(up: true) }
        decreaseHalf.press = { [weak self] in self?.step(up: false) }
        addSubview(increaseHalf); addSubview(decreaseHalf)
        updateSteppers()
    }

    override var isEnabled: Bool { didSet { updateTextColor(); updateSteppers(); needsDisplay = true } }

    private func updateTextColor() {
        guard let tokens else { return }
        textColor = tokens.color("text").withAlphaComponent(isEnabled ? 1 : 0.5)
    }
    override var stringValue: String { didSet { updateSteppers() } }
    override func setAccessibilityLabel(_ accessibilityLabel: String?) {
        super.setAccessibilityLabel(accessibilityLabel)
        let name = accessibilityLabel ?? ""
        increaseHalf.setAccessibilityLabel(name.isEmpty ? "Increase" : "Increase \(name)")
        decreaseHalf.setAccessibilityLabel(name.isEmpty ? "Decrease" : "Decrease \(name)")
    }

    override func resizeSubviews(withOldSize oldSize: NSSize) {
        super.resizeSubviews(withOldSize: oldSize)
        updateSteppers()
    }

    /// Lay out and enable the steppers for the current text and bounds.
    func updateSteppers() {
        let visible = isEnabled && isEditable
        (cell as? TokenNumberFieldCell)?.trailingInset = visible ? Self.stepperWidth + 4 : 12
        let column = NSRect(x: bounds.maxX - Self.stepperWidth - 1, y: 1,
                            width: Self.stepperWidth, height: max(0, bounds.height - 2))
        let half = floor(column.height / 2)
        increaseHalf.frame = NSRect(x: column.minX, y: column.minY, width: column.width, height: half)
        decreaseHalf.frame = NSRect(x: column.minX, y: column.minY + half, width: column.width,
                                    height: column.height - half)
        // Subviews of a text field follow its (unflipped) coordinates.
        if !isFlipped {
            increaseHalf.frame.origin.y = column.maxY - half
            decreaseHalf.frame.origin.y = column.minY
        }
        increaseHalf.isHidden = !visible; decreaseHalf.isHidden = !visible
        let text = currentEditor()?.string ?? stringValue
        let limits = ControlsBridge.bounds(text, min: minimum(), max: maximum())
        increaseHalf.available = !limits.atMax
        decreaseHalf.available = !limits.atMin
        needsDisplay = true
    }

    /// One shipping step from the current text, as if typed.
    func step(up: Bool) {
        let text = currentEditor()?.string ?? stringValue
        guard isEnabled, let next = ControlsBridge.step(text, up: up, min: minimum(), max: maximum())
        else { return }
        stringValue = next
        currentEditor()?.string = next
        stepped?(self)
    }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder(); needsDisplay = true; return accepted
    }
    override func textDidEndEditing(_ notification: Notification) {
        super.textDidEndEditing(notification); needsDisplay = true; updateSteppers()
    }
    override func textDidChange(_ notification: Notification) {
        super.textDidChange(notification); updateSteppers()
    }

    override func draw(_ dirtyRect: NSRect) {
        if let tokens {
            let radius = tokens.number("r-md")
            let alpha: CGFloat = isEnabled ? 1 : 0.5
            let editing = currentEditor() != nil
            let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                                       xRadius: radius, yRadius: radius)
            tokens.color("surface-field").withAlphaComponent(alpha).setFill(); outline.fill()
            if !increaseHalf.isHidden {
                NSGraphicsContext.saveGraphicsState()
                outline.addClip()
                let column = NSRect(x: bounds.maxX - Self.stepperWidth - 1, y: 0,
                                    width: Self.stepperWidth + 1, height: bounds.height)
                tokens.color("surface-hover").setFill(); column.fill()
                tokens.color("border-subtle").setFill()
                NSRect(x: column.minX, y: 0, width: 1, height: bounds.height).fill()
                NSRect(x: column.minX, y: floor(bounds.midY), width: column.width, height: 1).fill()
                NSGraphicsContext.restoreGraphicsState()
            }
            tokens.color(editing ? "theme-accent" : "control-border").withAlphaComponent(alpha).setStroke()
            outline.lineWidth = 1; outline.stroke()
            if editing { drawFocusRing(tokens, in: bounds, radius: radius) }
        }
        super.draw(dirtyRect)
    }
}

/// Shipping `RangeSlider` track and thumb: a 4 pt pill in `--n-6` filled with
/// the accent up to a 14 pt raised thumb, with the token focus ring.
final class TokenSliderCell: NSSliderCell {
    var tokens: Tokens?
    private static let thumb: CGFloat = 14

    override func knobRect(flipped: Bool) -> NSRect {
        let base = super.knobRect(flipped: flipped)
        return NSRect(x: base.midX - Self.thumb / 2, y: base.midY - Self.thumb / 2,
                      width: Self.thumb, height: Self.thumb)
    }

    override func drawBar(inside rect: NSRect, flipped: Bool) {
        guard let tokens else { return super.drawBar(inside: rect, flipped: flipped) }
        let alpha: CGFloat = isEnabled ? 1 : 0.45
        let track = NSRect(x: rect.minX, y: rect.midY - 2, width: rect.width, height: 4)
        tokens.color("n-6").withAlphaComponent(alpha).setFill()
        NSBezierPath(roundedRect: track, xRadius: 2, yRadius: 2).fill()
        let knob = knobRect(flipped: flipped)
        let filled = NSRect(x: track.minX, y: track.minY, width: max(0, knob.midX - track.minX),
                            height: track.height)
        tokens.color("theme-accent").withAlphaComponent(alpha).setFill()
        NSBezierPath(roundedRect: filled, xRadius: 2, yRadius: 2).fill()
    }

    override func drawKnob(_ knobRect: NSRect) {
        guard let tokens else { return super.drawKnob(knobRect) }
        let alpha: CGFloat = isEnabled ? 1 : 0.45
        let circle = NSBezierPath(ovalIn: knobRect.insetBy(dx: 0.5, dy: 0.5))
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.12)
        shadow.shadowBlurRadius = 3; shadow.shadowOffset = NSSize(width: 0, height: -1)
        shadow.set()
        tokens.color("surface-raised").setFill(); circle.fill()
        NSGraphicsContext.restoreGraphicsState()
        tokens.color("border-strong").withAlphaComponent(alpha).setStroke()
        circle.lineWidth = 1; circle.stroke()
        if let view = controlView, view.window?.firstResponder === view {
            drawFocusRing(tokens, in: knobRect.insetBy(dx: -3, dy: -3), radius: Self.thumb / 2 + 2)
        }
    }
}

/// An `NSSlider` drawn as shipping `RangeSlider`; value, target/action,
/// keyboard stepping and accessibility stay AppKit's.
final class TokenSlider: NSSlider {
    override class var cellClass: AnyClass? {
        get { TokenSliderCell.self }
        set { super.cellClass = newValue }
    }

    var tokens: Tokens? {
        get { (cell as? TokenSliderCell)?.tokens }
        set { (cell as? TokenSliderCell)?.tokens = newValue; focusRingType = .none; needsDisplay = true }
    }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder(); needsDisplay = true; return accepted
    }
    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder(); needsDisplay = true; return accepted
    }
}
