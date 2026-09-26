import AppKit
import CCapturesSettings

protocol LoginItemServicing {
    func setEnabled(_ enabled: Bool?, completion: @escaping (Result<Bool, Error>) -> Void)
}

final class NativeLoginItemService: LoginItemServicing {
    private let transport: SettingsTransport
    private let appTransport: AppTransport
    private let historyRoot: String?
    private let settingsFile: String?
    private let queue = DispatchQueue(label: "es.captures.native.login-item")

    init(historyRoot: String?, settingsFile: String?, transport: SettingsTransport = SettingsBridge(),
         appTransport: AppTransport = AppBridge()) {
        self.historyRoot = historyRoot; self.settingsFile = settingsFile; self.transport = transport
        self.appTransport = appTransport
    }

    func setEnabled(_ enabled: Bool?, completion: @escaping (Result<Bool, Error>) -> Void) {
        queue.async {
            let result = Result { () throws -> Bool in
                let history: String
                if let root = self.historyRoot { history = root }
                else {
                    guard let root = try self.appTransport.request(["operation": "default_history_root"])["path"] as? String,
                          !root.isEmpty else { throw AppBridgeError.invalidResponse }
                    history = root
                }
                let settings = try self.settingsFile ?? self.path(operation: "default_path")
                var request: [String: Any] = ["operation": "login_item", "history_root": history,
                                               "settings_file": settings]
                if let enabled { request["enabled"] = enabled }
                guard let actual = try self.transport.request(request)["enabled"] as? Bool else {
                    throw SettingsStoreError.invalidResponse
                }
                return actual
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    private func path(operation: String) throws -> String {
        guard let path = try transport.request(["operation": operation])["path"] as? String,
              !path.isEmpty else { throw SettingsStoreError.invalidResponse }
        return path
    }
}

final class ClosurePopUpButton: NSPopUpButton {
    var tokens: Tokens!
    var change: ((Int) -> Void)?
    @objc func selectedValue() { change?(indexOfSelectedItem) }

    /// Shipping `.custom-select-trigger`: a field with the selected label and a chevron.
    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-md")
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        let alpha: CGFloat = isEnabled ? 1 : 0.5
        tokens.color("surface-field").withAlphaComponent(alpha).setFill(); outline.fill()
        let focused = window?.firstResponder === self
        tokens.color(focused ? "theme-accent" : "control-border").withAlphaComponent(alpha).setStroke()
        outline.lineWidth = 1; outline.stroke()
        if focused { drawPreferenceFocusRing(tokens, in: bounds.insetBy(dx: -1, dy: -1), radius: radius + 1) }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
            .foregroundColor: tokens.color("text").withAlphaComponent(alpha),
        ]
        let padding = tokens.number("s-4")
        let text = titleOfSelectedItem ?? title
        let size = (text as NSString).size(withAttributes: attributes)
        let textRect = NSRect(x: padding, y: (bounds.height - size.height) / 2,
            width: max(0, bounds.width - padding * 2 - 14 - tokens.number("s-3")), height: size.height)
        (text as NSString).draw(with: textRect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
            attributes: attributes)
        // The shipping 16-unit chevron `m4 6 4 4 4-4`, independent of flipping.
        let glyph = NSRect(x: bounds.width - padding - 14, y: (bounds.height - 14) / 2, width: 14, height: 14)
        let scale = glyph.width / 16
        func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            let down = glyph.minY + y * scale
            return NSPoint(x: glyph.minX + x * scale, y: isFlipped ? down : bounds.height - down)
        }
        let chevron = NSBezierPath()
        chevron.move(to: point(4, 6)); chevron.line(to: point(8, 10)); chevron.line(to: point(12, 6))
        chevron.lineWidth = 1.7 * scale; chevron.lineCapStyle = .round; chevron.lineJoinStyle = .round
        tokens.color("text-subtle").withAlphaComponent(alpha).setStroke(); chevron.stroke()
    }
}

final class ClosureColorWell: NSColorWell {
    var change: ((NSColor) -> Void)?
    @objc func selectedColor() { change?(color) }
}

final class ShortcutRecorderButton: NSButton {
    var tokens: Tokens!
    var beginRecording: (() -> Void)?
    var recordEvent: ((NSEvent) -> Void)?
    var cancelRecording: (() -> Void)?
    private(set) var recording = false
    private(set) var keys: [String] = []
    private(set) var error = ""

    override var acceptsFirstResponder: Bool { true }
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens, label: String, keys: [String]) {
        super.init(frame: frame)
        self.tokens = tokens; self.keys = keys
        title = ""; isBordered = false; setButtonType(.momentaryPushIn)
        target = self; action = #selector(activateRecorder)
        setAccessibilityRole(.button); setAccessibilityLabel(label)
        updateAccessibility()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    @objc private func activateRecorder() { beginRecording?() }

    func startRecording() {
        recording = true; keys = []; error = ""; updateAccessibility(); needsDisplay = true
    }
    func update(keys: [String], error: String = "") {
        self.keys = keys; self.error = error; updateAccessibility(); needsDisplay = true
    }
    func stopRecording(keys: [String]) {
        recording = false; self.keys = keys; error = ""; updateAccessibility(); needsDisplay = true
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        if recording { cancelRecording?() }
        needsDisplay = true
        return accepted
    }
    override func keyDown(with event: NSEvent) {
        if recording {
            recordEvent?(event)
        } else if event.keyCode == 36 || event.keyCode == 49 {
            beginRecording?()
        } else {
            super.keyDown(with: event)
        }
    }
    override func flagsChanged(with event: NSEvent) {
        if recording { recordEvent?(event) } else { super.flagsChanged(with: event) }
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard recording else { return super.performKeyEquivalent(with: event) }
        recordEvent?(event)
        return true
    }

    private func updateAccessibility() {
        state = recording ? .on : .off
        setAccessibilityValue(keys.isEmpty ? (recording ? "Press shortcut" : "None") : keys.joined(separator: " + "))
        setAccessibilitySelected(recording)
        setAccessibilityHelp(error.isEmpty ? nil : error)
    }

    private func chipLayout(_ values: [String]) -> (NSFont, [NSRect]) {
        let available = max(0, bounds.width - 20)
        // Shipping `kbd`: `--text-2xs` semibold chips with 5 pt padding.
        let baseSize = tokens.number("text-2xs")
        let gap: CGFloat = 4
        let minimumFontSize: CGFloat = 9
        func layout(_ size: CGFloat, minimumPadding: CGFloat = 4) -> (NSFont, [NSRect], CGFloat) {
            let font = NSFont.systemFont(ofSize: size, weight: .semibold)
            let widths = values.map { ceil(($0 as NSString).size(withAttributes: [.font: font]).width) }
            let remaining = available - widths.reduce(0, +)
                - CGFloat(max(0, values.count - 1)) * gap
            let padding = values.isEmpty ? minimumPadding
                : max(minimumPadding, min(5, remaining / CGFloat(values.count * 2)))
            var x: CGFloat = 10
            let height: CGFloat = 20
            let frames = widths.map { width -> NSRect in
                let chip = max(20, width + padding * 2)
                defer { x += chip + gap }
                return NSRect(x: x, y: (bounds.height - height) / 2, width: chip, height: height)
            }
            return (font, frames, (frames.last?.maxX ?? 10) - 10)
        }
        // Shrink in small steps against the measured layout (text width is not
        // exactly linear in point size, and short chips keep a 20 pt minimum).
        var size = baseSize
        var result = layout(size)
        while result.2 > available, size - 0.25 >= minimumFontSize {
            size -= 0.25
            result = layout(size)
        }
        // At the smallest legible size, tighten chip padding before overflowing.
        var padding: CGFloat = 4
        while result.2 > available, padding - 0.5 >= 2 {
            padding -= 0.5
            result = layout(size, minimumPadding: padding)
        }
        return (result.0, result.1)
    }

    func displayedChipFrames() -> [NSRect] { chipLayout(keys).1 }
    func displayedChipFontSize() -> CGFloat { chipLayout(keys).0.pointSize }

    /// Shipping `.shortcut-recorder`: a field of `kbd` chips, or the
    /// "Press shortcut…" prompt while empty.
    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-md")
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        tokens.color("surface-field").setFill(); outline.fill()
        let focused = recording || window?.firstResponder === self
        tokens.color(focused ? "theme-accent" : "control-border").setStroke()
        outline.lineWidth = 1; outline.stroke()
        if focused { drawPreferenceFocusRing(tokens, in: bounds.insetBy(dx: -1, dy: -1), radius: radius + 1) }
        guard !keys.isEmpty else {
            let prompt = PreferencesPolicy.text("shortcuts.prompt")
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: tokens.number("text-sm")),
                .foregroundColor: tokens.color("text-faint"),
            ]
            let size = (prompt as NSString).size(withAttributes: attributes)
            (prompt as NSString).draw(at: NSPoint(x: tokens.number("s-4"), y: (bounds.height - size.height) / 2),
                withAttributes: attributes)
            return
        }
        let (font, frames) = chipLayout(keys)
        let chipRadius = tokens.number("r-xs")
        for (value, frame) in zip(keys, frames) {
            let chip = NSBezierPath(roundedRect: frame.insetBy(dx: 0.5, dy: 0.5), xRadius: chipRadius, yRadius: chipRadius)
            tokens.color("surface-raised").setFill(); chip.fill()
            tokens.color("border").setStroke(); chip.lineWidth = 1; chip.stroke()
            // `border-bottom-width: 2px`.
            let bottom = NSBezierPath()
            bottom.move(to: NSPoint(x: frame.minX + chipRadius, y: frame.maxY - 1.5))
            bottom.line(to: NSPoint(x: frame.maxX - chipRadius, y: frame.maxY - 1.5))
            bottom.lineWidth = 1; bottom.stroke()
            let attributes: [NSAttributedString.Key: Any] = [
                .font: font, .foregroundColor: tokens.color("text-muted"),
            ]
            let size = (value as NSString).size(withAttributes: attributes)
            (value as NSString).draw(at: NSPoint(x: frame.midX - size.width / 2, y: frame.midY - size.height / 2 - 1),
                withAttributes: attributes)
        }
    }
}

/// Shipping Preferences copy and policy from `captures-app::preferences`
/// (`captures_preferences_v1`), shared with the wgpu host.
enum PreferencesPolicy {
    struct Row {
        let title: String
        let detail: String
        let accessibilityLabel: String
    }

    struct Section {
        let id: String
        let title: String
        let detail: String
    }

    struct Theme {
        let id: String
        let label: String
        let detail: String
        let accessibilityLabel: String
    }

    struct Emphasized {
        let lead: String
        let emphasis: String
        let trail: String
        var text: String { lead + emphasis + trail }
    }

