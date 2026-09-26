import AppKit
import CCapturesSettings

/// Shipping New Capture copy and presentation policy from
/// `captures-app::capture_menu`, shared with the wgpu host.
enum CaptureMenuPolicy {
    struct Note: Equatable {
        let lead: String
        let emphasis: String
        let trail: String
        let hint: String
        /// Preferences setting the note links to, or nil for plain text.
        let setting: String?
        var text: String { lead + emphasis + trail + hint }
    }

    struct Menu: Equatable {
        let note: Note
        let confirmText: String
        let confirmSetting: String?
        let primaryLabel: String
        let primaryAccessibilityLabel: String
        let primaryHidden: Bool
        /// Toggle key ("show_cursor", "highlight_clicks", "system_audio") → On/Off/Unavailable.
        let toggleStatus: [String: String]
    }

    struct Toggle: Equatable {
        let key: String
        let label: String
        let accessibilityLabel: String
        let unavailableReason: String
    }

    struct Guidance: Equatable {
        let title: String
        let hint: String
    }

    struct MicrophoneEntry: Equatable {
        let id: String?
        let label: String
        let enabled: Bool
    }

    struct Copy {
        let fpsLabel: String
        let fpsAccessibilityLabel: String
        let maxResolutionLabel: String
        let maxResolutionAccessibilityLabel: String
        let microphoneLabel: String
        let fpsOptions: [Int]
        let resolutionValues: [String]
        let resolutionLabels: [String]
        let toggles: [Toggle]
        let guidance: [String: Guidance]
        let separator: String
        let confirm: String
        let autoStart: String
        let highlightSeconds: TimeInterval
    }

    static func request(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_capture_menu_v1($0)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
    }

    static let copy: Copy = {
        do { return try CaptureMenuPolicy.loadCopy() } catch {
            preconditionFailure("Capture menu copy is unavailable: \(error)")
        }
    }()

    private static func loadCopy() throws -> Copy {
        let value = try request(["operation": "copy"])
        guard let fields = value["fields"] as? [String: Any],
              let fps = fields["fps"] as? String,
              let fpsAccessibility = fields["fps_accessibility_label"] as? String,
              let resolution = fields["max_resolution"] as? String,
              let resolutionAccessibility = fields["max_resolution_accessibility_label"] as? String,
              let microphone = fields["microphone"] as? String,
              let fpsOptions = value["fps_options"] as? [Int],
              let resolutions = value["resolution_options"] as? [[String: Any]],
              let toggles = value["toggles"] as? [[String: Any]],
              let guidance = value["guidance"] as? [String: [String: Any]],
              let separator = value["separator"] as? String,
              let confirm = value["confirm"] as? String,
              let autoStart = value["auto_start"] as? String,
              let highlight = value["highlight_ms"] as? Double
        else { throw AppBridgeError.invalidResponse }
        let decodedToggles = toggles.compactMap { toggle -> Toggle? in
            guard let key = toggle["key"] as? String, let label = toggle["label"] as? String,
                  let accessibility = toggle["accessibility_label"] as? String,
                  let reason = toggle["unavailable_reason"] as? String else { return nil }
            return Toggle(key: key, label: label, accessibilityLabel: accessibility,
                unavailableReason: reason)
        }
        var decodedGuidance: [String: Guidance] = [:]
        for (key, entry) in guidance {
            guard let title = entry["title"] as? String, let hint = entry["hint"] as? String else {
                throw AppBridgeError.invalidResponse
            }
            decodedGuidance[key] = Guidance(title: title, hint: hint)
        }
        let values = resolutions.compactMap { $0["value"] as? String }
        let labels = resolutions.compactMap { $0["label"] as? String }
        guard decodedToggles.count == toggles.count, values.count == resolutions.count,
              labels.count == resolutions.count else { throw AppBridgeError.invalidResponse }
        return Copy(fpsLabel: fps, fpsAccessibilityLabel: fpsAccessibility,
            maxResolutionLabel: resolution, maxResolutionAccessibilityLabel: resolutionAccessibility,
            microphoneLabel: microphone, fpsOptions: fpsOptions, resolutionValues: values,
            resolutionLabels: labels, toggles: decodedToggles, guidance: decodedGuidance,
            separator: separator, confirm: confirm, autoStart: autoStart,
            highlightSeconds: highlight / 1000)
    }

    static func guidance(_ key: String) -> Guidance {
        guard let guidance = copy.guidance[key] else {
            preconditionFailure("Missing capture guidance \(key)")
        }
        return guidance
    }

    static func menu(mode: UnifiedCaptureMode, autoStart: Bool, canExcludeControls: Bool,
                     controlsExcluded: Bool, error: Bool = false,
                     state: RecordingControlState,
                     availability: RecordingControlAvailability?) throws -> Menu {
        let value = try request([
            "operation": "menu", "mode": mode == .record ? "recording" : "screenshot",
            "auto_start": autoStart, "can_exclude_controls": canExcludeControls,
            "controls_excluded": controlsExcluded, "state": ["error": error],
            "options": ["show_cursor": state.showCursor, "highlight_clicks": state.highlightClicks,
                        "system_audio": state.systemAudio],
            "available": ["cursor_control": availability?.cursor ?? false,
                          "click_highlights": availability?.clicks ?? false,
                          "system_audio": availability?.systemAudio ?? false],
        ])
        guard let note = value["note"] as? [String: Any],
              let lead = note["lead"] as? String, let emphasis = note["emphasis"] as? String,
              let trail = note["trail"] as? String,
              let confirm = value["confirm"] as? [String: Any],
              let confirmText = confirm["text"] as? String,
              let primary = value["primary"] as? [String: Any],
              let label = primary["label"] as? String,
              let accessibility = primary["accessibility_label"] as? String,
              let hidden = primary["hidden"] as? Bool,
              let toggles = value["toggles"] as? [[String: Any]]
        else { throw AppBridgeError.invalidResponse }
        var status: [String: String] = [:]
        for toggle in toggles {
            guard let key = toggle["key"] as? String, let text = toggle["status"] as? String else {
                throw AppBridgeError.invalidResponse
            }
            status[key] = text
        }
        return Menu(note: Note(lead: lead, emphasis: emphasis, trail: trail,
                hint: note["hint"] as? String ?? "",
                setting: (note["target"] as? [String: Any])?["setting"] as? String),
            confirmText: confirmText,
            confirmSetting: (confirm["target"] as? [String: Any])?["setting"] as? String,
            primaryLabel: label, primaryAccessibilityLabel: accessibility, primaryHidden: hidden,
            toggleStatus: status)
    }

    /// Shipping cursor/clicks coupling after `changed` flipped.
    static func coupled(changed: String, showCursor: Bool, highlightClicks: Bool) throws
        -> (showCursor: Bool, highlightClicks: Bool) {
        let value = try request(["operation": "toggle", "changed": changed,
            "show_cursor": showCursor, "highlight_clicks": highlightClicks])
        guard let cursor = value["show_cursor"] as? Bool,
              let clicks = value["highlight_clicks"] as? Bool else {
            throw AppBridgeError.invalidResponse
        }
        return (cursor, clicks)
    }

    static func microphones(available: Bool, loading: Bool, selected: String?,
                            devices: [NativeMicrophoneDevice]) throws
        -> (entries: [MicrophoneEntry], selectedLabel: String) {
        let value = try request([
            "operation": "microphones", "available": available, "loading": loading,
            "selected": selected.map { $0 as Any } ?? NSNull(),
            "devices": devices.map { ["id": $0.id, "name": $0.name] },
        ])
        guard let entries = value["entries"] as? [[String: Any]],
              let label = value["selected_label"] as? String else {
            throw AppBridgeError.invalidResponse
        }
        let decoded = entries.compactMap { entry -> MicrophoneEntry? in
            guard let text = entry["label"] as? String,
                  let enabled = entry["enabled"] as? Bool else { return nil }
            return MicrophoneEntry(id: entry["id"] as? String, label: text, enabled: enabled)
        }
        guard decoded.count == entries.count else { throw AppBridgeError.invalidResponse }
        return (decoded, label)
    }

