import AppKit
import ImageIO
import CCapturesSettings

enum CaptureHistoryFilter: String, CaseIterable {
    case all, screenshot, video, gif

    var title: String {
        switch self {
        case .all: return "All"
        case .screenshot: return "Screenshots"
        case .video: return "Video"
        case .gif: return "GIF"
        }
    }

    func matches(_ artifact: CaptureArtifact) -> Bool {
        self == .all || artifact.kind == rawValue
    }
}

enum StillCaptureKind: Equatable {
    case display
    case region
    case window
}

/// A capture that waits for a fresh display list before it starts.
enum PendingDisplayCapture: Equatable {
    case still(StillCaptureKind)
    case menu(recordingTarget: UnifiedCaptureTarget?, screenshotTarget: UnifiedCaptureTarget?)

    /// The capture a failure would retry after a relaunch. New Capture and
    /// recordings retry as Region, like shipping `report_capture_error`.
    var retryKind: StillCaptureKind {
        switch self {
        case .still(let kind): return kind
        case .menu: return .region
        }
    }
}

/// Shipping looks the display up for every capture
/// (`capture_display_at_point`). List again when the list is empty or lacks
/// the display under the pointer, so a failed or stale list recovers without
/// a relaunch.
func displayListNeedsRefresh(listed: [String], pointer: String?) -> Bool {
    guard !listed.isEmpty else { return true }
    guard let pointer else { return false }
    return !listed.contains(pointer)
}

/// A running or paused take accepts one region, window or display screenshot
/// at a time; a take that is counting down, finalizing or busy with an action
/// does not (shipping `screenshot_capture_is_blocked`).
func displayScreenshotAvailableDuringRecording(routeState: String?, screenshotActive: Bool,
                                               lifecycleBusy: Bool) -> Bool {
    (routeState == "recording" || routeState == "paused") && !screenshotActive && !lifecycleBusy
}

/// Shipping hides the recording controls for a screenshot unless they are
/// opted into captures (`conceal_capture_chrome_for_snapshot`).
func recordingDisplayScreenshotHidesControls(includeControls: Bool) -> Bool {
    !includeControls
}

/// A region or window screenshot beside a running recording: the selector,
/// the display it opens on and the preferences it uses.
private struct RecordingSelectorShot {
    let kind: StillCaptureKind
    let display: DisplayItem
    let screen: NSScreen
    let preferences: CapturePreferences
}

/// A display screenshot taken beside a running recording.
private struct RecordingDisplayShot {
    let display: DisplayItem
    let screen: NSScreen
    let preferences: CapturePreferences
}

/// The capture UI an action puts in place of the open one, on a fresh
/// snapshot that shows it (`captures_app::capture_error::busy_route`).
private enum Recapture {
    case selector(StillCaptureKind)
    case menu(record: Bool, target: UnifiedCaptureTarget)
    /// Beside a running take: the display under the pointer, at once.
    case display
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

final class LiveCaptureController: NSObject {
    private let root: Surface
    private let window: NSWindow
    private let tokens: Tokens
    private let transport: AppTransport
    private let recoveryWorker: RecordingRecoveryWorking
    static let queue = DispatchQueue(label: "es.captures.native.capture", qos: .userInitiated)
    private let historyRootOverride: String?
    private let settingsPath: String?
    private let showPreferenceSetting: (String) -> Void
    private var permissionsVisible = false
    private let captureStateChanged: (Bool) -> Void
    private let selectorGenerationChanged: (UInt64?) -> Void
    private let recordingControlsVisibilityChanged: (Bool) -> Void
    private let reportError: (String) -> Void
    /// Shipping Screen Recording recovery for a denied capture. Returns true
    /// when it handled the failure (the host offers Restart & Retry).
    private let screenPermissionDenied: (StillCaptureKind, String) -> Bool
    /// The still capture a failure would retry after a relaunch. New Capture
    /// and recordings retry as Region, like shipping `report_capture_error`.
    private var captureAttemptKind: StillCaptureKind?
    /// A capture (or a permission-relaunch retry) waiting on a display list.
    private var pendingDisplayCapture: PendingDisplayCapture?
    private weak var miniPreviews: MiniPreviewController?
    private weak var miniPreviewActions: MiniPreviewActions?
    private let initialSelectionID: String?
    private var historyRoot = ""
    private var displays: [DisplayItem] = []
    private var artifacts: [CaptureArtifact] = []
    private var historyFilter = CaptureHistoryFilter.all
    private var historyFilterButtons: [(CaptureHistoryFilter, HistoryFilterPill)] = []
    private var historyRows: [Int] = []
    private var historyGeneration = 0
    private var recoveryDrafts: [RecordingRecoveryDraft] = []
    private var recoveryError: String?
    private var recoveryActionError: String?
    private var recoveryGeneration = 0
    private var recoveryLoading = false
    private var recoveryBusy = false
    /// Shipping's inline confirmation: the session whose Discard was pressed
    /// once and now reads "Discard permanently?" (a second press deletes).
    private(set) var recoveryDiscardArmedID: String?
    private var recoveryCancel: NativeRecordingEditorCancel?
    private var recoveryStage = ""
    private var recoveryActionGeneration = 0
    private var recordingRetiring = false
    private var selectedIndex: Int?
    private var userSelectionGeneration = 0
    private var reloadingHistorySelection = false
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
    /// The menu's Screenshot target as its shortcuts and tray items last set
    /// it, or nil in Record mode. Like shipping's selection summary
    /// (`open_menu_screenshot_target`), picks inside the menu leave it as is.
    private var unifiedRouteTarget: UnifiedCaptureTarget?
    private var recordingCapabilities: NativeRecordingCapabilities?
    private var microphoneDevices: [NativeMicrophoneDevice] = []
    /// This menu's devices were enumerated; a display switch keeps them, as
    /// shipping's selection session does.
    private var microphonesLoaded = false
    private var recordingControlState = RecordingControlState(framesPerSecond: 60,
        maxResolution: "original", showCursor: true, highlightClicks: false,
        systemAudio: false, microphoneDeviceID: nil)
    private let recordingGate = RecordingGenerationGate()
    private var recordingSession: NativeRecordingSession?
    private var recordingHUD: RecordingHUDPanel?
    private var recordingMeter: RecordingMicrophoneSampler?
    private var recordingRegionPanel: RecordingRegionPanel?
    private var recordingHiddenNotice: RecordingControlsHiddenNoticePanel?
    private var recordingHiddenNoticeTimer: Timer?
    private lazy var recordingSavedNotice: RecordingSavedNoticeController = {
        let controller = RecordingSavedNoticeController(tokens: tokens)
        controller.save = { [weak self] id, completion in
            guard let self, let artifact = self.artifacts.first(where: { $0.id == id }) else {
                completion(.failure(AppBridgeError.backend("The recording is no longer in Capture History.")))
                return
            }
            self.save(artifact, completion: completion)
        }
        return controller
    }()
    private var screenshotEditor: ScreenshotEditorController?
    private var recordingEditor: RecordingEditorController?
    private var editorTerminationPending = false
    private var pendingOpenImages: [String] = []
    private(set) var externalOpenPending = false
    private var externalOpenErrors: [String] = []
    private(set) var recordingControlsHidden = false
    private var recordingPollTimer: Timer?
    private var recordingPollPending = false
    private var recordingLifecycle = RecordingLifecycleGate()
    private var recordingScreenshotGeneration: UInt64?
    private var recordingScreenshotSession: NativeRegionSession?
    private var recordingScreenshotPanel: RegionSelectionPanel?
    private var recordingScreenshotRect: CapturesSelectionRect?
    private var recordingScreenshotWindowSession: NativeWindowSession?
    private var recordingScreenshotWindowPanel: WindowSelectionPanel?
    private var recordingScreenshotWindowTarget: WindowSelectionChoice?
    private var recordingScreenshotTimer: Timer?
    private var recordingScreenshotCountdownPanel: ScreenshotCountdownPanel?
    private var recordingScreenshotSnapshotPending = false
    private var recordingScreenshotPreviewGeneration: UInt64?
    /// Set while the recording screenshot is a display screenshot.
    private var recordingDisplayShot: RecordingDisplayShot?
    /// The open selector's shot beside the take, for a recapture.
    private var recordingSelectorShot: RecordingSelectorShot?
    /// Preferences of the capture in flight; a recapture swaps in
    /// `recapturing()` ones, which the final capture honours.
    private var capturePreferences: CapturePreferences?
    /// The open capture UI a recapture keeps on screen, and in the new
    /// snapshot, until the UI replacing it is ready.
    private var recapturedPanels: [NSWindow] = []
    /// Last value published to the shortcut routes.
    private var publishedRecordingDisplayRoute = false
    private var preparingRecording = false
    private var recordingPendingStart = false
    private var activeRecordingGeneration: UInt64?
    private var recordingDisplay: DisplayItem?
    /// Saved-notice screens for recording editors opened after a take.
    private var recordingEditorNoticeScreens: [String: NSScreen] = [:]
    private var lastRecordingNoticeScreen: NSScreen?
    private var recordingPreferences: CapturePreferences?
    private var selectorShortcutGeneration: UInt64? {
        didSet {
            if oldValue != selectorShortcutGeneration {
                selectorGenerationChanged(selectorShortcutGeneration)
            }
        }
    }
    /// The display a capture starts on. Shipping has no display picker in
    /// History: every capture starts on the display under the pointer, and the
    /// capture menu switches displays.
    private var selectedDisplayIndex = 0
    private let historyCopy = HistoryCopy.current
    private static let thumbnailQueue = DispatchQueue(label: "es.captures.native.history-thumbnails", qos: .utility)
    private var grid: HistoryGridView!
    private var historyScroll: NSScrollView!
    private var emptyState: HistoryEmptyView!
    /// Shipping filtered-empty copy, shown in the grid area (not the shared
    /// status line, which display and save messages also write).
    private var filteredEmptyLabel: NSTextField!
    private var toolbarDivider: Surface!
    private var deleteAllButton: HistoryButton!
    private var deleteAllCancelButton: HistoryButton!
    private var shareSelectedButton: CaptureButton!
    private var historyLoaded = false
    /// Shared card presentation and bounded thumbnail residency, by artifact ID.
    private var cards: [String: HistoryCard] = [:]
    private var thumbnails: [String: NSImage] = [:]
    private var thumbnailKeys: [String: String] = [:]
    private var thumbnailFailures: [String: String] = [:]
    private var thumbnailRequests: Set<String> = []
    private static let thumbnailCacheLimit = 96
    /// Shipping two-step deletion: the armed card or Delete all, and their reverts.
    private var confirmDeleteID: String?
    private var confirmDeleteTimer: Timer?
    private var confirmDeleteAll = false
    private var confirmDeleteAllTimer: Timer?
    private var cardBusy: (id: String, action: HistoryCardAction)?
    /// Shipping "✓ Restored" on one card for `HistoryCopy.feedbackDuration`.
    private var restoredCardID: String?
    private var restoredCardReset: DispatchWorkItem?
    /// Shipping `HistoryCard` errors (`.history-card-error`) from a failed
    /// Restore or Edit restore, by artifact, until that card acts again.
    private var cardErrors: [String: String] = [:]
    private var recoveryPanel: Surface!
    private var recoveryScroll: NSScrollView!
    private var recoveryStatus: NSTextField!
    private var recoveryCancelButton: CaptureButton!
    private var recoveryRetryButton: CaptureButton!
    /// Shipping `.history-error`: the latest progress or error message, shown
    /// only while it is an error (`statusAlert`), between the filters and the
    /// grid. Shipping History has no status line.
    private var status: NSTextField!
    private var statusBackground: Surface!
    private var statusAlert = false
    private var eyebrowLabel: NSTextField!
    private var headingLabel: NSTextField!
    private var ledeLabel: NSTextField!
    /// Top of the filters row, `s-6` below the header (shipping `.history-shell` gap).
    private var contentTop: CGFloat = 136
    private var renderedRecoveryWidth: CGFloat = 0
    /// Called with true when a capture hides this window and false when the
    /// capture ends, so the host can hide its other windows too.
    var workspaceHidden: ((Bool) -> Void)?
    var showSharing: ((CaptureArtifact) -> Void)? { didSet { layoutHeaderActions() } }
    var nativeProfileRoot: String? { historyRoot.isEmpty ? nil : historyRoot }

    init(root: Surface, window: NSWindow, tokens: Tokens, historyRoot: String?, settingsPath: String?,
         transport: AppTransport = AppBridge(), recoveryWorker: RecordingRecoveryWorking = RecordingRecoveryWorker(),
         miniPreviews: MiniPreviewController? = nil,
         miniPreviewActions: MiniPreviewActions? = nil,
         initialSelectionID: String? = nil,
         captureStateChanged: @escaping (Bool) -> Void = { _ in },
         selectorGenerationChanged: @escaping (UInt64?) -> Void = { _ in },
         recordingControlsVisibilityChanged: @escaping (Bool) -> Void = { _ in },
         reportError: @escaping (String) -> Void = { _ in },
         screenPermissionDenied: @escaping (StillCaptureKind, String) -> Bool = { _, _ in false },
         showPreferenceSetting: @escaping (String) -> Void = { _ in }) {
        self.root = root; self.window = window; self.tokens = tokens
        historyRootOverride = historyRoot; self.transport = transport
        self.recoveryWorker = recoveryWorker
        self.showPreferenceSetting = showPreferenceSetting
        self.settingsPath = settingsPath; self.miniPreviews = miniPreviews
        self.miniPreviewActions = miniPreviewActions
        self.initialSelectionID = initialSelectionID
        self.captureStateChanged = captureStateChanged
        self.selectorGenerationChanged = selectorGenerationChanged
        self.recordingControlsVisibilityChanged = recordingControlsVisibilityChanged
        self.reportError = reportError
        self.screenPermissionDenied = screenPermissionDenied
        super.init(); build(); loadInitial()
        // Shipping History has no Refresh: History reloads after every change
        // it makes, and the display list follows display changes.
        NotificationCenter.default.addObserver(self, selector: #selector(displaysChanged),
            name: NSApplication.didChangeScreenParametersNotification, object: nil)
    }

    /// The History error card's message, when it is showing one.
    var historyError: String? { statusAlert ? status.stringValue : nil }

    /// The shipping recording state Screenshot Display is routed on, or nil
    /// without a recording session. The HUD mirrors the take's state.
    var recordingRouteState: String? {
        guard recordingSession != nil else { return nil }
        guard !recordingPendingStart, let hud = recordingHUD else { return "countdown" }
        return hud.hud.state
    }

    /// Where the display shortcut and tray "Screenshot Display" go now.
    var displayCaptureRoute: DisplayCaptureRoute {
        (try? DisplayCaptureRoute(recordingState: recordingRouteState))
            ?? (recordingSession == nil ? .captureMenu : .ignore)
    }

    /// Where New Capture goes now
    /// (`captures_app::capture_error::new_capture_route`).
    var newCaptureRoute: NewCaptureRoute {
        (try? NewCaptureRoute(recordingState: recordingRouteState,
                              controlsHidden: recordingControlsHidden))
            ?? (recordingControlsHidden ? .restoreControls : .captureMenu)
    }

    /// A running or paused take can take a region, window or display
    /// screenshot right now.
    var recordingDisplayScreenshotAvailable: Bool {
        displayScreenshotAvailableDuringRecording(routeState: recordingRouteState,
            screenshotActive: recordingScreenshotGeneration != nil,
            lifecycleBusy: recordingLifecycle.busy)
    }

    /// A capture or recording owns the flow, so every capture shortcut goes to
    /// the host's routes.
    var captureInFlight: Bool { capturing }

    /// What is open or in flight for `busy_route`.
    var captureActivity: CaptureActivity {
        if recordingScreenshotGeneration != nil {
            return recordingScreenshotPanel != nil || recordingScreenshotWindowPanel != nil
                ? .selector : .busy
        }
        if recordingSession != nil || !capturing { return .idle }
        // A menu showing "Starting…" or "Switching…" is busy, as before it
        // stayed up for them.
        if let panel = unifiedPanel, panel.selector.controls.inFlight == nil {
            return .menu(screenshotTarget: unifiedRouteTarget)
        }
        if regionPanel != nil || windowPanel != nil { return .selector }
        return .busy
    }

    /// Routes a capture shortcut or tray item while a capture is open or in
    /// flight (shipping `open_capture_controls` and `start_capture_inner`).
    func routeBusyCaptureAction(_ action: CaptureAction) -> BusyCaptureOutcome {
        // The open UI is already being recaptured.
        if !recapturedPanels.isEmpty { return .handled }
        let activity = captureActivity
        if activity == .idle { return .idle }
        let route = (try? BusyCaptureRoute(action: action, activity: activity,
            recordingState: recordingRouteState,
            controlsOnScreen: recordingHUD?.isVisible == true)) ?? .idle
        switch route {
        case .idle:
            return .idle
        case .restoreControls:
            // Shipping `restore_hidden_recording_controls` shows the HUD
            // window whenever it is off screen, including while a screenshot
            // beside the take conceals it.
            if recordingControlsHidden { _ = showRecordingControls() }
            else if let hud = recordingHUD { hud.orderFrontRegardless(); updateRecordingMeter() }
            return .handled
        case .inProgress(let message):
            return .inProgress(message: message)
        case .ignore:
            return .handled
        case .switchMenu(let record, let target):
            unifiedPanel?.selector.setTargetFromShortcut(target, mode: record ? .record : .screenshot)
            unifiedRouteTarget = record ? nil : target
            return .handled
        case .recaptureSelector(let kind):
            recapture(.selector(kind))
            return .handled
        case .recaptureDisplay:
            recapture(.display)
            return .handled
        case .recaptureMenu(let record, let target):
            recapture(.menu(record: record, target: target))
            return .handled
        }
    }

