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
/// `NSPopUpButton` (its items, selection, target/action and VoiceOver name)
/// that opens `TokenSelectListView`, shipping's `.custom-select-listbox`, in
/// place of the native menu. The listbox shows each item's tool tip as its
/// description (shipping `<small>`), and shared `captures_app::controls::select`
/// drives its keys (ArrowUp/Down, Home/End, Enter/Space, Escape) and placement.
final class ClosurePopUpButton: NSPopUpButton {
    var tokens: Tokens! { didSet { needsDisplay = true } }
    var selectStyle: TokenSelectStyle = .field { didSet { needsDisplay = true } }
    var change: ((Int) -> Void)?
    private var pointerInside = false
    private var pointerTracking: NSTrackingArea?
    /// The open listbox and its child window.
    private(set) var listbox: TokenSelectListView?
    private var listboxWindow: NSPanel?
    private var listboxMonitor: Any?
    var isListboxOpen: Bool { listbox != nil }

    @objc func selectedValue() { change?(indexOfSelectedItem) }

    func bindChange(_ callback: @escaping (Int) -> Void) {
        change = callback; target = self; action = #selector(selectedValue)
    }

    deinit {
        if let listboxMonitor { NSEvent.removeMonitor(listboxMonitor) }
        listboxWindow?.orderOut(nil)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        focusRingType = .none
        if window == nil { closeListbox() }
    }

    override func mouseDown(with event: NSEvent) {
        guard isEnabled else { return }
        if acceptsFirstResponder { window?.makeFirstResponder(self) }
        if isListboxOpen { closeListbox() } else { openListbox() }
    }

    /// Space, VoiceOver's press and a programmatic click open the listbox.
    override func performClick(_ sender: Any?) {
        guard isEnabled else { return }
        if isListboxOpen { closeListbox() } else { openListbox() }
    }

    override func accessibilityPerformPress() -> Bool {
        guard isEnabled else { return false }
        performClick(nil); return true
    }

    override func keyDown(with event: NSEvent) {
        let keys: [UInt16: String] = [125: "arrow_down", 126: "arrow_up", 115: "home", 119: "end",
                                      36: "enter", 76: "enter", 49: "space", 53: "escape"]
        let modifiers = event.modifierFlags.intersection([.command, .control, .option])
        if isEnabled, modifiers.isEmpty, let key = keys[event.keyCode], handleSelectKey(key) { return }
        if event.keyCode == 48 { closeListbox() }
        super.keyDown(with: event)
    }

    /// Shipping `CustomSelect` trigger keys from `captures_app::controls::select`.
    /// Returns whether the select used the key.
    @discardableResult
    func handleSelectKey(_ key: String) -> Bool {
        guard isEnabled, numberOfItems > 0 else { return false }
        let selected = max(0, indexOfSelectedItem)
        guard let outcome = ControlsBridge.selectKey(
            open: isListboxOpen, active: listbox?.active ?? selected, selected: selected,
            disabled: itemArray.map { !$0.isEnabled || $0.isSeparatorItem || $0.isHidden }, key: key)
        else { return false }
        if outcome.open {
            if isListboxOpen { listbox?.active = outcome.active } else { openListbox(active: outcome.active) }
            listbox?.revealActive()
        }
        if let chosen = outcome.chosen { choose(chosen) }
        if !outcome.open { closeListbox() }
        return outcome.handled
    }

    /// Select an item as a click in the listbox does, reporting the change.
    func choose(_ index: Int) {
        closeListbox()
        guard (0..<numberOfItems).contains(index), let option = self.item(at: index), option.isEnabled else { return }
        // Like the native menu, choosing reports even the current item.
        selectItem(at: index)
        needsDisplay = true
        _ = sendAction(action, to: target)
    }