    static func displayIdentity(name: String, width: Int, height: Int, recordingFPS: Int?) throws
        -> (name: String, detail: String) {
        var request: [String: Any] = ["operation": "display_identity", "name": name,
                                      "width": max(0, width), "height": max(0, height)]
        if let recordingFPS { request["recording_fps"] = recordingFPS }
        let value = try self.request(request)
        guard let title = value["name"] as? String, let detail = value["detail"] as? String else {
            throw AppBridgeError.invalidResponse
        }
        return (title, detail)
    }

    /// Guidance ducking in flipped (top-left) view coordinates.
    static func pointerOverGuidance(_ point: NSPoint, chip: NSRect, currentlyOver: Bool) -> Bool {
        captures_capture_guidance_pointer_over_v1(Double(point.x), Double(point.y),
            Double(chip.minX), Double(chip.minY), Double(chip.maxX), Double(chip.maxY),
            currentlyOver)
    }
}

/// Shipping `CaptureGuidance` copy, shared by New Capture and the direct
/// region/window overlays.
enum CaptureGuidanceCopy {
    static var regionTitle: String { CaptureMenuPolicy.guidance("region").title }
    /// Shipping feedback after a click that selected nothing (1.8 s).
    static var regionFeedbackTitle: String { CaptureMenuPolicy.guidance("region_feedback").title }
    static var regionHint: String { CaptureMenuPolicy.guidance("region").hint }
    static var windowTitle: String { CaptureMenuPolicy.guidance("window").title }
    static var displayTitle: String { CaptureMenuPolicy.guidance("display").title }
    static var hint: String { CaptureMenuPolicy.guidance("window").hint }
    static var confirm: String { CaptureMenuPolicy.copy.confirm }
    static var autoStart: String { CaptureMenuPolicy.copy.autoStart }

    /// Direct overlays commit on release/click; `confirm` is only for fixtures
    /// and the manual selection mode.
    static func directHint(_ title: String, _ hint: String, confirm: Bool) -> String {
        ([title, hint] + (confirm ? [Self.confirm] : [])).joined(separator: " · ")
    }
}

enum UnifiedCaptureTarget: String, CaseIterable {
    case region
    case window
    case display

    var title: String {
        switch self {
        case .region: return "Region"
        case .window: return "Window"
        case .display: return "Full screen"
        }
    }
}

enum UnifiedCaptureMode: String, Equatable {
    case screenshot
    case record
}

struct RecordingControlState: Equatable {
    var framesPerSecond: Int
    var maxResolution: String
    var showCursor: Bool
    var highlightClicks: Bool
    var systemAudio: Bool
    var microphoneDeviceID: String?
}

struct RecordingControlAvailability: Equatable {
    let cursor: Bool
    let clicks: Bool
    let systemAudio: Bool
    let microphone: Bool
}

/// Whether "these controls" are kept out of captures (capability-derived).
struct CaptureControlsVisibility: Equatable {
    let canExclude: Bool
    let excluded: Bool

    static let excludedByDefault = CaptureControlsVisibility(canExclude: true, excluded: true)
}

/// The Full screen target's display, for the shipping identity pill.
struct CaptureDisplayIdentity: Equatable {
    let name: String
    let width: Int
    let height: Int
}

struct UnifiedCaptureControlsState: Equatable {
    var mode: UnifiedCaptureMode
    var target: UnifiedCaptureTarget
    var aspectIndex: Int

    static let initial = UnifiedCaptureControlsState(mode: .screenshot, target: .region, aspectIndex: 0)
}

private final class GlassPopUpButton: NSPopUpButton {
    var tokens: Tokens!
    var change: ((Int) -> Void)?

    @objc private func selectedValue() { change?(indexOfSelectedItem) }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1),
            xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md"))
        tokens.color("glass-raised").setFill(); path.fill()
        tokens.color(window?.firstResponder === self ? "theme-accent" : "glass-border").setStroke()
        path.stroke()
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium),
            .foregroundColor: tokens.color(isEnabled ? "glass-text" : "glass-text-subtle"),
        ]
        (title as NSString).draw(at: NSPoint(x: tokens.number("s-4"), y: 10),
            withAttributes: attributes)
        ("⌄" as NSString).draw(at: NSPoint(x: bounds.width - tokens.number("s-7"), y: 10),
            withAttributes: attributes)
    }

    func bindChange(_ callback: @escaping (Int) -> Void) {
        change = callback; target = self; action = #selector(selectedValue)
    }
}

/// Shipping `recording-toggle`: a 30×18 switch followed by On/Off/Unavailable,
/// with the unavailable reason as its tooltip.
final class RecordingSwitchButton: NSButton {
    private let tokens: Tokens
    let key: String
    var isOn = false { didSet { refresh() } }
    var status = "Off" { didSet { refresh() } }
    var actionBlock: (() -> Void)?
    private var hovered = false
    private var hoverTracking: NSTrackingArea?

    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens, toggle: CaptureMenuPolicy.Toggle, available: Bool) {
        self.tokens = tokens; key = toggle.key
        super.init(frame: frame)
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; action = #selector(activate)
        isEnabled = available
        toolTip = available ? nil : toggle.unavailableReason
        setAccessibilityRole(.checkBox); setAccessibilityLabel(toggle.accessibilityLabel)
        setAccessibilityHelp(available ? nil : toggle.unavailableReason)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    @objc private func activate() { if isEnabled { actionBlock?() } }

    private func refresh() {
        setAccessibilityValue(isOn ? 1 : 0)
        needsDisplay = true
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking); hoverTracking = tracking
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        let track = NSRect(x: 1, y: (bounds.height - 18) / 2, width: 30, height: 18)
        let path = NSBezierPath(roundedRect: track, xRadius: 9, yRadius: 9)
        if isOn {
            tokens.color("theme-accent").setFill(); path.fill()
        } else {
            NSColor.black.withAlphaComponent(0.35).setFill(); path.fill()
            tokens.color("glass-border-strong").setStroke(); path.lineWidth = 1; path.stroke()
        }
        let knob = NSRect(x: track.minX + 3 + (isOn ? 12 : 0), y: track.midY - 6, width: 12, height: 12)
        tokens.color(isOn ? "theme-accent-ink" : "glass-text-subtle").setFill()
        NSBezierPath(ovalIn: knob).fill()
        if window?.firstResponder === self {
            let ring = NSBezierPath(roundedRect: track.insetBy(dx: -2, dy: -2), xRadius: 11, yRadius: 11)
            tokens.color("theme-accent").setStroke(); ring.lineWidth = 1; ring.stroke()
        }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-xs"), weight: .medium),
            .foregroundColor: tokens.color(isEnabled && hovered ? "glass-text" : "glass-text-muted"),
        ]
        let size = (status as NSString).size(withAttributes: attributes)
        (status as NSString).draw(at: NSPoint(x: track.maxX + tokens.number("s-3"),
            y: (bounds.height - size.height) / 2), withAttributes: attributes)
    }
}

/// Shipping `capture-selector-preferences-link`: note text with an external
/// glyph that opens Preferences at the setting it describes.
final class CaptureNoteLink: NSButton {
    private let tokens: Tokens
    private let parts: [(String, Bool)]
    let setting: String
    var actionBlock: (() -> Void)?
    private var hovered = false
    private var hoverTracking: NSTrackingArea?
    static let padding = NSSize(width: 5, height: 2)
    static let icon: CGFloat = 10

    override var isFlipped: Bool { true }

