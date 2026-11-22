import AppKit

/// Shipping `ColorField` palette and copy from `captures-app::editor_chrome::colors`,
/// shared with the wgpu host through the editor chrome ABI.
enum EditorColors {
    private static var group: [String: Any] { EditorChrome.copy["colors"] as? [String: Any] ?? [:] }

    static func text(_ key: String) -> String { group[key] as? String ?? "" }

    static func metric(_ key: String) -> CGFloat {
        CGFloat((group[key] as? NSNumber)?.doubleValue ?? 0)
    }

    static var swatches: [String] { group["swatches"] as? [String] ?? [] }

    /// The custom-color picker's copy and metrics (`colors::picker`).
    static var picker: [String: Any] { group["picker"] as? [String: Any] ?? [:] }

    /// An editor color the renderer reads, uppercased: `#RRGGBB`, `#RGB`, or
    /// `#RRGGBBAA` with the picker's opacity.
    static func normalized(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.utf8.count == 9 else { return PreferencesController.normalizeHex(trimmed) }
        let alpha = trimmed.suffix(2)
        guard let base = PreferencesController.normalizeHex(String(trimmed.prefix(7))),
              alpha.allSatisfy({ $0.isASCII && $0.isHexDigit }) else { return nil }
        return base + alpha.uppercased()
    }

    /// Lowercase `#rrggbb`, plus an alpha byte when `alpha` is allowed and the
    /// color is translucent (`Hsva::hex`).
    static func hex(_ color: NSColor, alpha: Bool) -> String? {
        guard let srgb = color.usingColorSpace(.sRGB), let base = srgb.rgbHex?.lowercased() else { return nil }
        let byte = Int((min(max(srgb.alphaComponent, 0), 1) * 255).rounded())
        return alpha && byte < 255 ? base + String(format: "%02x", byte) : base
    }

    static var defaultCanvasBackground: String {
        group["default_canvas_background"] as? String ?? "#f7f7f5"
    }

    /// Shipping marks a swatch active when the value starts with it, ignoring case.
    static func swatchActive(_ value: String, _ swatch: String) -> Bool {
        !swatch.isEmpty && value.lowercased().hasPrefix(swatch.lowercased())
    }

    static func swatchLabel(_ field: String, _ swatch: String) -> String { "\(field): \(swatch)" }

    static func backgroundLabel(_ background: String?) -> String {
        "\(text("background")): \(background ?? "transparent")"
    }

    /// `repeat(auto-fill, minmax(minCell, 1fr))` for `count` tiles, as in
    /// `editor_chrome::colors::grid`.
    struct Grid: Equatable {
        let columns: Int
        let rows: Int
        let cellWidth: CGFloat

        init(width: CGFloat, minCell: CGFloat, count: Int) {
            let usable = width.isFinite ? max(0, width) : 0
            let fit = minCell > 0 ? Int((usable / minCell).rounded(.down)) : 1
            columns = min(max(1, fit), max(1, count))
            rows = (max(1, count) + columns - 1) / columns
            cellWidth = usable > 0 ? usable / CGFloat(max(1, fit)) : minCell
        }

        func center(_ index: Int, tile: CGFloat, rowGap: CGFloat, padding: CGFloat) -> NSPoint {
            NSPoint(x: (CGFloat(index % columns) + 0.5) * cellWidth,
                    y: padding + CGFloat(index / columns) * (tile + rowGap) + tile / 2)
        }

        func height(tile: CGFloat, rowGap: CGFloat, padding: CGFloat) -> CGFloat {
            2 * padding + CGFloat(rows) * tile + CGFloat(max(0, rows - 1)) * rowGap
        }
    }
}

/// One round shipping swatch (`.screenshot-color-field button`).
final class ColorSwatchButton: NSButton {
    let swatchHex: String
    var tokens: Tokens { didSet { needsDisplay = true } }
    var active = false {
        didSet { needsDisplay = true; setAccessibilityValue(active) }
    }
    var pressed: (() -> Void)?
    private var hovered = false
    private var hoverTracking: NSTrackingArea?

    override var isEnabled: Bool {
        didSet { window?.invalidateCursorRects(for: self); needsDisplay = true }
    }

    init(swatchHex: String, label: String, tokens: Tokens) {
        self.swatchHex = swatchHex; self.tokens = tokens
        super.init(frame: .zero)
        title = ""; isBordered = false; setButtonType(.momentaryChange)
        focusRingType = .exterior
        target = self; action = #selector(activateSwatch)
        setAccessibilityRole(.radioButton); setAccessibilityLabel(label)
        setAccessibilityValue(false)
        toolTip = swatchHex
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    @objc private func activateSwatch() { pressed?() }

    override func resetCursorRects() {
        if isEnabled { addCursorRect(bounds, cursor: .pointingHand) }
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking); hoverTracking = tracking
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }

