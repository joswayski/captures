import AppKit
import QuartzCore
import CCapturesSettings

enum Metrics {
    static func emit(_ event: String, milliseconds: Double, detail: String = "") {
        write(["event": event, "milliseconds": milliseconds, "detail": detail])
    }
    static func write(_ fields: [String: Any]) {
        var row = fields
        row["schema"] = 1
        row["pid"] = ProcessInfo.processInfo.processIdentifier
        row["uptime"] = ProcessInfo.processInfo.systemUptime
        if let data = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]) {
            FileHandle.standardOutput.write(data + Data([10]))
        }
    }
}

final class Surface: NSView {
    override var isFlipped: Bool { true }
}

enum CaptureButtonIcon {
    case capture
    case record
    case window
    case display
    case microphone(muted: Bool)
    case editorSelect, editorCrop, editorText, editorShapes, editorArrow, editorPen, editorBackground
    /// A named icon from the shared shipping set (`captures_icon_polylines_v1`).
    case shipping(String)
    /// The shipping HUD stop control: an 11-point rounded signal square.
    case stopSquare

    var isEditorTool: Bool {
        switch self {
        case .capture, .record, .window, .display, .microphone, .shipping, .stopSquare: return false
        default: return true
        }
    }

    var isShipping: Bool {
        switch self {
        case .shipping, .stopSquare: return true
        default: return false
        }
    }
}

/// Polylines for the shared shipping icon set, in 24-unit y-down space.
enum ShippingIcons {
    private static var cache: [String: [[NSPoint]]] = [:]

    static func polylines(_ name: String) -> [[NSPoint]] {
        if let cached = cache[name] { return cached }
        guard let response = captures_icon_polylines_v1(name) else { return [] }
        defer { captures_settings_free_v1(response) }
        guard let object = try? JSONSerialization.jsonObject(with: Data(String(cString: response).utf8))
                as? [String: Any],
              object["ok"] as? Bool == true,
              let lines = object["result"] as? [[[NSNumber]]] else { return [] }
        let result = lines.map { line in
            line.compactMap { $0.count == 2 ? NSPoint(x: $0[0].doubleValue, y: $0[1].doubleValue) : nil }
        }
        cache[name] = result
        return result
    }

    /// Stroke a named icon into `rect` (flipped view coordinates), 1.8-unit round strokes.
    static func stroke(_ name: String, in rect: NSRect) {
        for line in polylines(name) where line.count > 1 {
            let path = NSBezierPath()
            path.lineWidth = 1.8 * rect.width / 24
            path.lineCapStyle = .round; path.lineJoinStyle = .round
            for (index, point) in line.enumerated() {
                let mapped = NSPoint(x: rect.minX + point.x * rect.width / 24,
                                     y: rect.minY + point.y * rect.height / 24)
                if index == 0 { path.move(to: mapped) } else { path.line(to: mapped) }
            }
            path.stroke()
        }
    }
}

// NSButton keeps keyboard activation, target/action and accessibility behavior;
// only the visual treatment is custom. No stock Aqua bezel in the content UI.
final class CaptureButton: NSButton {
    var tokens: Tokens!
    var selected = false
    var glass = false
    var primary = false
    /// Neutral high-contrast action (`--solid`), e.g. the setup primary button.
    var solid = false { didSet { needsDisplay = true } }
    var signal = false
    var hudControl = false { didSet { updateTrackingAreas(); needsDisplay = true } }
    private var hoverTracking: NSTrackingArea?
    private var hovered = false
    var icon: CaptureButtonIcon? { didSet { updateTrackingAreas(); needsDisplay = true } }
    var actionBlock: (() -> Void)?
    /// Hover or keyboard focus changed; the recording HUD shows its styled tooltip.
    var highlightChanged: ((CaptureButton, Bool) -> Void)?
    var enterActionBlock: (() -> Void)?
    var escapeActionBlock: (() -> Void)?

    init(_ title: String, frame: NSRect, tokens: Tokens, glass: Bool = false, action: @escaping () -> Void) {
        super.init(frame: frame)
        self.tokens = tokens
        self.glass = glass
        self.title = title
        actionBlock = action
        isBordered = false
        setButtonType(.momentaryPushIn)
        target = self
        self.action = #selector(activate)
        setAccessibilityLabel(title)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    @objc private func activate() { actionBlock?() }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTracking { removeTrackingArea(hoverTracking) }
        hoverTracking = nil
        if hudControl || icon?.isEditorTool == true {
            let tracking = NSTrackingArea(rect: .zero,
                options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                owner: self, userInfo: nil)
            addTrackingArea(tracking); hoverTracking = tracking
        }
    }

