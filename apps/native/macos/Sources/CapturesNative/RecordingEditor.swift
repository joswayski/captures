import AppKit
import UniformTypeIdentifiers

final class RecordingEditorController: NSObject, NSWindowDelegate, NSTextFieldDelegate {
    let window: NSWindow
    let root = Surface()
    private let worker: RecordingEditorWorking
    private let reportError: (String) -> Void
    private let didSaveCopy: () -> Void
    private let confirmDiscard: () -> Bool
    private var tokens: Tokens
    private var generation = 0
    private var artifactID: String?
    private var presentation: RecordingEditorPresentation?
    private var savedEdit: Data?
    private var savedExport: Data?
    private var busy = false
    private var pickerOpen = false
    private var activeCancel: NativeRecordingEditorCancel?
    private var estimate: RecordingEditorEstimate?

    private let previewPanel = Surface()
    private let preview = NSImageView()
    private let sourceLabel = NSTextField(labelWithString: "Opening recording…")
    private let seekSlider = NSSlider(value: 0, minValue: 0, maxValue: 1,
                                      target: nil, action: nil)
    private let seekLabel = NSTextField(labelWithString: "0:00.000 / 0:00.000")
    private let trimPanel = Surface()
    private let trimStart = NSTextField()
    private let trimEnd = NSTextField()
    private let format = NSPopUpButton()
    private let quality = NSPopUpButton()
    private let destination = NSTextField()
    private let status = NSTextField(wrappingLabelWithString: "")
    private let estimateLabel = NSTextField(labelWithString: "Size not estimated")
    private let progress = NSProgressIndicator()
    private var applyButton: CaptureButton!
    private var estimateButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var cancelButton: CaptureButton!
    private var changeButton: CaptureButton!

