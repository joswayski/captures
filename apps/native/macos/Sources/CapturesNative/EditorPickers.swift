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

    private var circle: NSRect {
        let side = EditorColors.metric("tile")
        return NSRect(x: bounds.midX - side / 2, y: bounds.midY - side / 2, width: side, height: side)
    }

    override func draw(_ dirtyRect: NSRect) {
        let alpha: CGFloat = isEnabled ? 1 : 0.4
        if active {
            // `box-shadow: 0 0 0 2px var(--editor-panel), 0 0 0 4px var(--theme-accent)`.
            tokens.color("theme-accent").withAlphaComponent(alpha).setFill()
            NSBezierPath(ovalIn: circle.insetBy(dx: -4, dy: -4)).fill()
            tokens.color("surface-raised").setFill()
            NSBezierPath(ovalIn: circle.insetBy(dx: -2, dy: -2)).fill()
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

/// The shipping custom-color tile (`.screenshot-custom-color`): a conic hue
/// square that opens the system color panel, like the shipping `<input type=color>`.
final class CustomColorWell: NSColorWell {
    var tokens: Tokens { didSet { needsDisplay = true } }
    var change: ((NSColor) -> Void)?

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        isContinuous = true
        target = self; action = #selector(customColorChanged)
        setAccessibilityLabel(EditorColors.text("custom_color"))
        toolTip = EditorColors.text("custom_color")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

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
    let fieldLabel: String
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
            if !isEnabled { customWell.deactivate() }
        }
    }

    init(tokens: Tokens, label: String, compact: Bool) {
        self.tokens = tokens; fieldLabel = label; self.compact = compact
        customWell = CustomColorWell(tokens: tokens)
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
            if let value = color.rgbHex?.lowercased() { self?.choose(value) }
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
        if !customWell.isActive, let seed = NSColor(hex: String(selectedHex.prefix(7))) {
            customWell.color = seed
        }
    }

    func deactivate() { customWell.deactivate() }

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