    private var circle: NSRect {
        let side = EditorColors.metric("tile")
        return NSRect(x: bounds.midX - side / 2, y: bounds.midY - side / 2, width: side, height: side)
    }

    override func draw(_ dirtyRect: NSRect) {
        let alpha: CGFloat = isEnabled ? 1 : 0.4
        if active {
            // `box-shadow: 0 0 0 2px var(--editor-panel), 0 0 0 4px var(--theme-accent)`.
            tokens.color("theme-accent").faded(alpha).setFill()
            NSBezierPath(ovalIn: circle.insetBy(dx: -4, dy: -4)).fill()
            tokens.color("surface-raised").setFill()
            NSBezierPath(ovalIn: circle.insetBy(dx: -2, dy: -2)).fill()
        } else if isEnabled && hovered {
            tokens.color("border-strong").setFill()
            NSBezierPath(ovalIn: circle.insetBy(dx: -3, dy: -3)).fill()
            tokens.color("surface-raised").setFill()
            NSBezierPath(ovalIn: circle.insetBy(dx: -1.5, dy: -1.5)).fill()
        }
        (NSColor(hex: swatchHex) ?? .black).withAlphaComponent(alpha).setFill()
        let dot = NSBezierPath(ovalIn: circle.insetBy(dx: 0.5, dy: 0.5))
        dot.fill()
        tokens.color("border-strong").setStroke()
        dot.lineWidth = 1; dot.stroke()
    }

    override func drawFocusRingMask() { NSBezierPath(ovalIn: circle).fill() }
    override var focusRingMaskBounds: NSRect { circle }
}

/// A color well that opens Captures' own picker (`editor_chrome::colors::picker`)
/// instead of the system Colors panel: a hue/saturation wheel, brightness and,
/// when `allowsAlpha`, opacity. A click, Space and VoiceOver's press all reach
/// `activate(_:)`, which never calls super, so `NSColorPanel.shared` is never
/// built. Each change sets `color` and sends the action, as the panel did.
class PickerColorWell: NSColorWell {
    var allowsAlpha = false
    var pickerTokens: Tokens?
    private(set) var picker: ColorPickerView?
    private var panel: NSPanel?
    private var monitors: [Any] = []

    override var isActive: Bool { panel != nil }

    override func activate(_ exclusive: Bool) {
        guard isEnabled, panel == nil, let window, let tokens = pickerTokens else { return }
        let view = ColorPickerView(tokens: tokens, color: color, allowsAlpha: allowsAlpha)
        view.setAccessibilityLabel(accessibilityLabel() ?? EditorColors.text("custom_color"))
        view.changed = { [weak self] color in
            guard let self else { return }
            self.color = color
            _ = self.sendAction(self.action, to: self.target)
        }
        view.dismiss = { [weak self] in self?.deactivate() }
        let size = view.frame.size
        let anchor = window.convertToScreen(convert(bounds, to: nil))
        let screen = (window.screen ?? NSScreen.main)?.visibleFrame ?? anchor.insetBy(dx: -400, dy: -400)
        // Below the well, or above it at the bottom of the screen, kept on screen.
        let gap = tokens.number("s-2"), margin = tokens.number("s-2")
        var origin = NSPoint(x: anchor.midX - size.width / 2, y: anchor.minY - gap - size.height)
        if origin.y < screen.minY + margin { origin.y = anchor.maxY + gap }
        origin.x = min(max(origin.x, screen.minX + margin), screen.maxX - margin - size.width)
        origin.y = min(max(origin.y, screen.minY + margin), screen.maxY - margin - size.height)
        let panel = NSPanel(contentRect: NSRect(origin: origin, size: size),
                            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false; panel.backgroundColor = .clear; panel.hasShadow = true
        panel.level = window.level
        let surface = TokenSelectSurfaceView(frame: NSRect(origin: .zero, size: size),
                                             fill: tokens.color("surface-overlay"),
                                             border: tokens.color("border"), radius: tokens.number("r-lg"))
        surface.addSubview(view)
        panel.contentView = surface
        window.addChildWindow(panel, ordered: .above)
        // Shipping popovers: ui-pop-in over --dur-2.
        NativeMotion.play("popover_in", on: surface, tokens: tokens)
        self.panel = panel; picker = view
        needsDisplay = true
        NSAccessibility.post(element: self, notification: .layoutChanged)
        // A click elsewhere closes the picker; a click on the well toggles it.
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: {
            [weak self] event in
            guard let self, let panel = self.panel, event.window !== panel else { return event }
            let onWell = event.window === self.window
                && self.bounds.contains(self.convert(event.locationInWindow, from: nil))
            self.deactivate()
            return onWell ? nil : event
        }) { monitors.append(monitor) }
        // The panel never becomes key, so the picker reads keys from here.
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: { [weak self] event in
            guard let picker = self?.picker else { return event }
            return picker.handleKey(event) ? nil : event
        }) { monitors.append(monitor) }
    }

    override func deactivate() {
        monitors.forEach(NSEvent.removeMonitor); monitors = []
        guard let panel else { return }
        panel.parent?.removeChildWindow(panel)
        panel.orderOut(nil)
        self.panel = nil; picker = nil
        needsDisplay = true
        NSAccessibility.post(element: self, notification: .layoutChanged)
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil { deactivate() }
        super.viewWillMove(toWindow: newWindow)
    }
}

