import AppKit

/// AppKit text over a fixed media palette, matching the shipping countdown.
/// Only the numeral changes each second; no per-frame view/layer construction.
final class ScreenshotCountdownContent: NSView {
    private let tokens: Tokens
    private let heading = NSTextField(labelWithString: "SCREENSHOT IN")
    private let number = NSTextField(labelWithString: "")
    private let hint = NSTextField(labelWithString: "Press Esc to cancel")
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens, remaining: Int) {
        self.tokens = tokens
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-countdown-scrim").cgColor
        for field in [heading, number, hint] {
            field.alignment = .center
            field.textColor = tokens.color(field === number ? "glass-text" : "glass-text-muted")
            addSubview(field)
        }
        setRemaining(remaining)
        needsLayout = true
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func setRemaining(_ value: Int) {
        guard number.stringValue != String(value) else { return }
        number.stringValue = String(value)
        number.setAccessibilityLabel("Screenshot in \(value) seconds")
    }
    override func layout() {
        super.layout()
        let labelSize = min(tokens.number("countdown-label-max"), max(tokens.number("countdown-label-min"), bounds.width * 0.016))
        let numberSize = min(tokens.number("countdown-number-max"), max(tokens.number("countdown-number-min"), bounds.width * 0.26))
        heading.font = .systemFont(ofSize: labelSize, weight: .medium)
        number.font = .monospacedDigitSystemFont(ofSize: numberSize, weight: .bold)
        hint.font = .systemFont(ofSize: tokens.number("text-md"))
        let height = labelSize * 1.4 + numberSize * 1.2 + tokens.number("s-7") + tokens.number("text-md") * 1.5
        let top = (bounds.height - height) / 2
        heading.frame = NSRect(x: 0, y: top, width: bounds.width, height: labelSize * 1.4)
        number.frame = NSRect(x: 0, y: heading.frame.maxY, width: bounds.width, height: numberSize * 1.2)
        hint.frame = NSRect(x: 0, y: number.frame.maxY + tokens.number("s-7"), width: bounds.width, height: tokens.number("text-md") * 1.5)
    }
}

final class ScreenshotCountdownPanel: NSPanel {
    let countdownContent: ScreenshotCountdownContent
    override var canBecomeKey: Bool { true }

    init(screen: NSScreen, tokens: Tokens, remaining: Int) {
        countdownContent = ScreenshotCountdownContent(frame: NSRect(origin: .zero, size: screen.frame.size), tokens: tokens, remaining: remaining)
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear; hasShadow = false
        level = .screenSaver; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none; contentView = countdownContent
    }
}
