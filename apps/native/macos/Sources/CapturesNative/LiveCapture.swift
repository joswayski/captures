import AppKit
import ImageIO

private final class CaptureHistoryRow: NSTableRowView {
    var tokens: Tokens!
    override func drawSelection(in dirtyRect: NSRect) {
        tokens.color("surface-selected").setFill()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2),
                     xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md")).fill()
    }
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
}

final class LiveCaptureController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private let root: Surface
    private let window: NSWindow
    private let tokens: Tokens
    private let transport: AppTransport
    private static let queue = DispatchQueue(label: "es.captures.native.capture", qos: .userInitiated)
    private let historyRootOverride: String?
    private let showPreferences: () -> Void
    private var historyRoot = ""
    private var displays: [DisplayItem] = []
    private var artifacts: [CaptureArtifact] = []
    private var selectedImage: NSImage?
    private var selectedIndex: Int?
    private var selectionGeneration = 0
    private var capturing = false
    private var displayMenu: ClosurePopUpButton!
    private var table: NSTableView!
    private var preview: NSImageView!
    private var status: NSTextField!
    private var detail: NSTextField!
    private var captureButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var copyButton: CaptureButton!
    private var revealButton: CaptureButton!
    private var deleteButton: CaptureButton!

    init(root: Surface, window: NSWindow, tokens: Tokens, historyRoot: String?,
         transport: AppTransport = AppBridge(), showPreferences: @escaping () -> Void) {
        self.root = root; self.window = window; self.tokens = tokens
        historyRootOverride = historyRoot; self.transport = transport; self.showPreferences = showPreferences
        super.init(); build(); loadInitial()
    }

    private func build() {
        title("Capture workspace", frame: NSRect(x: 28, y: 22, width: 360, height: 30), size: 21, weight: .semibold)
        title("Full-display PNG capture · local native history", frame: NSRect(x: 28, y: 52, width: 450, height: 20), muted: true)
        button("Preferences", frame: NSRect(x: 846, y: 24, width: 126, height: 34), action: showPreferences)

        displayMenu = ClosurePopUpButton(frame: NSRect(x: 28, y: 90, width: 360, height: 34), pullsDown: false)
        displayMenu.tokens = tokens; displayMenu.setAccessibilityLabel("Display to capture")
        displayMenu.change = { _ in }; displayMenu.target = displayMenu; displayMenu.action = #selector(ClosurePopUpButton.selectedValue)
        root.addSubview(displayMenu)
        button("Refresh", frame: NSRect(x: 400, y: 90, width: 90, height: 34)) { [weak self] in self?.loadHistory(); self?.loadDisplays() }
        button("Request screen access", frame: NSRect(x: 502, y: 90, width: 176, height: 34)) { [weak self] in self?.requestPermission() }
        captureButton = button("Capture display", frame: NSRect(x: 690, y: 90, width: 156, height: 34)) { [weak self] in self?.capture() }
        captureButton.selected = true

        let scroll = NSScrollView(frame: NSRect(x: 28, y: 148, width: 320, height: 472))
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        table = NSTableView(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("history")); column.width = 300
        table.addTableColumn(column); table.headerView = nil; table.rowHeight = 62
        table.backgroundColor = tokens.color("surface-canvas"); table.dataSource = self; table.delegate = self
        table.setAccessibilityLabel("Screenshot history"); scroll.documentView = table; root.addSubview(scroll)

        let previewPanel = Surface(frame: NSRect(x: 372, y: 148, width: 600, height: 400))
        previewPanel.wantsLayer = true; previewPanel.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        previewPanel.layer?.cornerRadius = tokens.number("r-xl"); previewPanel.layer?.borderWidth = 1
        previewPanel.layer?.borderColor = tokens.color("border").cgColor; root.addSubview(previewPanel)
        preview = NSImageView(frame: previewPanel.bounds.insetBy(dx: 16, dy: 16)); preview.imageScaling = .scaleProportionallyUpOrDown
        preview.setAccessibilityLabel("Selected screenshot preview"); previewPanel.addSubview(preview)
        detail = title("Select a screenshot to preview it.", frame: NSRect(x: 372, y: 560, width: 600, height: 24), muted: true)
        saveButton = button("Save PNG…", frame: NSRect(x: 372, y: 594, width: 118, height: 34)) { [weak self] in self?.save() }
        copyButton = button("Copy image", frame: NSRect(x: 500, y: 594, width: 118, height: 34)) { [weak self] in self?.copyImage() }
        revealButton = button("Reveal export", frame: NSRect(x: 628, y: 594, width: 120, height: 34)) { [weak self] in self?.reveal() }
        deleteButton = button("Delete from history", frame: NSRect(x: 758, y: 594, width: 166, height: 34)) { [weak self] in self?.confirmDelete() }
        status = title("Loading capture history…", frame: NSRect(x: 28, y: 642, width: 944, height: 24), muted: true)
        let limits = title("This slice captures full displays as PNG only. Cursor, countdown, regions, recording, editor, and mini previews are not available yet. Preference capture defaults are not applied. Copy is explicit; captures are never auto-copied.", frame: NSRect(x: 28, y: 674, width: 944, height: 38), muted: true)
        limits.maximumNumberOfLines = 2; updateActions()
    }

    @discardableResult private func title(_ text: String, frame: NSRect, size: CGFloat = 13,
                                           weight: NSFont.Weight = .regular, muted: Bool = false) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text); label.frame = frame
        label.font = .systemFont(ofSize: size, weight: weight); label.textColor = tokens.color(muted ? "text-muted" : "text")
        root.addSubview(label); return label
    }
    @discardableResult private func button(_ title: String, frame: NSRect, action: @escaping () -> Void) -> CaptureButton {
        let value = CaptureButton(title, frame: frame, tokens: tokens, action: action); root.addSubview(value); return value
    }

    private func loadInitial() {
        run({ [historyRootOverride, transport] in
            if let historyRootOverride { return historyRootOverride }
            let result = try transport.request(["operation": "default_history_root"])
            guard let path = result["path"] as? String else { throw AppBridgeError.invalidResponse }; return path
        }) { [weak self] result in
            guard let self else { return }
            switch result { case .success(let path): self.historyRoot = path; self.loadHistory(); self.loadDisplays()
            case .failure(let error): self.showError("Couldn’t locate native history", error) }
        }
    }

    private func loadDisplays() {
        status.stringValue = "Refreshing displays…"
        run({ [transport] in
            let result = try transport.request(["operation": "displays"])
            guard let values = result["displays"] as? [[String: Any]] else { throw AppBridgeError.invalidResponse }
            let parsed = values.compactMap(DisplayItem.init)
            guard parsed.count == values.count else { throw AppBridgeError.invalidResponse }
            return parsed
        }) { [weak self] result in
            guard let self else { return }
            switch result { case .success(let values):
                self.displays = values; self.displayMenu.removeAllItems(); self.displayMenu.addItems(withTitles: values.map(\.title))
                self.status.stringValue = values.isEmpty ? "No displays are available. Screen access may be required." : self.historyStatus()
            case .failure(let error): self.showError("Couldn’t list displays", error) }
            self.updateActions()
        }
    }

    private func loadHistory(select id: String? = nil) {
        status.stringValue = "Loading capture history…"
        run({ [transport, historyRoot] in
            let result = try transport.request(["operation": "history", "root": historyRoot])
            guard let values = result["artifacts"] as? [[String: Any]] else { throw AppBridgeError.invalidResponse }
            let parsed = values.compactMap(CaptureArtifact.init)
            guard parsed.count == values.count else { throw AppBridgeError.invalidResponse }; return parsed
        }) { [weak self] result in
            guard let self else { return }
            switch result { case .success(let values):
                let previousID = id ?? self.selectedIndex.flatMap { self.artifacts.indices.contains($0) ? self.artifacts[$0].id : nil }
                self.clearSelection()
                self.artifacts = values; self.table.reloadData(); self.status.stringValue = self.historyStatus()
                if let previousID, let index = values.firstIndex(where: { $0.id == previousID }) { self.table.selectRowIndexes([index], byExtendingSelection: false) }
                else if !values.isEmpty { self.table.selectRowIndexes([0], byExtendingSelection: false) }
                else { self.clearSelection() }
            case .failure(let error): self.clearSelection(); self.artifacts = []; self.table.reloadData(); self.showError("Couldn’t load capture history", error) }
        }
    }

    private func historyStatus() -> String { artifacts.isEmpty ? "No screenshots yet. Choose a display, then capture." : "\(artifacts.count) screenshot\(artifacts.count == 1 ? "" : "s") in native history." }
    private func requestPermission() {
        status.stringValue = "Requesting screen access…"
        run({ [transport] in _ = try transport.request(["operation": "request_permission"]) }) { [weak self] result in
            switch result { case .success: self?.status.stringValue = "Screen access granted."; self?.loadDisplays()
            case .failure(let error): self?.showError("Screen access wasn’t granted", error) }
        }
    }

    private func capture() {
        let index = displayMenu.indexOfSelectedItem
        guard !capturing, displays.indices.contains(index), !historyRoot.isEmpty else { return }
        let display = displays[index]; setBusy(true, message: "Hiding Captures before capture…")
        window.orderOut(nil)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self else { return }
            self.run({ [transport, historyRoot] in
                let result = try transport.request(["operation": "capture_display", "root": historyRoot, "display_id": display.id])
                guard let value = result["artifact"] as? [String: Any], let artifact = CaptureArtifact(value) else { throw AppBridgeError.invalidResponse }
                return artifact
            }) { [weak self, window = self.window] result in
                window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
                guard let self else { return }; self.setBusy(false)
                switch result { case .success(let artifact): self.loadHistory(select: artifact.id)
                case .failure(let error): self.showError("Capture failed", error) }
            }
        }
    }

    private func setBusy(_ busy: Bool, message: String = "") { capturing = busy; updateActions(); if busy { status.stringValue = message } }
    private func showError(_ context: String, _ error: Error) { status.stringValue = "\(context): \(error.localizedDescription)"; status.textColor = tokens.color("danger-text") }
    private func updateActions() {
        let selected = selectedIndex.map { artifacts.indices.contains($0) } == true
        saveButton?.isEnabled = selected && !capturing; copyButton?.isEnabled = selectedImage != nil && !capturing
        deleteButton?.isEnabled = selected && !capturing; revealButton?.isEnabled = selected && selectedIndex.flatMap { artifacts[$0].savedPath } != nil
        captureButton?.isEnabled = !capturing && !displays.isEmpty && !historyRoot.isEmpty
    }

    func numberOfRows(in tableView: NSTableView) -> Int { artifacts.count }
    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let view = CaptureHistoryRow(); view.tokens = tokens; return view
    }
    func tableViewSelectionDidChange(_ notification: Notification) { select(table.selectedRow) }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView(); let artifact = artifacts[row]
        let name = NSTextField(labelWithString: "Screenshot · \(artifact.width) × \(artifact.height)")
        name.frame = NSRect(x: 12, y: 31, width: 280, height: 20); name.font = .systemFont(ofSize: 13, weight: .medium); name.textColor = tokens.color("text")
        let date = NSTextField(labelWithString: artifact.createdAt); date.frame = NSRect(x: 12, y: 10, width: 280, height: 18); date.font = .systemFont(ofSize: 11); date.textColor = tokens.color("text-muted")
        cell.addSubview(name); cell.addSubview(date); cell.textField = name; return cell
    }

    private func select(_ index: Int) {
        selectionGeneration += 1
        let generation = selectionGeneration
        selectedImage = nil; preview.image = nil
        guard artifacts.indices.contains(index) else { clearSelection(); return }
        selectedIndex = index; let artifact = artifacts[index]; detail.stringValue = "Loading \(artifact.width) × \(artifact.height) PNG…"; updateActions()
        run({ () throws -> NSImage in
            guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: artifact.imagePath) as CFURL, nil),
                  let image = CGImageSourceCreateImageAtIndex(source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
            else { throw AppBridgeError.invalidResponse }
            return NSImage(cgImage: image, size: NSSize(width: CGFloat(image.width), height: CGFloat(image.height)))
        }) { [weak self] result in
            guard let self, self.selectionGeneration == generation else { return }
            switch result { case .success(let image): self.selectedImage = image; self.preview.image = image; self.detail.stringValue = "\(artifact.width) × \(artifact.height) · PNG"
            case .failure(let error): self.showError("Couldn’t decode screenshot", error); self.detail.stringValue = "Preview unavailable" }
            self.updateActions()
        }
    }
    private func clearSelection() {
        selectionGeneration += 1; selectedIndex = nil; selectedImage = nil; preview?.image = nil
        if table.selectedRow >= 0 { table.deselectAll(nil) }
        detail?.stringValue = artifacts.isEmpty ? "Capture a display to begin." : "Select a screenshot to preview it."
        updateActions()
    }

    private func save() {
        guard let index = selectedIndex, artifacts.indices.contains(index) else { return }; let artifact = artifacts[index]
        let panel = NSOpenPanel(); panel.canChooseDirectories = true; panel.canChooseFiles = false; panel.allowsMultipleSelection = false; panel.prompt = "Save Here"
        panel.beginSheetModal(for: window) { [weak self] response in
            guard let self, response == .OK, let directory = panel.url else { return }
            self.status.stringValue = "Saving PNG…"
            self.run({ [transport, historyRoot] in
                let result = try transport.request(["operation": "save_png", "root": historyRoot, "id": artifact.id, "directory": directory.path])
                guard let value = result["artifact"] as? [String: Any], let updated = CaptureArtifact(value), let path = result["path"] as? String else { throw AppBridgeError.invalidResponse }
                return (updated, path)
            }) { result in
                switch result { case .success(let value):
                    if let current = self.artifacts.firstIndex(where: { $0.id == artifact.id }) { self.artifacts[current] = value.0 }
                    self.status.stringValue = "Saved PNG to \(value.1)"; self.table.reloadData()
                case .failure(let error): self.showError("Couldn’t save PNG", error) }; self.updateActions()
            }
        }
    }
    private func copyImage() {
        guard let index = selectedIndex, artifacts.indices.contains(index) else { return }
        let path = artifacts[index].imagePath
        run({ try Data(contentsOf: URL(fileURLWithPath: path)) }) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let png):
                let pasteboard = NSPasteboard.general; pasteboard.clearContents()
                self.status.stringValue = pasteboard.setData(png, forType: .png)
                    ? "Copied the selected image." : "Couldn’t copy the selected image."
            case .failure(let error): self.showError("Couldn’t copy image", error)
            }
        }
    }
    private func reveal() { guard let index = selectedIndex, artifacts.indices.contains(index), let path = artifacts[index].savedPath else { return }; NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
    private func confirmDelete() {
        guard let index = selectedIndex, artifacts.indices.contains(index) else { return }; let artifact = artifacts[index]
        let alert = NSAlert(); alert.messageText = "Delete this screenshot from history?"; alert.informativeText = "This removes the history copy. Any PNG you exported stays on disk."; alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete from History"); alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in guard response == .alertFirstButtonReturn else { return }; self?.delete(artifact) }
    }
    private func delete(_ artifact: CaptureArtifact) {
        status.stringValue = "Deleting from history…"
        run({ [transport, historyRoot] in _ = try transport.request(["operation": "delete", "root": historyRoot, "id": artifact.id]) }) { [weak self] result in
            switch result { case .success: self?.loadHistory()
            case .failure(let error): self?.showError("Couldn’t delete screenshot", error) }
        }
    }

    private func run<T>(_ work: @escaping () throws -> T, completion: @escaping (Result<T, Error>) -> Void) {
        status.textColor = tokens.color("text-muted")
        Self.queue.async { let result = Result(catching: work); DispatchQueue.main.async { completion(result) } }
    }

    // One process-wide queue also drains operations from a closed workspace view.
    static func flush() { queue.sync {} }
}