/// The picker card's content (`editor_chrome::colors::picker`): a hue and
/// saturation wheel drawn at the current brightness, a brightness track and an
/// opacity track over a checkerboard. Each channel is an adjustable element;
/// Tab moves between them and the arrow keys step them.
final class ColorPickerView: NSView {
    enum Channel: CaseIterable { case hue, saturation, brightness, opacity }

    override var isFlipped: Bool { true }
    let tokens: Tokens
    let allowsAlpha: Bool
    private(set) var hue: CGFloat
    private(set) var saturation: CGFloat
    private(set) var brightness: CGFloat
    private(set) var alpha: CGFloat
    var changed: (NSColor) -> Void = { _ in }
    var dismiss: () -> Void = {}
    private(set) var focused: Channel = .hue
    private var showsFocus = false
    private var dragging: Channel?
    private var wheelCache: (key: [CGFloat], image: NSImage)?
    private lazy var elements: [ColorChannelElement] = channels.map { ColorChannelElement(picker: self, channel: $0) }

    static func metric(_ key: String) -> CGFloat { CGFloat((EditorColors.picker[key] as? NSNumber)?.doubleValue ?? 0) }

    /// `picker::width()` by `picker::height(alpha)`.
    static func size(allowsAlpha: Bool) -> NSSize {
        let padding = metric("padding"), wheel = metric("wheel")
        let rows = CGFloat(allowsAlpha ? 2 : 1) * (metric("gap") + metric("slider"))
        return NSSize(width: wheel + 2 * padding, height: 2 * padding + wheel + rows)
    }

