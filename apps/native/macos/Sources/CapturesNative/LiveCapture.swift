import AppKit
import ImageIO
import CCapturesSettings

private final class CaptureHistoryRow: NSTableRowView {
    var tokens: Tokens!
    override func drawSelection(in dirtyRect: NSRect) {
        tokens.color("surface-selected").setFill()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2),
                     xRadius: tokens.number("r-md"), yRadius: tokens.number("r-md")).fill()
    }
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
}

enum StillCaptureKind: Equatable {
    case display
    case region
    case window
}

enum CaptureWindowRestoreAction: Equatable {
    case none
    case visible
    case key
}

struct CaptureWindowRestoration {
    private var wasVisible = false
    private var wasKey = false

    mutating func begin(windowIsVisible: Bool, windowIsKey: Bool) {
        wasVisible = windowIsVisible
        wasKey = windowIsKey
    }
    mutating func finish(restoreRequested: Bool) -> CaptureWindowRestoreAction {
        defer { wasVisible = false; wasKey = false }
        guard restoreRequested, wasVisible else { return .none }
        return wasKey ? .key : .visible
    }
}

struct CapturePreparationGate {
    private(set) var current = 0

    mutating func begin() -> Int {
        current += 1
        return current
    }

    mutating func invalidate() { current += 1 }
    func accepts(_ request: Int) -> Bool { request == current }
}

struct RecordingLifecycleGate {
    private(set) var busy = false

    mutating func begin() -> Bool {
        guard !busy else { return false }
        busy = true
        return true
    }

    mutating func end() { busy = false }
}

