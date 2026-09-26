import AppKit
import CCapturesSettings

enum RecordingHUDColorToken: String, CaseIterable {
    case glassStrong = "glass-strong"
    case glassBorder = "glass-border"
    case glassActive = "glass-active"
    case glassText = "glass-text"
    case glassTextSubtle = "glass-text-subtle"
    case themeAccent = "theme-accent"
    case themeSignal = "theme-signal"
    case themeSignalSurface = "theme-signal-surface"
    case themeSignalText = "theme-signal-text"
    case info = "info"
}

/// The shared shipping HUD policy (`captures_recording_hud_request_v1`): control
/// states and copy, status label/dot, the inline error line and tooltip placement.
enum RecordingHUDPolicy {
    struct Control: Equatable {
        let id: String
        let label: String
        let tooltip: String
        let enabled: Bool
        let selected: Bool
        let tooltipRightAligned: Bool
    }

    struct View: Equatable {
        let statusLabel: String
        let dotToken: String
        let dotHalo: Bool
        let pulsing: Bool
        let timerRunning: Bool
        let showMeter: Bool
        let restartConfirms: Bool
        let controls: [String: Control]
    }

    static func request(_ object: [String: Any]) -> [String: Any]? {
        guard let data = try? JSONSerialization.data(withJSONObject: object) else { return nil }
        let pointer = String(decoding: data, as: UTF8.self).withCString {
            captures_recording_hud_request_v1($0)
        }
        guard let pointer else { return nil }
        defer { captures_settings_free_v1(pointer) }
        guard let response = try? JSONSerialization.jsonObject(
                with: Data(bytes: pointer, count: strlen(pointer))) as? [String: Any],
              response["ok"] as? Bool == true else { return nil }
        return response["result"] as? [String: Any]
    }

    static func present(state: String, busy: Bool, hasMicrophone: Bool, microphoneMuted: Bool,
                        hideAvailable: Bool = true) -> View? {
        guard let result = request([
            "operation": "present", "state": state, "busy": busy,
            "has_microphone": hasMicrophone, "microphone_muted": microphoneMuted,
            "hide_available": hideAvailable,
        ]), let controls = result["controls"] as? [[String: Any]] else { return nil }
        var byID: [String: Control] = [:]
        for control in controls {
            guard let id = control["control"] as? String else { return nil }
            byID[id] = Control(id: id, label: control.string("label"),
                tooltip: control.string("tooltip"),
                enabled: control["enabled"] as? Bool == true,
                selected: control["selected"] as? Bool == true,
                tooltipRightAligned: control["tooltip_right_aligned"] as? Bool == true)
        }
        return View(statusLabel: result.string("status_label"),
            dotToken: result.string("dot_token", "theme-signal"),
            dotHalo: result["dot_halo"] as? Bool != false,
            pulsing: result["pulsing"] as? Bool == true,
            timerRunning: result["timer_running"] as? Bool == true,
            showMeter: result["show_meter"] as? Bool == true,
            restartConfirms: result["restart_confirms"] as? Bool != false,
            controls: byID)
    }

    /// Apply one error-line event; returns the new state and the text to show.
    static func errorLine(_ line: [String: Any], event: [String: Any],
                          sessionError: String?) -> (line: [String: Any], text: String?) {
        var object: [String: Any] = ["operation": "error_line", "line": line, "event": event]
        if let sessionError { object["session_error"] = sessionError }
        guard let result = request(object), let next = result["line"] as? [String: Any] else {
            return (line, nil)
        }
        return (next, result["text"] as? String)
    }