    init(tokens: Tokens, worker: RecordingEditorWorking = RecordingEditorWorker(),
         reportError: @escaping (String) -> Void = { _ in },
         didSaveCopy: @escaping () -> Void = {},
         confirmDiscard: (() -> Bool)? = nil) {
        self.tokens = tokens; self.worker = worker; self.reportError = reportError
        self.didSaveCopy = didSaveCopy
        self.confirmDiscard = confirmDiscard ?? RecordingEditorController.confirmDiscardAlert
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 960, height: 760),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        super.init()
        window.title = "Recording editor"
        window.minSize = NSSize(width: 760, height: 540)
        window.appearance = NSAppearance(named: tokens.color("text").brightnessComponent > 0.5
            ? .darkAqua : .aqua)
        window.delegate = self
        root.frame = window.contentView?.bounds ?? NSRect(x: 0, y: 0, width: 960, height: 760)
        root.autoresizingMask = [.width, .height]
        root.wantsLayer = true
        window.contentView = root
        buildUI()
        layout()
    }

    func present(artifact: CaptureArtifact, historyRoot: String, outputDirectory: String) {
        if artifactID == artifact.id {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }
        if artifactID != nil, busy || pickerOpen || dirty {
            showError("Finish, cancel, save, or discard the current recording edits first.")
            window.makeKeyAndOrderFront(nil); return
        }
        generation += 1
        let current = generation
        artifactID = artifact.id; presentation = nil; savedEdit = nil; savedExport = nil
        estimate = nil; activeCancel = nil; busy = true; pickerOpen = false
        preview.image = nil
        destination.stringValue = URL(fileURLWithPath: outputDirectory)
            .appendingPathComponent("recording-edit-\(artifact.id.prefix(8)).mp4").path
        sourceLabel.stringValue = "Opening recording…"
        status.stringValue = "Decoding the first source-relative frame…"
        progress.isHidden = true
        updateControls(); layout()
        window.center(); window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        worker.open(historyRoot: historyRoot, artifactID: artifact.id) { [weak self] result in
            guard let self, self.generation == current, self.artifactID == artifact.id else { return }
            self.busy = false
            switch result {
            case .success(let value):
                self.publish(value, initialize: true)
                self.status.stringValue = "Original remains unchanged. Save creates a new copy."
            case .failure(let error): self.showError("Couldn’t open recording: \(error.localizedDescription)")
            }
            self.updateControls(); self.layout()
        }
    }

    var dirty: Bool {
        guard let snapshot = presentation?.snapshot else { return false }
        return stagedDiffers || canonical(snapshot.edit) != savedEdit
            || canonical(snapshot.export) != savedExport
    }

    func prepareForTermination() -> Bool {
        if busy || pickerOpen {
            showError("Cancel or wait for the recording operation before quitting.")
            window.makeKeyAndOrderFront(nil); return false
        }
        if dirty {
            showError("Save or discard recording edits before quitting. Recording drafts are not available.")
            window.makeKeyAndOrderFront(nil); return false
        }
        closeSession(); return true
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if busy || pickerOpen {
            showError("Cancel or wait for the recording operation before closing.")
            return false
        }
        if dirty && !confirmDiscard() { return false }
        closeSession(); return true
    }

    func windowDidResize(_ notification: Notification) { layout() }
    func controlTextDidChange(_ notification: Notification) { estimate = nil; updateControls() }

    private func buildUI() {
        root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
        _ = label("Edit recording", size: 22, weight: .semibold)
        let note = label("Decoded frame preview · playback is not included in this slice", muted: true)
        note.identifier = NSUserInterfaceItemIdentifier("recording-editor-note")
        previewPanel.wantsLayer = true
        previewPanel.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        previewPanel.layer?.cornerRadius = tokens.number("r-md")
        preview.imageScaling = .scaleProportionallyUpOrDown
        preview.setAccessibilityLabel("Decoded recording frame")
        previewPanel.addSubview(preview)
        root.addSubview(previewPanel)
        sourceLabel.textColor = tokens.color("text-muted")
        sourceLabel.font = .systemFont(ofSize: 12)
        sourceLabel.setAccessibilityLabel("Recording source details")
        root.addSubview(sourceLabel)
        seekSlider.target = self; seekSlider.action = #selector(seekChanged)
        seekSlider.setAccessibilityLabel("Recording frame position")
        root.addSubview(seekSlider)
        seekLabel.textColor = tokens.color("text-muted"); seekLabel.alignment = .right
        root.addSubview(seekLabel)
        trimPanel.wantsLayer = true; trimPanel.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        trimPanel.layer?.cornerRadius = tokens.number("r-md")
        root.addSubview(trimPanel)
        configureNumberField(trimStart, label: "Trim start milliseconds")
        configureNumberField(trimEnd, label: "Trim end milliseconds")
        trimPanel.addSubview(label("Trim (milliseconds)", size: 14, weight: .semibold))
        trimPanel.addSubview(label("Start", muted: true)); trimPanel.addSubview(trimStart)
        trimPanel.addSubview(label("End", muted: true)); trimPanel.addSubview(trimEnd)
        applyButton = button("Apply edits") { [weak self] in self?.applyEdits() }
        trimPanel.addSubview(applyButton)

        format.addItems(withTitles: ["MP4", "GIF"])
        format.target = self; format.action = #selector(formatChanged)
        format.setAccessibilityLabel("Recording export format")
        quality.addItems(withTitles: ["Preserve", "Highest", "High", "Standard", "Small", "Tiny"])
        quality.target = self; quality.action = #selector(stageChanged)
        quality.setAccessibilityLabel("Recording export quality")
        destination.delegate = self; destination.setAccessibilityLabel("Recording destination")
        root.addSubview(format); root.addSubview(quality); root.addSubview(destination)
        changeButton = button("Change…") { [weak self] in self?.chooseDestination() }
        estimateButton = button("Estimate size") { [weak self] in self?.estimateSize() }
        saveButton = button("Save new copy") { [weak self] in self?.saveNewCopy() }
        saveButton.primary = true
        cancelButton = button("Cancel operation") { [weak self] in self?.activeCancel?.cancel() }
        cancelButton.signal = true
        status.textColor = tokens.color("text-muted"); status.maximumNumberOfLines = 2
        status.setAccessibilityLabel("Recording editor status")
        estimateLabel.textColor = tokens.color("text-muted")
        estimateLabel.setAccessibilityLabel("Recording size estimate")
        progress.minValue = 0; progress.maxValue = 1000; progress.isIndeterminate = false
        progress.setAccessibilityLabel("Recording export progress")
        root.addSubview(status); root.addSubview(estimateLabel); root.addSubview(progress)
    }

    private func layout() {
        let width = root.bounds.width, height = root.bounds.height
        guard width > 0, height > 0 else { return }
        root.subviews.first { ($0 as? NSTextField)?.stringValue == "Edit recording" }?.frame =
            NSRect(x: 24, y: 18, width: width - 48, height: 28)
        root.subviews.first { $0.identifier?.rawValue == "recording-editor-note" }?.frame =
            NSRect(x: 24, y: 48, width: width - 48, height: 20)
        let saveHeight: CGFloat = 150
        let trimHeight: CGFloat = 92
        let previewHeight = max(170, height - saveHeight - trimHeight - 150)
        previewPanel.frame = NSRect(x: 24, y: 76, width: width - 48, height: previewHeight)
        preview.frame = previewPanel.bounds.insetBy(dx: 12, dy: 12)
        let seekY = previewPanel.frame.maxY + 10
        sourceLabel.frame = NSRect(x: 24, y: seekY, width: width * 0.38, height: 20)
        seekSlider.frame = NSRect(x: width * 0.38, y: seekY, width: width * 0.38, height: 20)
        seekLabel.frame = NSRect(x: width * 0.77, y: seekY, width: width * 0.2 - 24, height: 20)
        trimPanel.frame = NSRect(x: 24, y: seekY + 30, width: width - 48, height: trimHeight)
        let labels = trimPanel.subviews.compactMap { $0 as? NSTextField }.filter { !$0.isEditable }
        labels.first { $0.stringValue == "Trim (milliseconds)" }?.frame = NSRect(x: 14, y: 12, width: 150, height: 20)
        labels.first { $0.stringValue == "Start" }?.frame = NSRect(x: 180, y: 14, width: 42, height: 18)
        labels.first { $0.stringValue == "End" }?.frame = NSRect(x: 354, y: 14, width: 34, height: 18)
        trimStart.frame = NSRect(x: 222, y: 8, width: 116, height: 28)
        trimEnd.frame = NSRect(x: 390, y: 8, width: 116, height: 28)
        applyButton.frame = NSRect(x: trimPanel.bounds.width - 126, y: 8, width: 112, height: 30)
        let explanation = labels.first { $0.stringValue.hasPrefix("Apply before") }
            ?? label("Apply before seeking or saving. The original is immutable.", muted: true,
                     parent: trimPanel)
        explanation.frame = NSRect(x: 14, y: 52, width: trimPanel.bounds.width - 28, height: 20)

        let barY = height - saveHeight
        status.frame = NSRect(x: 24, y: barY + 8, width: width - 48, height: 36)
        progress.frame = NSRect(x: 24, y: barY + 44, width: width - 174, height: 16)
        cancelButton.frame = NSRect(x: width - 140, y: barY + 38, width: 116, height: 28)
        destination.frame = NSRect(x: 24, y: barY + 72, width: width - 150, height: 28)
        changeButton.frame = NSRect(x: width - 116, y: barY + 70, width: 92, height: 30)
        format.frame = NSRect(x: 24, y: barY + 110, width: 92, height: 28)
        quality.frame = NSRect(x: 124, y: barY + 110, width: 116, height: 28)
        estimateLabel.frame = NSRect(x: 252, y: barY + 114, width: 190, height: 20)
        estimateButton.frame = NSRect(x: width - 296, y: barY + 106, width: 126, height: 32)
        saveButton.frame = NSRect(x: width - 160, y: barY + 106, width: 136, height: 32)
    }

    private func publish(_ value: RecordingEditorPresentation, initialize: Bool = false) {
        let old = presentation?.snapshot
        presentation = value
        preview.image = NSImage(cgImage: value.image,
                                size: NSSize(width: CGFloat(value.image.width),
                                             height: CGFloat(value.image.height)))
        sourceLabel.stringValue = "\(value.snapshot.width) × \(value.snapshot.height) source frame"
        seekSlider.maxValue = Double(max(1, value.snapshot.durationMilliseconds))
        seekSlider.doubleValue = Double(value.snapshot.positionMilliseconds)
        seekLabel.stringValue = "\(time(value.snapshot.positionMilliseconds)) / \(time(value.snapshot.durationMilliseconds))"
        let start = (value.snapshot.edit["trim_start_ms"] as? NSNumber)?.uint64Value ?? 0
        let end = (value.snapshot.edit["trim_end_ms"] as? NSNumber)?.uint64Value
            ?? value.snapshot.durationMilliseconds
        trimStart.stringValue = String(start)
        trimEnd.stringValue = String(end)
        select(format, value: value.snapshot.export["format"] as? String ?? "mp4")
        select(quality, value: value.snapshot.export["quality"] as? String ?? "preserve")
        if initialize {
            savedEdit = canonical(value.snapshot.edit); savedExport = canonical(value.snapshot.export)
        } else if canonical(old?.edit) != canonical(value.snapshot.edit)
                    || canonical(old?.export) != canonical(value.snapshot.export) {
            estimate = nil
        }
        updateControls()
    }

    private var stagedEdit: [String: Any]? {
        guard let accepted = presentation?.snapshot.edit,
              let start = UInt64(trimStart.stringValue), let end = UInt64(trimEnd.stringValue),
              let duration = presentation?.snapshot.durationMilliseconds,
              start < end, end <= duration else { return nil }
        var edit = accepted
        edit["trim_start_ms"] = start
        edit["trim_end_ms"] = end == duration ? NSNull() : end
        return edit
    }

    private var stagedExport: [String: Any]? {
        guard presentation != nil else { return nil }
        var value = presentation!.snapshot.export
        value["format"] = format.indexOfSelectedItem == 1 ? "gif" : "mp4"
        value["quality"] = quality.titleOfSelectedItem?.lowercased() ?? "preserve"
        return value
    }

    private var stagedDiffers: Bool {
        guard let snapshot = presentation?.snapshot else { return false }
        return canonical(stagedEdit) != canonical(snapshot.edit)
            || canonical(stagedExport) != canonical(snapshot.export)
    }

    private func applyEdits() {
        guard !busy, let edit = stagedEdit, let export = stagedExport else {
            showError("Enter a valid trim range within the recording duration."); return
        }
        request(["operation": "update_preview", "edit": edit, "export": export],
                activity: "Applying edits and decoding preview…")
    }

    @objc private func seekChanged() {
        guard !busy, !stagedDiffers else {
            seekSlider.doubleValue = Double(presentation?.snapshot.positionMilliseconds ?? 0)
            if stagedDiffers { showError("Apply staged trim and format changes before seeking.") }
            return
        }
        request(["operation": "seek", "position_ms": UInt64(seekSlider.doubleValue.rounded())],
                activity: "Decoding source-relative frame…")
    }

    private func request(_ object: [String: Any], activity: String) {
        guard !busy else { return }
        let current = generation; busy = true; status.stringValue = activity
        updateControls()
        worker.request(object) { [weak self] result in
            guard let self, self.generation == current else { return }
            self.busy = false
            switch result {
            case .success(let value): self.publish(value); self.status.stringValue = "Preview updated."
            case .failure(let error):
                self.seekSlider.doubleValue = Double(self.presentation?.snapshot.positionMilliseconds ?? 0)
                self.showError("Recording preview failed: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func estimateSize() {
        guard !busy, !stagedDiffers, presentation != nil,
              let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation; busy = true; activeCancel = cancel; estimate = nil
        status.stringValue = "Estimating accepted recording settings…"; updateControls()
        worker.estimate(cancel: cancel) { [weak self] result in
            guard let self, self.generation == current else { return }
            self.busy = false; self.activeCancel = nil
            switch result {
            case .success(let value): self.estimate = value; self.status.stringValue = "Estimate ready."
            case .failure(let error): self.showError("Size estimate failed: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func saveNewCopy() {
        guard !busy, !stagedDiffers, let export = presentation?.snapshot.export,
              !destination.stringValue.isEmpty,
              let cancel = NativeRecordingEditorCancel() else { return }
        let current = generation; busy = true; activeCancel = cancel
        progress.doubleValue = 0; progress.isHidden = false
        status.stringValue = "Saving new copy…"; updateControls()
        worker.save(destination: destination.stringValue, export: export, cancel: cancel,
            progress: { [weak self] value in
                guard let self, self.generation == current else { return }
                self.progress.doubleValue = Double(value.completedPerMille)
                self.status.stringValue = value.message
            }, completion: { [weak self] result in
                guard let self, self.generation == current else { return }
                self.busy = false; self.activeCancel = nil; self.progress.isHidden = true
                switch result {
                case .success(let saved):
                    if let snapshot = self.presentation?.snapshot {
                        self.savedEdit = self.canonical(snapshot.edit)
                        self.savedExport = self.canonical(snapshot.export)
                    }
                    switch saved {
                    case .saved(let path):
                        self.status.stringValue = "Saved new copy: \(path)"; self.didSaveCopy()
                    case .savedWithoutHistory(let path, let warning):
                        self.showError("Saved new copy: \(path). History could not be updated: \(warning)")
                    }
                case .failure(let error): self.showError("Save failed: \(error.localizedDescription)")
                }
                self.updateControls()
            })
    }

    private func chooseDestination() {
        guard !busy else { return }
        pickerOpen = true; updateControls()
        let panel = NSSavePanel(); panel.title = "Save recording as new copy"
        panel.canCreateDirectories = true; panel.nameFieldStringValue = URL(fileURLWithPath: destination.stringValue).lastPathComponent
        panel.directoryURL = URL(fileURLWithPath: destination.stringValue).deletingLastPathComponent()
        panel.allowedContentTypes = format.indexOfSelectedItem == 1 ? [.gif] : [.mpeg4Movie]
        panel.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            self.pickerOpen = false
            if response == .OK, let url = panel.url { self.destination.stringValue = url.path }
            self.updateControls()
        }
    }

    @objc private func formatChanged() {
        let ext = format.indexOfSelectedItem == 1 ? "gif" : "mp4"
        if !destination.stringValue.isEmpty {
            destination.stringValue = URL(fileURLWithPath: destination.stringValue)
                .deletingPathExtension().appendingPathExtension(ext).path
        }
        estimate = nil; updateControls()
    }
    @objc private func stageChanged() { estimate = nil; updateControls() }

    private func updateControls() {
        let available = presentation != nil && !busy && !pickerOpen
        let valid = stagedEdit != nil && stagedExport != nil
        [trimStart, trimEnd, format, quality, destination].forEach { $0.isEnabled = available }
        applyButton?.isEnabled = available && valid && stagedDiffers
        seekSlider.isEnabled = available && valid && !stagedDiffers
        changeButton?.isEnabled = available
        estimateButton?.isEnabled = available && valid && !stagedDiffers
        saveButton?.isEnabled = available && valid && !stagedDiffers && !destination.stringValue.isEmpty
        cancelButton?.isHidden = activeCancel == nil
        cancelButton?.isEnabled = activeCancel != nil
        if stagedDiffers { estimateLabel.stringValue = "Apply edits to estimate size" }
        else if let estimate {
            estimateLabel.stringValue = "\(estimate.exact ? "" : "≈ ")\(ByteCountFormatter.string(fromByteCount: Int64(estimate.sizeBytes), countStyle: .file))"
        } else { estimateLabel.stringValue = "Size not estimated" }
    }

    private func closeSession() {
        generation += 1; artifactID = nil; presentation = nil; activeCancel = nil
        worker.close()
    }

    private func showError(_ message: String) {
        status.textColor = tokens.color("danger-text"); status.stringValue = message
        reportError(message)
    }

    private func canonical(_ value: [String: Any]?) -> Data? {
        value.flatMap { try? JSONSerialization.data(withJSONObject: $0, options: [.sortedKeys]) }
    }

    private func time(_ milliseconds: UInt64) -> String {
        String(format: "%llu:%02llu.%03llu", milliseconds / 60_000,
               (milliseconds / 1_000) % 60, milliseconds % 1_000)
    }

    private func select(_ popup: NSPopUpButton, value: String) {
        let title = value == "mp4" ? "MP4" : value == "gif" ? "GIF"
            : value.prefix(1).uppercased() + String(value.dropFirst())
        popup.selectItem(withTitle: title)
    }

    private func configureNumberField(_ field: NSTextField, label: String) {
        field.delegate = self; field.setAccessibilityLabel(label)
        field.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
    }

    @discardableResult private func label(_ text: String, size: CGFloat = 12,
                                          weight: NSFont.Weight = .regular,
                                          muted: Bool = false, parent: NSView? = nil) -> NSTextField {
        let value = NSTextField(labelWithString: text)
        value.font = .systemFont(ofSize: size, weight: weight)
        value.textColor = tokens.color(muted ? "text-muted" : "text")
        (parent ?? root).addSubview(value); return value
    }

    @discardableResult private func button(_ title: String,
                                            action: @escaping () -> Void) -> CaptureButton {
        let value = CaptureButton(title, frame: .zero, tokens: tokens, action: action)
        root.addSubview(value); return value
    }

    private static func confirmDiscardAlert() -> Bool {
        let alert = NSAlert()
        alert.messageText = "Discard unsaved recording edits?"
        alert.informativeText = "Recording drafts are not available. The original recording is unchanged."
        alert.addButton(withTitle: "Keep editing")
        alert.addButton(withTitle: "Discard edits")
        return alert.runModal() == .alertSecondButtonReturn
    }
}