    /// Open shipping's listbox below (or above) the trigger, highlighting
    /// `active` (default: the selected item).
    func openListbox(active: Int? = nil) {
        guard let window, isEnabled, !isListboxOpen, let tokens, numberOfItems > 0 else { return }
        let options = itemArray.map { item in
            TokenSelectListView.Option(title: item.title, detail: item.toolTip.flatMap { $0.isEmpty ? nil : $0 },
                                       image: item.image,
                                       enabled: item.isEnabled && !item.isSeparatorItem && !item.isHidden)
        }
        let list = TokenSelectListView(tokens: tokens, glass: selectStyle == .glass, options: options,
                                       selected: indexOfSelectedItem)
        list.active = active ?? max(0, indexOfSelectedItem)
        list.choose = { [weak self] index in self?.choose(index) }
        let content = list.contentSize(minimumWidth: bounds.width)
        let trigger = window.convertToScreen(convert(bounds, to: nil))
        let screen = (window.screen ?? NSScreen.main)?.visibleFrame ?? trigger.insetBy(dx: -400, dy: -400)
        // Shipping `placeCustomSelectMenu`, in y-down points of the visible screen.
        let layout = ControlsBridge.selectLayout(
            trigger: NSRect(x: trigger.minX - screen.minX, y: screen.maxY - trigger.maxY,
                            width: trigger.width, height: trigger.height),
            menuSize: content, viewport: screen.size, optionCount: options.count)
        let height = min(content.height, layout.maxHeight)
        let frame = NSRect(x: screen.minX + layout.left, y: screen.maxY - layout.top - height,
                           width: layout.width, height: height)
        let panel = NSPanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                            backing: .buffered, defer: true)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false; panel.backgroundColor = .clear; panel.hasShadow = true
        panel.level = window.level
        let scroll = NSScrollView(frame: NSRect(origin: .zero, size: frame.size))
        scroll.drawsBackground = false; scroll.hasVerticalScroller = content.height > height
        scroll.scrollerStyle = .overlay; scroll.useTokenScrollers(tokens)
        scroll.wantsLayer = true
        scroll.layer?.cornerRadius = tokens.number("r-lg"); scroll.layer?.masksToBounds = true
        scroll.layer?.borderWidth = 1
        scroll.layer?.borderColor = tokens.color(selectStyle == .glass ? "glass-border" : "border").cgColor
        scroll.layer?.backgroundColor = tokens.color(selectStyle == .glass ? "glass-raised" : "surface-overlay").cgColor
        list.frame = NSRect(x: 0, y: 0, width: frame.width, height: content.height)
        scroll.documentView = list
        panel.contentView = scroll
        window.addChildWindow(panel, ordered: .above)
        listbox = list; listboxWindow = panel
        list.revealActive()
        needsDisplay = true
        // A click elsewhere closes the listbox, like shipping's outside press.
        listboxMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) {
            [weak self] event in
            guard let self, let panel = self.listboxWindow else { return event }
            if event.window !== panel && !(event.window === self.window
                && self.bounds.contains(self.convert(event.locationInWindow, from: nil))) {
                self.closeListbox()
            }
            return event
        }
    }

    func closeListbox() {
        if let listboxMonitor { NSEvent.removeMonitor(listboxMonitor) }
        listboxMonitor = nil
        guard let panel = listboxWindow else { listbox = nil; return }
        panel.parent?.removeChildWindow(panel)
        panel.orderOut(nil)
        listboxWindow = nil; listbox = nil
        needsDisplay = true
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
        let accepted = super.resignFirstResponder()
        if accepted { closeListbox() }
        needsDisplay = true; return accepted
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let tokens else { return }
        let radius = tokens.number("r-md")
        let alpha: CGFloat = isEnabled ? 1 : 0.5
        let focused = window?.firstResponder === self || isListboxOpen
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
        var textX = selectStyle == .inline ? tokens.number("s-4") : padding
        // Shipping `.screenshot-text-style-trigger`: the selected item's
        // preview chip, centred in an 82 pt column, then the label.
        if !pullsDown, let image = selectedItem?.image {
            let slot = max(82, image.size.width)
            let chip = NSRect(x: textX + (slot - image.size.width) / 2,
                              y: (bounds.height - image.size.height) / 2,
                              width: image.size.width, height: image.size.height)
            image.draw(in: chip, from: .zero, operation: .sourceOver, fraction: alpha,
                       respectFlipped: true, hints: nil)
            textX += slot + tokens.number("s-4")
        }
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
        // Shipping turns the chevron over while the listbox is open.
        let (edge, tip): (CGFloat, CGFloat) = isListboxOpen ? (10, 6) : (6, 10)
        let chevron = NSBezierPath()
        chevron.move(to: point(4, edge)); chevron.line(to: point(8, tip)); chevron.line(to: point(12, edge))
        chevron.lineWidth = 1.7 * scale; chevron.lineCapStyle = .round; chevron.lineJoinStyle = .round
        glyph.withAlphaComponent(alpha).setStroke(); chevron.stroke()
    }
}