    init(tokens: Tokens, color: NSColor, allowsAlpha: Bool) {
        self.tokens = tokens; self.allowsAlpha = allowsAlpha
        let rgb = color.usingColorSpace(.sRGB) ?? .black
        let hsv = Self.hsv(rgb.redComponent, rgb.greenComponent, rgb.blueComponent)
        hue = hsv.h; saturation = hsv.s; brightness = hsv.v
        alpha = allowsAlpha ? rgb.alphaComponent : 1
        super.init(frame: NSRect(origin: .zero, size: Self.size(allowsAlpha: allowsAlpha)))
        setAccessibilityElement(true); setAccessibilityRole(.group)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var channels: [Channel] { Channel.allCases.filter { allowsAlpha || $0 != .opacity } }

    /// The picked color; opaque unless `allowsAlpha`.
    var color: NSColor {
        let rgb = Self.rgb(hue, saturation, brightness)
        return NSColor(srgbRed: rgb.r, green: rgb.g, blue: rgb.b, alpha: alpha)
    }

    /// `Hsva::from_rgb`.
    static func hsv(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat) -> (h: CGFloat, s: CGFloat, v: CGFloat) {
        let top = max(r, g, b), delta = top - min(r, g, b)
        var turn: CGFloat = 0
        if delta > 0 {
            if top == r {
                let sector = (g - b) / delta
                turn = (sector < 0 ? sector + 6 : sector) / 6
            } else if top == g {
                turn = ((b - r) / delta + 2) / 6
            } else {
                turn = ((r - g) / delta + 4) / 6
            }
        }
        return (turn, top == 0 ? 0 : delta / top, top)
    }

    /// `Hsva::rgb`.
    static func rgb(_ h: CGFloat, _ s: CGFloat, _ v: CGFloat) -> (r: CGFloat, g: CGFloat, b: CGFloat) {
        let sector = (h - h.rounded(.down)) * 6
        let c = v * s, m = v - c
        let x = c * (1 - abs(sector.truncatingRemainder(dividingBy: 2) - 1))
        switch Int(sector) {
        case 0: return (c + m, x + m, m)
        case 1: return (x + m, c + m, m)
        case 2: return (m, c + m, x + m)
        case 3: return (m, x + m, c + m)
        case 4: return (x + m, m, c + m)
        default: return (c + m, m, x + m)
        }
    }

    // MARK: Geometry

    var wheelRect: NSRect {
        let padding = Self.metric("padding"), wheel = Self.metric("wheel")
        return NSRect(x: padding, y: padding, width: wheel, height: wheel)
    }

    /// Slider row `index` (0 brightness, 1 opacity): `picker::slider_top`.
    func sliderRow(_ index: Int) -> NSRect {
        let padding = Self.metric("padding"), wheel = Self.metric("wheel")
        let gap = Self.metric("gap"), slider = Self.metric("slider")
        return NSRect(x: padding, y: padding + wheel + gap + CGFloat(index) * (slider + gap),
                      width: wheel, height: slider)
    }

    /// The track the thumb's center travels: inset half a thumb at each end.
    func track(_ index: Int) -> NSRect {
        let row = sliderRow(index), height = Self.metric("track")
        return NSRect(x: row.minX + row.height / 2, y: row.midY - height / 2,
                      width: row.width - row.height, height: height)
    }

    func channelFrame(_ channel: Channel) -> NSRect {
        switch channel {
        case .hue, .saturation: return wheelRect
        case .brightness: return sliderRow(0)
        case .opacity: return sliderRow(1)
        }
    }

    // MARK: Values

    func value(of channel: Channel) -> CGFloat {
        switch channel {
        case .hue: return hue
        case .saturation: return saturation
        case .brightness: return brightness
        case .opacity: return alpha
        }
    }

    func valueText(of channel: Channel) -> String {
        channel == .hue ? "\(Int((hue * 360).rounded()))°" : "\(Int((value(of: channel) * 100).rounded()))%"
    }

    func label(of channel: Channel) -> String {
        let key: String
        switch channel {
        case .hue: key = "hue"
        case .saturation: key = "saturation"
        case .brightness: key = "brightness"
        case .opacity: key = "opacity"
        }
        return EditorColors.picker[key] as? String ?? ""
    }

    /// Step `channel` by `steps` keyboard steps (`picker::STEP` of its range).
    func adjust(_ channel: Channel, by steps: CGFloat) {
        let delta = steps * Self.metric("step"), clamp = { (value: CGFloat) in min(max(value, 0), 1) }
        switch channel {
        case .hue:
            let turned = hue + delta
            hue = turned - turned.rounded(.down)
        case .saturation: saturation = clamp(saturation + delta)
        case .brightness: brightness = clamp(brightness + delta)
        case .opacity: alpha = allowsAlpha ? clamp(alpha + delta) : 1
        }
        focused = channel
        publish(channel)
    }

    private func publish(_ channel: Channel) {
        needsDisplay = true
        changed(color)
        if let element = elements.first(where: { $0.channel == channel }) {
            NSAccessibility.post(element: element, notification: .valueChanged)
        }
    }

    // MARK: Input

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let wheel = wheelRect
        let reach = Self.metric("marker") / 2
        if hypot(point.x - wheel.midX, point.y - wheel.midY) <= wheel.width / 2 + reach {
            dragging = .hue
        } else if sliderRow(0).insetBy(dx: 0, dy: -reach).contains(point) {
            dragging = .brightness
        } else if allowsAlpha, sliderRow(1).insetBy(dx: 0, dy: -reach).contains(point) {
            dragging = .opacity
        } else {
            dragging = nil
        }
        showsFocus = false
        pick(point)
    }

    override func mouseDragged(with event: NSEvent) { pick(convert(event.locationInWindow, from: nil)) }
    override func mouseUp(with event: NSEvent) { dragging = nil }

    /// `Hsva::pick_wheel` and `Hsva::track_value` for the dragged channel.
    func pick(_ point: NSPoint) {
        guard let channel = dragging else { return }
        switch channel {
        case .hue, .saturation:
            let wheel = wheelRect, radius = wheel.width / 2
            let dx = point.x - wheel.midX, dy = point.y - wheel.midY
            let distance = hypot(dx, dy)
            if distance > 0 {
                let turn = atan2(-dy, dx) / (2 * .pi)
                hue = turn < 0 ? turn + 1 : turn
            }
            saturation = min(distance / radius, 1)
        case .brightness, .opacity:
            let track = self.track(channel == .brightness ? 0 : 1)
            let value = track.width > 0 ? min(max((point.x - track.minX) / track.width, 0), 1) : 0
            if channel == .brightness { brightness = value } else { alpha = value }
        }
        focused = channel
        publish(channel)
    }

