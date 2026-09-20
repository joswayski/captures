import AppKit

struct ScreenshotEditorState: Equatable {
    private(set) var generation = 0
    private(set) var artifactID: String?
    private(set) var snapshot: NativeEditorSnapshot?
    private(set) var busy = false

    mutating func beginOpen(artifactID: String) -> Int {
        generation += 1; self.artifactID = artifactID; snapshot = nil; busy = true
        return generation
    }

    mutating func beginCommand() -> Int? {
        guard artifactID != nil, snapshot != nil, !busy else { return nil }
        busy = true; return generation
    }

    mutating func complete(_ value: NativeEditorSnapshot, generation: Int) -> Bool {
        guard self.generation == generation, artifactID == value.artifactID else { return false }
        snapshot = value; busy = false; return true
    }

    mutating func fail(generation: Int) -> Bool {
        guard self.generation == generation else { return false }
        busy = false; return true
    }

    mutating func close() { generation += 1; artifactID = nil; snapshot = nil; busy = false }
}

final class ScreenshotEditorController: NSObject, NSWindowDelegate {
    let window: NSWindow
    let root: Surface
    private(set) var state = ScreenshotEditorState()
    private let worker: EditorWorking
    private let reportError: (String) -> Void
    private var tokens: Tokens
    private let preview = NSImageView()
    private let cropX = NSTextField()
    private let cropY = NSTextField()
    private let cropWidth = NSTextField()
    private let cropHeight = NSTextField()
    private let canvasWidth = NSTextField()
    private let canvasHeight = NSTextField()
    private let status = NSTextField(wrappingLabelWithString: "")
    private let dimensions = NSTextField(labelWithString: "")
    private var undoButton: CaptureButton!
    private var redoButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var discardButton: CaptureButton!
    private var applyCropButton: CaptureButton!
    private var resizeButton: CaptureButton!
    private var fields: [NSTextField] = []
    private var closeAfterCommand = false

    init(tokens: Tokens, worker: EditorWorking = EditorWorker(),
         reportError: @escaping (String) -> Void = { _ in }) {
        self.tokens = tokens; self.worker = worker; self.reportError = reportError
        let bounds = NSRect(x: 0, y: 0, width: 1000, height: 700)
        root = Surface(frame: bounds)
        window = NSWindow(contentRect: bounds, styleMask: [.titled, .closable, .miniaturizable],
                          backing: .buffered, defer: false)
        super.init()
        window.isReleasedWhenClosed = false; window.title = "Edit screenshot"
        window.delegate = self
        window.contentView = root
        build(); restyle(tokens); updateControls()
    }

