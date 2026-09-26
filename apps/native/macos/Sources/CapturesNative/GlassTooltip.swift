import AppKit

/// Fixed-glass tooltip chip that replaces the delayed system tooltip on
/// floating surfaces: the recording HUD (`.recording-tooltip`) and mini-preview
/// icons (`.icon-button::after`). It appears with no delay on hover or keyboard
/// focus and never takes the mouse; the owning surface places and animates it.
class GlassTooltipView: NSView {
    struct Style {
        let fontToken: String
        let radiusToken: String
        /// Label inset: the shipping padding plus any border.
        let inset: NSSize
        let bordered: Bool
        let shadowed: Bool

        /// `.recording-tooltip > [role="tooltip"]`: xs medium text in an `--r-sm`
        /// pill with a glass border.
        static let recordingHUD = Style(fontToken: "text-xs", radiusToken: "r-sm",
                                        inset: NSSize(width: 9, height: 6),
                                        bordered: true, shadowed: false)
        /// `.icon-button::after`: 2xs medium text, `padding: 4px 7px`, `--r-xs`,
        /// `--shadow-sm` and no border.
        static let previewIcon = Style(fontToken: "text-2xs", radiusToken: "r-xs",
                                       inset: NSSize(width: 7, height: 4),
                                       bordered: false, shadowed: true)
    }

    let label = NSTextField(labelWithString: "")
    private let chipStyle: Style
    override var isFlipped: Bool { true }

    init(tokens: Tokens, style: Style) {
        chipStyle = style
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number(style.radiusToken)
        if style.bordered {
            layer?.borderColor = tokens.color("glass-border").cgColor
            layer?.borderWidth = 1
        }
        if style.shadowed {
            // `--shadow-sm`: 0 2px 6px rgba(0, 0, 0, .32).
            layer?.masksToBounds = false
            layer?.shadowColor = NSColor.black.cgColor
            layer?.shadowOpacity = 0.32
            layer?.shadowRadius = 3
            layer?.shadowOffset = CGSize(width: 0, height: -2)
        }
        label.font = .systemFont(ofSize: tokens.number(style.fontToken), weight: .medium)
        label.textColor = tokens.color("glass-text")
        label.alignment = .center
        label.lineBreakMode = .byTruncatingTail
        addSubview(label)
        isHidden = true
        alphaValue = 0
        setAccessibilityElement(false)
        label.setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func layout() {
        super.layout()
        label.frame = bounds.insetBy(dx: chipStyle.inset.width, dy: chipStyle.inset.height)
    }
}
