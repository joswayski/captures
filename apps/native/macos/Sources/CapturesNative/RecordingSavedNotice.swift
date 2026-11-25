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

/// The Recording ready notice: the launch notice's compact solid glass card
/// and shadow inside a transparent frame, one row of a positive check badge,
/// the copy, the action button and a round × like the launch notice's Close.
final class RecordingSavedNoticeView: NSView {
    /// The card, sized like the launch notice so the floating notices match.
    static let card = NSSize(width: 420, height: 68)
    /// Transparent room so the card's shadow is never clipped by the window.
    static let pad: CGFloat = 28
    static let size = NSSize(width: card.width + pad * 2, height: card.height + pad * 2)

    private let tokens: Tokens
    let tile: ShippingIconTile
    private let heading = NSTextField(labelWithString: "")
    private let detail = NSTextField(wrappingLabelWithString: "")
    let primaryButton: CaptureButton
    let dismissButton: CaptureButton

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    var cardRect: NSRect { bounds.insetBy(dx: Self.pad, dy: Self.pad) }

    init(frame: NSRect, tokens: Tokens) {
        self.tokens = tokens
        let card = frame.insetBy(dx: Self.pad, dy: Self.pad).offsetBy(dx: -frame.minX, dy: -frame.minY)
        tile = ShippingIconTile(frame: NSRect(x: card.minX + 1 + tokens.number("s-5"), y: card.midY - 12,
            width: 24, height: 24), tokens: tokens, glyph: "check", fill: "positive", ink: "positive-ink",
            glyphSide: 14, strokeUnits: 2.4)
        tile.layer?.cornerRadius = 12
        let side = tokens.number("h-sm")
        primaryButton = CaptureButton("Save file", frame: NSRect(x: 0, y: 0, width: 0, height: side),
            tokens: tokens, glass: true) {}
        dismissButton = CaptureButton("Dismiss", frame: NSRect(x: card.maxX - tokens.number("s-3") - side,
            y: card.midY - side / 2, width: side, height: side), tokens: tokens, glass: true) {}
        super.init(frame: frame)
        wantsLayer = true; layer?.backgroundColor = NSColor.clear.cgColor
        setAccessibilityRole(.group); setAccessibilityLabel("Recording ready")
        addSubview(tile)
        heading.font = .capturesUI(ofSize: tokens.number("text-md"), weight: .semibold)
        heading.textColor = tokens.color("glass-text"); heading.lineBreakMode = .byTruncatingTail
        addSubview(heading)
        detail.font = .capturesUI(ofSize: tokens.number("text-sm"))
        detail.textColor = tokens.color("glass-text-muted"); detail.maximumNumberOfLines = 2
        detail.lineBreakMode = .byWordWrapping; detail.cell?.truncatesLastVisibleLine = true
        addSubview(detail)
        primaryButton.textSize = tokens.number("text-sm"); primaryButton.iconSide = 14
        primaryButton.setAccessibilityLabel("Save recording file"); addSubview(primaryButton)
        dismissButton.hudControl = true; dismissButton.iconOnly = true; dismissButton.circular = true
        dismissButton.icon = .shipping("close"); dismissButton.iconSide = 14
        dismissButton.setAccessibilityLabel("Dismiss recording notice"); addSubview(dismissButton)
        update(.ready)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        let card = cardRect, radius = tokens.number("r-lg")
        let path = NSBezierPath(roundedRect: card, xRadius: radius, yRadius: radius)
        NSGraphicsContext.saveGraphicsState()
        // `--tooltip-shadow-compact`, as the launch notice draws it.
        let layer = tokens.shadow("tooltip-shadow-compact")[0], shadow = NSShadow()
        shadow.shadowColor = layer.nsColor
        shadow.shadowBlurRadius = CGFloat(layer.blur / 2)
        shadow.shadowOffset = NSSize(width: layer.x, height: -layer.y)
        shadow.set()
        tokens.color("glass-strong-solid").setFill(); path.fill()
        NSGraphicsContext.restoreGraphicsState()
        let border = NSBezierPath(roundedRect: card.insetBy(dx: 0.5, dy: 0.5),
            xRadius: radius - 0.5, yRadius: radius - 0.5)
        tokens.color("glass-border").setStroke(); border.lineWidth = 1; border.stroke()
    }