    init(parts: [(String, Bool)], setting: String, tokens: Tokens) {
        self.parts = parts; self.setting = setting; self.tokens = tokens
        super.init(frame: .zero)
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; action = #selector(activate)
        setAccessibilityRole(.link); setAccessibilityLabel(text)
        frame.size = intrinsicContentSize
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var text: String { parts.map(\.0).joined() }
    @objc private func activate() { actionBlock?() }

    private func attributed(active: Bool) -> NSAttributedString {
        let result = NSMutableAttributedString()
        for (text, strong) in parts {
            result.append(NSAttributedString(string: text, attributes: [
                .font: NSFont.systemFont(ofSize: tokens.number("text-xs"),
                    weight: strong ? .bold : .medium),
                .foregroundColor: tokens.color(active ? "theme-accent-text-strong"
                    : strong ? "glass-text" : "glass-text-subtle"),
            ]))
        }
        return result
    }

    override var intrinsicContentSize: NSSize {
        let size = attributed(active: false).size()
        return NSSize(width: ceil(size.width) + Self.padding.width * 2 + Self.icon + 5,
            height: ceil(size.height) + Self.padding.height * 2)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking); hoverTracking = tracking
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        let focused = window?.firstResponder === self
        let active = hovered || focused
        if active {
            tokens.color("glass-hover").setFill()
            NSBezierPath(roundedRect: bounds, xRadius: tokens.number("r-sm"),
                yRadius: tokens.number("r-sm")).fill()
        }
        if focused {
            let ring = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1),
                xRadius: tokens.number("r-sm"), yRadius: tokens.number("r-sm"))
            tokens.color("theme-accent").setStroke(); ring.lineWidth = 2; ring.stroke()
        }
        let text = attributed(active: active)
        let size = text.size()
        text.draw(at: NSPoint(x: Self.padding.width, y: (bounds.height - size.height) / 2))
        // Shipping `ExternalPreferenceIcon`: an open box with an outward arrow.
        let origin = NSPoint(x: Self.padding.width + ceil(size.width) + 5,
            y: (bounds.height - Self.icon) / 2)
        func at(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            NSPoint(x: origin.x + x * Self.icon / 16, y: origin.y + y * Self.icon / 16)
        }
        let glyph = NSBezierPath()
        glyph.move(to: at(6.5, 3)); glyph.line(to: at(3, 3)); glyph.line(to: at(3, 13))
        glyph.line(to: at(13, 13)); glyph.line(to: at(13, 9.5))
        glyph.move(to: at(9, 3)); glyph.line(to: at(13, 3)); glyph.line(to: at(13, 7))
        glyph.move(to: at(8.5, 7.5)); glyph.line(to: at(13, 3))
        glyph.lineWidth = 1.2; glyph.lineCapStyle = .round; glyph.lineJoinStyle = .round
        (active ? tokens.color("theme-accent-text-strong")
            : tokens.color("glass-text-subtle").withAlphaComponent(0.72)).setStroke()
        glyph.stroke()
    }
}

final class CaptureControlsView: NSView {
    private let tokens: Tokens
    private let autoStart: Bool
    private let visibility: CaptureControlsVisibility
    private var targetButtons: [UnifiedCaptureTarget: CaptureButton] = [:]
    private let aspectLabel = NSTextField(labelWithString: "Aspect")
    private let aspectMenu: GlassPopUpButton
    private let displayMenu: GlassPopUpButton
    private let captureButton: CaptureButton
    private let screenshotButton: CaptureButton
    private let recordButton: CaptureButton
    private let recordingAvailability: RecordingControlAvailability?
    private var recordingSwitches: [RecordingSwitchButton] = []
    private var fieldLabels: [NSTextField] = []
    private var noteViews: [NSView] = []
    private let fpsMenu: GlassPopUpButton
    private let resolutionMenu: GlassPopUpButton
    private let microphoneMenu: GlassPopUpButton
    private var microphoneIDs: [String?] = []
    private var panelDragOffset: NSPoint?
    /// Shipping `.capture-segmented-indicator`s for the mode and target switches.
    let modeIndicator = NSView()
    let targetIndicator = NSView()
    private var captureEnabled = false
    private(set) var menuState: CaptureMenuPolicy.Menu?
    var switchTarget: (UnifiedCaptureTarget) -> Void = { _ in }
    var switchMode: (UnifiedCaptureMode) -> Void = { _ in }
    var recordingControlsChanged: (RecordingControlState) -> Void = { _ in }
    var changeAspect: (Int) -> Void = { _ in }
    var changeDisplay: (Int) -> Void = { _ in }
    /// Opens Preferences at a setting key (a note link); the owner dismisses the menu.
    var openPreference: (String) -> Void = { _ in }
    /// Internal observer (the selector's identity pill); owners use recordingControlsChanged.
    var stateChanged: () -> Void = {}
    var confirm: () -> Void = {}
    var cancel: () -> Void = {}
    private(set) var target: UnifiedCaptureTarget = .region
    private(set) var mode: UnifiedCaptureMode = .screenshot
    private(set) var recordingState: RecordingControlState

    override var isFlipped: Bool { true }

    /// The note text the menu currently shows, for accessibility and tests.
    var noteText: String {
        guard let menu = menuState else { return "" }
        return [menu.note.text, CaptureMenuPolicy.copy.separator, menu.confirmText]
            .joined(separator: " ")
    }
    var noteLinks: [CaptureNoteLink] { noteViews.compactMap { $0 as? CaptureNoteLink } }
    var primaryTitle: String { captureButton.title }
    var primaryHidden: Bool { captureButton.isHidden }