    /// Flipped HUD coordinates in and out.
    static func tooltipFrame(anchor: NSRect, textSize: NSSize, rightAligned: Bool,
                             progress: Double, bounds: NSRect) -> NSRect? {
        func rect(_ value: NSRect) -> [String: Double] {
            ["x": Double(value.minX), "y": Double(value.minY),
             "width": Double(value.width), "height": Double(value.height)]
        }
        guard let result = request([
            "operation": "tooltip_frame", "anchor": rect(anchor),
            "text_width": Double(textSize.width), "text_height": Double(textSize.height),
            "right_aligned": rightAligned, "progress": progress, "bounds": rect(bounds),
        ]), let x = result["x"] as? Double, let y = result["y"] as? Double,
              let width = result["width"] as? Double, let height = result["height"] as? Double
        else { return nil }
        return NSRect(x: x, y: y, width: width, height: height)
    }
}

/// Shipping `.recording-tooltip > [role="tooltip"]`: a fixed-glass pill with an
/// xs medium label. It never takes the mouse.
final class RecordingHUDTooltipView: NSView {
    let label = NSTextField(labelWithString: "")
    override var isFlipped: Bool { true }

    init(tokens: Tokens) {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = tokens.color(RecordingHUDColorToken.glassStrong.rawValue).cgColor
        layer?.borderColor = tokens.color(RecordingHUDColorToken.glassBorder.rawValue).cgColor
        layer?.borderWidth = 1
        layer?.cornerRadius = tokens.number("r-sm")
        label.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        label.textColor = tokens.color(RecordingHUDColorToken.glassText.rawValue)
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
        label.frame = bounds.insetBy(dx: 9, dy: 6)
    }
}

final class RecordingHUDView: NSView {
    private let tokens: Tokens
    private let defaultNotice: String
    private let statusDot = NSView()
    private let timerLabel = NSTextField(labelWithString: "0:00")
    private let statusLabel = NSTextField(labelWithString: "")
    private let noticeLabel = NSTextField(labelWithString: "These controls won’t show in recordings")
    /// Shipping `.recording-hud-error`: one signal line below the controls.
    private let errorLabel = NSTextField(labelWithString: "")
    private let tooltipView: RecordingHUDTooltipView
    private let stopButton: CaptureButton
    private let pauseButton: CaptureButton
    private let restartButton: CaptureButton
    private let screenshotButton: CaptureButton
    private let microphoneButton: CaptureButton
    private let trashButton: CaptureButton
    private let hideButton: CaptureButton
    private let meterTrack = NSView()
    private let meterFill = NSView()
    private let meterLabel = NSTextField(labelWithString: "OFF")
    private var meterLevel = 0.0
    private var lifecycleButtons: [CaptureButton] = []
    private var controlIDs: [ObjectIdentifier: String] = [:]
    private var tooltips: [ObjectIdentifier: (text: String, rightAligned: Bool)] = [:]
    private weak var tooltipOwner: CaptureButton?
    private var lifecycleActionsEnabled = true
    private var elapsedMilliseconds: UInt64 = 0
    private var resumedAt: Date?
    private var timer: Timer?
    private var errorState: [String: Any] = [:]
    private var sessionError: String?
    /// "recording", "paused", "finalizing" or "failed" (shared RecordingState names).
    private(set) var state = "recording"
    private(set) var paused = false
    private(set) var microphoneMuted = false
    private(set) var microphoneAvailable = false
    private(set) var policy: RecordingHUDPolicy.View?
    private(set) var errorText: String?
    var pauseOrResume: () -> Void = {}
    var toggleMicrophone: () -> Void = {}
    var restart: () -> Void = {}
    var screenshot: () -> Void = {}
    var stop: () -> Void = {}
    var discard: () -> Void = {}
    var hide: () -> Void = {}

    override var isFlipped: Bool { true }

    /// A failed take retries at once; a running take asks first (shared policy).
    var restartConfirms: Bool { policy?.restartConfirms ?? true }
    /// The styled tooltip currently shown, for tests and accessibility checks.
    var visibleTooltip: String? { tooltipView.isHidden ? nil : tooltipView.label.stringValue }

