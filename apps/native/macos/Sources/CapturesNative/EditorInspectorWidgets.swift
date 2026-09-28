import AppKit
import CoreText

/// Shipping Eraser slider copy (`editor_chrome::eraser`) and text format
/// button copy (`editor_chrome::text_format`), shared with the wgpu host
/// through the editor chrome ABI.
enum EditorInspectorCopy {
    static func eraser(_ key: String) -> String { EditorChrome.text("eraser", key) }
    static func textFormat(_ key: String) -> String { EditorChrome.text("text_format", key) }

    private static var eraserGroup: [String: Any] { EditorChrome.copy["eraser"] as? [String: Any] ?? [:] }

    static func eraserRange(_ key: String, fallback: ClosedRange<Double>) -> ClosedRange<Double> {
        let values = (eraserGroup[key] as? [NSNumber])?.map(\.doubleValue) ?? []
        guard values.count == 2, values[0].isFinite, values[1].isFinite, values[0] < values[1] else {
            return fallback
        }
        return values[0]...values[1]
    }

    static func eraserMarks(_ key: String) -> [EditorMarkedSlider.Mark] {
        (eraserGroup[key] as? [[String: Any]] ?? []).compactMap { item in
            guard let value = (item["value"] as? NSNumber)?.doubleValue,
                  let label = item["label"] as? String else { return nil }
            return EditorMarkedSlider.Mark(value: value, label: label)
        }
    }

    /// `(value, accessible name, icon)` for the three alignment buttons.
    static var alignments: [(value: String, label: String, icon: String)] {
        let group = EditorChrome.copy["text_format"] as? [String: Any] ?? [:]
        let parsed: [(value: String, label: String, icon: String)] =
            (group["align"] as? [[String: Any]] ?? []).compactMap { item in
                guard let value = item["value"] as? String, let label = item["label"] as? String,
                      let icon = item["icon"] as? String else { return nil }
                return (value: value, label: label, icon: icon)
            }
        if parsed.count == 3 { return parsed }
        return [(value: "left", label: "Align left", icon: "align-left"),
                (value: "center", label: "Align center", icon: "align-center"),
                (value: "right", label: "Align right", icon: "align-right")]
    }
}

/// Shipping `<label>Name<RangeSlider marks/></label>`: the visible name, a
/// right-aligned readout, the token slider, tick dots and mark labels (the
/// first left-aligned, the last right-aligned). Frames depend only on the
/// width, never on fonts.
final class EditorMarkedSlider: NSView {
    struct Mark: Equatable {
        let value: Double
        let label: String
    }

    override var isFlipped: Bool { true }
    static let height: CGFloat = 64
    /// A slider without marks: the name and readout over the track.
    static let plainHeight: CGFloat = 44
    private static let thumb: CGFloat = 14
    private static let markWidth: CGFloat = 64
    let slider: TokenSlider
    let titleLabel: NSTextField
    let readout = NSTextField(labelWithString: "")
    private(set) var markLabels: [NSTextField] = []
    private let marks: [Mark]
    private let describe: (Double) -> String
    var changed: (Double) -> Void = { _ in }
    var tokens: Tokens { didSet { restyle() } }

    /// The whole-number value shown by the slider.
    var value: Double {
        get { slider.doubleValue.rounded() }
        set {
            slider.doubleValue = min(slider.maxValue, max(slider.minValue, newValue.rounded()))
            readout.stringValue = describe(value)
        }
    }

    var isEnabled: Bool {
        get { slider.isEnabled }
        set { slider.isEnabled = newValue; alphaValue = newValue ? 1 : 0.45 }
    }