    init(frame: NSRect, tokens: Tokens, autoStart: Bool, displayTitles: [String],
         selectedDisplay: Int, recordingState: RecordingControlState = RecordingControlState(
            framesPerSecond: 60, maxResolution: "original", showCursor: true,
            highlightClicks: false, systemAudio: false, microphoneDeviceID: nil),
         recordingAvailability: RecordingControlAvailability? = nil,
         microphoneDevices: [NativeMicrophoneDevice] = [],
         visibility: CaptureControlsVisibility = .excludedByDefault) {
        self.tokens = tokens; self.autoStart = autoStart; self.visibility = visibility
        self.recordingState = recordingState; self.recordingAvailability = recordingAvailability
        aspectMenu = GlassPopUpButton(frame: .zero, pullsDown: false)
        displayMenu = GlassPopUpButton(frame: .zero, pullsDown: false)
        fpsMenu = GlassPopUpButton(frame: .zero, pullsDown: false)
        resolutionMenu = GlassPopUpButton(frame: .zero, pullsDown: false)
        microphoneMenu = GlassPopUpButton(frame: .zero, pullsDown: false)
        captureButton = CaptureButton("Capture", frame: .zero, tokens: tokens, glass: true) {}
        screenshotButton = CaptureButton("Screenshot", frame: .zero, tokens: tokens, glass: true) {}
        recordButton = CaptureButton("Record", frame: .zero, tokens: tokens, glass: true) {}
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number("r-2xl")
        layer?.borderWidth = 1; layer?.borderColor = tokens.color("glass-border").cgColor
        layer?.shadowColor = NSColor.black.cgColor; layer?.shadowOpacity = 0.44
        layer?.shadowRadius = 22; layer?.shadowOffset = NSSize(width: 0, height: -8)
        setAccessibilityRole(.group); setAccessibilityLabel("Capture controls")

        let narrow = frame.width < 820
        let close = control("×", x: 8, width: 32) { [weak self] in self?.cancel() }
        close.setAccessibilityLabel("Close capture controls")
        let screenshot = screenshotButton
        screenshot.frame = NSRect(x: 48, y: 12, width: narrow ? 88 : 92, height: 36)
        addSubview(screenshot)
        screenshot.icon = .capture
        screenshot.actionBlock = { [weak self] in self?.selectMode(.screenshot, notify: true) }
        screenshot.setAccessibilityRole(.radioButton); screenshot.setAccessibilityLabel("Screenshot")
        screenshot.enterActionBlock = { [weak self] in self?.confirm() }
        let recordX = screenshot.frame.maxX + 4
        let record = recordButton
        record.frame = NSRect(x: recordX, y: 12, width: narrow ? 64 : 76, height: 36)
        addSubview(record)
        record.icon = .record
        record.actionBlock = { [weak self] in self?.selectMode(.record, notify: true) }
        record.setAccessibilityRole(.radioButton); record.setAccessibilityLabel("Record video")

        let dividerX = record.frame.maxX + 8
        let divider = NSView(frame: NSRect(x: dividerX, y: 18, width: 1, height: 24))
        divider.wantsLayer = true; divider.layer?.backgroundColor = tokens.color("glass-border").cgColor
        addSubview(divider)

        var x = dividerX + 9
        for mode in UnifiedCaptureTarget.allCases {
            let width: CGFloat
            if narrow {
                width = mode == .region ? 62 : mode == .window ? 68 : 94
            } else {
                width = mode == .display ? 106 : 78
            }
            let button = control(mode.title, x: x, width: width) { [weak self] in
                self?.selectTarget(mode, notify: true)
            }
            button.setAccessibilityRole(.radioButton)
            button.setAccessibilityLabel(mode.title)
            button.icon = mode == .window ? .window : mode == .display ? .display : .capture
            button.enterActionBlock = { [weak self] in self?.confirm() }
            targetButtons[mode] = button
            x += width + 4
        }

        let pickerX = x + 4
        let pickerWidth = frame.width - 122 - 8 - pickerX
        aspectLabel.frame = NSRect(x: pickerX, y: 21, width: 42, height: 18)
        aspectLabel.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        aspectLabel.textColor = tokens.color("glass-text-subtle")
        addSubview(aspectLabel)
        aspectMenu.frame = NSRect(x: pickerX + 44, y: 12,
            width: pickerWidth - 44, height: 36)
        aspectMenu.tokens = tokens
        aspectMenu.addItems(withTitles: RegionSelection.presets.map { $0.0 })
        aspectMenu.setAccessibilityLabel("Region aspect ratio")
        aspectMenu.bindChange { [weak self] index in self?.changeAspect(index) }
        addSubview(aspectMenu)

        displayMenu.frame = NSRect(x: pickerX, y: 12, width: pickerWidth, height: 36)
        displayMenu.tokens = tokens; displayMenu.addItems(withTitles: displayTitles)
        displayMenu.selectItem(at: selectedDisplay)
        displayMenu.setAccessibilityLabel("Display")
        displayMenu.bindChange { [weak self] index in self?.changeDisplay(index) }
        addSubview(displayMenu)

        captureButton.frame = NSRect(x: frame.width - 122, y: 10, width: 112, height: 40)
        captureButton.icon = .capture
        captureButton.primary = true; captureButton.actionBlock = { [weak self] in self?.confirm() }
        captureButton.enterActionBlock = { [weak self] in self?.confirm() }
        captureButton.setAccessibilityLabel("Take screenshot")
        captureButton.keyEquivalent = "\r"; captureButton.keyEquivalentModifierMask = []
        addSubview(captureButton)

        for button in subviews.compactMap({ $0 as? CaptureButton }) {
            button.escapeActionBlock = { [weak self] in self?.cancel() }
        }
        let indicators: [(NSView, NSView?)] = [
            (modeIndicator, screenshot), (targetIndicator, targetButtons[.region]),
        ]
        for (indicator, below) in indicators {
            indicator.wantsLayer = true
            indicator.layer?.backgroundColor = tokens.color("glass-active").cgColor
            indicator.layer?.borderColor = tokens.color("theme-accent").cgColor
            indicator.layer?.borderWidth = 1
            indicator.layer?.cornerRadius = tokens.number("r-md")
            indicator.setAccessibilityElement(false)
            addSubview(indicator, positioned: .below, relativeTo: below)
        }
        for button in [screenshot, record] + Array(targetButtons.values) { button.slidingSegment = true }
        configureRecordingRow(microphoneDevices: microphoneDevices)
        selectTarget(.region, notify: false)
        selectMode(.screenshot, notify: false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard let hit = super.hitTest(point) else { return nil }
        return hit is NSButton ? hit : self
    }

    override func mouseDown(with event: NSEvent) {
        guard let superview else { return }
        beginPanelDrag(at: superview.convert(event.locationInWindow, from: nil))
    }

    override func mouseDragged(with event: NSEvent) {
        guard let superview else { return }
        dragPanel(to: superview.convert(event.locationInWindow, from: nil))
    }

    override func mouseUp(with event: NSEvent) { endPanelDrag() }

    func beginPanelDrag(at point: NSPoint) {
        panelDragOffset = NSPoint(x: point.x - frame.minX, y: point.y - frame.minY)
    }

    func dragPanel(to point: NSPoint) {
        guard let superview, let offset = panelDragOffset else { return }
        let inset: CGFloat = 16
        let maxX = max(inset, superview.bounds.width - frame.width - inset)
        let maxY = max(inset, superview.bounds.height - frame.height - inset)
        frame.origin = NSPoint(x: min(max(inset, point.x - offset.x), maxX),
            y: min(max(inset, point.y - offset.y), maxY))
    }

    func endPanelDrag() { panelDragOffset = nil }

    private func control(_ title: String, x: CGFloat, width: CGFloat,
                         action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: NSRect(x: x, y: 12, width: width, height: 36),
            tokens: tokens, glass: true, action: action)
        addSubview(button); return button
    }

    /// Shipping `recording-options-row` columns: FPS, Max resolution, three
    /// switches, then the microphone select taking the remaining width.
    private static let fieldColumns: [(x: CGFloat, width: CGFloat)] = [
        (16, 76), (100, 124), (232, 100), (340, 100), (448, 112),
    ]
    private static let microphoneX: CGFloat = 568
    private static let fieldLabelY: CGFloat = 56
    private static let fieldControlY: CGFloat = 74

