import AppKit

enum RecordingHUDColorToken: String, CaseIterable {
    case glassStrong = "glass-strong"
    case glassBorder = "glass-border"
    case glassText = "glass-text"
    case glassTextSubtle = "glass-text-subtle"
    case themeAccent = "theme-accent"
    case themeSignal = "theme-signal"
    case themeSignalSurface = "theme-signal-surface"
}

final class RecordingHUDView: NSView {
    private let tokens: Tokens
    private let defaultNotice: String
    private let statusDot = NSView()
    private let timerLabel = NSTextField(labelWithString: "0:00")
    private let statusLabel = NSTextField(labelWithString: "RECORDING")
    private let noticeLabel = NSTextField(labelWithString: "These controls won’t show in recordings")
    private let pauseButton: CaptureButton
    private var lifecycleButtons: [CaptureButton] = []
    private var elapsedMilliseconds: UInt64 = 0
    private var resumedAt: Date?
    private var timer: Timer?
    private(set) var paused = false
    var pauseOrResume: () -> Void = {}
    var restart: () -> Void = {}
    var stop: () -> Void = {}
    var discard: () -> Void = {}

    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens, excludedFromCapture: Bool = true) {
        self.tokens = tokens
        defaultNotice = excludedFromCapture
            ? "These controls won’t show in recordings"
            : "These controls will appear in recordings"
        pauseButton = CaptureButton("Ⅱ", frame: .zero, tokens: tokens, glass: true) {}
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color(RecordingHUDColorToken.glassStrong.rawValue).cgColor
        layer?.cornerRadius = tokens.number("r-2xl")
        layer?.borderWidth = 1
        layer?.borderColor = tokens.color(RecordingHUDColorToken.glassBorder.rawValue).cgColor
        layer?.shadowColor = NSColor.black.cgColor; layer?.shadowOpacity = 0.44
        layer?.shadowRadius = 22; layer?.shadowOffset = NSSize(width: 0, height: -8)
        setAccessibilityRole(.group); setAccessibilityLabel("Recording controls")

        noticeLabel.stringValue = defaultNotice
        noticeLabel.frame = NSRect(x: 60, y: 8, width: 310, height: 18)
        noticeLabel.alignment = .center
        noticeLabel.font = .systemFont(ofSize: 11, weight: .medium)
        noticeLabel.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        noticeLabel.lineBreakMode = .byTruncatingTail
        addSubview(noticeLabel)

        statusDot.frame = NSRect(x: 18, y: 44, width: 9, height: 9)
        statusDot.wantsLayer = true; statusDot.layer?.cornerRadius = 4.5
        addSubview(statusDot)
        timerLabel.frame = NSRect(x: 34, y: 31, width: 64, height: 30)
        timerLabel.font = .monospacedDigitSystemFont(ofSize: 22, weight: .semibold)
        timerLabel.textColor = tokens.color(RecordingHUDColorToken.glassText.rawValue); addSubview(timerLabel)
        statusLabel.frame = NSRect(x: 34, y: 60, width: 76, height: 16)
        statusLabel.font = .systemFont(ofSize: 9, weight: .semibold)
        statusLabel.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        addSubview(statusLabel)

        let stop = hudButton("■", x: 104, help: "Stop and save recording") { [weak self] in self?.stop() }
        stop.signal = true; stop.setAccessibilityLabel("Stop recording")
        pauseButton.frame = NSRect(x: 144, y: 35, width: 38, height: 42)
        pauseButton.actionBlock = { [weak self] in self?.pauseOrResume() }
        pauseButton.setAccessibilityLabel("Pause recording"); addSubview(pauseButton)
        let restart = hudButton("↻", x: 184, help: "Restart recording") {
            [weak self] in self?.restart()
        }
        restart.setAccessibilityLabel("Restart recording")
        unavailable("⌗", x: 224, label: "Screenshot during recording is unavailable in this version")
        unavailable("—", x: 264, label: "Audio meter is unavailable in this version")
        unavailable("♩", x: 304, label: "Microphone mute is unavailable in this version")
        let trash = hudButton("⌫", x: 344, help: "Discard recording") { [weak self] in self?.discard() }
        trash.setAccessibilityLabel("Discard recording")
        lifecycleButtons = [stop, pauseButton, restart, trash]
        unavailable("◉̸", x: 384, label: "Hide controls is unavailable in this version")
        setPaused(false, elapsedMilliseconds: 0)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private func hudButton(_ title: String, x: CGFloat, help: String,
                           action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: NSRect(x: x, y: 35, width: 38, height: 42),
            tokens: tokens, glass: true, action: action)
        button.toolTip = help; addSubview(button); return button
    }

    private func unavailable(_ title: String, x: CGFloat, label: String) {
        let button = hudButton(title, x: x, help: label) {}
        button.isEnabled = false; button.setAccessibilityLabel(label)
    }

    func setPaused(_ paused: Bool, elapsedMilliseconds: UInt64) {
        self.paused = paused; self.elapsedMilliseconds = elapsedMilliseconds
        resumedAt = paused ? nil : Date()
        statusLabel.stringValue = paused ? "PAUSED" : "RECORDING"
        let statusToken: RecordingHUDColorToken = paused ? .themeAccent : .themeSignal
        statusDot.layer?.backgroundColor = tokens.color(statusToken.rawValue).cgColor
        pauseButton.title = paused ? "▶" : "Ⅱ"
        pauseButton.setAccessibilityLabel(paused ? "Resume recording" : "Pause recording")
        updateTimer()
        timer?.invalidate()
        guard !paused else { timer = nil; return }
        let timer = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in self?.updateTimer() }
        self.timer = timer; RunLoop.main.add(timer, forMode: .common)
    }

    func setWarning(_ warning: String?) {
        noticeLabel.stringValue = warning ?? defaultNotice
        let noticeToken: RecordingHUDColorToken = warning == nil ? .glassTextSubtle : .themeSignal
        noticeLabel.textColor = tokens.color(noticeToken.rawValue)
        noticeLabel.toolTip = warning
        noticeLabel.setAccessibilityLabel(noticeLabel.stringValue)
    }

    func setLifecycleActionsEnabled(_ enabled: Bool) {
        lifecycleButtons.forEach {
            $0.isEnabled = enabled
            $0.needsDisplay = true
        }
    }

    private func updateTimer() {
        let running = resumedAt.map { UInt64(max(0, Date().timeIntervalSince($0)) * 1_000) } ?? 0
        let seconds = (elapsedMilliseconds + running) / 1_000
        timerLabel.stringValue = "\(seconds / 60):\(String(format: "%02d", seconds % 60))"
    }

    deinit { timer?.invalidate() }
}

final class RecordingHUDPanel: NSPanel {
    let hud: RecordingHUDView

    init(screen: NSScreen, tokens: Tokens, excludedFromCapture: Bool) {
        let size = NSSize(width: 430, height: 102)
        let visible = screen.visibleFrame
        let origin = NSPoint(x: visible.midX - size.width / 2, y: visible.minY + 20)
        hud = RecordingHUDView(frame: NSRect(origin: .zero, size: size), tokens: tokens,
            excludedFromCapture: excludedFromCapture)
        super.init(contentRect: NSRect(origin: origin, size: size), styleMask: [.borderless],
            backing: .buffered, defer: false)
        title = "Captures Recording Controls"
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear
        hasShadow = false; level = .floating; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = excludedFromCapture ? .none : .readOnly
        contentView = hud
    }
}
