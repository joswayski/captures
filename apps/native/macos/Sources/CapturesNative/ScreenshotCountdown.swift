import AppKit

/// Shipping countdown headings. The shipping CSS uppercases the heading; the
/// source copy stays sentence case for assistive technology.
enum CountdownKind {
    case screenshot, recording

    var heading: String {
        switch self {
        case .screenshot: return "Screenshot in"
        case .recording: return "Recording starts in"
        }
    }
}

/// AppKit text over a fixed media palette, matching the shipping countdown.
/// Only the numeral changes each second; no per-frame view/layer construction.
final class ScreenshotCountdownContent: NSView {
    private let tokens: Tokens
    private let kind: CountdownKind
    private let heading: NSTextField
    private let number = NSTextField(labelWithString: "")
    private let hint = NSTextField(labelWithString: "Press Esc to cancel")
    private(set) var cancelling = false
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens, remaining: Int, kind: CountdownKind = .screenshot) {
        self.tokens = tokens
        self.kind = kind
        heading = NSTextField(labelWithString: kind.heading.uppercased())
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-countdown-scrim").cgColor
        heading.setAccessibilityLabel(kind.heading)
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
        number.setAccessibilityLabel("\(kind.heading) \(value) seconds")
    }

    /// Shipping copy while Esc is being honoured, before the overlay closes.
    func setCancelling() {
        guard !cancelling else { return }
        cancelling = true
        hint.stringValue = "Cancelling…"
    }

    /// Shipping `recording-countdown-fade-in` on the scrim and `-content-in`
    /// on the text. Presentation-only; skipped under Reduce Motion.
    func playEntrance() {
        NativeMotion.play("countdown_in", on: self, tokens: tokens)
        for field in [heading, number, hint] {
            NativeMotion.play("countdown_content_in", on: field, tokens: tokens)
        }
    }

    /// Shipping `.recording-countdown.exiting`: the scrim and text fade out
    /// and hold there until the panel closes (at once under Reduce Motion).
    func playExit() {
        NativeMotion.play("countdown_out", on: self, tokens: tokens, holdEnd: true)
        for field in [heading, number, hint] {
            NativeMotion.play("countdown_content_out", on: field, tokens: tokens, holdEnd: true)
        }
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
    /// Shipping `RECORDING_COUNTDOWN_FADE_OUT_MS`: how long "Cancelling…" stays up.
    static let cancelLinger: TimeInterval = 0.18
    let countdownContent: ScreenshotCountdownContent
    private var playedEntrance = false
    override var canBecomeKey: Bool { true }

    override func orderFrontRegardless() {
        super.orderFrontRegardless()
        guard !playedEntrance else { return }
        playedEntrance = true
        countdownContent.playEntrance()
    }

    init(screen: NSScreen, tokens: Tokens, remaining: Int, kind: CountdownKind = .screenshot) {
        countdownContent = ScreenshotCountdownContent(frame: NSRect(origin: .zero, size: screen.frame.size),
            tokens: tokens, remaining: remaining, kind: kind)
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear; hasShadow = false
        level = .screenSaver; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none; contentView = countdownContent
    }

    /// Shows "Cancelling…" briefly, then closes. The panel ignores input while
    /// it lingers so the restored desktop is immediately usable.
    func closeAfterCancelling() {
        countdownContent.setCancelling()
        countdownContent.playExit()
        ignoresMouseEvents = true
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.cancelLinger) { [self] in close() }
    }
}