    func update(_ state: RecordingSavedNoticeState) {
        switch state {
        case .ready:
            heading.stringValue = "Recording ready"
            detail.stringValue = "Kept in History for 30 days."
            primaryButton.title = "Save file"; primaryButton.isEnabled = true
            primaryButton.icon = .shipping("save")
            primaryButton.setAccessibilityLabel("Save recording file")
        case .saving:
            heading.stringValue = "Saving recording…"; detail.stringValue = "Saving a copy to your Captures folder."
            primaryButton.title = "Saving…"; primaryButton.isEnabled = false
            primaryButton.icon = .shipping("save")
        case .saved:
            heading.stringValue = "Recording saved"; detail.stringValue = "Saved to your Captures folder."
            primaryButton.title = "Show in Folder"; primaryButton.isEnabled = true
            primaryButton.icon = .shipping("folder")
            primaryButton.setAccessibilityLabel("Show saved recording in Folder")
        case .error(let message, let retry):
            heading.stringValue = retry == .save ? "Couldn’t save recording" : "Couldn’t show recording"
            detail.stringValue = message
            primaryButton.title = retry == .save ? "Retry save" : "Retry reveal"
            primaryButton.icon = .shipping(retry == .save ? "save" : "folder")
            primaryButton.isEnabled = true; primaryButton.setAccessibilityLabel(primaryButton.title)
        }
        heading.setAccessibilityLabel(heading.stringValue)
        detail.setAccessibilityLabel(detail.stringValue)
        detail.toolTip = detail.stringValue
        setAccessibilityLabel(heading.stringValue)
        layoutRow()
        needsDisplay = true
    }

    /// One row, sized from the measured button label and copy so any system
    /// font keeps the badge, copy, button and × apart inside the card.
    private func layoutRow() {
        let card = cardRect, gap = tokens.number("s-5")
        let labelWidth = ceil((primaryButton.title as NSString).size(withAttributes:
            [.font: NSFont.capturesUI(ofSize: tokens.number("text-sm"), weight: .medium)]).width)
        // `padding: 0 var(--s-4)`, the icon, `gap: var(--s-3)` and a 1 pt border.
        let buttonWidth = tokens.number("s-4") * 2 + 14 + tokens.number("s-3") + labelWidth + 2
        let height = tokens.number("h-sm")
        primaryButton.frame = NSRect(x: dismissButton.frame.minX - tokens.number("s-2") - buttonWidth,
            y: card.midY - height / 2, width: buttonWidth, height: height)
        let left = tile.frame.maxX + gap
        let width = max(0, primaryButton.frame.minX - gap - left)
        let headingHeight = ceil(heading.intrinsicContentSize.height)
        let maxDetail = card.height - tokens.number("s-4") * 2 - headingHeight - tokens.number("s-1")
        let detailHeight = min(maxDetail, ceil(detail.cell?.cellSize(forBounds:
            NSRect(x: 0, y: 0, width: width, height: .greatestFiniteMagnitude)).height ?? 0))
        let top = floor(card.midY - (headingHeight + tokens.number("s-1") + detailHeight) / 2)
        heading.frame = NSRect(x: left, y: top, width: width, height: headingHeight)
        detail.frame = NSRect(x: left, y: heading.frame.maxY + tokens.number("s-1"), width: width, height: detailHeight)
    }
}

final class RecordingSavedNoticePanel: NSPanel {
    let noticeView: RecordingSavedNoticeView
    override var canBecomeKey: Bool { true }

    init(screen: NSScreen, tokens: Tokens) {
        // The card sits 20 pt from the work area's top-right corner; the
        // transparent pad around it holds the shadow.
        let size = RecordingSavedNoticeView.size, pad = RecordingSavedNoticeView.pad, visible = screen.visibleFrame
        let origin = NSPoint(x: visible.maxX - size.width - 20 + pad, y: visible.maxY - size.height - 20 + pad)
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
