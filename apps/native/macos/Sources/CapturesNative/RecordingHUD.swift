import AppKit

enum RecordingHUDColorToken: String, CaseIterable {
    case glassStrong = "glass-strong"
    case glassBorder = "glass-border"
    case glassActive = "glass-active"
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
    private let statusLabel = NSTextField(labelWithString: "")
    private let noticeLabel = NSTextField(labelWithString: "These controls won’t show in recordings")
    private let pauseButton: CaptureButton
    private let microphoneButton: CaptureButton
    private let meterTrack = NSView()
    private let meterFill = NSView()
    private let meterLabel = NSTextField(labelWithString: "OFF")
    private var meterLevel = 0.0
    private var lifecycleButtons: [CaptureButton] = []
    private var lifecycleActionsEnabled = true
    private var elapsedMilliseconds: UInt64 = 0
    private var resumedAt: Date?
    private var timer: Timer?
    private(set) var paused = false
    private(set) var microphoneMuted = false
    private(set) var microphoneAvailable = false
    var pauseOrResume: () -> Void = {}
    var toggleMicrophone: () -> Void = {}
    var restart: () -> Void = {}
    var screenshot: () -> Void = {}
    var stop: () -> Void = {}
    var discard: () -> Void = {}
    var hide: () -> Void = {}

    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens, excludedFromCapture: Bool = true) {
        self.tokens = tokens
        defaultNotice = excludedFromCapture
            ? "These controls won’t show in recordings"
            : "These controls will show in recordings · Use Hide controls to keep them out"
        pauseButton = CaptureButton("Ⅱ", frame: .zero, tokens: tokens, glass: true) {}
        microphoneButton = CaptureButton("", frame: .zero, tokens: tokens, glass: true) {}
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color(RecordingHUDColorToken.glassStrong.rawValue).cgColor
        layer?.cornerRadius = tokens.number("r-xl")
        layer?.borderWidth = 1
        layer?.borderColor = tokens.color(RecordingHUDColorToken.glassBorder.rawValue).cgColor
        layer?.shadowColor = NSColor.black.cgColor; layer?.shadowOpacity = 0.44
        layer?.shadowRadius = 22; layer?.shadowOffset = NSSize(width: 0, height: -8)
        setAccessibilityRole(.group); setAccessibilityLabel("Recording controls")

        noticeLabel.stringValue = defaultNotice
        noticeLabel.frame = NSRect(x: 16, y: 8, width: 398, height: 18)
        noticeLabel.alignment = .center
        noticeLabel.font = .systemFont(ofSize: tokens.number("text-2xs"), weight: .medium)
        noticeLabel.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        noticeLabel.lineBreakMode = .byTruncatingTail
        addSubview(noticeLabel)

        statusDot.frame = NSRect(x: 18, y: 44, width: 9, height: 9)
        statusDot.wantsLayer = true; statusDot.layer?.cornerRadius = 4.5
        addSubview(statusDot)
        timerLabel.frame = NSRect(x: 34, y: 31, width: 68, height: 30)
        timerLabel.font = .monospacedDigitSystemFont(ofSize: 22, weight: .semibold)
        timerLabel.textColor = tokens.color(RecordingHUDColorToken.glassText.rawValue); addSubview(timerLabel)
        statusLabel.frame = NSRect(x: 34, y: 60, width: 76, height: 16)
        statusLabel.font = .systemFont(ofSize: 9, weight: .semibold)
        statusLabel.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        addSubview(statusLabel)

        let stop = hudButton("■", x: 104, help: "Stop and save") { [weak self] in self?.stop() }
        stop.signal = true; stop.setAccessibilityLabel("Stop recording")
        pauseButton.frame = NSRect(x: 144, y: 39, width: 38, height: 34)
        pauseButton.actionBlock = { [weak self] in self?.pauseOrResume() }
        pauseButton.setAccessibilityLabel("Pause recording"); addSubview(pauseButton)
        let restart = hudButton("↻", x: 184, help: "Restart recording") {
            [weak self] in self?.restart()
        }
        restart.setAccessibilityLabel("Restart recording")
        let screenshot = hudButton("⌗", x: 224, help: "Take a region screenshot") {
            [weak self] in self?.screenshot()
        }
        screenshot.setAccessibilityLabel("Take a region screenshot")
        meterTrack.frame = NSRect(x: 267, y: 58, width: 32, height: 6)
        meterTrack.wantsLayer = true
        meterTrack.layer?.backgroundColor = tokens.color(RecordingHUDColorToken.glassActive.rawValue).cgColor
        meterTrack.layer?.cornerRadius = 3
        meterFill.frame = NSRect(x: 0, y: 0, width: 0, height: 6)
        meterFill.wantsLayer = true
        meterFill.layer?.backgroundColor = tokens.color(RecordingHUDColorToken.glassText.rawValue).cgColor
        meterFill.layer?.cornerRadius = 3
        meterFill.setAccessibilityElement(false)
        meterTrack.addSubview(meterFill); addSubview(meterTrack)
        meterLabel.frame = NSRect(x: 262, y: 39, width: 42, height: 16)
        meterLabel.alignment = .center
        meterLabel.font = .systemFont(ofSize: 9, weight: .medium)
        meterLabel.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        meterLabel.setAccessibilityElement(false)
        addSubview(meterLabel)
        meterTrack.setAccessibilityElement(true)
        meterTrack.setAccessibilityRole(.progressIndicator)
        meterTrack.setAccessibilityLabel("Microphone level")
        microphoneButton.frame = NSRect(x: 304, y: 39, width: 38, height: 34)
        microphoneButton.actionBlock = { [weak self] in self?.toggleMicrophone() }
        addSubview(microphoneButton)
        let trash = hudButton("⌫", x: 344, help: "Delete recording") { [weak self] in self?.discard() }
        trash.setAccessibilityLabel("Delete recording")
        lifecycleButtons = [stop, pauseButton, restart, screenshot, microphoneButton, trash]
        let hide = hudButton("◉̸", x: 384, help: "Hide controls") {
            [weak self] in self?.hide()
        }
        hide.setAccessibilityLabel("Hide recording controls")
        lifecycleButtons.append(hide)
        for case let button as CaptureButton in subviews { button.hudControl = true }
        setPaused(false, elapsedMilliseconds: 0)
        setMicrophone(muted: false, available: false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private func hudButton(_ title: String, x: CGFloat, help: String,
                           action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: NSRect(x: x, y: 39, width: 38, height: 34),
            tokens: tokens, glass: true, action: action)
        button.toolTip = help; addSubview(button); return button
    }

    func setPaused(_ paused: Bool, elapsedMilliseconds: UInt64) {
        if self.paused != paused { setMicrophoneLevel(0) }
        self.paused = paused; self.elapsedMilliseconds = elapsedMilliseconds
        resumedAt = paused ? nil : Date()
        let status = recordingStatusLabel(paused: paused)
        // Shipping CSS uppercases the status label; assistive tech reads its copy.
        statusLabel.stringValue = status.uppercased(); statusLabel.setAccessibilityLabel(status)
        updateMeterLabel()
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

    func setMicrophone(muted: Bool, available: Bool) {
        if microphoneMuted != muted || microphoneAvailable != available { setMicrophoneLevel(0) }
        microphoneMuted = muted
        microphoneAvailable = available
        microphoneButton.icon = .microphone(muted: muted)
        microphoneButton.selected = muted
        microphoneButton.isEnabled = available && lifecycleActionsEnabled
        let action = muted ? "Unmute microphone" : "Mute microphone"
        let unavailable = "Microphone unavailable because no microphone was selected"
        microphoneButton.toolTip = available ? action : unavailable
        microphoneButton.setAccessibilityLabel(available ? action : unavailable)
        microphoneButton.setAccessibilityValue(muted ? 1 : 0)
        microphoneButton.needsDisplay = true
        updateMeterLabel()
    }

    func setMicrophoneLevel(_ peak: Double) {
        let level = !paused && !microphoneMuted && microphoneAvailable && lifecycleActionsEnabled
            ? min(1, max(0, peak.isFinite ? peak : 0)) : 0
        meterLevel = level
        meterFill.frame.size.width = meterTrack.bounds.width * level
        updateMeterLabel()
    }

    private func updateMeterLabel() {
        let state = !microphoneAvailable ? "off" : paused ? "paused"
            : microphoneMuted ? "muted" : !lifecycleActionsEnabled ? "busy" : "recording"
        meterLabel.stringValue = state == "recording" ? "\(Int((meterLevel * 100).rounded()))%" : state.uppercased()
        let value = "\(Int((meterLevel * 100).rounded()))%, \(state)"
        meterTrack.setAccessibilityValue(value)
        meterTrack.toolTip = "Microphone level: \(value)"
    }

    func setLifecycleActionsEnabled(_ enabled: Bool) {
        lifecycleActionsEnabled = enabled
        if !enabled { setMicrophoneLevel(0) }
        lifecycleButtons.forEach {
            $0.isEnabled = enabled
            $0.needsDisplay = true
        }
        microphoneButton.isEnabled = enabled && microphoneAvailable
        updateMeterLabel()
    }

    private func updateTimer() {
        let running = resumedAt.map { UInt64(max(0, Date().timeIntervalSince($0)) * 1_000) } ?? 0
        let text = formatRecordingTime(milliseconds: elapsedMilliseconds + running)
        guard timerLabel.stringValue != text else { return }
        // h:mm:ss must fit the fixed status column beside Stop.
        timerLabel.font = .monospacedDigitSystemFont(ofSize: text.count > 5 ? 17 : 22, weight: .semibold)
        timerLabel.stringValue = text
    }

    deinit { timer?.invalidate() }
}

/// Shipping `recordingStatusLabel` copy for the states this HUD renders.
func recordingStatusLabel(paused: Bool) -> String { paused ? "Paused" : "Recording" }

/// Mirrors `captures_app::recording_timeline::format_recording_time` and the
/// shipping `formatRecordingTime`: m:ss below an hour, h:mm:ss from one hour.
func formatRecordingTime(milliseconds: UInt64) -> String {
    let total = milliseconds / 1_000
    let hours = total / 3_600
    let minutes = total % 3_600 / 60
    let seconds = total % 60
    let padded: (UInt64) -> String = { $0 < 10 ? "0\($0)" : "\($0)" }
    return hours > 0 ? "\(hours):\(padded(minutes)):\(padded(seconds))" : "\(minutes):\(padded(seconds))"
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

final class RecordingControlsHiddenNoticeView: NSView {
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color(RecordingHUDColorToken.glassStrong.rawValue).cgColor
        layer?.cornerRadius = tokens.number("r-xl")
        layer?.borderWidth = 1
        layer?.borderColor = tokens.color(RecordingHUDColorToken.glassBorder.rawValue).cgColor
        layer?.shadowColor = NSColor.black.cgColor
        layer?.shadowOpacity = 0.4
        layer?.shadowRadius = 18
        layer?.shadowOffset = NSSize(width: 0, height: -6)
        setAccessibilityRole(.group)
        setAccessibilityLabel("Recording controls hidden")

        let title = NSTextField(labelWithString: "Recording controls hidden")
        title.frame = NSRect(x: 20, y: 14, width: 320, height: 22)
        title.alignment = .center
        title.font = .systemFont(ofSize: 15, weight: .semibold)
        title.textColor = tokens.color(RecordingHUDColorToken.glassText.rawValue)
        addSubview(title)

        let detail = NSTextField(wrappingLabelWithString:
            "Open Captures from the menu bar, reactivate the app, or press New Capture to bring them back.")
        detail.frame = NSRect(x: 20, y: 40, width: 320, height: 42)
        detail.alignment = .center
        detail.font = .systemFont(ofSize: 11, weight: .medium)
        detail.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        addSubview(detail)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
}

final class RecordingControlsHiddenNoticePanel: NSPanel {
    init(screen: NSScreen, tokens: Tokens) {
        let size = NSSize(width: 360, height: 96)
        let visible = screen.visibleFrame
        let origin = NSPoint(x: visible.midX - size.width / 2, y: visible.minY + 23)
        super.init(contentRect: NSRect(origin: origin, size: size), styleMask: [.borderless],
            backing: .buffered, defer: false)
        title = "Recording controls hidden"
        isReleasedWhenClosed = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none
        ignoresMouseEvents = true
        contentView = RecordingControlsHiddenNoticeView(
            frame: NSRect(origin: .zero, size: size), tokens: tokens)
    }
}
