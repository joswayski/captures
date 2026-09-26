import AppKit
import CCapturesSettings

/// Shipping "Captures is ready to use" launch notice: a dark glass pill with a
/// triangle caret pointing at the menu bar item. Placement is the shared Rust
/// policy (`captures_app::tray_notice`), so every native host agrees.
enum StartupNoticeCopy {
    static let windowTitle = "Captures is running"
    static let title = "Captures is ready to use"
    static let hint = "Open New Capture with"
    static let defaultShortcut = "CommandOrControl+Shift+Space"
}

enum StartupNoticeTrigger: Equatable {
    /// First-run setup just completed.
    case afterSetup
    /// Hidden login / quiet launch into the menu bar.
    case quietLaunch

    var lifetime: TimeInterval { self == .afterSetup ? 15 : 5 }
}

/// Which notice (if any) a finished launch shows. Opening media skips the
/// quiet-launch notice, like the shipping app; a visible launch never shows it.
func startupNoticeTrigger(setupWasPresented: Bool, hiddenLaunch: Bool,
                          openingMedia: Bool) -> StartupNoticeTrigger? {
    if setupWasPresented { return .afterSetup }
    return hiddenLaunch && !openingMedia ? .quietLaunch : nil
}

struct StartupNoticeLayout: Equatable {
    enum Caret: UInt32 { case none = 0, top = 1, bottom = 2 }

    /// Window frame in top-left logical desktop coordinates.
    var frame: NSRect
    /// Visible card, window-local with a top-left origin (flipped view).
    var card: NSRect
    var caret: Caret
    /// Caret tip x, window-local.
    var caretX: CGFloat

    static let caretSize: CGFloat = 8
    static let caretSpan: CGFloat = 12

    static func resolve(monitor: NSRect, workArea: NSRect, tray: NSRect?) -> StartupNoticeLayout? {
        func abi(_ rect: NSRect) -> CapturesTrayNoticeRect {
            CapturesTrayNoticeRect(x: Double(rect.minX), y: Double(rect.minY),
                width: Double(rect.width), height: Double(rect.height))
        }
        var output = CapturesTrayNoticePlacement()
        let ok: Bool
        // macOS: the menu bar is at the top (fallback edge 0 = top).
        if let tray {
            var trayRect = abi(tray)
            ok = captures_startup_notice_placement_v1(abi(monitor), abi(workArea), &trayRect, true, 0, &output)
        } else {
            ok = captures_startup_notice_placement_v1(abi(monitor), abi(workArea), nil, true, 0, &output)
        }
        guard ok, let caret = Caret(rawValue: output.caret) else { return nil }
        return StartupNoticeLayout(
            frame: NSRect(x: output.x, y: output.y, width: output.width, height: output.height),
            card: NSRect(x: output.card.x, y: output.card.y, width: output.card.width, height: output.card.height),
            caret: caret, caretX: CGFloat(output.caret_x))
    }

    /// AppKit global frames have a bottom-left origin at the primary display.
    /// The shared policy uses top-left logical points; the flip is its own inverse.
    static func flip(_ rect: NSRect, primaryHeight: CGFloat) -> NSRect {
        NSRect(x: rect.minX, y: primaryHeight - rect.maxY, width: rect.width, height: rect.height)
    }

    /// `statusItemFrame` is the status item button's window frame (AppKit
    /// coordinates), or nil before the item has a screen.
    static func current(statusItemFrame: NSRect?, screens: [NSScreen] = NSScreen.screens) -> StartupNoticeLayout? {
        guard let primary = screens.first else { return nil }
        let height = primary.frame.maxY
        let screen = statusItemFrame.flatMap { item in
            screens.first { $0.frame.contains(NSPoint(x: item.midX, y: item.midY)) }
        } ?? NSScreen.main ?? primary
        return resolve(monitor: flip(screen.frame, primaryHeight: height),
            workArea: flip(screen.visibleFrame, primaryHeight: height),
            tray: statusItemFrame.map { flip($0, primaryHeight: height) })
    }

    /// Frame to pass to `NSWindow.setFrame`.
    func appKitFrame(primaryHeight: CGFloat) -> NSRect {
        Self.flip(frame, primaryHeight: primaryHeight)
    }

    /// Triangle `[tip, baseLeft, baseRight]`, window-local top-left.
    var caretTriangle: [NSPoint]? {
        let half = Self.caretSpan / 2
        switch caret {
        case .none: return nil
        case .top:
            return [NSPoint(x: caretX, y: 0), NSPoint(x: caretX - half, y: Self.caretSize),
                    NSPoint(x: caretX + half, y: Self.caretSize)]
        case .bottom:
            let h = frame.height
            return [NSPoint(x: caretX, y: h), NSPoint(x: caretX - half, y: h - Self.caretSize),
                    NSPoint(x: caretX + half, y: h - Self.caretSize)]
        }
    }
}

final class StartupNoticeCloseButton: NSButton {
    private let tokens: Tokens
    private var tracking: NSTrackingArea?
    private(set) var hovered = false
    var actionBlock: (() -> Void)?