    init(tokens: Tokens, title: String, accessibilityLabel: String, range: ClosedRange<Double>,
         value: Double, marks: [Mark], describe: @escaping (Double) -> String) {
        self.tokens = tokens; self.marks = marks; self.describe = describe
        slider = TokenSlider(value: value, minValue: range.lowerBound, maxValue: range.upperBound,
                             target: nil, action: nil)
        titleLabel = NSTextField(labelWithString: title)
        super.init(frame: NSRect(x: 0, y: 0, width: 252, height: Self.height))
        titleLabel.setAccessibilityElement(false)
        addSubview(titleLabel)
        readout.alignment = .right
        readout.setAccessibilityElement(false)
        addSubview(readout)
        slider.tokens = tokens
        slider.isContinuous = true
        slider.target = self; slider.action = #selector(sliderMoved)
        slider.setAccessibilityLabel(accessibilityLabel)
        addSubview(slider)
        for (index, mark) in marks.enumerated() {
            let label = NSTextField(labelWithString: mark.label)
            label.alignment = index == 0 ? .left : index == marks.count - 1 ? .right : .center
            label.setAccessibilityElement(false)
            addSubview(label); markLabels.append(label)
        }
        self.value = value
        restyle()
        layoutParts()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        layoutParts()
    }

    /// The thumb centre for `value`, inset by half the thumb like the track.
    func markX(_ value: Double) -> CGFloat {
        let span = slider.maxValue - slider.minValue
        let fraction = span > 0 ? min(1, max(0, (value - slider.minValue) / span)) : 0
        return slider.frame.minX + Self.thumb / 2 + CGFloat(fraction) * max(0, slider.frame.width - Self.thumb)
    }

    private func layoutParts() {
        let width = bounds.width
        titleLabel.frame = NSRect(x: 0, y: 0, width: max(0, width - 80), height: 18)
        readout.frame = NSRect(x: max(0, width - 76), y: 0, width: 76, height: 18)
        slider.frame = NSRect(x: 0, y: 20, width: width, height: 24)
        for (index, (mark, label)) in zip(marks, markLabels).enumerated() {
            let center = markX(mark.value)
            let x: CGFloat
            if index == 0 {
                x = center
            } else if index == marks.count - 1 {
                x = center - Self.markWidth
            } else {
                x = center - Self.markWidth / 2
            }
            label.frame = NSRect(x: x, y: 46, width: Self.markWidth, height: 14)
        }
        needsDisplay = true
    }

    private func restyle() {
        titleLabel.font = .systemFont(ofSize: tokens.number("text-sm"))
        titleLabel.textColor = tokens.color("text-muted")
        readout.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        readout.textColor = tokens.color("text-subtle")
        for label in markLabels {
            label.font = .systemFont(ofSize: tokens.number("text-2xs"), weight: .medium)
            label.textColor = tokens.color("text-faint")
        }
        slider.tokens = tokens
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        // `.range-slider-ticks i`: 2 pt dots in `--border-strong` on the track.
        tokens.color("border-strong").setFill()
        for mark in marks {
            let center = NSPoint(x: markX(mark.value), y: slider.frame.midY)
            NSBezierPath(ovalIn: NSRect(x: center.x - 1, y: center.y - 1, width: 2, height: 2)).fill()
        }
    }

    @objc private func sliderMoved() {
        let rounded = value
        slider.doubleValue = rounded
        readout.stringValue = describe(rounded)
        changed(rounded)
    }
}

/// One shipping text format button (`.screenshot-format-buttons button`):
/// B, I or an alignment icon; `active` fills it with the accent.
final class EditorFormatButton: NSButton {
    enum Glyph: Equatable {
        case bold
        case italic
        case icon(String)
    }

    override var isFlipped: Bool { true }
    let glyph: Glyph
    var tokens: Tokens { didSet { needsDisplay = true } }
    var active = false {
        didSet { needsDisplay = true; setAccessibilityValue(active) }
    }
    var pressed: (() -> Void)?
    /// Icon side in points; nil uses the text format copy's side.
    var iconSide: CGFloat? { didSet { needsDisplay = true } }
    /// Shipping `.screenshot-tool-flyout button`: transparent until hovered,
    /// `--r-md` corners and no border.
    var borderless = false { didSet { needsDisplay = true } }
    private var pointerInside = false
    private var pointerTracking: NSTrackingArea?