    private func fieldLabel(_ text: String, column: (x: CGFloat, width: CGFloat)) {
        // Shipping `.recording-field` captions render uppercase.
        let label = NSTextField(labelWithString: text.uppercased())
        label.frame = NSRect(x: column.x, y: Self.fieldLabelY, width: column.width, height: 14)
        label.font = .systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold)
        label.textColor = tokens.color("glass-text-subtle")
        label.lineBreakMode = .byTruncatingTail
        label.setAccessibilityElement(false)
        fieldLabels.append(label); addSubview(label)
    }

    private func configureRecordingRow(microphoneDevices: [NativeMicrophoneDevice]) {
        let copy = CaptureMenuPolicy.copy
        let columns = Self.fieldColumns
        fieldLabel(copy.fpsLabel, column: columns[0])
        fpsMenu.frame = NSRect(x: columns[0].x, y: Self.fieldControlY, width: columns[0].width, height: 36)
        fpsMenu.tokens = tokens
        fpsMenu.addItems(withTitles: copy.fpsOptions.map(String.init))
        fpsMenu.selectItem(at: copy.fpsOptions.firstIndex(of: recordingState.framesPerSecond) ?? 0)
        fpsMenu.setAccessibilityLabel(copy.fpsAccessibilityLabel)
        fpsMenu.bindChange { [weak self] index in
            guard let self, copy.fpsOptions.indices.contains(index) else { return }
            self.recordingState.framesPerSecond = copy.fpsOptions[index]
            self.recordingControlsChanged(self.recordingState)
            self.stateChanged()
        }
        addSubview(fpsMenu)
        fieldLabel(copy.maxResolutionLabel, column: columns[1])
        resolutionMenu.frame = NSRect(x: columns[1].x, y: Self.fieldControlY,
            width: columns[1].width, height: 36)
        resolutionMenu.tokens = tokens
        resolutionMenu.addItems(withTitles: copy.resolutionLabels)
        resolutionMenu.selectItem(at: copy.resolutionValues
            .firstIndex(of: recordingState.maxResolution) ?? 0)
        resolutionMenu.setAccessibilityLabel(copy.maxResolutionAccessibilityLabel)
        resolutionMenu.bindChange { [weak self] index in
            guard let self, copy.resolutionValues.indices.contains(index) else { return }
            self.recordingState.maxResolution = copy.resolutionValues[index]
            self.recordingControlsChanged(self.recordingState)
        }
        addSubview(resolutionMenu)
        for (index, toggle) in copy.toggles.enumerated() {
            let column = columns[min(2 + index, columns.count - 1)]
            fieldLabel(toggle.label, column: column)
            let available: Bool
            switch toggle.key {
            case "show_cursor": available = recordingAvailability?.cursor ?? false
            case "highlight_clicks": available = recordingAvailability?.clicks ?? false
            default: available = recordingAvailability?.systemAudio ?? false
            }
            let button = RecordingSwitchButton(frame: NSRect(x: column.x, y: Self.fieldControlY,
                width: column.width, height: 36), tokens: tokens, toggle: toggle, available: available)
            button.actionBlock = { [weak self, weak button] in
                guard let self, let button else { return }
                self.toggleRecordingOption(button.key)
            }
            recordingSwitches.append(button); addSubview(button)
        }
        fieldLabel(copy.microphoneLabel, column: (x: Self.microphoneX,
            width: max(116, frame.width - Self.microphoneX - 16)))
        configureMicrophoneMenu(devices: microphoneDevices,
            available: recordingAvailability?.microphone ?? false)
    }

    private func toggleRecordingOption(_ key: String) {
        switch key {
        case "show_cursor": recordingState.showCursor.toggle()
        case "highlight_clicks": recordingState.highlightClicks.toggle()
        case "system_audio": recordingState.systemAudio.toggle()
        default: return
        }
        if key != "system_audio",
           let coupled = try? CaptureMenuPolicy.coupled(changed: key,
               showCursor: recordingState.showCursor,
               highlightClicks: recordingState.highlightClicks) {
            recordingState.showCursor = coupled.showCursor
            recordingState.highlightClicks = coupled.highlightClicks
        }
        updateRecordingControls()
        recordingControlsChanged(recordingState)
        stateChanged()
    }

    private func refreshMenu() {
        menuState = try? CaptureMenuPolicy.menu(mode: mode, autoStart: autoStart,
            canExcludeControls: visibility.canExclude, controlsExcluded: visibility.excluded,
            state: recordingState, availability: recordingAvailability)
    }

    private func updateRecordingControls() {
        refreshMenu()
        let recording = mode == .record
        fpsMenu.isHidden = !recording
        resolutionMenu.isHidden = !recording
        microphoneMenu.isHidden = !recording
        for label in fieldLabels { label.isHidden = !recording }
        for button in recordingSwitches {
            button.isHidden = !recording
            let on: Bool
            switch button.key {
            case "show_cursor": on = recordingState.showCursor
            case "highlight_clicks": on = recordingState.highlightClicks
            default: on = recordingState.systemAudio
            }
            button.isOn = button.isEnabled && on
            button.status = menuState?.toggleStatus[button.key] ?? (button.isEnabled ? "Off" : "Unavailable")
        }
        updatePrimary()
        layoutNote()
    }

    private func updatePrimary() {
        let recording = mode == .record
        captureButton.title = menuState?.primaryLabel ?? (recording ? "Start recording" : "Capture")
        captureButton.icon = recording ? .record : .capture
        captureButton.setAccessibilityLabel(menuState?.primaryAccessibilityLabel
            ?? (recording ? "Start recording" : "Take screenshot"))
        captureButton.isEnabled = captureEnabled
        captureButton.isHidden = menuState?.primaryHidden ?? autoStart
        captureButton.needsDisplay = true
    }

    /// Shipping `capture-selector-note`, centered on the panel's last row.
    private func layoutNote() {
        noteViews.forEach { $0.removeFromSuperview() }; noteViews = []
        guard let menu = menuState else { return }
        let note = menu.note
        let noteParts: [(String, Bool)] = [(note.lead, false), (note.emphasis, true),
            (note.trail, false), (note.hint, false)].filter { !$0.0.isEmpty }
        noteViews.append(noteView(parts: noteParts, setting: note.setting))
        noteViews.append(noteView(parts: [(CaptureMenuPolicy.copy.separator, false)], setting: nil))
        noteViews.append(noteView(parts: [(menu.confirmText, false)], setting: menu.confirmSetting))
        let gap = tokens.number("s-2")
        let total = noteViews.reduce(CGFloat(0)) { $0 + $1.frame.width } + gap * CGFloat(noteViews.count - 1)
        var x = max(16, (bounds.width - total) / 2)
        let midY = bounds.height - 15
        for view in noteViews {
            view.frame.origin = NSPoint(x: x, y: (midY - view.frame.height / 2).rounded())
            x += view.frame.width + gap
            addSubview(view)
        }
    }

    private func noteView(parts: [(String, Bool)], setting: String?) -> NSView {
        if let setting {
            let link = CaptureNoteLink(parts: parts, setting: setting, tokens: tokens)
            link.actionBlock = { [weak self] in self?.openPreference(setting) }
            return link
        }
        let text = NSMutableAttributedString()
        for (part, strong) in parts {
            text.append(NSAttributedString(string: part, attributes: [
                .font: NSFont.systemFont(ofSize: tokens.number("text-xs"),
                    weight: strong ? .bold : .medium),
                .foregroundColor: tokens.color(strong ? "glass-text" : "glass-text-subtle"),
            ]))
        }
        let label = NSTextField(labelWithAttributedString: text)
        label.sizeToFit()
        label.setAccessibilityLabel(text.string)
        return label
    }

    func selectMode(_ mode: UnifiedCaptureMode, notify: Bool) {
        self.mode = mode
        if let superview {
            let height: CGFloat = mode == .record ? 154 : 86
            let width = min(superview.bounds.width - 32, mode == .record ? 902 : 854)
            let x = min(max(16, frame.midX - width / 2), superview.bounds.width - width - 16)
            frame = NSRect(x: x, y: superview.bounds.height - height - 26,
                width: width, height: height)
            captureButton.frame = mode == .record
                ? NSRect(x: width - 162, y: 10, width: 152, height: 40)
                : NSRect(x: width - 122, y: 10, width: 112, height: 40)
            microphoneMenu.frame.size.width = max(116, width - microphoneMenu.frame.minX - 16)
            if let label = fieldLabels.last { label.frame.size.width = microphoneMenu.frame.width }
        }
        let arriving = mode == .record && !recordButton.selected
        screenshotButton.selected = mode == .screenshot
        recordButton.selected = mode == .record
        screenshotButton.setAccessibilityValue(mode == .screenshot ? 1 : 0)
        recordButton.setAccessibilityValue(mode == .record ? 1 : 0)
        slideIndicator(modeIndicator, to: mode == .record ? recordButton : screenshotButton)
        updateRecordingControls()
        if arriving { playRecordingRowEntrance() }
        if notify { switchMode(mode) }
    }

    /// Moves a switch's indicator under the selected segment. The first
    /// placement and offscreen or Reduce Motion changes jump; otherwise it
    /// slides over the shipping `--dur-4` `--ease-standard` transition.
    private func slideIndicator(_ indicator: NSView, to button: NSButton) {
        let target = button.frame.insetBy(dx: 1, dy: 1)
        let placed = indicator.frame.width > 0
        let tween = NativeMotion.transition("segmented_indicator", tokens: tokens)
        guard placed, window?.isVisible == true, tween.duration > 0 else {
            indicator.frame = target
            return
        }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = tween.duration
            context.timingFunction = tween.timing
            indicator.animator().frame = target
        }
    }

    /// Shipping `recording-options-arrive` (40 ms delay, 5 pt drop and fade)
    /// on the Record row. Presentation-only; skipped under Reduce Motion.
    private func playRecordingRowEntrance() {
        guard window?.isVisible == true else { return }
        let row: [NSView] = [fpsMenu, resolutionMenu, microphoneMenu]
            + (fieldLabels as [NSView]) + (recordingSwitches as [NSView])
        for view in row where !view.isHidden {
            NativeMotion.play("capture_menu_options_arrive", on: view, tokens: tokens)
        }
    }

    func selectTarget(_ target: UnifiedCaptureTarget, notify: Bool) {
        self.target = target
        for (mode, button) in targetButtons {
            button.selected = mode == target
            button.setAccessibilityValue(button.selected ? 1 : 0)
            button.needsDisplay = true
        }
        if let button = targetButtons[target] { slideIndicator(targetIndicator, to: button) }
        aspectLabel.isHidden = target != .region
        aspectMenu.isHidden = target != .region
        displayMenu.isHidden = target != .display
        if notify { switchTarget(target) }
    }

    func selectAspect(_ index: Int) { aspectMenu.selectItem(at: index) }
    func selectDisplay(_ index: Int) { displayMenu.selectItem(at: index) }
    func selectMicrophone(_ index: Int, notify: Bool) {
        guard microphoneIDs.indices.contains(index), microphoneMenu.isEnabled,
              microphoneMenu.item(at: index)?.isEnabled != false else { return }
        microphoneMenu.selectItem(at: index)
        recordingState.microphoneDeviceID = microphoneIDs[index]
        if notify { recordingControlsChanged(recordingState) }
    }
    /// Tests and the selector: flip a recording switch as a click would.
    func toggleRecordingSwitch(_ key: String) {
        guard let button = recordingSwitches.first(where: { $0.key == key }), button.isEnabled else { return }
        toggleRecordingOption(key)
    }
    func setCaptureEnabled(_ enabled: Bool) {
        captureEnabled = enabled
        updatePrimary()
    }

    private func configureMicrophoneMenu(devices: [NativeMicrophoneDevice], available: Bool) {
        microphoneMenu.frame = NSRect(x: Self.microphoneX, y: Self.fieldControlY,
            width: max(116, frame.width - Self.microphoneX - 16), height: 36)
        microphoneMenu.tokens = tokens
        microphoneMenu.setAccessibilityLabel(CaptureMenuPolicy.copy.microphoneLabel)
        microphoneMenu.isEnabled = available
        microphoneMenu.autoenablesItems = false
        // AppKit enumerates devices before the menu opens, so it never shows
        // the shipping "Loading microphones…" row.
        let options = try? CaptureMenuPolicy.microphones(available: available, loading: false,
            selected: recordingState.microphoneDeviceID, devices: devices)
        let entries = options?.entries ?? [CaptureMenuPolicy.MicrophoneEntry(id: nil,
            label: available ? "Off" : "Unavailable", enabled: true)]
        microphoneIDs = entries.map(\.id)
        microphoneMenu.addItems(withTitles: entries.map(\.label))
        for (index, entry) in entries.enumerated() {
            microphoneMenu.item(at: index)?.isEnabled = entry.enabled
        }
        let selected = microphoneIDs.firstIndex(where: {
            $0 == recordingState.microphoneDeviceID
        }) ?? 0
        microphoneMenu.selectItem(at: selected)
        microphoneMenu.bindChange { [weak self] index in
            self?.selectMicrophone(index, notify: true)
        }
        addSubview(microphoneMenu)
    }
}