    static func request(_ object: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: object)
        let response = String(decoding: data, as: UTF8.self).withCString {
            captures_preferences_v1($0)
        }
        guard let response else { throw AppBridgeError.invalidResponse }
        defer { captures_settings_free_v1(response) }
        return try AppBridge.decode(Data(bytes: response, count: strlen(response)))
    }

    static let copy: [String: Any] = {
        do { return try request(["operation": "copy", "platform": "macos"]) } catch {
            preconditionFailure("Preferences copy is unavailable: \(error)")
        }
    }()

    /// The string at a dotted path of the copy, such as "shortcuts.intro".
    static func text(_ path: String) -> String {
        var value: Any? = copy
        for key in path.split(separator: ".") { value = (value as? [String: Any])?[String(key)] }
        return value as? String ?? ""
    }

    /// A `{error}` template from the copy with the message filled in.
    static func template(_ path: String, error: String) -> String {
        text(path).replacingOccurrences(of: "{error}", with: error)
    }

    static func row(_ key: String) -> Row {
        let row = (copy["rows"] as? [String: Any])?[key] as? [String: Any] ?? [:]
        return Row(title: row.string("title"), detail: row.string("description"),
            accessibilityLabel: row.string("accessibility_label"))
    }

    static var sections: [Section] {
        (copy["sections"] as? [[String: Any]] ?? []).map {
            Section(id: $0.string("id"), title: $0.string("title"), detail: $0.string("description"))
        }
    }

    static var themes: [Theme] {
        (copy["themes"] as? [[String: Any]] ?? []).map {
            Theme(id: $0.string("id"), label: $0.string("name"), detail: $0.string("description"),
                accessibilityLabel: $0.string("accessibility_label"))
        }
    }

    /// Labelled choices for a select, keyed like `rows`.
    static func options(_ key: String) -> [(value: Any, label: String)] {
        let entries = (copy["options"] as? [String: Any])?[key] as? [[String: Any]] ?? []
        return entries.compactMap { entry in
            guard let value = entry["value"], let label = entry["label"] as? String else { return nil }
            return (value, label)
        }
    }

    /// `[{value, label}]` string pairs at the top level of the copy.
    static func pairs(_ key: String) -> [(value: String, label: String)] {
        (copy[key] as? [[String: Any]] ?? []).compactMap { entry in
            guard let value = entry["value"] as? String, let label = entry["label"] as? String else {
                return nil
            }
            return (value, label)
        }
    }

    static var recordingToggles: [String] { copy["recording_toggles"] as? [String] ?? [] }

    static var customThemeFields: [(key: String, label: String, detail: String)] {
        let fields = (copy["custom_theme"] as? [String: Any])?["fields"] as? [[String: Any]] ?? []
        return fields.map { ($0.string("key"), $0.string("label"), $0.string("description")) }
    }

    static var systemShortcutTargets: [String] {
        (copy["shortcuts"] as? [String: Any])?["system_targets"] as? [String] ?? []
    }

    /// State-dependent row copy (mini previews and recording controls).
    static func dynamicDescription(_ key: String, showMiniPreviews: Bool = true, include: Bool,
                            canExclude: Bool = true) -> Emphasized {
        let value = (try? request(["operation": "description", "key": key,
            "show_mini_previews": showMiniPreviews, "include": include,
            "can_exclude": canExclude])) ?? [:]
        return Emphasized(lead: value.string("lead"), emphasis: value.string("emphasis"),
            trail: value.string("trail"))
    }

    /// Matching text indices and the shipping "n of m" / "No results" label.
    static func find(query: String, texts: [String], index: Int) -> (matches: [Int], label: String) {
        let value = (try? request(["operation": "find", "query": query, "texts": texts,
            "index": max(0, index)])) ?? [:]
        let matches = (value["matches"] as? [NSNumber] ?? []).map(\.intValue)
        return (matches, value.string("label"))
    }
}

/// `--focus-ring-tight` drawn just inside `rect`.
func drawPreferenceFocusRing(_ tokens: Tokens, in rect: NSRect, radius: CGFloat) {
    let ring = NSBezierPath(roundedRect: rect.insetBy(dx: 1, dy: 1), xRadius: radius, yRadius: radius)
    ring.lineWidth = 2
    tokens.color("theme-accent").withAlphaComponent(0.55).setStroke()
    ring.stroke()
}

/// Hover tracking shared by the Preferences controls.
class PreferenceHoverButton: NSButton {
    private(set) var hovered = false
    private var hoverTracking: NSTrackingArea?
    var actionBlock: (() -> Void)?

    override var isFlipped: Bool { true }
    override var isEnabled: Bool { didSet { needsDisplay = true } }

    func configure(label: String, role: NSAccessibility.Role) {
        isBordered = false; setButtonType(.momentaryPushIn)
        target = self; action = #selector(activate)
        setAccessibilityRole(role); setAccessibilityLabel(label)
    }

    @objc private func activate() { if isEnabled { actionBlock?() } }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        let tracking = NSTrackingArea(rect: .zero,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking); hoverTracking = tracking
    }
    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }
    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder(); needsDisplay = true; return accepted
    }
    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder(); needsDisplay = true; return accepted
    }
    var isFocused: Bool { window?.firstResponder === self }
}

/// Shipping `.preferences-nav nav button`: left-aligned, hover and active fills.
final class PreferenceNavButton: PreferenceHoverButton {
    private let tokens: Tokens
    var active = false { didSet { setAccessibilitySelected(active); needsDisplay = true } }

    init(_ label: String, frame: NSRect, tokens: Tokens, onPress: @escaping () -> Void) {
        self.tokens = tokens
        super.init(frame: frame)
        title = label; actionBlock = onPress
        configure(label: label, role: .button)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-md")
        let path = NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius)
        if active { tokens.color("surface-active").setFill(); path.fill() }
        else if hovered { tokens.color("surface-hover").setFill(); path.fill() }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .medium),
            .foregroundColor: tokens.color(active || hovered ? "text" : "text-subtle"),
        ]
        let size = (title as NSString).size(withAttributes: attributes)
        (title as NSString).draw(at: NSPoint(x: tokens.number("s-4"), y: (bounds.height - size.height) / 2),
            withAttributes: attributes)
        if isFocused { drawPreferenceFocusRing(tokens, in: bounds, radius: radius) }
    }
}

/// Shipping `.check-row.switch-row`: the whole row is the control, with a
/// 32×19 switch at its trailing edge (accent when on).
final class PreferenceSwitchButton: PreferenceHoverButton {
    private let tokens: Tokens
    var isOn = false { didSet { setAccessibilityValue(isOn); needsDisplay = true } }

    init(frame: NSRect, tokens: Tokens, label: String, isOn: Bool, onPress: @escaping () -> Void) {
        self.tokens = tokens
        super.init(frame: frame)
        title = isOn ? "On" : "Off"; actionBlock = onPress
        configure(label: label, role: .checkBox)
        self.isOn = isOn
        setAccessibilityValue(isOn)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    var switchRect: NSRect { NSRect(x: bounds.maxX - 32, y: (bounds.height - 19) / 2, width: 32, height: 19) }

    override func draw(_ dirtyRect: NSRect) {
        let alpha: CGFloat = isEnabled ? 1 : 0.55
        let track = switchRect
        let path = NSBezierPath(roundedRect: track.insetBy(dx: 0.5, dy: 0.5), xRadius: 9, yRadius: 9)
        if isOn {
            tokens.color("theme-accent").withAlphaComponent(alpha).setFill(); path.fill()
        } else {
            tokens.color("surface-sunken").withAlphaComponent(alpha).setFill(); path.fill()
            tokens.color(isEnabled && hovered ? "text-subtle" : "border-strong")
                .withAlphaComponent(alpha).setStroke()
            path.lineWidth = 1; path.stroke()
        }
        let knob = NSRect(x: track.minX + 3 + (isOn ? 13 : 0), y: track.midY - 6.5, width: 13, height: 13)
        tokens.color(isOn ? "theme-accent-ink" : "text-subtle").withAlphaComponent(alpha).setFill()
        NSBezierPath(ovalIn: knob).fill()
        if isFocused { drawPreferenceFocusRing(tokens, in: track.insetBy(dx: -2, dy: -2), radius: 11) }
    }
}

/// One segment of the shipping `.ui-segmented` control.
final class PreferenceSegmentButton: PreferenceHoverButton {
    private let tokens: Tokens
    var active = false { didSet { setAccessibilityValue(active); needsDisplay = true } }

    init(_ label: String, frame: NSRect, tokens: Tokens, onPress: @escaping () -> Void) {
        self.tokens = tokens
        super.init(frame: frame)
        title = label; actionBlock = onPress
        configure(label: label, role: .radioButton)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    static func width(_ label: String, tokens: Tokens) -> CGFloat {
        let font = NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        return ceil((label as NSString).size(withAttributes: [.font: font]).width) + tokens.number("s-5") * 2
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-sm")
        if active {
            let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
            tokens.color("surface-raised").setFill(); path.fill()
            tokens.color("border-subtle").setStroke(); path.lineWidth = 1; path.stroke()
        }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium),
            .foregroundColor: tokens.color(active || hovered ? "text" : "text-subtle"),
        ]
        let size = (title as NSString).size(withAttributes: attributes)
        (title as NSString).draw(at: NSPoint(x: (bounds.width - size.width) / 2,
            y: (bounds.height - size.height) / 2), withAttributes: attributes)
        if isFocused { drawPreferenceFocusRing(tokens, in: bounds, radius: radius) }
    }
}

/// One shipping `.theme-option` accent chip: swatch, name and a check.
final class ThemeChipButton: PreferenceHoverButton {
    private let tokens: Tokens
    private let chipLabel: String
    /// Accent and signal for the swatch, or nil for the custom gradient.
    private let swatch: (accent: NSColor, signal: NSColor)?
    var active = false { didSet { setAccessibilityValue(active); needsDisplay = true } }

    init(frame: NSRect, tokens: Tokens, label: String, accessibility: String,
         swatch: (accent: NSColor, signal: NSColor)?, onPress: @escaping () -> Void) {
        self.tokens = tokens; chipLabel = label; self.swatch = swatch
        super.init(frame: frame)
        title = ""; actionBlock = onPress
        configure(label: accessibility, role: .radioButton)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-md")
        let chip = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        if active {
            tokens.color("surface-raised").setFill(); chip.fill()
            tokens.color("border").setStroke(); chip.lineWidth = 1; chip.stroke()
        } else if hovered {
            tokens.color("surface-hover").setFill(); chip.fill()
        }
        let gap = tokens.number("s-3")
        let square = NSRect(x: gap, y: (bounds.height - 18) / 2, width: 18, height: 18)
        let swatchRadius = tokens.number("r-sm")
        let shape = NSBezierPath(roundedRect: square, xRadius: swatchRadius, yRadius: swatchRadius)
        NSGraphicsContext.saveGraphicsState()
        shape.addClip()
        if let swatch {
            swatch.accent.setFill(); square.fill()
            // The signal hue is a wedge in the lower right half.
            let wedge = NSBezierPath()
            wedge.move(to: NSPoint(x: square.maxX, y: square.minY))
            wedge.line(to: NSPoint(x: square.maxX, y: square.maxY))
            wedge.line(to: NSPoint(x: square.midX, y: square.maxY))
            wedge.close()
            swatch.signal.setFill(); wedge.fill()
        } else {
            let stops = [(255, 71, 87), (255, 166, 61), (245, 225, 61), (69, 219, 124),
                         (64, 205, 237), (192, 82, 245), (255, 82, 180)].map {
                NSColor(srgbRed: CGFloat($0.0) / 255, green: CGFloat($0.1) / 255,
                        blue: CGFloat($0.2) / 255, alpha: 1)
            }
            NSGradient(colors: stops)?.draw(in: square, angle: 0)
        }
        NSGraphicsContext.restoreGraphicsState()
        NSColor.black.withAlphaComponent(0.16).setStroke()
        let edge = NSBezierPath(roundedRect: square.insetBy(dx: 0.5, dy: 0.5),
            xRadius: swatchRadius, yRadius: swatchRadius)
        edge.lineWidth = 1; edge.stroke()
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium),
            .foregroundColor: tokens.color(active ? "text" : "text-muted"),
        ]
        let size = (chipLabel as NSString).size(withAttributes: attributes)
        (chipLabel as NSString).draw(at: NSPoint(x: square.maxX + gap, y: (bounds.height - size.height) / 2),
            withAttributes: attributes)
        if active {
            let dark = tokens.color("text").brightnessComponent > 0.5
            tokens.color(dark ? "theme-accent" : "theme-accent-readable").setStroke()
            ShippingIcons.stroke("check", in: NSRect(x: bounds.maxX - gap - 11,
                y: (bounds.height - 11) / 2, width: 11, height: 11))
        }
        if isFocused { drawPreferenceFocusRing(tokens, in: bounds, radius: radius) }
    }
}

/// One corner of the shipping mini preview placement picker.
final class PreferenceCornerButton: PreferenceHoverButton {
    private let tokens: Tokens
    var active = false { didSet { setAccessibilityValue(active); needsDisplay = true } }

    init(frame: NSRect, tokens: Tokens, label: String, onPress: @escaping () -> Void) {
        self.tokens = tokens
        super.init(frame: frame)
        title = ""; actionBlock = onPress
        configure(label: label, role: .radioButton)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-sm")
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 2.5, dy: 2.5), xRadius: radius, yRadius: radius)
        if active {
            tokens.color("theme-accent").setFill(); path.fill()
        } else {
            tokens.color("surface-raised").setFill(); path.fill()
            tokens.color(isEnabled && hovered ? "theme-accent" : "border-strong").setStroke()
            path.lineWidth = 1; path.stroke()
        }
        if isFocused {
            let ring = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                xRadius: radius + 2, yRadius: radius + 2)
            ring.lineWidth = 1; tokens.color("theme-accent").setStroke(); ring.stroke()
        }
    }
}