    init(glyph: Glyph, label: String, tokens: Tokens) {
        self.glyph = glyph; self.tokens = tokens
        super.init(frame: .zero)
        title = ""; isBordered = false; setButtonType(.momentaryChange)
        focusRingType = .exterior
        target = self; action = #selector(activateFormat)
        setAccessibilityRole(.checkBox); setAccessibilityLabel(label)
        setAccessibilityValue(false)
        toolTip = label
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    @objc private func activateFormat() { pressed?() }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let pointerTracking { removeTrackingArea(pointerTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking); pointerTracking = tracking
    }
    override func mouseEntered(with event: NSEvent) { pointerInside = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { pointerInside = false; needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        let alpha: CGFloat = isEnabled ? 1 : 0.45
        let hovered = isEnabled && pointerInside
        let radius = tokens.number(borderless ? "r-md" : "r-sm")
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        let ink: NSColor
        if active {
            tokens.color("theme-accent").withAlphaComponent(alpha).setFill(); shape.fill()
            tokens.color("theme-accent").withAlphaComponent(alpha).setStroke()
            ink = tokens.color("theme-accent-ink").withAlphaComponent(alpha)
            shape.lineWidth = 1; shape.stroke()
        } else if borderless {
            if hovered { tokens.color("surface-hover").setFill(); shape.fill() }
            ink = tokens.color(hovered ? "text" : "text-muted").withAlphaComponent(alpha)
        } else {
            tokens.color(hovered ? "control-hover" : "control").withAlphaComponent(alpha).setFill(); shape.fill()
            tokens.color("border-subtle").withAlphaComponent(alpha).setStroke()
            ink = tokens.color(hovered ? "text" : "text-muted").withAlphaComponent(alpha)
            shape.lineWidth = 1; shape.stroke()
        }
        switch glyph {
        case .bold, .italic:
            var font = NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .semibold)
            if glyph == .italic { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
            let text = NSAttributedString(string: glyph == .bold ? "B" : "I",
                                          attributes: [.font: font, .foregroundColor: ink])
            let size = text.size()
            text.draw(at: NSPoint(x: bounds.midX - size.width / 2, y: bounds.midY - size.height / 2))
        case .icon(let name):
            let side = iconSide
                ?? CGFloat((EditorChrome.copy["text_format"] as? [String: Any])?["icon"] as? Double ?? 14)
            ink.setStroke()
            ShippingIcons.stroke(name, in: NSRect(x: bounds.midX - side / 2, y: bounds.midY - side / 2,
                                                  width: side, height: side))
        }
    }

    override func drawFocusRingMask() {
        let radius = tokens.number("r-sm")
        NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
    }
    override var focusRingMaskBounds: NSRect { bounds }
}

/// Shipping `.screenshot-format-buttons`: Bold, Italic and the three
/// alignment buttons in five equal columns separated by `--s-2`.
final class EditorTextFormatButtons: NSView {
    override var isFlipped: Bool { true }
    let boldButton: EditorFormatButton
    let italicButton: EditorFormatButton
    let alignButtons: [(value: String, button: EditorFormatButton)]
    /// A button changed `bold`, `italic` or `align`.
    var changed: () -> Void = {}
    var tokens: Tokens {
        didSet { allButtons.forEach { $0.tokens = tokens }; layoutButtons() }
    }
    var bold = false { didSet { boldButton.active = bold } }
    var italic = false { didSet { italicButton.active = italic } }
    var align = "left" {
        didSet { alignButtons.forEach { $0.button.active = $0.value == align } }
    }
    var isEnabled = true {
        didSet { allButtons.forEach { $0.isEnabled = isEnabled; $0.needsDisplay = true } }
    }
    private var allButtons: [EditorFormatButton] { [boldButton, italicButton] + alignButtons.map(\.button) }

    init(tokens: Tokens) {
        self.tokens = tokens
        boldButton = EditorFormatButton(glyph: .bold, label: EditorInspectorCopy.textFormat("bold"), tokens: tokens)
        italicButton = EditorFormatButton(glyph: .italic, label: EditorInspectorCopy.textFormat("italic"),
                                          tokens: tokens)
        alignButtons = EditorInspectorCopy.alignments.map { item in
            (value: item.value, button: EditorFormatButton(glyph: .icon(item.icon), label: item.label,
                                                           tokens: tokens))
        }
        super.init(frame: NSRect(x: 0, y: 0, width: 252, height: 32))
        setAccessibilityElement(true)
        setAccessibilityRole(.group); setAccessibilityLabel("Text format")
        boldButton.pressed = { [weak self] in
            guard let self, self.isEnabled else { return }
            self.bold.toggle(); self.changed()
        }
        italicButton.pressed = { [weak self] in
            guard let self, self.isEnabled else { return }
            self.italic.toggle(); self.changed()
        }
        for (value, button) in alignButtons {
            button.pressed = { [weak self] in
                guard let self, self.isEnabled, self.align != value else { return }
                self.align = value; self.changed()
            }
        }
        allButtons.forEach { addSubview($0) }
        // Observers do not run during init; show the default alignment.
        for (value, button) in alignButtons { button.active = value == align }
        layoutButtons()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        layoutButtons()
    }

    private func layoutButtons() {
        let gap = tokens.number("s-2")
        let buttons = allButtons
        let count = CGFloat(buttons.count)
        let width = max(0, (bounds.width - gap * (count - 1)) / count)
        for (index, button) in buttons.enumerated() {
            button.frame = NSRect(x: CGFloat(index) * (width + gap), y: 0, width: width, height: bounds.height)
        }
    }
}

/// Bundled editor faces (`captures_app::editor_fonts`) registered with Core
/// Text, so the inline editor draws in the layer's own face. A draft's own
/// font that this build does not bundle falls back to the system face.
enum EditorTextFaces {
    private static var cache: [String: CTFontDescriptor] = [:]
    private static var missing: Set<String> = []

    /// `name` is the draft's font name for `family`; a draft that pins its own
    /// font under a bundled key does not get the bundled face.
    static func font(family: String, name: String?, bold: Bool, italic: Bool, size: CGFloat) -> NSFont {
        let key = "\(family):\(name ?? ""):\(bold):\(italic)"
        if cache[key] == nil, !missing.contains(key) {
            var request: [String: Any] = ["operation": "text_face", "family": family, "bold": bold, "italic": italic]
            if let name { request["name"] = name }
            if let encoded = EditorChrome.request(request) as? String,
               let data = Data(base64Encoded: encoded),
               let descriptor = CTFontManagerCreateFontDescriptorFromData(data as CFData) {
                cache[key] = descriptor
            } else {
                missing.insert(key)
            }
        }
        if let descriptor = cache[key] {
            return CTFontCreateWithFontDescriptor(descriptor, size, nil) as NSFont
        }
        var font = NSFont.systemFont(ofSize: size, weight: bold ? .bold : .regular)
        if italic { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
        return font
    }
}

/// Shipping `.screenshot-inline-text-frame`: the canvas text box drawn in
/// the layer's own plate, with a 1 pt accent outline `--s-3` outside it
/// (`::after`). The host rotates the whole frame about its centre.
final class EditorInlineTextFrame: NSView {
    override var isFlipped: Bool { true }
    var tokens: Tokens { didSet { needsDisplay = true } }
    var plateColor: NSColor? { didSet { needsDisplay = true } }
    var plateRadius: CGFloat = 0 { didSet { needsDisplay = true } }
    /// How far the outline sits outside the text frame.
    var outset: CGFloat { tokens.number("s-3") }

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(false)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        let textFrame = bounds.insetBy(dx: outset, dy: outset)
        if let plateColor, textFrame.width > 0, textFrame.height > 0 {
            plateColor.setFill()
            NSBezierPath(roundedRect: textFrame, xRadius: plateRadius, yRadius: plateRadius).fill()
        }
        let radius = tokens.number("r-sm")
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        tokens.color("theme-accent").setStroke()
        outline.lineWidth = 1; outline.stroke()
    }
}

/// Where the inline editor draws, from `captures_editor_chrome_v1`
/// `inline_text_layout` (shared with the wgpu host), in document units.
struct NativeInlineTextLayout: Equatable {
    let frame: NSRect
    /// Top, right, bottom, left.
    let padding: [CGFloat]
    let rotation: CGFloat
    let lineHeight: CGFloat
    let plateRadius: CGFloat
    let outlineWidth: CGFloat
    let autoWidth: Bool
    let fontSize: CGFloat
    let fontFamily: String
    let bold: Bool
    let italic: Bool
    let align: String
    let color: String
    let background: String?
    let outlined: Bool
    let opacity: CGFloat

    /// `element` is a document text element (`documentJSON`); `create` the
    /// `TextCreate` a Text click sends. Pass exactly one.
    static func resolve(element: [String: Any]? = nil, create: [String: Any]? = nil) -> NativeInlineTextLayout? {
        var request: [String: Any] = ["operation": "inline_text_layout"]
        if let element { request["element"] = element }
        if let create { request["create"] = create }
        guard let result = EditorChrome.request(request) as? [String: Any],
              let element = result["element"] as? [String: Any],
              let layout = result["layout"] as? [String: Any] else { return nil }
        return NativeInlineTextLayout(element: element, layout: layout)
    }

    init?(element: [String: Any], layout: [String: Any]) {
        func number(_ object: [String: Any], _ key: String) -> CGFloat? {
            guard let value = (object[key] as? NSNumber)?.doubleValue, value.isFinite else { return nil }
            return CGFloat(value)
        }
        guard let frame = layout["frame"] as? [String: Any],
              let x = number(frame, "x"), let y = number(frame, "y"),
              let width = number(frame, "width"), let height = number(frame, "height"),
              let padding = (layout["padding"] as? [NSNumber])?.map({ CGFloat($0.doubleValue) }),
              padding.count == 4,
              let rotation = number(layout, "rotation"), let lineHeight = number(layout, "line_height"),
              let plateRadius = number(layout, "plate_radius"),
              let outlineWidth = number(layout, "outline_width"),
              let autoWidth = layout["auto_width"] as? Bool,
              let fontSize = number(element, "fontSize"),
              let fontFamily = element["fontFamily"] as? String,
              let color = element["color"] as? String else { return nil }
        self.frame = NSRect(x: x, y: y, width: width, height: height)
        self.padding = padding; self.rotation = rotation; self.lineHeight = lineHeight
        self.plateRadius = plateRadius; self.outlineWidth = outlineWidth; self.autoWidth = autoWidth
        self.fontSize = fontSize; self.fontFamily = fontFamily
        bold = element["bold"] as? Bool ?? false
        italic = element["italic"] as? Bool ?? false
        align = element["align"] as? String ?? "left"
        self.color = color
        background = (element["background"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        outlined = element["outlined"] as? Bool ?? false
        opacity = number(element, "opacity") ?? 100
    }
}

/// Shipping grouped shapes (`SHAPE_GROUP_ITEMS`): the Properties
/// `.screenshot-shape-picker` (three columns of 40 pt buttons, `--s-2` apart)
/// or, with `flyout`, the rail's `.screenshot-tool-flyout` (three 44 pt
/// columns in a raised card with `--s-3` padding). The current shape is
/// accent-filled; choosing one reports its key. Frames never depend on fonts.
final class EditorShapePicker: NSView {
    override var isFlipped: Bool { true }
    let flyout: Bool
    private(set) var buttons: [(key: String, button: EditorFormatButton)] = []
    /// The shape key the user chose.
    var chosen: (String) -> Void = { _ in }
    var tokens: Tokens {
        didSet { buttons.forEach { $0.button.tokens = tokens }; restyle(); layoutButtons() }
    }
    /// The active shape key.
    var current = "" {
        didSet { buttons.forEach { $0.button.active = $0.key == current } }
    }
    var isEnabled = true {
        didSet { buttons.forEach { $0.button.isEnabled = isEnabled; $0.button.needsDisplay = true } }
    }

    /// Picker rows are 40 pt; flyout buttons are 44 pt squares.
    var side: CGFloat { flyout ? 44 : 40 }
    var padding: CGFloat { flyout ? tokens.number("s-3") : 0 }

    init(tokens: Tokens, shapes: [EditorChrome.Tool], flyout: Bool) {
        self.tokens = tokens; self.flyout = flyout
        super.init(frame: NSRect(x: 0, y: 0, width: 252, height: 0))
        wantsLayer = true
        setAccessibilityElement(true)
        setAccessibilityRole(.group); setAccessibilityLabel(flyout ? "Shapes" : "Shape")
        for shape in shapes {
            let button = EditorFormatButton(glyph: .icon(shape.icon), label: shape.label, tokens: tokens)
            button.toolTip = shape.name
            button.iconSide = flyout ? 22 : 18
            button.borderless = flyout
            let key = shape.key
            button.pressed = { [weak self] in
                guard let self, self.isEnabled else { return }
                self.current = key
                self.chosen(key)
            }
            buttons.append((key: key, button: button)); addSubview(button)
        }
        restyle()
        frame.size = pickerSize(width: 252)
        layoutButtons()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private var rows: Int { (buttons.count + 2) / 3 }

    /// The picker's size at `width`; a flyout keeps its fixed three columns.
    func pickerSize(width: CGFloat) -> NSSize {
        let gap = tokens.number("s-2")
        let height = 2 * padding + CGFloat(rows) * side + CGFloat(max(0, rows - 1)) * gap
        return NSSize(width: flyout ? 2 * padding + 3 * side + 2 * gap : width, height: height)
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        layoutButtons()
    }

    private func restyle() {
        guard flyout else { return }
        layer?.cornerRadius = tokens.number("r-lg")
        layer?.borderWidth = 1
        layer?.borderColor = tokens.color("border-subtle").cgColor
        layer?.backgroundColor = tokens.color("surface-overlay").cgColor
    }

    private func layoutButtons() {
        let gap = tokens.number("s-2")
        let width = flyout ? side : max(0, (bounds.width - 2 * gap) / 3)
        for (index, entry) in buttons.enumerated() {
            entry.button.frame = NSRect(x: padding + CGFloat(index % 3) * (width + gap),
                                        y: padding + CGFloat(index / 3) * (side + gap),
                                        width: width, height: side)
        }
    }
}

/// Shipping `DropShadowFields` settings (`.screenshot-drop-shadow-settings`),
/// shown under the "Drop shadow" check row and indented to its label: the
/// Shadow color `ColorField`, the Opacity (0–100%) and Blur (0–100 px)
/// `RangeSlider`s, then the X/Y offset `NumberInput` pair (±500, whole
/// pixels). Swatches and sliders apply as they change; an offset applies on
/// Enter, on leaving its field or from a stepper, clamped and rounded like
/// shipping's `commitOffset`. Frames depend only on the width.
final class EditorDropShadowFields: NSView, NSTextFieldDelegate {
    struct Value: Equatable {
        var color: String
        var opacity: Double
        var blur: Double
        var offsetX: Double
        var offsetY: Double
    }

    /// The 15 pt check box plus its 8 pt gap: the settings align with the label.
    static let indent: CGFloat = 23
    static let blurMaximum = 100.0
    static let offsetMaximum = 500.0
    private static let gap: CGFloat = 12
    private static let label: CGFloat = 22
    private static let fieldHeight: CGFloat = 30

    override var isFlipped: Bool { true }
    let colorLegend = NSTextField(labelWithString: "Shadow color")
    let swatches: ColorSwatchRow
    let opacitySlider: EditorMarkedSlider
    let blurSlider: EditorMarkedSlider
    let offsetXLegend = NSTextField(labelWithString: "X offset")
    let offsetYLegend = NSTextField(labelWithString: "Y offset")
    let offsetXField = TokenNumberField()
    let offsetYField = TokenNumberField()
    private let formatter: NumberFormatter
    /// The shown shadow. Controls update it before reporting `changed`.
    private(set) var shadow = Value(color: "#000000", opacity: 35, blur: 12, offsetX: 0, offsetY: 8)
    /// A control changed `shadow`: "color", "opacity", "blur", "offsetX" or "offsetY".
    var changed: (String) -> Void = { _ in }
    var tokens: Tokens {
        didSet {
            swatches.tokens = tokens; opacitySlider.tokens = tokens; blurSlider.tokens = tokens
            offsetXField.tokens = tokens; offsetYField.tokens = tokens
            restyle()
        }
    }
    var isEnabled = true {
        didSet {
            swatches.isEnabled = isEnabled
            opacitySlider.isEnabled = isEnabled; blurSlider.isEnabled = isEnabled
            offsetXField.isEnabled = isEnabled; offsetYField.isEnabled = isEnabled
        }
    }

    /// The height of the settings at `width`.
    static func height(width: CGFloat, tokens: Tokens) -> CGFloat {
        label + ColorSwatchRow.height(width: width, compact: false, tokens: tokens)
            + 2 * (gap + EditorMarkedSlider.plainHeight) + gap + label + fieldHeight
    }

    /// `name` prefixes accessible names ("" gives shipping's "Shadow blur";
    /// "Text" gives "Text shadow blur"), so several instances stay distinct.
    init(tokens: Tokens, formatter: NumberFormatter, name: String = "") {
        self.tokens = tokens; self.formatter = formatter
        func accessible(_ base: String) -> String { name.isEmpty ? base : "\(name) \(base.lowercased())" }
        swatches = ColorSwatchRow(tokens: tokens, label: accessible("Shadow color"), compact: false)
        opacitySlider = EditorMarkedSlider(tokens: tokens, title: "Opacity",
            accessibilityLabel: accessible("Shadow opacity"), range: 0...100, value: 35, marks: []) {
            "\(Int($0))%"
        }
        blurSlider = EditorMarkedSlider(tokens: tokens, title: "Blur",
            accessibilityLabel: accessible("Shadow blur"), range: 0...EditorDropShadowFields.blurMaximum, value: 12,
            marks: []) {
            "\(Int($0)) px"
        }
        super.init(frame: NSRect(x: 0, y: 0, width: 229, height: 0))
        setAccessibilityElement(true)
        setAccessibilityRole(.group); setAccessibilityLabel(accessible("Shadow settings"))
        for legend in [colorLegend, offsetXLegend, offsetYLegend] {
            legend.setAccessibilityElement(false); addSubview(legend)
        }
        addSubview(swatches); addSubview(opacitySlider); addSubview(blurSlider)
        swatches.changed = { [weak self] color in
            guard let self, self.isEnabled else { return }
            self.shadow.color = color; self.changed("color")
        }
        opacitySlider.changed = { [weak self] opacity in
            guard let self else { return }
            self.shadow.opacity = opacity; self.changed("opacity")
        }
        blurSlider.changed = { [weak self] blur in
            guard let self else { return }
            self.shadow.blur = blur; self.changed("blur")
        }
        for (field, axis) in [(offsetXField, "X"), (offsetYField, "Y")] {
            field.setAccessibilityLabel(accessible("Shadow \(axis) offset"))
            field.alignment = .left
            field.formatter = nil
            field.placeholderString = "0"
            field.minimum = { -EditorDropShadowFields.offsetMaximum }
            field.maximum = { EditorDropShadowFields.offsetMaximum }
            field.delegate = self
            field.target = self; field.action = #selector(offsetCommitted(_:))
            field.cell?.sendsActionOnEndEditing = true
            field.stepped = { [weak self] stepped in self?.commitOffset(stepped) }
            addSubview(field)
        }
        restyle()
        show(shadow)
        setFrameSize(NSSize(width: 229, height: Self.height(width: 229, tokens: tokens)))
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    /// Show `shadow` without reporting a change; a field being typed in keeps its text.
    func show(_ value: Value) {
        shadow = value
        swatches.selectedHex = value.color
        opacitySlider.value = value.opacity
        blurSlider.value = value.blur
        for (field, offset) in [(offsetXField, value.offsetX), (offsetYField, value.offsetY)]
            where field.currentEditor() == nil {
            field.stringValue = format(offset)
        }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        layoutFields()
    }

    private func layoutFields() {
        let width = bounds.width
        var y: CGFloat = 0
        colorLegend.frame = NSRect(x: 0, y: y, width: width, height: 20)
        let swatchHeight = ColorSwatchRow.height(width: width, compact: false, tokens: tokens)
        swatches.frame = NSRect(x: 0, y: y + Self.label, width: width, height: swatchHeight)
        y = swatches.frame.maxY + Self.gap
        opacitySlider.frame = NSRect(x: 0, y: y, width: width, height: EditorMarkedSlider.plainHeight)
        y = opacitySlider.frame.maxY + Self.gap
        blurSlider.frame = NSRect(x: 0, y: y, width: width, height: EditorMarkedSlider.plainHeight)
        y = blurSlider.frame.maxY + Self.gap
        // Shipping `.screenshot-number-pair`: two columns `--s-4` apart.
        let column = max(0, (width - tokens.number("s-4")) / 2)
        let second = column + tokens.number("s-4")
        offsetXLegend.frame = NSRect(x: 0, y: y, width: column, height: 20)
        offsetYLegend.frame = NSRect(x: second, y: y, width: column, height: 20)
        offsetXField.frame = NSRect(x: 0, y: y + Self.label, width: column, height: Self.fieldHeight)
        offsetYField.frame = NSRect(x: second, y: y + Self.label, width: column, height: Self.fieldHeight)
        offsetXField.updateSteppers(); offsetYField.updateSteppers()
    }

    private func restyle() {
        for legend in [colorLegend, offsetXLegend, offsetYLegend] {
            legend.font = .systemFont(ofSize: tokens.number("text-sm"))
            legend.textColor = tokens.color("text-muted")
        }
        offsetXField.tokens = tokens; offsetYField.tokens = tokens
        needsDisplay = true
    }

    private func format(_ offset: Double) -> String {
        let rounded = offset.rounded()
        return formatter.string(from: NSNumber(value: rounded)) ?? String(Int(rounded))
    }

    private func parse(_ text: String) -> Double? {
        let trimmed = text.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "\u{2212}", with: "-")
        guard let parsed = formatter.number(from: trimmed)?.doubleValue ?? Double(trimmed),
              parsed.isFinite else { return nil }
        return parsed
    }

    @objc private func offsetCommitted(_ sender: TokenNumberField) { commitOffset(sender) }

    /// Shipping `NumberInput` steps on ArrowUp/ArrowDown while editing.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        guard let field = control as? TokenNumberField else { return false }
        if commandSelector == #selector(NSResponder.moveUp(_:)) { field.step(up: true); return true }
        if commandSelector == #selector(NSResponder.moveDown(_:)) { field.step(up: false); return true }
        return false
    }

    private func show(_ text: String, in field: TokenNumberField) {
        if field.stringValue != text { field.stringValue = text }
        if let editor = field.currentEditor(), editor.string != text { editor.string = text }
    }

    /// Shipping `commitOffset`: a finite entry clamps to ±500 and rounds; an
    /// unparsable one keeps the previous offset.
    private func commitOffset(_ field: TokenNumberField) {
        guard isEnabled else { return }
        let axisX = field === offsetXField
        let current = axisX ? shadow.offsetX : shadow.offsetY
        guard let parsed = parse(field.currentEditor()?.string ?? field.stringValue) else {
            show(format(current), in: field); return
        }
        let offset = min(Self.offsetMaximum, max(-Self.offsetMaximum, parsed.rounded()))
        show(format(offset), in: field)
        guard offset != current else { return }
        if axisX { shadow.offsetX = offset } else { shadow.offsetY = offset }
        changed(axisX ? "offsetX" : "offsetY")
    }

}