    /// Keys while the picker is open, from its well's monitor: Escape and
    /// Return close it, Tab moves between channels, arrows step the focused one.
    func handleKey(_ event: NSEvent) -> Bool {
        switch event.keyCode {
        case 53, 36, 76:
            dismiss(); return true
        case 48:
            let list = channels, index = list.firstIndex(of: focused) ?? 0
            let step = event.modifierFlags.contains(.shift) ? list.count - 1 : 1
            focused = list[(index + step) % list.count]
            showsFocus = true; needsDisplay = true
            return true
        case 123, 125:
            showsFocus = true; adjust(focused, by: -1); return true
        case 124, 126:
            showsFocus = true; adjust(focused, by: 1); return true
        default:
            return false
        }
    }

    // MARK: Accessibility

    override func accessibilityChildren() -> [Any]? { elements }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        let wheel = wheelRect, radius = wheel.width / 2
        wheelImage(side: wheel.width).draw(in: wheel, from: .zero, operation: .sourceOver, fraction: 1,
                                           respectFlipped: true, hints: nil)
        let rim = NSBezierPath(ovalIn: wheel.insetBy(dx: 0.5, dy: 0.5))
        tokens.color("border").setStroke(); rim.lineWidth = 1; rim.stroke()
        let angle = hue * 2 * .pi, marker = Self.metric("marker")
        let center = NSPoint(x: wheel.midX + cos(angle) * saturation * radius,
                             y: wheel.midY - sin(angle) * saturation * radius)
        let ring = NSBezierPath(ovalIn: NSRect(x: center.x - marker / 2, y: center.y - marker / 2,
                                               width: marker, height: marker))
        color.withAlphaComponent(1).setFill(); ring.fill()
        // A white ring with a dark edge reads on every color, like the
        // fixed media palette over a wallpaper.
        NSColor.white.setStroke(); ring.lineWidth = 2; ring.stroke()
        let edge = NSBezierPath(ovalIn: ring.bounds.insetBy(dx: -1.5, dy: -1.5))
        NSColor.black.withAlphaComponent(0.35).setStroke(); edge.lineWidth = 1; edge.stroke()
        if showsFocus, focused == .hue || focused == .saturation {
            drawFocusRing(tokens, in: wheel.insetBy(dx: -3, dy: -3), radius: radius + 3)
        }

        let full = Self.rgb(hue, saturation, 1)
        if let gradient = NSGradient(starting: .black,
                                     ending: NSColor(srgbRed: full.r, green: full.g, blue: full.b, alpha: 1)) {
            drawTrack(0, gradient: gradient, checker: false, channel: .brightness)
        }
        if allowsAlpha {
            let opaque = color.withAlphaComponent(1)
            if let gradient = NSGradient(starting: opaque.withAlphaComponent(0), ending: opaque) {
                drawTrack(1, gradient: gradient, checker: true, channel: .opacity)
            }
        }
    }

    private func drawTrack(_ index: Int, gradient: NSGradient, checker: Bool, channel: Channel) {
        let track = self.track(index), row = sliderRow(index)
        let bar = NSRect(x: row.minX, y: track.minY, width: row.width, height: track.height)
        let pill = NSBezierPath(roundedRect: bar, xRadius: bar.height / 2, yRadius: bar.height / 2)
        if checker {
            // The usual transparency checkerboard, so opacity reads as see-through.
            NSGraphicsContext.saveGraphicsState()
            pill.addClip()
            NSColor.white.setFill(); bar.fill()
            NSColor(white: 0.8, alpha: 1).setFill()
            let cell = bar.height / 2
            var column = 0, x = bar.minX
            while x < bar.maxX {
                NSRect(x: x, y: bar.minY + (column % 2 == 0 ? 0 : cell), width: cell, height: cell).fill()
                x += cell; column += 1
            }
            NSGraphicsContext.restoreGraphicsState()
        }
        gradient.draw(in: pill, angle: 0)
        tokens.color("border").setStroke(); pill.lineWidth = 1; pill.stroke()
        // `TokenSliderCell`'s raised thumb.
        let thumb = row.height
        let knob = NSRect(x: track.minX + value(of: channel) * track.width - thumb / 2,
                          y: row.midY - thumb / 2, width: thumb, height: thumb)
        let circle = NSBezierPath(ovalIn: knob.insetBy(dx: 0.5, dy: 0.5))
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.12)
        shadow.shadowBlurRadius = 3; shadow.shadowOffset = NSSize(width: 0, height: -1)
        shadow.set()
        tokens.color("surface-raised").setFill(); circle.fill()
        NSGraphicsContext.restoreGraphicsState()
        tokens.color("border-strong").setStroke(); circle.lineWidth = 1; circle.stroke()
        if showsFocus, focused == channel {
            drawFocusRing(tokens, in: knob.insetBy(dx: -3, dy: -3), radius: thumb / 2 + 2)
        }
    }

    /// The wheel at the current brightness, antialiased at the rim; rebuilt
    /// only when the brightness or backing scale changes.
    private func wheelImage(side: CGFloat) -> NSImage {
        let scale = window?.backingScaleFactor ?? 2
        let key = [side, scale, brightness]
        if let cache = wheelCache, cache.key == key { return cache.image }
        let pixels = max(1, Int((side * scale).rounded()))
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
                                         bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                         colorSpaceName: .deviceRGB, bytesPerRow: pixels * 4, bitsPerPixel: 32),
              let data = rep.bitmapData else { return NSImage() }
        let radius = CGFloat(pixels) / 2
        for y in 0..<pixels {
            for x in 0..<pixels {
                // Rows run top down, matching `pick_wheel`'s y-down offsets.
                let dx = CGFloat(x) + 0.5 - radius, dy = CGFloat(y) + 0.5 - radius
                let distance = hypot(dx, dy)
                let coverage = min(max(radius - distance + 0.5, 0), 1)
                let index = (y * pixels + x) * 4
                guard coverage > 0 else {
                    data[index] = 0; data[index + 1] = 0; data[index + 2] = 0; data[index + 3] = 0
                    continue
                }
                let turn = atan2(-dy, dx) / (2 * .pi)
                let rgb = Self.rgb(turn < 0 ? turn + 1 : turn, min(distance / radius, 1), brightness)
                // Premultiplied, as the bitmap format expects.
                data[index] = UInt8((rgb.r * coverage * 255).rounded())
                data[index + 1] = UInt8((rgb.g * coverage * 255).rounded())
                data[index + 2] = UInt8((rgb.b * coverage * 255).rounded())
                data[index + 3] = UInt8((coverage * 255).rounded())
            }
        }
        let tagged = rep.retagging(with: .sRGB) ?? rep
        let image = NSImage(size: NSSize(width: side, height: side))
        image.addRepresentation(tagged)
        wheelCache = (key, image)
        return image
    }
}