/// The marker in the shipping `.preferences-save-status` pill.
final class PreferenceStatusMark: NSView {
    private let tokens: Tokens
    var kind = "idle" { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: frame)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        let circle = bounds.insetBy(dx: 1, dy: 1)
        switch kind {
        case "saved":
            tokens.color("positive").setFill(); NSBezierPath(ovalIn: circle).fill()
            tokens.color("positive-ink").setStroke()
            ShippingIcons.stroke("check", in: circle.insetBy(dx: 1.5, dy: 1.5))
        case "error":
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 9, weight: .bold),
                .foregroundColor: tokens.color("danger-text"),
            ]
            let size = ("!" as NSString).size(withAttributes: attributes)
            ("!" as NSString).draw(at: NSPoint(x: (bounds.width - size.width) / 2,
                y: (bounds.height - size.height) / 2), withAttributes: attributes)
        default:
            let ring = NSBezierPath(ovalIn: circle.insetBy(dx: 1, dy: 1))
            ring.lineWidth = 2; tokens.color("border-strong").setStroke(); ring.stroke()
            let arc = NSBezierPath()
            arc.appendArc(withCenter: NSPoint(x: bounds.midX, y: bounds.midY), radius: circle.width / 2 - 1,
                startAngle: 200, endAngle: 330)
            arc.lineWidth = 2; tokens.color("theme-accent").setStroke(); arc.stroke()
        }
    }
}

final class PreferencesController: NSObject, NSTextFieldDelegate {
    typealias ShortcutPolicy = (String, Bool, Bool, Bool, Bool) throws -> [String: Any]
    typealias ShortcutDisplay = (String) throws -> [String]

    private let root: Surface
    private let store: SettingsStore
    private let tokensProvider: () -> Tokens
    private let appearanceChanged: (String, String, [String: Any]) -> Void
    private let settingsChanged: ([String: Any]) -> Void
    private let settingsPersisted: ([String: Any]) -> Void
    private let shortcutPolicy: ShortcutPolicy
    private let shortcutDisplay: ShortcutDisplay
    private let showHistory: () -> Void
    private let showFeedback: () -> Void
    private let liveCaptureAvailable: Bool
    private let loginItemService: LoginItemServicing?
    private var settings: [String: Any] = [:]
    /// Default microphone choices, enumerated off the main thread once.
    private var microphones: [NativeMicrophoneDevice]?
    private var microphonesLoading = false
    /// Refreshed in place when devices arrive: rebuilding every card would
    /// reset controls in use, such as an in-progress shortcut recording.
    private weak var microphonePopUp: ClosurePopUpButton?
    private var scroll = NSScrollView()
    private var document = Surface()
    private var headerRule = Surface()
    private var status = NSTextField(labelWithString: "")
    private var statusPill = Surface()
    private var statusMark: PreferenceStatusMark?
    private var statusKind = "idle"
    private var statusGeneration = 0
    private var historyButton: CaptureButton?
    private var retryButton: CaptureButton?
    private var findBar: Surface?
    private var findField: NSTextField?
    private var findCount: NSTextField?
    private var findPrevious: CaptureButton?
    private var findNext: CaptureButton?
    private var matches: [Int] = []
    private var matchIndex = 0
    private var latestRevision = 0
    private var saveFailed = false
    private var rebuilding = false
    private var shortcutRecorders: [String: ShortcutRecorderButton] = [:]
    private var shortcutErrors: [String: NSTextField] = [:]
    private weak var recordingShortcut: ShortcutRecorderButton?
    private var shortcutEventMonitor: Any?
    private var shortcutFocusObserver: NSObjectProtocol?
    private let sections = PreferencesPolicy.sections.map { ($0.id, $0.title) }
    private var sectionViews: [String: NSView] = [:]
    /// Nav entries, highlighted for the section in view (shipping scroll-spy).
    private var navButtons: [String: PreferenceNavButton] = [:]
    private(set) var activeSection = "appearance"
    private var scrollObserver: NSObjectProtocol?
    /// Find targets: a hidden wash behind each row and the row's text.
    private var searchable: [(NSView, String)] = []
    private var loginItemEnabled: Bool?
    private var loginItemPending = false
    private var loginItemError: String?
    private var loginItemGeneration = 0
    private var keyboardSettingsError: String?
    /// Capture-menu deep link: the setting row to reveal and highlight.
    private(set) var highlightedSetting: String?
    private var highlightRevealed = false
    private var highlightGeneration = 0
    private weak var highlightView: NSView?

    private static let sidebarWidth: CGFloat = 196
    private static let headerHeight: CGFloat = 72
    private static let findHeight: CGFloat = 48
    private static let shortcutErrorHeight: CGFloat = 18