    init(frame: NSRect, tokens: Tokens, excludedFromCapture: Bool = true) {
        self.tokens = tokens
        defaultNotice = excludedFromCapture
            ? "These controls won’t show in recordings"
            : "These controls will show in recordings · Use Hide controls to keep them out"
        func button(_ icon: CaptureButtonIcon, x: CGFloat) -> CaptureButton {
            let button = CaptureButton("", frame: NSRect(x: x, y: 39, width: 38, height: 34),
                tokens: tokens, glass: true) {}
            button.icon = icon
            return button
        }
        stopButton = button(.stopSquare, x: 104)
        pauseButton = button(.shipping("pause"), x: 144)
        restartButton = button(.shipping("restart"), x: 184)
        screenshotButton = button(.shipping("capture"), x: 224)
        microphoneButton = button(.shipping("microphone"), x: 304)
        trashButton = button(.shipping("trash"), x: 344)
        hideButton = button(.shipping("hide-controls"), x: 384)
        tooltipView = RecordingHUDTooltipView(tokens: tokens)
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color(RecordingHUDColorToken.glassStrong.rawValue).cgColor
        layer?.cornerRadius = tokens.number("r-xl")
        layer?.borderWidth = 1
        layer?.borderColor = tokens.color(RecordingHUDColorToken.glassBorder.rawValue).cgColor
        layer?.shadowColor = NSColor.black.cgColor; layer?.shadowOpacity = 0.44
        layer?.shadowRadius = 22; layer?.shadowOffset = NSSize(width: 0, height: -8)
        setAccessibilityRole(.group); setAccessibilityLabel("Recording controls")

        noticeLabel.frame = NSRect(x: 16, y: 8, width: 398, height: 18)
        noticeLabel.alignment = .center
        noticeLabel.font = .systemFont(ofSize: tokens.number("text-2xs"), weight: .medium)
        noticeLabel.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        noticeLabel.lineBreakMode = .byTruncatingTail
        addSubview(noticeLabel)
        showNotice(defaultNotice)

        statusDot.frame = NSRect(x: 17, y: 43, width: 10, height: 10)
        statusDot.wantsLayer = true; statusDot.layer?.cornerRadius = 5
        // Shipping `box-shadow: 0 0 0 4px rgba(signal, 0.16)` halo.
        statusDot.layer?.borderWidth = 0; statusDot.layer?.masksToBounds = false
        statusDot.layer?.shadowOpacity = 1; statusDot.layer?.shadowRadius = 0
        statusDot.layer?.shadowOffset = .zero
        statusDot.layer?.shadowPath = CGPath(ellipseIn: CGRect(x: -4, y: -4, width: 18, height: 18),
                                             transform: nil)
        addSubview(statusDot)
        timerLabel.frame = NSRect(x: 34, y: 31, width: 68, height: 30)
        timerLabel.font = .monospacedDigitSystemFont(ofSize: 22, weight: .semibold)
        timerLabel.textColor = tokens.color(RecordingHUDColorToken.glassText.rawValue); addSubview(timerLabel)
        statusLabel.frame = NSRect(x: 34, y: 60, width: 76, height: 16)
        statusLabel.font = .systemFont(ofSize: 9, weight: .semibold)
        statusLabel.textColor = tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue)
        addSubview(statusLabel)

        stopButton.signal = true
        stopButton.actionBlock = { [weak self] in self?.stop() }
        pauseButton.actionBlock = { [weak self] in self?.pauseOrResume() }
        restartButton.actionBlock = { [weak self] in self?.restart() }
        screenshotButton.actionBlock = { [weak self] in self?.screenshot() }
        microphoneButton.actionBlock = { [weak self] in self?.toggleMicrophone() }
        trashButton.actionBlock = { [weak self] in self?.discard() }
        hideButton.actionBlock = { [weak self] in self?.hide() }
        for (button, id) in [(stopButton, "stop"), (pauseButton, "pause_resume"),
                             (restartButton, "restart"), (screenshotButton, "screenshot"),
                             (microphoneButton, "microphone"), (trashButton, "delete"),
                             (hideButton, "hide")] {
            controlIDs[ObjectIdentifier(button)] = id
            button.highlightChanged = { [weak self] button, highlighted in
                self?.setTooltip(for: button, visible: highlighted)
            }
            addSubview(button)
        }
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
        lifecycleButtons = [stopButton, pauseButton, restartButton, screenshotButton,
                            microphoneButton, trashButton, hideButton]
        for case let button as CaptureButton in subviews { button.hudControl = true }