    /// Keeps the open capture UI up and in captures while a fresh snapshot
    /// of it is taken (shipping `include_capture_ui_in_snapshot`).
    private func holdForRecapture(_ panels: [NSWindow?]) {
        for case let panel? in panels {
            panel.sharingType = .readOnly
            recapturedPanels.append(panel)
        }
    }

    private func closeRecapturedPanels() {
        let panels = recapturedPanels
        recapturedPanels.removeAll()
        for panel in panels { panel.close() }
    }

    /// The display under the pointer, or `fallback`.
    private func pointerDisplay(or fallback: DisplayItem?) -> DisplayItem? {
        let pointer = pointerDisplayID()
        return displays.first(where: { $0.id == pointer }) ?? fallback
    }

    /// Freezes the display under the pointer with the open capture UI still on
    /// it and opens the requested UI on that snapshot in its place; beside a
    /// take, a display recapture saves that frame instead.
    private func recapture(_ kind: Recapture) {
        if recordingScreenshotGeneration != nil {
            recaptureBesideRecording(kind)
            return
        }
        if case .display = kind { return }
        let listed: DisplayItem? = displays.indices.contains(selectedDisplayIndex)
            ? displays[selectedDisplayIndex] : nil
        let fallback = unifiedDisplay ?? listed
        guard capturing, recordingSession == nil, let generation = flowGeneration,
              let preferences = capturePreferences?.recapturing(),
              let display = pointerDisplay(or: fallback), let screen = screen(for: display)
        else { return }
        holdForRecapture([regionPanel, windowPanel, unifiedPanel])
        regionPanel = nil; windowPanel = nil; unifiedPanel = nil
        selectorShortcutGeneration = nil
        regionSession = nil; regionRect = nil
        windowSession = nil; windowTarget = nil
        unifiedSession = nil; unifiedTarget = nil
        capturePreferences = preferences
        let menu: Bool
        if case .menu = kind { menu = true } else { menu = false }
        if !menu { unifiedDisplay = nil; unifiedScreen = nil }
        // The flow's poll follows the new UI's display and preferences, not
        // the replaced UI's.
        countdownTimer?.invalidate()
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.tickCountdown(display: menu ? (self.unifiedDisplay ?? display) : display,
                preferences: preferences, generation: generation)
        }
        countdownTimer = timer
        RunLoop.main.add(timer, forMode: .common)
        status.stringValue = "Capturing the open capture UI… Press Escape to cancel."
        switch kind {
        case .selector(.window):
            preparingWindow = true
            prepareWindow(display: display, screen: screen, preferences: preferences,
                generation: generation)
        case .selector:
            preparingRegion = true
            prepareRegion(display: display, screen: screen, preferences: preferences,
                generation: generation)
        case .menu(let record, let target):
            unifiedControlsState = .initial
            if record { unifiedControlsState.mode = .record }
            unifiedControlsState.target = target
            unifiedRouteTarget = record ? nil : target
            preparingUnified = true
            run({ [settingsPath] in
                let loaded = try CapturePreferences.load(path: settingsPath)
                let capabilities = try NativeRecordingInfo.capabilities(
                    includeControls: loaded.includeRecordingControlsInCaptures)
                return (loaded, capabilities)
            }) { [weak self] result in
                guard let self, self.flowGeneration == generation else { return }
                do {
                    let (loaded, capabilities) = try result.get()
                    self.applyMenuState(loaded, capabilities: capabilities)
                    self.prepareUnified(display: display, preferences: preferences,
                        generation: generation)
                } catch {
                    self.finishCapture()
                    self.showCaptureError("Couldn’t start New Capture", error)
                }
            }
        case .display:
            return
        }
    }

    /// Recaptures the selector of a screenshot beside the take: a new region
    /// or window selector on a snapshot that shows it, or the display at once.
    private func recaptureBesideRecording(_ kind: Recapture) {
        guard let generation = recordingScreenshotGeneration, let shot = recordingSelectorShot,
              recordingScreenshotPanel != nil || recordingScreenshotWindowPanel != nil,
              let display = pointerDisplay(or: shot.display), let screen = screen(for: display)
        else { return }
        if case .menu = kind { return }
        let preferences = shot.preferences.recapturing()
        holdForRecapture([recordingScreenshotPanel, recordingScreenshotWindowPanel])
        recordingScreenshotPanel = nil; recordingScreenshotWindowPanel = nil
        recordingScreenshotTimer?.invalidate(); recordingScreenshotTimer = nil
        recordingScreenshotSession = nil; recordingScreenshotRect = nil
        recordingScreenshotWindowSession = nil; recordingScreenshotWindowTarget = nil
        switch kind {
        case .display:
            recordingSelectorShot = nil
            recordingDisplayShot = RecordingDisplayShot(display: display, screen: screen,
                preferences: preferences)
            startRecordingDisplayCountdown(generation: generation)
        case .selector(let still):
            let next = RecordingSelectorShot(kind: still, display: display, screen: screen,
                preferences: preferences)
            recordingSelectorShot = next
            if still == .window {
                prepareRecordingWindowSelector(next, generation: generation, fromControls: false)
            } else {
                prepareRecordingRegionSelector(next, generation: generation, fromControls: false)
            }
        case .menu:
            return
        }
    }

    /// The capture menu's recording row for these preferences.
    private func applyMenuState(_ preferences: CapturePreferences,
                                capabilities: NativeRecordingCapabilities) {
        recordingCapabilities = capabilities
        // Like shipping, devices enumerate once the menu first shows Record.
        microphoneDevices = []; microphonesLoaded = false
        recordingControlState = RecordingControlState(
            framesPerSecond: preferences.recording.framesPerSecond,
            maxResolution: preferences.recording.maxResolution,
            showCursor: preferences.recording.showCursor,
            highlightClicks: preferences.recording.highlightClicks,
            systemAudio: preferences.recording.captureSystemAudio,
            microphoneDeviceID: capabilities.microphone
                ? preferences.recording.microphoneDeviceID : nil)
    }

    @objc private func displaysChanged() {
        guard !capturing, !historyRoot.isEmpty else { return }
        loadDisplays()
    }

    private func build() {
        let copy = historyCopy
        // Shipping `.history-header`: eyebrow, title and lede; Delete all on the right.
        let eyebrow = title(copy.eyebrow.uppercased(), frame: NSRect(x: 28, y: 24, width: 420, height: 14),
                            size: tokens.number("text-2xs"), weight: .semibold)
        eyebrow.textColor = tokens.color("text-subtle")
        eyebrowLabel = eyebrow
        headingLabel = title(copy.title, frame: NSRect(x: 28, y: 40, width: 420, height: 36),
                             size: tokens.number("text-3xl"), weight: .semibold)
        let lede = title(copy.lede, frame: NSRect(x: 28, y: 80, width: 600, height: 20),
                         size: tokens.number("text-md"))
        lede.textColor = tokens.color("text-subtle")
        ledeLabel = lede
        deleteAllButton = HistoryButton(copy.deleteAll, frame: .zero, tokens: tokens, style: .danger,
                                        glyph: .trash) { [weak self] in self?.deleteAllHistory() }
        deleteAllCancelButton = HistoryButton(copy.cancel, frame: .zero, tokens: tokens, style: .ghost) {
            [weak self] in self?.cancelHistoryConfirmations()
        }
        deleteAllCancelButton.setAccessibilityLabel(copy.cancelLabel)
        root.addSubview(deleteAllCancelButton); root.addSubview(deleteAllButton)
        shareSelectedButton = CaptureButton("Share selected capture…", frame: .zero, tokens: tokens) { [weak self] in
            guard let self, !self.historyBusy, self.cardBusy == nil, let index = self.selectedIndex,
                  self.artifacts.indices.contains(index) else { return }
            self.showSharing?(self.artifacts[index])
        }
        shareSelectedButton.icon = .shipping("share")
        root.addSubview(shareSelectedButton)

        // Shipping `.history-error`: danger text on a `danger-surface` card.
        statusBackground = Surface(frame: .zero)
        statusBackground.wantsLayer = true
        statusBackground.layer?.backgroundColor = tokens.color("danger-surface").cgColor
        statusBackground.layer?.cornerRadius = tokens.number("r-md")
        statusBackground.isHidden = true
        root.addSubview(statusBackground)
        status = title("Loading capture history…", frame: .zero, size: tokens.number("text-sm"), muted: true)
        status.isHidden = true

        for filter in CaptureHistoryFilter.allCases {
            let control = HistoryFilterPill(label: filter.title, tokens: tokens) { [weak self] in
                guard let self else { return }
                let previousID = self.selectedIndex.map { self.artifacts[$0].id }
                self.userSelectionGeneration += 1
                self.historyFilter = filter
                self.cancelHistoryConfirmations()
                self.reloadHistorySelection(previousID)
            }
            historyFilterButtons.append((filter, control)); root.addSubview(control)
        }
        toolbarDivider = Surface(frame: .zero)
        toolbarDivider.wantsLayer = true
        toolbarDivider.layer?.backgroundColor = tokens.color("border-subtle").cgColor
        root.addSubview(toolbarDivider)

        historyScroll = NSScrollView(frame: NSRect(x: 28, y: 246, width: 944, height: 300))
        historyScroll.hasVerticalScroller = true; historyScroll.autohidesScrollers = true
        historyScroll.useTokenScrollers(tokens)
        historyScroll.drawsBackground = false
        grid = HistoryGridView(tokens: tokens)
        grid.frame = NSRect(origin: .zero, size: historyScroll.contentSize)
        historyScroll.documentView = grid
        grid.onSelectionChange = { [weak self] in self?.gridSelectionChanged() }
        grid.onOpen = { [weak self] row in self?.performCard(row: row, action: .edit) }
        grid.onAction = { [weak self] row, action in self?.performCard(row: row, action: action) }
        grid.onDelete = { [weak self] row in self?.deleteCard(row: row) }
        grid.onNeedsThumbnail = { [weak self] row in self?.loadThumbnail(row: row) }
        grid.onPrepareDrag = { [weak self] row, completion in self?.prepareHistoryDrag(row: row, completion: completion) }
        grid.onDragError = { [weak self] row, error in self?.showHistoryDragError(row: row, error: error) }
        grid.onCancel = { [weak self] in self?.cancelHistoryConfirmations() }
        root.addSubview(historyScroll)
        emptyState = HistoryEmptyView(tokens: tokens)
        emptyState.show(title: copy.loading, body: nil)
        root.addSubview(emptyState)
        filteredEmptyLabel = NSTextField(labelWithString: "")
        filteredEmptyLabel.font = .systemFont(ofSize: tokens.number("text-md"))
        filteredEmptyLabel.textColor = tokens.color("text-subtle")
        filteredEmptyLabel.lineBreakMode = .byTruncatingTail
        filteredEmptyLabel.isHidden = true
        root.addSubview(filteredEmptyLabel)

        recoveryPanel = Surface(frame: NSRect(x: 28, y: 246, width: 944, height: 176))
        recoveryPanel.wantsLayer = true
        recoveryPanel.layer?.backgroundColor = tokens.color("surface-raised").cgColor
        recoveryPanel.layer?.cornerRadius = tokens.number("r-xl")
        recoveryPanel.layer?.borderWidth = 1
        recoveryPanel.layer?.borderColor = tokens.color("caution-surface").cgColor
        let heading = NSTextField(labelWithString: copy.recoveryTitle)
        heading.frame = NSRect(x: 16, y: 12, width: 600, height: 20)
        heading.font = .systemFont(ofSize: tokens.number("text-lg"), weight: .semibold)
        heading.textColor = tokens.color("text")
        recoveryPanel.addSubview(heading)
        let help = NSTextField(labelWithString: copy.recoveryHelp)
        help.frame = NSRect(x: 16, y: 36, width: 800, height: 17)
        help.font = .systemFont(ofSize: tokens.number("text-sm"))
        help.textColor = tokens.color("text-subtle")
        help.lineBreakMode = .byTruncatingTail
        help.autoresizingMask = [.width]
        recoveryPanel.addSubview(help)
        recoveryCancelButton = CaptureButton("Cancel", frame: NSRect(x: 842, y: 10, width: 86, height: 24),
                                             tokens: tokens) { [weak self] in self?.recoveryCancel?.cancel() }
        recoveryCancelButton.autoresizingMask = [.minXMargin]
        recoveryPanel.addSubview(recoveryCancelButton)
        recoveryRetryButton = CaptureButton("Retry list", frame: NSRect(x: 842, y: 10, width: 86, height: 24),
                                            tokens: tokens) { [weak self] in
            self?.recoveryActionError = nil; self?.refreshRecovery()
        }
        recoveryRetryButton.autoresizingMask = [.minXMargin]
        recoveryPanel.addSubview(recoveryRetryButton)
        recoveryScroll = NSScrollView(frame: NSRect(x: 12, y: 78, width: 920, height: 90))
        recoveryScroll.hasVerticalScroller = true; recoveryScroll.drawsBackground = false
        recoveryScroll.useTokenScrollers(tokens)
        recoveryScroll.setAccessibilityLabel("Interrupted recording details")
        recoveryScroll.autoresizingMask = [.width]
        recoveryPanel.addSubview(recoveryScroll)
        recoveryStatus = NSTextField(wrappingLabelWithString: "")
        recoveryStatus.font = .systemFont(ofSize: 11)
        recoveryStatus.textColor = tokens.color("text-muted")
        recoveryStatus.setAccessibilityLabel("Interrupted recording status")
        recoveryStatus.frame = NSRect(x: 16, y: 57, width: 900, height: 17)
        recoveryStatus.autoresizingMask = [.width]
        recoveryPanel.addSubview(recoveryStatus)
        root.addSubview(recoveryPanel)
        renderRecovery()
        updateActions()
        installKeyViewLoop()
        // The History window is resizable: lay the fixed-frame chrome out again.
        root.sizeDidChange = { [weak self] _ in self?.layoutForSize() }
        layoutForSize()
    }

    /// Reflow the header, capture row, recovery section and grid for the
    /// window's current size, down to its 640 × 440 minimum.
    private func layoutForSize() {
        guard recoveryPanel != nil else { return }
        recoveryPanel.frame.size.width = max(0, root.bounds.width - 56)
        layoutHeaderActions()
        layoutHistory()
        if !recoveryPanel.isHidden && renderedRecoveryWidth != recoveryScroll.contentSize.width {
            renderRecovery()
        }
    }

    /// Shipping DOM order: Cancel before Delete all, the filters, interrupted
    /// recordings, then the grid. The grid starts focused so its arrow keys
    /// work without a click.
    private func installKeyViewLoop() {
        guard let grid, let historyScroll, let recoveryPanel else { return }
        var order = root.subviews.filter { $0 !== recoveryPanel }
        order.insert(recoveryPanel, at: order.firstIndex { $0 === historyScroll } ?? order.endIndex)
        KeyViewLoop.install(order, window: window, initial: grid)
    }

    /// Stack the toolbar, error, recovery section and grid from the current root size.
    private func layoutHistory() {
        guard let historyScroll, let grid else { return }
        let width = root.bounds.width - 56
        let showToolbar = historyLoaded && !artifacts.isEmpty
        var x: CGFloat = 28
        for (_, pill) in historyFilterButtons {
            pill.isHidden = !showToolbar
            pill.frame = NSRect(x: x, y: contentTop, width: pill.preferredWidth, height: tokens.number("h-sm"))
            x += pill.preferredWidth + tokens.number("s-2")
        }
        toolbarDivider.isHidden = !showToolbar
        toolbarDivider.frame = NSRect(x: 28, y: contentTop + tokens.number("h-sm") + tokens.number("s-5"),
                                      width: width, height: 1)
        var y: CGFloat = showToolbar ? toolbarDivider.frame.maxY + tokens.number("s-6") : contentTop
        statusBackground.isHidden = !statusAlert
        status.isHidden = !statusAlert
        if statusAlert, let font = status.font {
            let insetX = tokens.number("s-5"), insetY = tokens.number("s-4")
            let textWidth = max(0, width - 2 * insetX)
            let height = textHeight(status.stringValue, font: font, width: textWidth)
            statusBackground.frame = NSRect(x: 28, y: y, width: width, height: height + 2 * insetY)
            status.frame = NSRect(x: 28 + insetX, y: y + insetY, width: textWidth, height: height)
            y = statusBackground.frame.maxY + tokens.number("s-6")
        }
        if !recoveryPanel.isHidden {
            recoveryPanel.frame.origin = NSPoint(x: 28, y: y)
            y = recoveryPanel.frame.maxY + tokens.number("s-6")
        }
        historyScroll.frame = NSRect(x: 28, y: y, width: width, height: max(0, root.bounds.height - y - 16))
        grid.compact = AppWindowLayout.compact(width: root.bounds.width)
        grid.tile(force: true)
        emptyState.frame = historyScroll.frame
        filteredEmptyLabel.frame = NSRect(x: historyScroll.frame.minX, y: historyScroll.frame.minY,
                                          width: width, height: 20)
    }