    override func mouseEntered(with event: NSEvent) {
        hovered = true; needsDisplay = true
        highlightChanged?(self, true)
    }
    override func mouseExited(with event: NSEvent) {
        hovered = false; needsDisplay = true
        highlightChanged?(self, window?.firstResponder === self)
    }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        needsDisplay = true
        if accepted { highlightChanged?(self, true) }
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        needsDisplay = true
        if accepted { highlightChanged?(self, hovered) }
        return accepted
    }

    override func keyDown(with event: NSEvent) {
        if (event.keyCode == 36 || event.keyCode == 76), let enterActionBlock {
            enterActionBlock()
        } else if event.keyCode == 53, let escapeActionBlock {
            escapeActionBlock()
        } else {
            super.keyDown(with: event)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let editorTool = icon?.isEditorTool == true
        let radius = tokens.number(editorTool ? "r-lg" : hudControl ? "r-sm" : "r-md")
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1),
            xRadius: radius, yRadius: radius)
        if hudControl {
            let active = cell?.isHighlighted == true
            let fill: String? = !isEnabled ? nil
                : signal ? (hovered || active ? "theme-signal" : "theme-signal-surface")
                : active || selected ? "glass-active" : hovered ? "glass-hover" : nil
            if let fill { tokens.color(fill).setFill(); path.fill() }
            if isEnabled && (signal || selected) {
                tokens.color(signal ? "theme-signal" : "theme-accent")
                    .withAlphaComponent(signal ? 0.4 : 0.3).setStroke()
                path.lineWidth = 1; path.stroke()
            }
        } else if editorTool {
            if isEnabled && (selected || hovered || cell?.isHighlighted == true) {
                tokens.color(selected ? "theme-accent" : "surface-hover").setFill()
                path.fill()
            }
        } else if solid {
            tokens.color(cell?.isHighlighted == true ? "solid-hover" : "solid")
                .withAlphaComponent(isEnabled ? 1 : 0.4).setFill()
            path.fill()
        } else {
            let fill = !isEnabled ? (glass ? "glass" : "surface-sunken")
                : signal ? "theme-signal-surface"
                : primary ? "theme-accent"
                : cell?.isHighlighted == true ? (glass ? "glass-active" : "surface-active")
                : selected ? (glass ? "glass-active" : "surface-selected") : (glass ? "glass-raised" : "control")
            tokens.color(fill).setFill()
            path.fill()
            tokens.color(signal && isEnabled ? "theme-signal"
                : primary && isEnabled ? "theme-accent"
                : selected && isEnabled ? "theme-accent"
                : (glass ? "glass-border" : "control-border")).setStroke()
            path.lineWidth = 1
            path.stroke()
        }
        let font = NSFont.systemFont(ofSize: tokens.number("text-md"), weight: solid ? .semibold : .medium)
        var foreground = solid
            ? tokens.color("solid-ink").withAlphaComponent(isEnabled ? 1 : 0.4)
            : tokens.color(isEnabled
            ? (signal ? "theme-signal"
                : primary ? "theme-accent-ink" : glass ? "glass-text" : "text")
            : (glass ? "glass-text-subtle" : "text-faint"))
        if hudControl {
            foreground = tokens.color(!isEnabled ? "glass-text-subtle"
                : signal && !hovered && cell?.isHighlighted != true ? "theme-signal"
                : selected ? "theme-accent"
                : hovered || cell?.isHighlighted == true ? "glass-text" : "glass-text-muted")
        } else if editorTool && isEnabled && !selected {
            foreground = tokens.color(hovered ? "text" : "text-muted")
        }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font, .foregroundColor: foreground,
        ]
        let size = (title as NSString).size(withAttributes: attributes)
        let iconSide: CGFloat = icon?.isEditorTool == true ? tokens.number("s-6") + tokens.number("s-1")
            : icon?.isShipping == true ? 16 : 14
        let iconWidth: CGFloat = icon == nil ? 0 : (title.isEmpty ? iconSide : iconSide + 6)
        let startX = (bounds.width - size.width - iconWidth) / 2
        if let icon { draw(icon, in: NSRect(x: startX, y: (bounds.height - iconSide) / 2,
            width: iconSide, height: iconSide), color: foreground) }
        (title as NSString).draw(at: CGPoint(x: startX + iconWidth,
            y: (bounds.height - size.height) / 2), withAttributes: attributes)
        if window?.firstResponder === self {
            tokens.color("theme-accent").setStroke()
            path.lineWidth = 2
            path.stroke()
        }
    }

    private func draw(_ icon: CaptureButtonIcon, in rect: NSRect, color: NSColor) {
        color.setStroke(); color.setFill()
        switch icon {
        case .shipping(let name):
            ShippingIcons.stroke(name, in: rect)
        case .stopSquare:
            tokens.color(isEnabled ? "theme-signal" : "glass-text-subtle").setFill()
            NSBezierPath(roundedRect: NSRect(x: rect.midX - 5.5, y: rect.midY - 5.5, width: 11, height: 11),
                         xRadius: 2, yRadius: 2).fill()
        case .editorSelect, .editorCrop, .editorText, .editorShapes, .editorArrow, .editorPen, .editorBackground:
            // The shipping EditorIcon silhouettes, in their 24-unit coordinate space.
            func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
                NSPoint(x: rect.minX + x * rect.width / 24, y: rect.minY + y * rect.height / 24)
            }
            let path = NSBezierPath()
            path.lineWidth = 1.75 * rect.width / 24
            path.lineCapStyle = .round; path.lineJoinStyle = .round
            func line(_ points: [(CGFloat, CGFloat)]) {
                path.move(to: point(points[0].0, points[0].1))
                points.dropFirst().forEach { path.line(to: point($0.0, $0.1)) }
            }
            switch icon {
            case .editorSelect: line([(5, 3), (18, 12), (11, 14), (8, 21), (5, 3)])
            case .editorCrop:
                line([(7, 3), (7, 17), (9, 19), (21, 19)]); line([(3, 7), (17, 7), (19, 9), (19, 21)])
            case .editorText:
                line([(5, 5), (19, 5)]); line([(12, 5), (12, 19)]); line([(8, 19), (16, 19)])
            case .editorShapes:
                path.appendRoundedRect(NSRect(origin: point(3.5, 8.5), size: NSSize(width: rect.width * 11 / 24, height: rect.height * 11 / 24)), xRadius: 1, yRadius: 1)
                path.appendOval(in: NSRect(origin: point(10, 4.5), size: NSSize(width: rect.width * 10.5 / 24, height: rect.height * 10.5 / 24)))
            case .editorArrow: line([(4, 20), (20, 4)]); line([(12, 4), (20, 4), (20, 12)])
            case .editorPen:
                path.move(to: point(4, 16))
                path.curve(to: point(12, 13), controlPoint1: point(8, 9), controlPoint2: point(10, 8))
                path.curve(to: point(20, 9), controlPoint1: point(14, 18), controlPoint2: point(16, 17))
                line([(4, 20), (20, 20)])
            case .editorBackground:
                line([(14.8, 20.5), (6, 11.4), (14.9, 2.3), (21.7, 9.1), (11, 19.8), (8.2, 17)])
                line([(8.6, 11.8), (12.2, 15.4)]); line([(4, 21), (12, 21)])
            default: break
            }
            path.stroke()
        case .record:
            NSBezierPath(ovalIn: rect.insetBy(dx: 3, dy: 3)).fill()
        case .capture:
            let path = NSBezierPath(); path.lineWidth = 1.5
            let length: CGFloat = 5
            for (origin, dx, dy) in [(NSPoint(x: rect.minX, y: rect.minY), length, length),
                                     (NSPoint(x: rect.maxX, y: rect.minY), -length, length),
                                     (NSPoint(x: rect.minX, y: rect.maxY), length, -length),
                                     (NSPoint(x: rect.maxX, y: rect.maxY), -length, -length)] {
                path.move(to: NSPoint(x: origin.x + dx, y: origin.y))
                path.line(to: origin); path.line(to: NSPoint(x: origin.x, y: origin.y + dy))
            }
            path.stroke()
        case .window:
            let path = NSBezierPath(roundedRect: rect.insetBy(dx: 1, dy: 2), xRadius: 2, yRadius: 2)
            path.lineWidth = 1.4; path.stroke()
            let divider = NSBezierPath(); divider.move(to: NSPoint(x: rect.minX + 1, y: rect.minY + 5))
            divider.line(to: NSPoint(x: rect.maxX - 1, y: rect.minY + 5)); divider.stroke()
        case .display:
            let screen = NSBezierPath(roundedRect: NSRect(x: rect.minX, y: rect.minY + 3,
                width: rect.width, height: 10), xRadius: 2, yRadius: 2)
            screen.lineWidth = 1.4; screen.stroke()
            let stand = NSBezierPath(); stand.move(to: NSPoint(x: rect.midX, y: rect.minY + 3))
            stand.line(to: NSPoint(x: rect.midX, y: rect.minY))
            stand.move(to: NSPoint(x: rect.midX - 3, y: rect.minY))
            stand.line(to: NSPoint(x: rect.midX + 3, y: rect.minY)); stand.stroke()
        case .microphone(let muted):
            // The geometry below is bottom-up; NSButton draws in flipped coordinates.
            NSGraphicsContext.saveGraphicsState()
            defer { NSGraphicsContext.restoreGraphicsState() }
            if isFlipped {
                let transform = NSAffineTransform()
                transform.translateX(by: 0, yBy: rect.minY + rect.maxY)
                transform.scaleX(by: 1, yBy: -1)
                transform.concat()
            }
            let capsule = NSBezierPath(roundedRect: NSRect(x: rect.midX - 2.5, y: rect.minY + 5,
                width: 5, height: 8), xRadius: 2.5, yRadius: 2.5)
            capsule.lineWidth = 1.4; capsule.stroke()
            let stand = NSBezierPath(); stand.lineWidth = 1.4
            stand.move(to: NSPoint(x: rect.minX + 2, y: rect.minY + 8))
            stand.curve(to: NSPoint(x: rect.maxX - 2, y: rect.minY + 8),
                controlPoint1: NSPoint(x: rect.minX + 2, y: rect.minY + 1),
                controlPoint2: NSPoint(x: rect.maxX - 2, y: rect.minY + 1))
            stand.move(to: NSPoint(x: rect.midX, y: rect.minY + 3))
            stand.line(to: NSPoint(x: rect.midX, y: rect.minY))
            stand.move(to: NSPoint(x: rect.midX - 3, y: rect.minY))
            stand.line(to: NSPoint(x: rect.midX + 3, y: rect.minY)); stand.stroke()
            if muted {
                let slash = NSBezierPath(); slash.lineWidth = 1.6
                slash.move(to: NSPoint(x: rect.minX, y: rect.maxY))
                slash.line(to: NSPoint(x: rect.maxX, y: rect.minY)); slash.stroke()
            }
        }
    }
}

final class RootWindowCloseHandler: NSObject, NSWindowDelegate {
    weak var rootWindow: NSWindow?
    private let closePreviews: () -> Void
    private let terminate: () -> Void
    private let hidesRootWindow: Bool

    init(rootWindow: NSWindow, closePreviews: @escaping () -> Void,
         terminate: @escaping () -> Void, hidesRootWindow: Bool = false) {
        self.rootWindow = rootWindow; self.closePreviews = closePreviews
        self.terminate = terminate; self.hidesRootWindow = hidesRootWindow
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard sender === rootWindow else { return true }
        if hidesRootWindow {
            sender.orderOut(nil)
            return false
        }
        closePreviews()
        return true
    }

    func windowWillClose(_ notification: Notification) {
        guard let closing = notification.object as? NSWindow,
              closing === rootWindow else { return }
        terminate()
    }
}

final class LiveStatusActions: NSObject {
    private let newCaptureAction: () -> Void
    private let captureAction: (StillCaptureKind) -> Void
    private let recordAction: (UnifiedCaptureTarget) -> Void
    private let historyAction: () -> Void
    private let preferencesAction: () -> Void
    private let feedbackAction: () -> Void
    private let outputFolderAction: () -> Void
    private let quitAction: () -> Void
    /// `captureShortcutSignature` order: New Capture, Screenshot Region/Window/
    /// Display, Record Region/Window/Display. Shown as key equivalents.
    var shortcuts: [String] = []