        // Shipping `.recording-hud-error`: 2xs signal text, one line, ellipsis.
        errorLabel.frame = NSRect(x: 16, y: 79, width: 398, height: 15)
        errorLabel.alignment = .center
        errorLabel.font = .systemFont(ofSize: tokens.number("text-2xs"), weight: .medium)
        errorLabel.textColor = tokens.color(RecordingHUDColorToken.themeSignalText.rawValue)
        errorLabel.lineBreakMode = .byTruncatingTail
        errorLabel.maximumNumberOfLines = 1
        errorLabel.isHidden = true
        errorLabel.setAccessibilityRole(.staticText)
        addSubview(errorLabel)
        addSubview(tooltipView)
        setPaused(false, elapsedMilliseconds: 0)
        setMicrophone(muted: false, available: false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    /// Shipping `recording-pulse`: 1.6 s, opacity 0.6 and scale 0.84 at the midpoint.
    private func setDotPulsing(_ pulsing: Bool) {
        guard let layer = statusDot.layer else { return }
        let wanted = pulsing && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        // Status polls refresh the HUD every second; keep a running pulse in phase.
        guard wanted != (layer.animation(forKey: "recording-pulse") != nil) else { return }
        layer.removeAnimation(forKey: "recording-pulse")
        guard wanted else { return }
        let opacity = CAKeyframeAnimation(keyPath: "opacity")
        opacity.values = [1, 0.6, 1]
        let scale = CAKeyframeAnimation(keyPath: "transform.scale")
        scale.values = [1, 0.84, 1]
        let group = CAAnimationGroup()
        group.animations = [opacity, scale]
        group.duration = 1.6; group.repeatCount = .infinity
        group.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        layer.add(group, forKey: "recording-pulse")
    }

    var dotPulsing: Bool { statusDot.layer?.animation(forKey: "recording-pulse") != nil }

    func setPaused(_ paused: Bool, elapsedMilliseconds: UInt64) {
        setState(paused ? "paused" : "recording", elapsedMilliseconds: elapsedMilliseconds)
    }

    /// Shipping keeps the HUD up as "Saving…" with a frozen timer while finalizing.
    func setSaving() {
        let running = resumedAt.map { UInt64(max(0, Date().timeIntervalSince($0)) * 1_000) } ?? 0
        setState("finalizing", elapsedMilliseconds: elapsedMilliseconds + running)
    }

    /// The engine could not start the take: Retry recording and Delete remain.
    func setFailed(error: String, sessionError: String?, elapsedMilliseconds: UInt64) {
        self.sessionError = sessionError
        setState("failed", elapsedMilliseconds: elapsedMilliseconds)
        applyError(["event": "action_failed", "message": error])
    }

    private func setState(_ state: String, elapsedMilliseconds: UInt64) {
        let paused = state == "paused"
        if self.paused != paused || self.state != state { setMicrophoneLevel(0) }
        if state != "failed" { sessionError = nil }
        self.state = state; self.paused = paused
        self.elapsedMilliseconds = elapsedMilliseconds
        refreshControls()
        let running = policy?.timerRunning ?? (state == "recording")
        resumedAt = running ? Date() : nil
        pauseButton.icon = .shipping(paused ? "resume" : "pause")
        updateTimer()
        timer?.invalidate()
        guard running else { timer = nil; applyError(nil); return }
        let timer = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in self?.updateTimer() }
        self.timer = timer; RunLoop.main.add(timer, forMode: .common)
        applyError(nil)
    }