final class LiveCaptureController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private let root: Surface
    private let window: NSWindow
    private let tokens: Tokens
    private let transport: AppTransport
    static let queue = DispatchQueue(label: "es.captures.native.capture", qos: .userInitiated)
    private let historyRootOverride: String?
    private let settingsPath: String?
    private let showPreferences: () -> Void
    private let captureStateChanged: (Bool) -> Void
    private let selectorGenerationChanged: (UInt64?) -> Void
    private let reportError: (String) -> Void
    private weak var miniPreviews: MiniPreviewController?
    private weak var miniPreviewActions: MiniPreviewActions?
    private let initialSelectionID: String?
    private var historyRoot = ""
    private var displays: [DisplayItem] = []
    private var artifacts: [CaptureArtifact] = []
    private var selectedImage: NSImage?
    private var selectedIndex: Int?
    private var selectionGeneration = 0
    private var capturing = false
    private var windowRestoration = CaptureWindowRestoration()
    private var clearingHistory = false
    private var flowGeneration: UInt64?
    private var previewCaptureGeneration: UInt64?
    private var countdownTimer: Timer?
    private var countdownPanel: ScreenshotCountdownPanel?
    private var snapshotPending = false
    private var preparingRegion = false
    private var preparingWindow = false
    private var regionSession: NativeRegionSession?
    private var regionPanel: RegionSelectionPanel?
    private var regionRect: CapturesSelectionRect?
    private var windowSession: NativeWindowSession?
    private var windowPanel: WindowSelectionPanel?
    private var windowTarget: WindowSelectionChoice?
    private var unifiedPreparation = CapturePreparationGate()
    private var preparingUnified = false
    private var unifiedSession: NativeWindowSession?
    private var unifiedPanel: UnifiedCapturePanel?
    private var unifiedTarget: WindowSelectionChoice?
    private var unifiedDisplay: DisplayItem?
    private var unifiedScreen: NSScreen?
    private var unifiedControlsState = UnifiedCaptureControlsState.initial
    private var recordingCapabilities: NativeRecordingCapabilities?
    private var microphoneDevices: [NativeMicrophoneDevice] = []
    private var recordingControlState = RecordingControlState(framesPerSecond: 60,
        maxResolution: "original", showCursor: true, highlightClicks: false,
        systemAudio: false, microphoneDeviceID: nil)
    private let recordingGate = RecordingGenerationGate()
    private var recordingSession: NativeRecordingSession?
    private var recordingHUD: RecordingHUDPanel?
    private var recordingPollTimer: Timer?
    private var recordingPollPending = false
    private var recordingLifecycle = RecordingLifecycleGate()
    private var preparingRecording = false
    private var recordingPendingStart = false
    private var activeRecordingGeneration: UInt64?
    private var recordingDisplay: DisplayItem?
    private var recordingPreferences: CapturePreferences?
    private var selectorShortcutGeneration: UInt64? {
        didSet {
            if oldValue != selectorShortcutGeneration {
                selectorGenerationChanged(selectorShortcutGeneration)
            }
        }
    }
    private var displayMenu: ClosurePopUpButton!
    private var table: NSTableView!
    private var preview: NSImageView!
    private var status: NSTextField!
    private var detail: NSTextField!
    private var captureButton: CaptureButton!
    private var regionButton: CaptureButton!
    private var windowButton: CaptureButton!
    private var saveButton: CaptureButton!
    private var copyButton: CaptureButton!
    private var revealButton: CaptureButton!
    private var deleteButton: CaptureButton!
    private var clearHistoryButton: CaptureButton!
    private var newCaptureButton: CaptureButton!

    init(root: Surface, window: NSWindow, tokens: Tokens, historyRoot: String?, settingsPath: String?,
         transport: AppTransport = AppBridge(), miniPreviews: MiniPreviewController? = nil,
         miniPreviewActions: MiniPreviewActions? = nil,
         initialSelectionID: String? = nil,
         captureStateChanged: @escaping (Bool) -> Void = { _ in },
         selectorGenerationChanged: @escaping (UInt64?) -> Void = { _ in },
         reportError: @escaping (String) -> Void = { _ in },
         showPreferences: @escaping () -> Void) {
        self.root = root; self.window = window; self.tokens = tokens
        historyRootOverride = historyRoot; self.transport = transport; self.showPreferences = showPreferences
        self.settingsPath = settingsPath; self.miniPreviews = miniPreviews
        self.miniPreviewActions = miniPreviewActions
        self.initialSelectionID = initialSelectionID
        self.captureStateChanged = captureStateChanged
        self.selectorGenerationChanged = selectorGenerationChanged
        self.reportError = reportError
        super.init(); build(); loadInitial()
    }

    private func build() {
        title("Capture workspace", frame: NSRect(x: 28, y: 22, width: 360, height: 30), size: 21, weight: .semibold)
        title("Region, window and display capture · local native history", frame: NSRect(x: 28, y: 52, width: 480, height: 20), muted: true)
        button("Preferences", frame: NSRect(x: 846, y: 24, width: 126, height: 34), action: showPreferences)

        displayMenu = ClosurePopUpButton(frame: NSRect(x: 28, y: 90, width: 300, height: 34), pullsDown: false)
        displayMenu.tokens = tokens; displayMenu.setAccessibilityLabel("Display to capture")
        displayMenu.change = { _ in }; displayMenu.target = displayMenu; displayMenu.action = #selector(ClosurePopUpButton.selectedValue)
        root.addSubview(displayMenu)
        button("Refresh", frame: NSRect(x: 340, y: 90, width: 90, height: 34)) { [weak self] in self?.loadHistory(); self?.loadDisplays() }
        button("Screen access", frame: NSRect(x: 442, y: 90, width: 148, height: 34)) { [weak self] in self?.requestPermission() }
        newCaptureButton = button("New Capture…", frame: NSRect(x: 708, y: 24, width: 126, height: 34)) { [weak self] in self?.newCapture() }
        newCaptureButton.primary = true
        captureButton = button("Capture display", frame: NSRect(x: 602, y: 90, width: 116, height: 34)) { [weak self] in self?.capture(.display) }
        captureButton.selected = true
        regionButton = button("Capture region", frame: NSRect(x: 730, y: 90, width: 116, height: 34)) { [weak self] in self?.capture(.region) }
        windowButton = button("Capture window", frame: NSRect(x: 858, y: 90, width: 114, height: 34)) { [weak self] in self?.capture(.window) }

        let scroll = NSScrollView(frame: NSRect(x: 28, y: 148, width: 320, height: 426))
        scroll.hasVerticalScroller = true; scroll.drawsBackground = false
        table = NSTableView(frame: scroll.bounds)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("history")); column.width = 300
        table.addTableColumn(column); table.headerView = nil; table.rowHeight = 62
        table.backgroundColor = tokens.color("surface-canvas"); table.dataSource = self; table.delegate = self
        table.setAccessibilityLabel("Capture history"); scroll.documentView = table; root.addSubview(scroll)

        let previewPanel = Surface(frame: NSRect(x: 372, y: 148, width: 600, height: 400))
        previewPanel.wantsLayer = true; previewPanel.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        previewPanel.layer?.cornerRadius = tokens.number("r-xl"); previewPanel.layer?.borderWidth = 1
        previewPanel.layer?.borderColor = tokens.color("border").cgColor; root.addSubview(previewPanel)
        preview = NSImageView(frame: previewPanel.bounds.insetBy(dx: 16, dy: 16)); preview.imageScaling = .scaleProportionallyUpOrDown
        preview.setAccessibilityLabel("Selected capture preview"); previewPanel.addSubview(preview)
        detail = title("Select a capture to preview it.", frame: NSRect(x: 372, y: 560, width: 600, height: 24), muted: true)
        saveButton = button("Save image", frame: NSRect(x: 372, y: 594, width: 118, height: 34)) { [weak self] in self?.save() }
        copyButton = button("Copy image", frame: NSRect(x: 500, y: 594, width: 118, height: 34)) { [weak self] in self?.copyImage() }
        revealButton = button("Reveal export", frame: NSRect(x: 628, y: 594, width: 120, height: 34)) { [weak self] in self?.reveal() }
        deleteButton = button("Delete from history", frame: NSRect(x: 758, y: 594, width: 166, height: 34)) { [weak self] in self?.confirmDelete() }
        clearHistoryButton = button("Clear screenshots…", frame: NSRect(x: 28, y: 594, width: 180, height: 34)) { [weak self] in self?.confirmClearHistory() }
        status = title("Loading capture history…", frame: NSRect(x: 28, y: 642, width: 944, height: 24), muted: true)
        let limits = title("Screenshots and H.264 MP4 recordings are kept in native History. Recording mute, hide, and screenshots while recording remain unavailable in this native slice.", frame: NSRect(x: 28, y: 674, width: 944, height: 38), muted: true)
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
            switch result { case .success(let path):
                self.historyRoot = path; self.miniPreviewActions?.configure(historyRoot: path)
                self.loadHistory(select: self.initialSelectionID); self.loadDisplays()
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

    private func loadHistory(select id: String? = nil, completion: (() -> Void)? = nil) {
        status.stringValue = "Loading capture history…"
        run({ [transport, historyRoot] in
            let result = try transport.request(["operation": "history", "root": historyRoot])
            guard let values = result["artifacts"] as? [[String: Any]] else { throw AppBridgeError.invalidResponse }
            let recordings = try NativeRecordingInfo.request([
                "operation": "history", "root": historyRoot,
            ])["recordings"] as? [[String: Any]] ?? []
            let all = values + recordings
            let parsed = all.compactMap(CaptureArtifact.init)
                .sorted { $0.createdAt > $1.createdAt }
            guard parsed.count == all.count else { throw AppBridgeError.invalidResponse }; return parsed
        }) { [weak self] result in
            guard let self else { return }
            switch result { case .success(let values):
                let previousID = id ?? self.selectedIndex.flatMap { self.artifacts.indices.contains($0) ? self.artifacts[$0].id : nil }
                self.clearSelection()
                self.artifacts = values; self.table.reloadData(); self.status.stringValue = self.historyStatus()
                self.miniPreviews?.reconcileHistory(ids: Set(values.map(\.id)))
                if let previousID, let index = values.firstIndex(where: { $0.id == previousID }) { self.table.selectRowIndexes([index], byExtendingSelection: false) }
                else if !values.isEmpty { self.table.selectRowIndexes([0], byExtendingSelection: false) }
                else { self.clearSelection() }
            case .failure(let error): self.clearSelection(); self.artifacts = []; self.table.reloadData(); self.showError("Couldn’t load capture history", error) }
            self.updateActions(); completion?()
        }
    }

    private func historyStatus() -> String { artifacts.isEmpty ? "No captures yet. Choose New Capture to begin." : "\(artifacts.count) capture\(artifacts.count == 1 ? "" : "s") in native history." }
    private func requestPermission() {
        status.stringValue = "Requesting screen access…"
        run({ [transport] in _ = try transport.request(["operation": "request_permission"]) }) { [weak self] result in
            switch result { case .success: self?.status.stringValue = "Screen access granted."; self?.loadDisplays()
            case .failure(let error): self?.showError("Screen access wasn’t granted", error) }
        }
    }

    @discardableResult func capture(_ kind: StillCaptureKind) -> Bool {
        let index = displayMenu.indexOfSelectedItem
        guard !capturing, displays.indices.contains(index), !historyRoot.isEmpty else { return false }
        windowRestoration.begin(windowIsVisible: window.isVisible, windowIsKey: window.isKeyWindow)
        let display = displays[index]; setBusy(true, message: "Preparing capture…")
        run({ [settingsPath] in try CapturePreferences.load(path: settingsPath) }) { [weak self] result in
            guard let self else { return }
            do {
                let preferences = try result.get()
                guard let screen = NSScreen.screens.first(where: {
                    ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue == display.id
                }) else { throw AppBridgeError.backend("The selected display is no longer available.") }
                let selecting = kind != .display
                let response = try AppBridge.flow(["operation": "begin", "seconds": selecting ? 0 : preferences.countdown])
                guard let generation = response["generation"] as? NSNumber else { throw AppBridgeError.invalidResponse }
                self.flowGeneration = generation.uint64Value; self.snapshotPending = false
                self.previewCaptureGeneration = self.miniPreviews?.beginCapture(
                    settings: preferences.miniPreviewSettings)
                self.preparingRegion = kind == .region
                self.preparingWindow = kind == .window
                self.window.orderOut(nil)
                if kind == .display && preferences.countdown > 0 {
                    let panel = ScreenshotCountdownPanel(screen: screen, tokens: self.tokens, remaining: preferences.countdown)
                    self.countdownPanel = panel; panel.orderFrontRegardless()
                }
                let tick: () -> Void = { [weak self] in self?.tickCountdown(display: display, preferences: preferences, generation: generation.uint64Value) }
                // Cancellation must still poll while AppKit tracks a drag/control.
                let timer = Timer(timeInterval: 0.1, repeats: true) { _ in tick() }
                self.countdownTimer = timer; RunLoop.main.add(timer, forMode: .common)
                tick()
                if kind == .region {
                    self.prepareRegion(display: display, screen: screen, preferences: preferences, generation: generation.uint64Value)
                } else if kind == .window {
                    self.prepareWindow(display: display, screen: screen, preferences: preferences, generation: generation.uint64Value)
                }
            } catch {
                self.finishCapture(); self.showError("Couldn’t start capture", error)
            }
        }
        return true
    }

    @discardableResult func newCapture(recordingTarget: UnifiedCaptureTarget? = nil) -> Bool {
        let index = displayMenu.indexOfSelectedItem
        guard !capturing, displays.indices.contains(index), !historyRoot.isEmpty else { return false }
        windowRestoration.begin(windowIsVisible: window.isVisible, windowIsKey: window.isKeyWindow)
        let display = displays[index]
        unifiedControlsState = .initial
        if let recordingTarget {
            unifiedControlsState.mode = .record
            unifiedControlsState.target = recordingTarget
        }
        setBusy(true, message: "Preparing capture controls…")
        let request = unifiedPreparation.begin()
        run({ [settingsPath] in
            let preferences = try CapturePreferences.load(path: settingsPath)
            let capabilities = try NativeRecordingInfo.capabilities(
                includeControls: preferences.includeRecordingControlsInCaptures)
            let devices = capabilities.microphone
                ? try NativeRecordingInfo.microphoneDevices() : []
            return (preferences, capabilities, devices)
        }) { [weak self] result in
            guard let self, self.capturing, self.unifiedPreparation.accepts(request) else { return }
            do {
                let (preferences, capabilities, devices) = try result.get()
                self.recordingCapabilities = capabilities
                self.microphoneDevices = devices
                self.recordingControlState = RecordingControlState(
                    framesPerSecond: preferences.recording.framesPerSecond,
                    maxResolution: preferences.recording.maxResolution,
                    showCursor: preferences.recording.showCursor,
                    highlightClicks: preferences.recording.highlightClicks,
                    systemAudio: preferences.recording.captureSystemAudio,
                    microphoneDeviceID: capabilities.microphone
                        ? preferences.recording.microphoneDeviceID : nil)
                let response = try AppBridge.flow(["operation": "begin", "seconds": 0])
                guard let generation = response["generation"] as? NSNumber else {
                    throw AppBridgeError.invalidResponse
                }
                self.flowGeneration = generation.uint64Value
                self.snapshotPending = false
                self.previewCaptureGeneration = self.miniPreviews?.beginCapture(
                    settings: preferences.miniPreviewSettings)
                self.window.orderOut(nil)
                let tick: () -> Void = { [weak self] in
                    guard let self, let display = self.unifiedDisplay else { return }
                    self.tickCountdown(display: display, preferences: preferences,
                        generation: generation.uint64Value)
                }
                let timer = Timer(timeInterval: 0.1, repeats: true) { _ in tick() }
                self.countdownTimer = timer
                RunLoop.main.add(timer, forMode: .common)
                self.prepareUnified(display: display, preferences: preferences,
                    generation: generation.uint64Value)
                tick()
            } catch {
                self.finishCapture()
                self.showError("Couldn’t start New Capture", error)
            }
        }
        return true
    }

    private func prepareUnified(display: DisplayItem, preferences: CapturePreferences,
                                generation: UInt64) {
        selectorShortcutGeneration = nil
        guard let screen = screen(for: display) else {
            finishCapture()
            showError("Couldn’t prepare capture controls",
                AppBridgeError.backend("The selected display is no longer available."))
            return
        }
        if let selector = unifiedPanel?.selector {
            unifiedControlsState = selector.controlsState
        }
        unifiedPanel?.close(); unifiedPanel = nil
        unifiedSession = nil; unifiedTarget = nil
        unifiedDisplay = display; unifiedScreen = screen
        preparingUnified = true
        status.stringValue = "Preparing capture controls…"
        let request = unifiedPreparation.begin()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self, self.flowGeneration == generation,
                  self.unifiedPreparation.accepts(request) else { return }
            self.run({
                let session = try NativeWindowSession.prepare(display: display.id,
                    generation: generation, preferences: preferences)
                return (session, try session.image())
            }) { [weak self] result in
                guard let self, self.flowGeneration == generation,
                      self.unifiedPreparation.accepts(request) else { return }
                do {
                    let state = try AppBridge.flow(["operation": "poll", "generation": generation])
                    guard state["current"] as? Bool == true else {
                        self.finishCapture()
                        self.status.stringValue = "Capture cancelled."
                        return
                    }
                    let (session, image) = try result.get()
                    guard session.display.size == screen.frame.size else {
                        throw AppBridgeError.backend(
                            "The selected display changed. Open New Capture again.")
                    }
                    self.unifiedSession = session
                    let selectedDisplay = self.displays.firstIndex(where: { $0.id == display.id }) ?? 0
                    let panel = UnifiedCapturePanel(screen: screen, image: image,
                        targets: session.windows, tokens: self.tokens,
                        autoStart: preferences.autoStart,
                        hitTest: { [weak session] point in session?.hitTest(point) },
                        displayTitles: self.displays.map(\.title), selectedDisplay: selectedDisplay,
                        confirm: { [weak self] target in
                            self?.confirmUnified(target, preferences: preferences,
                                generation: generation)
                        }, cancel: { [weak self] in
                            guard let self, self.flowGeneration == generation,
                                  self.unifiedPanel != nil else { return }
                            self.finishCapture()
                            self.status.stringValue = "Capture cancelled."
                        }, changeDisplay: { [weak self] index in
                            guard let self, self.flowGeneration == generation,
                                  self.displays.indices.contains(index),
                                  self.displays[index].id != self.unifiedDisplay?.id else { return }
                            self.prepareUnified(display: self.displays[index],
                                preferences: preferences, generation: generation)
                        }, recordingState: self.recordingControlState,
                        recordingAvailability: self.recordingCapabilities.map {
                            RecordingControlAvailability(cursor: $0.cursorControl,
                                clicks: $0.clickHighlights, systemAudio: $0.systemAudio,
                                microphone: $0.microphone)
                        }, microphoneDevices: self.microphoneDevices)
                    panel.selector.controls.recordingControlsChanged = { [weak self] state in
                        self?.recordingControlState = state
                    }
                    self.unifiedPanel = panel
                    self.preparingUnified = false
                    panel.selector.restoreControls(self.unifiedControlsState)
                    guard self.unifiedPanel === panel else { return }
                    panel.makeKeyAndOrderFront(nil)
                    self.selectorShortcutGeneration = generation
                    NSApp.activate(ignoringOtherApps: true)
                    panel.selector.updatePointerLocation()
                } catch {
                    self.finishCapture()
                    self.showError("Couldn’t prepare capture controls", error)
                }
            }
        }
    }

    private func confirmUnified(_ target: WindowSelectionChoice,
                                preferences: CapturePreferences, generation: UInt64) {
        guard flowGeneration == generation, unifiedPanel != nil,
              let screen = unifiedScreen, let display = unifiedDisplay else { return }
        if unifiedPanel?.selector.mode == .record {
            prepareRecording(target: target, display: display, screen: screen,
                preferences: preferences, generation: generation)
            return
        }
        do {
            selectorShortcutGeneration = nil
            unifiedTarget = target
            unifiedPanel?.close(); unifiedPanel = nil
            _ = try AppBridge.flow(["operation": "start_countdown", "generation": generation,
                "seconds": preferences.countdown])
            if preferences.countdown > 0 {
                let countdown = ScreenshotCountdownPanel(screen: screen, tokens: tokens,
                    remaining: preferences.countdown)
                countdownPanel = countdown
                countdown.orderFrontRegardless()
            }
            tickCountdown(display: display, preferences: preferences, generation: generation)
        } catch {
            finishCapture()
            showError("Capture failed", error)
        }
    }

    private func prepareRecording(target: WindowSelectionChoice, display: DisplayItem,
                                  screen: NSScreen, preferences: CapturePreferences,
                                  generation: UInt64) {
        guard let capabilities = recordingCapabilities else {
            finishCapture()
            showError("Couldn’t start recording",
                AppBridgeError.backend("Recording capabilities are unavailable."))
            return
        }
        do {
            let targetValue = try nativeRecordingTarget(target, displayID: display.id)
            let options = nativeRecordingOptions(preferences: preferences.recording,
                target: targetValue, capabilities: capabilities,
                controls: recordingControlState)
            selectorShortcutGeneration = nil
            unifiedTarget = target
            unifiedPanel?.close(); unifiedPanel = nil
            preparingRecording = true
            status.stringValue = "Preparing recording…"
            let recoveryRoot = URL(fileURLWithPath: historyRoot).deletingLastPathComponent()
                .appendingPathComponent("recording-recovery").path
            run({
                let tools = try NativeMediaTools.locate()
                try tools.verify()
                return try NativeRecordingSession.prepare(recoveryRoot: recoveryRoot,
                    options: options, display: display.descriptor)
            }) { [weak self] result in
                guard let self else { return }
                guard self.flowGeneration == generation else {
                    if case .success(let value) = result {
                        Self.queue.async { _ = try? value.0.discard() }
                    }
                    return
                }
                do {
                    let (session, _) = try result.get()
                    self.recordingSession = session
                    self.recordingDisplay = display
                    self.recordingPreferences = preferences
                    self.recordingPendingStart = true
                    self.recordingGate.set(generation)
                    self.activeRecordingGeneration = generation
                    _ = try AppBridge.flow(["operation": "start_countdown",
                        "generation": generation, "seconds": preferences.recording.countdown])
                    if preferences.recording.countdown > 0 {
                        let countdown = ScreenshotCountdownPanel(screen: screen, tokens: self.tokens,
                            remaining: preferences.recording.countdown)
                        self.countdownPanel = countdown; countdown.orderFrontRegardless()
                    }
                    self.preparingRecording = false
                    self.tickCountdown(display: display, preferences: preferences,
                        generation: generation)
                } catch {
                    self.preparingRecording = false
                    self.finishCapture(); self.showError("Couldn’t prepare recording", error)
                }
            }
        } catch {
            finishCapture(); showError("Couldn’t prepare recording", error)
        }
    }

    private func screen(for display: DisplayItem) -> NSScreen? {
        NSScreen.screens.first {
            ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue
                == display.id
        }
    }

    private func prepareRegion(display: DisplayItem, screen: NSScreen, preferences: CapturePreferences, generation: UInt64) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self, self.flowGeneration == generation else { return }
            self.run({
                let session = try NativeRegionSession.prepare(display: display.id, generation: generation, preferences: preferences)
                return (session, try session.image())
            }) { [weak self] result in
                guard let self, self.flowGeneration == generation else { return }
                do {
                    let state = try AppBridge.flow(["operation": "poll", "generation": generation])
                    guard state["current"] as? Bool == true else {
                        self.finishCapture(); self.status.stringValue = "Capture cancelled."; return
                    }
                    let (session, image) = try result.get()
                    guard session.logicalSize == screen.frame.size else {
                        throw AppBridgeError.backend("The selected display changed. Select the region again.")
                    }
                    self.regionSession = session
                    let panel = RegionSelectionPanel(screen: screen, image: image, tokens: self.tokens,
                        autoStart: preferences.autoStart, confirm: { [weak self] rect in
                            guard let self, self.flowGeneration == generation, self.regionPanel != nil else { return }
                            do {
                                self.regionRect = rect
                                self.regionPanel?.close(); self.regionPanel = nil
                                _ = try AppBridge.flow(["operation": "start_countdown", "generation": generation, "seconds": preferences.countdown])
                                if preferences.countdown > 0 {
                                    let countdown = ScreenshotCountdownPanel(screen: screen, tokens: self.tokens, remaining: preferences.countdown)
                                    self.countdownPanel = countdown; countdown.orderFrontRegardless()
                                }
                                self.tickCountdown(display: display, preferences: preferences, generation: generation)
                            } catch { self.finishCapture(); self.showError("Capture failed", error) }
                        }, cancel: { [weak self] in
                            guard let self, self.flowGeneration == generation, self.regionPanel != nil else { return }
                            self.finishCapture(); self.status.stringValue = "Capture cancelled."
                        })
                    self.regionPanel = panel; self.preparingRegion = false
                    panel.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
                } catch { self.finishCapture(); self.showError("Couldn’t prepare region", error) }
            }
        }
    }

    private func prepareWindow(display: DisplayItem, screen: NSScreen,
                               preferences: CapturePreferences, generation: UInt64) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self, self.flowGeneration == generation else { return }
            self.run({
                let session = try NativeWindowSession.prepare(display: display.id,
                    generation: generation, preferences: preferences)
                return (session, try session.image())
            }) { [weak self] result in
                guard let self, self.flowGeneration == generation else { return }
                do {
                    let state = try AppBridge.flow(["operation": "poll", "generation": generation])
                    guard state["current"] as? Bool == true else {
                        self.finishCapture(); self.status.stringValue = "Capture cancelled."; return
                    }
                    let (session, image) = try result.get()
                    guard session.display.size == screen.frame.size else {
                        throw AppBridgeError.backend("The selected display changed. Select the window again.")
                    }
                    self.windowSession = session
                    let panel = WindowSelectionPanel(screen: screen, image: image,
                        targets: session.windows, tokens: self.tokens,
                        autoStart: preferences.autoStart,
                        hitTest: { [weak session] point in session?.hitTest(point) },
                        confirm: { [weak self] target in
                            guard let self, self.flowGeneration == generation, self.windowPanel != nil else { return }
                            do {
                                self.windowTarget = target
                                self.windowPanel?.close(); self.windowPanel = nil
                                _ = try AppBridge.flow(["operation": "start_countdown",
                                    "generation": generation, "seconds": preferences.countdown])
                                if preferences.countdown > 0 {
                                    let countdown = ScreenshotCountdownPanel(screen: screen,
                                        tokens: self.tokens, remaining: preferences.countdown)
                                    self.countdownPanel = countdown; countdown.orderFrontRegardless()
                                }
                                self.tickCountdown(display: display, preferences: preferences,
                                    generation: generation)
                            } catch { self.finishCapture(); self.showError("Capture failed", error) }
                        }, cancel: { [weak self] in
                            guard let self, self.flowGeneration == generation,
                                  self.windowPanel != nil else { return }
                            self.finishCapture(); self.status.stringValue = "Capture cancelled."
                        })
                    self.windowPanel = panel; self.preparingWindow = false
                    panel.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
                    panel.updatePointerLocation()
                } catch { self.finishCapture(); self.showError("Couldn’t prepare window selection", error) }
            }
        }
    }

    private func tickCountdown(display: DisplayItem, preferences: CapturePreferences, generation: UInt64) {
        guard flowGeneration == generation else { return }
        do {
            let state = try AppBridge.flow(["operation": "poll", "generation": generation])
            guard state["current"] as? Bool == true else {
                finishCapture(); status.stringValue = "Capture cancelled (Escape or desktop session unavailable)."; return
            }
            guard !preparingRegion, !preparingWindow, !preparingUnified, !preparingRecording,
                  regionPanel == nil, windowPanel == nil, unifiedPanel == nil else { return }
            guard let remaining = state["remaining"] as? Int else { throw AppBridgeError.invalidResponse }
            countdownPanel?.countdownContent.setRemaining(remaining)
            guard remaining == 0, !snapshotPending else { return }
            snapshotPending = true; countdownPanel?.close(); countdownPanel = nil
            if recordingPendingStart {
                startRecording(on: screen(for: display), generation: generation)
                return
            }
            if unifiedSession != nil { status.stringValue = "Capturing screenshot…" }
            else if windowSession != nil { status.stringValue = "Capturing window…" }
            else if regionSession != nil { status.stringValue = "Capturing region…" }
            else { status.stringValue = "Capturing display…" }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                guard let self, self.flowGeneration == generation else { return }
                self.run({ [transport, historyRoot, regionSession, regionRect, windowSession, windowTarget,
                            unifiedSession, unifiedTarget] in
                    if let unifiedSession, let unifiedTarget {
                        return try unifiedSession.capture(root: historyRoot, target: unifiedTarget,
                            afterCountdown: preferences.countdown > 0)
                    }
                    if let windowSession, let windowTarget {
                        return try windowSession.capture(root: historyRoot, target: windowTarget,
                            afterCountdown: preferences.countdown > 0)
                    }
                    if let regionSession, let regionRect {
                        return try regionSession.capture(root: historyRoot, rect: regionRect, afterCountdown: preferences.countdown > 0)
                    }
                    let result = try transport.request(["operation": "capture_display", "root": historyRoot,
                        "display_id": display.id, "generation": generation, "include_cursor": preferences.includeCursor])
                    guard let value = result["artifact"] as? [String: Any], let artifact = CaptureArtifact(value) else { throw AppBridgeError.invalidResponse }
                    return artifact
                }) { [weak self] result in
                    guard let self, self.flowGeneration == generation else { return }
                    switch result { case .success(let artifact):
                        let previewGeneration = self.previewCaptureGeneration
                        self.previewCaptureGeneration = nil
                        self.finishCapture(restorePreview: false)
                        self.miniPreviews?.present(artifact, on: display.id,
                            settings: preferences.miniPreviewSettings,
                            generation: previewGeneration)
                        self.loadHistory(select: artifact.id)
                        if preferences.autoCopy { self.copyImage(at: artifact.imagePath) }
                    case .failure(let error):
                        self.finishCapture(); self.showError("Capture failed", error)
                    }
                }
            }
        } catch { finishCapture(); showError("Capture failed", error) }
    }

    private func startRecording(on screen: NSScreen?, generation: UInt64) {
        guard let session = recordingSession, let screen else {
            finishCapture(); showError("Recording failed",
                AppBridgeError.backend("The selected display is no longer available."))
            return
        }
        status.stringValue = "Starting recording…"
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self, self.flowGeneration == generation else { return }
            self.run({ [recordingGate, recordingCapabilities] in
                try session.start(generation: generation,
                    excludeCapturesApp: recordingCapabilities?.controlsExcluded == true,
                    gate: recordingGate)
            }) { [weak self] result in
                guard let self, self.flowGeneration == generation else { return }
                do {
                    let snapshot = try result.get()
                    do {
                        _ = try AppBridge.flow(["operation": "disarm_escape",
                            "generation": generation])
                    } catch {
                        self.discardRecording()
                        self.showError("Recording start was cancelled", error)
                        return
                    }
                    self.recordingPendingStart = false
                    self.finishSelectorForRecording(generation: generation)
                    let hud = RecordingHUDPanel(screen: screen, tokens: self.tokens,
                        excludedFromCapture: self.recordingCapabilities?.controlsExcluded == true)
                    hud.hud.pauseOrResume = { [weak self] in self?.pauseOrResumeRecording() }
                    hud.hud.restart = { [weak self] in self?.confirmRestartRecording() }
                    hud.hud.stop = { [weak self] in self?.stopRecording() }
                    hud.hud.discard = { [weak self] in self?.discardRecording() }
                    hud.hud.setPaused(false, elapsedMilliseconds: snapshot.elapsedMilliseconds)
                    hud.hud.setWarning(snapshot.warning)
                    self.recordingHUD = hud; hud.orderFrontRegardless()
                    self.status.stringValue = snapshot.warning ?? "Recording in progress…"
                    self.startRecordingPolling()
                } catch {
                    self.recordingSession = nil
                    Self.queue.async { withExtendedLifetime(session) {} }
                    self.finishCapture(); self.showError("Recording failed to start", error)
                }
            }
        }
    }

    private func finishSelectorForRecording(generation: UInt64) {
        selectorShortcutGeneration = nil
        countdownTimer?.invalidate(); countdownTimer = nil
        countdownPanel?.close(); countdownPanel = nil
        unifiedSession = nil; unifiedTarget = nil; unifiedDisplay = nil; unifiedScreen = nil
        // Keep the disarmed flow as the single process-wide owner until the
        // recording is finalized or discarded. Only selector shortcuts end.
        guard flowGeneration == generation else { return }
        miniPreviews?.restoreCapture(generation: previewCaptureGeneration)
        previewCaptureGeneration = nil
        snapshotPending = false
    }

    private func pauseOrResumeRecording() {
        guard let session = recordingSession, let hud = recordingHUD,
              let generation = activeRecordingGeneration,
              recordingLifecycle.begin() else { return }
        hud.hud.setLifecycleActionsEnabled(false)
        let wasPaused = hud.hud.paused
        let excludeCapturesApp = recordingCapabilities?.controlsExcluded == true
        status.stringValue = wasPaused ? "Resuming recording…" : "Pausing recording…"
        run({ [recordingGate] in
            if wasPaused {
                return try session.start(generation: generation,
                    excludeCapturesApp: excludeCapturesApp,
                    gate: recordingGate)
            }
            return try session.pause()
        }) { [weak self] result in
            guard let self, self.recordingSession === session else { return }
            self.recordingLifecycle.end()
            hud.hud.setLifecycleActionsEnabled(true)
            do {
                let snapshot = try result.get()
                guard snapshot.state != "failed" else {
                    self.preserveFailedRecording(session, warning: snapshot.warning)
                    return
                }
                if wasPaused {
                    do {
                        _ = try AppBridge.flow(["operation": "disarm_escape",
                            "generation": generation])
                    } catch {
                        self.stopRecording()
                        self.showError("Recording resume was cancelled", error)
                        return
                    }
                }
                hud.hud.setPaused(snapshot.state == "paused",
                    elapsedMilliseconds: snapshot.elapsedMilliseconds)
                self.status.stringValue = snapshot.warning
                    ?? (snapshot.state == "paused" ? "Recording paused." : "Recording in progress…")
            } catch {
                if wasPaused {
                    // Resume cancellation retains the already accepted segments.
                    // Finalize them rather than treating the whole take as disposable.
                    self.stopRecording()
                    self.showError("Couldn’t resume recording; saving the existing take", error)
                } else {
                    // A platform pause failure transitions the shared owner to Failed.
                    self.preserveFailedRecording(session, warning: error.localizedDescription)
                }
            }
        }
    }

    private func startRecordingPolling() {
        recordingPollTimer?.invalidate()
        let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            self?.pollRecording()
        }
        recordingPollTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private func restartRecording() {
        guard let session = recordingSession, let hud = recordingHUD,
              let generation = activeRecordingGeneration,
              let display = recordingDisplay, let preferences = recordingPreferences,
              recordingLifecycle.begin() else { return }
        do {
            _ = try AppBridge.flow([
                "operation": "restart_countdown", "generation": generation,
                "seconds": preferences.recording.countdown,
            ])
        } catch {
            recordingLifecycle.end()
            showError("Couldn’t restart recording", error)
            return
        }

        hud.hud.setLifecycleActionsEnabled(false)
        hud.orderOut(nil)
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        recordingPendingStart = true
        snapshotPending = false
        status.stringValue = "Restarting recording…"
        run({ try session.restart() }) { [weak self] result in
            guard let self, self.recordingSession === session else { return }
            do {
                let snapshot = try result.get()
                let flow = try AppBridge.flow(["operation": "poll", "generation": generation])
                guard flow["current"] as? Bool == true else {
                    self.recordingLifecycle.end()
                    self.discardRecording()
                    return
                }
                self.recordingHUD?.close(); self.recordingHUD = nil
                if preferences.recording.countdown > 0,
                   let screen = self.screen(for: display) {
                    let countdown = ScreenshotCountdownPanel(screen: screen, tokens: self.tokens,
                        remaining: preferences.recording.countdown)
                    self.countdownPanel = countdown; countdown.orderFrontRegardless()
                }
                let tick: () -> Void = { [weak self] in
                    self?.tickCountdown(display: display, preferences: preferences,
                        generation: generation)
                }
                let timer = Timer(timeInterval: 0.1, repeats: true) { _ in tick() }
                self.countdownTimer = timer; RunLoop.main.add(timer, forMode: .common)
                self.recordingLifecycle.end()
                self.status.stringValue = snapshot.warning
                    ?? "Recording ready. Press Escape to cancel."
                tick()
            } catch {
                self.preserveFailedRecording(session, warning: error.localizedDescription)
            }
        }
    }

    private func confirmRestartRecording() {
        guard !recordingLifecycle.busy else { return }
        let alert = NSAlert()
        alert.messageText = "Restart recording?"
        alert.informativeText =
            "The current recording will be deleted and a new countdown will begin."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Restart")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        restartRecording()
    }

    private func pollRecording() {
        guard !recordingPollPending, !recordingLifecycle.busy,
              let session = recordingSession,
              let generation = activeRecordingGeneration else { return }
        do {
            let flow = try AppBridge.flow(["operation": "poll", "generation": generation])
            guard flow["current"] as? Bool == true else {
                stopRecording()
                status.stringValue = "Recording stopped because the desktop session changed. Saving…"
                return
            }
        } catch {
            stopRecording()
            showError("Recording session monitoring failed; saving the recording", error)
            return
        }
        recordingPollPending = true
        run({ try session.snapshot() }) { [weak self] result in
            guard let self else { return }
            self.recordingPollPending = false
            guard self.recordingSession === session else { return }
            switch result {
            case .success(let snapshot):
                if snapshot.state == "failed" {
                    self.preserveFailedRecording(session, warning: snapshot.warning)
                    return
                }
                self.recordingHUD?.hud.setPaused(snapshot.state == "paused",
                    elapsedMilliseconds: snapshot.elapsedMilliseconds)
                self.recordingHUD?.hud.setWarning(snapshot.warning)
                if let warning = snapshot.warning { self.status.stringValue = warning }
            case .failure(let error):
                self.recordingPollTimer?.invalidate(); self.recordingPollTimer = nil
                self.showError("Couldn’t refresh recording status", error)
            }
        }
    }

    private func preserveFailedRecording(_ session: NativeRecordingSession, warning: String?) {
        guard recordingSession === session else { return }
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        recordingGate.set(nil); activeRecordingGeneration = nil
        recordingLifecycle.end()
        recordingSession = nil
        recordingHUD?.close(); recordingHUD = nil
        Self.queue.async { withExtendedLifetime(session) {} }
        finishCapture()
        showError("Recording stopped; recovery files were preserved",
            AppBridgeError.backend(warning ?? "The recording engine stopped unexpectedly."))
    }

    private func stopRecording() {
        guard let session = recordingSession, recordingLifecycle.begin() else { return }
        recordingHUD?.hud.setLifecycleActionsEnabled(false)
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        status.stringValue = "Finalizing recording…"
        recordingHUD?.hud.isHidden = true
        run({ [historyRoot] in
            _ = try session.stop()
            return try session.finish(historyRoot: historyRoot, tools: NativeMediaTools.locate())
        }) { [weak self] result in
            guard let self, self.recordingSession === session else { return }
            switch result {
            case .success(let finalized):
                self.recordingSession = nil
                Self.queue.async { withExtendedLifetime(session) {} }
                self.recordingHUD?.close(); self.recordingHUD = nil
                self.recordingGate.set(nil); self.activeRecordingGeneration = nil
                self.recordingLifecycle.end()
                self.finishCapture()
                let finalStatus = finalized.warning
                    ?? "Recording saved to \(finalized.path)"
                self.loadHistory(select: finalized.id) { [weak self] in
                    guard let self,
                          self.artifacts.contains(where: { $0.id == finalized.id }) else { return }
                    self.status.stringValue = finalStatus
                }
            case .failure(let error):
                self.preserveFailedRecording(session, warning: error.localizedDescription)
            }
        }
    }

    private func discardRecording() {
        guard let session = recordingSession, recordingLifecycle.begin() else { return }
        recordingHUD?.hud.setLifecycleActionsEnabled(false)
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        status.stringValue = "Discarding recording…"
        recordingHUD?.hud.isHidden = true
        run({ try session.discard() }) { [weak self] result in
            guard let self, self.recordingSession === session else { return }
            switch result {
            case .success:
                self.recordingSession = nil
                Self.queue.async { withExtendedLifetime(session) {} }
                self.recordingHUD?.close(); self.recordingHUD = nil
                self.recordingGate.set(nil); self.activeRecordingGeneration = nil
                self.recordingLifecycle.end()
                self.finishCapture(); self.status.stringValue = "Recording discarded."
            case .failure(let error):
                self.recordingLifecycle.end()
                if let hud = self.recordingHUD {
                    hud.hud.isHidden = false
                    hud.hud.setLifecycleActionsEnabled(true)
                    self.showError("Couldn’t discard recording", error)
                } else {
                    self.preserveFailedRecording(session, warning: error.localizedDescription)
                }
            }
        }
    }

    func finishCapture(restoreWindow: Bool = true, restorePreview: Bool = true) {
        if let session = recordingSession {
            recordingPollTimer?.invalidate(); recordingPollTimer = nil
            recordingGate.set(nil); activeRecordingGeneration = nil
            recordingLifecycle.end()
            recordingSession = nil
            recordingHUD?.close(); recordingHUD = nil
            if recordingPendingStart {
                Self.queue.async { _ = try? session.discard() }
            } else {
                let root = historyRoot
                Self.queue.async {
                    _ = try? session.stop()
                    if let tools = try? NativeMediaTools.locate() {
                        _ = try? session.finish(historyRoot: root, tools: tools)
                    }
                }
            }
        }
        selectorShortcutGeneration = nil
        countdownTimer?.invalidate(); countdownTimer = nil
        countdownPanel?.close(); countdownPanel = nil
        regionPanel?.close(); regionPanel = nil
        windowPanel?.close(); windowPanel = nil
        unifiedPanel?.close(); unifiedPanel = nil
        unifiedPreparation.invalidate()
        regionSession = nil; regionRect = nil; preparingRegion = false
        windowSession = nil; windowTarget = nil; preparingWindow = false
        unifiedSession = nil; unifiedTarget = nil; unifiedDisplay = nil; unifiedScreen = nil
        unifiedControlsState = .initial
        preparingUnified = false; preparingRecording = false; recordingPendingStart = false
        recordingDisplay = nil; recordingPreferences = nil
        recordingCapabilities = nil; microphoneDevices = []
        if let generation = flowGeneration {
            _ = try? AppBridge.flow(["operation": "finish", "generation": generation])
            flowGeneration = nil
        }
        if restorePreview {
            miniPreviews?.restoreCapture(generation: previewCaptureGeneration)
            previewCaptureGeneration = nil
        }
        snapshotPending = false; setBusy(false)
        switch windowRestoration.finish(restoreRequested: restoreWindow) {
        case .none: break
        case .visible:
            window.orderFront(nil)
        case .key:
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    @discardableResult func selectUnifiedTargetFromShortcut(_ shortcut: CaptureShortcut) -> Bool {
        guard let panel = unifiedPanel, selectorShortcutGeneration == flowGeneration,
              let target = shortcut.target else { return false }
        panel.selector.setTargetFromShortcut(target, mode: shortcut.mode)
        return true
    }

    private func setBusy(_ busy: Bool, message: String = "") {
        capturing = busy
        captureStateChanged(busy)
        updateActions()
        if busy { status.stringValue = message }
    }
    func showShortcutError(_ error: Error) {
        status.stringValue = "Capture shortcuts unavailable: \(error.localizedDescription)"
        status.textColor = tokens.color("danger-text")
    }
    private func showError(_ context: String, _ error: Error) {
        let message = "\(context): \(error.localizedDescription)"
        status.stringValue = message
        status.textColor = tokens.color("danger-text")
        reportError(message)
    }
    private func updateActions() {
        let selected = selectedIndex.map { artifacts.indices.contains($0) } == true
        let selectedScreenshot = selectedIndex.map { artifacts.indices.contains($0) && !artifacts[$0].isRecording } == true
        let busy = capturing || clearingHistory
        saveButton?.isEnabled = selectedScreenshot && !busy
        copyButton?.isEnabled = selectedScreenshot && selectedImage != nil && !busy
        deleteButton?.isEnabled = selected && !busy
        revealButton?.isEnabled = selected && selectedIndex.flatMap {
            artifacts[$0].mediaPath ?? artifacts[$0].savedPath
        } != nil
        clearHistoryButton?.isEnabled = artifacts.contains { !$0.isRecording } && !busy
        captureButton?.isEnabled = !busy && !displays.isEmpty && !historyRoot.isEmpty
        regionButton?.isEnabled = !busy && !displays.isEmpty && !historyRoot.isEmpty
        windowButton?.isEnabled = !busy && !displays.isEmpty && !historyRoot.isEmpty
        newCaptureButton?.isEnabled = !busy && !displays.isEmpty && !historyRoot.isEmpty
    }

    func numberOfRows(in tableView: NSTableView) -> Int { artifacts.count }
    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let view = CaptureHistoryRow(); view.tokens = tokens; return view
    }
    func tableViewSelectionDidChange(_ notification: Notification) { select(table.selectedRow) }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView(); let artifact = artifacts[row]
        let name = NSTextField(labelWithString: "\(artifact.isRecording ? "Recording" : "Screenshot") · \(artifact.width) × \(artifact.height)")
        name.frame = NSRect(x: 12, y: 31, width: 280, height: 20); name.font = .systemFont(ofSize: 13, weight: .medium); name.textColor = tokens.color("text")
        let date = NSTextField(labelWithString: artifact.createdAt); date.frame = NSRect(x: 12, y: 10, width: 280, height: 18); date.font = .systemFont(ofSize: 11); date.textColor = tokens.color("text-muted")
        cell.addSubview(name); cell.addSubview(date); cell.textField = name; return cell
    }

    private func select(_ index: Int) {
        selectionGeneration += 1
        let generation = selectionGeneration
        selectedImage = nil; preview.image = nil
        guard artifacts.indices.contains(index) else { clearSelection(); return }
        selectedIndex = index; let artifact = artifacts[index]
        detail.stringValue = artifact.isRecording
            ? "Loading recording poster…" : "Loading \(artifact.width) × \(artifact.height) PNG…"
        updateActions()
        run({ () throws -> NSImage in
            guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: artifact.imagePath) as CFURL, nil),
                  let image = CGImageSourceCreateImageAtIndex(source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
            else { throw AppBridgeError.invalidResponse }
            return NSImage(cgImage: image, size: NSSize(width: CGFloat(image.width), height: CGFloat(image.height)))
        }) { [weak self] result in
            guard let self, self.selectionGeneration == generation else { return }
            switch result { case .success(let image):
                self.selectedImage = image; self.preview.image = image
                self.detail.stringValue = artifact.isRecording
                    ? "\(artifact.width) × \(artifact.height) · H.264 MP4 · Editor unavailable"
                    : "\(artifact.width) × \(artifact.height) · PNG"
            case .failure(let error):
                self.showError(artifact.isRecording
                    ? "Couldn’t decode recording poster" : "Couldn’t decode screenshot", error)
                self.detail.stringValue = "Preview unavailable"
            }
            self.updateActions()
        }
    }
    private func clearSelection() {
        selectionGeneration += 1; selectedIndex = nil; selectedImage = nil; preview?.image = nil
        if table.selectedRow >= 0 { table.deselectAll(nil) }
        detail?.stringValue = artifacts.isEmpty ? "Choose New Capture to begin." : "Select a capture to preview it."
        updateActions()
    }

    private func save() {
        guard let index = selectedIndex, artifacts.indices.contains(index) else { return }; let artifact = artifacts[index]
        save(artifact)
    }
    private func save(_ artifact: CaptureArtifact) {
        status.stringValue = "Saving image…"
        miniPreviews?.setStatus("Saving…", for: artifact.id)
        run({ [transport, historyRoot, settingsPath] in
            let preferences = try CapturePreferences.load(path: settingsPath)
            let result = try transport.request(["operation": "save_screenshot", "root": historyRoot, "id": artifact.id,
                "directory": preferences.directory, "format": preferences.format])
            guard let value = result["artifact"] as? [String: Any], let updated = CaptureArtifact(value), let path = result["path"] as? String else { throw AppBridgeError.invalidResponse }
            return (updated, path)
        }) { [weak self] result in
            guard let self else { return }
            switch result { case .success(let value):
                if let current = self.artifacts.firstIndex(where: { $0.id == artifact.id }) { self.artifacts[current] = value.0 }
                self.status.stringValue = "Saved image to \(value.1)"; self.table.reloadData()
                self.miniPreviews?.setStatus("Saved", for: artifact.id)
            case .failure(let error):
                self.showError("Couldn’t save image", error)
                self.miniPreviews?.setStatus("Save failed", for: artifact.id)
            }; self.updateActions()
        }
    }
    private func copyImage() {
        guard let index = selectedIndex, artifacts.indices.contains(index) else { return }
        copyImage(at: artifacts[index].imagePath)
    }
    private func copyImage(at path: String) {
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
    func openPreview(_ artifact: CaptureArtifact) {
        window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        loadHistory(select: artifact.id)
    }
    func refreshHistory() { loadHistory() }
    private func reveal() {
        guard let index = selectedIndex, artifacts.indices.contains(index),
              let path = artifacts[index].mediaPath ?? artifacts[index].savedPath else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
    private func confirmDelete() {
        guard let index = selectedIndex, artifacts.indices.contains(index) else { return }; let artifact = artifacts[index]
        let alert = NSAlert()
        alert.messageText = "Delete this \(artifact.isRecording ? "recording" : "screenshot") from history?"
        alert.informativeText = "This removes the native history copy. Exported files stay on disk."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete from History"); alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in guard response == .alertFirstButtonReturn else { return }; self?.delete(artifact) }
    }
    private func delete(_ artifact: CaptureArtifact) {
        status.stringValue = "Deleting from history…"
        run({ [transport, historyRoot] in _ = try transport.request(["operation": "delete", "root": historyRoot, "id": artifact.id]) }) { [weak self] result in
            switch result { case .success: self?.loadHistory()
            case .failure(let error): self?.showError("Couldn’t delete capture", error) }
        }
    }

    private func confirmClearHistory() {
        guard !artifacts.isEmpty, !capturing, !clearingHistory else { return }
        let alert = NSAlert(); alert.messageText = "Clear screenshot history?"
        alert.informativeText = "This deletes all screenshots in native history. Exported files stay on disk."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete All"); alert.addButton(withTitle: "Cancel")
        alert.buttons[0].keyEquivalent = ""; alert.buttons[1].keyEquivalent = "\r"
        alert.beginSheetModal(for: window) { [weak self] response in
            guard response == .alertFirstButtonReturn, let self else { return }
            self.clearingHistory = true; self.updateActions(); self.status.stringValue = "Clearing history…"
            self.run({ [transport = self.transport, historyRoot = self.historyRoot] in
                _ = try transport.request(["operation": "clear_history", "root": historyRoot])
            }) { [weak self] result in
                guard let self else { return }
                // A failed bulk delete can still remove some entries; always reload.
                self.loadHistory { [weak self] in
                    guard let self else { return }
                    self.clearingHistory = false; self.updateActions()
                    if case .failure(let error) = result { self.showError("Couldn’t clear history", error) }
                }
            }
        }
    }

    private func run<T>(_ work: @escaping () throws -> T, completion: @escaping (Result<T, Error>) -> Void) {
        status.textColor = tokens.color("text-muted")
        Self.queue.async { let result = Result(catching: work); DispatchQueue.main.async { completion(result) } }
    }

    // One process-wide queue also drains operations from a closed workspace view.
    static func flush() { queue.sync {} }
}
