import AppKit

/// Preserve the live profile and all queued Apple-event/instance media, without
/// replaying fixture deadlines, screenshot flags or the original media twice.
func permissionRestartArguments(options: Options, pendingMedia: [String]) -> [String] {
    var arguments = ["--live", "--scene", "preferences"]
    if let path = options.historyRoot { arguments += ["--history-root", path] }
    if let path = options.settingsFile { arguments += ["--settings-file", path] }
    if options.appearanceOverride { arguments += ["--appearance", options.appearance] }
    if options.themeOverride { arguments += ["--theme", options.theme] }
    if !pendingMedia.isEmpty { arguments += ["--"] + pendingMedia }
    return arguments
}

/// State-independent setup copy from `captures_app::onboarding` (shared
/// with the wgpu host and identical to the shipping Tauri window).
struct OnboardingCopy: Equatable {
    let eyebrow, title, lede, checking, screenTitle, microphoneTitle, optional: String
    let opening, restarting, finishing, refresh, start: String
    let recoveryTitle, recoveryLede, recoveryDone: String

    static let current: OnboardingCopy = {
        do { return try OnboardingCopy(transport: SettingsBridge()) }
        catch { preconditionFailure("Setup copy is unavailable: \(error.localizedDescription)") }
    }()

    init(transport: SettingsTransport) throws {
        guard let copy = try transport.request(["operation": "onboarding_copy"])["copy"] as? [String: Any]
        else { throw SettingsStoreError.invalidResponse }
        func text(_ key: String) throws -> String {
            guard let value = copy[key] as? String else { throw SettingsStoreError.invalidResponse }
            return value
        }
        eyebrow = try text("eyebrow"); title = try text("title"); lede = try text("lede")
        checking = try text("checking"); screenTitle = try text("screen_title")
        microphoneTitle = try text("microphone_title"); optional = try text("optional")
        opening = try text("opening"); restarting = try text("restarting")
        finishing = try text("finishing"); refresh = try text("refresh"); start = try text("start")
        recoveryTitle = try text("recovery_title"); recoveryLede = try text("recovery_lede")
        recoveryDone = try text("recovery_done")
    }
}

/// "Granted ✓" / "Ready ✓" (`ready`) or a neutral "Restart required" / "Still off".
struct OnboardingStatus: Equatable {
    let label: String
    let ready: Bool

    init?(_ value: Any?) throws {
        guard let value, !(value is NSNull) else { return nil }
        guard let object = value as? [String: Any], let label = object["label"] as? String,
              let ready = object["ready"] as? Bool else { throw SettingsStoreError.invalidResponse }
        self.label = label; self.ready = ready
    }
}

/// Per-state copy and decisions derived in shared Rust (`State::presentation`).
struct OnboardingPresentation: Equatable {
    let title: String
    let screenDescription: String
    let screenStatus: OnboardingStatus?
    let screenAction: String?
    let showMicrophone: Bool
    let microphoneDescription: String
    let microphoneStatus: OnboardingStatus?
    let microphoneAction: String?
    let screenReady: Bool
    let restartRequired: Bool
    let primaryLabel: String

    init(_ value: [String: Any]) throws {
        guard let title = value["title"] as? String,
              let screenDescription = value["screen_description"] as? String,
              let showMicrophone = value["show_microphone"] as? Bool,
              let microphoneDescription = value["microphone_description"] as? String,
              let screenReady = value["screen_ready"] as? Bool,
              let restartRequired = value["restart_required"] as? Bool,
              let primaryLabel = value["primary_label"] as? String else {
            throw SettingsStoreError.invalidResponse
        }
        func optionalText(_ key: String) throws -> String? {
            guard let raw = value[key], !(raw is NSNull) else { return nil }
            guard let text = raw as? String else { throw SettingsStoreError.invalidResponse }
            return text
        }
        self.title = title; self.screenDescription = screenDescription
        self.screenStatus = try OnboardingStatus(value["screen_status"])
        self.screenAction = try optionalText("screen_action")
        self.showMicrophone = showMicrophone; self.microphoneDescription = microphoneDescription
        self.microphoneStatus = try OnboardingStatus(value["microphone_status"])
        self.microphoneAction = try optionalText("microphone_action")
        self.screenReady = screenReady; self.restartRequired = restartRequired
        self.primaryLabel = primaryLabel
    }
}