    /// Re-evaluate every control from the shared shipping policy.
    private func refreshControls() {
        guard let view = RecordingHUDPolicy.present(state: state, busy: !lifecycleActionsEnabled,
            hasMicrophone: microphoneAvailable, microphoneMuted: microphoneMuted) else { return }
        policy = view
        // Shipping CSS uppercases the status label; assistive tech reads its copy.
        statusLabel.stringValue = view.statusLabel.uppercased()
        statusLabel.setAccessibilityLabel(view.statusLabel)
        let dot = tokens.color(view.dotToken)
        statusDot.layer?.backgroundColor = dot.cgColor
        statusDot.layer?.shadowColor = view.dotHalo ? dot.withAlphaComponent(0.16).cgColor : nil
        setDotPulsing(view.pulsing)
        for button in lifecycleButtons {
            guard let id = controlIDs[ObjectIdentifier(button)], let control = view.controls[id] else {
                continue
            }
            button.isEnabled = control.enabled
            button.selected = control.selected
            button.setAccessibilityLabel(control.label)
            // The styled tooltip replaces the delayed system tooltip.
            button.toolTip = nil
            tooltips[ObjectIdentifier(button)] = (control.tooltip, control.tooltipRightAligned)
            button.needsDisplay = true
        }
        microphoneButton.setAccessibilityValue(microphoneMuted ? 1 : 0)
        if let owner = tooltipOwner { setTooltip(for: owner, visible: true, animated: false) }
    }

