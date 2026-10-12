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

    var text = "" { didSet { needsDisplay = true } }
    private let chipStyle: Style
    private let textAttributes: [NSAttributedString.Key: Any]
    private var attributedText: NSAttributedString { NSAttributedString(string: text, attributes: textAttributes) }
    var textSize: NSSize {
        let measured = attributedText.size()
        return NSSize(width: ceil(measured.width), height: ceil(measured.height))
    }
    override var isFlipped: Bool { true }

    init(tokens: Tokens, style: Style) {
        chipStyle = style
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = .center
        paragraph.lineBreakMode = .byTruncatingTail
        textAttributes = [.font: NSFont.systemFont(ofSize: tokens.number(style.fontToken), weight: .medium),
                          .foregroundColor: tokens.color("glass-text"), .paragraphStyle: paragraph]
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
        isHidden = true
        alphaValue = 0
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    var textBounds: NSRect {
        // An oversized CGRect inset is null, with infinite x/y origins. Hidden,
        // unsized chips have no drawable text area.
        let content = bounds.insetBy(dx: chipStyle.inset.width, dy: chipStyle.inset.height)
        return content.isNull ? .zero : content
    }

    override func draw(_ dirtyRect: NSRect) {
        let content = textBounds
        guard content.width > 0, content.height > 0 else { return }
        // Measure and paint the same glyphs. A separate NSTextField cell can
        // truncate even when its intrinsic width was used to place this chip.
        attributedText.draw(in: content)
    }
}