    /// Delete all sits at the header's bottom right, with Cancel while armed;
    /// a compact window stacks it under the heading (shipping
    /// `@media (max-width: 720px)`). Everything below follows the header.
    private func layoutHeaderActions() {
        guard let deleteAllButton else { return }
        let copy = historyCopy
        let busy = historyBusy
        deleteAllButton.isHidden = !historyLoaded || artifacts.isEmpty
        deleteAllButton.title = clearingHistory ? copy.deleteAllBusy
            : confirmDeleteAll ? copy.deleteAllConfirm : copy.deleteAll
        deleteAllButton.setAccessibilityLabel(confirmDeleteAll ? copy.deleteAllConfirmLabel : copy.deleteAllLabel)
        deleteAllButton.style = confirmDeleteAll ? .confirm : .danger
        deleteAllButton.isEnabled = !busy && !artifacts.isEmpty
        deleteAllCancelButton.isHidden = deleteAllButton.isHidden || !confirmDeleteAll
        deleteAllCancelButton.isEnabled = !clearingHistory
        let font = NSFont.systemFont(ofSize: tokens.number("text-sm"), weight: .medium)
        func width(_ text: String) -> CGFloat { ceil((text as NSString).size(withAttributes: [.font: font]).width) }
        let height = tokens.number("h-md"), left: CGFloat = 28, right = root.bounds.width - 28
        let compact = AppWindowLayout.compact(width: root.bounds.width)
        let deleteWidth = width(deleteAllButton.title) + 15 + tokens.number("s-3") + 2 * tokens.number("s-5")
        let cancelWidth = width(copy.cancel) + 2 * tokens.number("s-5")
        let gap = tokens.number("s-3")

        let headingWidth = max(0, min(420, right - left))
        eyebrowLabel?.frame = NSRect(x: left, y: 24, width: headingWidth, height: 14)
        headingLabel?.frame = NSRect(x: left, y: 40, width: headingWidth, height: 36)

        var headerBottom: CGFloat
        if compact {
            let ledeWidth = max(0, right - left)
            let ledeHeight = ledeLabel.map { max(20, textHeight(copy.lede, font: $0.font!, width: ledeWidth)) } ?? 20
            ledeLabel?.frame = NSRect(x: left, y: 80, width: ledeWidth, height: ledeHeight)
            headerBottom = 80 + ledeHeight
            if !deleteAllButton.isHidden {
                let y = headerBottom + tokens.number("s-5")
                var x = left
                if !deleteAllCancelButton.isHidden {
                    deleteAllCancelButton.frame = NSRect(x: x, y: y, width: cancelWidth, height: height)
                    x += cancelWidth + gap
                }
                deleteAllButton.frame = NSRect(x: x, y: y, width: deleteWidth, height: height)
                headerBottom = y + height
            }
        } else {
            deleteAllButton.frame = NSRect(x: right - deleteWidth, y: 70, width: deleteWidth, height: height)
            deleteAllCancelButton.frame = NSRect(x: deleteAllButton.frame.minX - gap - cancelWidth,
                                                 y: 70, width: cancelWidth, height: height)
            let actionsLeft = deleteAllButton.isHidden ? right + 12
                : deleteAllCancelButton.isHidden ? deleteAllButton.frame.minX : deleteAllCancelButton.frame.minX
            let ledeWidth = max(0, min(600, actionsLeft - 12 - left))
            let ledeHeight = ledeLabel.map { max(20, textHeight(copy.lede, font: $0.font!, width: ledeWidth)) } ?? 20
            ledeLabel?.frame = NSRect(x: left, y: 80, width: ledeWidth, height: ledeHeight)
            headerBottom = max(80 + ledeHeight, deleteAllButton.isHidden ? 0 : 70 + height)
        }

        shareSelectedButton.isHidden = !historyLoaded || selectedIndex == nil || showSharing == nil
        shareSelectedButton.isEnabled = !busy && cardBusy == nil
        if !shareSelectedButton.isHidden {
            shareSelectedButton.frame = NSRect(x: left, y: headerBottom + tokens.number("s-5"),
                width: min(right - left, width(shareSelectedButton.title) + 15 + gap + 2 * tokens.number("s-5")), height: height)
            headerBottom = shareSelectedButton.frame.maxY
        }
        let top = headerBottom + tokens.number("s-6")
        if top != contentTop {
            contentTop = top
            layoutHistory()
        }
    }