/// One picker channel as a VoiceOver slider.
final class ColorChannelElement: NSAccessibilityElement {
    weak var picker: ColorPickerView?
    let channel: ColorPickerView.Channel

    init(picker: ColorPickerView, channel: ColorPickerView.Channel) {
        self.picker = picker; self.channel = channel
        super.init()
        setAccessibilityRole(.slider)
        setAccessibilityLabel(picker.label(of: channel))
        setAccessibilityParent(picker)
    }

    override func accessibilityValue() -> Any? { picker?.valueText(of: channel) }
    override func accessibilityFrame() -> NSRect {
        guard let picker else { return .zero }
        return NSAccessibility.screenRect(fromView: picker, rect: picker.channelFrame(channel))
    }
    override func accessibilityPerformIncrement() -> Bool { picker?.adjust(channel, by: 1); return picker != nil }
    override func accessibilityPerformDecrement() -> Bool { picker?.adjust(channel, by: -1); return picker != nil }
}

/// The shipping custom-color tile (`.screenshot-custom-color`): a conic hue
/// square that opens the system color panel, like the shipping `<input type=color>`.
final class CustomColorWell: PickerColorWell {
    var tokens: Tokens { didSet { pickerTokens = tokens; needsDisplay = true } }
    var change: ((NSColor) -> Void)?

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        pickerTokens = tokens
        isContinuous = true
        target = self; action = #selector(customColorChanged)
        setAccessibilityLabel(EditorColors.text("custom_color"))
        toolTip = EditorColors.text("custom_color")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func resetCursorRects() {
        if isEnabled { addCursorRect(bounds, cursor: .pointingHand) }
    }

    @objc private func customColorChanged() { change?(color) }

    private var tile: NSRect {
        let side = EditorColors.metric("tile")
        return NSRect(x: bounds.midX - side / 2, y: bounds.midY - side / 2, width: side, height: side)
    }