/// Shipping `.custom-select-listbox` for `ClosurePopUpButton`: `--s-2`
/// padding, rows with `--s-4` side padding and the label in `--text-sm`,
/// a description in `--text-xs` `--text-faint` under it (shipping `<small>`),
/// a check on the selected option and `--surface-hover` behind the active
/// one; the glass variant uses the media palette. Rows follow the pointer and
/// a click chooses. Row heights never depend on fonts.
final class TokenSelectListView: NSView {
    struct Option {
        let title: String
        let detail: String?
        let image: NSImage?
        let enabled: Bool
    }

    override var isFlipped: Bool { true }
    let tokens: Tokens
    let glass: Bool
    let options: [Option]
    let selected: Int
    var active = 0 { didSet { if active != oldValue { needsDisplay = true } } }
    var choose: (Int) -> Void = { _ in }
    private var pointerTracking: NSTrackingArea?

    /// 30 pt rows, 46 pt with a description, taller for a preview chip.
    static func rowHeight(_ option: Option) -> CGFloat {
        let height: CGFloat = option.detail == nil ? 30 : 46
        guard let image = option.image else { return height }
        return max(height, image.size.height + 12)
    }

    init(tokens: Tokens, glass: Bool, options: [Option], selected: Int) {
        self.tokens = tokens; self.glass = glass; self.options = options; self.selected = selected
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.list)
        setAccessibilityLabel("Options")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private var padding: CGFloat { tokens.number("s-2") }

    func rowRect(_ index: Int) -> NSRect {
        var y = padding
        for previous in options.prefix(max(0, index)) { y += Self.rowHeight(previous) }
        let height = options.indices.contains(index) ? Self.rowHeight(options[index]) : 0
        return NSRect(x: padding, y: y, width: max(0, bounds.width - 2 * padding), height: height)
    }

    /// The listbox's natural size: its widest option (up to shipping's
    /// 360 pt) and at least the trigger's width, and every row.
    func contentSize(minimumWidth: CGFloat) -> NSSize {
        let label = NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        let small = NSFont.systemFont(ofSize: tokens.number("text-xs"))
        var widest: CGFloat = 0
        for option in options {
            var width = (option.title as NSString).size(withAttributes: [.font: label]).width
            if let detail = option.detail {
                width = max(width, (detail as NSString).size(withAttributes: [.font: small]).width)
            }
            if let image = option.image { width += image.size.width + tokens.number("s-4") }
            widest = max(widest, width)
        }
        // Row padding, shipping's `--s-6` gap and the 14 pt check column.
        widest += 2 * tokens.number("s-4") + tokens.number("s-6") + 14 + 2 * padding
        let height = options.reduce(2 * padding) { $0 + Self.rowHeight($1) }
        return NSSize(width: ceil(max(minimumWidth, min(widest, 360))), height: height)
    }

    func index(at point: NSPoint) -> Int? {
        options.indices.first { rowRect($0).contains(point) }
    }