    init(root: Surface, store: SettingsStore, tokens: @escaping () -> Tokens,
         appearanceChanged: @escaping (String, String, [String: Any]) -> Void,
         settingsChanged: @escaping ([String: Any]) -> Void = { _ in },
         settingsPersisted: @escaping ([String: Any]) -> Void = { _ in },
         shortcutPolicy: @escaping ShortcutPolicy = { code, control, shift, alt, meta in
             try NativeCaptureShortcuts.record(code: code, control: control, shift: shift,
                 alt: alt, meta: meta)
         },
         shortcutDisplay: @escaping ShortcutDisplay = { try NativeCaptureShortcuts.display($0) },
         showHistory: @escaping () -> Void, liveCaptureAvailable: Bool = false,
         showFeedback: @escaping () -> Void = {},
         loginItemService: LoginItemServicing? = nil,
         initialAppearance: String? = nil, initialTheme: String? = nil) {
        self.root = root; self.store = store; tokensProvider = tokens
        self.appearanceChanged = appearanceChanged; self.showHistory = showHistory
        self.showFeedback = showFeedback
        self.settingsChanged = settingsChanged
        self.settingsPersisted = settingsPersisted
        self.shortcutPolicy = shortcutPolicy; self.shortcutDisplay = shortcutDisplay
        self.liveCaptureAvailable = liveCaptureAvailable
        self.loginItemService = loginItemService
        super.init()
        buildShell()
        setStatus(PreferencesPolicy.text("loading"), kind: "saving")
        store.load { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let value):
                self.settings = value
                if let initialAppearance { self.settings["appearance"] = initialAppearance }
                if let initialTheme { self.settings["theme"] = initialTheme }
                self.appearanceChanged(self.settings.string("appearance", "system"), self.settings.string("theme", "mustard"), value["custom_theme"] as? [String: Any] ?? [:])
                self.settingsChanged(self.settings)
                self.settingsPersisted(self.settings)
                self.restyle()
                self.setStatus("", kind: "idle")
                self.queryLoginItem()
            case .failure(let error):
                self.setStatus(PreferencesPolicy.template("load_error_template",
                    error: error.localizedDescription), kind: "error")
            }
        }
    }

    private var tokens: Tokens { tokensProvider() }
    private var dark: Bool { tokens.color("text").brightnessComponent > 0.5 }

    // MARK: Shell

    private func buildShell() {
        let width = Self.sidebarWidth
        let nav = Surface(frame: NSRect(x: 0, y: 0, width: width, height: root.bounds.height))
        nav.autoresizingMask = [.height]
        nav.wantsLayer = true; nav.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        root.addSubview(nav)
        let border = Surface(frame: NSRect(x: width - 1, y: 0, width: 1, height: root.bounds.height))
        border.autoresizingMask = [.height]
        border.wantsLayer = true; border.layer?.backgroundColor = tokens.color("border-subtle").cgColor
        nav.addSubview(border)
        // Shipping `.preferences-nav-brand`: a 26 pt accent tile and the name.
        let padding = tokens.number("s-5"), top = tokens.number("s-6")
        let mark = BrandMarkView(frame: NSRect(x: padding + tokens.number("s-3"), y: top, width: 26, height: 26),
            tokens: tokens)
        nav.addSubview(mark)
        addLabel("Captures", frame: NSRect(x: mark.frame.maxX + tokens.number("s-4"), y: top + 4,
            width: 120, height: 18), size: tokens.number("text-md"), weight: .semibold, parent: nav)
        let itemHeight = tokens.number("h-md")
        for (index, section) in sections.enumerated() {
            let y = top + 26 + tokens.number("s-6") + CGFloat(index) * (itemHeight + 1)
            let button = PreferenceNavButton(section.1,
                frame: NSRect(x: padding, y: y, width: width - padding * 2, height: itemHeight),
                tokens: tokens) { [weak self] in self?.reveal(section.0) }
            nav.addSubview(button)
            navButtons[section.0] = button
        }
        let left = width + tokens.number("s-8")
        addLabel(PreferencesPolicy.text("title"), frame: NSRect(x: left, y: 16, width: 320, height: 24),
            size: tokens.number("text-xl"), weight: .semibold, parent: root)
        addLabel(PreferencesPolicy.text("subtitle"), frame: NSRect(x: left, y: 41, width: 320, height: 16),
            size: tokens.number("text-sm"), muted: true, parent: root)
        let historyTitle = PreferencesPolicy.text("history")
        historyButton = actionButton(historyTitle, x: 0, y: 20, width: buttonWidth(historyTitle),
            parent: root) { [weak self] in self?.flush(); self?.showHistory() }
        historyButton?.autoresizingMask = [.minXMargin]
        statusPill = Surface(frame: NSRect(x: 0, y: 22, width: 120, height: tokens.number("h-sm")))
        statusPill.wantsLayer = true; statusPill.layer?.cornerRadius = tokens.number("h-sm") / 2
        statusPill.autoresizingMask = [.minXMargin]; statusPill.isHidden = true
        let mark13 = PreferenceStatusMark(frame: NSRect(x: tokens.number("s-4"),
            y: (tokens.number("h-sm") - 13) / 2, width: 13, height: 13), tokens: tokens)
        statusPill.addSubview(mark13); statusMark = mark13
        status.frame = NSRect(x: mark13.frame.maxX + tokens.number("s-3"), y: 7, width: 80, height: 15)
        status.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        status.lineBreakMode = .byTruncatingTail
        statusPill.addSubview(status); root.addSubview(statusPill)
        retryButton = actionButton("Retry", x: 0, y: 20, width: 64, parent: root) { [weak self] in self?.retrySave() }
        retryButton?.autoresizingMask = [.minXMargin]; retryButton?.isHidden = true
        headerRule = Surface(frame: NSRect(x: width, y: Self.headerHeight, width: root.bounds.width - width, height: 1))
        headerRule.autoresizingMask = [.width]
        headerRule.wantsLayer = true; headerRule.layer?.backgroundColor = tokens.color("border-subtle").cgColor
        root.addSubview(headerRule)
        scroll = NSScrollView(frame: NSRect(x: width, y: Self.headerHeight + 1, width: root.bounds.width - width,
            height: root.bounds.height - Self.headerHeight - 1))
        scroll.autoresizingMask = [.width, .height]; scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        document = Surface(frame: NSRect(x: 0, y: 0, width: scroll.bounds.width, height: 400))
        scroll.documentView = document; root.addSubview(scroll)
        scroll.contentView.postsBoundsChangedNotifications = true
        scrollObserver = NotificationCenter.default.addObserver(
            forName: NSView.boundsDidChangeNotification, object: scroll.contentView, queue: .main
        ) { [weak self] _ in self?.updateActiveSection() }
        layoutHeaderActions()
        updateActiveSection()
    }

    /// Shipping `.preferences-header-actions`: History, then the save status
    /// (and the native Retry after a failed save) at the trailing edge.
    private func layoutHeaderActions() {
        var x = root.bounds.width - tokens.number("s-8")
        if let retryButton, !retryButton.isHidden {
            retryButton.frame.origin.x = x - retryButton.frame.width
            x = retryButton.frame.minX - tokens.number("s-4")
        }
        if !statusPill.isHidden {
            statusPill.frame.origin.x = x - statusPill.frame.width
            x = statusPill.frame.minX - tokens.number("s-4")
        }
        if let historyButton { historyButton.frame.origin.x = x - historyButton.frame.width }
    }

    /// The last section whose card top has scrolled past the top of the view,
    /// or the final section once the document reaches its end.
    func updateActiveSection() {
        let visible = scroll.contentView.bounds
        let atEnd = visible.minY > 0 && visible.maxY >= document.frame.height - 1
        let active = atEnd ? sections.last?.0 : sections.last(where: {
            (sectionViews[$0.0]?.frame.minY ?? .infinity) <= visible.minY + 80
        })?.0
        activeSection = active ?? sections[0].0
        for (id, button) in navButtons { button.active = id == activeSection }
    }

    private func rebuildCards() {
        let oldY = scroll.contentView.bounds.origin.y
        document.subviews.forEach { $0.removeFromSuperview() }; sectionViews.removeAll(); searchable.removeAll()
        document.frame.size.width = scroll.contentSize.width
        var y = tokens.number("s-8")
        y = appearanceCard(y); y = captureCard(y); y = shortcutsCard(y); y = recordingCard(y); y = gifCard(y)
        y = updatesCard(y); y = aboutCard(y)
        document.frame.size = NSSize(width: scroll.contentSize.width, height: y - tokens.number("s-6") + tokens.number("s-12"))
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(oldY, max(0, document.frame.height - scroll.contentSize.height))))
        updateFind()
        updateActiveSection()
        if !rebuilding { revealHighlightIfNeeded() }
    }

    /// Shipping `preferences-target`: scroll a setting row to the middle of the
    /// view and highlight it for `PREFERENCE_HIGHLIGHT` (2.4 s).
    func revealSetting(_ key: String) {
        highlightedSetting = key; highlightRevealed = false
        highlightGeneration += 1
        let generation = highlightGeneration
        if !settings.isEmpty { rebuildCards() }
        DispatchQueue.main.asyncAfter(deadline: .now() + CaptureMenuPolicy.copy.highlightSeconds) {
            [weak self] in
            guard let self, self.highlightGeneration == generation else { return }
            self.highlightedSetting = nil
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = Double(self.tokens.number("dur-3")) / 1000
                self.highlightView?.animator().alphaValue = 0
            }, completionHandler: { [weak self] in
                guard let self, self.highlightedSetting == nil else { return }
                self.highlightView?.removeFromSuperview()
            })
        }
    }

    /// The highlighted row's frame in the scrolled document, if shown.
    var highlightedRowFrame: NSRect? {
        guard let view = highlightView, let parent = view.superview else { return nil }
        return parent.convert(view.frame, to: document)
    }

    private func revealHighlightIfNeeded() {
        guard !highlightRevealed, let target = highlightedRowFrame else { return }
        highlightRevealed = true
        let visible = scroll.contentSize.height
        let maxY = max(0, document.frame.height - visible)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(maxY, max(0, target.midY - visible / 2))))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    // MARK: Card and row layout

    private var cardWidth: CGFloat { min(720 - tokens.number("s-8") * 2, scroll.contentSize.width - tokens.number("s-8") * 2) }
    private var contentWidth: CGFloat { cardWidth - tokens.number("s-6") * 2 }
    private var inset: CGFloat { tokens.number("s-6") }

    /// A shipping `.settings-card`: title, description and the first row's y.
    private func makeCard(_ id: String, y: CGFloat, detail: String? = nil) -> (Surface, CGFloat) {
        let section = PreferencesPolicy.sections.first(where: { $0.id == id })
        let title = section?.title ?? id
        let description = detail ?? section?.detail ?? ""
        let card = Surface(frame: NSRect(x: tokens.number("s-8"), y: y, width: cardWidth, height: 200))
        card.identifier = NSUserInterfaceItemIdentifier("preferences-card.\(id)")
        card.wantsLayer = true; card.layer?.cornerRadius = tokens.number("r-xl")
        card.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        card.layer?.borderWidth = 1; card.layer?.borderColor = tokens.color("border-subtle").cgColor
        card.layer?.shadowColor = NSColor.black.cgColor
        card.layer?.shadowOpacity = dark ? 0.32 : 0.08
        card.layer?.shadowRadius = dark ? 3 : 1.5
        card.layer?.shadowOffset = NSSize(width: 0, height: dark ? -2 : -1)
        document.addSubview(card); sectionViews[id] = card
        let detailWidth = min(contentWidth, descriptionWidth)
        let detailHeight = textHeight(description, size: tokens.number("text-sm"), width: detailWidth)
        let background = findBackground(NSRect(x: inset, y: inset, width: contentWidth,
            height: 20 + tokens.number("s-2") + detailHeight), parent: card)
        addLabel(title, frame: NSRect(x: inset, y: inset, width: contentWidth, height: 20),
            size: tokens.number("text-lg"), weight: .semibold, parent: card)
        wrappingLabel(description, frame: NSRect(x: inset, y: inset + 20 + tokens.number("s-2"),
            width: detailWidth, height: detailHeight), size: tokens.number("text-sm"), parent: card)
        searchable.append((background, "\(title) \(description)"))
        return (card, inset + 20 + tokens.number("s-2") + detailHeight + tokens.number("s-5") + tokens.number("s-1"))
    }

    private func finish(_ card: Surface, _ y: CGFloat) -> CGFloat {
        card.frame.size.height = y + inset
        return card.frame.maxY + tokens.number("s-6")
    }

    /// Shipping `max-width: 52ch` for row and card descriptions.
    private var descriptionWidth: CGFloat { 52 * tokens.number("text-sm") * 0.56 }

    private func textHeight(_ text: String, size: CGFloat, width: CGFloat,
                            weight: NSFont.Weight = .regular) -> CGFloat {
        guard !text.isEmpty else { return 0 }
        let bounds = (text as NSString).boundingRect(
            with: NSSize(width: width - 4, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: NSFont.systemFont(ofSize: size, weight: weight)])
        return ceil(bounds.height) + 2
    }

    /// Shipping `.settings-card > * + *`: a subtle rule between rows.
    private func divider(_ y: CGFloat, _ card: NSView) -> CGFloat {
        let line = Surface(frame: NSRect(x: inset, y: y + tokens.number("s-5"), width: contentWidth, height: 1))
        line.wantsLayer = true; line.layer?.backgroundColor = tokens.color("border-subtle").cgColor
        card.addSubview(line)
        return y + tokens.number("s-5") * 2 + 1
    }

    /// A hidden find wash behind a row (`.preference-find-match`).
    private func findBackground(_ rect: NSRect, parent: NSView) -> Surface {
        let wash = Surface(frame: rect.insetBy(dx: -tokens.number("s-3"), dy: -tokens.number("s-3")))
        wash.wantsLayer = true; wash.layer?.cornerRadius = tokens.number("r-lg")
        wash.isHidden = true; wash.setAccessibilityElement(false)
        parent.addSubview(wash)
        return wash
    }

    /// Row copy: a `--text-md` title over a subtle, 52ch description.
    private func copyHeight(_ title: String, detail: String, width: CGFloat) -> CGFloat {
        let titleHeight = textHeight(title, size: tokens.number("text-md"), width: width, weight: .medium)
        let detailHeight = textHeight(detail, size: tokens.number("text-sm"), width: min(width, descriptionWidth))
        return titleHeight + (detail.isEmpty ? 0 : 3 + detailHeight)
    }

    @discardableResult
    private func addCopy(_ title: String, detail: String, emphasis: PreferencesPolicy.Emphasized? = nil,
                         x: CGFloat, y: CGFloat, width: CGFloat, parent: NSView,
                         enabled: Bool = true) -> [NSTextField] {
        let titleHeight = textHeight(title, size: tokens.number("text-md"), width: width, weight: .medium)
        let first = addLabel(title, frame: NSRect(x: x, y: y, width: width, height: titleHeight),
            size: tokens.number("text-md"), weight: .medium, parent: parent)
        var labels = [first]
        let text = emphasis?.text ?? detail
        if !text.isEmpty {
            let detailWidth = min(width, descriptionWidth)
            let second = wrappingLabel(text, frame: NSRect(x: x, y: y + titleHeight + 3, width: detailWidth,
                height: textHeight(text, size: tokens.number("text-sm"), width: detailWidth)),
                size: tokens.number("text-sm"), parent: parent)
            if let emphasis, !emphasis.emphasis.isEmpty {
                let font = NSFont.systemFont(ofSize: tokens.number("text-sm"))
                let rich = NSMutableAttributedString(string: emphasis.lead,
                    attributes: [.font: font, .foregroundColor: tokens.color("text-subtle")])
                rich.append(NSAttributedString(string: emphasis.emphasis, attributes: [
                    .font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .semibold),
                    .foregroundColor: tokens.color("text")]))
                rich.append(NSAttributedString(string: emphasis.trail,
                    attributes: [.font: font, .foregroundColor: tokens.color("text-subtle")]))
                second.attributedStringValue = rich
            }
            labels.append(second)
        }
        if !enabled { labels.forEach { $0.alphaValue = 0.55 } }
        return labels
    }

    /// Shipping `.setting-row-inline`: copy on the left and a control of
    /// `control` size, both centered on the row. Returns the row rect and the
    /// control frame.
    private func inlineRow(_ title: String, detail: String, emphasis: PreferencesPolicy.Emphasized? = nil,
                           control: NSSize, y: CGFloat, card: NSView, enabled: Bool = true,
                           highlight key: String? = nil) -> (row: NSRect, control: NSRect) {
        let width = contentWidth - control.width - tokens.number("s-6")
        let text = emphasis?.text ?? detail
        let copy = copyHeight(title, detail: text, width: width)
        let height = max(copy, control.height)
        let row = NSRect(x: inset, y: y, width: contentWidth, height: height)
        if let key, key == highlightedSetting {
            // Shipping `.preference-target-highlight`: selected wash and accent ring.
            let highlight = Surface(frame: row.insetBy(dx: -tokens.number("s-4"), dy: -tokens.number("s-4")))
            highlight.identifier = NSUserInterfaceItemIdentifier("preferences-highlight.\(key)")
            highlight.wantsLayer = true
            highlight.layer?.cornerRadius = tokens.number("r-lg")
            highlight.layer?.backgroundColor = tokens.color("surface-selected").cgColor
            highlight.layer?.borderWidth = 2
            highlight.layer?.borderColor = tokens.color("theme-accent").withAlphaComponent(0.62).cgColor
            highlight.setAccessibilityElement(false)
            card.addSubview(highlight); highlightView = highlight
        }
        searchable.append((findBackground(row, parent: card), "\(title) \(text)"))
        addCopy(title, detail: detail, emphasis: emphasis, x: inset, y: y + (height - copy) / 2,
            width: width, parent: card, enabled: enabled)
        let frame = NSRect(x: row.maxX - control.width, y: y + (height - control.height) / 2,
            width: control.width, height: control.height)
        return (row, frame)
    }

    /// A whole-row switch for a boolean setting at `path` (top level or
    /// `["recording", key]`).
    @discardableResult
    private func switchRow(_ path: [String], title: String, detail: String,
                           emphasis: PreferencesPolicy.Emphasized? = nil, y: CGFloat, card: NSView,
                           enabled: Bool = true) -> CGFloat {
        let isOn = boolSetting(path)
        let layout = inlineRow(title, detail: detail, emphasis: emphasis, control: NSSize(width: 32, height: 19),
            y: y, card: card, enabled: enabled, highlight: path.count == 1 ? path[0] : nil)
        let button = PreferenceSwitchButton(frame: layout.row, tokens: tokens, label: title, isOn: isOn) {
            [weak self] in
            guard let self else { return }
            self.setBool(!self.boolSetting(path), path: path)
        }
        button.identifier = NSUserInterfaceItemIdentifier("setting.\(path.joined(separator: "."))")
        button.isEnabled = enabled
        card.addSubview(button)
        return layout.row.maxY
    }

    private func boolSetting(_ path: [String]) -> Bool {
        if path.count == 2 { return (settings[path[0]] as? [String: Any] ?? [:]).bool(path[1]) }
        return settings.bool(path[0])
    }

    private func setBool(_ value: Bool, path: [String]) {
        if path.count == 2 {
            var nested = settings[path[0]] as? [String: Any] ?? [:]
            nested[path[1]] = value; settings[path[0]] = nested
        } else {
            settings[path[0]] = value
        }
        changed(rerender: true)
    }

    /// A shipping `CustomSelect` for `key` (`recording.` prefixes nested keys).
    private func select(_ key: String, frame: NSRect, parent: NSView, enabled: Bool = true) {
        let options = PreferencesPolicy.options(key)
        let nested = key.hasPrefix("recording.")
        let name = nested ? String(key.dropFirst("recording.".count)) : key
        let current = nested ? (settings["recording"] as? [String: Any])?[name] : settings[name]
        let menu = ClosurePopUpButton(frame: frame, pullsDown: false)
        menu.tokens = tokens
        // Add items individually: labels may repeat.
        for option in options { menu.menu?.addItem(NSMenuItem(title: option.label, action: nil, keyEquivalent: "")) }
        let selected = current.map { String(describing: $0) }
        menu.selectItem(at: options.firstIndex { String(describing: $0.value) == selected } ?? 0)
        menu.setAccessibilityLabel(PreferencesPolicy.row(key).accessibilityLabel)
        menu.isEnabled = enabled
        menu.change = { [weak self] index in
            guard let self, options.indices.contains(index) else { return }
            if nested {
                var recording = self.settings["recording"] as? [String: Any] ?? [:]
                recording[name] = options[index].value; self.settings["recording"] = recording
            } else {
                self.settings[name] = options[index].value
            }
            self.changed(rerender: false)
        }
        menu.target = menu; menu.action = #selector(ClosurePopUpButton.selectedValue)
        parent.addSubview(menu)
    }

    private func selectRow(_ key: String, y: CGFloat, card: NSView) -> CGFloat {
        let copy = PreferencesPolicy.row(key)
        let layout = inlineRow(copy.title, detail: copy.detail,
            control: NSSize(width: 160, height: tokens.number("h-md")), y: y, card: card)
        select(key, frame: layout.control, parent: card)
        return layout.row.maxY
    }

    /// Shipping `.setting-grid`: stacked title-over-select cells.
    private func selectGrid(_ keys: [String], y: CGFloat, card: NSView) -> CGFloat {
        let gap = tokens.number("s-5")
        let width = (contentWidth - gap * CGFloat(keys.count - 1)) / CGFloat(keys.count)
        let height = 17 + tokens.number("s-3") + tokens.number("h-md")
        for (index, key) in keys.enumerated() {
            let x = inset + CGFloat(index) * (width + gap)
            let copy = PreferencesPolicy.row(key)
            searchable.append((findBackground(NSRect(x: x, y: y, width: width, height: height), parent: card),
                copy.title))
            addLabel(copy.title, frame: NSRect(x: x, y: y, width: width, height: 17),
                size: tokens.number("text-md"), weight: .medium, parent: card)
            select(key, frame: NSRect(x: x, y: y + 17 + tokens.number("s-3"), width: width,
                height: tokens.number("h-md")), parent: card)
        }
        return y + height
    }

    /// Shipping `.settings-utility-row`: copy and one secondary action.
    @discardableResult
    private func utilityRow(_ title: String, detail: String, action: String, y: CGFloat, card: NSView,
                            enabled: Bool = true, identifier: String? = nil,
                            onPress: @escaping () -> Void) -> CGFloat {
        let width = buttonWidth(action)
        let layout = inlineRow(title, detail: detail, control: NSSize(width: width, height: tokens.number("h-md")),
            y: y, card: card)
        let button = CaptureButton(action, frame: layout.control, tokens: tokens, action: onPress)
        button.isEnabled = enabled
        if let identifier { button.identifier = NSUserInterfaceItemIdentifier(identifier) }
        card.addSubview(button)
        return layout.row.maxY
    }

    private func buttonWidth(_ title: String) -> CGFloat {
        let font = NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .medium)
        return ceil((title as NSString).size(withAttributes: [.font: font]).width) + tokens.number("s-5") * 2 + 4
    }

    // MARK: Cards

    private func appearanceCard(_ y: CGFloat) -> CGFloat {
        let (card, top) = makeCard("appearance", y: y)
        var y = top
        let appearance = PreferencesPolicy.row("appearance")
        let modes = PreferencesPolicy.pairs("appearance_modes")
        let widths = modes.map { PreferenceSegmentButton.width($0.label, tokens: tokens) }
        let segmentHeight = tokens.number("h-sm")
        let layout = inlineRow(appearance.title, detail: appearance.detail,
            control: NSSize(width: widths.reduce(0, +) + 8, height: segmentHeight + 8), y: y, card: card)
        let track = Surface(frame: layout.control)
        track.wantsLayer = true; track.layer?.cornerRadius = tokens.number("r-lg")
        track.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        track.layer?.borderWidth = 1; track.layer?.borderColor = tokens.color("border-subtle").cgColor
        track.setAccessibilityRole(.radioGroup); track.setAccessibilityLabel(appearance.title)
        card.addSubview(track)
        let selected = settings.string("appearance", "system")
        var x: CGFloat = 4
        for (mode, width) in zip(modes, widths) {
            let value = mode.value
            let button = PreferenceSegmentButton(mode.label, frame: NSRect(x: x, y: 4, width: width, height: segmentHeight),
                tokens: tokens) { [weak self] in self?.set(value, for: "appearance", rerender: true) }
            button.active = selected == value
            track.addSubview(button)
            x += width
        }
        y = divider(layout.row.maxY, card)
        let accent = PreferencesPolicy.row("theme")
        let accentHeight = copyHeight(accent.title, detail: accent.detail, width: contentWidth)
        searchable.append((findBackground(NSRect(x: inset, y: y, width: contentWidth, height: accentHeight),
            parent: card), "\(accent.title) \(accent.detail)"))
        addCopy(accent.title, detail: accent.detail, x: inset, y: y, width: contentWidth, parent: card)
        y += accentHeight + tokens.number("s-4")
        let columns = (PreferencesPolicy.copy["theme_columns"] as? NSNumber)?.intValue ?? 5
        let gap = tokens.number("s-2")
        let chipWidth = (contentWidth - gap * CGFloat(columns - 1)) / CGFloat(columns)
        let theme = settings.string("theme", "mustard")
        let mode = dark ? "dark" : "light"
        for (index, choice) in PreferencesPolicy.themes.enumerated() {
            let frame = NSRect(x: inset + CGFloat(index % columns) * (chipWidth + gap),
                y: y + CGFloat(index / columns) * (34 + gap), width: chipWidth, height: 34)
            let preview = Tokens.variants["\(mode)-\(choice.id)"]
            let swatch = choice.id == "custom" ? nil
                : (accent: (preview ?? tokens).color("theme-accent"), signal: (preview ?? tokens).color("theme-signal"))
            searchable.append((findBackground(frame, parent: card), "\(choice.label) \(choice.detail)"))
            let value = choice.id
            let chip = ThemeChipButton(frame: frame, tokens: tokens, label: choice.label,
                accessibility: choice.accessibilityLabel, swatch: swatch) { [weak self] in
                self?.set(value, for: "theme", rerender: true)
            }
            chip.toolTip = choice.detail
            chip.active = theme == choice.id
            card.addSubview(chip)
        }
        let rows = (PreferencesPolicy.themes.count + columns - 1) / columns
        y += CGFloat(rows) * 34 + CGFloat(max(0, rows - 1)) * gap
        if theme == "custom" {
            y = divider(y, card)
            y = customThemeEditor(y, card: card)
        }
        return finish(card, y)
    }

    /// Shipping `.custom-theme-editor`: heading, Reset colors and two fields.
    private func customThemeEditor(_ y: CGFloat, card: NSView) -> CGFloat {
        let padding = tokens.number("s-5")
        let editor = Surface(frame: NSRect(x: inset, y: y, width: contentWidth, height: 200))
        editor.wantsLayer = true; editor.layer?.cornerRadius = tokens.number("r-lg")
        editor.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        editor.layer?.borderWidth = 1; editor.layer?.borderColor = tokens.color("border").cgColor
        card.addSubview(editor)
        let title = PreferencesPolicy.text("custom_theme.title")
        let detail = PreferencesPolicy.text("custom_theme.description")
        let reset = PreferencesPolicy.text("custom_theme.reset")
        let resetWidth = buttonWidth(reset)
        let copyWidth = min(44 * tokens.number("text-sm") * 0.56, editor.bounds.width - padding * 3 - resetWidth)
        addCopy(title, detail: detail, x: padding, y: padding, width: copyWidth, parent: editor)
        let resetButton = CaptureButton(reset, frame: NSRect(x: editor.bounds.width - padding - resetWidth,
            y: padding, width: resetWidth, height: tokens.number("h-sm")), tokens: tokens) { [weak self] in
            self?.settings["custom_theme"] = ["accent": "#32d3ff", "signal": "#ff4fc3"]
            self?.changed(rerender: true)
        }
        editor.addSubview(resetButton)
        var top = padding + copyHeight(title, detail: detail, width: copyWidth) + padding
        let gap = tokens.number("s-4")
        let fields = PreferencesPolicy.customThemeFields
        let fieldWidth = (editor.bounds.width - padding * 2 - gap) / CGFloat(max(1, fields.count))
        let custom = settings["custom_theme"] as? [String: Any] ?? [:]
        var fieldHeight: CGFloat = 0
        for (index, field) in fields.enumerated() {
            let box = Surface(frame: NSRect(x: padding + CGFloat(index) * (fieldWidth + gap), y: top,
                width: fieldWidth, height: 100))
            box.wantsLayer = true; box.layer?.cornerRadius = tokens.number("r-md")
            box.layer?.backgroundColor = tokens.color("surface-raised").cgColor
            box.layer?.borderWidth = 1; box.layer?.borderColor = tokens.color("border-subtle").cgColor
            editor.addSubview(box)
            let pad = tokens.number("s-4")
            addLabel(field.label, frame: NSRect(x: pad, y: pad, width: fieldWidth - pad * 2, height: 16),
                size: tokens.number("text-sm"), weight: .medium, parent: box)
            let fallback = field.key == "accent" ? "#32d3ff" : "#ff4fc3"
            colorField(field.label, value: custom.string(field.key, fallback), key: field.key,
                frame: NSRect(x: pad, y: pad + 16 + tokens.number("s-3"), width: fieldWidth - pad * 2,
                    height: tokens.number("h-md")), parent: box)
            let detailTop = pad + 16 + tokens.number("s-3") * 2 + tokens.number("h-md")
            let detailHeight = textHeight(field.detail, size: tokens.number("text-xs"), width: fieldWidth - pad * 2)
            wrappingLabel(field.detail, frame: NSRect(x: pad, y: detailTop, width: fieldWidth - pad * 2,
                height: detailHeight), size: tokens.number("text-xs"), parent: box)
            box.frame.size.height = detailTop + detailHeight + pad
            fieldHeight = max(fieldHeight, box.frame.height)
        }
        top += fieldHeight + padding
        editor.frame.size.height = top
        let text = fields.reduce("\(title) \(detail)") { "\($0) \($1.label) \($1.detail)" }
        searchable.append((findBackground(editor.frame, parent: card), text))
        card.addSubview(editor)
        return y + top
    }

    private func captureCard(_ y: CGFloat) -> CGFloat {
        let (card, top) = makeCard("capture", y: y)
        var y = top
        let folder = PreferencesPolicy.row("output_directory")
        let choose = "Choose…"
        let chooseWidth = buttonWidth(choose)
        let folderHeight = 17 + tokens.number("s-4") + tokens.number("h-md")
        searchable.append((findBackground(NSRect(x: inset, y: y, width: contentWidth, height: folderHeight),
            parent: card), "\(folder.title) folder output directory"))
        addLabel(folder.title, frame: NSRect(x: inset, y: y, width: contentWidth, height: 17),
            size: tokens.number("text-md"), weight: .medium, parent: card)
        let fieldY = y + 17 + tokens.number("s-4")
        let directory = NSTextField(string: settings.string("output_directory"))
        directory.frame = NSRect(x: inset, y: fieldY, width: contentWidth - chooseWidth - tokens.number("s-4"),
            height: tokens.number("h-md"))
        directory.identifier = NSUserInterfaceItemIdentifier("setting.output_directory")
        directory.setAccessibilityLabel(folder.title)
        directory.delegate = self; styleField(directory); card.addSubview(directory)
        _ = actionButton(choose, x: inset + contentWidth - chooseWidth, y: fieldY, width: chooseWidth,
            parent: card) { [weak self] in self?.chooseFolder() }
        y += folderHeight
        for key in ["auto_copy_to_clipboard", "auto_start_on_selection", "show_mini_previews"] {
            y = divider(y, card)
            let copy = PreferencesPolicy.row(key)
            y = switchRow([key], title: copy.title, detail: copy.detail, y: y, card: card)
        }
        y = divider(y, card)
        let previews = settings.bool("show_mini_previews")
        y = cornerPickerRow(y, card: card, enabled: previews)
        y = divider(y, card)
        let includePreviews = settings.bool("include_mini_previews_in_captures")
        let previewsCopy = PreferencesPolicy.row("include_mini_previews_in_captures")
        y = switchRow(["include_mini_previews_in_captures"], title: previewsCopy.title,
            detail: PreferencesPolicy.dynamicDescription("include_mini_previews_in_captures",
                showMiniPreviews: previews, include: includePreviews).text,
            y: y, card: card, enabled: previews)
        y = divider(y, card)
        // AppKit keeps the HUD out of captures (sharingType) unless this is on.
        let controlsCopy = PreferencesPolicy.row("include_recording_controls_in_captures")
        y = switchRow(["include_recording_controls_in_captures"], title: controlsCopy.title, detail: "",
            emphasis: PreferencesPolicy.dynamicDescription("include_recording_controls_in_captures",
                include: settings.bool("include_recording_controls_in_captures"), canExclude: true),
            y: y, card: card)
        for key in ["freeze_screen", "show_cursor_in_screenshots"] {
            y = divider(y, card)
            let copy = PreferencesPolicy.row(key)
            y = switchRow([key], title: copy.title, detail: copy.detail, y: y, card: card)
        }
        y = divider(y, card)
        y = selectRow("screenshot_format", y: y, card: card)
        y = divider(y, card)
        y = selectRow("screenshot_countdown_seconds", y: y, card: card)
        return finish(card, y)
    }

    /// Shipping `MiniPreviewPlacementPicker`: a small screen with four corners.
    private func cornerPickerRow(_ y: CGFloat, card: NSView, enabled: Bool) -> CGFloat {
        let copy = PreferencesPolicy.row("mini_preview_placement")
        let size = NSSize(width: 108, height: 72 + tokens.number("s-2") + 16)
        let layout = inlineRow(copy.title, detail: copy.detail, control: size, y: y, card: card)
        let picker = Surface(frame: layout.control)
        picker.alphaValue = enabled ? 1 : 0.55
        picker.setAccessibilityRole(.radioGroup)
        picker.setAccessibilityLabel(copy.title)
        card.addSubview(picker)
        let screen = Surface(frame: NSRect(x: 0, y: 0, width: 108, height: 72))
        screen.wantsLayer = true; screen.layer?.cornerRadius = tokens.number("r-md")
        screen.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        screen.layer?.borderWidth = 1; screen.layer?.borderColor = tokens.color("border-subtle").cgColor
        picker.addSubview(screen)
        let placements = PreferencesPolicy.pairs("mini_preview_placements")
        let selected = settings.string("mini_preview_placement", "bottom_left")
        for placement in placements {
            let x: CGFloat = placement.value.hasSuffix("left") ? 5 : 108 - 5 - 26
            let top: CGFloat = placement.value.hasPrefix("top") ? 5 : 72 - 5 - 26
            let value = placement.value
            let corner = PreferenceCornerButton(frame: NSRect(x: x, y: top, width: 26, height: 26),
                tokens: tokens, label: placement.label) { [weak self] in
                self?.set(value, for: "mini_preview_placement", rerender: true)
            }
            corner.active = selected == value
            corner.isEnabled = enabled
            corner.identifier = NSUserInterfaceItemIdentifier("mini-preview-placement.\(value)")
            screen.addSubview(corner)
        }
        let name = placements.first(where: { $0.value == selected })?.label
            ?? placements.first(where: { $0.value == "bottom_left" })?.label ?? ""
        let label = addLabel(name, frame: NSRect(x: 0, y: 72 + tokens.number("s-2"), width: 108, height: 16),
            size: tokens.number("text-sm"), muted: true, parent: picker)
        label.alignment = .right
        return layout.row.maxY
    }

    private func shortcutsCard(_ y: CGFloat) -> CGFloat {
        let section = PreferencesPolicy.sections.first(where: { $0.id == "shortcuts" })?.detail ?? ""
        var detail = "\(section) \(PreferencesPolicy.text("shortcuts.intro"))"
        if !liveCaptureAvailable { detail += " \(PreferencesPolicy.text("shortcuts.fixture_note"))" }
        let (card, top) = makeCard("shortcuts", y: y, detail: detail)
        var y = top
        shortcutRecorders.removeAll(); shortcutErrors.removeAll()
        y = utilityRow(PreferencesPolicy.text("shortcuts.system_title"),
            detail: PreferencesPolicy.text("shortcuts.system_body"),
            action: PreferencesPolicy.text("shortcuts.system_action"), y: y, card: card,
            enabled: liveCaptureAvailable, identifier: "shortcuts.system-settings") { [weak self] in
            self?.openKeyboardSettings()
        }
        if let keyboardSettingsError {
            let height = textHeight(keyboardSettingsError, size: tokens.number("text-xs"), width: contentWidth)
            wrappingLabel(keyboardSettingsError, frame: NSRect(x: inset, y: y + tokens.number("s-2"),
                width: contentWidth, height: height), size: tokens.number("text-xs"), parent: card,
                color: "danger-text")
            y += tokens.number("s-2") + height
        }
        y = divider(y, card)
        let rows = (PreferencesPolicy.copy["shortcuts"] as? [String: Any])?["rows"] as? [[String: Any]] ?? []
        for (index, row) in rows.enumerated() {
            guard let path = row["path"] as? [String], let key = path.last else { continue }
            if index > 0 { y += tokens.number("s-2") }
            y = shortcutRow(key: key, title: row.string("label"), recordingSetting: path.count == 2,
                y: y, parent: card)
        }
        return finish(card, y)
    }

    private func openKeyboardSettings() {
        let opened = PreferencesPolicy.systemShortcutTargets.contains { target in
            guard let url = URL(string: target) else { return false }
            return NSWorkspace.shared.open(url)
        }
        keyboardSettingsError = opened ? nil : "Couldn’t open Keyboard settings."
        rebuildCards()
    }

    private func shortcutRow(key: String, title: String, recordingSetting: Bool, y: CGFloat,
                             parent: NSView) -> CGFloat {
        let identifier = recordingSetting ? "recording.\(key)" : key
        let shortcut = recordingSetting
            ? (settings["recording"] as? [String: Any] ?? [:]).string(key)
            : settings.string(key)
        let width = min(260, max(180, contentWidth * 0.45))
        let layout = inlineRow(title, detail: "", control: NSSize(width: width, height: tokens.number("h-md")),
            y: y, card: parent)
        let keys = (try? shortcutDisplay(shortcut)) ?? [shortcut]
        let recorder = ShortcutRecorderButton(frame: layout.control,
            tokens: tokens, label: title, keys: keys.filter { !$0.isEmpty })
        recorder.identifier = NSUserInterfaceItemIdentifier("shortcut.\(identifier)")
        recorder.beginRecording = { [weak self, weak recorder] in
            guard let self, let recorder else { return }
            self.beginShortcutRecording(recorder)
        }
        recorder.recordEvent = { [weak self] event in self?.recordShortcut(event) }
        recorder.cancelRecording = { [weak self, weak recorder] in
            guard let self, let recorder, self.recordingShortcut === recorder else { return }
            self.stopShortcutRecording(recorder, identifier: identifier)
        }
        parent.addSubview(recorder); shortcutRecorders[identifier] = recorder
        let error = addLabel("", frame: NSRect(x: layout.control.minX, y: layout.control.maxY + tokens.number("s-2"),
            width: width, height: 14), size: tokens.number("text-xs"), parent: parent, color: "danger-text")
        error.identifier = NSUserInterfaceItemIdentifier("shortcut-error.\(identifier)")
        error.setAccessibilityRole(.staticText); error.setAccessibilityLabel("\(title) shortcut error")
        error.isHidden = true
        shortcutErrors[identifier] = error
        return layout.row.maxY
    }

    /// Shows or clears a recorder's inline error, moving the rows and cards
    /// below it like the shipping `.shortcut-error` line.
    private func setShortcutError(_ message: String, identifier: String) {
        guard let label = shortcutErrors[identifier], let card = sectionViews["shortcuts"] else { return }
        let wasShown = !label.isHidden
        label.stringValue = message
        let shown = !message.isEmpty
        label.isHidden = !shown
        guard shown != wasShown else { return }
        let delta = shown ? Self.shortcutErrorHeight : -Self.shortcutErrorHeight
        let threshold = label.frame.minY - tokens.number("s-3") - 1
        for view in card.subviews where view !== label && view.frame.minY >= threshold {
            view.frame.origin.y += delta
        }
        card.frame.size.height += delta
        for view in document.subviews where view !== card && view.frame.minY > card.frame.minY {
            view.frame.origin.y += delta
        }
        document.frame.size.height += delta
        updateActiveSection()
    }

    private func beginShortcutRecording(_ recorder: ShortcutRecorderButton) {
        if let current = recordingShortcut, current !== recorder {
            stopShortcutRecording(current, identifier: shortcutIdentifier(current))
        }
        recordingShortcut = recorder
        recorder.startRecording()
        setShortcutError("", identifier: shortcutIdentifier(recorder))
        root.window?.makeFirstResponder(recorder)
        if shortcutEventMonitor == nil {
            shortcutEventMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .flagsChanged]) {
                [weak self] event in
                guard let self, self.recordingShortcut != nil else { return event }
                self.recordShortcut(event)
                return nil
            }
        }
        if shortcutFocusObserver == nil, let window = root.window {
            shortcutFocusObserver = NotificationCenter.default.addObserver(
                forName: NSWindow.didResignKeyNotification, object: window, queue: .main
            ) { [weak self, weak recorder] _ in
                guard let self, let recorder, self.recordingShortcut === recorder else { return }
                self.stopShortcutRecording(recorder, identifier: self.shortcutIdentifier(recorder))
            }
        }
    }

    private func recordShortcut(_ event: NSEvent) {
        handleShortcutInput(code: Self.domCode(for: event),
            control: event.modifierFlags.contains(.control),
            shift: event.modifierFlags.contains(.shift),
            alt: event.modifierFlags.contains(.option),
            meta: event.modifierFlags.contains(.command))
    }

    func handleShortcutInput(code: String, control: Bool, shift: Bool, alt: Bool, meta: Bool) {
        guard let recorder = recordingShortcut else { return }
        let identifier = shortcutIdentifier(recorder)
        do {
            let result = try shortcutPolicy(code, control, shift, alt, meta)
            guard let kind = result["kind"] as? String else { throw AppBridgeError.invalidResponse }
            switch kind {
            case "cancel":
                stopShortcutRecording(recorder, identifier: identifier)
            case "waiting":
                guard let keys = result["keys"] as? [String] else {
                    throw AppBridgeError.invalidResponse
                }
                recorder.update(keys: keys)
                setShortcutError("", identifier: identifier)
            case "invalid":
                guard let keys = result["keys"] as? [String] else {
                    throw AppBridgeError.invalidResponse
                }
                let message = result["message"] as? String ?? "That shortcut is not supported."
                recorder.update(keys: keys, error: message)
                setShortcutError(message, identifier: identifier)
            case "complete":
                guard let keys = result["keys"] as? [String],
                      let shortcut = result["shortcut"] as? String else {
                    throw AppBridgeError.invalidResponse
                }
                setShortcut(shortcut, identifier: identifier)
                finishShortcutRecording(recorder, keys: keys)
            default: throw AppBridgeError.invalidResponse
            }
        } catch {
            let message = error.localizedDescription
            recorder.update(keys: recorder.keys, error: message)
            setShortcutError(message, identifier: identifier)
        }
    }

    private func stopShortcutRecording(_ recorder: ShortcutRecorderButton, identifier: String) {
        let shortcut = shortcutValue(identifier: identifier)
        finishShortcutRecording(recorder,
            keys: (try? shortcutDisplay(shortcut)) ?? [shortcut].filter { !$0.isEmpty })
    }

    private func finishShortcutRecording(_ recorder: ShortcutRecorderButton, keys: [String]) {
        recorder.stopRecording(keys: keys)
        setShortcutError("", identifier: shortcutIdentifier(recorder))
        if recordingShortcut === recorder { recordingShortcut = nil }
        if let monitor = shortcutEventMonitor {
            NSEvent.removeMonitor(monitor); shortcutEventMonitor = nil
        }
        if let observer = shortcutFocusObserver {
            NotificationCenter.default.removeObserver(observer); shortcutFocusObserver = nil
        }
    }

    private func setShortcut(_ shortcut: String, identifier: String) {
        if identifier.hasPrefix("recording.") {
            let key = String(identifier.dropFirst("recording.".count))
            var recording = settings["recording"] as? [String: Any] ?? [:]
            recording[key] = shortcut; settings["recording"] = recording
        } else {
            settings[identifier] = shortcut
        }
        changed(rerender: false)
    }

    private func shortcutValue(identifier: String) -> String {
        if identifier.hasPrefix("recording.") {
            return (settings["recording"] as? [String: Any] ?? [:])
                .string(String(identifier.dropFirst("recording.".count)))
        }
        return settings.string(identifier)
    }

    private func shortcutIdentifier(_ recorder: ShortcutRecorderButton) -> String {
        guard let value = recorder.identifier?.rawValue else { return "" }
        return String(value.dropFirst("shortcut.".count))
    }

    func shortcutRecorder(identifier: String) -> ShortcutRecorderButton? {
        shortcutRecorders[identifier]
    }

    func shortcutCard() -> NSView? { sectionViews["shortcuts"] }

    deinit {
        if let monitor = shortcutEventMonitor { NSEvent.removeMonitor(monitor) }
        if let observer = shortcutFocusObserver { NotificationCenter.default.removeObserver(observer) }
        if let scrollObserver { NotificationCenter.default.removeObserver(scrollObserver) }
    }

    static func domCode(for event: NSEvent) -> String {
        domCode(forKeyCode: event.keyCode)
    }

    static func domCode(forKeyCode keyCode: UInt16) -> String {
        // Physical key codes follow Carbon Events.h and winit-appkit's inverse table.
        let codes: [UInt16: String] = [
            0: "KeyA", 1: "KeyS", 2: "KeyD", 3: "KeyF", 4: "KeyH", 5: "KeyG",
            6: "KeyZ", 7: "KeyX", 8: "KeyC", 9: "KeyV", 11: "KeyB", 12: "KeyQ",
            13: "KeyW", 14: "KeyE", 15: "KeyR", 16: "KeyY", 17: "KeyT",
            18: "Digit1", 19: "Digit2", 20: "Digit3", 21: "Digit4", 22: "Digit6",
            23: "Digit5", 24: "Equal", 25: "Digit9", 26: "Digit7", 27: "Minus",
            28: "Digit8", 29: "Digit0", 30: "BracketRight", 31: "KeyO", 32: "KeyU",
            33: "BracketLeft", 34: "KeyI", 35: "KeyP", 36: "Enter", 37: "KeyL",
            38: "KeyJ", 39: "Quote", 40: "KeyK", 41: "Semicolon", 42: "Backslash",
            43: "Comma", 44: "Slash", 45: "KeyN", 46: "KeyM", 47: "Period",
            48: "Tab", 49: "Space", 50: "Backquote", 51: "Backspace", 53: "Escape",
            54: "MetaRight", 55: "MetaLeft", 56: "ShiftLeft", 57: "CapsLock",
            58: "AltLeft", 59: "ControlLeft", 60: "ShiftRight", 61: "AltRight",
            62: "ControlRight", 64: "F17", 65: "NumpadDecimal", 67: "NumpadMultiply",
            69: "NumpadAdd", 71: "NumLock", 75: "NumpadDivide", 76: "NumpadEnter",
            78: "NumpadSubtract", 79: "F18", 80: "F19", 81: "NumpadEqual",
            82: "Numpad0", 83: "Numpad1",
            84: "Numpad2", 85: "Numpad3", 86: "Numpad4", 87: "Numpad5",
            88: "Numpad6", 89: "Numpad7", 90: "F20", 91: "Numpad8", 92: "Numpad9",
            96: "F5", 97: "F6", 98: "F7", 99: "F3", 100: "F8", 101: "F9",
            103: "F11", 105: "F13", 106: "F16", 107: "F14", 109: "F10",
            111: "F12", 113: "F15", 114: "Insert", 115: "Home", 116: "PageUp",
            117: "Delete", 118: "F4", 119: "End", 120: "F2", 121: "PageDown",
            122: "F1", 123: "ArrowLeft", 124: "ArrowRight", 125: "ArrowDown",
            126: "ArrowUp",
        ]
        return codes[keyCode] ?? "Unidentified"
    }

    private func recordingCard(_ y: CGFloat) -> CGFloat {
        let (card, top) = makeCard("recording", y: y)
        var y = selectRow("recording.video_format", y: top, card: card)
        y = divider(y, card)
        y = selectGrid(["recording.video_fps", "recording.video_max_resolution"], y: y, card: card)
        y = divider(y, card)
        y = selectRow("recording.countdown_seconds", y: y, card: card)
        y = divider(y, card)
        y = microphoneRow(y, card: card)
        for key in PreferencesPolicy.recordingToggles {
            y = divider(y, card)
            let copy = PreferencesPolicy.row("recording.\(key)")
            y = switchRow(["recording", key], title: copy.title, detail: copy.detail, y: y, card: card)
        }
        return finish(card, y)
    }

    private func gifCard(_ y: CGFloat) -> CGFloat {
        let (card, top) = makeCard("gif", y: y)
        let bottom = selectGrid(["recording.gif_fps", "recording.gif_max_width", "recording.gif_max_colors"],
            y: top, card: card)
        return finish(card, bottom)
    }

    /// Shipping Default microphone select: Off, then each input device. A saved
    /// device that is not connected stays selectable by its id.
    private func microphoneRow(_ y: CGFloat, card: NSView) -> CGFloat {
        let copy = PreferencesPolicy.row("recording.microphone_device_id")
        let layout = inlineRow(copy.title, detail: copy.detail,
            control: NSSize(width: 160, height: tokens.number("h-md")), y: y, card: card)
        let menu = ClosurePopUpButton(frame: layout.control, pullsDown: false)
        menu.tokens = tokens
        menu.setAccessibilityLabel(copy.accessibilityLabel)
        menu.target = menu; menu.action = #selector(ClosurePopUpButton.selectedValue); card.addSubview(menu)
        microphonePopUp = menu
        fillMicrophoneMenu(menu)
        loadMicrophonesIfNeeded()
        return layout.row.maxY
    }

    private func fillMicrophoneMenu(_ menu: ClosurePopUpButton) {
        let saved = (settings["recording"] as? [String: Any])?["microphone_device_id"] as? String
        var options: [(id: String?, title: String)] = [(nil, PreferencesPolicy.text("microphone_off"))]
        options += (microphones ?? []).map { (id: Optional($0.id), title: $0.name) }
        if let saved, !options.contains(where: { $0.id == saved }) { options.append((id: saved, title: saved)) }
        menu.menu?.removeAllItems()
        // Add items individually: devices can share a display name.
        for option in options { menu.menu?.addItem(NSMenuItem(title: option.title, action: nil, keyEquivalent: "")) }
        menu.selectItem(at: options.firstIndex { $0.id == saved } ?? 0)
        menu.change = { [weak self] index in
            guard let self, options.indices.contains(index) else { return }
            var recording = self.settings["recording"] as? [String: Any] ?? [:]
            if let id = options[index].id { recording["microphone_device_id"] = id }
            else { recording["microphone_device_id"] = NSNull() }
            self.settings["recording"] = recording; self.changed(rerender: false)
        }
        menu.needsDisplay = true
    }

    private func loadMicrophonesIfNeeded() {
        guard microphones == nil, !microphonesLoading else { return }
        microphonesLoading = true
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let devices = (try? NativeRecordingInfo.microphoneDevices()) ?? []
            DispatchQueue.main.async {
                guard let self else { return }
                self.microphones = devices; self.microphonesLoading = false
                if let menu = self.microphonePopUp { self.fillMicrophoneMenu(menu) }
            }
        }
    }

    private func updatesCard(_ y: CGFloat) -> CGFloat {
        let (card, top) = makeCard("updates", y: y)
        var y = utilityRow(PreferencesPolicy.text("updates.title"), detail: PreferencesPolicy.text("updates.detail"),
            action: PreferencesPolicy.text("updates.action"), y: top, card: card, enabled: false,
            identifier: "updates.check") {}
        y = divider(y, card)
        let copy = PreferencesPolicy.row("show_update_changelog")
        y = switchRow(["show_update_changelog"], title: copy.title, detail: copy.detail, y: y, card: card)
        return finish(card, y)
    }

    private func aboutCard(_ y: CGFloat) -> CGFloat {
        let (card, top) = makeCard("about", y: y)
        var y = utilityRow(PreferencesPolicy.text("feedback.title"), detail: PreferencesPolicy.text("feedback.detail"),
            action: PreferencesPolicy.text("feedback.action"), y: top, card: card,
            identifier: "about.feedback") { [weak self] in self?.showFeedback() }
        y = divider(y, card)
        let title = PreferencesPolicy.text("login_item.title")
        guard loginItemService != nil else {
            // Fixtures and exercise runs cannot register a login item.
            let layout = inlineRow(title, detail: PreferencesPolicy.text("login_item.unavailable"),
                control: NSSize(width: 32, height: 19), y: y, card: card, enabled: false)
            let button = PreferenceSwitchButton(frame: layout.row, tokens: tokens, label: title, isOn: false) {}
            button.identifier = NSUserInterfaceItemIdentifier("login-item-unavailable")
            button.isEnabled = false
            card.addSubview(button)
            return finish(card, layout.row.maxY)
        }
        if let loginItemError {
            y = utilityRow(title, detail: PreferencesPolicy.template("login_item.error_template", error: loginItemError),
                action: PreferencesPolicy.text("login_item.retry"), y: y, card: card, enabled: !loginItemPending,
                identifier: "login-item") { [weak self] in self?.toggleLoginItem() }
            return finish(card, y)
        }
        let detail = loginItemPending ? PreferencesPolicy.text("login_item.checking")
            : PreferencesPolicy.text("login_item.detail")
        let layout = inlineRow(title, detail: detail, control: NSSize(width: 32, height: 19), y: y, card: card)
        let button = PreferenceSwitchButton(frame: layout.row, tokens: tokens, label: title,
            isOn: loginItemEnabled == true) { [weak self] in self?.toggleLoginItem() }
        button.title = loginItemPending ? PreferencesPolicy.text("login_item.checking")
            : loginItemEnabled == true ? "On" : "Off"
        button.identifier = NSUserInterfaceItemIdentifier("login-item")
        button.isEnabled = !loginItemPending
        card.addSubview(button)
        return finish(card, layout.row.maxY)
    }

    private func queryLoginItem() { requestLoginItem(nil) }

    private func toggleLoginItem() {
        guard !loginItemPending else { return }
        requestLoginItem(loginItemError != nil ? nil : !(loginItemEnabled ?? false))
    }

    private func requestLoginItem(_ enabled: Bool?) {
        guard let loginItemService else { return }
        loginItemGeneration += 1
        let generation = loginItemGeneration
        loginItemPending = true; loginItemError = nil; rebuildCards()
        loginItemService.setEnabled(enabled) { [weak self] result in
            guard let self, generation == self.loginItemGeneration else { return }
            self.loginItemPending = false
            switch result {
            case .success(let actual): self.loginItemEnabled = actual; self.loginItemError = nil
            case .failure(let error): self.loginItemError = error.localizedDescription
            }
            self.rebuildCards()
        }
    }

    private func colorField(_ label: String, value: String, key: String, frame: NSRect, parent: NSView) {
        let well = ClosureColorWell(frame: NSRect(x: frame.minX, y: frame.minY, width: 36, height: frame.height))
        well.color = NSColor(hex: value) ?? .white
        well.setAccessibilityLabel("\(label) color picker")
        let field = NSTextField(string: value.uppercased())
        field.frame = NSRect(x: frame.minX + 36 + tokens.number("s-3"), y: frame.minY,
            width: frame.width - 36 - tokens.number("s-3"), height: frame.height)
        field.identifier = NSUserInterfaceItemIdentifier("custom.\(key)")
        field.setAccessibilityLabel("\(label) hex value")
        styleField(field); field.font = .monospacedSystemFont(ofSize: tokens.number("text-xs"), weight: .regular)
        field.delegate = self; parent.addSubview(field)
        well.change = { [weak self, weak field] color in
            guard let self, let field, let normalized = color.rgbHex else { return }
            field.stringValue = normalized; var custom = self.settings["custom_theme"] as? [String: Any] ?? [:]
            custom[key] = normalized; self.settings["custom_theme"] = custom; self.changed(rerender: true)
        }
        well.target = well; well.action = #selector(ClosureColorWell.selectedColor); parent.addSubview(well)
    }

    func controlTextDidEndEditing(_ obj: Notification) {
        guard !rebuilding else { return }
        guard let field = obj.object as? NSTextField else { return }
        if field.identifier?.rawValue.hasPrefix("custom.") == true {
            var custom = settings["custom_theme"] as? [String: Any] ?? [:]
            guard let normalized = Self.normalizeHex(field.stringValue) else { field.stringValue = custom.string(String(field.identifier!.rawValue.dropFirst(7))); return }
            field.stringValue = normalized; custom[String(field.identifier!.rawValue.dropFirst(7))] = normalized
            settings["custom_theme"] = custom; changed(rerender: true)
        } else if field.identifier?.rawValue == "setting.output_directory" {
            set(field.stringValue, for: "output_directory", rerender: false)
        }
    }

    static func normalizeHex(_ input: String) -> String? {
        var value = input.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        guard value.hasPrefix("#") else { return nil }
        if value.count == 4 { value = "#" + value.dropFirst().map { "\($0)\($0)" }.joined() }
        guard value.utf8.count == 7, value.dropFirst().allSatisfy({ $0.isASCII && $0.isHexDigit }) else { return nil }
        return value
    }

    private func chooseFolder() {
        let picker = NSOpenPanel(); picker.canChooseDirectories = true; picker.canChooseFiles = false
        picker.allowsMultipleSelection = false; picker.prompt = "Choose"; picker.message = "Choose capture folder"
        picker.beginSheetModal(for: root.window!) { [weak self] response in
            if response == .OK, let path = picker.url?.path { self?.set(path, for: "output_directory", rerender: true) }
        }
    }

    private func set(_ value: Any, for key: String, rerender: Bool) { settings[key] = value; changed(rerender: rerender) }
    private func changed(rerender: Bool) {
        setStatus(PreferencesPolicy.text("saving"), kind: "saving"); saveFailed = false
        settingsChanged(settings)
        latestRevision = store.save(settings) { [weak self] revision, result in
            guard let self, revision == self.latestRevision else { return }
            switch result {
            case .success(let saved):
                self.settings = saved; self.settingsChanged(saved)
                self.settingsPersisted(saved)
                self.setStatus(PreferencesPolicy.text("saved"), kind: "saved")
            case .failure(let error):
                self.saveFailed = true
                self.setStatus(PreferencesPolicy.template("save_error_template", error: error.localizedDescription),
                    kind: "error")
            }
        }
        if rerender {
            appearanceChanged(settings.string("appearance", "system"), settings.string("theme", "mustard"), settings["custom_theme"] as? [String: Any] ?? [:]); restyle()
        }
    }

    /// Shipping `.preferences-save-status`: a pill that is hidden when idle
    /// and clears "Changes saved" after two seconds.
    private func setStatus(_ text: String, kind: String) {
        statusKind = text.isEmpty ? "idle" : kind
        statusGeneration += 1
        let visible = statusKind != "idle"
        statusPill.isHidden = !visible
        status.stringValue = text
        status.setAccessibilityLabel(text)
        statusMark?.kind = statusKind
        let (fill, border, color): (String, String?, String) = statusKind == "saved"
            ? ("positive-surface", nil, "positive-text")
            : statusKind == "error" ? ("danger-surface", nil, "danger-text") : ("surface-raised", "border", "text-muted")
        statusPill.layer?.backgroundColor = tokens.color(fill).cgColor
        statusPill.layer?.borderWidth = border == nil ? 0 : 1
        statusPill.layer?.borderColor = tokens.color(border ?? fill).cgColor
        status.textColor = tokens.color(color)
        let textWidth = min(300, ceil((text as NSString).size(withAttributes: [.font: status.font ?? NSFont.systemFont(ofSize: 11)]).width) + 4)
        status.frame.size.width = textWidth
        statusPill.frame.size.width = status.frame.minX + textWidth + tokens.number("s-4")
        retryButton?.isHidden = statusKind != "error" || !saveFailed
        layoutHeaderActions()
        if statusKind == "saved" {
            let generation = statusGeneration
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in
                guard let self, self.statusGeneration == generation else { return }
                self.setStatus("", kind: "idle")
            }
        }
    }

    func retrySave() { if saveFailed { changed(rerender: false) } }
    func flush() {
        if let field = root.window?.firstResponder as? NSTextView,
           let editor = field.delegate as? NSTextField {
            controlTextDidEndEditing(Notification(name: NSControl.textDidEndEditingNotification, object: editor))
        }
        store.flush()
    }
    private func reveal(_ id: String) {
        guard let view = sectionViews[id] else { return }
        // Shipping `scroll-margin-top: var(--s-6)`.
        let maxY = max(0, document.frame.height - scroll.contentSize.height)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(maxY, max(0, view.frame.minY - tokens.number("s-6")))))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    // MARK: Find

    /// Shipping `.preferences-find` inside the header: field, count,
    /// previous/next and close.
    func showFind() {
        if findBar != nil { windowFocusFind(); return }
        let left = Self.sidebarWidth + tokens.number("s-8")
        let side = tokens.number("h-md")
        let bar = Surface(frame: NSRect(x: left, y: Self.headerHeight, width: root.bounds.width - left - tokens.number("s-8"),
            height: side))
        bar.autoresizingMask = [.width]
        let gap = tokens.number("s-3")
        let countWidth: CGFloat = 5.5 * tokens.number("text-md")
        let fieldWidth = bar.bounds.width - countWidth - side * 3 - gap * 4
        let field = NSTextField(string: ""); field.placeholderString = PreferencesPolicy.text("find.placeholder")
        field.frame = NSRect(x: 0, y: 0, width: fieldWidth, height: side)
        field.autoresizingMask = [.width]; field.identifier = NSUserInterfaceItemIdentifier("find")
        field.setAccessibilityLabel(PreferencesPolicy.text("find.placeholder"))
        styleField(field); field.font = .systemFont(ofSize: tokens.number("text-md"))
        field.delegate = self; bar.addSubview(field)
        let count = addLabel("", frame: NSRect(x: fieldWidth + gap, y: (side - 16) / 2, width: countWidth, height: 16),
            size: tokens.number("text-sm"), muted: true, parent: bar)
        count.autoresizingMask = [.minXMargin]; findCount = count
        var x = fieldWidth + gap + countWidth + gap
        for (icon, label, delta) in [("chevron-up", "find.previous", -1), ("chevron-down", "find.next", 1),
                                     ("close", "find.close", 0)] {
            let button = actionButton("", x: x, y: 0, width: side, parent: bar) { [weak self] in
                if delta == 0 { self?.closeFind() } else { self?.stepFind(delta) }
            }
            button.icon = .shipping(icon); button.autoresizingMask = [.minXMargin]
            button.setAccessibilityLabel(PreferencesPolicy.text(label)); button.toolTip = PreferencesPolicy.text(label)
            if delta < 0 { findPrevious = button } else if delta > 0 { findNext = button }
            x += side + gap
        }
        root.addSubview(bar); findBar = bar; findField = field
        setHeader(height: Self.headerHeight + Self.findHeight)
        updateFind()
        windowFocusFind()
    }

    private func setHeader(height: CGFloat) {
        headerRule.frame.origin.y = height
        scroll.frame = NSRect(x: Self.sidebarWidth, y: height + 1, width: root.bounds.width - Self.sidebarWidth,
            height: root.bounds.height - height - 1)
    }

    private func windowFocusFind() { root.window?.makeFirstResponder(findField) }
    func controlTextDidChange(_ obj: Notification) {
        if (obj.object as? NSTextField)?.identifier?.rawValue == "find" { matchIndex = 0; updateFind(scroll: true) }
    }

    private func updateFind(scroll reveal: Bool = false) {
        guard findBar != nil else {
            matches = []
            for entry in searchable { entry.0.isHidden = true }
            return
        }
        let query = findField?.stringValue ?? ""
        let result = PreferencesPolicy.find(query: query, texts: searchable.map(\.1), index: matchIndex)
        matches = findBar == nil ? [] : result.matches
        matchIndex = min(matchIndex, max(0, matches.count - 1))
        findCount?.stringValue = findBar == nil ? "" : result.label
        findPrevious?.isEnabled = !matches.isEmpty; findNext?.isEnabled = !matches.isEmpty
        highlightMatches(scroll: reveal)
    }

    func stepFind(_ delta: Int) {
        guard !matches.isEmpty else { return }
        matchIndex = (matchIndex + delta + matches.count) % matches.count
        updateFind(scroll: true)
    }

    /// Shipping `.preference-find-match` wash and `.preference-find-current` ring.
    private func highlightMatches(scroll reveal: Bool) {
        for (index, entry) in searchable.enumerated() {
            let wash = entry.0
            guard let position = matches.firstIndex(of: index) else { wash.isHidden = true; continue }
            wash.isHidden = false
            wash.layer?.backgroundColor = tokens.color("surface-selected").cgColor
            let current = position == matchIndex
            wash.layer?.borderWidth = current ? 2 : 0
            wash.layer?.borderColor = tokens.color("theme-accent").withAlphaComponent(0.62).cgColor
        }
        guard reveal, matches.indices.contains(matchIndex), searchable.indices.contains(matches[matchIndex]) else { return }
        let view = searchable[matches[matchIndex]].0
        guard let parent = view.superview else { return }
        let target = parent.convert(view.frame, to: document)
        let visible = scroll.contentSize.height
        let maxY = max(0, document.frame.height - visible)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(maxY, max(0, target.midY - visible / 2))))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    func closeFind() {
        findBar?.removeFromSuperview(); findBar = nil; findField = nil; findCount = nil
        findPrevious = nil; findNext = nil; matches = []
        setHeader(height: Self.headerHeight)
        updateFind()
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        guard control.identifier?.rawValue == "find" else { return false }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)), findBar != nil { closeFind(); return true }
        if commandSelector == #selector(NSResponder.insertNewline(_:)) {
            stepFind(NSEvent.modifierFlags.contains(.shift) ? -1 : 1); return true
        }
        return false
    }

    func exerciseAppearance(_ value: String) { set(value, for: "appearance", rerender: true) }

    func restyle() {
        guard !rebuilding else { return }
        rebuilding = true
        defer { rebuilding = false }
        let oldY = scroll.contentView.bounds.origin.y
        let query = findField?.stringValue
        let focused = (root.window?.firstResponder as? NSView)?.accessibilityLabel()
        let text = status.stringValue, kind = statusKind
        root.subviews.forEach { $0.removeFromSuperview() }
        findBar = nil; findField = nil; findCount = nil; findPrevious = nil; findNext = nil
        root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        buildShell()
        rebuildCards()
        scroll.contentView.scroll(to: NSPoint(x: 0, y: oldY))
        revealHighlightIfNeeded()
        if let query { showFind(); findField?.stringValue = query; updateFind() }
        else if let focused {
            func restore(_ view: NSView) -> NSView? {
                if view.accessibilityLabel() == focused, view.acceptsFirstResponder { return view }
                for child in view.subviews { if let found = restore(child) { return found } }
                return nil
            }
            if let view = restore(root) { root.window?.makeFirstResponder(view) }
        }
        setStatus(text, kind: kind == "error" && !saveFailed ? "idle" : kind)
    }

    @discardableResult private func addLabel(_ text: String, frame: NSRect, size: CGFloat, weight: NSFont.Weight = .regular,
        muted: Bool = false, parent: NSView, centered: Bool = false, color: String? = nil) -> NSTextField {
        let label = NSTextField(labelWithString: text); label.frame = frame; label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = tokens.color(color ?? (muted ? "text-subtle" : "text")); label.alignment = centered ? .center : .left
        parent.addSubview(label); return label
    }

    /// A wrapping `--text-subtle` description label.
    @discardableResult private func wrappingLabel(_ text: String, frame: NSRect, size: CGFloat, parent: NSView,
                                                  color: String = "text-subtle") -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.frame = frame; label.font = .systemFont(ofSize: size)
        label.textColor = tokens.color(color); label.preferredMaxLayoutWidth = frame.width
        label.maximumNumberOfLines = 0
        parent.addSubview(label); return label
    }

    private func actionButton(_ title: String, x: CGFloat, y: CGFloat, width: CGFloat, parent: NSView, action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: NSRect(x: x, y: y, width: width, height: tokens.number("h-md")), tokens: tokens, action: action); parent.addSubview(button); return button
    }
    private func styleField(_ field: NSTextField) {
        field.isBordered = false; field.drawsBackground = true; field.backgroundColor = tokens.color("surface-field")
        field.textColor = tokens.color("text"); field.font = .monospacedSystemFont(ofSize: tokens.number("text-sm"), weight: .regular)
        field.wantsLayer = true; field.layer?.cornerRadius = tokens.number("r-md"); field.layer?.borderWidth = 1; field.layer?.borderColor = tokens.color("control-border").cgColor
    }
}

/// Shipping `.preferences-nav-brand span`: the capture mark on an accent tile.
final class BrandMarkView: NSView {
    private let tokens: Tokens
    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: frame)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        let radius = tokens.number("r-md")
        tokens.color("theme-accent").setFill()
        NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
        tokens.color("theme-accent-ink").setStroke()
        ShippingIcons.stroke("capture", in: NSRect(x: (bounds.width - 16) / 2, y: (bounds.height - 16) / 2,
            width: 16, height: 16))
    }
}