struct OnboardingState: Equatable {
    let platform: String
    let completed: Bool
    let screenRequired: Bool
    let screenGranted: Bool
    let screenCanRequest: Bool
    let screenRequestedThisLaunch: Bool
    let microphoneGranted: Bool
    let microphoneCanRequest: Bool
    let microphoneRequestedThisLaunch: Bool
    let presentation: OnboardingPresentation

    init(_ value: [String: Any]) throws {
        guard let platform = value["platform"] as? String,
              let completed = value["onboarding_completed"] as? Bool,
              let screenRequired = value["screen_recording_required"] as? Bool,
              let screenGranted = value["screen_recording_granted"] as? Bool,
              let screenCanRequest = value["screen_recording_can_request"] as? Bool,
              let screenRequested = value["screen_recording_requested_this_launch"] as? Bool,
              let microphoneGranted = value["microphone_granted"] as? Bool,
              let microphoneCanRequest = value["microphone_can_request"] as? Bool,
              let presentation = value["presentation"] as? [String: Any] else {
            throw SettingsStoreError.invalidResponse
        }
        self.platform = platform; self.completed = completed
        self.screenRequired = screenRequired; self.screenGranted = screenGranted
        self.screenCanRequest = screenCanRequest
        self.screenRequestedThisLaunch = screenRequested
        self.microphoneGranted = microphoneGranted
        self.microphoneCanRequest = microphoneCanRequest
        self.microphoneRequestedThisLaunch = value["microphone_requested_this_launch"] as? Bool ?? false
        self.presentation = try OnboardingPresentation(presentation)
    }
}

final class OnboardingController {
    private let store: SettingsStore
    private(set) var state: OnboardingState?
    private(set) var busy = false
    /// The in-flight action ("check", "request_screen", …) for busy labels.
    private(set) var pendingAction: String?
    private(set) var error: String?
    var changed: () -> Void = {}
    var completed: () -> Void = {}
    var requiresAttention: () -> Void = {}

    init(store: SettingsStore) { self.store = store }

    func check() { request("check") }
    func requestScreen() { request("request_screen") }
    func requestMicrophone() { request("request_microphone") }
    func complete() { request("complete") }
    func flush() { store.flush() }

    private func request(_ action: String) {
        guard !busy else { return }
        busy = true; pendingAction = action; error = nil; changed()
        store.onboarding(action) { [weak self] result in
            guard let self else { return }
            self.busy = false; self.pendingAction = nil
            switch result {
            case .success(let state):
                self.state = state
                if state.completed { self.completed() }
                else { self.requiresAttention() }
            case .failure(let error):
                self.error = error.localizedDescription
                self.requiresAttention()
            }
            self.changed()
        }
    }
}

/// Shipping setup glyphs in their 24-unit grid (`Onboarding.tsx`).
final class OnboardingGlyphView: NSView {
    enum Glyph { case appMark, screen, microphone }
    override var isFlipped: Bool { true }
    private let glyph: Glyph
    var tokens: Tokens { didSet { needsDisplay = true } }