    @discardableResult private func title(_ text: String, frame: NSRect, size: CGFloat = 13,
                                           weight: NSFont.Weight = .regular, muted: Bool = false) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text); label.frame = frame
        label.font = .systemFont(ofSize: size, weight: weight); label.textColor = tokens.color(muted ? "text-muted" : "text")
        root.addSubview(label); return label
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
                self.loadHistory(select: self.initialSelectionID, cleanup: { [weak self] in
                    self?.processNextOpenImage()
                }); self.loadDisplays()
            case .failure(let error): self.showHistoryError("Couldn’t locate native history", error) }
        }
    }

    private func loadDisplays() {
        status.stringValue = "Refreshing displays…"
        // Listing displays is not a capture: its failures never offer a retry
        // unless a relaunched retry is waiting on this list.
        captureAttemptKind = nil
        run({ [transport] in
            let result = try transport.request(["operation": "displays"])
            guard let values = result["displays"] as? [[String: Any]] else { throw AppBridgeError.invalidResponse }
            let parsed = values.compactMap(DisplayItem.init)
            guard parsed.count == values.count else { throw AppBridgeError.invalidResponse }
            return parsed
        }) { [weak self] result in
            guard let self else { return }
            switch result { case .success(let values):
                let selectedID = self.displays.indices.contains(self.selectedDisplayIndex)
                    ? self.displays[self.selectedDisplayIndex].id : nil
                self.displays = values
                self.selectedDisplayIndex = values.firstIndex { $0.id == selectedID } ?? 0
                self.status.stringValue = values.isEmpty
                    ? "No displays are available. Screen access may be required." : "Displays refreshed."
            case .failure(let error):
                if let pending = self.pendingDisplayCapture {
                    // The capture waiting on this list fails here, as
                    // shipping's capture would.
                    self.pendingDisplayCapture = nil
                    self.captureAttemptKind = pending.retryKind
                    self.showCaptureError("Couldn’t list displays", error)
                } else {
                    // Listing ahead of a capture is not a capture: the next
                    // capture lists again and reports its own failure.
                    self.status.stringValue = "Couldn’t list displays: \(error.localizedDescription)"
                }
            }
            self.updateActions()
            if case .success = result, let pending = self.pendingDisplayCapture {
                self.pendingDisplayCapture = nil
                DispatchQueue.main.async { [weak self] in self?.runPendingDisplayCapture(pending) }
            }
        }
    }

    private func loadHistory(select id: String? = nil, selectIfUserGeneration: Int? = nil,
                             cleanup: (() -> Void)? = nil, completion: (() -> Void)? = nil) {
        historyGeneration += 1
        let generation = historyGeneration
        refreshRecovery()
        status.stringValue = "Loading capture history…"
        run({ [transport, historyRoot] in
            let result = try transport.request(["operation": "history", "root": historyRoot])
            guard let values = result["artifacts"] as? [[String: Any]] else { throw AppBridgeError.invalidResponse }
            let recordings = try NativeRecordingInfo.request([
                "operation": "history", "root": historyRoot,
            ])["recordings"] as? [[String: Any]] ?? []
            let all = values + recordings
            let parsed = all.compactMap(CaptureArtifact.init)
            guard parsed.count == all.count else { throw AppBridgeError.invalidResponse }
            // Shared, I/O-free card presentation; a malformed entry has no card.
            var cards: [String: HistoryCard] = [:]
            for (artifact, card) in zip(parsed, (try? HistoryCard.cards(for: all)) ?? []) {
                if let card { cards[artifact.id] = card }
            }
            return (parsed.sorted { $0.createdAt > $1.createdAt }, cards)
        }) { [weak self] result in
            guard let self else { return }
            defer { cleanup?() }
            guard self.historyGeneration == generation else { return }
            self.historyLoaded = true
            switch result { case .success(let (values, cards)):
                self.cards = cards
                let eligibleID = selectIfUserGeneration == nil || selectIfUserGeneration == self.userSelectionGeneration
                    ? id : nil
                let previousID = eligibleID ?? self.selectedIndex.flatMap { self.artifacts.indices.contains($0) ? self.artifacts[$0].id : nil }
                if let eligibleID, let requested = values.first(where: { $0.id == eligibleID }),
                   !self.historyFilter.matches(requested) { self.historyFilter = .all }
                self.artifacts = values; self.reloadHistorySelection(previousID)
                self.miniPreviews?.refreshArtifacts(values)
                self.miniPreviews?.reconcileHistory(ids: Set(values.map(\.id)))
            case .failure(let error): self.artifacts = []; self.reloadHistorySelection(nil); self.showHistoryError("Couldn’t load capture history", error) }
            self.updateActions(); completion?()
        }
    }

    private func refreshRecovery() {
        guard !historyRoot.isEmpty, !capturing, !recoveryBusy,
              !recordingRetiring else { return }
        recoveryGeneration += 1
        let current = recoveryGeneration, root = historyRoot
        recoveryLoading = true; renderRecovery()
        recoveryWorker.list(historyRoot: root) { [weak self] result in
            guard let self, self.recoveryGeneration == current, self.historyRoot == root else { return }
            self.recoveryLoading = false
            switch result {
            case .success(let drafts):
                self.recoveryDrafts = drafts
                self.recoveryError = nil
            case .failure(let error):
                self.recoveryDrafts = []
                self.recoveryError = "Couldn’t list interrupted recordings: \(error.localizedDescription)"
            }
            self.renderRecovery()
        }
    }

    private func renderRecovery() {
        guard let recoveryPanel else { return }
        let visible = !recoveryDrafts.isEmpty || recoveryError != nil
            || recoveryActionError != nil || recoveryBusy
        let changed = recoveryPanel.isHidden == visible
        recoveryPanel.isHidden = !visible
        recoveryCancelButton.isHidden = recoveryCancel == nil
        recoveryCancelButton.isEnabled = recoveryCancel != nil && recoveryCancel?.isCancelled == false
        recoveryRetryButton.isHidden = recoveryError == nil && recoveryActionError == nil
        recoveryRetryButton.isEnabled = !recoveryLoading && !recoveryBusy
        // Shipping `.recording-recovery-row`: details on the left, actions on the right.
        renderedRecoveryWidth = recoveryScroll.contentSize.width
        let width = recoveryScroll.contentSize.width - 4
        let textWidth = max(0, width - 260)
        let content = Surface(frame: NSRect(x: 0, y: 0, width: width, height: 86))
        var nextY: CGFloat = 0
        for (index, draft) in recoveryDrafts.enumerated() {
            let y = nextY
            if index > 0 {
                let divider = Surface(frame: NSRect(x: 2, y: y, width: width - 4, height: 1))
                divider.wantsLayer = true
                divider.layer?.backgroundColor = tokens.color("border-subtle").cgColor
                content.addSubview(divider)
            }
            let title = NSTextField(labelWithString: "\(draft.kind == "gif" ? "GIF" : draft.kind == "video" ? "Video" : "Unavailable") recording")
            title.frame = NSRect(x: 2, y: y + 8, width: textWidth, height: 18)
            title.font = .systemFont(ofSize: tokens.number("text-md"), weight: .medium)
            title.textColor = tokens.color("text")
            content.addSubview(title)
            let date = draft.createdAtMilliseconds.map {
                Date(timeIntervalSince1970: Double($0) / 1_000).formatted(date: .abbreviated, time: .shortened)
            } ?? "Unknown date"
            let details = NSTextField(labelWithString:
                "\(date) · \(formatRecordingTime(milliseconds: draft.completedDurationMilliseconds)) recovered so far")
            details.frame = NSRect(x: 2, y: y + 27, width: textWidth, height: 17)
            details.font = .systemFont(ofSize: tokens.number("text-sm")); details.textColor = tokens.color("text-muted")
            content.addSubview(details)
            if draft.status == "recoverable", draft.identity != nil {
                let recover = CaptureButton("Recover", frame: NSRect(
                    x: width - (recoveryDiscardArmedID == draft.sessionID ? 304 : 244), y: y + 10,
                    width: 112, height: 29),
                                            tokens: tokens) { [weak self] in self?.recover(draft) }
                // Shipping `.recording-recovery-row button.danger`: the first
                // press arms "Discard permanently?"; the second deletes.
                let armed = recoveryDiscardArmedID == draft.sessionID
                let discard = CaptureButton(armed ? "Discard permanently?" : "Discard",
                                            frame: NSRect(x: width - (armed ? 184 : 124), y: y + 10,
                                                          width: armed ? 172 : 112, height: 29),
                                            tokens: tokens) { [weak self] in self?.confirmDiscard(draft) }
                discard.signal = armed
                discard.escapeActionBlock = { [weak self] in self?.disarmRecoveryDiscard() }
                recover.isEnabled = !capturing && !clearingHistory && !recoveryBusy
                    && !recoveryLoading && !recordingRetiring
                discard.isEnabled = recover.isEnabled
                content.addSubview(recover); content.addSubview(discard)
                nextY += 52
            } else {
                let message = draft.reason ?? "This bundle cannot be recovered."
                let reason = NSTextField(wrappingLabelWithString: message)
                reason.font = .systemFont(ofSize: tokens.number("text-sm")); reason.textColor = tokens.color("danger-text")
                reason.toolTip = message
                reason.setAccessibilityHelp(message)
                let height = textHeight(message, font: reason.font!, width: textWidth)
                reason.frame = NSRect(x: 2, y: y + 46, width: textWidth, height: height)
                content.addSubview(reason)
                nextY += 46 + height + 8
            }
        }
        if let message = recoveryError ?? recoveryActionError {
            let y = nextY
            let error = NSTextField(wrappingLabelWithString: message)
            error.font = .systemFont(ofSize: tokens.number("text-sm")); error.textColor = tokens.color("danger-text")
            error.toolTip = message
            error.setAccessibilityHelp(message)
            let height = textHeight(message, font: error.font!, width: width - 4)
            error.frame = NSRect(x: 2, y: y + 4, width: width - 4, height: height)
            content.addSubview(error)
            nextY += height + 10
        }
        content.frame.size.height = max(86, nextY)
        recoveryScroll.documentView = content
        installKeyViewLoop()
        recoveryStatus.stringValue = recoveryBusy ? recoveryStage
            : (nextY > recoveryScroll.bounds.height ? "Scroll for full details." : "")
        if changed { layoutHistory() }
    }

    private func textHeight(_ message: String, font: NSFont, width: CGFloat) -> CGFloat {
        ceil((message as NSString).boundingRect(with: NSSize(width: width, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font]).height) + 3
    }

    private func currentRecovery(_ draft: RecordingRecoveryDraft) -> Bool {
        !capturing && !clearingHistory && !recoveryBusy && !recoveryLoading && !recordingRetiring
            && !externalOpenPending
            && recoveryDrafts.contains { $0.sessionID == draft.sessionID && $0.identity == draft.identity
                && $0.status == "recoverable" && draft.identity != nil }
    }

    private func recover(_ draft: RecordingRecoveryDraft) {
        guard currentRecovery(draft),
              let cancel = NativeRecordingEditorCancel() else { return }
        recoveryDiscardArmedID = nil
        recoveryActionGeneration += 1
        let current = recoveryActionGeneration
        let selectedAtDispatch = userSelectionGeneration
        recoveryBusy = true; recoveryCancel = cancel; recoveryActionError = nil
        recoveryStage = "Preparing…"
        updateActions(); renderRecovery()
        recoveryWorker.recover(historyRoot: historyRoot, draft: draft, cancel: cancel,
            progress: { [weak self] stage in
                guard let self, self.recoveryActionGeneration == current,
                      self.recoveryCancel === cancel else { return }
                self.recoveryStage = "\(stage.capitalized)…"
                self.recoveryStatus.stringValue = self.recoveryStage
            }, completion: { [weak self] result in
                guard let self, self.recoveryActionGeneration == current,
                      self.recoveryCancel === cancel else { return }
                self.recoveryCancel = nil
                switch result {
                case .success(let recovered):
                    self.recoveryStage = "Opening recovered recording…"
                    self.renderRecovery()
                    let shouldOpen = self.userSelectionGeneration == selectedAtDispatch
                    self.loadHistory(select: shouldOpen ? recovered.artifactID : nil,
                        cleanup: { [weak self] in
                            guard let self, self.recoveryActionGeneration == current else { return }
                            self.recoveryBusy = false; self.updateActions(); self.refreshRecovery()
                            self.processNextOpenImage()
                        }) { [weak self] in
                        guard let self, self.recoveryActionGeneration == current else { return }
                        guard shouldOpen, self.selectedIndex.flatMap({ self.artifacts.indices.contains($0)
                            ? self.artifacts[$0].id : nil }) == recovered.artifactID else { return }
                        self.status.stringValue = recovered.warning ?? "Recording recovered into History."
                        self.editScreenshot()
                    }
                case .failure(let error):
                    self.recoveryBusy = false; self.updateActions(); self.refreshRecovery()
                    self.processNextOpenImage()
                    self.recoveryActionError = "Couldn’t recover recording: \(error.localizedDescription)"
                    self.renderRecovery()
                }
            })
    }

    private func confirmDiscard(_ draft: RecordingRecoveryDraft) {
        guard currentRecovery(draft) else { return }
        guard recoveryDiscardArmedID == draft.sessionID else {
            recoveryDiscardArmedID = draft.sessionID
            renderRecovery()
            return
        }
        recoveryDiscardArmedID = nil
        recoveryActionGeneration += 1
        let current = recoveryActionGeneration
        recoveryBusy = true; recoveryActionError = nil
        recoveryStage = "Discarding…"
        updateActions(); renderRecovery()
        recoveryWorker.discard(historyRoot: historyRoot, draft: draft) { [weak self] result in
            guard let self, self.recoveryActionGeneration == current else { return }
            self.recoveryBusy = false; self.updateActions(); self.refreshRecovery()
            self.processNextOpenImage()
            if case .failure(let error) = result {
                self.recoveryActionError = "Couldn’t discard recording: \(error.localizedDescription)"
                self.renderRecovery()
            }
        }
    }

    /// Escape disarms an armed Discard.
    func disarmRecoveryDiscard() {
        guard recoveryDiscardArmedID != nil else { return }
        recoveryDiscardArmedID = nil
        renderRecovery()
    }

    private func reloadHistorySelection(_ previousID: String?) {
        reloadingHistorySelection = true
        defer { reloadingHistorySelection = false }
        historyRows = artifacts.indices.filter { historyFilter.matches(artifacts[$0]) }
        confirmDeleteID = nil; confirmDeleteTimer?.invalidate(); confirmDeleteTimer = nil
        for (filter, control) in historyFilterButtons {
            control.count = artifacts.filter(filter.matches).count
            control.active = filter == historyFilter
        }
        // Like shipping, loading or filtering never selects a card on the
        // user's behalf; an explicit selection that is still visible survives.
        selectedIndex = historyRows.first { artifacts[$0].id == previousID }
        updateActions()
        layoutHistory()
    }

    /// Render History cards from the current rows, presentation and thumbnails.
    private func refreshGrid(busy: Bool) {
        guard let grid else { return }
        grid.enabled = !busy
        grid.reload(historyRows.map { index in
            let artifact = artifacts[index]
            let key = thumbnailKey(artifact)
            return HistoryGridItem(id: artifact.id, card: cards[artifact.id],
                                   image: thumbnailKeys[artifact.id] == key ? thumbnails[artifact.id] : nil,
                                   confirmingDelete: confirmDeleteID == artifact.id,
                                   busy: cardBusy?.id == artifact.id ? cardBusy?.action : nil,
                                   done: restoredCardID == artifact.id ? .restore : nil,
                                   error: cardErrors[artifact.id])
        })
        grid.setSelectedRow(selectedIndex.flatMap { historyRows.firstIndex(of: $0) } ?? -1, notify: false)
        let copy = historyCopy
        if !historyLoaded {
            emptyState.show(title: copy.loading, body: nil); emptyState.isHidden = false
        } else if artifacts.isEmpty && recoveryPanel.isHidden {
            emptyState.show(title: copy.emptyTitle, body: copy.emptyBody); emptyState.isHidden = false
        } else {
            emptyState.isHidden = true
        }
        let filteredEmpty = historyLoaded && !artifacts.isEmpty && historyRows.isEmpty
        filteredEmptyLabel.stringValue = filteredEmpty ? copy.filteredEmpty : ""
        filteredEmptyLabel.isHidden = !filteredEmpty
    }

    private func thumbnailKey(_ artifact: CaptureArtifact) -> String {
        "\(artifact.previewPath)|\(artifact.width)x\(artifact.height)|\(artifact.sizeBytes)"
    }

    /// Decode a visible card's small preview off the main thread. Stale or
    /// failed results for an older entry version are ignored.
    private func loadThumbnail(row: Int) {
        guard historyRows.indices.contains(row) else { return }
        let artifact = artifacts[historyRows[row]]
        let id = artifact.id, key = thumbnailKey(artifact), path = artifact.previewPath
        guard thumbnailKeys[id] != key || thumbnails[id] == nil, thumbnailFailures[id] != key,
              !thumbnailRequests.contains(id) else { return }
        thumbnailRequests.insert(id)
        Self.thumbnailQueue.async {
            var image: NSImage?
            if let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
               let cgImage = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                   kCGImageSourceCreateThumbnailFromImageAlways: true,
                   kCGImageSourceCreateThumbnailWithTransform: true,
                   kCGImageSourceThumbnailMaxPixelSize: 640,
               ] as CFDictionary) {
                image = NSImage(cgImage: cgImage, size: NSSize(width: CGFloat(cgImage.width),
                                                                height: CGFloat(cgImage.height)))
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.thumbnailRequests.remove(id)
                guard let current = self.artifacts.first(where: { $0.id == id }),
                      self.thumbnailKey(current) == key else { return }
                guard let image else { self.thumbnailFailures[id] = key; return }
                self.thumbnails[id] = image; self.thumbnailKeys[id] = key
                if self.thumbnails.count > Self.thumbnailCacheLimit {
                    let visible = Set(self.grid.visibleCards.map(\.artifactID))
                    for cached in self.thumbnails.keys where !visible.contains(cached) && cached != id {
                        self.thumbnails[cached] = nil; self.thumbnailKeys[cached] = nil
                    }
                }
                self.updateActions()
            }
        }
    }

    private func gridSelectionChanged() {
        if !reloadingHistorySelection { userSelectionGeneration += 1 }
        let rows = historyRows
        guard rows.indices.contains(grid.selectedRow) else { clearSelection(); return }
        select(rows[grid.selectedRow])
    }

    private var historyBusy: Bool {
        capturing || clearingHistory || recoveryBusy
            || recordingRetiring || externalOpenPending || permissionsVisible || editorTerminationPending
    }
    /// No capture or History operation holds the workspace.
    var historyIdle: Bool { !historyBusy }
    /// Tray, shortcut and New Capture actions can start: idle, with a display
    /// list and a History root.
    var captureReady: Bool { historyIdle && !displays.isEmpty && !historyRoot.isEmpty }

    private func performCard(row: Int, action: HistoryCardAction) {
        guard historyRows.indices.contains(row), !historyBusy, cardBusy == nil else { return }
        let artifact = artifacts[historyRows[row]]
        // Shipping clears a card's error when it starts another action.
        cardErrors[artifact.id] = nil
        switch action {
        case .edit:
            guard !historyRoot.isEmpty, cards[artifact.id]?.missing != true else { return }
            // Shipping History Edit restores the floating preview first
            // (`restore_history_artifact`), then opens the editor; a failed
            // restore opens nothing.
            guard !artifact.isRecording, let miniPreviews else {
                presentEditor(artifact, requiresCurrentSelection: false); return
            }
            cardBusy = (artifact.id, action); updateActions()
            restore(artifact, with: miniPreviews) { [weak self] outcome in
                guard let self else { return }
                if self.cardBusy?.id == artifact.id, self.cardBusy?.action == .edit { self.cardBusy = nil }
                switch outcome {
                case .shown, .alreadyShowing, .cancelled:
                    self.presentEditor(artifact, requiresCurrentSelection: false)
                case .failed(let error):
                    self.cardErrors[artifact.id] = error.localizedDescription
                }
                self.updateActions()
            }
        case .restore:
            guard !artifact.isRecording, let miniPreviews else { return }
            cardBusy = (artifact.id, action); setRestoredCard(nil); updateActions()
            restore(artifact, with: miniPreviews) { [weak self] outcome in
                self?.finishRestore(artifact.id, outcome)
            }
        case .copy:
            guard !artifact.isRecording else { return }
            copyImage(at: artifact.imagePath, artifactID: artifact.id)
        case .saveImage, .saveFile:
            cardBusy = (artifact.id, action); updateActions()
            save(artifact)
        case .showInFolder:
            reveal(artifact)
        }
    }

    /// Prepare only the pressed History card's transferable media. The Rust
    /// operation returns saved media when available, otherwise a retained
    /// screenshot/GIF/video file; it never exports a recording poster.
    private func prepareHistoryDrag(row: Int, completion: @escaping (Result<String, Error>) -> Void) {
        guard historyRows.indices.contains(row), !historyBusy, cardBusy == nil, !historyRoot.isEmpty else {
            completion(.failure(AppBridgeError.invalidResponse)); return
        }
        let artifact = artifacts[historyRows[row]]
        guard cards[artifact.id]?.missing != true else {
            completion(.failure(AppBridgeError.invalidResponse)); return
        }
        let id = artifact.id, root = historyRoot
        Self.queue.async { [weak self, transport] in
            let result = Result { () throws -> String in
                let response = try transport.request(["operation": "prepare_preview_drag", "root": root, "id": id])
                guard response["id"] as? String == id, let path = response["path"] as? String, !path.isEmpty
                else { throw AppBridgeError.invalidResponse }
                return path
            }
            DispatchQueue.main.async {
                guard let self, self.historyRoot == root,
                      self.artifacts.contains(where: { $0.id == id }) else { return }
                completion(result)
            }
        }
    }

    private func showHistoryDragError(row: Int, error: Error) {
        guard historyRows.indices.contains(row) else { return }
        let id = artifacts[historyRows[row]].id
        cardErrors[id] = error.localizedDescription
        updateActions()
    }

    /// Shipping History Restore: reopen a screenshot through the mini-preview
    /// stack, with no clipboard copy. An empty stack opens on the selected display.
    /// Shared by Restore and Edit; `completion` runs on the main queue.
    private func restore(_ artifact: CaptureArtifact, with previews: MiniPreviewController,
                         completion: @escaping (MiniPreviewRestoreOutcome) -> Void) {
        let index = selectedDisplayIndex
        let screenID = displays.indices.contains(index)
            ? displays[index].id : window.screen.flatMap(MiniPreviewController.displayID(for:))
        run({ [settingsPath] in try CapturePreferences.load(path: settingsPath) }) { [weak previews] result in
            switch result {
            case .success(let preferences):
                guard let previews else { completion(.cancelled); return }
                previews.restore(artifact, on: screenID, settings: preferences.miniPreviewSettings,
                                 completion: completion)
            case .failure(let error):
                completion(.failed(error))
            }
        }
    }

    private func finishRestore(_ id: String, _ outcome: MiniPreviewRestoreOutcome) {
        if cardBusy?.id == id, cardBusy?.action == .restore { cardBusy = nil }
        switch outcome {
        case .shown, .alreadyShowing: setRestoredCard(id)
        case .cancelled: break
        case .failed(let error): cardErrors[id] = error.localizedDescription
        }
        updateActions()
    }

    /// Show "✓ Restored" on `id` (or clear it), reverting after the shipping 2.5 s.
    private func setRestoredCard(_ id: String?) {
        restoredCardReset?.cancel(); restoredCardReset = nil
        restoredCardID = id
        guard id != nil else { return }
        let reset = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.restoredCardID = nil; self.restoredCardReset = nil; self.updateActions()
        }
        restoredCardReset = reset
        DispatchQueue.main.asyncAfter(deadline: .now() + historyCopy.feedbackDuration, execute: reset)
    }

    /// Shipping card trash: the first click arms "Delete forever" for four
    /// seconds; the second deletes. A missing recording is removed at once.
    private func deleteCard(row: Int) {
        guard historyRows.indices.contains(row), !historyBusy, cardBusy == nil else { return }
        let artifact = artifacts[historyRows[row]]
        let requiresConfirmation = cards[artifact.id]?.deleteRequiresConfirmation ?? true
        if requiresConfirmation && confirmDeleteID != artifact.id {
            confirmDeleteAll = false; confirmDeleteAllTimer?.invalidate(); confirmDeleteAllTimer = nil
            confirmDeleteID = artifact.id
            confirmDeleteTimer?.invalidate()
            confirmDeleteTimer = Timer.scheduledTimer(withTimeInterval: historyCopy.confirmTimeout, repeats: false) {
                [weak self] _ in
                self?.confirmDeleteID = nil; self?.confirmDeleteTimer = nil; self?.updateActions()
            }
            updateActions()
            return
        }
        confirmDeleteID = nil; confirmDeleteTimer?.invalidate(); confirmDeleteTimer = nil
        updateActions()
        delete(artifact)
    }

    private func cancelHistoryConfirmations() {
        disarmRecoveryDiscard()
        confirmDeleteID = nil; confirmDeleteTimer?.invalidate(); confirmDeleteTimer = nil
        confirmDeleteAll = false; confirmDeleteAllTimer?.invalidate(); confirmDeleteAllTimer = nil
        updateActions()
    }

    func setPermissionsVisible(_ visible: Bool) {
        permissionsVisible = visible
        captureStateChanged(capturing || visible)
        updateActions()
        if !visible { loadDisplays(); processNextOpenImage() }
    }

    /// Shipping shortcut, tray and New Capture flows start on the display under
    /// the pointer.
    private func selectDisplayUnderPointer() {
        guard let id = pointerDisplayID(),
              let index = displays.firstIndex(where: { $0.id == id }) else { return }
        selectedDisplayIndex = index
    }

    private func pointerDisplayID() -> String? {
        let location = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { NSMouseInRect(location, $0.frame, false) })
        else { return nil }
        return (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue
    }

    /// Lists the displays again before `request` when the list is empty or
    /// stale. Returns true when `request` now waits on that list.
    private func refreshDisplaysBeforeCapture(_ request: PendingDisplayCapture) -> Bool {
        guard !capturing, !recoveryBusy, !recordingRetiring,
              !externalOpenPending, !permissionsVisible, !historyRoot.isEmpty,
              pendingDisplayCapture == nil,
              displayListNeedsRefresh(listed: displays.map { $0.id }, pointer: pointerDisplayID())
        else { return false }
        pendingDisplayCapture = request
        loadDisplays()
        return true
    }

    private func runPendingDisplayCapture(_ pending: PendingDisplayCapture) {
        let started: Bool
        switch pending {
        case .still(let kind):
            started = capture(kind, refreshingDisplays: false)
        case .menu(let recordingTarget, let screenshotTarget):
            started = newCapture(recordingTarget: recordingTarget, screenshotTarget: screenshotTarget,
                                 refreshingDisplays: false)
        }
        guard !started, displays.isEmpty else { return }
        captureAttemptKind = pending.retryKind
        showCaptureError("Couldn’t start capture",
            AppBridgeError.backend("No displays are available. Screen access may be required."))
    }

    @discardableResult func capture(_ kind: StillCaptureKind) -> Bool {
        capture(kind, refreshingDisplays: true)
    }

    private func capture(_ kind: StillCaptureKind, refreshingDisplays: Bool) -> Bool {
        recordingSavedNotice.dismiss()
        if refreshingDisplays, refreshDisplaysBeforeCapture(.still(kind)) { return true }
        if !capturing { selectDisplayUnderPointer() }
        let index = selectedDisplayIndex
        guard !capturing, !recoveryBusy, !recordingRetiring,
              !externalOpenPending, !permissionsVisible,
              displays.indices.contains(index), !historyRoot.isEmpty else { return false }
        captureAttemptKind = kind
        windowRestoration.begin(windowIsVisible: window.isVisible, windowIsKey: window.isKeyWindow)
        let display = displays[index]; setBusy(true, message: "Preparing capture…")
        run({ [settingsPath] in try CapturePreferences.load(path: settingsPath) }) { [weak self] result in
            guard let self else { return }
            do {
                let preferences = try result.get()
                self.capturePreferences = preferences
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
                self.workspaceHidden?(true)
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
                self.finishCapture(); self.showCaptureError("Couldn’t start capture", error)
            }
        }
        return true
    }

    @discardableResult func newCapture(recordingTarget: UnifiedCaptureTarget? = nil,
                                       screenshotTarget: UnifiedCaptureTarget? = nil) -> Bool {
        newCapture(recordingTarget: recordingTarget, screenshotTarget: screenshotTarget,
                   refreshingDisplays: true)
    }

    private func newCapture(recordingTarget: UnifiedCaptureTarget?,
                            screenshotTarget: UnifiedCaptureTarget?,
                            refreshingDisplays: Bool) -> Bool {
        recordingSavedNotice.dismiss()
        if refreshingDisplays,
           refreshDisplaysBeforeCapture(.menu(recordingTarget: recordingTarget,
                                              screenshotTarget: screenshotTarget)) { return true }
        if !capturing { selectDisplayUnderPointer() }
        let index = selectedDisplayIndex
        guard !capturing, !recoveryBusy, !recordingRetiring,
              !externalOpenPending, !permissionsVisible,
              displays.indices.contains(index), !historyRoot.isEmpty else { return false }
        captureAttemptKind = .region
        windowRestoration.begin(windowIsVisible: window.isVisible, windowIsKey: window.isKeyWindow)
        let display = displays[index]
        unifiedControlsState = .initial
        if let recordingTarget {
            unifiedControlsState.mode = .record
            unifiedControlsState.target = recordingTarget
        } else if let screenshotTarget {
            // Shipping `open_capture_controls_with_target(Screenshot, target)`.
            unifiedControlsState.target = screenshotTarget
        }
        unifiedRouteTarget = recordingTarget == nil ? (screenshotTarget ?? .region) : nil
        setBusy(true, message: "Preparing capture controls…")
        let request = unifiedPreparation.begin()
        run({ [settingsPath] in
            let preferences = try CapturePreferences.load(path: settingsPath)
            let capabilities = try NativeRecordingInfo.capabilities(
                includeControls: preferences.includeRecordingControlsInCaptures)
            return (preferences, capabilities)
        }) { [weak self] result in
            guard let self, self.capturing, self.unifiedPreparation.accepts(request) else { return }
            do {
                let (preferences, capabilities) = try result.get()
                self.applyMenuState(preferences, capabilities: capabilities)
                self.capturePreferences = preferences
                let response = try AppBridge.flow(["operation": "begin", "seconds": 0])
                guard let generation = response["generation"] as? NSNumber else {
                    throw AppBridgeError.invalidResponse
                }
                self.flowGeneration = generation.uint64Value
                self.snapshotPending = false
                self.previewCaptureGeneration = self.miniPreviews?.beginCapture(
                    settings: preferences.miniPreviewSettings)
                self.window.orderOut(nil)
                self.workspaceHidden?(true)
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
                self.showCaptureError("Couldn’t start New Capture", error)
            }
        }
        return true
    }

    private func prepareUnified(display: DisplayItem, preferences: CapturePreferences,
                                generation: UInt64) {
        selectorShortcutGeneration = nil
        // A display switch keeps the current menu up showing "Switching…"
        // until the new display is ready, as shipping's `switchDisplay` does.
        let outgoing = unifiedPanel
        let replacingDisplay = outgoing != nil
        let previousDisplay = unifiedDisplay, previousScreen = unifiedScreen
        guard let screen = screen(for: display) else {
            let error = AppBridgeError.backend("The selected display is no longer available.")
            if !keepUnifiedMenuOpen(outgoing, error: error, generation: generation,
                                    restoring: previousDisplay, on: previousScreen) {
                finishCapture()
                showCaptureError("Couldn’t prepare capture controls", error)
            }
            return
        }
        outgoing?.selector.controls.setInFlight(.switching)
        unifiedTarget = nil
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
                    if let outgoing, self.unifiedPanel === outgoing {
                        self.unifiedControlsState = outgoing.selector.controlsState
                        outgoing.close(); self.unifiedPanel = nil
                    }
                    self.unifiedSession = session
                    self.closeRecapturedPanels()
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
                        }, microphoneDevices: self.microphoneDevices,
                        microphonesLoaded: self.microphonesLoaded,
                        visibility: self.recordingCapabilities.map {
                            CaptureControlsVisibility(canExclude: $0.canExcludeControls,
                                excluded: $0.controlsExcluded)
                        } ?? .excludedByDefault,
                        displayIdentity: CaptureDisplayIdentity(name: display.name,
                            width: display.width, height: display.height))
                    panel.selector.controls.recordingControlsChanged = { [weak self] state in
                        self?.recordingControlState = state
                    }
                    panel.selector.controls.loadMicrophones = { [weak self, weak panel] in
                        self?.run({ try NativeRecordingInfo.microphoneDevices() }) { [weak self] result in
                            guard let self, self.flowGeneration == generation else { return }
                            // Like shipping, a failed enumeration lists no devices.
                            let devices = (try? result.get()) ?? []
                            self.microphoneDevices = devices; self.microphonesLoaded = true
                            panel?.selector.controls.setMicrophones(devices)
                        }
                    }
                    panel.selector.openPreference = { [weak self] setting in
                        // Shipping `openCapturePreference`: dismiss the menu,
                        // then show Preferences at the linked setting.
                        guard let self, self.flowGeneration == generation,
                              self.unifiedPanel != nil else { return }
                        self.finishCapture()
                        self.status.stringValue = "Capture cancelled."
                        self.showPreferenceSetting(setting)
                    }
                    self.unifiedPanel = panel
                    self.preparingUnified = false
                    panel.selector.restoreControls(self.unifiedControlsState,
                        armAutoStart: replacingDisplay)
                    guard self.unifiedPanel === panel else { return }
                    panel.makeKeyAndOrderFront(nil)
                    self.selectorShortcutGeneration = generation
                    NSApp.activate(ignoringOtherApps: true)
                    panel.selector.updatePointerLocation()
                } catch {
                    // Shipping `select_capture_display` restores the previous
                    // display and the menu shows the error inline.
                    if !self.keepUnifiedMenuOpen(outgoing, error: error, generation: generation,
                                                 restoring: previousDisplay, on: previousScreen) {
                        self.finishCapture()
                        self.showCaptureError("Couldn’t prepare capture controls", error)
                    }
                }
            }
        }
    }

    /// Shipping `RecordingSelector` stays open when `start_recording` or
    /// `select_capture_display` fails: the in-flight label ends, the mode,
    /// target, region and options stay, and the error shows inline under the
    /// note so the user can retry or pick something else. A display switch
    /// returns to the display it was on. False when that menu is gone, so the
    /// caller ends the capture with the host's dialog as before.
    private func keepUnifiedMenuOpen(_ panel: UnifiedCapturePanel?, error: Error,
                                     generation: UInt64, restoring previousDisplay: DisplayItem? = nil,
                                     on previousScreen: NSScreen? = nil) -> Bool {
        guard flowGeneration == generation, let panel, unifiedPanel === panel else { return false }
        if let previousDisplay, let previousScreen {
            unifiedDisplay = previousDisplay; unifiedScreen = previousScreen
            unifiedPreparation.invalidate()
            if let index = displays.firstIndex(where: { $0.id == previousDisplay.id }) {
                panel.selector.controls.selectDisplay(index)
            }
        }
        preparingUnified = false; preparingRecording = false
        unifiedTarget = nil
        panel.selector.controls.showInlineError(error.localizedDescription)
        selectorShortcutGeneration = generation
        status.stringValue = "\(error.localizedDescription) Retry or choose again. Press Escape to cancel."
        return true
    }

    private func confirmUnified(_ target: WindowSelectionChoice,
                                preferences: CapturePreferences, generation: UInt64) {
        guard flowGeneration == generation, let panel = unifiedPanel,
              panel.selector.controls.inFlight == nil,
              let screen = unifiedScreen, let display = unifiedDisplay else { return }
        // Shipping shows "Capturing…" / "Starting…" until its start command
        // hides the selector.
        panel.selector.controls.setInFlight(.starting)
        if panel.selector.mode == .record {
            prepareRecording(target: target, display: display, screen: screen,
                preferences: preferences, generation: generation)
            return
        }
        do {
            selectorShortcutGeneration = nil
            unifiedTarget = target
            // A screenshot hides the selector at once, as shipping's
            // `capture_selection_screenshot` does first; "Capturing…" gets
            // this one display pass.
            panel.displayIfNeeded()
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
            showCaptureError("Capture failed", error)
        }
    }

    private func prepareRecording(target: WindowSelectionChoice, display: DisplayItem,
                                  screen: NSScreen, preferences: CapturePreferences,
                                  generation: UInt64) {
        guard let capabilities = recordingCapabilities else {
            let error = AppBridgeError.backend("Recording capabilities are unavailable.")
            if !keepUnifiedMenuOpen(unifiedPanel, error: error, generation: generation) {
                finishCapture(); showCaptureError("Couldn’t start recording", error)
            }
            return
        }
        let menu = unifiedPanel
        do {
            let targetValue = try nativeRecordingTarget(target, displayID: display.id)
            let options = nativeRecordingOptions(preferences: preferences.recording,
                target: targetValue, capabilities: capabilities,
                controls: recordingControlState)
            selectorShortcutGeneration = nil
            unifiedTarget = target
            // The menu stays up showing "Starting…" until the take is
            // prepared, as shipping's selector does until `start_recording`
            // hides it.
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
                        self.retireRecordingSession(value.0)
                    } else {
                        self.recordingRetiring = false
                        self.updateActions(); self.refreshRecovery()
                    }
                    return
                }
                do {
                    let (session, snapshot) = try result.get()
                    self.unifiedPanel?.close(); self.unifiedPanel = nil
                    self.recordingSession = session
                    self.recordingDisplay = display
                    self.recordingPreferences = preferences
                    self.recordingPendingStart = true
                    self.recordingGate.set(generation)
                    self.activeRecordingGeneration = generation
                    _ = try AppBridge.flow(["operation": "start_countdown",
                        "generation": generation, "seconds": preferences.recording.countdown])
                    if let region = snapshot.region {
                        let guide = RecordingRegionPanel(screen: screen, region: region, tokens: self.tokens)
                        self.recordingRegionPanel = guide
                        guide.orderFrontRegardless()
                    }
                    if preferences.recording.countdown > 0 {
                        let countdown = ScreenshotCountdownPanel(screen: screen, tokens: self.tokens,
                            remaining: preferences.recording.countdown, kind: .recording)
                        self.countdownPanel = countdown; countdown.orderFrontRegardless()
                    }
                    self.preparingRecording = false
                    self.tickCountdown(display: display, preferences: preferences,
                        generation: generation)
                } catch {
                    self.preparingRecording = false
                    // Shipping `start_recording` restores the selection before
                    // its HUD is up; the menu shows the error inline.
                    if !self.keepUnifiedMenuOpen(menu, error: error, generation: generation) {
                        self.finishCapture()
                        self.showCaptureError("Couldn’t prepare recording", error)
                    }
                }
            }
        } catch {
            if !keepUnifiedMenuOpen(menu, error: error, generation: generation) {
                finishCapture(); showCaptureError("Couldn’t prepare recording", error)
            }
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
                    self.closeRecapturedPanels()
                    // Shipping direct overlays commit on release; auto-start
                    // applies only to the New Capture menu.
                    let panel = RegionSelectionPanel(screen: screen, image: image, tokens: self.tokens,
                        autoStart: true, confirm: { [weak self] rect in
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
                            } catch { self.finishCapture(); self.showCaptureError("Capture failed", error) }
                        }, cancel: { [weak self] in
                            guard let self, self.flowGeneration == generation, self.regionPanel != nil else { return }
                            self.finishCapture(); self.status.stringValue = "Capture cancelled."
                        })
                    self.regionPanel = panel; self.preparingRegion = false
                    panel.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
                } catch { self.finishCapture(); self.showCaptureError("Couldn’t prepare region", error) }
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
                    self.closeRecapturedPanels()
                    let panel = WindowSelectionPanel(screen: screen, image: image,
                        targets: session.windows, tokens: self.tokens,
                        autoStart: true,
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
                            } catch { self.finishCapture(); self.showCaptureError("Capture failed", error) }
                        }, cancel: { [weak self] in
                            guard let self, self.flowGeneration == generation,
                                  self.windowPanel != nil else { return }
                            self.finishCapture(); self.status.stringValue = "Capture cancelled."
                        })
                    self.windowPanel = panel; self.preparingWindow = false
                    panel.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
                    panel.updatePointerLocation()
                } catch { self.finishCapture(); self.showCaptureError("Couldn’t prepare window selection", error) }
            }
        }
    }

    private func tickCountdown(display: DisplayItem, preferences: CapturePreferences, generation: UInt64) {
        guard flowGeneration == generation else { return }
        do {
            let state = try AppBridge.flow(["operation": "poll", "generation": generation])
            guard state["current"] as? Bool == true else {
                if let panel = countdownPanel { countdownPanel = nil; panel.closeAfterCancelling() }
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
            // A recaptured selection has no countdown and keeps its frozen
            // pixels (`capturePreferences` holds its `recapturing()` copy).
            let afterCountdown = (capturePreferences ?? preferences).countdown > 0
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                guard let self, self.flowGeneration == generation else { return }
                self.run({ [transport, historyRoot, regionSession, regionRect, windowSession, windowTarget,
                            unifiedSession, unifiedTarget] in
                    if let unifiedSession, let unifiedTarget {
                        return try unifiedSession.capture(root: historyRoot, target: unifiedTarget,
                            afterCountdown: afterCountdown)
                    }
                    if let windowSession, let windowTarget {
                        return try windowSession.capture(root: historyRoot, target: windowTarget,
                            afterCountdown: afterCountdown)
                    }
                    if let regionSession, let regionRect {
                        return try regionSession.capture(root: historyRoot, rect: regionRect, afterCountdown: afterCountdown)
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
                        if preferences.autoCopy { self.copyImage(at: artifact.imagePath, artifactID: artifact.id) }
                    case .failure(let error):
                        self.finishCapture(); self.showCaptureError("Capture failed", error)
                    }
                }
            }
        } catch { finishCapture(); showCaptureError("Capture failed", error) }
    }

    private func startRecording(on screen: NSScreen?, generation: UInt64) {
        guard let session = recordingSession, let screen else {
            finishCapture(); showCaptureError("Recording failed",
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
                    guard snapshot.state != "failed" else {
                        self.preserveFailedRecording(session, warning: snapshot.warning)
                        return
                    }
                    do {
                        _ = try AppBridge.flow(["operation": "disarm_escape",
                            "generation": generation])
                    } catch {
                        self.discardRecording()
                        self.showCaptureError("Recording start was cancelled", error)
                        return
                    }
                    self.recordingPendingStart = false
                    self.finishSelectorForRecording(generation: generation)
                    let hud = self.makeRecordingHUD(screen: screen)
                    hud.hud.setPaused(false, elapsedMilliseconds: snapshot.elapsedMilliseconds)
                    hud.hud.setMicrophone(muted: snapshot.microphoneMuted,
                        available: snapshot.hasMicrophone)
                    hud.hud.setWarning(snapshot.warning)
                    self.recordingHUD = hud; hud.orderFrontRegardless()
                    self.recordingMeter = RecordingMicrophoneSampler(queue: Self.queue,
                        read: { try session.microphoneLevel() }, useful: { [weak self, weak hud] in
                            guard let self, let hud else { return false }
                            return self.recordingSession === session && self.recordingHUD === hud
                                && !self.recordingLifecycle.busy && !self.recordingPollPending
                                && !self.recordingControlsHidden && hud.isVisible && !hud.hud.isHidden
                                && !hud.hud.paused && !hud.hud.microphoneMuted
                                && hud.hud.microphoneAvailable
                        }, deliver: { [weak self, weak hud] peak in
                            guard let self, let hud, self.recordingSession === session,
                                  self.recordingHUD === hud else { return }
                            hud.hud.setMicrophoneLevel(peak)
                        })
                    self.updateRecordingMeter()
                    self.status.stringValue = snapshot.warning ?? "Recording in progress…"
                    self.startRecordingPolling()
                } catch {
                    // Shipping keeps a take whose engine could not start, with the
                    // HUD showing the error, Retry recording and Delete. Read the
                    // owner's state on the same worker before deciding.
                    self.run({ try session.snapshot() }) { [weak self] snapshotResult in
                        guard let self, self.flowGeneration == generation,
                              self.recordingSession === session else { return }
                        let current = (try? AppBridge.flow(["operation": "poll",
                            "generation": generation]))?["current"] as? Bool == true
                        if case .success(let snapshot) = snapshotResult,
                           snapshot.state == "failed", current {
                            self.enterFailedRecording(snapshot, error: error, screen: screen,
                                generation: generation)
                            return
                        }
                        self.recordingSession = nil
                        self.retireRecordingSession(session)
                        self.finishCapture(); self.showCaptureError("Recording failed to start", error)
                    }
                }
            }
        }
    }

    private func makeRecordingHUD(screen: NSScreen) -> RecordingHUDPanel {
        let hud = RecordingHUDPanel(screen: screen, tokens: tokens,
            excludedFromCapture: recordingCapabilities?.controlsExcluded == true)
        hud.hud.pauseOrResume = { [weak self] in self?.pauseOrResumeRecording() }
        hud.hud.toggleMicrophone = { [weak self] in self?.toggleRecordingMicrophone() }
        hud.hud.restart = { [weak self] in self?.confirmRestartRecording() }
        hud.hud.screenshot = { [weak self] in self?.takeRecordingScreenshot() }
        hud.hud.stop = { [weak self] in self?.stopRecording() }
        hud.hud.discard = { [weak self] in self?.confirmDeleteRecording() }
        hud.hud.hide = { [weak self] in self?.hideRecordingControls() }
        return hud
    }

    /// The initial engine start failed (for example the selected microphone is
    /// missing or screen access was denied). Like shipping, keep the take and
    /// show the HUD with its error, Retry recording and Delete. Nothing counts
    /// down, so Escape is released; Retry rearms it for the new countdown.
    private func enterFailedRecording(_ snapshot: NativeRecordingSnapshot, error: Error,
                                      screen: NSScreen, generation: UInt64) {
        finishSelectorForRecording(generation: generation)
        _ = try? AppBridge.flow(["operation": "disarm_escape", "generation": generation])
        recordingMeter?.setActive(false); recordingMeter = nil
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        clearRecordingControlsHiddenState()
        // Shipping removes the region indicator from a failed take.
        recordingRegionPanel?.orderOut(nil)
        recordingHUD?.close()
        let hud = makeRecordingHUD(screen: screen)
        hud.hud.setMicrophone(muted: snapshot.microphoneMuted, available: snapshot.hasMicrophone)
        hud.hud.resetErrors()
        hud.hud.setFailed(error: error.localizedDescription, sessionError: snapshot.error,
            elapsedMilliseconds: snapshot.elapsedMilliseconds)
        recordingHUD = hud; hud.orderFrontRegardless()
        status.stringValue = "Recording failed. Retry or delete it from the recording controls."
    }

    /// Show a recording action failure on the visible HUD's inline error line
    /// (shipping `.recording-hud-error`) as well as the workspace status.
    private func showRecordingError(_ context: String, _ error: Error) {
        if let hud = recordingHUD, hud.isVisible, !hud.hud.isHidden {
            hud.hud.showActionError(error.localizedDescription)
        }
        showError(context, error)
    }

    /// The recording controls' Screenshot button: shipping
    /// `start_capture_inner(Region)`, on the recording's display.
    private func takeRecordingScreenshot() {
        guard recordingSession != nil, let hud = recordingHUD,
              let display = recordingDisplay, let preferences = recordingPreferences,
              let screen = screen(for: display),
              let generation = beginRecordingScreenshotFlow("Couldn’t start recording screenshot",
                                                            fromControls: true) else { return }
        hud.hud.actionStarted()
        updateRecordingMeter()
        openRecordingSelector(RecordingSelectorShot(kind: .region, display: display, screen: screen,
            preferences: preferences), generation: generation, fromControls: true)
    }

    /// Shipping region, window and display shortcuts and tray items during a
    /// recording (`start_capture_inner`): the same screenshot, beside the take.
    /// Region and window open their selector on the display under the pointer
    /// with the current preferences, like the controls' Screenshot button.
    /// Refuses silently when the take is counting down, finalizing or busy.
    @discardableResult func captureWhileRecording(_ kind: StillCaptureKind) -> Bool {
        if kind == .display { return captureDisplayWhileRecording() }
        guard recordingDisplayScreenshotAvailable, recordingHUD != nil else { return false }
        // Shipping looks the display up for every capture: the pointer's.
        let pointer = pointerDisplayID()
        guard let display = displays.first(where: { $0.id == pointer }) ?? recordingDisplay,
              let screen = screen(for: display) else {
            showCaptureError("Couldn’t start capture",
                AppBridgeError.backend("The selected display is no longer available."))
            return false
        }
        guard let generation = beginRecordingScreenshotFlow("Couldn’t start capture",
                                                            fromControls: false) else { return false }
        status.stringValue = "Preparing screenshot… Press Escape to cancel."
        run({ [settingsPath] in try CapturePreferences.load(path: settingsPath) }) { [weak self] result in
            guard let self, self.recordingScreenshotGeneration == generation else { return }
            do {
                let preferences = try result.get()
                self.openRecordingSelector(RecordingSelectorShot(kind: kind, display: display,
                    screen: screen, preferences: preferences), generation: generation, fromControls: false)
            } catch {
                self.finishRecordingScreenshot()
                self.showCaptureError("Couldn’t start capture", error)
            }
        }
        return true
    }

    /// Opens a child of the recording's capture flow for one screenshot, or
    /// returns nil when the take is busy or the flow refuses it.
    private func beginRecordingScreenshotFlow(_ context: String, fromControls: Bool) -> UInt64? {
        guard let parentGeneration = activeRecordingGeneration, recordingScreenshotGeneration == nil,
              recordingLifecycle.begin() else { return nil }
        do {
            let response = try AppBridge.flow([
                "operation": "begin_recording_screenshot",
                "parent_generation": parentGeneration,
                "seconds": 0,
            ])
            guard let value = response["generation"] as? NSNumber else {
                throw AppBridgeError.invalidResponse
            }
            recordingScreenshotGeneration = value.uint64Value
            recordingScreenshotSnapshotPending = false
            recordingHUD?.hud.setLifecycleActionsEnabled(false)
            updateRecordingMeter()
            return value.uint64Value
        } catch {
            recordingLifecycle.end()
            if fromControls { showRecordingError(context, error) } else { showCaptureError(context, error) }
            return nil
        }
    }

    /// Shipping `hide_capture_huds_before_snapshot`: the controls leave unless
    /// they are opted into captures, then the region or window selector opens.
    private func openRecordingSelector(_ shot: RecordingSelectorShot, generation: UInt64,
                                       fromControls: Bool) {
        recordingSelectorShot = shot
        recordingScreenshotPreviewGeneration = miniPreviews?.beginCapture(
            settings: shot.preferences.miniPreviewSettings)
        if recordingDisplayScreenshotHidesControls(
            includeControls: shot.preferences.includeRecordingControlsInCaptures) {
            recordingHUD?.orderOut(nil)
        }
        status.stringValue = shot.kind == .window
            ? "Preparing window screenshot… Press Escape to cancel."
            : "Preparing region screenshot… Press Escape to cancel."
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self, self.recordingScreenshotGeneration == generation else { return }
            if shot.kind == .window {
                self.prepareRecordingWindowSelector(shot, generation: generation, fromControls: fromControls)
            } else {
                self.prepareRecordingRegionSelector(shot, generation: generation, fromControls: fromControls)
            }
        }
    }

    private func reportRecordingSelectorError(_ error: Error, fromControls: Bool) {
        finishRecordingScreenshot()
        if fromControls { showRecordingError("Couldn’t prepare recording screenshot", error) }
        else { showCaptureError("Couldn’t prepare screenshot", error) }
    }

    /// Starts the screenshot countdown for a confirmed selection, then polls it.
    private func startRecordingSelectorCountdown(_ shot: RecordingSelectorShot, generation: UInt64) {
        do {
            _ = try AppBridge.flow([
                "operation": "start_countdown",
                "generation": generation,
                "seconds": shot.preferences.countdown,
            ])
            if shot.preferences.countdown > 0 {
                let countdown = ScreenshotCountdownPanel(screen: shot.screen, tokens: tokens,
                    remaining: shot.preferences.countdown)
                recordingScreenshotCountdownPanel = countdown
                countdown.orderFrontRegardless()
            }
            tickRecordingScreenshot(display: shot.display, preferences: shot.preferences,
                generation: generation)
        } catch {
            finishRecordingScreenshot()
            showCaptureError("Screenshot failed", error)
        }
    }

    /// Polls the child flow so Escape cancels while AppKit tracks a drag.
    private func startRecordingSelectorTimer(_ shot: RecordingSelectorShot, generation: UInt64) {
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
            self?.tickRecordingScreenshot(display: shot.display, preferences: shot.preferences,
                generation: generation)
        }
        recordingScreenshotTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private func prepareRecordingRegionSelector(_ shot: RecordingSelectorShot, generation: UInt64,
                                                fromControls: Bool) {
        run({
            let session = try NativeRegionSession.prepare(display: shot.display.id,
                generation: generation, preferences: shot.preferences)
            return (session, try session.image())
        }) { [weak self] result in
            guard let self, self.recordingScreenshotGeneration == generation else { return }
            do {
                let state = try AppBridge.flow(["operation": "poll", "generation": generation])
                guard state["current"] as? Bool == true else {
                    self.finishRecordingScreenshot()
                    return
                }
                let (session, image) = try result.get()
                guard session.logicalSize == shot.screen.frame.size else {
                    throw AppBridgeError.backend("The display changed. Select the region again.")
                }
                self.recordingScreenshotSession = session
                self.closeRecapturedPanels()
                let panel = RegionSelectionPanel(screen: shot.screen, image: image,
                    tokens: self.tokens, autoStart: true,
                    confirm: { [weak self] rect in
                        guard let self, self.recordingScreenshotGeneration == generation,
                              self.recordingScreenshotPanel != nil else { return }
                        self.recordingScreenshotRect = rect
                        self.recordingScreenshotPanel?.close()
                        self.recordingScreenshotPanel = nil
                        self.startRecordingSelectorCountdown(shot, generation: generation)
                    }, cancel: { [weak self] in
                        guard let self, self.recordingScreenshotGeneration == generation,
                              self.recordingScreenshotPanel != nil else { return }
                        self.finishRecordingScreenshot()
                        self.status.stringValue = "Screenshot cancelled; recording continues."
                    })
                self.recordingScreenshotPanel = panel
                panel.makeKeyAndOrderFront(nil)
                NSApp.activate(ignoringOtherApps: true)
                self.startRecordingSelectorTimer(shot, generation: generation)
            } catch {
                self.reportRecordingSelectorError(error, fromControls: fromControls)
            }
        }
    }

    private func prepareRecordingWindowSelector(_ shot: RecordingSelectorShot, generation: UInt64,
                                                fromControls: Bool) {
        run({
            let session = try NativeWindowSession.prepare(display: shot.display.id,
                generation: generation, preferences: shot.preferences)
            return (session, try session.image())
        }) { [weak self] result in
            guard let self, self.recordingScreenshotGeneration == generation else { return }
            do {
                let state = try AppBridge.flow(["operation": "poll", "generation": generation])
                guard state["current"] as? Bool == true else {
                    self.finishRecordingScreenshot()
                    return
                }
                let (session, image) = try result.get()
                guard session.display.size == shot.screen.frame.size else {
                    throw AppBridgeError.backend("The display changed. Select the window again.")
                }
                self.recordingScreenshotWindowSession = session
                self.closeRecapturedPanels()
                let panel = WindowSelectionPanel(screen: shot.screen, image: image,
                    targets: session.windows, tokens: self.tokens, autoStart: true,
                    hitTest: { [weak session] point in session?.hitTest(point) },
                    confirm: { [weak self] target in
                        guard let self, self.recordingScreenshotGeneration == generation,
                              self.recordingScreenshotWindowPanel != nil else { return }
                        self.recordingScreenshotWindowTarget = target
                        self.recordingScreenshotWindowPanel?.close()
                        self.recordingScreenshotWindowPanel = nil
                        self.startRecordingSelectorCountdown(shot, generation: generation)
                    }, cancel: { [weak self] in
                        guard let self, self.recordingScreenshotGeneration == generation,
                              self.recordingScreenshotWindowPanel != nil else { return }
                        self.finishRecordingScreenshot()
                        self.status.stringValue = "Screenshot cancelled; recording continues."
                    })
                self.recordingScreenshotWindowPanel = panel
                panel.makeKeyAndOrderFront(nil)
                NSApp.activate(ignoringOtherApps: true)
                panel.updatePointerLocation()
                self.startRecordingSelectorTimer(shot, generation: generation)
            } catch {
                self.reportRecordingSelectorError(error, fromControls: fromControls)
            }
        }
    }

    private func tickRecordingScreenshot(display: DisplayItem, preferences: CapturePreferences,
                                         generation: UInt64) {
        guard recordingScreenshotGeneration == generation else { return }
        do {
            let state = try AppBridge.flow(["operation": "poll", "generation": generation])
            guard state["current"] as? Bool == true else {
                if let panel = recordingScreenshotCountdownPanel {
                    recordingScreenshotCountdownPanel = nil; panel.closeAfterCancelling()
                }
                finishRecordingScreenshot()
                status.stringValue = "Screenshot cancelled; recording continues."
                return
            }
            guard recordingScreenshotPanel == nil, recordingScreenshotWindowPanel == nil,
                  let remaining = state["remaining"] as? Int else { return }
            recordingScreenshotCountdownPanel?.countdownContent.setRemaining(remaining)
            let regionSession = recordingScreenshotSession, regionRect = recordingScreenshotRect
            let windowSession = recordingScreenshotWindowSession
            let windowTarget = recordingScreenshotWindowTarget
            guard remaining == 0, !recordingScreenshotSnapshotPending,
                  (regionSession != nil && regionRect != nil)
                    || (windowSession != nil && windowTarget != nil) else { return }
            recordingScreenshotSnapshotPending = true
            recordingScreenshotCountdownPanel?.close()
            recordingScreenshotCountdownPanel = nil
            status.stringValue = "Capturing screenshot while recording…"
            let afterCountdown = preferences.countdown > 0
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                guard let self, self.recordingScreenshotGeneration == generation else { return }
                let root = self.historyRoot
                self.run({
                    if let windowSession, let windowTarget {
                        return try windowSession.capture(root: root, target: windowTarget,
                            afterCountdown: afterCountdown)
                    }
                    guard let regionSession, let regionRect else { throw AppBridgeError.invalidResponse }
                    return try regionSession.capture(root: root, rect: regionRect,
                        afterCountdown: afterCountdown)
                }) { [weak self] result in
                    guard let self, self.recordingScreenshotGeneration == generation else { return }
                    switch result {
                    case .success(let artifact):
                        let previewGeneration = self.recordingScreenshotPreviewGeneration
                        self.recordingScreenshotPreviewGeneration = nil
                        self.finishRecordingScreenshot(restorePreview: false)
                        self.miniPreviews?.present(artifact, on: display.id,
                            settings: preferences.miniPreviewSettings,
                            generation: previewGeneration)
                        self.loadHistory(select: artifact.id)
                        if preferences.autoCopy { self.copyImage(at: artifact.imagePath, artifactID: artifact.id) }
                    case .failure(let error):
                        self.finishRecordingScreenshot()
                        self.showCaptureError("Screenshot failed", error)
                    }
                }
            }
        } catch {
            finishRecordingScreenshot()
            showCaptureError("Screenshot failed", error)
        }
    }

    private func finishRecordingScreenshot(restorePreview: Bool = true) {
        recordingDisplayShot = nil
        recordingSelectorShot = nil
        closeRecapturedPanels()
        recordingScreenshotTimer?.invalidate(); recordingScreenshotTimer = nil
        recordingScreenshotCountdownPanel?.close(); recordingScreenshotCountdownPanel = nil
        recordingScreenshotPanel?.close(); recordingScreenshotPanel = nil
        recordingScreenshotSession = nil; recordingScreenshotRect = nil
        recordingScreenshotWindowPanel?.close(); recordingScreenshotWindowPanel = nil
        recordingScreenshotWindowSession = nil; recordingScreenshotWindowTarget = nil
        recordingScreenshotSnapshotPending = false
        if let generation = recordingScreenshotGeneration {
            _ = try? AppBridge.flow(["operation": "finish", "generation": generation])
            recordingScreenshotGeneration = nil
        }
        if restorePreview {
            miniPreviews?.restoreCapture(generation: recordingScreenshotPreviewGeneration)
            recordingScreenshotPreviewGeneration = nil
        }
        recordingLifecycle.end()
        if recordingSession != nil, !recordingControlsHidden, let hud = recordingHUD {
            hud.hud.setLifecycleActionsEnabled(true)
            hud.orderFrontRegardless()
        }
        updateRecordingMeter()
    }

    /// Shipping display shortcut and tray "Screenshot Display" during a
    /// recording (`start_capture_inner(Display)`): capture the display under
    /// the pointer with the screenshot countdown, without the recording
    /// controls unless they are opted into captures, into History, the
    /// clipboard and the mini previews. The take keeps running; only a child
    /// of its capture flow is used. Refuses silently when the take is busy.
    @discardableResult func captureDisplayWhileRecording() -> Bool {
        guard recordingDisplayScreenshotAvailable, let hud = recordingHUD,
              let parentGeneration = activeRecordingGeneration else { return false }
        // Shipping looks the display up for every capture: the pointer's.
        let pointer = pointerDisplayID()
        guard let display = displays.first(where: { $0.id == pointer }) ?? recordingDisplay,
              let screen = screen(for: display) else {
            showCaptureError("Couldn’t start capture",
                AppBridgeError.backend("The selected display is no longer available."))
            return false
        }
        guard recordingLifecycle.begin() else { return false }
        let generation: UInt64
        do {
            let response = try AppBridge.flow([
                "operation": "begin_recording_screenshot",
                "parent_generation": parentGeneration,
                "seconds": 0,
            ])
            guard let value = response["generation"] as? NSNumber else {
                throw AppBridgeError.invalidResponse
            }
            generation = value.uint64Value
        } catch {
            recordingLifecycle.end()
            showCaptureError("Couldn’t start capture", error)
            return false
        }
        recordingScreenshotGeneration = generation
        recordingScreenshotSnapshotPending = false
        hud.hud.setLifecycleActionsEnabled(false)
        updateRecordingMeter()
        status.stringValue = "Preparing screenshot… Press Escape to cancel."
        run({ [settingsPath] in try CapturePreferences.load(path: settingsPath) }) { [weak self] result in
            guard let self, self.recordingScreenshotGeneration == generation else { return }
            do {
                let preferences = try result.get()
                self.recordingDisplayShot = RecordingDisplayShot(display: display, screen: screen,
                    preferences: preferences)
                self.recordingScreenshotPreviewGeneration = self.miniPreviews?.beginCapture(
                    settings: preferences.miniPreviewSettings)
                let hides = recordingDisplayScreenshotHidesControls(
                    includeControls: preferences.includeRecordingControlsInCaptures)
                if hides { self.recordingHUD?.orderOut(nil) }
                DispatchQueue.main.asyncAfter(deadline: .now() + (hides ? 0.3 : 0)) { [weak self] in
                    self?.startRecordingDisplayCountdown(generation: generation)
                }
            } catch {
                self.finishRecordingScreenshot()
                self.showCaptureError("Couldn’t start capture", error)
            }
        }
        return true
    }

    private func startRecordingDisplayCountdown(generation: UInt64) {
        guard recordingScreenshotGeneration == generation, let shot = recordingDisplayShot else { return }
        do {
            _ = try AppBridge.flow([
                "operation": "start_countdown",
                "generation": generation,
                "seconds": shot.preferences.countdown,
            ])
            if shot.preferences.countdown > 0 {
                let countdown = ScreenshotCountdownPanel(screen: shot.screen, tokens: tokens,
                    remaining: shot.preferences.countdown)
                recordingScreenshotCountdownPanel = countdown
                countdown.orderFrontRegardless()
            }
            // Cancellation must still poll while AppKit tracks a drag/control.
            let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
                self?.tickRecordingDisplayScreenshot(generation: generation)
            }
            recordingScreenshotTimer = timer
            RunLoop.main.add(timer, forMode: .common)
            tickRecordingDisplayScreenshot(generation: generation)
        } catch {
            finishRecordingScreenshot()
            showCaptureError("Screenshot failed", error)
        }
    }

    private func tickRecordingDisplayScreenshot(generation: UInt64) {
        guard recordingScreenshotGeneration == generation, let shot = recordingDisplayShot else { return }
        do {
            let state = try AppBridge.flow(["operation": "poll", "generation": generation])
            guard state["current"] as? Bool == true else {
                if let panel = recordingScreenshotCountdownPanel {
                    recordingScreenshotCountdownPanel = nil; panel.closeAfterCancelling()
                }
                finishRecordingScreenshot()
                status.stringValue = "Screenshot cancelled; recording continues."
                return
            }
            guard let remaining = state["remaining"] as? Int else { throw AppBridgeError.invalidResponse }
            recordingScreenshotCountdownPanel?.countdownContent.setRemaining(remaining)
            guard remaining == 0, !recordingScreenshotSnapshotPending else { return }
            recordingScreenshotSnapshotPending = true
            recordingScreenshotCountdownPanel?.close()
            recordingScreenshotCountdownPanel = nil
            status.stringValue = "Capturing screenshot while recording…"
            let preferences = shot.preferences
            let displayID = shot.display.id
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                guard let self, self.recordingScreenshotGeneration == generation else { return }
                self.run({ [transport, historyRoot] in
                    let result = try transport.request(["operation": "capture_display",
                        "root": historyRoot, "display_id": displayID, "generation": generation,
                        "include_cursor": preferences.includeCursor])
                    guard let value = result["artifact"] as? [String: Any],
                          let artifact = CaptureArtifact(value) else { throw AppBridgeError.invalidResponse }
                    return artifact
                }) { [weak self] result in
                    guard let self, self.recordingScreenshotGeneration == generation else { return }
                    switch result {
                    case .success(let artifact):
                        let previewGeneration = self.recordingScreenshotPreviewGeneration
                        self.recordingScreenshotPreviewGeneration = nil
                        self.finishRecordingScreenshot(restorePreview: false)
                        self.miniPreviews?.present(artifact, on: displayID,
                            settings: preferences.miniPreviewSettings,
                            generation: previewGeneration)
                        self.loadHistory(select: artifact.id)
                        if preferences.autoCopy { self.copyImage(at: artifact.imagePath, artifactID: artifact.id) }
                    case .failure(let error):
                        self.finishRecordingScreenshot()
                        self.showCaptureError("Screenshot failed", error)
                    }
                }
            }
        } catch {
            finishRecordingScreenshot()
            showCaptureError("Screenshot failed", error)
        }
    }

    func hideRecordingControls() {
        guard recordingSession != nil, let hud = recordingHUD,
              !recordingLifecycle.busy, !recordingControlsHidden,
              let screen = hud.screen ?? recordingDisplay.flatMap(screen(for:)) else { return }
        recordingControlsHidden = true
        updateRecordingMeter()
        hud.orderOut(nil)
        let notice = RecordingControlsHiddenNoticePanel(screen: screen, tokens: tokens,
            shortcut: recordingPreferences?.newCaptureShortcut ?? "")
        recordingHiddenNotice = notice
        notice.orderFrontRegardless()
        // Shipping `recording-controls-hidden-lifecycle` (6 s) ends 200 ms before the close.
        let motion = RecordingControlsHiddenNoticePanel.motion
        if let content = notice.contentView {
            NativeMotion.playEntrance(motion, on: content, tokens: tokens)
            let exitAt = 6.0 - NativeMotion.exitDuration(motion, tokens: tokens)
            if !NativeMotion.reduceMotion {
                DispatchQueue.main.asyncAfter(deadline: .now() + exitAt) { [weak self, weak notice] in
                    guard let self, let notice, self.recordingHiddenNotice === notice,
                          let content = notice.contentView else { return }
                    NativeMotion.playExit(motion, on: content, tokens: self.tokens)
                }
            }
        }
        recordingHiddenNoticeTimer?.invalidate()
        recordingHiddenNoticeTimer = Timer.scheduledTimer(withTimeInterval: 6.2,
            repeats: false) { [weak self, weak notice] _ in
                guard let self, self.recordingHiddenNotice === notice else { return }
                notice?.close()
                self.recordingHiddenNotice = nil
                self.recordingHiddenNoticeTimer = nil
            }
        recordingControlsVisibilityChanged(true)
    }

    @discardableResult func showRecordingControls() -> Bool {
        guard recordingSession != nil, let hud = recordingHUD, recordingControlsHidden else {
            return false
        }
        recordingControlsHidden = false
        recordingHiddenNoticeTimer?.invalidate(); recordingHiddenNoticeTimer = nil
        recordingHiddenNotice?.close(); recordingHiddenNotice = nil
        hud.sharingType = recordingCapabilities?.controlsExcluded == true ? .none : .readOnly
        hud.orderFrontRegardless()
        updateRecordingMeter()
        recordingControlsVisibilityChanged(false)
        return true
    }

    private func clearRecordingControlsHiddenState() {
        let changed = recordingControlsHidden
        recordingControlsHidden = false
        recordingHiddenNoticeTimer?.invalidate(); recordingHiddenNoticeTimer = nil
        recordingHiddenNotice?.close(); recordingHiddenNotice = nil
        if changed { recordingControlsVisibilityChanged(false) }
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
        hud.hud.actionStarted()
        hud.hud.setLifecycleActionsEnabled(false)
        updateRecordingMeter()
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
                self.updateRecordingMeter()
                self.status.stringValue = snapshot.warning
                    ?? (snapshot.state == "paused" ? "Recording paused." : "Recording in progress…")
            } catch {
                if wasPaused {
                    // Shipping leaves the take paused, with the error on the HUD,
                    // when the engine cannot reopen (for example a missing
                    // microphone). Cancellation still saves the accepted segments.
                    _ = self.recordingLifecycle.begin()
                    hud.hud.setLifecycleActionsEnabled(false)
                    self.run({ try session.snapshot() }) { [weak self] snapshotResult in
                        guard let self, self.recordingSession === session else { return }
                        self.recordingLifecycle.end()
                        hud.hud.setLifecycleActionsEnabled(true)
                        let current = (try? AppBridge.flow(["operation": "poll",
                            "generation": generation]))?["current"] as? Bool == true
                        switch snapshotResult {
                        case .success(let snapshot) where snapshot.state == "paused" && current:
                            hud.hud.setPaused(true, elapsedMilliseconds: snapshot.elapsedMilliseconds)
                            hud.hud.showActionError(error.localizedDescription)
                            self.updateRecordingMeter()
                            self.status.stringValue = "Recording paused; resume failed."
                        case .success(let snapshot) where snapshot.state == "failed":
                            self.preserveFailedRecording(session, warning: error.localizedDescription)
                        default:
                            self.stopRecording()
                            self.showError("Couldn’t resume recording; saving the existing take", error)
                        }
                    }
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

    private func updateRecordingMeter() {
        defer { publishRecordingDisplayRoute() }
        guard let session = recordingSession, let hud = recordingHUD,
              let meter = recordingMeter else {
            recordingMeter?.setActive(false)
            return
        }
        meter.setActive(recordingSession === session && !recordingLifecycle.busy
            && !recordingControlsHidden
            && recordingScreenshotGeneration == nil && !recordingPendingStart
            && hud.isVisible && !hud.hud.isHidden && !hud.hud.paused
            && !hud.hud.microphoneMuted && hud.hud.microphoneAvailable)
    }

    /// Tells the host when the display shortcut starts or stops routing to a
    /// screenshot beside the take, so it can update the shortcut routes.
    private func publishRecordingDisplayRoute() {
        let available = recordingDisplayScreenshotAvailable
        guard available != publishedRecordingDisplayRoute else { return }
        publishedRecordingDisplayRoute = available
        recordingControlsVisibilityChanged(recordingControlsHidden)
    }

    private func toggleRecordingMicrophone() {
        guard let session = recordingSession, let hud = recordingHUD,
              let generation = activeRecordingGeneration,
              recordingLifecycle.begin() else { return }
        let muted = !hud.hud.microphoneMuted
        let excludeCapturesApp = recordingCapabilities?.controlsExcluded == true
        hud.hud.actionStarted()
        hud.hud.setLifecycleActionsEnabled(false)
        updateRecordingMeter()
        status.stringValue = muted ? "Muting microphone…" : "Unmuting microphone…"
        run({ [recordingGate] in
            try session.setMicrophoneMuted(muted, generation: generation,
                excludeCapturesApp: excludeCapturesApp, gate: recordingGate)
        }) { [weak self] result in
            guard let self, self.recordingSession === session else { return }
            switch result {
            case .success(let snapshot):
                guard snapshot.state != "failed" else {
                    self.preserveFailedRecording(session, warning: snapshot.warning)
                    return
                }
                self.recordingLifecycle.end()
                hud.hud.setLifecycleActionsEnabled(true)
                hud.hud.setPaused(snapshot.state == "paused",
                    elapsedMilliseconds: snapshot.elapsedMilliseconds)
                hud.hud.setMicrophone(muted: snapshot.microphoneMuted,
                    available: snapshot.hasMicrophone)
                hud.hud.setWarning(snapshot.warning)
                self.updateRecordingMeter()
                self.status.stringValue = snapshot.warning
                    ?? (snapshot.state == "paused" ? "Recording paused." : "Recording in progress…")
            case .failure(let mutationError):
                // The shared owner may have durably completed the old segment
                // before cancellation or a replacement-device failure. Read its
                // state on the same worker before deciding how to preserve it.
                self.run({ try session.snapshot() }) { [weak self] snapshotResult in
                    guard let self, self.recordingSession === session else { return }
                    self.recordingLifecycle.end()
                    hud.hud.setLifecycleActionsEnabled(true)
                    let current = (try? AppBridge.flow(["operation": "poll",
                        "generation": generation]))?["current"] as? Bool == true
                    switch snapshotResult {
                    case .success(let snapshot) where snapshot.state == "failed":
                        self.preserveFailedRecording(session,
                            warning: mutationError.localizedDescription)
                    case .success(let snapshot) where snapshot.state == "paused" && current:
                        // Shipping keeps the take paused when the replacement
                        // segment cannot open and shows the error on the HUD.
                        hud.hud.setPaused(true, elapsedMilliseconds: snapshot.elapsedMilliseconds)
                        hud.hud.setMicrophone(muted: snapshot.microphoneMuted,
                            available: snapshot.hasMicrophone)
                        hud.hud.showActionError(mutationError.localizedDescription)
                        self.updateRecordingMeter()
                        self.status.stringValue = "Recording paused; the microphone change failed."
                    case .success(let snapshot):
                        hud.hud.setPaused(snapshot.state == "paused",
                            elapsedMilliseconds: snapshot.elapsedMilliseconds)
                        hud.hud.setMicrophone(muted: snapshot.microphoneMuted,
                            available: snapshot.hasMicrophone)
                        self.stopRecording()
                        self.showError("Couldn’t change microphone; saving the existing take",
                            mutationError)
                    case .failure(let snapshotError):
                        self.preserveFailedRecording(session,
                            warning: "\(mutationError.localizedDescription); \(snapshotError.localizedDescription)")
                    }
                }
            }
        }
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

        clearRecordingControlsHiddenState()
        updateRecordingMeter()
        hud.hud.actionStarted()
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
                guard snapshot.state != "failed" else {
                    self.preserveFailedRecording(session, warning: snapshot.warning)
                    return
                }
                let flow = try AppBridge.flow(["operation": "poll", "generation": generation])
                guard flow["current"] as? Bool == true else {
                    self.recordingLifecycle.end()
                    self.discardRecording()
                    return
                }
                self.recordingHUD?.close(); self.recordingHUD = nil
                self.recordingRegionPanel?.orderFrontRegardless()
                if preferences.recording.countdown > 0,
                   let screen = self.screen(for: display) {
                    let countdown = ScreenshotCountdownPanel(screen: screen, tokens: self.tokens,
                        remaining: preferences.recording.countdown, kind: .recording)
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

    /// Shipping `deleteRecording` asks before discarding a started take.
    private func confirmDeleteRecording() {
        guard !recordingLifecycle.busy else { return }
        let alert = NSAlert()
        alert.messageText = "Delete recording?"
        alert.informativeText = "This recording will be deleted permanently."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        discardRecording()
    }

    private func confirmRestartRecording() {
        guard !recordingLifecycle.busy else { return }
        // Shipping "Retry recording" replaces a failed take without asking.
        if recordingHUD?.hud.restartConfirms == false {
            restartRecording()
            return
        }
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
                self.recordingHUD?.hud.setMicrophone(muted: snapshot.microphoneMuted,
                    available: snapshot.hasMicrophone)
                self.recordingHUD?.hud.setWarning(snapshot.warning)
                self.updateRecordingMeter()
                if let warning = snapshot.warning { self.status.stringValue = warning }
            case .failure(let error):
                self.recordingPollTimer?.invalidate(); self.recordingPollTimer = nil
                self.recordingMeter?.setActive(false)
                self.showError("Couldn’t refresh recording status", error)
            }
        }
    }

    private func preserveFailedRecording(_ session: NativeRecordingSession, warning: String?) {
        guard recordingSession === session else { return }
        recordingMeter?.setActive(false); recordingMeter = nil
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        recordingGate.set(nil); activeRecordingGeneration = nil
        recordingLifecycle.end()
        recordingSession = nil
        clearRecordingControlsHiddenState()
        recordingHUD?.close(); recordingHUD = nil
        retireRecordingSession(session)
        finishCapture()
        showError("Recording stopped; recovery files were preserved",
            AppBridgeError.backend(warning ?? "The recording engine stopped unexpectedly."))
    }

    private func stopRecording() {
        guard let session = recordingSession, recordingLifecycle.begin() else { return }
        recordingMeter?.setActive(false); recordingMeter = nil
        let noticeScreen = recordingHUD?.screen ?? recordingDisplay.flatMap(screen(for:))
        let openEditor = recordingPreferences?.recording.openEditorAfterRecording ?? true
        clearRecordingControlsHiddenState()
        recordingHUD?.hud.actionStarted()
        recordingHUD?.hud.setLifecycleActionsEnabled(false)
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        status.stringValue = "Finalizing recording…"
        // Shipping keeps the HUD up as "Saving…" until the take is published.
        recordingHUD?.hud.setSaving()
        run({ [historyRoot] in
            _ = try session.stop()
            return try session.finish(historyRoot: historyRoot, tools: NativeMediaTools.locate())
        }) { [weak self] result in
            guard let self, self.recordingSession === session else { return }
            switch result {
            case .success(let finalized):
                self.recordingSession = nil
                self.retireRecordingSession(session)
                self.recordingHUD?.close(); self.recordingHUD = nil
                self.recordingGate.set(nil); self.activeRecordingGeneration = nil
                self.recordingLifecycle.end()
                self.finishCapture()
                let finalStatus = finalized.warning
                    ?? "Recording saved to \(finalized.path)"
                let noticeGeneration = self.recordingSavedNotice.model.generation
                self.loadHistory(select: finalized.id) { [weak self] in
                    guard let self,
                          self.recordingSavedNotice.model.generation == noticeGeneration,
                          !self.capturing,
                          self.artifacts.contains(where: { $0.id == finalized.id }) else { return }
                    self.status.stringValue = finalStatus
                    // Shipping opens the recording editor when the preference is
                    // on; the saved notice follows that editor's close.
                    guard openEditor,
                          let artifact = self.artifacts.first(where: { $0.id == finalized.id })
                    else { return }
                    if let noticeScreen {
                        self.recordingEditorNoticeScreens[finalized.id] = noticeScreen
                        self.lastRecordingNoticeScreen = noticeScreen
                    }
                    self.presentEditor(artifact, requiresCurrentSelection: false)
                }
            case .failure(let error):
                self.preserveFailedRecording(session, warning: error.localizedDescription)
            }
        }
    }

    private func discardRecording() {
        guard let session = recordingSession, recordingLifecycle.begin() else { return }
        recordingMeter?.setActive(false)
        clearRecordingControlsHiddenState()
        recordingHUD?.hud.actionStarted()
        recordingHUD?.hud.setLifecycleActionsEnabled(false)
        recordingPollTimer?.invalidate(); recordingPollTimer = nil
        status.stringValue = "Discarding recording…"
        recordingHUD?.hud.isHidden = true
        run({ try session.discard() }) { [weak self] result in
            guard let self, self.recordingSession === session else { return }
            switch result {
            case .success:
                self.recordingSession = nil
                self.recordingMeter = nil
                self.retireRecordingSession(session)
                self.recordingHUD?.close(); self.recordingHUD = nil
                self.recordingGate.set(nil); self.activeRecordingGeneration = nil
                self.recordingLifecycle.end()
                self.finishCapture(); self.status.stringValue = "Recording discarded."
            case .failure(let error):
                self.recordingLifecycle.end()
                if let hud = self.recordingHUD {
                    self.run({ try session.snapshot() }) { [weak self] snapshotResult in
                        guard let self, self.recordingSession === session else { return }
                        if case .success(let snapshot) = snapshotResult,
                           ["failed", "discarded", "ready"].contains(snapshot.state) {
                            self.preserveFailedRecording(session, warning: error.localizedDescription)
                        } else {
                            hud.hud.isHidden = false
                            hud.hud.setLifecycleActionsEnabled(true)
                            self.updateRecordingMeter()
                            self.showRecordingError("Couldn’t discard recording", error)
                        }
                    }
                } else {
                    self.preserveFailedRecording(session, warning: error.localizedDescription)
                }
            }
        }
    }

    private func retireRecordingSession(_ session: NativeRecordingSession) {
        recordingRetiring = true; updateActions()
        // The first queue item drops the last owner after earlier worker work;
        // the second is a barrier before allowing a new lease/list request.
        Self.queue.async { withExtendedLifetime(session) {} }
        Self.queue.async { [weak self] in
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.recordingRetiring = false; self.updateActions(); self.refreshRecovery()
                self.processNextOpenImage()
            }
        }
    }

    func finishCapture(restoreWindow: Bool = true, restorePreview: Bool = true) {
        if recordingScreenshotGeneration != nil {
            finishRecordingScreenshot()
        }
        recordingMeter?.setActive(false); recordingMeter = nil
        if preparingRecording { recordingRetiring = true }
        recordingSavedNotice.dismiss()
        recordingRegionPanel?.close(); recordingRegionPanel = nil
        clearRecordingControlsHiddenState()
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
            retireRecordingSession(session)
        }
        selectorShortcutGeneration = nil
        countdownTimer?.invalidate(); countdownTimer = nil
        countdownPanel?.close(); countdownPanel = nil
        closeRecapturedPanels(); capturePreferences = nil
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
        recordingCapabilities = nil; microphoneDevices = []; microphonesLoaded = false
        if let generation = flowGeneration {
            _ = try? AppBridge.flow(["operation": "finish", "generation": generation])
            flowGeneration = nil
        }
        if restorePreview {
            miniPreviews?.restoreCapture(generation: previewCaptureGeneration)
            previewCaptureGeneration = nil
        }
        snapshotPending = false; setBusy(false)
        publishRecordingDisplayRoute()
        if restoreWindow { workspaceHidden?(false) }
        switch windowRestoration.finish(restoreRequested: restoreWindow) {
        case .none: break
        case .visible:
            window.orderFront(nil)
        case .key:
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    private func setBusy(_ busy: Bool, message: String = "") {
        capturing = busy
        captureStateChanged(busy || permissionsVisible)
        updateActions()
        if busy { status.stringValue = message }
        else { processNextOpenImage() }
    }
    /// Shipping `.history-error` (`role="alert"`) below the filters. It only
    /// shows History load and delete failures; capture and shortcut failures
    /// are host dialogs.
    private func showAlert(_ message: String, detail: String? = nil) {
        status.stringValue = message
        status.toolTip = detail
        status.textColor = tokens.color("danger-text")
        statusAlert = true
        layoutHistory()
    }
    private func showHistoryError(_ context: String, _ error: Error) {
        showAlert("\(context): \(error.localizedDescription)")
    }
    private func showError(_ context: String, _ error: Error) {
        let message = "\(context): \(error.localizedDescription)"
        showAlert(message)
        reportFailure(message)
    }
    /// Shipping `report_capture_error`: a failed capture is a modal dialog
    /// (or Screen Recording recovery), not the History error card.
    private func showCaptureError(_ context: String, _ error: Error) {
        reportFailure("\(context): \(error.localizedDescription)")
    }
    private func reportFailure(_ message: String) {
        if let kind = captureAttemptKind, screenPermissionDenied(kind, message) {
            captureAttemptKind = nil
            return
        }
        reportError(message)
    }

    /// Runs the capture a Screen Recording relaunch scheduled, once the
    /// display list is ready (shipping retries right after setup).
    func retryCaptureAfterRestart(_ kind: StillCaptureKind) {
        guard displays.isEmpty else { _ = capture(kind); return }
        pendingDisplayCapture = .still(kind)
        loadDisplays()
    }
    private func updateActions() {
        let busy = historyBusy
        for (filter, button) in historyFilterButtons {
            button.isEnabled = !busy && (filter == .all || artifacts.contains(where: filter.matches))
        }
        renderRecovery()
        layoutHeaderActions()
        refreshGrid(busy: busy)
    }

    private func select(_ index: Int) {
        guard artifacts.indices.contains(index) else { clearSelection(); return }
        selectedIndex = index
        updateActions()
    }
    private func clearSelection() {
        selectedIndex = nil
        if let grid, grid.selectedRow >= 0 { grid.setSelectedRow(-1, notify: false) }
        updateActions()
    }

    /// Shipping shows the saved notice whenever a recording editor closes and
    /// the recording is still in History.
    private func recordingEditorClosed(_ artifactID: String) {
        let screen = recordingEditorNoticeScreens.removeValue(forKey: artifactID)
            ?? lastRecordingNoticeScreen ?? NSScreen.main
        guard !capturing, artifacts.contains(where: { $0.id == artifactID }),
              let screen else { return }
        recordingSavedNotice.present(artifactID: artifactID, screen: screen)
    }

    private func editScreenshot() {
        guard let index = selectedIndex, artifacts.indices.contains(index),
              !historyRoot.isEmpty, !externalOpenPending else { return }
        let artifact = artifacts[index]
        presentEditor(artifact)
    }

    private func presentEditor(_ artifact: CaptureArtifact, outputDirectory: String? = nil,
                               requiresCurrentSelection: Bool = true,
                               completion: (() -> Void)? = nil) {
        guard !editorTerminationPending else { completion?(); return }
        run({ [settingsPath] in
            if artifact.isRecording {
                let preferences = try CapturePreferences.load(path: settingsPath)
                return (outputDirectory ?? preferences.directory, Optional(preferences.recording))
            }
            return (try outputDirectory ?? CapturePreferences.load(path: settingsPath).directory,
                    Optional<RecordingPreferences>.none)
        }) {
            [weak self] result in
            guard let self else { return }
            guard !self.editorTerminationPending else { completion?(); return }
            guard completion != nil || !self.externalOpenPending else { return }
            guard completion != nil || !requiresCurrentSelection
                || self.selectedIndex.flatMap({ self.artifacts.indices.contains($0)
                ? self.artifacts[$0].id : nil }) == artifact.id else { return }
            switch result {
            case .success(let (outputDirectory, recordingPreferences)):
                if artifact.isRecording {
                    if self.recordingEditor == nil {
                        self.recordingEditor = RecordingEditorController(
                            tokens: self.tokens,
                            reportError: { [weak self] message in self?.reportError(message) },
                            didSaveCopy: { [weak self] in self?.loadHistory() },
                            didReplaceOriginal: { [weak self] artifactID in
                                self?.miniPreviews?.dismiss(artifactID, exit: nil)
                                self?.loadHistory(select: artifactID)
                            })
                        self.recordingEditor?.didClose = { [weak self] artifactID in
                            self?.recordingEditorClosed(artifactID)
                        }
                    }
                    self.recordingEditor?.present(artifact: artifact,
                                                  historyRoot: self.historyRoot,
                                                  outputDirectory: outputDirectory,
                                                  recordingPreferences: recordingPreferences,
                                                  completion: completion.map { finished in
                        { [weak self] accepted in
                            if !accepted {
                                self?.externalOpenErrors.append("\(artifact.id): recording editor could not open; the History item remains available.")
                            }
                            finished()
                        }
                    })
                    return
                }
                if self.screenshotEditor == nil {
                    self.screenshotEditor = ScreenshotEditorController(
                        tokens: self.tokens,
                        reportError: { [weak self] message in self?.reportError(message) },
                        didSaveCopy: { [weak self] in self?.loadHistory() },
                        didReplaceOriginal: { [weak self] artifactID in
                            self?.miniPreviews?.dismiss(artifactID, exit: nil)
                            self?.loadHistory(select: artifactID)
                        })
                    // Mini previews show "In editor" while this window shows their capture.
                    self.screenshotEditor?.presenceChanged = { [weak self] artifactID in
                        self?.miniPreviews?.setEditorArtifacts(Set(artifactID.map { [$0] } ?? []))
                    }
                }
                self.screenshotEditor?.present(artifact: artifact, historyRoot: self.historyRoot,
                    outputDirectory: outputDirectory, completion: completion.map { finished in
                        { [weak self] accepted in
                            if !accepted {
                                self?.externalOpenErrors.append("\(artifact.id): screenshot editor could not open; the History item remains available.")
                            }
                            finished()
                        }
                    })
            case .failure(let error):
                self.showError("Couldn’t load the media save location", error)
                completion?()
            }
        }
    }
    private func save(_ artifact: CaptureArtifact,
                      completion: ((Result<String, Error>) -> Void)? = nil) {
        let noun = artifact.isRecording ? "recording" : "image"
        status.stringValue = "Saving \(noun)…"
        miniPreviews?.setStatus("", for: artifact.id)
        run({ [transport, historyRoot, settingsPath] in
            let preferences = try CapturePreferences.load(path: settingsPath)
            let result = try transport.request(["operation": artifact.isRecording ? "save_recording" : "save_screenshot", "root": historyRoot, "id": artifact.id,
                "directory": preferences.directory, "format": preferences.format])
            guard let value = result["artifact"] as? [String: Any], let updated = CaptureArtifact(value),
                  let path = result["path"] as? String,
                  !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            else { throw AppBridgeError.invalidResponse }
            let card = (try? HistoryCard.cards(for: [value]))?.first ?? nil
            return (updated, path, card)
        }) { [weak self] result in
            guard let self else { return }
            if self.cardBusy?.id == artifact.id { self.cardBusy = nil }
            switch result { case .success(let value):
                if let current = self.artifacts.firstIndex(where: { $0.id == artifact.id }) { self.artifacts[current] = value.0 }
                if let card = value.2 { self.cards[artifact.id] = card }
                self.status.stringValue = "Saved \(noun) to \(value.1)"
                _ = self.miniPreviews?.updateSavedPath(value.1, for: artifact)
                self.miniPreviews?.setStatus("", for: artifact.id)
                self.miniPreviews?.showSavedFeedback(for: artifact.id)
                completion?(.success(value.1))
                if completion == nil {
                    self.recordingSavedNotice.savedFromHistory(artifactID: artifact.id, path: value.1)
                }
            case .failure(let error):
                self.showError("Couldn’t save \(noun)", error)
                self.miniPreviews?.setStatus("Save failed", for: artifact.id)
                completion?(.failure(error))
            }; self.updateActions()
        }
    }
    private func copyImage(at path: String, artifactID: String) {
        run({ try Data(contentsOf: URL(fileURLWithPath: path)) }) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let png):
                let pasteboard = NSPasteboard.general; pasteboard.clearContents()
                let copied = pasteboard.setData(png, forType: .png)
                self.status.stringValue = copied
                    ? "Copied the selected image." : "Couldn’t copy the selected image."
                self.miniPreviews?.recordCopyResult(artifactID: artifactID, succeeded: copied)
                if copied {
                    self.miniPreviews?.recordClipboardCopy(artifactID: artifactID, pasteboard: pasteboard)
                }
            case .failure(let error):
                self.miniPreviews?.recordCopyResult(artifactID: artifactID, succeeded: false)
                self.showError("Couldn’t copy image", error)
            }
        }
    }
    func openPreview(_ artifact: CaptureArtifact) {
        guard !externalOpenPending, !permissionsVisible, !capturing, !clearingHistory,
              !recoveryBusy, !recordingRetiring else { return }
        presentEditor(artifact, requiresCurrentSelection: false)
    }
    func refreshHistory() { loadHistory() }

    /// The visible screenshot or recording editor window, which reopen focuses
    /// before History (shipping `primary_app_window_priority`).
    var visibleEditorWindow: NSWindow? {
        [screenshotEditor?.window, recordingEditor?.window].compactMap { $0 }.first { $0.isVisible }
    }

    func openImages(_ paths: [String]) {
        pendingOpenImages.append(contentsOf: paths)
        // loadInitial owns root discovery and the first History reload.
        guard !historyRoot.isEmpty else { return }
        processNextOpenImage()
    }

    private func processNextOpenImage() {
        guard !externalOpenPending, !historyRoot.isEmpty, !capturing,
              !clearingHistory, !recoveryBusy,
              !recordingRetiring, !permissionsVisible, !editorTerminationPending else { return }
        guard !pendingOpenImages.isEmpty else {
            if !externalOpenErrors.isEmpty {
                let message = externalOpenErrors.joined(separator: "\n")
                showAlert(externalOpenErrors.count == 1
                    ? message : "Couldn’t open \(externalOpenErrors.count) files. See details.", detail: message)
                reportError("Couldn’t open external media: \(message)")
                externalOpenErrors.removeAll()
            }
            return
        }
        let path = pendingOpenImages.removeFirst()
        let selectedAtDispatch = userSelectionGeneration
        externalOpenPending = true; updateActions()
        status.stringValue = "Opening media…"
        // Resolve editor settings first: a successful import must be openable.
        run({ [settingsPath] in try CapturePreferences.load(path: settingsPath).directory }) {
            [weak self] settingsResult in
            guard let self else { return }
            switch settingsResult {
            case .failure(let error):
                self.externalOpenErrors.append("\(path): \(error.localizedDescription)")
                self.externalOpenPending = false; self.updateActions(); self.processNextOpenImage()
            case .success(let outputDirectory):
                // Read active editors on the main thread immediately before
                // dispatch, not before an asynchronous settings read.
                let openIDs = (self.screenshotEditor?.activeArtifactID.map { [$0] } ?? [])
                    + (self.recordingEditor?.activeArtifactID.map { [$0] } ?? [])
                self.run({ [transport = self.transport, historyRoot = self.historyRoot] in
                    var request: [String: Any] = ["operation": "open_media", "root": historyRoot,
                        "path": path, "open_artifact_ids": openIDs]
                    // Resolve paths independent of suffix: the shared API detects
                    // content and invokes tools only for a new recording open.
                    if let tools = try? NativeMediaTools.locate() {
                        request["ffmpeg"] = tools.ffmpeg
                        request["ffprobe"] = tools.ffprobe
                    }
                    let response = try transport.request(request)
                    guard let value = response["artifact"] as? [String: Any],
                          let artifact = CaptureArtifact(value),
                          response["already_open"] is Bool else { throw AppBridgeError.invalidResponse }
                    return artifact
                }) { [weak self] result in
                    guard let self else { return }
                    switch result {
                    case .failure(let error):
                        self.externalOpenErrors.append("\(path): \(error.localizedDescription)")
                        self.externalOpenPending = false; self.updateActions(); self.processNextOpenImage()
                    case .success(let artifact):
                        // Do not steal a newer user selection. Still refresh History so
                        // the imported item appears even when focus has changed.
                        let shouldOpen = self.userSelectionGeneration == selectedAtDispatch
                        var opened = false
                        self.loadHistory(select: shouldOpen ? artifact.id : nil,
                            selectIfUserGeneration: selectedAtDispatch, cleanup: { [weak self] in
                            guard let self, !opened else { return }
                            self.externalOpenPending = false; self.updateActions(); self.processNextOpenImage()
                        }) { [weak self] in
                            guard let self, shouldOpen,
                                  self.userSelectionGeneration == selectedAtDispatch,
                                  self.selectedIndex.flatMap({ self.artifacts.indices.contains($0)
                                      ? self.artifacts[$0].id : nil }) == artifact.id else { return }
                            opened = true
                            self.window.makeKeyAndOrderFront(nil)
                            NSApp.activate(ignoringOtherApps: true)
                            self.presentEditor(artifact, outputDirectory: outputDirectory) { [weak self] in
                                guard let self else { return }
                                self.externalOpenPending = false; self.updateActions(); self.processNextOpenImage()
                            }
                        }
                    }
                }
            }
        }
    }
    private func reveal(_ artifact: CaptureArtifact) {
        guard let path = artifact.savedPath else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
    private func delete(_ artifact: CaptureArtifact) {
        status.stringValue = "Deleting from history…"
        run({ [transport, historyRoot] in _ = try transport.request(["operation": "delete", "root": historyRoot, "id": artifact.id]) }) { [weak self] result in
            switch result { case .success: self?.loadHistory()
            case .failure(let error): self?.showHistoryError("Couldn’t delete capture", error) }
        }
    }

    /// Shipping header Delete all: the first click arms "Delete all forever"
    /// (with Cancel) for four seconds; the second deletes every capture kind,
    /// including captures outside the selected filter.
    private func deleteAllHistory() {
        guard !artifacts.isEmpty, !historyBusy else { return }
        guard confirmDeleteAll else {
            confirmDeleteID = nil; confirmDeleteTimer?.invalidate(); confirmDeleteTimer = nil
            confirmDeleteAll = true
            confirmDeleteAllTimer?.invalidate()
            confirmDeleteAllTimer = Timer.scheduledTimer(withTimeInterval: historyCopy.confirmTimeout, repeats: false) {
                [weak self] _ in
                self?.confirmDeleteAll = false; self?.confirmDeleteAllTimer = nil; self?.updateActions()
            }
            updateActions()
            return
        }
        confirmDeleteAll = false; confirmDeleteAllTimer?.invalidate(); confirmDeleteAllTimer = nil
        clearingHistory = true; updateActions(); status.stringValue = "Clearing history…"
        run({ [transport, historyRoot] in
            _ = try transport.request(["operation": "clear_history", "root": historyRoot])
        }) { [weak self] result in
            guard let self else { return }
            // A failed bulk delete can still remove some entries; always reload.
            self.loadHistory(cleanup: { [weak self] in
                guard let self else { return }
                self.clearingHistory = false; self.updateActions()
                if case .failure(let error) = result { self.showHistoryError("Couldn’t delete capture history", error) }
                self.processNextOpenImage()
            })
        }
    }

    private func run<T>(_ work: @escaping () throws -> T, completion: @escaping (Result<T, Error>) -> Void) {
        // Like shipping, starting an operation clears the previous error.
        status.textColor = tokens.color("text-muted")
        if statusAlert { statusAlert = false; layoutHistory() }
        Self.queue.async { let result = Result(catching: work); DispatchQueue.main.async { completion(result) } }
    }

    // One process-wide queue also drains operations from a closed workspace view.
    func prepareEditorForTermination() -> Bool {
        guard !editorTerminationPending, canPrepareEditorsForTermination() else { return false }
        guard screenshotEditor?.prepareForTermination() ?? true else { return false }
        return recordingEditor?.prepareForTermination() ?? true
    }

    func prepareEditorForTermination(completion: @escaping (Bool) -> Void) {
        guard !editorTerminationPending, canPrepareEditorsForTermination() else { completion(false); return }
        editorTerminationPending = true
        updateActions()
        let finished: (Bool) -> Void = { [weak self] prepared in
            guard let self else { completion(false); return }
            // Media arriving during the drain remains queued and cancels Quit.
            let accepted = prepared && self.canPrepareEditorsForTermination()
                && (self.recordingEditor?.prepareForTermination() ?? true)
            self.editorTerminationPending = false
            self.updateActions()
            completion(accepted)
            if !accepted { self.processNextOpenImage() }
        }
        if let screenshotEditor { screenshotEditor.prepareForTermination(completion: finished) }
        else { finished(true) }
    }

    private func canPrepareEditorsForTermination() -> Bool {
        if externalOpenPending || !pendingOpenImages.isEmpty {
            status.stringValue = "Wait for external images to finish opening before quitting."
            return false
        }
        if recoveryBusy || recordingRetiring {
            status.stringValue = recordingRetiring
                ? "Wait for recording media to finish before quitting."
                : "Wait for or cancel recording recovery before quitting."
            return false
        }
        return true
    }

    static func flush() { queue.sync {}; EditorWorker.flush(); RecordingEditorWorker.flush() }
}