private final class UnifiedCaptureCanvas: NSView {
    weak var selector: UnifiedCaptureSelectionView?
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) { selector?.drawSelection() }
    override func mouseDown(with event: NSEvent) { selector?.mouseDown(with: event) }
    override func mouseDragged(with event: NSEvent) { selector?.mouseDragged(with: event) }
    override func mouseUp(with event: NSEvent) { selector?.mouseUp(with: event) }
    override func mouseMoved(with event: NSEvent) { selector?.mouseMoved(with: event) }
    override func resetCursorRects() {
        addCursorRect(bounds, cursor: selector?.target == .region ? .crosshair : .pointingHand)
    }
}

final class UnifiedCaptureSelectionView: NSView {
    typealias HitTest = (CapturesSelectionPoint) -> Int64?

    private let tokens: Tokens
    private let autoStart: Bool
    private let targets: [WindowSelectionTarget]
    private let hitTest: HitTest
    private let canvas = UnifiedCaptureCanvas()
    private(set) var region: RegionSelection
    private(set) var selectedWindowIndex: Int64?
    private(set) var hoveredWindowIndex: Int64 = -1
    private(set) var target: UnifiedCaptureTarget = .region
    private(set) var mode: UnifiedCaptureMode = .screenshot
    private(set) var aspectIndex = 0
    let controls: CaptureControlsView
    private let selectionLabel = NSTextField(labelWithString: "")
    private let guidance = Surface()
    private let guidanceTitle = NSTextField(labelWithString: "")
    private let guidanceDetail = NSTextField(labelWithString: "")
    private let identityName = NSTextField(labelWithString: "")
    private let identityDetail = NSTextField(labelWithString: "")
    private let displayIdentity: CaptureDisplayIdentity?
    private let currentDisplayTitle: String
    private var regionGestureActive = false
    /// Window mode pointer is over the desktop or shell chrome.
    private var hoveringDisplay = false
    private(set) var isGuidanceDucked = false
    var confirm: (WindowSelectionChoice) -> Void
    var cancel: () -> Void
    var changeDisplay: (Int) -> Void
    /// A note link: open Preferences at this setting key.
    var openPreference: (String) -> Void = { _ in }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    init(frame: NSRect, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, hitTest: @escaping HitTest, displayTitles: [String],
         selectedDisplay: Int, confirm: @escaping (WindowSelectionChoice) -> Void,
         cancel: @escaping () -> Void, changeDisplay: @escaping (Int) -> Void,
         recordingState: RecordingControlState = RecordingControlState(
            framesPerSecond: 60, maxResolution: "original", showCursor: true, highlightClicks: false,
            systemAudio: false, microphoneDeviceID: nil),
         recordingAvailability: RecordingControlAvailability? = nil,
         microphoneDevices: [NativeMicrophoneDevice] = [],
         visibility: CaptureControlsVisibility = .excludedByDefault,
         displayIdentity: CaptureDisplayIdentity? = nil) {
        self.tokens = tokens; self.autoStart = autoStart; self.targets = targets
        self.hitTest = hitTest; self.confirm = confirm; self.cancel = cancel
        self.changeDisplay = changeDisplay; self.displayIdentity = displayIdentity
        currentDisplayTitle = displayTitles.indices.contains(selectedDisplay)
            ? displayTitles[selectedDisplay] : "Full screen"
        region = RegionSelection(bounds: CapturesSelectionBounds(width: frame.width, height: frame.height))
        let controlsWidth = min(frame.width - 32, 854)
        controls = CaptureControlsView(frame: NSRect(x: (frame.width - controlsWidth) / 2,
            y: frame.height - 112, width: controlsWidth, height: 86), tokens: tokens,
            autoStart: autoStart, displayTitles: displayTitles, selectedDisplay: selectedDisplay,
            recordingState: recordingState, recordingAvailability: recordingAvailability,
            microphoneDevices: microphoneDevices, visibility: visibility)
        super.init(frame: frame)
        wantsLayer = true; layer?.backgroundColor = NSColor.clear.cgColor
        if let image {
            let background = NSImageView(frame: bounds)
            background.image = NSImage(cgImage: image, size: bounds.size)
            background.imageScaling = .scaleAxesIndependently
            background.setAccessibilityElement(false); addSubview(background)
        }
        canvas.frame = bounds; canvas.selector = self; canvas.setAccessibilityElement(false)
        addSubview(canvas)
        guidance.wantsLayer = true
        guidance.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        guidance.layer?.cornerRadius = tokens.number("r-xl")
        guidance.layer?.borderWidth = 1
        guidance.layer?.borderColor = tokens.color("glass-border-strong").cgColor
        guidanceTitle.alignment = .center
        guidanceTitle.font = .systemFont(ofSize: tokens.number("text-md"), weight: .semibold)
        guidanceTitle.textColor = tokens.color("glass-text")
        guidanceDetail.alignment = .center
        guidanceDetail.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        guidanceDetail.textColor = tokens.color("glass-text-subtle")
        guidance.addSubview(guidanceTitle); guidance.addSubview(guidanceDetail)
        addSubview(guidance)
        // Shipping `recording-display-identity`: shadowed text, no chip.
        for (label, size, weight, color) in [
            (identityName, tokens.number("text-2xl"), NSFont.Weight.semibold, "glass-text"),
            (identityDetail, tokens.number("text-md"), NSFont.Weight.regular, "glass-text-muted"),
        ] {
            label.alignment = .center
            label.font = .monospacedDigitSystemFont(ofSize: size, weight: weight)
            label.textColor = tokens.color(color)
            let shadow = NSShadow()
            shadow.shadowColor = NSColor.black.withAlphaComponent(0.5)
            shadow.shadowOffset = NSSize(width: 0, height: -2); shadow.shadowBlurRadius = 12
            label.shadow = shadow
            addSubview(label)
        }
        selectionLabel.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .semibold)
        selectionLabel.textColor = tokens.color("glass-text")
        selectionLabel.alignment = .center; selectionLabel.wantsLayer = true
        selectionLabel.layer?.backgroundColor = tokens.color("glass-strong").cgColor
        selectionLabel.layer?.cornerRadius = tokens.number("r-sm")
        addSubview(selectionLabel)
        controls.switchTarget = { [weak self] target in self?.setTarget(target) }
        controls.switchMode = { [weak self] mode in self?.setMode(mode) }
        controls.changeAspect = { [weak self] index in self?.setAspect(index) }
        controls.changeDisplay = { [weak self] index in self?.changeDisplay(index) }
        controls.openPreference = { [weak self] setting in self?.openPreference(setting) }
        controls.stateChanged = { [weak self] in self?.update() }
        controls.confirm = { [weak self] in self?.confirmSelection() }
        controls.cancel = { [weak self] in self?.cancel() }
        addSubview(controls)
        setAccessibilityRole(.group); setAccessibilityLabel("New Capture screenshot selector")
        update()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var isGuidanceVisible: Bool { !guidance.isHidden }
    var guidanceText: String { "\(guidanceTitle.stringValue) · \(guidanceDetail.stringValue)" }
    var guidanceFrame: NSRect { guidance.frame }
    var isDisplayIdentityVisible: Bool { !identityName.isHidden }
    var displayIdentityText: String { "\(identityName.stringValue) · \(identityDetail.stringValue)" }
    var controlsState: UnifiedCaptureControlsState {
        UnifiedCaptureControlsState(mode: mode, target: target, aspectIndex: aspectIndex)
    }