    init(newCapture: @escaping () -> Void,
         capture: @escaping (StillCaptureKind) -> Void,
         record: @escaping (UnifiedCaptureTarget) -> Void = { _ in },
         history: @escaping () -> Void, preferences: @escaping () -> Void,
         feedback: @escaping () -> Void = {},
         outputFolder: @escaping () -> Void, quit: @escaping () -> Void) {
        newCaptureAction = newCapture; captureAction = capture; recordAction = record
        historyAction = history; preferencesAction = preferences; feedbackAction = feedback
        outputFolderAction = outputFolder; quitAction = quit
    }

    @objc func newCapture() { newCaptureAction() }
    @objc func captureRegion() { captureAction(.region) }
    @objc func captureWindow() { captureAction(.window) }
    @objc func captureDisplay() { captureAction(.display) }
    @objc func recordRegion() { recordAction(.region) }
    @objc func recordWindow() { recordAction(.window) }
    @objc func recordDisplay() { recordAction(.display) }
    @objc func showHistory() { historyAction() }
    @objc func showPreferences() { preferencesAction() }
    @objc func sendFeedback() { feedbackAction() }
    @objc func openOutputFolder() { outputFolderAction() }
    @objc func quit() { quitAction() }

    /// Shipping `build_tray_menu` order. Hidden recording controls return
    /// through any capture item or New Capture, as in the shipping tray.
    func makeMenu() -> NSMenu {
        let menu = NSMenu()
        add("New Capture…", action: #selector(newCapture), shortcut: 0, to: menu)
        add("Screenshot Region", action: #selector(captureRegion), shortcut: 1, to: menu)
        add("Screenshot Window", action: #selector(captureWindow), shortcut: 2, to: menu)
        add("Screenshot Display", action: #selector(captureDisplay), shortcut: 3, to: menu)
        add("Record Region", action: #selector(recordRegion), shortcut: 4, to: menu)
        add("Record Window", action: #selector(recordWindow), shortcut: 5, to: menu)
        add("Record Display", action: #selector(recordDisplay), shortcut: 6, to: menu)
        menu.addItem(.separator())
        add("Capture History…", action: #selector(showHistory), to: menu)
        add("Open Save Location", action: #selector(openOutputFolder), to: menu)
        add("Preferences", action: #selector(showPreferences), to: menu)
        add("Send Feedback…", action: #selector(sendFeedback), to: menu)
        // Signed updates are not connected yet; keep the shipping row visible.
        let updates = NSMenuItem(title: "Check for Updates…", action: nil, keyEquivalent: "")
        updates.isEnabled = false
        menu.addItem(updates)
        menu.addItem(.separator())
        add("Quit Captures", action: #selector(quit), to: menu)
        menu.autoenablesItems = false
        return menu
    }

    private func add(_ title: String, action: Selector, shortcut: Int? = nil, to menu: NSMenu) {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
        if let shortcut, shortcuts.indices.contains(shortcut),
           let key = menuKeyEquivalent(shortcuts[shortcut]) {
            item.keyEquivalent = key.character
            item.keyEquivalentModifierMask = key.modifiers
        }
        item.target = self
        menu.addItem(item)
    }
}

/// A saved shortcut ("CommandOrControl+Shift+Space", "Ctrl+Alt+KeyR") as an
/// NSMenuItem key equivalent, or nil when AppKit cannot display it.
func menuKeyEquivalent(_ shortcut: String) -> (character: String, modifiers: NSEvent.ModifierFlags)? {
    let tokens = shortcut.split(separator: "+").map { $0.trimmingCharacters(in: .whitespaces) }
    guard let key = tokens.last, !key.isEmpty else { return nil }
    var modifiers: NSEvent.ModifierFlags = []
    for token in tokens.dropLast() {
        switch token.lowercased() {
        case "commandorcontrol", "commandorctrl", "cmdorctrl", "cmdorcontrol",
             "command", "cmd", "super", "meta": modifiers.insert(.command)
        case "control", "ctrl": modifiers.insert(.control)
        case "shift": modifiers.insert(.shift)
        case "alt", "option": modifiers.insert(.option)
        default: return nil
        }
    }
    let lower = key.lowercased()
    let character: String
    if lower.hasPrefix("key"), key.count == 4 { character = String(key.suffix(1)).lowercased() }
    else if lower.hasPrefix("digit"), key.count == 6 { character = String(key.suffix(1)) }
    else if key.count == 1 { character = key.lowercased() }
    else if lower == "space" { character = " " }
    else if lower == "enter" || lower == "return" { character = "\r" }
    else if lower == "tab" { character = "\t" }
    else if lower == "escape" || lower == "esc" { character = "\u{1b}" }
    else if lower.hasPrefix("f"), let number = Int(lower.dropFirst()), (1...20).contains(number),
            let scalar = UnicodeScalar(UInt32(NSF1FunctionKey + number - 1)) {
        character = String(Character(scalar))
    } else { return nil }
    return (character, modifiers)
}

enum LiveReopenAction: Equatable {
    case focusExisting
    case showPreferences
}

func liveReopenAction(hasVisibleWindows: Bool) -> LiveReopenAction {
    hasVisibleWindows ? .focusExisting : .showPreferences
}

struct StartupDecision: Equatable {
    let scene: String
    let showsWindow: Bool
    let activatesApplication: Bool
}

func startupDecision(options: Options) -> StartupDecision {
    let hiddenLive = options.live && options.scene == "idle"
    return StartupDecision(scene: options.live ? "live" : options.scene,
        showsWindow: options.scene != "idle", activatesApplication: !hiddenLive)
}

func captureShortcutSignature(_ settings: [String: Any]) -> [String] {
    let recording = settings["recording"] as? [String: Any] ?? [:]
    return [settings.string("new_capture_shortcut"), settings.string("region_shortcut"), settings.string("window_shortcut"),
        settings.string("display_shortcut"), recording.string("video_shortcut"),
        recording.string("window_shortcut"), recording.string("display_shortcut")]
}

func captureShortcutsEnabled(captureBusy: Bool, selectorGeneration: UInt64? = nil,
                             recordingControlsHidden: Bool = false) -> Bool {
    !captureBusy || selectorGeneration != nil || recordingControlsHidden
}

func captureShortcutsSuspended(preferencesFocused: Bool) -> Bool { preferencesFocused }

func preferencesWindowFocused(scene: String, visible: Bool, key: Bool,
                              attachedSheetKey: Bool) -> Bool {
    scene == "preferences" && visible && (key || attachedSheetKey)
}

func stillCaptureKind(for shortcut: CaptureShortcut) -> StillCaptureKind? {
    switch shortcut {
    case .newCapture, .recordRegion, .recordWindow, .recordDisplay: return nil
    case .region: return .region
    case .window: return .window
    case .display: return .display
    }
}

func configureStatusItemButton(_ button: NSStatusBarButton) {
    if let image = NSImage(systemSymbolName: "camera.viewfinder", accessibilityDescription: "Captures") {
        image.isTemplate = true
        button.image = image
    } else {
        button.title = "C"
    }
    button.setAccessibilityLabel("Captures")
}

func performTermination(flushPreferences: () -> Void, cancelCapture: () -> Void,
                        closeShortcuts: () -> Void, closePreviews: () -> Void,
                        drainActions: () -> Void,
                        removeExerciseDirectory: () -> Void) {
    flushPreferences()
    cancelCapture()
    closeShortcuts()
    closePreviews()
    drainActions()
    removeExerciseDirectory()
}

final class Workbench: NSObject, NSApplicationDelegate, NSTableViewDataSource, NSTableViewDelegate {
    let options: Options
    private var nativeInstance: NativeInstance?
    private var instanceWakeObserver: NSObjectProtocol?
    private var window: NSWindow!
    private var content: Surface!
    private var preview: PreviewView?
    private var table: NSTableView?
    private var preferencesController: PreferencesController?
    private var feedbackController: FeedbackController?
    private var liveController: LiveCaptureController?
    private var pendingOpenImages: [String] = []
    private var miniPreviews: MiniPreviewController?
    private var miniPreviewActions: MiniPreviewActions?
    private var rootWindowCloseHandler: RootWindowCloseHandler?
    private var statusItem: NSStatusItem?
    private var statusActions: LiveStatusActions?
    private var startupNotice: StartupNoticeController?
    private var captureShortcuts: NativeCaptureShortcuts?
    private var shortcutWakeObserver: NSObjectProtocol?
    private var shortcutFocusObservers: [NSObjectProtocol] = []
    private var shortcutSignature: [String]?
    private var shortcutEnabled: Bool?
    private var shortcutSelectorGeneration: UInt64?
    private var captureBusy = false
    private var terminating = false
    private var onboardingReady = false
    private var onboardingWasPresented = false
    private var onboardingController: OnboardingController?
    private var onboardingView: OnboardingView?
    private var permissionController: OnboardingController?
    private var permissionSheet: NSWindow?
    private var liveContent: Surface?
    private var liveStyleRevision = 0
    private var renderedLiveStyleRevision = -1
    private var regionSelector: RegionSelectionView?
    private var windowSelector: WindowSelectionView?
    private var updateNotice: UpdateNoticeController?
    private var updateNoticeSettings: SettingsStore?
    private var scene: String
    private var appearance: String
    private var theme: String
    private var customTheme: [String: Any] = [:]
    private var paused = false
    private var historyCount: Int
    private lazy var resolvedTokens = makeTokens()
    private var exerciseDirectory: URL?
    private var tokens: Tokens { resolvedTokens }
    private func makeTokens() -> Tokens {
        let mode = appearance == "system"
            ? (NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? "dark" : "light")
            : appearance
        let base = Tokens.variants["\(mode)-\(theme)"] ?? Tokens.variants["\(mode)-mustard"]!
        return theme == "custom" ? base.applyingCustomTheme(customTheme, light: mode == "light") : base
    }

    init(options: Options, nativeInstance: NativeInstance? = nil) {
        self.options = options
        self.nativeInstance = nativeInstance
        pendingOpenImages = options.openMedia
        scene = startupDecision(options: options).scene
        appearance = options.appearance
        theme = options.theme
        historyCount = options.historyCount
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        if nativeInstance != nil {
            instanceWakeObserver = NotificationCenter.default.addObserver(
                forName: NativeInstance.wakeNotification, object: nil, queue: .main
            ) { [weak self] _ in self?.drainInstanceRequests() }
        }
        let menu = NSMenu()
        let item = NSMenuItem()
        menu.addItem(item)
        let appMenu = NSMenu()
        let quit = appMenu.addItem(withTitle: options.live ? "Quit Captures" : "Quit Captures Native Workbench",
                                   action: #selector(quitApplication), keyEquivalent: "q")
        quit.target = self
        item.submenu = appMenu
        let editItem = NSMenuItem()
        menu.addItem(editItem)
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenu.addItem(.separator())
        let find = editMenu.addItem(withTitle: "Find…", action: #selector(showFind), keyEquivalent: "f")
        find.target = self
        let next = editMenu.addItem(withTitle: "Find Next", action: #selector(findNext), keyEquivalent: "g")
        next.target = self
        let previous = editMenu.addItem(withTitle: "Find Previous", action: #selector(findPrevious), keyEquivalent: "G")
        previous.keyEquivalentModifierMask = [.command, .shift]
        previous.target = self
        editItem.submenu = editMenu
        NSApp.mainMenu = menu
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 720),
            styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = options.live ? "Captures Native — capture workspace" : "Captures Native — development fixtures"
        window.isReleasedWhenClosed = false
        window.center()
        let miniPreviews = MiniPreviewController(tokens: tokens)
        let miniPreviewActions = MiniPreviewActions(settingsPath: options.settingsFile)
        miniPreviewActions.bind(previews: miniPreviews)
        miniPreviews.copyArtifact = { [weak miniPreviewActions] artifact in miniPreviewActions?.copy(artifact) }
        miniPreviews.saveArtifact = { [weak miniPreviewActions] artifact in miniPreviewActions?.save(artifact) }
        miniPreviews.openArtifact = { [weak self] artifact in self?.openPreview(artifact) }
        miniPreviews.trashArtifact = { [weak miniPreviewActions] artifact in miniPreviewActions?.trash(artifact) }
        miniPreviews.prepareDrag = { [weak miniPreviewActions] artifact, completion in
            miniPreviewActions?.prepareDrag(artifact, completion: completion)
        }
        self.miniPreviews = miniPreviews
        self.miniPreviewActions = miniPreviewActions
        let rootWindowCloseHandler = RootWindowCloseHandler(rootWindow: window,
            closePreviews: { [weak miniPreviews] in miniPreviews?.close() },
            terminate: { NSApp.terminate(nil) }, hidesRootWindow: options.live)
        self.rootWindowCloseHandler = rootWindowCloseHandler
        window.delegate = rootWindowCloseHandler
        if options.live { scene = "onboarding" }
        render()
        if options.live {
            installStatusItem()
            startOnboarding()
        }
        let startup = startupDecision(options: options)
        if !options.live, startup.showsWindow { window.makeKeyAndOrderFront(nil) }
        if !options.live, startup.activatesApplication { NSApp.activate(ignoringOtherApps: true) }
        Metrics.write(["event": "ready", "scene": scene, "window": window.windowNumber,
            "scale": window.backingScaleFactor, "appearance": appearance, "theme": theme,
            "historyCount": historyCount, "referenceChips": options.referenceChips])
        DistributedNotificationCenter.default().addObserver(self, selector: #selector(systemAppearanceChanged),
            name: NSNotification.Name("AppleInterfaceThemeChangedNotification"), object: nil)
        if let path = options.screenshot {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.captureWindow(to: path) }
        }
        if options.exercise {
            for cycle in 0..<6 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 2 + Double(cycle) * 4) { [weak self] in
                    self?.exercise(cycle)
                }
            }
        }
        if let seconds = options.quitAfter {
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { NSApp.terminate(nil) }
        }
        // Also drain once after the workspace is ready. A worker wake posted
        // before the observer existed therefore cannot strand startup traffic.
        drainInstanceRequests()
    }

    func application(_ sender: NSApplication, openFiles filenames: [String]) {
        guard options.live else {
            let message = "Opening external media requires --live; fixture mode cannot open files."
            if window != nil { presentHostError(title: "Media Open Unavailable", message: message) }
            else { FileHandle.standardError.write(Data("\(message)\n".utf8)) }
            sender.reply(toOpenOrPrint: .failure)
            return
        }
        pendingOpenImages.append(contentsOf: filenames)
        drainOpenImages()
        sender.reply(toOpenOrPrint: .success)
    }

    private func drainOpenImages() {
        guard options.live, onboardingReady, permissionSheet == nil, !pendingOpenImages.isEmpty else { return }
        if scene != "live" { scene = "live"; render() }
        guard let liveController else { return }
        liveController.openImages(pendingOpenImages)
        pendingOpenImages.removeAll()
    }

    private func drainInstanceRequests() {
        guard let nativeInstance else { return }
        do {
            for _ in 0..<32 {
                guard let paths = try nativeInstance.nextRequest() else { break }
                if paths.isEmpty {
                    _ = applicationShouldHandleReopen(NSApp,
                        hasVisibleWindows: NSApp.windows.contains(where: \.isVisible))
                    Metrics.write(["event": "instance-relaunch"])
                } else {
                    pendingOpenImages.append(contentsOf: paths)
                    drainOpenImages()
                }
            }
        } catch {
            presentHostError(title: "Couldn’t Open Captures", message: error.localizedDescription)
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { !options.live }
    func applicationDidBecomeActive(_ notification: Notification) {
        if let permissionController, !permissionController.busy { permissionController.check() }
        guard scene == "onboarding", onboardingController?.busy == false,
              onboardingController?.state != nil else { return }
        onboardingController?.check()
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        // Drain the editor's dedicated worker before capture teardown. A failed
        // draft save keeps its session/window recoverable and cancels this quit.
        if liveController?.prepareEditorForTermination() == false {
            terminating = false
            return .terminateCancel
        }
        terminating = true
        nativeInstance?.stopAccepting()
        onboardingController?.flush()
        permissionController?.flush()
        performTermination(flushPreferences: { [weak self] in self?.preferencesController?.flush() },
            cancelCapture: { [weak self] in self?.liveController?.finishCapture(restoreWindow: false) },
            closeShortcuts: { [weak self] in self?.closeCaptureShortcuts() },
            closePreviews: { [weak self] in self?.miniPreviews?.close() },
            drainActions: { LiveCaptureController.flush() },
            removeExerciseDirectory: { [weak self] in
                if let directory = self?.exerciseDirectory {
                    try? FileManager.default.removeItem(at: directory)
                }
            })
        if let instanceWakeObserver {
            NotificationCenter.default.removeObserver(instanceWakeObserver)
            self.instanceWakeObserver = nil
        }
        nativeInstance?.close()
        nativeInstance = nil
        return .terminateNow
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        guard options.live else { return true }
        guard onboardingReady else {
            window.makeKeyAndOrderFront(nil)
            sender.activate(ignoringOtherApps: true)
            return true
        }
        if liveController?.showRecordingControls() == true { return true }
        switch liveReopenAction(hasVisibleWindows: flag) {
        case .focusExisting:
            window.makeKeyAndOrderFront(nil)
            sender.activate(ignoringOtherApps: true)
        case .showPreferences:
            showPreferences()
        }
        return true
    }

    @objc private func quitApplication() { NSApp.terminate(nil) }

    @objc private func showFind() { if scene == "preferences" { preferencesController?.showFind() } }
    @objc private func findNext() { preferencesController?.stepFind(1) }
    @objc private func findPrevious() { preferencesController?.stepFind(-1) }
    @objc private func systemAppearanceChanged() {
        guard appearance == "system" else { return }
        resolvedTokens = makeTokens()
        if scene == "onboarding" { render(); return }
        preferencesController?.restyle()
        feedbackController?.restyle(tokens)
        renderPermissionSheet()
        liveStyleRevision += 1
        rebuildRenderedLiveWorkspaceIfNeeded()
    }

    private func showPermissions() {
        guard onboardingReady, !terminating, !captureBusy, window.attachedSheet == nil else { return }
        do {
            // Use a separate controller: a completed profile must not run the
            // first-run completion/restart callbacks when checking revoked access.
            preferencesController?.flush()
            permissionController = OnboardingController(store: try SettingsStore(path: options.settingsFile))
            let sheet = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 700, height: 620),
                styleMask: [.titled], backing: .buffered, defer: false)
            sheet.title = "Capture permissions"
            sheet.isReleasedWhenClosed = false
            permissionSheet = sheet
            renderPermissionSheet()
            liveController?.setPermissionsVisible(true)
            window.beginSheet(sheet)
            permissionController?.check()
        } catch {
            presentHostError(title: "Permissions Unavailable", message: error.localizedDescription)
        }
    }

    private func renderPermissionSheet() {
        guard let permissionSheet, let permissionController else { return }
        permissionSheet.appearance = NSAppearance(named: tokens.color("text").brightnessComponent > 0.5 ? .darkAqua : .aqua)
        permissionSheet.contentView = OnboardingView(frame: NSRect(x: 0, y: 0, width: 700, height: 620),
            tokens: tokens, controller: permissionController, done: { [weak self] in self?.closePermissions() })
    }

    private func closePermissions() {
        guard let sheet = permissionSheet, permissionController?.busy == false else { return }
        window.endSheet(sheet)
        sheet.orderOut(nil)
        permissionSheet = nil
        permissionController = nil
        liveController?.setPermissionsVisible(false)
        drainOpenImages()
    }

    private func startOnboarding() {
        do {
            let store = try SettingsStore(path: options.settingsFile)
            let controller = OnboardingController(store: store)
            controller.completed = { [weak self] in self?.finishOnboarding() }
            controller.requiresAttention = { [weak self] in
                guard let self, !self.onboardingWasPresented else { return }
                self.showOnboarding()
            }
            onboardingController = controller
            scene = "onboarding"
            render()
            store.load { [weak self, weak controller] result in
                guard let self, let controller else { return }
                if case .success(let settings) = result {
                    if !self.options.appearanceOverride { self.appearance = settings.string("appearance", "system") }
                    if !self.options.themeOverride { self.theme = settings.string("theme", "mustard") }
                    self.customTheme = settings["custom_theme"] as? [String: Any] ?? [:]
                    self.resolvedTokens = self.makeTokens()
                    self.render()
                }
                controller.check()
            }
        } catch {
            showOnboarding()
            presentHostError(title: "Setup Unavailable", message: error.localizedDescription)
        }
    }

    private func showOnboarding() {
        guard options.live else { return }
        onboardingWasPresented = true
        scene = "onboarding"
        render()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func finishOnboarding() {
        guard !onboardingReady else { return }
        onboardingReady = true
        scene = "live"
        render()
        installCaptureShortcuts()
        let startup = startupDecision(options: options)
        if options.live, options.screenshot == nil,
           let trigger = startupNoticeTrigger(setupWasPresented: onboardingWasPresented,
               hiddenLaunch: !startup.showsWindow, openingMedia: !pendingOpenImages.isEmpty) {
            showStartupNotice(trigger)
        }
        if startup.showsWindow || onboardingWasPresented || !pendingOpenImages.isEmpty {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        } else {
            window.orderOut(nil)
        }
        drainOpenImages()
    }

    /// Shipping launch notice pointing at the menu bar item, with the saved
    /// New Capture shortcut. Nonactivating; the Close button dismisses it.
    private func showStartupNotice(_ trigger: StartupNoticeTrigger) {
        let controller = startupNotice ?? StartupNoticeController(tokens: tokens)
        controller.statusItemFrame = { [weak self] in self?.statusItem?.button?.window?.frame }
        startupNotice = controller
        let present: ([String: Any]) -> Void = { [weak controller] settings in
            let saved = settings.string("new_capture_shortcut", StartupNoticeCopy.defaultShortcut)
            let shortcut = saved.trimmingCharacters(in: .whitespaces).isEmpty
                ? StartupNoticeCopy.defaultShortcut : saved
            let keys = (try? NativeCaptureShortcuts.display(shortcut)) ?? []
            controller?.present(trigger: trigger, keys: keys)
        }
        guard let store = try? SettingsStore(path: options.settingsFile) else { present([:]); return }
        store.load { result in
            if case .success(let settings) = result { present(settings) } else { present([:]) }
        }
    }

    private func restartAfterPermissionRequest() {
        // This first-run restart is unavailable after the workspace opens, so
        // there can be no active recording or unsaved editor to abandon.
        guard !onboardingReady, !terminating, onboardingController?.busy == false else {
            onboardingView?.restartFailed()
            return
        }
        onboardingController?.flush()
        preferencesController?.flush()
        LiveCaptureController.flush()
        nativeInstance?.stopAccepting()
        drainInstanceRequests()
        nativeInstance?.close()
        nativeInstance = nil
        do {
            let process = Process()
            process.executableURL = Bundle.main.executableURL ?? URL(fileURLWithPath: CommandLine.arguments[0])
            process.arguments = permissionRestartArguments(options: options, pendingMedia: pendingOpenImages)
            try process.run()
            terminating = true
            NSApp.terminate(nil)
        } catch {
            // Re-elect this process when spawning fails so instance delivery is
            // not silently left without an owner.
            let restartError = error.localizedDescription
            do {
                let result = try NativeInstance.start(historyRoot: options.historyRoot, paths: pendingOpenImages)
                nativeInstance = result.owner
                if !result.primary {
                    // Another primary accepted the queued media. Do not keep
                    // a second live host after losing the election.
                    NSApp.terminate(nil)
                    return
                }
                onboardingView?.restartFailed()
                presentHostError(title: "Couldn’t Restart Captures", message: restartError)
            } catch {
                presentHostError(title: "Couldn’t Restart Captures",
                    message: "\(restartError) Captures could not restore its application lock and will quit. Reopen it to continue setup.")
                NSApp.terminate(nil)
            }
        }
    }

    private func render() {
        let started = CACurrentMediaTime()
        preview = nil
        table = nil
        regionSelector = nil
        windowSelector = nil
        if scene != "update" {
            updateNotice?.close(); updateNotice = nil; updateNoticeSettings = nil
        }
        if !options.live {
            liveController?.finishCapture(restoreWindow: false)
            liveController = nil
            liveContent = nil
        }
        if options.live, scene == "live", let liveContent, liveController != nil {
            content = liveContent
            window.contentView = liveContent
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
            return
        }
        content = Surface(frame: NSRect(x: 0, y: 0, width: 1000, height: 720))
        content.wantsLayer = true
        content.layer!.backgroundColor = tokens.color("surface-canvas").cgColor
        window.appearance = NSAppearance(named: tokens.color("text").brightnessComponent > 0.5 ? .darkAqua : .aqua)
        window.contentView = content
        if scene == "onboarding" {
            if let onboardingController {
                let view = OnboardingView(frame: content.bounds, tokens: tokens, controller: onboardingController)
                view.autoresizingMask = [.width, .height]
                view.restartRequested = { [weak self] in self?.restartAfterPermissionRequest() }
                content.addSubview(view)
                onboardingView = view
            } else {
                label("Setup is unavailable. Retry to continue.", x: 48, y: 80, width: 800)
                content.addSubview(CaptureButton("Retry setup", frame: NSRect(x: 48, y: 130, width: 150, height: 34), tokens: tokens) {
                    [weak self] in self?.startOnboarding()
                })
            }
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000,
                detail: scene)
            return
        }
        if scene == "preferences" {
            do {
                let path = options.exercise
                    ? exerciseSettingsPath()
                    : options.settingsFile
                let store = try SettingsStore(path: path)
                preferencesController = PreferencesController(root: content, store: store, tokens: { [weak self] in self?.tokens ?? Tokens.variants["dark-mustard"]! }, appearanceChanged: { [weak self] appearance, theme, customTheme in
                    guard let self else { return }
                    let changed = self.appearance != appearance || self.theme != theme || !NSDictionary(dictionary: self.customTheme).isEqual(to: customTheme)
                    self.appearance = appearance
                    self.theme = theme
                    self.customTheme = customTheme
                    if changed {
                        self.resolvedTokens = self.makeTokens()
                        self.liveStyleRevision += 1
                    }
                    self.window.appearance = appearance == "system" ? nil : NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
                }, settingsChanged: { [weak self] settings in
                    guard let enabled = settings["show_mini_previews"] as? Bool,
                          let placement = settings["mini_preview_placement"] as? String,
                          let include = settings["include_mini_previews_in_captures"] as? Bool
                    else { return }
                    self?.miniPreviews?.updateSettings(MiniPreviewSettings(enabled: enabled,
                        placement: placement, includeInCaptures: include))
                }, settingsPersisted: { [weak self] settings in
                    self?.updateCaptureShortcuts(settings: settings)
                }, showHistory: { [weak self] in self?.showHistory() },
                   liveCaptureAvailable: options.live,
                   showFeedback: { [weak self] in self?.showFeedback() },
                   loginItemService: options.live && !options.exercise
                    ? NativeLoginItemService(historyRoot: options.historyRoot,
                                             settingsFile: options.settingsFile) : nil,
                   initialAppearance: options.appearanceOverride ? options.appearance : nil,
                   initialTheme: options.themeOverride ? options.theme : nil)
            } catch {
                label("Preferences unavailable: \(error.localizedDescription)", x: 32, y: 32, width: 900)
            }
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
            return
        }
        preferencesController = nil
        if scene == "region" {
            let selector = RegionSelectionView(frame: content.bounds, image: PreviewView.fixtureImage(scale: 2048.0 / 284.0),
                tokens: tokens, autoStart: false, confirm: { rect in
                    Metrics.write(["event": "region-confirm", "x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height, "fixture": true])
                }, cancel: { [weak self] in self?.scene = "preferences"; self?.render() })
            regionSelector = selector; content.addSubview(selector); window.makeFirstResponder(selector)
            label("Region selection fixture · no capture access", x: 24, y: 20, width: 650, glass: true)
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
            return
        }
        if scene == "window" {
            let targets = [
                WindowSelectionTarget(id: "fixture-front", title: "Draft capture", appName: "Editor",
                    rect: NSRect(x: 96, y: 84, width: 510, height: 352), cornerRadius: 25),
                WindowSelectionTarget(id: "fixture-back", title: "Reference", appName: "Browser",
                    rect: NSRect(x: 440, y: 190, width: 470, height: 380), cornerRadius: 10),
            ]
            let selector = WindowSelectionView(frame: content.bounds,
                image: PreviewView.fixtureImage(scale: 2048.0 / 284.0), targets: targets,
                tokens: tokens, autoStart: false,
                hitTest: { point in point.x < 380 ? 0 : (point.x < 760 ? 1 : -1) },
                confirm: { target in Metrics.write(["event": "window-confirm", "target": String(describing: target), "fixture": true]) },
                cancel: { [weak self] in self?.scene = "preferences"; self?.render() })
            selector.hover(NSPoint(x: 180, y: 140))
            windowSelector = selector; content.addSubview(selector); window.makeFirstResponder(selector)
            label("Window selection fixture · shared hit testing is simulated", x: 24, y: 20, width: 650, glass: true)
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
            return
        }
        if scene == "live" {
            liveContent = content
            liveController = LiveCaptureController(root: content, window: window, tokens: tokens,
                historyRoot: options.historyRoot, settingsPath: options.settingsFile,
                miniPreviews: miniPreviews, miniPreviewActions: miniPreviewActions,
                captureStateChanged: { [weak self] busy in
                    self?.captureBusy = busy
                    self?.updateShortcutState()
                    if !busy {
                        DispatchQueue.main.async { [weak self] in
                            self?.rebuildRenderedLiveWorkspaceIfNeeded()
                        }
                    }
                }, selectorGenerationChanged: { [weak self] generation in
                    self?.shortcutSelectorGeneration = generation
                    self?.updateShortcutState()
                }, recordingControlsVisibilityChanged: { [weak self] _ in
                    self?.updateShortcutState()
                }, reportError: { [weak self] message in
                    self?.presentHostError(title: "Capture Failed", message: message)
                }, showPermissions: { [weak self] in self?.showPermissions() },
                showPreferenceSetting: { [weak self] setting in
                    self?.showPreferences(revealing: setting)
                }) { [weak self] in
                    self?.showPreferences()
                }
            renderedLiveStyleRevision = liveStyleRevision
            drainOpenImages()
            Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
            return
        }
        let sidebar = Surface(frame: NSRect(x: 0, y: 0, width: 196, height: 720))
        sidebar.wantsLayer = true
        sidebar.layer!.backgroundColor = tokens.color("surface-sunken").cgColor
        content.addSubview(sidebar)
        if let url = NativeResources.bundle.url(forResource: "icon", withExtension: "svg"),
           let image = NSImage(contentsOf: url) {
            let icon = NSImageView(frame: NSRect(x: 20, y: 20, width: 28, height: 28))
            icon.image = image
            icon.setAccessibilityElement(false)
            sidebar.addSubview(icon)
        }
        label("Captures", x: 58, y: 22, width: 125, size: "text-xl", parent: sidebar)
        for (i, name) in ["preferences", "history", "hud", "preview", "region", "window", "update"].enumerated() {
            let button = CaptureButton(Self.sceneTitle(name),
                frame: NSRect(x: 12, y: 70 + i * 44, width: 172, height: 34), tokens: tokens) { [weak self] in
                    self?.scene = name
                    self?.render()
                }
            button.selected = scene == name
            sidebar.addSubview(button)
        }
        label("Fixture mode", x: 24, y: 640, width: 155, size: "text-sm", muted: true, parent: sidebar)
        label("No capture access", x: 24, y: 663, width: 155, size: "text-sm", muted: true, parent: sidebar)
        label(Self.sceneTitle(scene),
            x: 220, y: 20, width: 650, size: "text-xl")
        label("Native rendering workbench · synthetic data, not functional parity",
            x: 220, y: 47, width: 740, size: "text-sm", muted: true)
        switch scene {
        case "history": history()
        case "hud": hud()
        case "preview": previews()
        case "update": updateNoticeFixture()
        default: break
        }
        Metrics.emit("scene-construction", milliseconds: (CACurrentMediaTime() - started) * 1000, detail: scene)
    }

    static func sceneTitle(_ name: String) -> String {
        switch name {
        case "hud": return "Recording controls"
        case "update": return "Update notice"
        default: return name.capitalized
        }
    }

    /// Stub-driven update notice. Only an explicit --settings-file persists the
    /// Hide / What's new choice; fixtures never touch the development profile.
    private func updateNoticeFixture() {
        label("Stub status source: no updater, download, install or relaunch is connected.",
            x: 220, y: 90, width: 740, size: "text-sm", muted: true)
        for (i, name) in UpdateNoticeModel.fixtures.enumerated() {
            let button = CaptureButton(name, frame: NSRect(x: 220 + (i % 5) * 150, y: 130 + (i / 5) * 44,
                width: 140, height: 34), tokens: tokens) { [weak self] in self?.showUpdateNotice(name) }
            content.addSubview(button)
        }
        if updateNotice == nil { showUpdateNotice(options.updateState ?? "available") }
    }

    private func showUpdateNotice(_ fixture: String) {
        if updateNotice == nil {
            let controller = UpdateNoticeController(tokens: tokens, tray: options.updateTray ?? "top")
            if let path = options.settingsFile, let store = try? SettingsStore(path: path) {
                updateNoticeSettings = store
                store.load { [weak controller] result in
                    guard case .success(let settings) = result else { return }
                    controller?.model.showChangelog = settings.bool("show_update_changelog", true)
                    controller?.refresh()
                }
                controller.model.persistShowChangelog = { [weak store] show in
                    store?.load { result in
                        guard case .success(var settings) = result else { return }
                        settings["show_update_changelog"] = show
                        store?.save(settings) { _, _ in }
                    }
                }
            }
            updateNotice = controller
        }
        updateNotice?.present(fixture: fixture)
    }

    private func openPreview(_ artifact: CaptureArtifact) {
        if permissionSheet != nil || liveController?.externalOpenPending == true { return }
        liveController?.openPreview(artifact)
    }

    private func installStatusItem() {
        let actions = LiveStatusActions(newCapture: { [weak self] in
            self?.preferencesController?.flush()
            self?.launchNewCapture()
        }, capture: { [weak self] kind in
            self?.preferencesController?.flush()
            self?.launchCapture(kind)
        }, record: { [weak self] target in
            self?.preferencesController?.flush()
            self?.launchNewCapture(recordingTarget: target)
        }, history: { [weak self] in
            self?.showHistory()
        }, preferences: { [weak self] in
            self?.showPreferences()
        }, feedback: { [weak self] in
            self?.showFeedback()
        }, outputFolder: { [weak self] in
            self?.openOutputFolder()
        }, quit: {
            NSApp.terminate(nil)
        })
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = item.button { configureStatusItemButton(button) }
        item.menu = actions.makeMenu()
        statusActions = actions
        statusItem = item
    }

    private func installCaptureShortcuts() {
        shortcutWakeObserver = NotificationCenter.default.addObserver(
            forName: NativeCaptureShortcuts.wakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.drainCaptureShortcuts()
        }
        for name in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification] {
            shortcutFocusObservers.append(NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] notification in
                guard let self, let changed = notification.object as? NSWindow,
                      changed === self.window || changed.sheetParent === self.window else { return }
                self.updateShortcutState()
            })
        }
        do {
            let store = try SettingsStore(path: options.settingsFile)
            store.load { [weak self] result in
                guard let self else { return }
                switch result {
                case .success(let settings): self.updateCaptureShortcuts(settings: settings)
                case .failure(let error): self.reportShortcutError(error)
                }
            }
        } catch {
            reportShortcutError(error)
        }
    }

    private func updateCaptureShortcuts(settings: [String: Any]) {
        guard !terminating else { return }
        let signature = captureShortcutSignature(settings)
        guard signature != shortcutSignature else { return }
        do {
            if let captureShortcuts {
                try captureShortcuts.update(settings: settings)
            } else {
                captureShortcuts = try NativeCaptureShortcuts(settings: settings)
                shortcutEnabled = nil
            }
            shortcutSignature = signature
            statusActions?.shortcuts = signature
            if let statusActions { statusItem?.menu = statusActions.makeMenu() }
            updateShortcutState()
        } catch {
            reportShortcutError(error)
        }
    }

    private func updateShortcutState() {
        guard let captureShortcuts else { return }
        let suspended = captureShortcutsSuspended(preferencesFocused: preferencesFocused)
        do {
            try captureShortcuts.setSuspended(suspended)
            try captureShortcuts.setSelectorGeneration(shortcutSelectorGeneration)
        } catch {
            reportShortcutError(error)
            return
        }
        let controlsHidden = liveController?.recordingControlsHidden == true
        captureShortcuts.setRestoreOnly(controlsHidden)
        let enabled = captureShortcutsEnabled(captureBusy: captureBusy,
            selectorGeneration: shortcutSelectorGeneration,
            recordingControlsHidden: controlsHidden)
        if enabled != shortcutEnabled {
            captureShortcuts.setEnabled(enabled)
            shortcutEnabled = enabled
        }
    }

    private func drainCaptureShortcuts() {
        guard onboardingReady, !terminating, !preferencesFocused,
              captureShortcutsEnabled(captureBusy: captureBusy,
                  selectorGeneration: shortcutSelectorGeneration,
                  recordingControlsHidden: liveController?.recordingControlsHidden == true),
              let captureShortcuts else { return }
        do {
            while let action = try captureShortcuts.nextAction() {
                if liveController?.recordingControlsHidden == true {
                    if action == .newCapture { _ = liveController?.showRecordingControls() }
                } else if shortcutSelectorGeneration != nil {
                    _ = liveController?.selectUnifiedTargetFromShortcut(action)
                } else if action.mode == .record, let target = action.target {
                    launchNewCapture(recordingTarget: target)
                } else if let kind = stillCaptureKind(for: action) {
                    launchCapture(kind)
                } else {
                    launchNewCapture()
                }
                if captureBusy && shortcutSelectorGeneration == nil { break }
            }
        } catch {
            reportShortcutError(error)
        }
    }

    private func closeCaptureShortcuts() {
        if let shortcutWakeObserver {
            NotificationCenter.default.removeObserver(shortcutWakeObserver)
            self.shortcutWakeObserver = nil
        }
        for observer in shortcutFocusObservers {
            NotificationCenter.default.removeObserver(observer)
        }
        shortcutFocusObservers.removeAll()
        captureShortcuts?.close()
        captureShortcuts = nil
        shortcutEnabled = nil
        shortcutSelectorGeneration = nil
    }

    private func reportShortcutError(_ error: Error) {
        Metrics.write(["event": "shortcut-error", "detail": error.localizedDescription])
        liveController?.showShortcutError(error)
        presentHostError(title: "Capture Shortcuts Unavailable", message: error.localizedDescription)
    }

    private func launchCapture(_ kind: StillCaptureKind) {
        guard onboardingReady else { showOnboarding(); return }
        guard permissionSheet == nil else { window.makeKeyAndOrderFront(nil); return }
        // Tray capture items bring hidden recording controls back, like New Capture.
        if liveController?.showRecordingControls() == true { return }
        guard liveController?.capture(kind) == true else {
            presentHostError(title: "Capture Unavailable",
                message: "The capture workspace is still loading or another capture is already active.")
            return
        }
    }

    private func launchNewCapture(recordingTarget: UnifiedCaptureTarget? = nil) {
        guard onboardingReady else { showOnboarding(); return }
        guard permissionSheet == nil else { window.makeKeyAndOrderFront(nil); return }
        if liveController?.showRecordingControls() == true { return }
        guard liveController?.newCapture(recordingTarget: recordingTarget) == true else {
            presentHostError(title: "Capture Unavailable",
                message: "The capture workspace is still loading or another capture is already active.")
            return
        }
    }

    private func showHistory() {
        guard permissionSheet == nil else { window.makeKeyAndOrderFront(nil); return }
        guard options.live else {
            scene = "history"; render(); return
        }
        guard onboardingReady else { showOnboarding(); return }
        preferencesController?.flush()
        scene = "live"
        _ = discardLiveWorkspaceForStyleChange()
        render()
        updateShortcutState()
        liveController?.refreshHistory()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    /// `revealing`: a capture-menu note link's setting key, scrolled to and
    /// briefly highlighted like the shipping `preferences-target` event.
    private func showPreferences(revealing setting: String? = nil) {
        guard permissionSheet == nil else { window.makeKeyAndOrderFront(nil); return }
        guard !options.live || onboardingReady else { showOnboarding(); return }
        preferencesController?.flush()
        scene = "preferences"
        render()
        if let setting { preferencesController?.revealSetting(setting) }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        updateShortcutState()
    }

    private func showFeedback() {
        if feedbackController == nil {
            feedbackController = FeedbackController(tokens: tokens, live: options.live)
        }
        feedbackController?.restyle(tokens)
        feedbackController?.present(on: window)
    }

    private func openOutputFolder() {
        LiveCaptureController.queue.async { [settingsPath = options.settingsFile] in
            let result = Result { () throws -> URL in
                let path = try CapturePreferences.load(path: settingsPath).directory
                let url = URL(fileURLWithPath: path, isDirectory: true)
                try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
                return url
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                switch result {
                case .success(let url):
                    if !NSWorkspace.shared.open(url) {
                        self.presentHostError(title: "Couldn’t Open Save Location",
                            message: "The configured output folder could not be opened.")
                    }
                case .failure(let error):
                    self.presentHostError(title: "Couldn’t Open Save Location",
                        message: error.localizedDescription)
                }
            }
        }
    }

    @discardableResult private func discardLiveWorkspaceForStyleChange() -> Bool {
        guard scene == "live", !captureBusy,
              renderedLiveStyleRevision != liveStyleRevision else { return false }
        guard liveController?.prepareEditorForTermination() != false else { return false }
        liveController?.finishCapture(restoreWindow: false)
        liveController = nil
        liveContent = nil
        return true
    }

    private func rebuildRenderedLiveWorkspaceIfNeeded() {
        guard discardLiveWorkspaceForStyleChange() else { return }
        render()
        liveController?.refreshHistory()
    }

    private func presentHostError(title: String, message: String) {
        guard window.attachedSheet == nil else {
            Metrics.write(["event": "host-error", "detail": "\(title): \(message)"])
            return
        }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = title
        alert.informativeText = message
        alert.addButton(withTitle: "OK")
        if !window.isVisible {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
        alert.beginSheetModal(for: window)
    }

    private var preferencesFocused: Bool {
        preferencesWindowFocused(scene: scene, visible: window.isVisible,
            key: window.isKeyWindow, attachedSheetKey: window.attachedSheet?.isKeyWindow == true)
    }

    private func exerciseSettingsPath() -> String {
        if exerciseDirectory == nil {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("captures-native-exercise-\(ProcessInfo.processInfo.processIdentifier)")
            try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            exerciseDirectory = directory
        }
        return exerciseDirectory!.appendingPathComponent("settings.json").path
    }

    @discardableResult private func label(_ text: String, x: CGFloat, y: CGFloat, width: CGFloat,
        size: String = "text-md", muted: Bool = false, glass: Bool = false, parent: NSView? = nil) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.frame = NSRect(x: x, y: y, width: width, height: 26)
        field.font = NSFont.systemFont(ofSize: tokens.number(size), weight: size == "text-xl" ? .semibold : .regular)
        field.textColor = tokens.color(glass ? "glass-text" : (muted ? "text-muted" : "text"))
        (parent ?? content).addSubview(field)
        return field
    }

    private func panel(_ rect: NSRect, glass: Bool = false) -> Surface {
        let view = Surface(frame: rect)
        view.wantsLayer = true
        view.layer!.backgroundColor = tokens.color(glass ? "glass-strong" : "surface-raised").cgColor
        view.layer!.cornerRadius = tokens.number("r-xl")
        view.layer!.borderWidth = 1
        view.layer!.borderColor = tokens.color(glass ? "glass-border" : "border").cgColor
        content.addSubview(view)
        return view
    }

    private func captureWindow(to path: String) {
        guard let view = window.contentView else { return }
        let representation = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        guard let representation else { return }
        view.cacheDisplay(in: view.bounds, to: representation)
        guard let data = representation.representation(using: .png, properties: [:]) else { return }
        do {
            try data.write(to: URL(fileURLWithPath: path), options: .atomic)
            Metrics.write(["event": "screenshot", "path": path, "window": window.windowNumber])
        } catch { Metrics.write(["event": "screenshot-error", "detail": error.localizedDescription]) }
    }

    private func history() {
        label("\(historyCount) synthetic captures · reusable native rows", x: 220, y: 90, width: 620, muted: true)
        let empty = CaptureButton(historyCount == 0 ? "Load 1,000" : "Empty state",
            frame: NSRect(x: 836, y: 82, width: 136, height: 34), tokens: tokens) { [weak self] in
                guard let self else { return }
                self.historyCount = self.historyCount == 0 ? 1000 : 0
                self.render()
            }
        content.addSubview(empty)
        if historyCount == 0 {
            label("No captures yet", x: 240, y: 170, width: 650, size: "text-xl")
            return
        }
        let scroll = NSScrollView(frame: NSRect(x: 220, y: 130, width: 752, height: 550))
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        let table = NSTableView(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("capture"))
        column.width = 728
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 68
        table.backgroundColor = tokens.color("surface-canvas")
        table.selectionHighlightStyle = .none
        table.dataSource = self
        table.delegate = self
        table.setAccessibilityLabel("Capture history fixtures")
        scroll.documentView = table
        content.addSubview(scroll)
        self.table = table
    }

    func numberOfRows(in tableView: NSTableView) -> Int { historyCount }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("capture-row")
        let view = tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView ?? NSTableCellView()
        view.identifier = id
        view.subviews.forEach { $0.removeFromSuperview() }
        view.wantsLayer = true
        view.layer!.backgroundColor = tokens.color("surface-raised").cgColor
        view.layer!.cornerRadius = tokens.number("r-lg")
        let title = NSTextField(labelWithString: "\(row % 3 == 0 ? "Recording" : "Screenshot") \(row + 1)")
        title.frame = NSRect(x: 16, y: 32, width: 690, height: 22)
        title.font = .systemFont(ofSize: tokens.number("text-md"), weight: .medium)
        title.textColor = tokens.color("text")
        let subtitle = NSTextField(labelWithString: row % 3 == 0 ? "Video · 00:24 · fixture" : "1920 × 1080 · PNG · fixture")
        subtitle.frame = NSRect(x: 16, y: 10, width: 690, height: 20)
        subtitle.font = .systemFont(ofSize: tokens.number("text-sm"))
        subtitle.textColor = tokens.color("text-muted")
        view.addSubview(title)
        view.addSubview(subtitle)
        view.textField = title
        return view
    }

    private func hud() {
        let view = panel(NSRect(x: 320, y: 270, width: 550, height: 100), glass: true)
        label(paused ? "Paused · 00:24" : "Recording · 00:24", x: 24, y: 19, width: 250,
            size: "text-lg", glass: true, parent: view)
        label("Static timer fixture — no recording engine", x: 24, y: 53, width: 320,
            size: "text-sm", glass: true, parent: view)
        let button = CaptureButton(paused ? "Resume" : "Pause", frame: NSRect(x: 378, y: 30, width: 145, height: 36), tokens: tokens, glass: true) { [weak self] in
            guard let self else { return }
            self.paused.toggle()
            self.render()
        }
        view.addSubview(button)
    }

    private func previews() {
        do {
            let view = try PreviewView(frame: NSRect(x: 320, y: 80, width: 524, height: 550), atlas: !options.referenceChips)
            content.addSubview(view)
            preview = view
            for (index, name) in ["Cold dissolve", "Warm dissolve", "Reset"].enumerated() {
                content.addSubview(CaptureButton(name, frame: NSRect(x: 280 + index * 220, y: 645, width: 200, height: 34), tokens: tokens) { [weak view] in
                    view?.reset()
                    if index < 2 { view?.dissolve(cold: index == 0) }
                })
            }
        } catch {
            label("Preview unavailable: \(error)", x: 220, y: 140, width: 750)
            Metrics.emit("effect-error", milliseconds: 0, detail: String(describing: error))
        }
    }

    private func exercise(_ cycle: Int) {
        let started = CACurrentMediaTime()
        switch scene {
        case "preferences": preferencesController?.exerciseAppearance(cycle % 2 == 0 ? "light" : "dark")
        case "history": table?.scrollRowToVisible(cycle % 2 == 0 ? max(0, historyCount - 1) : 0)
        case "hud": paused.toggle(); render()
        case "preview": preview?.reset(); preview?.dissolve(cold: cycle % 2 == 0)
        case "region":
            if let view = regionSelector {
                switch cycle {
                case 0: view.begin(NSPoint(x: 100, y: 80)); view.drag(NSPoint(x: 520, y: 300)); view.end()
                case 1:
                    let center = NSPoint(x: view.selection.nsRect.midX, y: view.selection.nsRect.midY)
                    view.begin(center); view.drag(NSPoint(x: center.x + 42, y: center.y + 27)); view.end()
                case 2: view.setAspect(1)
                case 3:
                    let corner = view.selection.corners[3]
                    view.begin(corner); view.drag(NSPoint(x: corner.x + 120, y: corner.y + 70)); view.end()
                case 4: view.setAspect(16.0 / 9.0); view.begin(NSPoint(x: 800, y: 100)); view.drag(NSPoint(x: 500, y: 400), shift: true)
                default: view.drag(NSPoint(x: 500, y: 400)); view.end()
                }
            }
        case "window":
            windowSelector?.hover(NSPoint(x: cycle % 2 == 0 ? 180 : 900, y: 140))
        default: break
        }
        Metrics.emit("scripted-action", milliseconds: (CACurrentMediaTime() - started) * 1000,
            detail: "\(scene), cycle=\(cycle); CPU submission, not presentation latency")
    }
}