    func present(artifact: CaptureArtifact, historyRoot: String) {
        guard !artifact.isRecording else {
            showError("Recording editing is not available in this native editor.")
            return
        }
        if state.snapshot?.unsavedChanges == true, state.artifactID != artifact.id {
            showError("Save or discard the open screenshot edits before editing another capture.")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return
        }
        if state.artifactID == artifact.id, state.snapshot != nil {
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return
        }
        let generation = state.beginOpen(artifactID: artifact.id)
        preview.image = nil; window.title = "Edit screenshot"
        status.stringValue = "Opening screenshot…"; updateControls()
        window.center(); window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        let draftsRoot = URL(fileURLWithPath: historyRoot).deletingLastPathComponent()
            .appendingPathComponent("editor-drafts", isDirectory: true).path
        worker.open(historyRoot: historyRoot, draftsRoot: draftsRoot, artifactID: artifact.id) {
            [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let presentation):
                guard self.state.complete(presentation.snapshot, generation: generation) else { return }
                self.publish(presentation, resetCrop: true)
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = presentation.snapshot.hasDraft
                    ? "Draft restored." : "Ready. Changes affect only the native editor draft."
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.showError("Couldn’t open screenshot: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    func prepareForTermination() -> Bool {
        let result = worker.prepareForTermination()
        switch result {
        case .success:
            state.close(); preview.image = nil; window.orderOut(nil); return true
        case .failure(let error):
            showError("Couldn’t save screenshot draft before quitting: \(error.localizedDescription)")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return false
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard !state.busy else {
            status.stringValue = "Wait for the current editor action to finish."
            return false
        }
        guard state.snapshot?.unsavedChanges == true else { closeNow(); return false }
        let alert = NSAlert()
        alert.messageText = "Save screenshot edits?"
        alert.informativeText = "Save a native draft to continue later, close without saving this session, or cancel."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Save and Close")
        alert.addButton(withTitle: "Close Without Saving")
        alert.addButton(withTitle: "Cancel Close")
        alert.beginSheetModal(for: window) { [weak self] response in
            switch response {
            case .alertFirstButtonReturn: self?.saveDraft(closeAfter: true)
            // Freeing a session has no implicit write. This drops only changes
            // since the last save and retains any previously persisted draft.
            case .alertSecondButtonReturn: self?.closeNow()
            default: break
            }
        }
        return false
    }

    private func build() {
        label("Screenshot editor", frame: NSRect(x: 24, y: 20, width: 400, height: 30),
              size: 21, weight: .semibold)
        label("Crop and resize a recoverable native draft. The original History image is unchanged.",
              frame: NSRect(x: 24, y: 54, width: 640, height: 22), muted: true)

        let previewPanel = Surface(frame: NSRect(x: 24, y: 90, width: 640, height: 550))
        previewPanel.wantsLayer = true
        previewPanel.layer?.cornerRadius = tokens.number("r-xl")
        previewPanel.layer?.borderWidth = 1
        root.addSubview(previewPanel)
        preview.frame = previewPanel.bounds.insetBy(dx: 18, dy: 18)
        preview.imageScaling = .scaleProportionallyUpOrDown
        preview.setAccessibilityLabel("Edited screenshot preview")
        previewPanel.addSubview(preview)
        dimensions.frame = NSRect(x: 24, y: 654, width: 640, height: 20)
        dimensions.setAccessibilityLabel("Edited canvas dimensions"); root.addSubview(dimensions)

        label("Crop", frame: NSRect(x: 688, y: 24, width: 280, height: 24),
              size: 16, weight: .semibold)
        label("Uses canvas coordinates and shared Rust geometry.",
              frame: NSRect(x: 688, y: 54, width: 280, height: 22), muted: true)
        fieldLabel("X", x: 688, y: 92); fieldLabel("Y", x: 832, y: 92)
        configure(cropX, frame: NSRect(x: 688, y: 116, width: 128, height: 30), label: "Crop X")
        configure(cropY, frame: NSRect(x: 832, y: 116, width: 128, height: 30), label: "Crop Y")
        fieldLabel("Width", x: 688, y: 154); fieldLabel("Height", x: 832, y: 154)
        configure(cropWidth, frame: NSRect(x: 688, y: 178, width: 128, height: 30), label: "Crop width")
        configure(cropHeight, frame: NSRect(x: 832, y: 178, width: 128, height: 30), label: "Crop height")
        applyCropButton = button("Apply crop", frame: NSRect(x: 688, y: 220, width: 272, height: 34)) {
            [weak self] in self?.applyCrop()
        }

        label("Canvas", frame: NSRect(x: 688, y: 278, width: 280, height: 24),
              size: 16, weight: .semibold)
        label("Resize the canvas without scaling the image layer.",
              frame: NSRect(x: 688, y: 308, width: 280, height: 22), muted: true)
        fieldLabel("Width", x: 688, y: 346); fieldLabel("Height", x: 832, y: 346)
        configure(canvasWidth, frame: NSRect(x: 688, y: 370, width: 128, height: 30), label: "Canvas width")
        configure(canvasHeight, frame: NSRect(x: 832, y: 370, width: 128, height: 30), label: "Canvas height")
        resizeButton = button("Resize canvas", frame: NSRect(x: 688, y: 412, width: 272, height: 34)) {
            [weak self] in self?.resizeCanvas()
        }

        undoButton = button("Undo", frame: NSRect(x: 688, y: 470, width: 128, height: 34)) {
            [weak self] in self?.command(["operation": "undo"], message: "Undoing…")
        }
        undoButton.keyEquivalent = "z"; undoButton.keyEquivalentModifierMask = .command
        redoButton = button("Redo", frame: NSRect(x: 832, y: 470, width: 128, height: 34)) {
            [weak self] in self?.command(["operation": "redo"], message: "Redoing…")
        }
        redoButton.keyEquivalent = "Z"; redoButton.keyEquivalentModifierMask = [.command, .shift]
        saveButton = button("Save draft", frame: NSRect(x: 688, y: 524, width: 128, height: 34)) {
            [weak self] in self?.saveDraft()
        }
        saveButton.primary = true
        discardButton = button("Discard edits…", frame: NSRect(x: 832, y: 524, width: 128, height: 34)) {
            [weak self] in self?.confirmDiscard()
        }
        status.frame = NSRect(x: 688, y: 580, width: 272, height: 72)
        status.maximumNumberOfLines = 3; status.setAccessibilityLabel("Screenshot editor status")
        root.addSubview(status)
        fields = [cropX, cropY, cropWidth, cropHeight, canvasWidth, canvasHeight]
    }

    private func applyCrop() {
        guard let x = number(cropX), let y = number(cropY),
              let width = positive(cropWidth), let height = positive(cropHeight) else {
            showError("Crop values must be finite numbers with positive width and height."); return
        }
        command(["operation": "crop", "rect": [
            "x": x, "y": y, "width": width, "height": height,
        ]], message: "Applying crop…", resetCrop: true)
    }

    private func resizeCanvas() {
        guard let width = positive(canvasWidth), let height = positive(canvasHeight) else {
            showError("Canvas width and height must be finite positive numbers."); return
        }
        command(["operation": "resize_canvas", "width": width, "height": height],
                message: "Resizing canvas…", resetCrop: true)
    }

    private func saveDraft(closeAfter: Bool = false) {
        closeAfterCommand = closeAfter
        command(["operation": "save_draft", "updated_at_ms": EditorWorker.timestamp()],
                message: "Saving draft…")
    }

    private func confirmDiscard() {
        let alert = NSAlert()
        alert.messageText = "Discard all screenshot edits?"
        alert.informativeText = "This removes the native editor draft and restores the original History image."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Discard Edits"); alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in
            if response == .alertFirstButtonReturn { self?.discardEdits() }
        }
    }

    private func discardEdits(closeAfter: Bool = false) {
        closeAfterCommand = closeAfter
        command(["operation": "discard_draft"], message: "Discarding edits…", resetCrop: true)
    }

    private func command(_ object: [String: Any], message: String, resetCrop: Bool = false) {
        guard let generation = state.beginCommand() else { return }
        status.stringValue = message; updateControls()
        worker.request(object) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let presentation):
                guard self.state.complete(presentation.snapshot, generation: generation) else { return }
                self.publish(presentation, resetCrop: resetCrop)
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = presentation.snapshot.unsavedChanges
                    ? "Unsaved changes." : presentation.snapshot.hasDraft ? "Draft saved." : "Original restored."
                if self.closeAfterCommand { self.closeAfterCommand = false; self.closeNow(); return }
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.closeAfterCommand = false
                self.showError("Editor action failed: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func publish(_ presentation: EditorPresentation, resetCrop: Bool) {
        let snapshot = presentation.snapshot
        preview.image = NSImage(cgImage: presentation.image,
            size: NSSize(width: CGFloat(presentation.image.width),
                         height: CGFloat(presentation.image.height)))
        dimensions.stringValue = "\(format(snapshot.width)) × \(format(snapshot.height)) pixels"
        canvasWidth.stringValue = format(snapshot.width); canvasHeight.stringValue = format(snapshot.height)
        if resetCrop || cropWidth.stringValue.isEmpty {
            cropX.stringValue = "0"; cropY.stringValue = "0"
            cropWidth.stringValue = format(snapshot.width); cropHeight.stringValue = format(snapshot.height)
        }
        window.title = snapshot.unsavedChanges ? "Edit screenshot — Unsaved" : "Edit screenshot"
    }

    private func closeNow() {
        closeAfterCommand = false; state.close(); preview.image = nil
        worker.close(); window.orderOut(nil); updateControls()
    }

    private func updateControls() {
        let ready = state.snapshot != nil && !state.busy
        fields.forEach { $0.isEnabled = ready }
        applyCropButton?.isEnabled = ready; resizeButton?.isEnabled = ready
        undoButton?.isEnabled = ready && state.snapshot?.canUndo == true
        redoButton?.isEnabled = ready && state.snapshot?.canRedo == true
        saveButton?.isEnabled = ready && state.snapshot?.unsavedChanges == true
        discardButton?.isEnabled = ready && (state.snapshot?.hasDraft == true || state.snapshot?.unsavedChanges == true)
    }

    private func restyle(_ tokens: Tokens) {
        self.tokens = tokens; root.wantsLayer = true
        root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        let dark = tokens.color("text").brightnessComponent > 0.5
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        for case let label as NSTextField in root.subviews {
            label.textColor = tokens.color(label.identifier?.rawValue == "editor-muted"
                ? "text-muted" : "text")
        }
        status.textColor = tokens.color("text-muted"); dimensions.textColor = tokens.color("text-muted")
        preview.superview?.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        preview.superview?.layer?.borderColor = tokens.color("border").cgColor
    }

    private func configure(_ field: NSTextField, frame: NSRect, label: String) {
        field.frame = frame; field.setAccessibilityLabel(label)
        field.alignment = .right; field.placeholderString = "0"
        field.formatter = decimalFormatter(); root.addSubview(field)
    }

    private func decimalFormatter() -> NumberFormatter {
        let formatter = NumberFormatter(); formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = false; formatter.maximumFractionDigits = 3
        formatter.minimum = -1_000_000; formatter.maximum = 1_000_000
        return formatter
    }

    private func number(_ field: NSTextField) -> Double? {
        guard let value = Double(field.stringValue), value.isFinite else { return nil }
        return value
    }
    private func positive(_ field: NSTextField) -> Double? {
        guard let value = number(field), value > 0 else { return nil }; return value
    }
    private func format(_ value: Double) -> String {
        value.rounded() == value ? String(Int(value)) : String(format: "%.3f", value)
    }

    private func showError(_ message: String) {
        status.stringValue = message; status.textColor = tokens.color("danger-text")
        reportError(message)
    }

    @discardableResult private func label(_ text: String, frame: NSRect, size: CGFloat = 13,
                                           weight: NSFont.Weight = .regular,
                                           muted: Bool = false) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text); label.frame = frame
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = tokens.color(muted ? "text-muted" : "text")
        if muted { label.identifier = NSUserInterfaceItemIdentifier("editor-muted") }
        root.addSubview(label); return label
    }

    private func fieldLabel(_ text: String, x: CGFloat, y: CGFloat) {
        _ = label(text, frame: NSRect(x: x, y: y, width: 128, height: 20), muted: true)
    }

    @discardableResult private func button(_ title: String, frame: NSRect,
                                            action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: frame, tokens: tokens, action: action)
        root.addSubview(button); return button
    }
}