    /// `conic-gradient(#f44, #ff4, #4f4, #4ff, #44f, #f4f, #f44)` at `turn`
    /// (0 = top, clockwise).
    static func conic(_ turn: CGFloat) -> NSColor {
        let stops: [[CGFloat]] = [[1, 0.267, 0.267], [1, 1, 0.267], [0.267, 1, 0.267],
                                  [0.267, 1, 1], [0.267, 0.267, 1], [1, 0.267, 1], [1, 0.267, 0.267]]
        let wrapped = turn - turn.rounded(.down)
        let position = wrapped * 6
        let index = min(5, Int(position.rounded(.down)))
        let t = position - CGFloat(index)
        let a = stops[index], b = stops[index + 1]
        return NSColor(srgbRed: a[0] + (b[0] - a[0]) * t, green: a[1] + (b[1] - a[1]) * t,
                       blue: a[2] + (b[2] - a[2]) * t, alpha: 1)
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-sm")
        let outline = NSBezierPath(roundedRect: tile, xRadius: radius, yRadius: radius)
        NSGraphicsContext.saveGraphicsState()
        outline.addClip()
        let center = NSPoint(x: tile.midX, y: tile.midY)
        let wedges = 36
        let reach = tile.width
        for index in 0..<wedges {
            let start = CGFloat(index) / CGFloat(wedges)
            let end = CGFloat(index + 1) / CGFloat(wedges)
            let wedge = NSBezierPath()
            wedge.move(to: center)
            // Top-origin clockwise turns; `isFlipped` views grow y downward.
            for turn in [start, end] {
                let angle = turn * 2 * .pi
                let dy = isFlipped ? -cos(angle) : cos(angle)
                wedge.line(to: NSPoint(x: center.x + sin(angle) * reach, y: center.y + dy * reach))
            }
            wedge.close()
            Self.conic((start + end) / 2).withAlphaComponent(isEnabled ? 1 : 0.4).setFill()
            wedge.fill()
        }
        NSGraphicsContext.restoreGraphicsState()
        let inset = EditorColors.metric("custom_inset")
        let inner = NSBezierPath(roundedRect: tile.insetBy(dx: inset, dy: inset), xRadius: 3, yRadius: 3)
        tokens.color("surface-raised").setFill(); inner.fill()
        tokens.color("border-strong").setStroke()
        inner.lineWidth = 1; inner.stroke()
        let border = NSBezierPath(roundedRect: tile.insetBy(dx: 0.5, dy: 0.5),
                                  xRadius: radius, yRadius: radius)
        border.lineWidth = 1; border.stroke()
    }

    override func drawFocusRingMask() {
        let radius = tokens.number("r-sm")
        NSBezierPath(roundedRect: tile, xRadius: radius, yRadius: radius).fill()
    }
    override var focusRingMaskBounds: NSRect { tile }
}

/// Shipping `ColorField`: eight swatches then the custom-color tile, laid out on
/// the CSS auto-fill grid. `compact` is the canvas background card's grid.
final class ColorSwatchRow: NSView {
    override var isFlipped: Bool { true }
    private(set) var fieldLabel: String
    let compact: Bool
    private(set) var swatchButtons: [ColorSwatchButton] = []
    let customWell: CustomColorWell
    var changed: (String) -> Void = { _ in }
    var tokens: Tokens {
        didSet {
            swatchButtons.forEach { $0.tokens = tokens }
            customWell.tokens = tokens
        }
    }
    /// The value the active swatch and custom seed follow; not an edit.
    var selectedHex = "" {
        didSet { publishSelection() }
    }
    var isEnabled = true {
        didSet {
            swatchButtons.forEach { $0.isEnabled = isEnabled }
            customWell.isEnabled = isEnabled
            if !isEnabled { deactivate() }
        }
    }

