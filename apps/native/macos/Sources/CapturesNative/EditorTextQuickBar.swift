import AppKit

/// `captures_app::editor_chrome::text_quick_bar`: the format bar that floats
/// over a selected text layer under Select. Font, Smaller / size / Larger,
/// then Bold, Italic and the alignments. The editor mirrors the selected
/// layer into it and applies its changes through the inspector's text fields.
final class EditorTextQuickBar: NSView {
    override var isFlipped: Bool { true }
    var tokens: Tokens {
        didSet {
            font.tokens = tokens; format.tokens = tokens
            [smaller, larger].forEach { $0.tokens = tokens }
            sizeLabel.textColor = tokens.color("text")
            refreshShadow(); needsDisplay = true
        }
    }
    let font = ClosurePopUpButton(frame: .zero, pullsDown: false)
    let smaller: EditorFormatButton
    let larger: EditorFormatButton
    let format: EditorTextFormatButtons
    private let sizeLabel = NSTextField(labelWithString: "")
    private var separators: [CGFloat] = []
    /// Smaller (false) or Larger (true) was pressed.
    var stepped: (Bool) -> Void = { _ in }

    static func copy(_ key: String) -> String { EditorChrome.text("text_quick_bar", key) }
    private static func metric(_ key: String, _ fallback: CGFloat) -> CGFloat {
        let group = EditorChrome.copy["text_quick_bar"] as? [String: Any]
        return (group?[key] as? NSNumber).map { CGFloat($0.doubleValue) } ?? fallback
    }
    /// The ladder Smaller / Larger walk (`text_quick_bar::SIZES`).
    static var sizes: [Double] {
        let group = EditorChrome.copy["text_quick_bar"] as? [String: Any]
        return (group?["sizes"] as? [NSNumber])?.map(\.doubleValue) ?? []
    }
    /// `text_quick_bar::step`.
    static func step(_ current: Double, larger: Bool) -> Double {
        let next = larger ? sizes.first { $0 > current + 1e-9 } : sizes.last { $0 < current - 1e-9 }
        return next ?? current
    }
    static var offset: CGFloat { metric("offset", 10) }

    var fontSize: Double = 32 { didSet { refreshSize() } }

    private func refreshSize() {
        sizeLabel.stringValue = fontSize.rounded() == fontSize
            ? String(Int(fontSize)) : String(format: "%.1f", fontSize)
        sizeLabel.setAccessibilityValue(sizeLabel.stringValue)
        smaller.isEnabled = Self.step(fontSize, larger: false) != fontSize
        larger.isEnabled = Self.step(fontSize, larger: true) != fontSize
    }

    init(tokens: Tokens) {
        self.tokens = tokens
        smaller = EditorFormatButton(glyph: .icon("minus"), label: Self.copy("smaller"), tokens: tokens)
        larger = EditorFormatButton(glyph: .icon("plus"), label: Self.copy("larger"), tokens: tokens)
        format = EditorTextFormatButtons(tokens: tokens)
        super.init(frame: .zero)
        wantsLayer = true
        setAccessibilityElement(true)
        setAccessibilityRole(.toolbar); setAccessibilityLabel(Self.copy("label"))
        font.tokens = tokens
        font.setAccessibilityLabel(Self.copy("font"))
        addSubview(font)
        for (button, larger) in [(smaller, false), (self.larger, true)] {
            button.borderless = true
            button.setAccessibilityRole(.button)
            button.pressed = { [weak self] in self?.stepped(larger) }
        }
        sizeLabel.font = .monospacedDigitSystemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        sizeLabel.alignment = .center
        sizeLabel.textColor = tokens.color("text")
        sizeLabel.setAccessibilityLabel(Self.copy("size"))
        [smaller, sizeLabel, larger, format].forEach { addSubview($0) }
        ([format.boldButton, format.italicButton] + format.alignButtons.map(\.button)).forEach {
            $0.borderless = true
        }
        refreshSize()
        layoutControls()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    /// Lay the controls out in one row and size the bar to fit them.
    private func layoutControls() {
        let control = Self.metric("control", 28), padding = Self.metric("padding", 4)
        let gap = Self.metric("gap", 4)
        var x = padding
        func place(_ view: NSView, width: CGFloat) {
            view.frame = NSRect(x: x, y: padding, width: width, height: control)
            x += width + gap
        }
        func separate() { separators.append(x); x += 1 + gap }
        separators = []
        place(font, width: Self.metric("font_width", 116))
        separate()
        place(smaller, width: control)
        let labelHeight = ceil(sizeLabel.intrinsicContentSize.height)
        place(sizeLabel, width: Self.metric("size_width", 36))
        sizeLabel.frame.origin.y = padding + (control - labelHeight) / 2
        sizeLabel.frame.size.height = labelHeight
        place(larger, width: control)
        separate()
        place(format, width: control * 5 + gap * 4)
        setFrameSize(NSSize(width: x - gap + padding, height: control + padding * 2))
        refreshShadow()
    }

    private func refreshShadow() {
        guard let layer, bounds.width > 0 else { return }
        layer.masksToBounds = false
        BoxShadowLayers.install(tokens.shadow("shadow-md"), on: layer, bounds: bounds,
                                radius: tokens.number("r-md"))
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-md")
        let card = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        tokens.color("surface-raised").setFill(); card.fill()
        tokens.color("border-subtle").setStroke(); card.lineWidth = 1; card.stroke()
        let inset = Self.metric("padding", 4) + 4
        tokens.color("border-subtle").setFill()
        for x in separators {
            NSRect(x: x, y: inset, width: 1, height: bounds.height - inset * 2).fill(using: .sourceOver)
        }
    }

    /// Press-through: clicks between controls must not reach the canvas.
    override func mouseDown(with event: NSEvent) {}
}
