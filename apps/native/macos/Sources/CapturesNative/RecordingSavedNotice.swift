import AppKit

enum RecordingSavedNoticeState: Equatable {
    case ready
    case saving
    case saved(path: String)
    case error(message: String, retry: RecordingSavedNoticeRetry)
}

enum RecordingSavedNoticeRetry: Equatable { case save, reveal(path: String) }

/// Owns the notice lifetime separately from the history artifact lifetime.
/// A monotonically increasing generation makes late transport and timer callbacks harmless.
final class RecordingSavedNoticeModel {
    private(set) var generation = 0
    private(set) var artifactID: String?
    private(set) var state: RecordingSavedNoticeState?
    var changed: () -> Void = {}

    @discardableResult func present(artifactID: String) -> Int {
        generation += 1; self.artifactID = artifactID; state = .ready; changed(); return generation
    }

    func beginSave() -> (generation: Int, artifactID: String)? {
        guard let artifactID, let state else { return nil }
        switch state {
        case .ready, .error(_, .save): break
        default: return nil
        }
        self.state = .saving; changed(); return (generation, artifactID)
    }

    func finishSave(generation: Int, result: Result<String, Error>) {
        guard generation == self.generation, state == .saving else { return }
        switch result {
        case .success(let path) where !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty:
            state = .saved(path: path)
        case .success:
            state = .error(message: "The save completed without a file path. Try again.", retry: .save)
        case .failure(let error):
            state = .error(message: error.localizedDescription, retry: .save)
        }
        changed()
    }

    @discardableResult func markSaved(artifactID: String, path: String) -> Bool {
        guard self.artifactID == artifactID else { return false }
        state = .saved(path: path); changed(); return true
    }

    func beginReveal() -> (generation: Int, path: String)? {
        guard let state else { return nil }
        switch state {
        case .saved(let path), .error(_, .reveal(let path)):
            return (generation, path)
        default: return nil
        }
    }

    func finishReveal(generation: Int, path: String, succeeded: Bool) {
        guard generation == self.generation else { return }
        if succeeded { dismiss() }
        else {
            state = .error(message: "Couldn’t show the saved recording in Finder.", retry: .reveal(path: path))
            changed()
        }
    }

    func expire(generation: Int) {
        if generation == self.generation, state != .saving { dismiss() }
    }
    func dismiss() { generation += 1; artifactID = nil; state = nil; changed() }
}

final class RecordingSavedNoticeView: NSView {
    private let tokens: Tokens
    private let icon = NSTextField(labelWithString: "●")
    private let heading = NSTextField(labelWithString: "")
    private let detail = NSTextField(wrappingLabelWithString: "")
    let primaryButton: CaptureButton
    let dismissButton: CaptureButton

    override var isFlipped: Bool { true }

    init(frame: NSRect, tokens: Tokens) {
        self.tokens = tokens
        primaryButton = CaptureButton("Save file", frame: NSRect(x: 246, y: 70, width: 104, height: 32), tokens: tokens, glass: true) {}
        dismissButton = CaptureButton("Dismiss", frame: NSRect(x: 354, y: 70, width: 70, height: 32), tokens: tokens, glass: true) {}
        super.init(frame: frame)
        wantsLayer = true; layer?.backgroundColor = tokens.color("glass-strong").cgColor
        layer?.cornerRadius = tokens.number("r-xl"); layer?.borderWidth = 1
        layer?.borderColor = tokens.color("glass-border").cgColor
        layer?.shadowColor = NSColor.black.cgColor; layer?.shadowOpacity = 0.4
        layer?.shadowRadius = 18; layer?.shadowOffset = NSSize(width: 0, height: -6)
        setAccessibilityRole(.group); setAccessibilityLabel("Recording ready")
        icon.frame = NSRect(x: 18, y: 17, width: 18, height: 20)
        icon.font = .systemFont(ofSize: 13, weight: .bold); addSubview(icon)
        heading.frame = NSRect(x: 42, y: 14, width: 360, height: 22)
        heading.font = .systemFont(ofSize: 15, weight: .semibold)
        heading.textColor = tokens.color("glass-text"); addSubview(heading)
        detail.frame = NSRect(x: 42, y: 39, width: 382, height: 29)
        detail.font = .systemFont(ofSize: 11, weight: .medium)
        detail.textColor = tokens.color("glass-text-subtle"); detail.maximumNumberOfLines = 2
        addSubview(detail)
        primaryButton.setAccessibilityLabel("Save recording file"); addSubview(primaryButton)
        dismissButton.setAccessibilityLabel("Dismiss recording notice"); addSubview(dismissButton)
        update(.ready)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func update(_ state: RecordingSavedNoticeState) {
        switch state {
        case .ready:
            heading.stringValue = "Recording ready"
            detail.stringValue = "Kept in Capture History for 30 days. Save a copy anytime."
            icon.textColor = tokens.color("positive")
            primaryButton.title = "Save file"; primaryButton.isEnabled = true
            primaryButton.setAccessibilityLabel("Save recording file")
        case .saving:
            heading.stringValue = "Saving recording…"; detail.stringValue = "Choosing your Captures folder and saving a copy."
            icon.textColor = tokens.color("theme-accent")
            primaryButton.title = "Saving…"; primaryButton.isEnabled = false
        case .saved:
            heading.stringValue = "Recording saved"; detail.stringValue = "Saved to your Captures folder."
            icon.textColor = tokens.color("positive")
            primaryButton.title = "Show in Folder"; primaryButton.isEnabled = true
            primaryButton.setAccessibilityLabel("Show saved recording in Folder")
        case .error(let message, let retry):
            heading.stringValue = retry == .save ? "Couldn’t save recording" : "Couldn’t show recording"
            detail.stringValue = message; icon.textColor = tokens.color("danger-text")
            primaryButton.title = retry == .save ? "Retry save" : "Retry reveal"
            primaryButton.isEnabled = true; primaryButton.setAccessibilityLabel(primaryButton.title)
        }
        heading.setAccessibilityLabel(heading.stringValue)
        detail.setAccessibilityLabel(detail.stringValue)
        detail.toolTip = detail.stringValue
        detail.textColor = tokens.color("glass-text")
        setAccessibilityLabel(heading.stringValue)
        needsDisplay = true
    }
}

final class RecordingSavedNoticePanel: NSPanel {
    let noticeView: RecordingSavedNoticeView
    override var canBecomeKey: Bool { true }