    init(_ glyph: Glyph, tokens: Tokens) {
        self.glyph = glyph; self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { nil }

    override func draw(_ dirtyRect: NSRect) {
        let mark = glyph == .appMark
        let side: CGFloat = mark ? 24 : 20
        let scale = side / 24
        let origin = NSPoint(x: bounds.midX - side / 2, y: bounds.midY - side / 2)
        func p(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            NSPoint(x: origin.x + x * scale, y: origin.y + y * scale)
        }
        let ink = tokens.color(mark ? "theme-accent-ink" : "text-subtle")
        let path = NSBezierPath()
        path.lineWidth = 1.7; path.lineCapStyle = .round; path.lineJoinStyle = .round
        func line(_ points: [(CGFloat, CGFloat)]) {
            path.move(to: p(points[0].0, points[0].1))
            points.dropFirst().forEach { path.line(to: p($0.0, $0.1)) }
        }
        switch glyph {
        case .appMark:
            let radius = tokens.number("r-lg")
            tokens.color("theme-accent").setFill()
            NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
            line([(9, 4), (4, 4), (4, 9)]); line([(15, 4), (20, 4), (20, 9)])
            line([(20, 15), (20, 20), (15, 20)]); line([(9, 20), (4, 20), (4, 15)])
            let spark = NSBezierPath()
            let c = p(12, 12)
            let points: [(CGFloat, CGFloat)] = [(0, -3.5), (0.9, -0.9), (3.5, 0), (0.9, 0.9),
                                                (0, 3.5), (-0.9, 0.9), (-3.5, 0), (-0.9, -0.9)]
            spark.move(to: NSPoint(x: c.x + points[0].0, y: c.y + points[0].1))
            points.dropFirst().forEach { spark.line(to: NSPoint(x: c.x + $0.0, y: c.y + $0.1)) }
            spark.close()
            ink.setFill(); spark.fill()
        case .screen:
            path.appendRoundedRect(NSRect(x: p(3, 4).x, y: p(3, 4).y, width: 18 * scale, height: 13 * scale),
                                   xRadius: 2.5 * scale, yRadius: 2.5 * scale)
            line([(8, 21), (16, 21)]); line([(12, 17), (12, 21)])
        case .microphone:
            path.appendRoundedRect(NSRect(x: p(9, 3).x, y: p(9, 3).y, width: 6 * scale, height: 11 * scale),
                                   xRadius: 3 * scale, yRadius: 3 * scale)
            // Lower half circle (y grows downward in this flipped view).
            path.move(to: p(6, 11))
            path.appendArc(withCenter: p(12, 11), radius: 6 * scale, startAngle: 180, endAngle: 0,
                           clockwise: true)
            line([(12, 17), (12, 21)]); line([(9, 21), (15, 21)])
        }
        ink.setStroke(); path.stroke()
    }
}

/// `.onboarding-permission-status`: label plus a check mark when ready.
final class OnboardingStatusView: NSView {
    override var isFlipped: Bool { true }
    var tokens: Tokens { didSet { needsDisplay = true } }
    var status: OnboardingStatus? {
        didSet {
            isHidden = status == nil
            setAccessibilityLabel(status.map { $0.ready ? "\($0.label) ✓" : $0.label })
            needsDisplay = true
        }
    }

    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        setAccessibilityElement(true); setAccessibilityRole(.staticText)
        isHidden = true
    }
    required init?(coder: NSCoder) { nil }

    private var attributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium),
         .foregroundColor: tokens.color(status?.ready == true ? "positive-text" : "text-subtle")]
    }
    override var intrinsicContentSize: NSSize {
        guard let status else { return .zero }
        let text = (status.label as NSString).size(withAttributes: attributes)
        return NSSize(width: ceil(text.width) + (status.ready ? 14 + tokens.number("s-2") : 0),
                      height: ceil(text.height))
    }
    override func draw(_ dirtyRect: NSRect) {
        guard let status else { return }
        let size = (status.label as NSString).size(withAttributes: attributes)
        (status.label as NSString).draw(at: NSPoint(x: 0, y: (bounds.height - size.height) / 2),
                                        withAttributes: attributes)
        guard status.ready else { return }
        let box = NSRect(x: bounds.maxX - 14, y: bounds.midY - 7, width: 14, height: 14)
        let check = NSBezierPath()
        check.lineWidth = 2; check.lineCapStyle = .round; check.lineJoinStyle = .round
        func q(_ x: CGFloat, _ y: CGFloat) -> NSPoint { NSPoint(x: box.minX + x * 14 / 16, y: box.minY + y * 14 / 16) }
        check.move(to: q(3.2, 8.2)); check.line(to: q(6.4, 11.4)); check.line(to: q(12.8, 4.6))
        tokens.color("positive-text").setStroke(); check.stroke()
    }
}