    /// Shipping shows the tooltip under a hovered or focused button, disabled ones
    /// included, with no delay; it fades and slides 3 pt over `--dur-1`.
    private func setTooltip(for button: CaptureButton, visible: Bool, animated: Bool = true) {
        // Offscreen (tests, hidden panels) there is nothing to animate.
        let duration = animated && window?.isVisible == true
            ? Double(tokens.number("dur-1")) / 1000 : 0
        guard visible, let tooltip = tooltips[ObjectIdentifier(button)] else {
            guard tooltipOwner === button else { return }
            tooltipOwner = nil
            guard duration > 0 else {
                tooltipView.alphaValue = 0; tooltipView.isHidden = true
                return
            }
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = duration
                tooltipView.animator().alphaValue = 0
            }, completionHandler: { [weak self] in
                guard let self, self.tooltipOwner == nil else { return }
                self.tooltipView.isHidden = true
            })
            return
        }
        tooltipOwner = button
        tooltipView.label.stringValue = tooltip.text
        let textSize = tooltipView.label.intrinsicContentSize
        guard let hidden = RecordingHUDPolicy.tooltipFrame(anchor: button.frame, textSize: textSize,
                  rightAligned: tooltip.rightAligned, progress: 0, bounds: bounds),
              let shown = RecordingHUDPolicy.tooltipFrame(anchor: button.frame, textSize: textSize,
                  rightAligned: tooltip.rightAligned, progress: 1, bounds: bounds) else { return }
        let wasHidden = tooltipView.isHidden || tooltipView.alphaValue == 0
        tooltipView.isHidden = false
        tooltipView.needsLayout = true
        guard duration > 0 else {
            tooltipView.frame = shown; tooltipView.alphaValue = 1
            return
        }
        if wasHidden { tooltipView.frame = hidden; tooltipView.alphaValue = 0 }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = duration
            tooltipView.animator().frame = shown
            tooltipView.animator().alphaValue = 1
        }
    }

    /// Engine warnings (microphone or desktop audio) appear on the inline error
    /// line once per change, like shipping's `recording-warning` event.
    func setWarning(_ warning: String?) {
        let value: Any = warning.map { $0 as Any } ?? NSNull()
        applyError(["event": "warning", "warning": value])
    }

    /// A HUD action failed; shipping shows it on the inline error line.
    func showActionError(_ message: String) {
        applyError(["event": "action_failed", "message": message])
    }

    /// Shipping clears the error line whenever a HUD action starts.
    func actionStarted() { applyError(["event": "action_started"]) }

    /// A new take starts with no error line.
    func resetErrors() { sessionError = nil; applyError(["event": "session_changed"]) }

    private func applyError(_ event: [String: Any]?) {
        if let event {
            let result = RecordingHUDPolicy.errorLine(errorState, event: event,
                sessionError: sessionError)
            errorState = result.line
            errorText = result.text
        } else {
            errorText = (errorState["message"] as? String) ?? sessionError.flatMap { error in
                RecordingHUDPolicy.request(["operation": "error_message", "message": error])?
                    .string("message")
            }
        }
        errorLabel.stringValue = errorText ?? ""
        errorLabel.setAccessibilityLabel(errorText)
        errorLabel.isHidden = errorText == nil
    }

    /// Shipping `.recording-hud-privacy`: subtle 2xs text with **will**/**won’t** in bold glass text.
    private func showNotice(_ text: String) {
        let size = tokens.number("text-2xs")
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = .center; paragraph.lineBreakMode = .byTruncatingTail
        let attributed = NSMutableAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: size, weight: .medium),
            .foregroundColor: tokens.color(RecordingHUDColorToken.glassTextSubtle.rawValue),
            .paragraphStyle: paragraph,
        ])
        if let range = ["won’t", "will"].lazy
            .map({ (text as NSString).range(of: " \($0) ") })
            .first(where: { $0.location != NSNotFound }) {
            attributed.addAttributes([
                .font: NSFont.systemFont(ofSize: size, weight: .bold),
                .foregroundColor: tokens.color(RecordingHUDColorToken.glassText.rawValue),
            ], range: NSRange(location: range.location + 1, length: range.length - 2))
        }
        noticeLabel.attributedStringValue = attributed
        noticeLabel.setAccessibilityLabel(text)
    }

    func setMicrophone(muted: Bool, available: Bool) {
        if microphoneMuted != muted || microphoneAvailable != available { setMicrophoneLevel(0) }
        microphoneMuted = muted
        microphoneAvailable = available
        microphoneButton.icon = .shipping(muted ? "microphone-muted" : "microphone")
        // Shipping hides the level meter entirely without a selected microphone.
        meterTrack.isHidden = !available; meterLabel.isHidden = !available
        refreshControls()
        updateMeterLabel()
    }

    func setMicrophoneLevel(_ peak: Double) {
        let level = state == "recording" && !microphoneMuted && microphoneAvailable
            && lifecycleActionsEnabled
            ? min(1, max(0, peak.isFinite ? peak : 0)) : 0
        meterLevel = level
        meterFill.frame.size.width = meterTrack.bounds.width * level
        updateMeterLabel()
    }

    private func updateMeterLabel() {
        let state = !microphoneAvailable ? "off" : paused ? "paused"
            : microphoneMuted ? "muted"
            : !lifecycleActionsEnabled || self.state != "recording" ? "busy" : "recording"
        meterLabel.stringValue = state == "recording" ? "\(Int((meterLevel * 100).rounded()))%" : state.uppercased()
        let value = "\(Int((meterLevel * 100).rounded()))%, \(state)"
        meterTrack.setAccessibilityValue(value)
        meterTrack.toolTip = "Microphone level: \(value)"
    }

    func setLifecycleActionsEnabled(_ enabled: Bool) {
        lifecycleActionsEnabled = enabled
        if !enabled { setMicrophoneLevel(0) }
        refreshControls()
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

/// Shipping `recordingStatusLabel` copy for the running states (shared policy
/// supplies the rest).
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
        // Shipping `startHudDrag`: the HUD background moves the window; buttons still click.
        isMovableByWindowBackground = true
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