    init(screen: NSScreen, tokens: Tokens) {
        let size = NSSize(width: 440, height: 116), visible = screen.visibleFrame
        let origin = NSPoint(x: visible.maxX - size.width - 20, y: visible.maxY - size.height - 20)
        noticeView = RecordingSavedNoticeView(frame: NSRect(origin: .zero, size: size), tokens: tokens)
        super.init(contentRect: NSRect(origin: origin, size: size), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        title = "Recording ready"; isReleasedWhenClosed = false; isOpaque = false
        becomesKeyOnlyIfNeeded = true
        hidesOnDeactivate = false
        backgroundColor = .clear; hasShadow = false; level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        sharingType = .none; contentView = noticeView
    }
}

final class RecordingSavedNoticeController {
    static let lifetime: TimeInterval = 15.2
    /// Shipping `recording-saved-lifecycle` (15 s) ends 200 ms before the window closes.
    static let motion = "recording_saved_lifecycle"
    static let closeAfterAnimation: TimeInterval = 0.2
    let model = RecordingSavedNoticeModel()
    private let tokens: Tokens
    private(set) var panel: RecordingSavedNoticePanel?
    private var timer: Timer?
    private var exitTimer: Timer?
    var save: (String, @escaping (Result<String, Error>) -> Void) -> Void = { _, _ in }
    var reveal: (String) -> Bool = {
        FileManager.default.fileExists(atPath: $0)
            && NSWorkspace.shared.selectFile($0, inFileViewerRootedAtPath: "")
    }

    init(tokens: Tokens) { self.tokens = tokens; model.changed = { [weak self] in self?.render() } }

    func present(artifactID: String, screen: NSScreen) {
        timer?.invalidate(); exitTimer?.invalidate(); exitTimer = nil; panel?.close()
        let panel = RecordingSavedNoticePanel(screen: screen, tokens: tokens)
        self.panel = panel
        panel.noticeView.primaryButton.actionBlock = { [weak self] in self?.performPrimaryAction() }
        panel.noticeView.dismissButton.actionBlock = { [weak self] in self?.dismiss() }
        _ = model.present(artifactID: artifactID); panel.orderFrontRegardless(); armExpiry()
        NativeMotion.playEntrance(Self.motion, on: panel.noticeView, tokens: tokens)
    }

    func dismiss() { timer?.invalidate(); timer = nil; exitTimer?.invalidate(); exitTimer = nil; model.dismiss() }

    func savedFromHistory(artifactID: String, path: String) {
        if model.markSaved(artifactID: artifactID, path: path) { armExpiry() }
    }

    private func performPrimaryAction() {
        if let request = model.beginSave() {
            timer?.invalidate(); timer = nil
            save(request.artifactID) { [weak self] result in
                guard let self else { return }
                self.model.finishSave(generation: request.generation, result: result)
                if self.model.generation == request.generation, self.model.state != nil {
                    self.armExpiry()
                }
            }
            return
        }
        guard let request = model.beginReveal() else { return }
        let succeeded = reveal(request.path)
        model.finishReveal(generation: request.generation, path: request.path, succeeded: succeeded)
        if !succeeded { armExpiry() }
    }

    private func armExpiry() {
        timer?.invalidate()
        exitTimer?.invalidate(); exitTimer = nil
        // A save or error restarts the life: hold steady rather than replay the entrance.
        if let view = panel?.noticeView, NativeMotion.isPlaying(on: view) { NativeMotion.cancel(on: view) }
        if model.state == .saving { return }
        let generation = model.generation
        timer = Timer.scheduledTimer(withTimeInterval: Self.lifetime, repeats: false) { [weak self] _ in
            self?.model.expire(generation: generation)
        }
        let exitAt = Self.lifetime - Self.closeAfterAnimation - NativeMotion.exitDuration(Self.motion, tokens: tokens)
        guard !NativeMotion.reduceMotion, exitAt > 0 else { return }
        exitTimer = Timer.scheduledTimer(withTimeInterval: exitAt, repeats: false) { [weak self] _ in
            guard let self, self.model.generation == generation, let view = self.panel?.noticeView else { return }
            NativeMotion.playExit(Self.motion, on: view, tokens: self.tokens)
        }
    }

    private func render() {
        guard let state = model.state else {
            timer?.invalidate(); timer = nil; exitTimer?.invalidate(); exitTimer = nil
            panel?.close(); panel = nil; return
        }
        panel?.noticeView.update(state)
    }

    deinit { timer?.invalidate(); exitTimer?.invalidate(); panel?.close(); model.dismiss() }
}