    init(frame: NSRect, tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: frame)
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; action = #selector(activate)
        setAccessibilityLabel("Close")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock?() }
    /// The panel never activates Captures, so the first click must act.
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(area); tracking = area
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        let active = hovered || cell?.isHighlighted == true
        if active {
            tokens.color("glass-hover").setFill()
            NSBezierPath(ovalIn: bounds).fill()
        }
        // 14px CloseIcon: a 24-unit × with a 2-unit stroke.
        let arm: CGFloat = 3.5, center = NSPoint(x: bounds.midX, y: bounds.midY)
        let path = NSBezierPath()
        path.move(to: NSPoint(x: center.x - arm, y: center.y - arm))
        path.line(to: NSPoint(x: center.x + arm, y: center.y + arm))
        path.move(to: NSPoint(x: center.x - arm, y: center.y + arm))
        path.line(to: NSPoint(x: center.x + arm, y: center.y - arm))
        path.lineWidth = 1.5; path.lineCapStyle = .round
        tokens.color(active ? "glass-text" : "glass-text-subtle").setStroke()
        path.stroke()
    }
}

final class StartupNoticeView: NSView {
    let layout: StartupNoticeLayout
    let keys: [String]
    let closeButton: StartupNoticeCloseButton
    private let tokens: Tokens

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }

    init(layout: StartupNoticeLayout, keys: [String], tokens: Tokens) {
        self.layout = layout; self.keys = keys; self.tokens = tokens
        let side = tokens.number("h-sm")
        closeButton = StartupNoticeCloseButton(frame: NSRect(
            x: layout.card.maxX - tokens.number("s-3") - side,
            y: layout.card.midY - side / 2, width: side, height: side), tokens: tokens)
        super.init(frame: NSRect(origin: .zero, size: layout.frame.size))
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
        addSubview(closeButton)
        setAccessibilityRole(.group)
        setAccessibilityLabel("\(StartupNoticeCopy.title). \(accessibleHint)")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var accessibleHint: String { ([StartupNoticeCopy.hint] + keys).joined(separator: " ") }

    var titleAttributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .semibold),
         .foregroundColor: tokens.color("glass-text")]
    }
    var hintAttributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
         .foregroundColor: tokens.color("glass-text-subtle")]
    }
    var keyAttributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .semibold),
         .foregroundColor: tokens.color("glass-text")]
    }

    /// Centered title and the hint row (text plus one chip per key), flipped.
    func contentLayout() -> (title: NSRect, hint: NSRect, chips: [NSRect]) {
        let card = layout.card
        let titleSize = (StartupNoticeCopy.title as NSString).size(withAttributes: titleAttributes)
        let hintSize = (StartupNoticeCopy.hint as NSString).size(withAttributes: hintAttributes)
        let padX = tokens.number("s-1") + 1, padY: CGFloat = 1, gap = tokens.number("s-1")
        let chipSizes = keys.map { key -> NSSize in
            let size = (key as NSString).size(withAttributes: keyAttributes)
            return NSSize(width: ceil(size.width) + padX * 2, height: ceil(size.height) + padY * 2)
        }
        let space = tokens.number("s-2") - gap
        let rowHeight = max(hintSize.height, chipSizes.map(\.height).max() ?? 0)
        let rowWidth = hintSize.width + space + chipSizes.reduce(0) { $0 + $1.width + gap }
        let contentHeight = titleSize.height + gap + rowHeight
        let top = card.midY - contentHeight / 2
        let title = NSRect(x: card.midX - titleSize.width / 2, y: top, width: titleSize.width, height: titleSize.height)
        let rowTop = title.maxY + gap
        var x = card.midX - rowWidth / 2
        let hint = NSRect(x: x, y: rowTop + (rowHeight - hintSize.height) / 2,
            width: hintSize.width, height: hintSize.height)
        x = hint.maxX + space
        var chips: [NSRect] = []
        for size in chipSizes {
            x += gap
            chips.append(NSRect(x: x, y: rowTop + (rowHeight - size.height) / 2, width: size.width, height: size.height))
            x += size.width
        }
        return (title, hint, chips)
    }

    override func draw(_ dirtyRect: NSRect) {
        let fill = tokens.color("glass-strong-solid")
        let card = layout.card
        let pill = NSBezierPath(roundedRect: card, xRadius: card.height / 2, yRadius: card.height / 2)
        let caret = layout.caretTriangle.map { points -> NSBezierPath in
            let path = NSBezierPath()
            path.move(to: points[0]); path.line(to: points[1]); path.line(to: points[2]); path.close()
            return path
        }
        NSGraphicsContext.saveGraphicsState()
        // --tooltip-shadow: drop-shadow(0 4px 10px rgba(0, 0, 0, 0.32)). NSShadow
        // offsets use base coordinates, so negative height is downward here too.
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.32)
        shadow.shadowBlurRadius = 10; shadow.shadowOffset = NSSize(width: 0, height: -4)
        shadow.set()
        // One layer so the caret and pill cast a single drop shadow, like CSS filter.
        let context = NSGraphicsContext.current?.cgContext
        context?.beginTransparencyLayer(auxiliaryInfo: nil)
        fill.setFill(); pill.fill(); caret?.fill()
        context?.endTransparencyLayer()
        NSGraphicsContext.restoreGraphicsState()

        let content = contentLayout()
        (StartupNoticeCopy.title as NSString).draw(in: content.title, withAttributes: titleAttributes)
        (StartupNoticeCopy.hint as NSString).draw(in: content.hint, withAttributes: hintAttributes)
        for (key, rect) in zip(keys, content.chips) {
            let chip = NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5),
                xRadius: tokens.number("r-sm"), yRadius: tokens.number("r-sm"))
            tokens.color("glass-hover").setFill(); chip.fill()
            tokens.color("glass-border").setStroke(); chip.lineWidth = 1; chip.stroke()
            let size = (key as NSString).size(withAttributes: keyAttributes)
            (key as NSString).draw(at: NSPoint(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2),
                withAttributes: keyAttributes)
        }
    }
}