    /// `allowsAlpha` adds the picker's opacity; shadow colors have their own.
    init(tokens: Tokens, label: String, compact: Bool, allowsAlpha: Bool = true) {
        self.tokens = tokens; fieldLabel = label; self.compact = compact
        customWell = CustomColorWell(tokens: tokens)
        customWell.allowsAlpha = allowsAlpha
        super.init(frame: NSRect(x: 0, y: 0, width: 248, height: 0))
        setAccessibilityElement(true)
        setAccessibilityRole(.radioGroup); setAccessibilityLabel(label)
        for swatch in EditorColors.swatches {
            let button = ColorSwatchButton(swatchHex: swatch,
                                           label: EditorColors.swatchLabel(label, swatch), tokens: tokens)
            button.pressed = { [weak self] in self?.choose(swatch) }
            swatchButtons.append(button); addSubview(button)
        }
        customWell.change = { [weak self] color in
            if let value = EditorColors.hex(color, alpha: allowsAlpha) { self?.choose(value) }
        }
        addSubview(customWell)
        setFrameSize(NSSize(width: frame.width,
                            height: Self.height(width: frame.width, compact: compact, tokens: tokens)))
        publishSelection()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private static func spacing(compact: Bool, tokens: Tokens) -> (rowGap: CGFloat, padding: CGFloat) {
        compact ? (tokens.number("s-3"), tokens.number("s-2")) : (tokens.number("s-4"), tokens.number("s-3"))
    }

    static func grid(width: CGFloat, compact: Bool) -> EditorColors.Grid {
        EditorColors.Grid(width: width,
                          minCell: EditorColors.metric(compact ? "compact_cell" : "cell"),
                          count: EditorColors.swatches.count + 1)
    }

    static func height(width: CGFloat, compact: Bool, tokens: Tokens) -> CGFloat {
        let (rowGap, padding) = spacing(compact: compact, tokens: tokens)
        return grid(width: width, compact: compact)
            .height(tile: EditorColors.metric("tile"), rowGap: rowGap, padding: padding)
    }

    private func choose(_ value: String) {
        guard isEnabled else { return }
        selectedHex = value
        changed(value)
    }

    private func publishSelection() {
        for button in swatchButtons {
            button.active = EditorColors.swatchActive(selectedHex, button.swatchHex)
        }
        if !customWell.isActive,
           let seed = NSColor(hex: customWell.allowsAlpha ? selectedHex : String(selectedHex.prefix(7))) {
            customWell.color = seed
        }
    }

    /// Close the custom-color picker if it is open.
    func deactivate() { customWell.deactivate() }

    /// Rename the field (shipping "Stroke color" vs "Color" for open tools)
    /// and each swatch's accessible name.
    func relabel(_ label: String) {
        guard label != fieldLabel else { return }
        fieldLabel = label
        setAccessibilityLabel(label)
        for button in swatchButtons {
            button.setAccessibilityLabel(EditorColors.swatchLabel(label, button.swatchHex))
        }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        layoutTiles()
    }

    /// Frames depend only on the width and tokens, never on fonts.
    func layoutTiles() {
        let (rowGap, padding) = Self.spacing(compact: compact, tokens: tokens)
        let tile = EditorColors.metric("tile")
        let grid = Self.grid(width: bounds.width, compact: compact)
        let hit = tile + 8 // Room for the active ring and focus ring.
        let tiles: [NSView] = swatchButtons + [customWell]
        for (index, view) in tiles.enumerated() {
            let center = grid.center(index, tile: tile, rowGap: rowGap, padding: padding)
            view.frame = NSRect(x: center.x - hit / 2, y: center.y - hit / 2, width: hit, height: hit)
        }
    }
}

/// Shipping `.screenshot-text-style-preview` chips for text style menus.
enum TextStyleChip {
    static let size = NSSize(width: 72, height: 24)

    static func font(for preset: NativeTextPreset) -> NSFont {
        switch preset.fontFamily {
        case "mono":
            return .monospacedSystemFont(ofSize: 15, weight: .semibold)
        case "rounded":
            let base = NSFont.systemFont(ofSize: 17, weight: .bold)
            guard let rounded = base.fontDescriptor.withDesign(.rounded) else { return base }
            return NSFont(descriptor: rounded, size: 17) ?? base
        default:
            return .systemFont(ofSize: 17, weight: .bold)
        }
    }

    /// The "Text" preview drawn with `tokens`; box styles use `--solid`.
    static func image(for preset: NativeTextPreset, tokens: Tokens) -> NSImage {
        let solid = tokens.color("solid"), solidInk = tokens.color("solid-ink")
        let ink = tokens.color("text"), radius = tokens.number("r-lg")
        let image = NSImage(size: size, flipped: false) { rect in
            let boxed = preset.background != nil
            if boxed {
                let corner = preset.roundedBackground ? radius : 0
                solid.setFill()
                NSBezierPath(roundedRect: rect, xRadius: corner, yRadius: corner).fill()
            }
            var attributes: [NSAttributedString.Key: Any] = [
                .font: TextStyleChip.font(for: preset), .foregroundColor: boxed ? solidInk : ink,
            ]
            if preset.outlined {
                // `-webkit-text-stroke: 1.2px` on a transparent fill; positive widths
                // stroke without filling, as a percentage of the font size.
                attributes[.strokeColor] = ink
                attributes[.strokeWidth] = 7.0
            }
            let text = NSAttributedString(string: "Text", attributes: attributes)
            let measured = text.size()
            text.draw(at: NSPoint(x: rect.midX - measured.width / 2, y: rect.midY - measured.height / 2))
            return true
        }
        image.accessibilityDescription = preset.label
        return image
    }
}

extension ColorSwatchButton: TokenRestyling { func restyle(tokens: Tokens) { self.tokens = tokens } }
extension CustomColorWell: TokenRestyling { func restyle(tokens: Tokens) { self.tokens = tokens } }
extension ColorSwatchRow: TokenRestyling { func restyle(tokens: Tokens) { self.tokens = tokens } }