    /// Keep the active option in view as keys move it.
    func revealActive() {
        guard options.indices.contains(active) else { return }
        scrollToVisible(rowRect(active).insetBy(dx: 0, dy: -padding))
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let pointerTracking { removeTrackingArea(pointerTracking) }
        let tracking = NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeAlways, .inVisibleRect],
                                      owner: self, userInfo: nil)
        addTrackingArea(tracking); pointerTracking = tracking
    }

    override func mouseMoved(with event: NSEvent) {
        if let index = index(at: convert(event.locationInWindow, from: nil)), options[index].enabled {
            active = index
        }
    }

    override func mouseDown(with event: NSEvent) {}

    override func mouseUp(with event: NSEvent) {
        guard let index = index(at: convert(event.locationInWindow, from: nil)), options[index].enabled else { return }
        choose(index)
    }

    override func accessibilityChildren() -> [Any]? {
        options.indices.map { index in
            let frame = window?.convertToScreen(convert(rowRect(index), to: nil)) ?? .zero
            return NSAccessibilityElement.element(withRole: .menuItem, frame: frame,
                                                  label: options[index].title, parent: self)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let text = tokens.color(glass ? "glass-text" : "text")
        let muted = tokens.color(glass ? "glass-text-muted" : "text-muted")
        let faint = tokens.color(glass ? "glass-text-subtle" : "text-faint")
        let dark = tokens.color("text").brightnessComponent > 0.5
        let check = tokens.color(glass || dark ? "theme-accent" : "theme-accent-readable")
        let inset = tokens.number("s-4")
        for (index, option) in options.enumerated() {
            let row = rowRect(index)
            guard row.intersects(dirtyRect) else { continue }
            let chosen = index == selected
            let highlighted = index == active && option.enabled
            if highlighted {
                tokens.color(glass ? "glass-hover" : "surface-hover").setFill()
                NSBezierPath(roundedRect: row, xRadius: tokens.number("r-sm"), yRadius: tokens.number("r-sm")).fill()
            }
            let alpha: CGFloat = option.enabled ? 1 : 0.5
            var x = row.minX + inset
            if let image = option.image {
                image.draw(in: NSRect(x: x, y: row.midY - image.size.height / 2,
                                      width: image.size.width, height: image.size.height),
                           from: .zero, operation: .sourceOver, fraction: alpha, respectFlipped: true, hints: nil)
                x += image.size.width + inset
            }
            let width = max(0, row.maxX - inset - 14 - tokens.number("s-6") - x)
            let label = NSAttributedString(string: option.title, attributes: [
                .font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: chosen ? .medium : .regular),
                .foregroundColor: (chosen || highlighted ? text : muted).withAlphaComponent(alpha),
            ])
            let labelHeight = ceil(label.size().height)
            var top = row.midY - labelHeight / 2
            if let detail = option.detail {
                let small = NSAttributedString(string: detail, attributes: [
                    .font: NSFont.systemFont(ofSize: tokens.number("text-xs")),
                    .foregroundColor: faint.withAlphaComponent(alpha),
                ])
                let smallHeight = ceil(small.size().height)
                top = row.midY - (labelHeight + 2 + smallHeight) / 2
                small.draw(with: NSRect(x: x, y: top + labelHeight + 2, width: width, height: smallHeight),
                           options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            }
            label.draw(with: NSRect(x: x, y: top, width: width, height: labelHeight),
                       options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            guard chosen else { continue }
            // The selected option's check (`> span:last-child`).
            let box = NSRect(x: row.maxX - inset - 14, y: row.midY - 7, width: 14, height: 14)
            let mark = NSBezierPath()
            mark.move(to: NSPoint(x: box.minX + 3, y: box.minY + 7.5))
            mark.line(to: NSPoint(x: box.minX + 6, y: box.minY + 10.5))
            mark.line(to: NSPoint(x: box.minX + 11, y: box.minY + 4))
            mark.lineWidth = 1.7; mark.lineCapStyle = .round; mark.lineJoinStyle = .round
            check.withAlphaComponent(alpha).setStroke(); mark.stroke()
        }
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

    /// Shipping `CustomSelect` trigger keys: the next open state, active
    /// option and any chosen option, and whether the key was used.
    static func selectKey(open: Bool, active: Int, selected: Int, disabled: [Bool],
                          key: String) -> (open: Bool, active: Int, chosen: Int?, handled: Bool)? {
        guard let result = try? request(["operation": "select_key", "open": open, "active": active,
                                         "selected": selected, "disabled": disabled, "key": key]),
              let isOpen = result["open"] as? Bool, let next = (result["active"] as? NSNumber)?.intValue,
              let handled = result["handled"] as? Bool else { return nil }
        return (isOpen, next, (result["chosen"] as? NSNumber)?.intValue, handled)
    }

    /// Shipping `placeCustomSelectMenu` in y-down viewport points.
    static func selectLayout(trigger: NSRect, menuSize: NSSize, viewport: NSSize,
                             optionCount: Int) -> (left: CGFloat, top: CGFloat, width: CGFloat, maxHeight: CGFloat) {
        let result = try? request([
            "operation": "select_layout",
            "trigger": [trigger.minX, trigger.minY, trigger.width, trigger.height].map { Double($0) },
            "menu_width": Double(menuSize.width), "menu_height": Double(menuSize.height),
            "viewport_width": Double(viewport.width), "viewport_height": Double(viewport.height),
            "option_count": optionCount,
        ])
        func number(_ key: String, _ fallback: CGFloat) -> CGFloat {
            (result?[key] as? NSNumber).map { CGFloat($0.doubleValue) } ?? fallback
        }
        return (number("left", trigger.minX), number("top", trigger.maxY + 6),
                number("width", max(trigger.width, menuSize.width)), number("max_height", 240))
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