/// One `.onboarding-permission` row: icon, title (+ Optional), description,
/// and a right-aligned status pill and/or action button.
final class OnboardingPermissionRow: NSView {
    override var isFlipped: Bool { true }
    private var tokens: Tokens
    private let icon: OnboardingGlyphView
    let titleLabel: NSTextField
    private let optionalLabel: NSTextField?
    let detail = NSTextField(wrappingLabelWithString: "")
    let status: OnboardingStatusView
    let button: CaptureButton
    var onAction: () -> Void = {}

    init(glyph: OnboardingGlyphView.Glyph, title: String, optional: String?, tokens: Tokens) {
        self.tokens = tokens
        icon = OnboardingGlyphView(glyph, tokens: tokens)
        titleLabel = NSTextField(labelWithString: title)
        optionalLabel = optional.map { NSTextField(labelWithString: $0) }
        status = OnboardingStatusView(tokens: tokens)
        button = CaptureButton("", frame: .zero, tokens: tokens) {}
        super.init(frame: .zero)
        button.actionBlock = { [weak self] in self?.onAction() }
        titleLabel.font = .systemFont(ofSize: tokens.number("text-md"), weight: .medium)
        titleLabel.textColor = tokens.color("text")
        optionalLabel?.font = .systemFont(ofSize: tokens.number("text-xs"), weight: .medium)
        optionalLabel?.textColor = tokens.color("text-faint")
        detail.font = .systemFont(ofSize: tokens.number("text-sm"))
        detail.textColor = tokens.color("text-subtle")
        var views: [NSView] = [icon, titleLabel, detail, status, button]
        if let optionalLabel { views.append(optionalLabel) }
        views.forEach(addSubview)
    }
    required init?(coder: NSCoder) { nil }

    func configure(description: String, status: OnboardingStatus?, action: String?, enabled: Bool) {
        detail.stringValue = description
        self.status.status = status
        button.isHidden = action == nil
        if let action {
            button.title = action; button.setAccessibilityLabel(action)
        }
        button.isEnabled = enabled
        button.needsDisplay = true
        needsLayout = true
    }

    private var padding: CGFloat { tokens.number("s-6") }
    private var gap: CGFloat { tokens.number("s-5") }
    private var actionsWidth: CGFloat {
        let buttonWidth = button.isHidden ? 0
            : ceil((button.title as NSString).size(withAttributes:
                [.font: NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .medium)]).width)
                + 2 * tokens.number("s-5")
        return max(118, buttonWidth, status.isHidden ? 0 : status.intrinsicContentSize.width)
    }
    private func copyWidth(_ width: CGFloat) -> CGFloat {
        max(120, width - 2 * padding - 22 - 2 * gap - actionsWidth)
    }
    private func detailHeight(_ width: CGFloat) -> CGFloat {
        ceil(detail.cell?.cellSize(forBounds: NSRect(x: 0, y: 0, width: width,
            height: .greatestFiniteMagnitude)).height ?? 0)
    }
    private var titleHeight: CGFloat { ceil(titleLabel.intrinsicContentSize.height) }
    private var actionsHeight: CGFloat {
        let statusHeight = status.isHidden ? 0 : status.intrinsicContentSize.height
        let buttonHeight = button.isHidden ? 0 : tokens.number("h-md")
        return statusHeight + buttonHeight
            + (status.isHidden || button.isHidden ? 0 : tokens.number("s-3"))
    }
    func height(forWidth width: CGFloat) -> CGFloat {
        let text = titleHeight + tokens.number("s-2") + detailHeight(copyWidth(width))
        return 2 * padding + max(text, actionsHeight, 22)
    }
    override func layout() {
        super.layout()
        let width = copyWidth(bounds.width)
        icon.frame = NSRect(x: padding, y: padding + 1, width: 22, height: 22)
        let x = padding + 22 + gap
        let titleWidth = ceil(titleLabel.intrinsicContentSize.width)
        titleLabel.frame = NSRect(x: x, y: padding, width: titleWidth, height: titleHeight)
        if let optionalLabel {
            let size = optionalLabel.intrinsicContentSize
            optionalLabel.frame = NSRect(x: x + titleWidth + tokens.number("s-4"),
                y: padding + titleHeight - ceil(size.height) - 1, width: ceil(size.width), height: ceil(size.height))
        }
        detail.preferredMaxLayoutWidth = width
        detail.frame = NSRect(x: x, y: padding + titleHeight + tokens.number("s-2"),
                              width: width, height: detailHeight(width))
        let right = bounds.width - padding
        var y = padding
        if !status.isHidden {
            let size = status.intrinsicContentSize
            status.frame = NSRect(x: right - size.width, y: y, width: size.width,
                                  height: max(size.height, titleHeight))
            y = status.frame.maxY + tokens.number("s-3")
        }
        if !button.isHidden {
            let w = actionsWidth
            button.frame = NSRect(x: right - w, y: y, width: w, height: tokens.number("h-md"))
        }
    }
}