final class StartupNoticePanel: NSPanel {
    let noticeView: StartupNoticeView
    // Like the recording notice: key only if a control needs it, which the
    // Close button does not, so the frontmost app keeps focus.
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    init(layout: StartupNoticeLayout, keys: [String], tokens: Tokens, primaryHeight: CGFloat) {
        noticeView = StartupNoticeView(layout: layout, keys: keys, tokens: tokens)
        super.init(contentRect: layout.appKitFrame(primaryHeight: primaryHeight),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        title = StartupNoticeCopy.windowTitle
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear
        hasShadow = false; hidesOnDeactivate = false; becomesKeyOnlyIfNeeded = true
        level = .floating; isFloatingPanel = true
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        contentView = noticeView
    }

    func apply(_ layout: StartupNoticeLayout, primaryHeight: CGFloat) {
        setFrame(layout.appKitFrame(primaryHeight: primaryHeight), display: true)
    }
}

/// Owns one launch notice at a time. Generation tokens make late timers and
/// retries harmless after dismissal or replacement.
final class StartupNoticeController {
    static let trayRetryAttempts = 20
    static let trayRetryDelay: TimeInterval = 0.05
    private let tokens: Tokens
    private(set) var panel: StartupNoticePanel?
    private(set) var generation = 0
    private var timer: Timer?
    /// Status item button frame in AppKit coordinates, or nil while unplaced.
    var statusItemFrame: () -> NSRect? = { nil }
    var screens: () -> [NSScreen] = { NSScreen.screens }

    init(tokens: Tokens) { self.tokens = tokens }

    var isVisible: Bool { panel != nil }

    /// Shows the notice for `trigger.lifetime`. Waits briefly for the status
    /// item to be laid out; after a relaunch AppKit can report it at the
    /// Cocoa origin before it reaches the menu bar.
    func present(trigger: StartupNoticeTrigger, keys: [String]) {
        dismiss()
        generation += 1
        let generation = self.generation
        attemptPresent(generation: generation, keys: keys, lifetime: trigger.lifetime,
            attemptsLeft: Self.trayRetryAttempts)
    }

    private func attemptPresent(generation: Int, keys: [String], lifetime: TimeInterval, attemptsLeft: Int) {
        guard generation == self.generation else { return }
        let available = self.screens()
        guard let primary = available.first,
              let layout = StartupNoticeLayout.current(statusItemFrame: statusItemFrame(), screens: available)
        else { return }
        if layout.caret == .none, attemptsLeft > 0 {
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.trayRetryDelay) { [weak self] in
                self?.attemptPresent(generation: generation, keys: keys, lifetime: lifetime,
                    attemptsLeft: attemptsLeft - 1)
            }
            return
        }
        show(layout: layout, keys: keys, lifetime: lifetime, primaryHeight: primary.frame.maxY)
    }

    /// Visible for tests: presents an already-resolved layout immediately.
    func show(layout: StartupNoticeLayout, keys: [String], lifetime: TimeInterval, primaryHeight: CGFloat) {
        dismiss()
        let panel = StartupNoticePanel(layout: layout, keys: keys, tokens: tokens, primaryHeight: primaryHeight)
        panel.noticeView.closeButton.actionBlock = { [weak self] in self?.dismiss() }
        self.panel = panel
        // Never activates Captures or takes key focus from the frontmost app.
        panel.orderFrontRegardless()
        panel.apply(layout, primaryHeight: primaryHeight)
        // Shipping `startup-arrive`, rising from below when the caret points down.
        NativeMotion.play(layout.caret == .bottom ? "startup_notice_in_from_below" : "startup_notice_in",
                          on: panel.noticeView, tokens: tokens)
        let generation = self.generation
        timer = Timer.scheduledTimer(withTimeInterval: lifetime, repeats: false) { [weak self] _ in
            guard let self, self.generation == generation else { return }
            self.dismiss()
        }
    }

    func dismiss() {
        generation += 1
        timer?.invalidate(); timer = nil
        panel?.orderOut(nil); panel?.close(); panel = nil
    }

    deinit { timer?.invalidate(); panel?.close() }
}