    var choice: WindowSelectionChoice? {
        switch target {
        case .region: return region.capturable && region.mode == nil ? .region(region.rect) : nil
        case .window:
            guard let index = selectedWindowIndex, index >= 0, index < Int64(targets.count) else { return nil }
            return .window(index: Int(index), id: targets[Int(index)].id)
        case .display: return .display
        }
    }

    func setTarget(_ target: UnifiedCaptureTarget) {
        self.target = target; controls.selectTarget(target, notify: false)
        hoveringDisplay = false
        // Shipping auto-start applies to both Screenshot and Record.
        if target == .display && autoStart { confirmSelection(); return }
        update()
    }

    func setMode(_ mode: UnifiedCaptureMode) {
        self.mode = mode; controls.selectMode(mode, notify: false); update()
    }

    func setTargetFromShortcut(_ target: UnifiedCaptureTarget, mode: UnifiedCaptureMode = .screenshot) {
        setMode(mode)
        hoveredWindowIndex = -1; hoveringDisplay = false
        if target != .window { selectedWindowIndex = nil }
        self.target = target
        controls.selectTarget(target, notify: false)
        update()
    }

    func setAspect(_ index: Int) {
        guard RegionSelection.presets.indices.contains(index) else { return }
        aspectIndex = index
        region.setAspect(RegionSelection.presets[index].1)
        controls.selectAspect(index); update()
    }

    /// `armAutoStart` is false for the first restoration of a tray/shortcut
    /// request: like keyboard target changes, that never auto-starts. A display
    /// replacement re-arms Full screen auto-start, as shipping does.
    func restoreControls(_ state: UnifiedCaptureControlsState, armAutoStart: Bool = true) {
        setAspect(state.aspectIndex)
        setMode(state.mode)
        if armAutoStart {
            setTarget(state.target)
        } else {
            target = state.target; hoveringDisplay = false
            controls.selectTarget(state.target, notify: false)
            update()
        }
    }

    func beginRegion(_ point: NSPoint, shift: Bool = false) {
        guard target == .region else { return }
        region.begin(point, handleRadius: tokens.number("s-4"), shift: shift); update()
    }
    func dragRegion(_ point: NSPoint, shift: Bool = false) {
        guard target == .region else { return }
        region.update(point, shift: shift); update()
    }
    func endRegion() {
        guard target == .region, region.mode != nil else { return }
        let created = region.mode == 0
        region.end(); update()
        if autoStart && created { confirmSelection() }
    }

    @discardableResult func hoverWindow(_ point: NSPoint) -> Bool {
        guard target == .window, point.x.isFinite, point.y.isFinite,
              let index = hitTest(CapturesSelectionPoint(x: point.x, y: point.y)),
              index == -1 || (index >= 0 && index < Int64(targets.count)) else { return false }
        hoveredWindowIndex = index; hoveringDisplay = index < 0; update(); return true
    }

    func selectWindow(_ point: NSPoint) {
        guard hoverWindow(point) else { return }
        if hoveredWindowIndex < 0 {
            selectedWindowIndex = nil; setTarget(.display)
        } else {
            selectedWindowIndex = hoveredWindowIndex; update()
            if autoStart { confirmSelection() }
        }
    }

