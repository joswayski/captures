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

    mutating func completeOutput(generation: Int, artifactID: String) -> Bool {
        guard self.generation == generation, self.artifactID == artifactID,
              snapshot != nil else { return false }
        busy = false; return true
    }

    mutating func fail(generation: Int) -> Bool {
        guard self.generation == generation else { return false }
        busy = false; return true
    }

    mutating func close() { generation += 1; artifactID = nil; snapshot = nil; busy = false }
}

final class ScreenshotEditorController: NSObject, NSWindowDelegate, NSTableViewDataSource,
                                        NSTableViewDelegate, NSTextFieldDelegate {
    let window: NSWindow
    let root: Surface
    private(set) var state = ScreenshotEditorState()
    private let worker: EditorWorking
    private let reportError: (String) -> Void
    private let editorNumberFormatter: NumberFormatter
    private let outputIntegerFormatter: NumberFormatter
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
    private let geometryPanel = Surface()
    private let layersPanel = Surface()
    private let outputPanel = Surface()
    private let layerName = NSTextField()
    private let layerOpacity = NSTextField()
    private let layerX = NSTextField()
    private let layerY = NSTextField()
    private let outputQualityValue = NSTextField()
    private let outputPngPalette = NSTextField()
    private let outputByteBudget = NSTextField()
    private let outputSize = NSTextField(wrappingLabelWithString: "No encoded preview yet.")
    private var outputQualityValueLabel: NSTextField!
    private var outputPngPaletteLabel: NSTextField!
    private var outputByteBudgetLabel: NSTextField!
    private var sectionControl: NSSegmentedControl!
    private var outputFormat: NSPopUpButton!
    private var outputQuality: NSPopUpButton!
    private var outputPreviewMode: NSSegmentedControl!
    private var layerTable: NSTableView!
    private var visibilityButton: CaptureButton!
    private var lockButton: CaptureButton!
    private var renameButton: CaptureButton!
    private var opacityButton: CaptureButton!
    private var moveButton: CaptureButton!
    private var duplicateButton: CaptureButton!
    private var deleteButton: CaptureButton!
    private var moveUpButton: CaptureButton!
    private var moveDownButton: CaptureButton!
    private var undoButton: CaptureButton!
    private var redoButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var discardButton: CaptureButton!
    private var applyCropButton: CaptureButton!
    private var resizeButton: CaptureButton!
    private var previewOutputButton: CaptureButton!
    private var fields: [NSTextField] = []
    private var closeAfterCommand = false
    private var selectedLayerID: String?
    private var selectedLayerIndex = 0
    private var preferredLayerID: String?
    private var reconcilingLayerSelection = false
    private var editedImage: NSImage?
    private var encodedOutput: EditorOutputPresentation?

    init(tokens: Tokens, worker: EditorWorking = EditorWorker(), numberLocale: Locale = .current,
         reportError: @escaping (String) -> Void = { _ in }) {
        self.tokens = tokens; self.worker = worker; self.reportError = reportError
        editorNumberFormatter = NumberFormatter()
        outputIntegerFormatter = NumberFormatter()
        editorNumberFormatter.locale = numberLocale
        editorNumberFormatter.numberStyle = .decimal
        editorNumberFormatter.usesGroupingSeparator = false
        editorNumberFormatter.maximumFractionDigits = 3
        editorNumberFormatter.minimum = -1_000_000
        editorNumberFormatter.maximum = 1_000_000
        outputIntegerFormatter.locale = numberLocale
        outputIntegerFormatter.numberStyle = .decimal
        outputIntegerFormatter.usesGroupingSeparator = false
        outputIntegerFormatter.maximumFractionDigits = 0
        outputIntegerFormatter.minimum = 0
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
        guard !state.busy else {
            showError("Wait for the current editor action to finish before opening another screenshot.")
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
            return
        }
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
        selectedLayerID = nil; selectedLayerIndex = 0; preferredLayerID = nil
        editedImage = nil; invalidateOutput(); preview.image = nil; window.title = "Edit screenshot"
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
            state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
            window.orderOut(nil); return true
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

        sectionControl = NSSegmentedControl(labels: ["Geometry", "Layers", "Output"], trackingMode: .selectOne,
                                            target: self, action: #selector(changeSection))
        sectionControl.frame = NSRect(x: 688, y: 24, width: 272, height: 28)
        sectionControl.selectedSegment = 0
        sectionControl.setAccessibilityLabel("Editor section")
        root.addSubview(sectionControl)

        geometryPanel.frame = NSRect(x: 688, y: 66, width: 272, height: 390)
        layersPanel.frame = geometryPanel.frame; layersPanel.isHidden = true
        outputPanel.frame = geometryPanel.frame; outputPanel.isHidden = true
        geometryPanel.setAccessibilityLabel("Geometry controls")
        layersPanel.setAccessibilityLabel("Layer controls")
        outputPanel.setAccessibilityLabel("Output controls")
        root.addSubview(geometryPanel); root.addSubview(layersPanel); root.addSubview(outputPanel)

        panelLabel("Crop", frame: NSRect(x: 0, y: 0, width: 272, height: 24),
                   size: 16, weight: .semibold, parent: geometryPanel)
        panelLabel("Shared Rust owns canvas geometry.", frame: NSRect(x: 0, y: 28, width: 272, height: 22),
                   muted: true, parent: geometryPanel)
        panelFieldLabel("X", x: 0, y: 66, parent: geometryPanel)
        panelFieldLabel("Y", x: 144, y: 66, parent: geometryPanel)
        configure(cropX, frame: NSRect(x: 0, y: 90, width: 128, height: 30), label: "Crop X",
                  parent: geometryPanel)
        configure(cropY, frame: NSRect(x: 144, y: 90, width: 128, height: 30), label: "Crop Y",
                  parent: geometryPanel)
        panelFieldLabel("Width", x: 0, y: 128, parent: geometryPanel)
        panelFieldLabel("Height", x: 144, y: 128, parent: geometryPanel)
        configure(cropWidth, frame: NSRect(x: 0, y: 152, width: 128, height: 30), label: "Crop width",
                  parent: geometryPanel)
        configure(cropHeight, frame: NSRect(x: 144, y: 152, width: 128, height: 30), label: "Crop height",
                  parent: geometryPanel)
        applyCropButton = button("Apply crop", frame: NSRect(x: 0, y: 194, width: 272, height: 34),
                                 parent: geometryPanel) {
            [weak self] in self?.applyCrop()
        }

        panelLabel("Canvas", frame: NSRect(x: 0, y: 242, width: 272, height: 24),
                   size: 16, weight: .semibold, parent: geometryPanel)
        panelLabel("Resize canvas without scaling the image.", frame: NSRect(x: 0, y: 270, width: 272, height: 22),
                   muted: true, parent: geometryPanel)
        panelFieldLabel("Width", x: 0, y: 300, parent: geometryPanel)
        panelFieldLabel("Height", x: 144, y: 300, parent: geometryPanel)
        configure(canvasWidth, frame: NSRect(x: 0, y: 324, width: 128, height: 30), label: "Canvas width",
                  parent: geometryPanel)
        configure(canvasHeight, frame: NSRect(x: 144, y: 324, width: 128, height: 30), label: "Canvas height",
                  parent: geometryPanel)
        resizeButton = button("Resize canvas", frame: NSRect(x: 0, y: 356, width: 272, height: 34),
                              parent: geometryPanel) {
            [weak self] in self?.resizeCanvas()
        }

        buildLayersPanel()
        buildOutputPanel()

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
        discardButton = button("Discard edits…", frame: NSRect(x: 824, y: 524, width: 136, height: 34)) {
            [weak self] in self?.confirmDiscard()
        }
        status.frame = NSRect(x: 688, y: 568, width: 272, height: 96)
        status.maximumNumberOfLines = 5; status.setAccessibilityLabel("Screenshot editor status")
        root.addSubview(status)
        fields = [cropX, cropY, cropWidth, cropHeight, canvasWidth, canvasHeight]
    }

    private func buildOutputPanel() {
        panelLabel("Output preview", frame: NSRect(x: 0, y: 0, width: 272, height: 24),
                   size: 16, weight: .semibold, parent: outputPanel)
        panelLabel("Encode without saving or changing the draft.",
                   frame: NSRect(x: 0, y: 28, width: 272, height: 22), muted: true,
                   parent: outputPanel)

        panelFieldLabel("Format", x: 0, y: 58, parent: outputPanel)
        outputFormat = NSPopUpButton(frame: NSRect(x: 0, y: 78, width: 120, height: 30))
        outputFormat.addItems(withTitles: ["PNG", "JPEG", "WebP"])
        outputFormat.setAccessibilityLabel("Output format")
        outputFormat.target = self; outputFormat.action = #selector(outputOptionsChanged)
        outputPanel.addSubview(outputFormat)

        panelFieldLabel("Quality mode", x: 128, y: 58, parent: outputPanel)
        outputQuality = NSPopUpButton(frame: NSRect(x: 128, y: 78, width: 144, height: 30))
        outputQuality.addItems(withTitles: ["Preserve", "Compress", "Maximum file size"])
        outputQuality.setAccessibilityLabel("Output quality mode")
        outputQuality.target = self; outputQuality.action = #selector(outputOptionsChanged)
        outputPanel.addSubview(outputQuality)

        outputQualityValueLabel = panelFieldLabel("Quality value", x: 0, y: 118,
                                                  parent: outputPanel)
        outputPngPaletteLabel = panelFieldLabel("PNG palette", x: 144, y: 118,
                                                parent: outputPanel)
        configure(outputQualityValue, frame: NSRect(x: 0, y: 138, width: 128, height: 30),
                  label: "Output quality value", parent: outputPanel)
        configure(outputPngPalette, frame: NSRect(x: 144, y: 138, width: 128, height: 30),
                  label: "PNG maximum colors", parent: outputPanel)
        outputQualityValue.stringValue = "98"
        outputPngPalette.placeholderString = "Optional"

        outputByteBudgetLabel = panelFieldLabel("Byte budget (minimum 10,000)", x: 0, y: 178,
                                               parent: outputPanel)
        configure(outputByteBudget, frame: NSRect(x: 0, y: 198, width: 272, height: 30),
                  label: "Output byte budget", parent: outputPanel)
        outputByteBudget.placeholderString = "Required for Maximum"
        outputByteBudget.stringValue = "10000000"
        [outputQualityValue, outputPngPalette, outputByteBudget].forEach {
            $0.formatter = outputIntegerFormatter; $0.delegate = self
        }

        previewOutputButton = button("Preview output", frame: NSRect(x: 0, y: 240, width: 272, height: 34),
                                     parent: outputPanel) { [weak self] in self?.previewOutput() }
        previewOutputButton.primary = true
        outputPreviewMode = NSSegmentedControl(labels: ["Edited canvas", "Encoded output"],
                                               trackingMode: .selectOne, target: self,
                                               action: #selector(changeOutputPreview))
        outputPreviewMode.frame = NSRect(x: 0, y: 286, width: 272, height: 28)
        outputPreviewMode.selectedSegment = 0
        outputPreviewMode.setAccessibilityLabel("Output preview image")
        outputPanel.addSubview(outputPreviewMode)
        outputSize.frame = NSRect(x: 0, y: 326, width: 272, height: 58)
        outputSize.maximumNumberOfLines = 3
        outputSize.setAccessibilityLabel("Encoded output size")
        outputPanel.addSubview(outputSize)
        updateOutputOptionControls()
    }

    private func buildLayersPanel() {
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 272, height: 88))
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        layerTable = NSTableView(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("editor-layer"))
        column.width = 252; layerTable.addTableColumn(column); layerTable.headerView = nil
        layerTable.rowHeight = 32; layerTable.dataSource = self; layerTable.delegate = self
        layerTable.allowsEmptySelection = false; layerTable.setAccessibilityLabel("Screenshot layers")
        scroll.documentView = layerTable; layersPanel.addSubview(scroll)

        panelFieldLabel("Name", x: 0, y: 94, parent: layersPanel)
        layerName.frame = NSRect(x: 0, y: 112, width: 190, height: 30)
        layerName.setAccessibilityLabel("Layer name")
        layerName.alignment = .left; layerName.placeholderString = "Layer name"
        layersPanel.addSubview(layerName)
        renameButton = button("Rename", frame: NSRect(x: 196, y: 112, width: 76, height: 30),
                              parent: layersPanel) { [weak self] in self?.renameLayer() }
        visibilityButton = button("Hide", frame: NSRect(x: 0, y: 150, width: 128, height: 30),
                                  parent: layersPanel) { [weak self] in self?.toggleVisibility() }
        lockButton = button("Lock", frame: NSRect(x: 144, y: 150, width: 128, height: 30),
                            parent: layersPanel) { [weak self] in self?.toggleLock() }

        panelFieldLabel("Opacity (0–100)", x: 0, y: 186, parent: layersPanel)
        configure(layerOpacity, frame: NSRect(x: 0, y: 204, width: 216, height: 30),
                  label: "Layer opacity", parent: layersPanel)
        opacityButton = button("Set", frame: NSRect(x: 222, y: 204, width: 50, height: 30),
                               parent: layersPanel) { [weak self] in self?.setOpacity() }

        panelFieldLabel("X", x: 0, y: 240, parent: layersPanel)
        panelFieldLabel("Y", x: 92, y: 240, parent: layersPanel)
        configure(layerX, frame: NSRect(x: 0, y: 258, width: 86, height: 30),
                  label: "Layer X", parent: layersPanel)
        configure(layerY, frame: NSRect(x: 92, y: 258, width: 86, height: 30),
                  label: "Layer Y", parent: layersPanel)
        moveButton = button("Move", frame: NSRect(x: 184, y: 258, width: 88, height: 30),
                            parent: layersPanel) { [weak self] in self?.moveLayer() }

        duplicateButton = button("Duplicate", frame: NSRect(x: 0, y: 296, width: 128, height: 30),
                                 parent: layersPanel) { [weak self] in self?.duplicateLayer() }
        deleteButton = button("Delete", frame: NSRect(x: 144, y: 296, width: 128, height: 30),
                              parent: layersPanel) { [weak self] in self?.deleteLayer() }
        moveUpButton = button("Move up", frame: NSRect(x: 0, y: 334, width: 128, height: 30),
                              parent: layersPanel) { [weak self] in self?.reorderLayer(up: true) }
        moveDownButton = button("Move down", frame: NSRect(x: 144, y: 334, width: 128, height: 30),
                                parent: layersPanel) { [weak self] in self?.reorderLayer(up: false) }
    }

    @objc private func changeSection() {
        geometryPanel.isHidden = sectionControl.selectedSegment != 0
        layersPanel.isHidden = sectionControl.selectedSegment != 1
        outputPanel.isHidden = sectionControl.selectedSegment != 2
    }

    @objc private func outputOptionsChanged() {
        normalizeOutputQuality()
        invalidateOutput(optionsChanged: true)
        updateOutputOptionControls()
        updateControls()
    }

    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField,
              [outputQualityValue, outputPngPalette, outputByteBudget].contains(where: { $0 === field })
        else { return }
        invalidateOutput(optionsChanged: true)
        updateControls()
    }

    @objc private func changeOutputPreview() {
        if outputPreviewMode.selectedSegment == 1, let encodedOutput {
            preview.image = NSImage(cgImage: encodedOutput.image,
                                    size: NSSize(width: encodedOutput.image.width,
                                                 height: encodedOutput.image.height))
        } else {
            outputPreviewMode.selectedSegment = 0
            preview.image = editedImage
        }
    }

    private func previewOutput() {
        guard let artifactID = state.artifactID,
              let options = outputOptions(),
              let generation = state.beginCommand() else { return }
        invalidateOutput()
        status.stringValue = "Encoding output preview…"; updateControls()
        worker.encode(options) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let output):
                guard self.state.completeOutput(generation: generation, artifactID: artifactID) else { return }
                self.encodedOutput = output
                self.outputSize.stringValue = "Exact encoded size: \(self.formatInteger(output.length)) bytes"
                self.outputPreviewMode.selectedSegment = 1
                self.changeOutputPreview()
                self.status.textColor = self.tokens.color("text-muted")
                self.status.stringValue = "Output preview encoded. No file was saved."
            case .failure(let error):
                guard self.state.fail(generation: generation) else { return }
                self.invalidateOutput()
                self.showError("Output preview failed: \(error.localizedDescription)")
            }
            self.updateControls()
        }
    }

    private func outputOptions() -> [String: Any]? {
        let formats = ["png", "jpeg", "webp"]
        let qualities = ["preserve", "compress", "maximum"]
        guard formats.indices.contains(outputFormat.indexOfSelectedItem),
              qualities.indices.contains(outputQuality.indexOfSelectedItem) else { return nil }
        let format = formats[outputFormat.indexOfSelectedItem]
        let quality = qualities[outputQuality.indexOfSelectedItem]
        let qualityValue: UInt64
        if quality == "compress" {
            let minimum: UInt64 = format == "jpeg" ? 40 : 1
            guard let value = outputInteger(outputQualityValue), (minimum...100).contains(value) else {
                showError("Output quality must be a whole number from \(minimum) through 100 for \(format.uppercased()).")
                return nil
            }
            qualityValue = value
        } else {
            qualityValue = 100
        }
        var png: [String: Any] = [:]
        if format == "png", quality == "compress", !outputPngPalette.stringValue.isEmpty {
            guard let colors = outputInteger(outputPngPalette), (1...256).contains(colors) else {
                showError("PNG palette size must be a whole number from 1 through 256."); return nil
            }
            png["max_colors"] = colors
        }
        var options: [String: Any] = [
            "format": format, "quality": quality, "quality_value": qualityValue, "png": png,
        ]
        if quality == "maximum" {
            guard let budget = outputInteger(outputByteBudget), budget >= 10_000 else {
                showError("Enter an output byte budget of at least 10,000."); return nil
            }
            options["max_size_bytes"] = budget
        }
        return options
    }

    private func invalidateOutput(optionsChanged: Bool = false) {
        let hadOutput = encodedOutput != nil
        encodedOutput = nil
        outputPreviewMode?.selectedSegment = 0
        preview.image = editedImage
        if optionsChanged && hadOutput {
            outputSize.stringValue = "Options changed. Preview output again."
        } else if !optionsChanged {
            outputSize.stringValue = "No encoded preview yet."
        }
    }

    private func updateOutputOptionControls() {
        guard outputFormat != nil, outputQuality != nil else { return }
        let ready = state.snapshot != nil && !state.busy
        let compress = outputQuality.indexOfSelectedItem == 1
        let maximum = outputQuality.indexOfSelectedItem == 2
        let pngPalette = compress && outputFormat.indexOfSelectedItem == 0
        outputQualityValueLabel.isHidden = !compress; outputQualityValue.isHidden = !compress
        outputPngPaletteLabel.isHidden = !pngPalette; outputPngPalette.isHidden = !pngPalette
        outputByteBudgetLabel.isHidden = !maximum; outputByteBudget.isHidden = !maximum
        outputQualityValue.isEnabled = ready && compress
        outputPngPalette.isEnabled = ready && pngPalette
        outputByteBudget.isEnabled = ready && maximum
    }

    private func normalizeOutputQuality() {
        guard outputQuality.indexOfSelectedItem == 1,
              let current = outputInteger(outputQualityValue) else { return }
        let minimum: UInt64 = outputFormat.indexOfSelectedItem == 1 ? 40 : 1
        outputQualityValue.stringValue = String(min(100, max(minimum, current)))
    }

    private var selectedLayer: NativeEditorLayer? {
        guard let id = selectedLayerID else { return nil }
        return state.snapshot?.layers.first { $0.id == id }
    }

    private func layerCommand(_ layer: NativeEditorLayer, edit: [String: Any], message: String,
                              preferredSelection: String? = nil) {
        command(["operation": "layer", "id": layer.id, "edit": edit], message: message,
                preferredSelection: preferredSelection)
    }

    private func toggleVisibility() {
        guard let layer = selectedLayer else { return }
        layerCommand(layer, edit: ["action": "visibility", "visible": !layer.visible],
                     message: layer.visible ? "Hiding layer…" : "Showing layer…")
    }

    private func toggleLock() {
        guard let layer = selectedLayer else { return }
        layerCommand(layer, edit: ["action": "lock", "locked": !layer.locked],
                     message: layer.locked ? "Unlocking layer…" : "Locking layer…")
    }

    private func renameLayer() {
        guard let layer = selectedLayer, layer.kind == .image,
              !layerName.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            showError("Image layer names cannot be empty."); return
        }
        layerCommand(layer, edit: ["action": "rename", "name": layerName.stringValue],
                     message: "Renaming layer…")
    }

    private func setOpacity() {
        guard let layer = selectedLayer, let opacity = number(layerOpacity),
              (0...100).contains(opacity) else {
            showError("Layer opacity must be between 0 and 100."); return
        }
        layerCommand(layer, edit: ["action": "opacity", "opacity": opacity],
                     message: "Updating layer opacity…")
    }

    private func moveLayer() {
        guard let layer = selectedLayer, !layer.locked,
              let x = number(layerX), let y = number(layerY) else {
            showError("Unlocked layer coordinates must be finite numbers."); return
        }
        layerCommand(layer, edit: ["action": "translate", "delta_x": x - layer.x,
                                   "delta_y": y - layer.y], message: "Moving layer…")
    }

    private func duplicateLayer() {
        guard let layer = selectedLayer else { return }
        let newID = UUID().uuidString.lowercased()
        layerCommand(layer, edit: ["action": "duplicate", "new_id": newID],
                     message: "Duplicating layer…", preferredSelection: newID)
    }

    private func deleteLayer() {
        guard let layer = selectedLayer, !layer.locked else { return }
        layerCommand(layer, edit: ["action": "delete"], message: "Deleting layer…")
    }

    private func reorderLayer(up: Bool) {
        guard let snapshot = state.snapshot, let layer = selectedLayer,
              !layer.locked, let index = snapshot.layers.firstIndex(where: { $0.id == layer.id }) else { return }
        let targetIndex = up ? index - 1 : index + 1
        guard snapshot.layers.indices.contains(targetIndex), !snapshot.layers[targetIndex].locked else { return }
        layerCommand(layer, edit: ["action": "reorder",
                                   "target_id": snapshot.layers[targetIndex].id,
                                   "placement": up ? "before" : "after"],
                     message: up ? "Moving layer up…" : "Moving layer down…")
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

    private func command(_ object: [String: Any], message: String, resetCrop: Bool = false,
                         preferredSelection: String? = nil) {
        guard let generation = state.beginCommand() else { return }
        invalidateOutput()
        preferredLayerID = preferredSelection
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
                self.preferredLayerID = nil
                self.closeAfterCommand = false
                self.showError("Editor action failed: \(error.localizedDescription)")
                self.publishSelectedLayerFields()
            }
            self.updateControls()
        }
    }

    private func publish(_ presentation: EditorPresentation, resetCrop: Bool) {
        let snapshot = presentation.snapshot
        editedImage = NSImage(cgImage: presentation.image,
            size: NSSize(width: CGFloat(presentation.image.width),
                         height: CGFloat(presentation.image.height)))
        preview.image = editedImage
        dimensions.stringValue = "\(format(snapshot.width)) × \(format(snapshot.height)) pixels"
        canvasWidth.stringValue = format(snapshot.width); canvasHeight.stringValue = format(snapshot.height)
        if resetCrop || cropWidth.stringValue.isEmpty {
            cropX.stringValue = "0"; cropY.stringValue = "0"
            cropWidth.stringValue = format(snapshot.width); cropHeight.stringValue = format(snapshot.height)
        }
        reconcileLayerSelection(snapshot.layers)
        window.title = snapshot.unsavedChanges ? "Edit screenshot — Unsaved" : "Edit screenshot"
    }

    private func closeNow() {
        closeAfterCommand = false; selectedLayerID = nil; preferredLayerID = nil
        state.close(); editedImage = nil; invalidateOutput(); preview.image = nil
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
        sectionControl?.isEnabled = ready
        outputFormat?.isEnabled = ready; outputQuality?.isEnabled = ready
        previewOutputButton?.isEnabled = ready
        outputPreviewMode?.isEnabled = ready && encodedOutput != nil
        updateOutputOptionControls()
        layerTable?.isEnabled = ready
        let layer = ready ? selectedLayer : nil
        let image = layer?.kind == .image
        layerName.isEnabled = image; renameButton?.isEnabled = image
        layerOpacity.isEnabled = layer != nil; opacityButton?.isEnabled = layer != nil
        visibilityButton?.isEnabled = layer != nil; lockButton?.isEnabled = layer != nil
        duplicateButton?.isEnabled = layer != nil
        let movable = layer != nil && layer?.locked == false
        layerX.isEnabled = movable; layerY.isEnabled = movable; moveButton?.isEnabled = movable
        deleteButton?.isEnabled = movable
        if let snapshot = state.snapshot, let layer,
           let index = snapshot.layers.firstIndex(where: { $0.id == layer.id }) {
            moveUpButton?.isEnabled = movable && index > 0 && !snapshot.layers[index - 1].locked
            moveDownButton?.isEnabled = movable && index + 1 < snapshot.layers.count
                && !snapshot.layers[index + 1].locked
        } else {
            moveUpButton?.isEnabled = false; moveDownButton?.isEnabled = false
        }
        visibilityButton?.title = layer?.visible == false ? "Show" : "Hide"
        lockButton?.title = layer?.locked == true ? "Unlock" : "Lock"
    }

    private func reconcileLayerSelection(_ layers: [NativeEditorLayer]) {
        let preferred = preferredLayerID
        preferredLayerID = nil
        if let preferred, layers.contains(where: { $0.id == preferred }) {
            selectedLayerID = preferred
        } else if let selectedLayerID, layers.contains(where: { $0.id == selectedLayerID }) {
            self.selectedLayerID = selectedLayerID
        } else if layers.isEmpty {
            selectedLayerID = nil; selectedLayerIndex = 0
        } else {
            selectedLayerIndex = min(selectedLayerIndex, layers.count - 1)
            selectedLayerID = layers[selectedLayerIndex].id
        }
        reconcilingLayerSelection = true
        defer { reconcilingLayerSelection = false }
        layerTable?.reloadData()
        if let selectedLayerID,
           let index = layers.firstIndex(where: { $0.id == selectedLayerID }) {
            selectedLayerIndex = index
            layerTable?.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            layerTable?.scrollRowToVisible(index)
        } else {
            layerTable?.deselectAll(nil)
        }
        publishSelectedLayerFields()
    }

    private func publishSelectedLayerFields() {
        guard let layer = selectedLayer else {
            [layerName, layerOpacity, layerX, layerY].forEach { $0.stringValue = "" }
            updateControls(); return
        }
        layerName.stringValue = layer.name
        layerOpacity.stringValue = format(layer.opacity)
        layerX.stringValue = format(layer.x); layerY.stringValue = format(layer.y)
        updateControls()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { state.snapshot?.layers.count ?? 0 }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !reconcilingLayerSelection else { return }
        guard let layers = state.snapshot?.layers, layers.indices.contains(layerTable.selectedRow) else {
            selectedLayerID = nil; publishSelectedLayerFields(); return
        }
        selectedLayerIndex = layerTable.selectedRow
        selectedLayerID = layers[selectedLayerIndex].id
        publishSelectedLayerFields()
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let layers = state.snapshot?.layers, layers.indices.contains(row) else { return nil }
        let layer = layers[row]
        let cell = NSTableCellView()
        let title = NSTextField(labelWithString: layer.name)
        title.frame = NSRect(x: 8, y: 4, width: 112, height: 22)
        title.lineBreakMode = .byTruncatingTail; title.toolTip = layer.name
        title.textColor = tokens.color("text")
        let detail = NSTextField(labelWithString:
            "\(layer.kind.rawValue.capitalized)\(layer.visible ? "" : " · Hidden")\(layer.locked ? " · Locked" : "")")
        detail.frame = NSRect(x: 124, y: 4, width: 124, height: 22)
        detail.alignment = .right; detail.font = .systemFont(ofSize: 10)
        detail.textColor = tokens.color("text-muted"); detail.toolTip = detail.stringValue
        cell.addSubview(title); cell.addSubview(detail); cell.textField = title
        return cell
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
        outputSize.textColor = tokens.color("text-muted")
        preview.superview?.layer?.backgroundColor = tokens.color("surface-sunken").cgColor
        preview.superview?.layer?.borderColor = tokens.color("border").cgColor
    }

    private func configure(_ field: NSTextField, frame: NSRect, label: String,
                           parent: NSView? = nil) {
        field.frame = frame; field.setAccessibilityLabel(label)
        field.alignment = .right; field.placeholderString = "0"
        field.formatter = editorNumberFormatter; (parent ?? root).addSubview(field)
    }

    private func number(_ field: NSTextField) -> Double? {
        guard let value = editorNumberFormatter.number(from: field.stringValue)?.doubleValue,
              value.isFinite else { return nil }
        return value
    }
    private func positive(_ field: NSTextField) -> Double? {
        guard let value = number(field), value > 0 else { return nil }; return value
    }
    private func format(_ value: Double) -> String {
        editorNumberFormatter.string(from: NSNumber(value: value)) ?? String(value)
    }
    private func outputInteger(_ field: NSTextField) -> UInt64? {
        guard let value = outputIntegerFormatter.number(from: field.stringValue)?.doubleValue,
              value.isFinite, value >= 0, value.rounded() == value,
              value < Double(UInt64.max) else { return nil }
        return UInt64(value)
    }
    private func formatInteger(_ value: Int) -> String {
        outputIntegerFormatter.string(from: NSNumber(value: value)) ?? String(value)
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

    @discardableResult private func panelFieldLabel(_ text: String, x: CGFloat, y: CGFloat,
                                                     parent: NSView) -> NSTextField {
        panelLabel(text, frame: NSRect(x: x, y: y, width: 128, height: 20),
                   muted: true, parent: parent)
    }

    @discardableResult private func panelLabel(_ text: String, frame: NSRect, size: CGFloat = 13,
                                                weight: NSFont.Weight = .regular,
                                                muted: Bool = false,
                                                parent: NSView) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text); label.frame = frame
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = tokens.color(muted ? "text-muted" : "text")
        if muted { label.identifier = NSUserInterfaceItemIdentifier("editor-muted") }
        parent.addSubview(label); return label
    }

    @discardableResult private func button(_ title: String, frame: NSRect,
                                            parent: NSView? = nil,
                                            action: @escaping () -> Void) -> CaptureButton {
        let button = CaptureButton(title, frame: frame, tokens: tokens, action: action)
        (parent ?? root).addSubview(button); return button
    }
}