/// `.onboarding-permissions`: raised card holding the rows with hairlines.
final class OnboardingCardsView: NSView {
    override var isFlipped: Bool { true }
    var tokens: Tokens
    init(tokens: Tokens) {
        self.tokens = tokens
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = tokens.number("r-xl")
        layer?.borderWidth = 1
        layer?.masksToBounds = true
        restyle()
    }
    required init?(coder: NSCoder) { nil }
    func restyle() {
        layer?.backgroundColor = tokens.color("surface-raised").cgColor
        layer?.borderColor = tokens.color("border-subtle").cgColor
    }
    override func draw(_ dirtyRect: NSRect) {
        tokens.color("border-subtle").setFill()
        for row in subviews.dropFirst() where !row.isHidden {
            NSRect(x: 0, y: row.frame.minY, width: bounds.width, height: 1).fill()
        }
    }
}

/// First-run setup (and, with `done`, permission recovery) laid out like the
/// shipping Tauri window: app mark, eyebrow, title, lede, permission cards,
/// and a right-aligned action row with Refresh status as the secondary action.
final class OnboardingView: NSView {
    override var isFlipped: Bool { true }
    private let controller: OnboardingController
    private let tokens: Tokens
    private let done: (() -> Void)?
    private let strings = OnboardingCopy.current
    private let mark: OnboardingGlyphView
    private let eyebrow: NSTextField
    private let title = NSTextField(wrappingLabelWithString: "")
    private let lede = NSTextField(wrappingLabelWithString: "")
    private let cards: OnboardingCardsView
    let screenRow: OnboardingPermissionRow
    let microphoneRow: OnboardingPermissionRow
    private let errorBox = NSView()
    private let errorLabel = NSTextField(wrappingLabelWithString: "")
    let refreshButton: CaptureButton
    let primaryButton: CaptureButton
    private var restarting = false
    var restartRequested: () -> Void = {}