    func confirmSelection() { if let choice { confirm(choice) } }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        guard !controls.frame.contains(point) else {
            regionGestureActive = false
            return
        }
        switch target {
        case .region:
            regionGestureActive = true
            beginRegion(point, shift: event.modifierFlags.contains(.shift))
        case .window: selectWindow(point)
        case .display: if autoStart { confirmSelection() }
        }
    }
    override func mouseDragged(with event: NSEvent) {
        duckGuidance(at: convert(event.locationInWindow, from: nil))
        if target == .region && regionGestureActive {
            dragRegion(convert(event.locationInWindow, from: nil),
                shift: event.modifierFlags.contains(.shift))
        }
    }
    override func mouseUp(with event: NSEvent) {
        if target == .region && regionGestureActive {
            dragRegion(convert(event.locationInWindow, from: nil),
                shift: event.modifierFlags.contains(.shift)); endRegion()
        }
        regionGestureActive = false
    }
    override func mouseMoved(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if target == .window { _ = hoverWindow(point) }
        duckGuidance(at: point)
    }

    /// Shipping guidance ducking: fade when the pointer nears the chip (28 pt),
    /// restore only once it leaves the wider 40 pt zone.
    func duckGuidance(at point: NSPoint) {
        let ducked = !guidance.isHidden && point.x.isFinite && point.y.isFinite
            && CaptureMenuPolicy.pointerOverGuidance(point, chip: guidance.frame,
                currentlyOver: isGuidanceDucked)
        guard ducked != isGuidanceDucked else { return }
        isGuidanceDucked = ducked
        NSAnimationContext.runAnimationGroup { context in
            context.duration = Double(tokens.number("dur-3")) / 1000
            guidance.animator().alphaValue = ducked ? 0 : 1
        }
        guidance.setAccessibilityElement(!ducked)
    }
    override func flagsChanged(with event: NSEvent) {
        if target == .region {
            region.recompute(shift: event.modifierFlags.contains(.shift)); update()
        }
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { cancel() }
        else if event.keyCode == 36 || event.keyCode == 76 { confirmSelection() }
        else { super.keyDown(with: event) }
    }
    func updatePointerLocation() {
        guard target == .window, let window else { return }
        let point = window.convertPoint(fromScreen: NSEvent.mouseLocation)
        _ = hoverWindow(convert(point, from: nil))
    }

    private func update() {
        controls.setCaptureEnabled(choice != nil)
        let label: String
        let rect: NSRect?
        var guidanceCopy: CaptureMenuPolicy.Guidance?
        switch target {
        case .region:
            guidanceCopy = CaptureMenuPolicy.guidance("region")
            label = region.capturable
                ? "\(Int(region.rect.width.rounded())) × \(Int(region.rect.height.rounded()))" : ""
            rect = region.capturable ? region.nsRect : nil
        case .window:
            let active = hoveredWindowIndex >= 0 ? hoveredWindowIndex : selectedWindowIndex ?? -1
            if active >= 0, active < Int64(targets.count) {
                label = targets[Int(active)].name; rect = targets[Int(active)].rect
            } else {
                label = ""; rect = nil
            }
            // Shipping: guidance stays until a window is selected; the desktop
            // and shell chrome switch it to the display copy.
            if selectedWindowIndex == nil {
                guidanceCopy = CaptureMenuPolicy.guidance(hoveringDisplay ? "display" : "window")
            }
        case .display:
            label = ""; rect = nil
        }
        // Region guidance hides while a selection gesture is active.
        guidance.isHidden = guidanceCopy == nil || (target == .region && region.mode != nil)
        if let guidanceCopy {
            guidanceTitle.stringValue = guidanceCopy.title
            guidanceDetail.stringValue = guidanceCopy.hint
        }
        guidanceTitle.sizeToFit(); guidanceDetail.sizeToFit()
        let padding = NSSize(width: tokens.number("s-6"), height: tokens.number("s-4"))
        let guidanceWidth = ceil(max(guidanceTitle.frame.width, guidanceDetail.frame.width))
            + padding.width * 2
        let guidanceHeight = ceil(guidanceTitle.frame.height + 2 + guidanceDetail.frame.height)
            + padding.height * 2
        guidance.frame = NSRect(x: ((bounds.width - guidanceWidth) / 2).rounded(),
            y: (bounds.height * 0.16).rounded(), width: guidanceWidth, height: guidanceHeight)
        guidanceTitle.frame = NSRect(x: padding.width, y: padding.height,
            width: guidanceWidth - padding.width * 2, height: guidanceTitle.frame.height)
        guidanceDetail.frame = NSRect(x: padding.width, y: guidanceTitle.frame.maxY + 2,
            width: guidanceWidth - padding.width * 2, height: guidanceDetail.frame.height)
        guidance.setAccessibilityLabel([guidanceTitle.stringValue, guidanceDetail.stringValue]
            .filter { !$0.isEmpty }.joined(separator: ". "))
        if guidance.isHidden && isGuidanceDucked {
            isGuidanceDucked = false; guidance.alphaValue = 1
        }
        updateDisplayIdentity()
        selectionLabel.stringValue = label
        selectionLabel.isHidden = label.isEmpty
        if let rect {
            let width = min(max(80, selectionLabel.intrinsicContentSize.width + tokens.number("s-6")),
                bounds.width - tokens.number("s-8"))
            let height = tokens.number("h-xs")
            selectionLabel.frame = NSRect(x: min(max(tokens.number("s-4"), rect.midX - width / 2),
                bounds.width - width - tokens.number("s-4")),
                y: rect.minY >= height + tokens.number("s-3")
                    ? rect.minY - height - tokens.number("s-3") : rect.minY + tokens.number("s-3"),
                width: width, height: height)
        }
        selectionLabel.setAccessibilityLabel(label)
        canvas.needsDisplay = true; canvas.discardCursorRects(); canvas.resetCursorRects()
    }

    /// Shipping Full screen identity: display name, then W × H and Record FPS.
    private func updateDisplayIdentity() {
        let visible = target == .display
        identityName.isHidden = !visible; identityDetail.isHidden = !visible
        guard visible else { return }
        let fallback = currentDisplayTitle.components(separatedBy: " · ")
        let identity = displayIdentity.flatMap {
            try? CaptureMenuPolicy.displayIdentity(name: $0.name, width: $0.width,
                height: $0.height,
                recordingFPS: mode == .record ? controls.recordingState.framesPerSecond : nil)
        } ?? (name: fallback.first ?? currentDisplayTitle,
               detail: fallback.dropFirst().joined(separator: " · "))
        identityName.stringValue = identity.name; identityDetail.stringValue = identity.detail
        identityName.sizeToFit(); identityDetail.sizeToFit()
        let gap = tokens.number("s-4")
        let height = identityName.frame.height + gap + identityDetail.frame.height
        let top = (bounds.height / 2 - height * 0.6).rounded()
        let width = min(bounds.width - 32, max(280, identityName.frame.width, identityDetail.frame.width))
        identityName.frame = NSRect(x: (bounds.width - width) / 2, y: top,
            width: width, height: identityName.frame.height)
        identityDetail.frame = NSRect(x: (bounds.width - width) / 2,
            y: identityName.frame.maxY + gap, width: width, height: identityDetail.frame.height)
        identityName.setAccessibilityLabel("\(identity.name), \(identity.detail)")
        identityDetail.setAccessibilityElement(false)
    }

    fileprivate func drawSelection() {
        let selectedRect: NSRect?
        let radius: CGFloat
        switch target {
        case .region: selectedRect = region.capturable ? region.nsRect : nil; radius = 0
        case .window:
            let active = hoveredWindowIndex >= 0 ? hoveredWindowIndex : selectedWindowIndex ?? -1
            if active >= 0, active < Int64(targets.count) {
                selectedRect = targets[Int(active)].rect; radius = targets[Int(active)].cornerRadius
            } else { selectedRect = nil; radius = 0 }
        case .display: selectedRect = bounds.insetBy(dx: 2, dy: 2); radius = 0
        }
        let veil = NSBezierPath(rect: bounds)
        if let selectedRect {
            veil.append(NSBezierPath(roundedRect: selectedRect, xRadius: radius, yRadius: radius))
        }
        veil.windingRule = .evenOdd; tokens.color("glass-veil").setFill(); veil.fill()
        guard let selectedRect else { return }
        tokens.color("theme-accent").setStroke()
        let border = NSBezierPath(roundedRect: selectedRect, xRadius: radius, yRadius: radius)
        border.lineWidth = 1.5; border.stroke()
        if target == .region {
            for corner in region.corners {
                let handle = NSBezierPath(ovalIn: NSRect(x: corner.x - 5, y: corner.y - 5,
                    width: 10, height: 10))
                tokens.color("theme-accent").setFill(); handle.fill()
                tokens.color("theme-accent-ink").setStroke(); handle.stroke()
            }
        }
    }
}

final class UnifiedCapturePanel: NSPanel {
    let selector: UnifiedCaptureSelectionView
    override var canBecomeKey: Bool { true }
    override func cancelOperation(_ sender: Any?) { selector.cancel() }

    init(screen: NSScreen, image: CGImage?, targets: [WindowSelectionTarget], tokens: Tokens,
         autoStart: Bool, hitTest: @escaping UnifiedCaptureSelectionView.HitTest,
         displayTitles: [String], selectedDisplay: Int,
         confirm: @escaping (WindowSelectionChoice) -> Void, cancel: @escaping () -> Void,
         changeDisplay: @escaping (Int) -> Void,
         recordingState: RecordingControlState = RecordingControlState(
            framesPerSecond: 60, maxResolution: "original", showCursor: true,
            highlightClicks: false, systemAudio: false, microphoneDeviceID: nil),
         recordingAvailability: RecordingControlAvailability? = nil,
         microphoneDevices: [NativeMicrophoneDevice] = [],
         visibility: CaptureControlsVisibility = .excludedByDefault,
         displayIdentity: CaptureDisplayIdentity? = nil) {
        selector = UnifiedCaptureSelectionView(frame: NSRect(origin: .zero, size: screen.frame.size),
            image: image, targets: targets, tokens: tokens, autoStart: autoStart,
            hitTest: hitTest, displayTitles: displayTitles, selectedDisplay: selectedDisplay,
            confirm: confirm, cancel: cancel, changeDisplay: changeDisplay,
            recordingState: recordingState, recordingAvailability: recordingAvailability,
            microphoneDevices: microphoneDevices, visibility: visibility,
            displayIdentity: displayIdentity)
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        title = "Captures Capture Controls"
        isReleasedWhenClosed = false; isOpaque = false; backgroundColor = .clear; hasShadow = false
        level = .screenSaver; collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none; acceptsMouseMovedEvents = true; contentView = selector
        makeFirstResponder(selector)
    }
}