    init(frame: NSRect, tokens: Tokens, controller: OnboardingController,
         done: (() -> Void)? = nil) {
        self.tokens = tokens; self.controller = controller; self.done = done
        mark = OnboardingGlyphView(.appMark, tokens: tokens)
        eyebrow = NSTextField(labelWithString: OnboardingCopy.current.eyebrow.uppercased())
        cards = OnboardingCardsView(tokens: tokens)
        screenRow = OnboardingPermissionRow(glyph: .screen, title: OnboardingCopy.current.screenTitle,
                                            optional: nil, tokens: tokens)
        microphoneRow = OnboardingPermissionRow(glyph: .microphone,
            title: OnboardingCopy.current.microphoneTitle, optional: OnboardingCopy.current.optional,
            tokens: tokens)
        refreshButton = CaptureButton(OnboardingCopy.current.refresh, frame: .zero, tokens: tokens) {}
        primaryButton = CaptureButton(OnboardingCopy.current.start, frame: .zero, tokens: tokens) {}
        super.init(frame: frame)
        wantsLayer = true; layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        refreshButton.actionBlock = { [weak self] in self?.controller.check() }
        primaryButton.actionBlock = { [weak self] in self?.primary() }
        primaryButton.solid = true
        eyebrow.font = .systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold)
        eyebrow.textColor = tokens.color("text-subtle")
        eyebrow.attributedStringValue = NSAttributedString(string: eyebrow.stringValue, attributes: [
            .kern: 0.6, .font: NSFont.systemFont(ofSize: tokens.number("text-2xs"), weight: .semibold),
            .foregroundColor: tokens.color("text-subtle")])
        title.font = .systemFont(ofSize: tokens.number("text-2xl"), weight: .semibold)
        title.textColor = tokens.color("text")
        title.setAccessibilityRole(.staticText)
        lede.font = .systemFont(ofSize: tokens.number("text-md"))
        lede.textColor = tokens.color("text-subtle")
        errorBox.wantsLayer = true
        errorBox.layer?.backgroundColor = tokens.color("danger-surface").cgColor
        errorBox.layer?.borderColor = tokens.color("danger-border").cgColor
        errorBox.layer?.borderWidth = 1
        errorBox.layer?.cornerRadius = tokens.number("r-md")
        errorLabel.font = .systemFont(ofSize: tokens.number("text-sm"))
        errorLabel.textColor = tokens.color("danger-text")
        errorBox.addSubview(errorLabel)
        screenRow.onAction = { [weak self] in self?.controller.requestScreen() }
        microphoneRow.onAction = { [weak self] in self?.controller.requestMicrophone() }
        cards.addSubview(screenRow); cards.addSubview(microphoneRow)
        if done != nil {
            title.stringValue = strings.recoveryTitle; lede.stringValue = strings.recoveryLede
        } else {
            title.stringValue = strings.title; lede.stringValue = strings.lede
            addSubview(mark); addSubview(eyebrow)
        }
        let views: [NSView] = [title, lede, cards, errorBox, refreshButton, primaryButton]
        views.forEach(addSubview)
        controller.changed = { [weak self] in self?.update() }
        needsLayout = true
        update()
    }
    required init?(coder: NSCoder) { nil }

    private var busyAction: String? { controller.busy ? controller.pendingAction ?? "check" : nil }

    func update() {
        let view = controller.state?.presentation
        let busy = controller.busy || restarting
        if done == nil, let view { title.stringValue = view.title }
        screenRow.configure(description: view?.screenDescription ?? strings.checking,
            status: view?.screenStatus,
            action: view?.screenAction.map { busyAction == "request_screen" ? strings.opening : $0 },
            enabled: !busy)
        microphoneRow.isHidden = view?.showMicrophone != true
        if let view {
            microphoneRow.configure(description: view.microphoneDescription,
                status: view.microphoneStatus,
                action: view.microphoneAction.map { busyAction == "request_microphone" ? strings.opening : $0 },
                enabled: !busy)
        }
        errorLabel.stringValue = controller.error ?? ""
        errorBox.isHidden = controller.error == nil
        refreshButton.isEnabled = !busy
        if done != nil {
            primaryButton.title = strings.recoveryDone
            primaryButton.isEnabled = !busy
        } else if restarting {
            primaryButton.title = strings.restarting
            primaryButton.isEnabled = false
        } else if busyAction == "complete" {
            primaryButton.title = strings.finishing
            primaryButton.isEnabled = false
        } else {
            primaryButton.title = view?.primaryLabel ?? strings.start
            primaryButton.isEnabled = !busy && (view?.screenReady == true || view?.restartRequired == true)
        }
        primaryButton.setAccessibilityLabel(primaryButton.title)
        refreshButton.needsDisplay = true; primaryButton.needsDisplay = true
        needsLayout = true
        needsDisplay = true
    }

    private func primary() {
        if let done { done(); return }
        if controller.state?.presentation.restartRequired == true {
            restarting = true; update()
            restartRequested()
        } else {
            controller.complete()
        }
    }

    /// Clears a restart that the host could not perform.
    func restartFailed() { restarting = false; update() }

    private func wrappedHeight(_ field: NSTextField, _ width: CGFloat) -> CGFloat {
        ceil(field.cell?.cellSize(forBounds: NSRect(x: 0, y: 0, width: width,
            height: .greatestFiniteMagnitude)).height ?? 0)
    }

    override func layout() {
        super.layout()
        let inset = tokens.number("s-8")
        let width = min(620, max(0, bounds.width - 2 * inset))
        let x = (bounds.width - width) / 2
        let gap = tokens.number("s-3")
        var items: [(NSView, CGFloat, CGFloat)] = [] // view, height, gap after
        if done == nil {
            items.append((mark as NSView, 40, tokens.number("s-2") + gap))
            items.append((eyebrow as NSView, ceil(eyebrow.intrinsicContentSize.height), gap))
        }
        title.preferredMaxLayoutWidth = width; lede.preferredMaxLayoutWidth = width
        items.append((title as NSView, wrappedHeight(title, width), gap))
        items.append((lede as NSView, wrappedHeight(lede, width), tokens.number("s-7")))
        let screenHeight = screenRow.height(forWidth: width)
        let microphoneHeight = microphoneRow.isHidden ? 0 : microphoneRow.height(forWidth: width)
        items.append((cards as NSView, screenHeight + microphoneHeight, tokens.number("s-5")))
        if !errorBox.isHidden {
            let labelWidth = width - 2 * tokens.number("s-5")
            errorLabel.preferredMaxLayoutWidth = labelWidth
            let labelHeight = wrappedHeight(errorLabel, labelWidth)
            errorLabel.frame = NSRect(x: tokens.number("s-5"), y: tokens.number("s-4"),
                                      width: labelWidth, height: labelHeight)
            items.append((errorBox as NSView, labelHeight + 2 * tokens.number("s-4"), tokens.number("s-5")))
        }
        let actionsHeight = tokens.number("h-xl")
        let total = items.reduce(actionsHeight) { $0 + $1.1 + $1.2 }
        var y = max(tokens.number(done == nil ? "s-9" : "s-7"), (bounds.height - total) / 2)
        for (view, height, after) in items {
            let w = view === mark ? 40 : view === eyebrow ? ceil(eyebrow.intrinsicContentSize.width) : width
            view.frame = NSRect(x: x, y: y, width: w, height: height)
            y += height + after
        }
        screenRow.frame = NSRect(x: 0, y: 0, width: width, height: screenHeight)
        microphoneRow.frame = NSRect(x: 0, y: screenHeight, width: width, height: microphoneHeight)
        cards.needsDisplay = true
        let font = NSFont.systemFont(ofSize: tokens.number("text-md"), weight: .semibold)
        func buttonWidth(_ title: String, minimum: CGFloat) -> CGFloat {
            max(minimum, ceil((title as NSString).size(withAttributes: [.font: font]).width) + 2 * tokens.number("s-6"))
        }
        let primaryWidth = buttonWidth(primaryButton.title, minimum: 148)
        primaryButton.frame = NSRect(x: x + width - primaryWidth, y: y, width: primaryWidth, height: actionsHeight)
        let refreshWidth = buttonWidth(refreshButton.title, minimum: 0)
        refreshButton.frame = NSRect(x: primaryButton.frame.minX - tokens.number("s-4") - refreshWidth, y: y,
                                     width: refreshWidth, height: actionsHeight)
    }

    /// A static `--surface-active` halo marks the ready primary action in
    /// place of the shipping CTA pulse, so there is no motion to reduce.
    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard done == nil, primaryButton.isEnabled, controller.state?.presentation.screenReady == true
        else { return }
        let radius = tokens.number("r-md") + 3
        let halo = NSBezierPath(roundedRect: primaryButton.frame.insetBy(dx: -2, dy: -2),
                                xRadius: radius, yRadius: radius)
        halo.lineWidth = 3
        tokens.color("surface-active").setStroke(); halo.stroke()
    }
}
