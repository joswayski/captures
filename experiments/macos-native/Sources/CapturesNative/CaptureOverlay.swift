import AppKit
import Combine
import SwiftUI

fileprivate struct OverlayDisplay: Identifiable, Hashable {
    let id: String
    let name: String
    let x: CGFloat
    let y: CGFloat
    let width: CGFloat
    let height: CGFloat
    let scaleFactor: CGFloat
    let isPrimary: Bool

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String else { return nil }
        self.id = id
        name = value["name"] as? String ?? "Display"
        x = Self.number(value["x"])
        y = Self.number(value["y"])
        width = Self.number(value["width"])
        height = Self.number(value["height"])
        scaleFactor = max(1, Self.number(value["scale_factor"], fallback: 1))
        isPrimary = value["is_primary"] as? Bool ?? false
    }

    private static func number(_ value: Any?, fallback: CGFloat = 0) -> CGFloat {
        if let number = value as? NSNumber { return CGFloat(number.doubleValue) }
        return fallback
    }
}

private struct OverlayWindow: Identifiable, Equatable {
    let id: String
    let title: String
    let appName: String
    let x: CGFloat
    let y: CGFloat
    let width: CGFloat
    let height: CGFloat
    let displayID: String
    let zOrder: Int

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String else { return nil }
        self.id = id
        title = value["title"] as? String ?? ""
        appName = value["app_name"] as? String ?? ""
        x = Self.number(value["x"])
        y = Self.number(value["y"])
        width = Self.number(value["width"])
        height = Self.number(value["height"])
        displayID = value["display_id"] as? String ?? ""
        zOrder = (value["z_order"] as? NSNumber)?.intValue ?? Int.max
    }

    var label: String { title.isEmpty ? (appName.isEmpty ? "Window" : appName) : title }

    private static func number(_ value: Any?) -> CGFloat {
        CGFloat((value as? NSNumber)?.doubleValue ?? 0)
    }
}

private struct FrozenFrame {
    let id: String
    let image: NSImage

    init?(_ value: [String: Any]) {
        guard
            let id = value["id"] as? String,
            let path = value["path"] as? String,
            let image = NSImage(contentsOfFile: path)
        else { return nil }
        self.id = id
        self.image = image
    }
}

private enum OverlayKind: String, CaseIterable, Identifiable {
    case image
    case video
    case gif

    var id: String { rawValue }
    var label: String {
        switch self {
        case .image: return "Screenshot"
        case .video: return "Video"
        case .gif: return "GIF"
        }
    }
}

private enum OverlayTarget: String, CaseIterable, Identifiable {
    case region
    case window
    case display

    var id: String { rawValue }
    var label: String { rawValue.capitalized }
    var symbol: String {
        switch self {
        case .region: return "viewfinder"
        case .window: return "macwindow"
        case .display: return "display"
        }
    }
}

private enum RegionDrag {
    case create(origin: CGPoint)
    case move(origin: CGPoint, initial: CGRect)
    case resize(corner: UnitPoint, initial: CGRect)
}

private final class CaptureOverlayPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func cancelOperation(_ sender: Any?) {
        CaptureController.shared.cancel()
    }
}

final class CaptureController {
    static let shared = CaptureController()

    private var panel: CaptureOverlayPanel?
    private var model: CaptureOverlayModel?
    private var generation = UUID()
    private var freezeGeneration = UUID()

    private init() {
        let center = NSWorkspace.shared.notificationCenter
        center.addObserver(forName: NSWorkspace.sessionDidResignActiveNotification, object: nil, queue: .main) { _ in
            CaptureController.shared.cancel()
        }
        DistributedNotificationCenter.default().addObserver(
            forName: Notification.Name("com.apple.screenIsLocked"),
            object: nil,
            queue: .main
        ) { _ in CaptureController.shared.cancel() }
    }

    func show(kind: String = "image", target: String = "region") {
        DispatchQueue.main.async {
            self.cancelNow(invalidate: false)
            let generation = UUID()
            self.generation = generation
            Backend.shared.call("describe", ["request_permission": true]) { result in
                guard self.generation == generation else { return }
                switch result {
                case .failure(let error):
                    AppStore.shared.report(error)
                case .success(let payload):
                    self.preparePresentation(payload: payload, kind: kind, target: target, generation: generation)
                }
            }
        }
    }

    func cancel() {
        DispatchQueue.main.async {
            self.cancelNow(invalidate: true)
        }
    }

    fileprivate func finish(discardFreeze: Bool = true) {
        freezeGeneration = UUID()
        model?.stopSafetyMonitoring()
        if discardFreeze { model?.discardFreeze() }
        panel?.orderOut(nil)
        panel?.contentView = nil
        panel = nil
        model = nil
        NSCursor.arrow.set()
    }

    private func cancelNow(invalidate: Bool) {
        if invalidate { generation = UUID() }
        freezeGeneration = UUID()
        model?.cancelCountdown()
        model?.stopSafetyMonitoring()
        model?.discardFreeze()
        panel?.orderOut(nil)
        panel?.contentView = nil
        panel = nil
        model = nil
        NSCursor.arrow.set()
    }

    private func preparePresentation(payload: [String: Any], kind: String, target: String, generation: UUID) {
        let displayValues = payload["displays"] as? [[String: Any]] ?? []
        let windowValues = payload["windows"] as? [[String: Any]] ?? []
        let displays = displayValues.compactMap(OverlayDisplay.init)
        guard !displays.isEmpty else {
            AppStore.shared.report(CaptureOverlayError.noDisplays)
            return
        }

        let screen = NSScreen.screens.first(where: { $0.frame.contains(NSEvent.mouseLocation) })
            ?? NSScreen.main
            ?? NSScreen.screens.first
        guard let screen else {
            AppStore.shared.report(CaptureOverlayError.noDisplays)
            return
        }
        let display = matchDisplay(displays, to: screen)
        let captureKind = OverlayKind(rawValue: kind) ?? .image
        let captureTarget = OverlayTarget(rawValue: target) ?? .region
        let present: (FrozenFrame?) -> Void = { frozenFrame in
            guard self.generation == generation else {
                if let frozenFrame { self.discardFreeze(id: frozenFrame.id) }
                return
            }
            self.presentPanel(
                display: display,
                displays: displays,
                windows: windowValues.compactMap(OverlayWindow.init),
                kind: captureKind,
                target: captureTarget,
                screen: screen,
                frozenFrame: frozenFrame
            )
        }
        guard AppStore.shared.settings.freezeScreen else {
            present(nil)
            return
        }
        Backend.shared.call("freeze_create", [
            "display_id": display.id,
            "cursor": captureKind == .image && AppStore.shared.settings.showCursor,
        ]) { result in
            guard self.generation == generation else {
                if case .success(let value) = result, let id = value["id"] as? String { self.discardFreeze(id: id) }
                return
            }
            switch result {
            case .failure(let error):
                AppStore.shared.report(error)
            case .success(let value):
                guard let frame = FrozenFrame(value) else {
                    if let id = value["id"] as? String { self.discardFreeze(id: id) }
                    AppStore.shared.report(CaptureOverlayError.freezePreviewUnavailable)
                    return
                }
                present(frame)
            }
        }
    }

    private func presentPanel(
        display: OverlayDisplay,
        displays: [OverlayDisplay],
        windows: [OverlayWindow],
        kind: OverlayKind,
        target: OverlayTarget,
        screen: NSScreen,
        frozenFrame: FrozenFrame?
    ) {
        let model = CaptureOverlayModel(
            display: display,
            displays: displays,
            windows: windows,
            kind: kind,
            target: target,
            screenSize: screen.frame.size,
            frozenFrame: frozenFrame
        )
        let panel = CaptureOverlayPanel(
            contentRect: screen.frame,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false,
            screen: screen
        )
        panel.level = .screenSaver
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.sharingType = .none
        panel.isMovable = false
        panel.acceptsMouseMovedEvents = true
        panel.contentView = NSHostingView(rootView: CaptureOverlayView(model: model))
        panel.setFrame(screen.frame, display: true)
        self.model = model
        self.panel = panel
        panel.orderFrontRegardless()
        panel.makeKey()
        model.startSafetyMonitoring()
    }

    private func matchDisplay(_ displays: [OverlayDisplay], to screen: NSScreen) -> OverlayDisplay {
        if let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber {
            let raw = String(number.uint32Value)
            if let exact = displays.first(where: { $0.id == raw }) { return exact }
        }
        if screen == NSScreen.main, let primary = displays.first(where: \.isPrimary) { return primary }
        return displays.first(where: { abs($0.width - screen.frame.width) < 2 && abs($0.height - screen.frame.height) < 2 })
            ?? displays.first!
    }

    fileprivate func selectDisplay(_ display: OverlayDisplay) {
        guard let panel, let model else { return }
        let screen = screen(for: display) ?? panel.screen
        guard let screen else { return }
        model.screenSize = screen.frame.size
        model.selection = nil
        model.hoveredWindowID = nil
        model.selectedWindowID = nil
        panel.setFrame(screen.frame, display: true)
        refreshFreeze(for: model)
    }

    fileprivate func targetChanged(for model: CaptureOverlayModel) {
        guard self.model === model else { return }
        model.selection = nil
        model.hoveredWindowID = nil
        model.selectedWindowID = nil
    }

    fileprivate func kindChanged(for model: CaptureOverlayModel) {
        guard self.model === model, AppStore.shared.settings.freezeScreen else { return }
        refreshFreeze(for: model)
    }

    private func refreshFreeze(for model: CaptureOverlayModel) {
        let expectedFreezeGeneration = UUID()
        freezeGeneration = expectedFreezeGeneration
        model.discardFreeze()
        guard AppStore.shared.settings.freezeScreen else {
            panel?.orderFrontRegardless()
            panel?.makeKey()
            return
        }
        let expectedGeneration = generation
        panel?.orderOut(nil)
        Backend.shared.call("freeze_create", [
            "display_id": model.display.id,
            "cursor": model.kind == .image && AppStore.shared.settings.showCursor,
        ]) { result in
            guard
                self.generation == expectedGeneration,
                self.freezeGeneration == expectedFreezeGeneration,
                self.model === model
            else {
                if case .success(let value) = result, let id = value["id"] as? String { self.discardFreeze(id: id) }
                return
            }
            switch result {
            case .failure(let error):
                AppStore.shared.report(error)
                self.cancel()
            case .success(let value):
                guard let frame = FrozenFrame(value) else {
                    if let id = value["id"] as? String { self.discardFreeze(id: id) }
                    AppStore.shared.report(CaptureOverlayError.freezePreviewUnavailable)
                    self.cancel()
                    return
                }
                model.frozenFrame = frame
                self.panel?.orderFrontRegardless()
                self.panel?.makeKey()
            }
        }
    }

    fileprivate func discardFreeze(id: String) {
        Backend.shared.call("freeze_discard", ["id": id]) { result in
            if case .failure(let error) = result { AppStore.shared.report(error) }
        }
    }

    private func screen(for display: OverlayDisplay) -> NSScreen? {
        if let numericID = UInt32(display.id), let exact = NSScreen.screens.first(where: {
            ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == numericID
        }) { return exact }
        if display.isPrimary { return NSScreen.main }
        return NSScreen.screens.first(where: {
            abs($0.frame.width - display.width) < 2 && abs($0.frame.height - display.height) < 2
        })
    }
}

private enum CaptureOverlayError: LocalizedError {
    case noDisplays
    case emptySelection
    case malformedArtifact
    case freezePreviewUnavailable

    var errorDescription: String? {
        switch self {
        case .noDisplays: return "No capturable display is available."
        case .emptySelection: return "Draw a region before capturing."
        case .malformedArtifact: return "The capture backend returned an invalid artifact."
        case .freezePreviewUnavailable: return "The frozen screen preview could not be loaded safely."
        }
    }
}

private final class CaptureOverlayModel: ObservableObject {
    @Published var display: OverlayDisplay
    @Published var kind: OverlayKind
    @Published var target: OverlayTarget
    @Published var selection: CGRect?
    @Published var hoveredWindowID: String?
    @Published var selectedWindowID: String?
    @Published var countdown: Int?
    @Published var busy = false
    @Published var error = ""
    @Published var frozenFrame: FrozenFrame?

    let displays: [OverlayDisplay]
    let windows: [OverlayWindow]
    @Published var screenSize: CGSize
    private var drag: RegionDrag?
    private var countdownWork: DispatchWorkItem?
    private var safetyTimer: Timer?
    private var safetyCheckInFlight = false
    private var safetyGeneration = UUID()

    init(
        display: OverlayDisplay,
        displays: [OverlayDisplay],
        windows: [OverlayWindow],
        kind: OverlayKind,
        target: OverlayTarget,
        screenSize: CGSize,
        frozenFrame: FrozenFrame?
    ) {
        self.display = display
        self.displays = displays
        self.windows = windows.sorted { $0.zOrder > $1.zOrder }
        self.kind = kind
        self.target = target
        self.screenSize = screenSize
        self.frozenFrame = frozenFrame
    }

    var localWindows: [OverlayWindow] {
        windows.filter { $0.displayID.isEmpty || $0.displayID == display.id }
    }

    var canCapture: Bool {
        if busy { return false }
        switch target {
        case .region: return selection.map { $0.width >= 2 && $0.height >= 2 } ?? false
        case .window: return selectedWindowID != nil
        case .display: return true
        }
    }

    func localRect(for window: OverlayWindow) -> CGRect {
        CGRect(
            x: (window.x - display.x) * screenSize.width / max(1, display.width),
            y: (window.y - display.y) * screenSize.height / max(1, display.height),
            width: window.width * screenSize.width / max(1, display.width),
            height: window.height * screenSize.height / max(1, display.height)
        )
    }

    func window(at point: CGPoint) -> OverlayWindow? {
        localWindows.first(where: { localRect(for: $0).contains(point) })
    }

    func updateWindowHover(_ point: CGPoint) {
        hoveredWindowID = window(at: point)?.id
    }

    func chooseWindow(_ point: CGPoint) {
        selectedWindowID = window(at: point)?.id
        if selectedWindowID != nil, AppStore.shared.settings.autoStart { capture() }
    }

    func beginCreate(at point: CGPoint) {
        guard target == .region, countdown == nil, drag == nil else { return }
        drag = .create(origin: point)
        selection = CGRect(origin: point, size: .zero)
    }

    func beginMove(at point: CGPoint) {
        guard drag == nil, let selection else { return }
        drag = .move(origin: point, initial: selection)
    }

    func beginResize(corner: UnitPoint) {
        guard drag == nil, let selection else { return }
        drag = .resize(corner: corner, initial: selection)
    }

    func updateDrag(to point: CGPoint, shift: Bool) {
        guard let drag else { return }
        switch drag {
        case .create(let origin):
            selection = boundedRect(from: origin, to: point, square: shift)
        case .move(let origin, let initial):
            var next = initial.offsetBy(dx: point.x - origin.x, dy: point.y - origin.y)
            next.origin.x = min(max(0, next.minX), max(0, screenSize.width - next.width))
            next.origin.y = min(max(0, next.minY), max(0, screenSize.height - next.height))
            selection = next
        case .resize(let corner, let initial):
            let opposite = CGPoint(
                x: corner.x == 0 ? initial.maxX : initial.minX,
                y: corner.y == 0 ? initial.maxY : initial.minY
            )
            var next = boundedRect(from: opposite, to: point, square: shift)
            if next.width < 16 || next.height < 16 {
                next.size.width = max(16, next.width)
                next.size.height = max(16, next.height)
            }
            selection = next.intersection(CGRect(origin: .zero, size: screenSize))
        }
    }

    func endDrag() {
        let created: Bool
        if case .some(.create) = drag { created = true } else { created = false }
        drag = nil
        if let selection, selection.width < 2 || selection.height < 2 { self.selection = nil }
        if created, self.selection != nil, AppStore.shared.settings.autoStart { capture() }
    }

    func startSafetyMonitoring() {
        stopSafetyMonitoring()
        let generation = UUID()
        safetyGeneration = generation
        safetyTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            guard let self, !self.safetyCheckInFlight else { return }
            self.safetyCheckInFlight = true
            Backend.shared.call("session_status") { result in
                guard self.safetyGeneration == generation else { return }
                self.safetyCheckInFlight = false
                if (try? result.get()["available"] as? Bool) != true { CaptureController.shared.cancel() }
            }
        }
    }

    func stopSafetyMonitoring() {
        safetyGeneration = UUID()
        safetyTimer?.invalidate()
        safetyTimer = nil
        safetyCheckInFlight = false
    }

    func discardFreeze() {
        guard let id = frozenFrame?.id else { return }
        frozenFrame = nil
        CaptureController.shared.discardFreeze(id: id)
    }

    func capture() {
        guard canCapture else {
            error = CaptureOverlayError.emptySelection.localizedDescription
            return
        }
        error = ""
        let seconds = kind == .image
            ? AppStore.shared.settings.screenshotCountdown
            : AppStore.shared.settings.recordingCountdown
        if seconds > 0 {
            startCountdown(seconds)
        } else {
            commit()
        }
    }

    func cancelCountdown() {
        countdownWork?.cancel()
        countdownWork = nil
        countdown = nil
        busy = false
    }

    private func startCountdown(_ seconds: Int) {
        cancelCountdown()
        busy = true
        countdown = seconds
        scheduleCountdownTick()
    }

    private func scheduleCountdownTick() {
        let work = DispatchWorkItem { [weak self] in
            guard let self, let value = self.countdown else { return }
            if value <= 1 {
                self.countdown = nil
                self.countdownWork = nil
                self.commit()
            } else {
                self.countdown = value - 1
                self.scheduleCountdownTick()
            }
        }
        countdownWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 1, execute: work)
    }

    private func commit() {
        busy = true
        let targetPayload = backendTarget(useFreeze: kind == .image)
        let settings = AppStore.shared.settings
        if kind == .image {
            let freezeID = frozenFrame?.id
            CaptureController.shared.finish(discardFreeze: freezeID == nil)
            Backend.shared.call("screenshot", [
                "target": targetPayload,
                "cursor": freezeID == nil && settings.showCursor,
                "output_dir": settings.outputDirectory,
            ]) { result in
                self.finishArtifact(result)
                if let freezeID { CaptureController.shared.discardFreeze(id: freezeID) }
            }
            return
        }

        CaptureController.shared.finish()

        let isGIF = kind == .gif
        let microphoneID: Any
        if isGIF || settings.microphoneID.isEmpty { microphoneID = NSNull() }
        else { microphoneID = settings.microphoneID }
        let options: [String: Any] = [
            "kind": isGIF ? "gif" : "video",
            "target": targetPayload,
            "frames_per_second": isGIF ? settings.gifFPS : settings.videoFPS,
            "max_resolution": settings.maxResolution,
            // The native overlay owns the cancellable countdown.
            "countdown_seconds": 0,
            "show_cursor": settings.showCursor,
            "highlight_clicks": false,
            "show_keystrokes": false,
            "audio": [
                "capture_system_audio": isGIF ? false : settings.systemAudio,
                "microphone_device_id": microphoneID,
                "mono_output": false,
                "system_volume_percent": 100,
                "microphone_volume_percent": 100,
                "microphone_muted": settings.microphoneMuted,
            ],
            "gif": [
                "max_width": settings.gifWidth,
                "max_colors": settings.gifColors,
                "optimize": true,
            ],
        ]
        Backend.shared.call("record_start", [
            "options": options,
            "exclude_app": settings.excludeControls,
            "output_dir": settings.outputDirectory,
        ]) { result in
            switch result {
            case .success:
                RecordingHUDController.shared.show()
            case .failure(let error):
                AppStore.shared.report(error)
            }
        }
    }

    private func finishArtifact(_ result: Result<[String: Any], Error>) {
        switch result {
        case .failure(let error):
            AppStore.shared.report(error)
        case .success(let response):
            do {
                let artifact = try Artifact(response: response)
                AppStore.shared.addArtifact(artifact)
                PreviewController.shared.refresh()
            } catch {
                AppStore.shared.report(error)
            }
        }
    }

    private func backendTarget(useFreeze: Bool) -> [String: Any] {
        switch target {
        case .display:
            if useFreeze, let frozenFrame {
                return [
                    "type": "frozen_region",
                    "freeze_id": frozenFrame.id,
                    "rect": ["x": 0, "y": 0, "width": Int(display.width), "height": Int(display.height)],
                ]
            }
            return ["type": "display", "display_id": display.id]
        case .window:
            if useFreeze,
               let frozenFrame,
               let id = selectedWindowID,
               let window = localWindows.first(where: { $0.id == id }) {
                let visible = CGRect(
                    x: window.x - display.x,
                    y: window.y - display.y,
                    width: window.width,
                    height: window.height
                ).intersection(CGRect(x: 0, y: 0, width: display.width, height: display.height))
                return [
                    "type": "frozen_region",
                    "freeze_id": frozenFrame.id,
                    "rect": [
                        "x": Int(visible.minX.rounded()),
                        "y": Int(visible.minY.rounded()),
                        "width": Int(visible.width.rounded()),
                        "height": Int(visible.height.rounded()),
                    ],
                ]
            }
            return ["type": "window", "window_id": selectedWindowID ?? ""]
        case .region:
            let rect = selection ?? .zero
            let sx = display.width / max(1, screenSize.width)
            let sy = display.height / max(1, screenSize.height)
            let freeze = useFreeze ? frozenFrame : nil
            var payload: [String: Any] = [
                "type": freeze == nil ? "region" : "frozen_region",
                "rect": [
                    "x": Int((rect.minX * sx).rounded()),
                    "y": Int((rect.minY * sy).rounded()),
                    "width": Int((rect.width * sx).rounded()),
                    "height": Int((rect.height * sy).rounded()),
                ],
            ]
            if let freeze { payload["freeze_id"] = freeze.id }
            else { payload["display_id"] = display.id }
            return payload
        }
    }

    private func boundedRect(from start: CGPoint, to end: CGPoint, square: Bool) -> CGRect {
        var dx = end.x - start.x
        var dy = end.y - start.y
        if square {
            let side = min(max(abs(dx), abs(dy)), min(
                dx >= 0 ? screenSize.width - start.x : start.x,
                dy >= 0 ? screenSize.height - start.y : start.y
            ))
            dx = side * (dx < 0 ? -1 : 1)
            dy = side * (dy < 0 ? -1 : 1)
        }
        let rect = CGRect(
            x: min(start.x, start.x + dx),
            y: min(start.y, start.y + dy),
            width: abs(dx),
            height: abs(dy)
        )
        return rect.intersection(CGRect(origin: .zero, size: screenSize))
    }
}

private struct CaptureOverlayView: View {
    @ObservedObject var model: CaptureOverlayModel

    var body: some View {
        ZStack {
            if let image = model.frozenFrame?.image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: model.screenSize.width, height: model.screenSize.height)
                    .clipped()
                    .allowsHitTesting(false)
            }
            captureSurface
            VStack {
                Spacer()
                CaptureToolbar(model: model)
                    .padding(.bottom, 26)
            }
            if let countdown = model.countdown {
                CountdownView(value: countdown, cancel: model.cancelCountdown)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.clear)
        .preferredColorScheme(.dark)
        .onExitCommand {
            if model.countdown != nil { model.cancelCountdown() }
            else { CaptureController.shared.cancel() }
        }
    }

    @ViewBuilder private var captureSurface: some View {
        GeometryReader { proxy in
            ZStack {
                if model.target == .region {
                    OverlayShade(hole: model.selection, cornerRadius: 0, opacity: 0.20)
                    regionSurface(proxy.size)
                } else if model.target == .window {
                    OverlayShade(hole: activeWindowRect, cornerRadius: 8, opacity: 0.42)
                    windowSurface(proxy.size)
                } else {
                    Color.black.opacity(0.20)
                    RoundedRectangle(cornerRadius: 1)
                        .stroke(NativeTheme.accent, lineWidth: 2)
                    guidance("Entire display", "Click Capture when ready")
                }
            }
            .contentShape(Rectangle())
            .onTapGesture {
                if model.target == .display, AppStore.shared.settings.autoStart { model.capture() }
            }
            .coordinateSpace(name: "capture-overlay")
            .onContinuousHover { phase in
                if model.target == .window, case .active(let point) = phase {
                    model.updateWindowHover(point)
                }
            }
        }
    }

    private var activeWindowRect: CGRect? {
        let id = model.hoveredWindowID ?? model.selectedWindowID
        guard let id, let window = model.localWindows.first(where: { $0.id == id }) else { return nil }
        return model.localRect(for: window)
    }

    private func regionSurface(_ size: CGSize) -> some View {
        ZStack {
            if let rect = model.selection {
                Rectangle()
                    .fill(.clear)
                    .overlay(Rectangle().stroke(NativeTheme.accent, lineWidth: 1.5))
                    .shadow(color: .black.opacity(0.45), radius: 0, x: 0, y: 1)
                    .frame(width: rect.width, height: rect.height)
                    .position(x: rect.midX, y: rect.midY)
                    .gesture(moveGesture)
                dimensionLabel(rect)
                ForEach(Array(regionCorners.enumerated()), id: \.offset) { _, corner in
                    resizeHandle(corner, rect: rect)
                }
            } else {
                guidance("Drag to capture a region", "Hold Shift for a square")
            }
        }
        .frame(width: size.width, height: size.height)
        .contentShape(Rectangle())
        .gesture(createGesture)
        .cursor(.crosshair)
    }

    private func windowSurface(_ size: CGSize) -> some View {
        ZStack {
            ForEach(model.localWindows.reversed()) { window in
                let rect = model.localRect(for: window)
                let active = model.hoveredWindowID == window.id || model.selectedWindowID == window.id
                RoundedRectangle(cornerRadius: 8)
                    .fill(active ? NativeTheme.accent.opacity(0.14) : .clear)
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(active ? NativeTheme.accent : .clear, lineWidth: 2))
                    .frame(width: rect.width, height: rect.height)
                    .position(x: rect.midX, y: rect.midY)
                    .allowsHitTesting(false)
                if active {
                    Text(window.label)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(NativeTheme.glassText)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 5)
                        .background(NativeTheme.glassRaised, in: RoundedRectangle(cornerRadius: 6))
                        .position(x: min(size.width - 90, max(90, rect.minX + 72)), y: max(18, rect.minY + 18))
                        .allowsHitTesting(false)
                }
            }
            if model.hoveredWindowID == nil && model.selectedWindowID == nil {
                guidance("Choose a window", "Move the pointer over a window")
            }
        }
        .frame(width: size.width, height: size.height)
        .contentShape(Rectangle())
        .gesture(
            SpatialTapGesture(coordinateSpace: .named("capture-overlay"))
                .onEnded { value in model.chooseWindow(value.location) }
        )
    }

    private var createGesture: some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .named("capture-overlay"))
            .onChanged { value in
                if value.translation == .zero { model.beginCreate(at: value.startLocation) }
                model.updateDrag(to: value.location, shift: NSEvent.modifierFlags.contains(.shift))
            }
            .onEnded { value in
                model.updateDrag(to: value.location, shift: NSEvent.modifierFlags.contains(.shift))
                model.endDrag()
            }
    }

    private var moveGesture: some Gesture {
        DragGesture(minimumDistance: 1, coordinateSpace: .named("capture-overlay"))
            .onChanged { value in
                model.beginMove(at: value.startLocation)
                model.updateDrag(to: value.location, shift: false)
            }
            .onEnded { _ in model.endDrag() }
    }

    private let regionCorners: [UnitPoint] = [.topLeading, .topTrailing, .bottomLeading, .bottomTrailing]

    private func resizeHandle(_ corner: UnitPoint, rect: CGRect) -> some View {
        let point = CGPoint(
            x: corner.x == 0 ? rect.minX : rect.maxX,
            y: corner.y == 0 ? rect.minY : rect.maxY
        )
        return Circle()
            .fill(NativeTheme.accent)
            .overlay(Circle().stroke(Color.white.opacity(0.85), lineWidth: 1))
            .frame(width: 10, height: 10)
            .position(point)
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .named("capture-overlay"))
                    .onChanged { value in
                        model.beginResize(corner: corner)
                        model.updateDrag(to: value.location, shift: NSEvent.modifierFlags.contains(.shift))
                    }
                    .onEnded { _ in model.endDrag() }
            )
    }

    private func dimensionLabel(_ rect: CGRect) -> some View {
        Text("\(Int(rect.width.rounded())) × \(Int(rect.height.rounded()))")
            .font(.system(size: 11, weight: .semibold, design: .rounded))
            .monospacedDigit()
            .foregroundColor(.black.opacity(0.86))
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(NativeTheme.accent, in: RoundedRectangle(cornerRadius: 6))
            .position(x: rect.minX + 48, y: rect.minY < 34 ? rect.minY + 18 : rect.minY - 17)
            .allowsHitTesting(false)
    }

    private func guidance(_ title: String, _ detail: String) -> some View {
        VStack(spacing: 2) {
            Text(title).font(.system(size: 14, weight: .semibold))
            Text(detail).font(.system(size: 11, weight: .medium)).foregroundColor(NativeTheme.glassMuted)
        }
        .foregroundColor(NativeTheme.glassText)
        .padding(.horizontal, 22)
        .padding(.vertical, 14)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color.white.opacity(0.16)))
        .position(x: model.screenSize.width / 2, y: model.screenSize.height * 0.16)
        .allowsHitTesting(false)
    }
}

private struct OverlayShade: View {
    let hole: CGRect?
    let cornerRadius: CGFloat
    let opacity: Double

    var body: some View {
        GeometryReader { proxy in
            Path { path in
                path.addRect(CGRect(origin: .zero, size: proxy.size))
                if let hole {
                    path.addRoundedRect(
                        in: hole,
                        cornerSize: CGSize(width: cornerRadius, height: cornerRadius)
                    )
                }
            }
            .fill(Color.black.opacity(opacity), style: FillStyle(eoFill: true))
        }
        .allowsHitTesting(false)
    }
}

private struct CaptureToolbar: View {
    @ObservedObject var model: CaptureOverlayModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Button(action: CaptureController.shared.cancel) {
                    Image(systemName: "xmark").frame(width: 18, height: 18)
                }
                .buttonStyle(CaptureButtonStyle(glass: true))
                .help("Cancel · Esc")

                Picker("Capture type", selection: $model.kind) {
                    ForEach(OverlayKind.allCases) { kind in Text(kind.label).tag(kind) }
                }
                .pickerStyle(.segmented)
                .frame(width: 220)
                .onChange(of: model.kind) { _ in CaptureController.shared.kindChanged(for: model) }

                Divider().frame(height: 24)

                Picker("Target", selection: $model.target) {
                    ForEach(OverlayTarget.allCases) { target in
                        Label(target.label, systemImage: target.symbol).tag(target)
                    }
                }
                .pickerStyle(.segmented)
                .frame(width: 260)
                .onChange(of: model.target) { _ in CaptureController.shared.targetChanged(for: model) }

                if model.displays.count > 1 {
                    Picker("Display", selection: $model.display) {
                        ForEach(model.displays) { display in Text(display.name).tag(display) }
                    }
                    .frame(width: 180)
                    .onChange(of: model.display) { display in
                        CaptureController.shared.selectDisplay(display)
                    }
                }

                Button(action: model.capture) {
                    Label(model.kind == .image ? "Capture" : "Record", systemImage: model.kind == .image ? "camera" : "record.circle")
                }
                .buttonStyle(CaptureButtonStyle(primary: true, glass: true))
                .disabled(!model.canCapture)
            }
            .padding(10)

            if model.kind != .image {
                recordingOptions
                    .padding(.horizontal, 12)
                    .padding(.bottom, 10)
            }
            if !model.error.isEmpty {
                Text(model.error)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(NativeTheme.signal)
                    .padding(.bottom, 8)
            }
        }
        .foregroundColor(NativeTheme.glassText)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 18))
        .background(NativeTheme.glass.opacity(0.72), in: RoundedRectangle(cornerRadius: 18))
        .overlay(RoundedRectangle(cornerRadius: 18).stroke(Color.white.opacity(0.16)))
        .shadow(color: .black.opacity(0.34), radius: 18, y: 8)
    }

    private var recordingOptions: some View {
        HStack(spacing: 14) {
            Label("\(model.kind == .gif ? AppStore.shared.settings.gifFPS : AppStore.shared.settings.videoFPS) FPS", systemImage: "speedometer")
            Label(AppStore.shared.settings.maxResolution == "original" ? "Original" : AppStore.shared.settings.maxResolution.dropFirst().uppercased(), systemImage: "rectangle.expand.vertical")
            Label(AppStore.shared.settings.showCursor ? "Cursor on" : "Cursor off", systemImage: "cursorarrow")
            if model.kind == .video {
                Label(AppStore.shared.settings.systemAudio ? "System audio" : "No system audio", systemImage: "speaker.wave.2")
                Label(AppStore.shared.settings.microphoneID.isEmpty ? "Mic off" : "Microphone", systemImage: AppStore.shared.settings.microphoneMuted ? "mic.slash" : "mic")
            }
            Button("Preferences") { AppStore.shared.showPreferences() }
                .buttonStyle(.plain)
                .foregroundColor(NativeTheme.accent)
        }
        .font(.system(size: 11, weight: .medium))
        .foregroundColor(NativeTheme.glassMuted)
    }
}

private struct CountdownView: View {
    let value: Int
    let cancel: () -> Void

    var body: some View {
        ZStack {
            Color.black.opacity(0.22).ignoresSafeArea()
            VStack(spacing: 14) {
                Text("\(value)")
                    .font(.system(size: 72, weight: .bold, design: .rounded))
                Button("Cancel", action: cancel)
                    .buttonStyle(CaptureButtonStyle(glass: true))
            }
            .foregroundColor(NativeTheme.glassText)
            .padding(28)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 22))
        }
    }
}

final class RecordingHUDController {
    static let shared = RecordingHUDController()

    private var panel: NSPanel?
    private var model: RecordingHUDModel?

    private init() {
        let center = NSWorkspace.shared.notificationCenter
        center.addObserver(forName: NSWorkspace.sessionDidResignActiveNotification, object: nil, queue: .main) { _ in
            RecordingHUDController.shared.hide()
        }
        DistributedNotificationCenter.default().addObserver(
            forName: Notification.Name("com.apple.screenIsLocked"),
            object: nil,
            queue: .main
        ) { _ in RecordingHUDController.shared.hide() }
    }

    func show() {
        DispatchQueue.main.async {
            let model = self.model ?? RecordingHUDModel()
            self.model = model
            let panel = self.panel ?? self.makePanel(model: model)
            self.panel = panel
            self.place(panel)
            panel.orderFrontRegardless()
            model.startPolling()
        }
    }

    func hide() {
        DispatchQueue.main.async { self.panel?.orderOut(nil) }
    }

    fileprivate func close() {
        model?.stopPolling()
        panel?.orderOut(nil)
        panel?.contentView = nil
        panel = nil
        model = nil
    }

    private func makePanel(model: RecordingHUDModel) -> NSPanel {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 430, height: 102),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.sharingType = AppStore.shared.settings.excludeControls ? .none : .readOnly
        panel.isMovableByWindowBackground = true
        panel.contentView = NSHostingView(rootView: RecordingHUDView(model: model))
        return panel
    }

    private func place(_ panel: NSPanel) {
        guard let visible = (NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main)?.visibleFrame else { return }
        panel.setFrameOrigin(NSPoint(x: visible.midX - panel.frame.width / 2, y: visible.minY + 20))
    }
}

private final class RecordingHUDModel: ObservableObject {
    @Published var state = "recording"
    @Published var elapsedMilliseconds = 0
    @Published var microphoneLevel = 0.0
    @Published var microphoneMuted = false
    @Published var warning = ""
    @Published var busy = false
    private var timer: Timer?
    private var statusInFlight = false

    var canControl: Bool { !busy && (state == "recording" || state == "paused") }
    var isPaused: Bool { state == "paused" }
    var elapsed: String {
        let total = elapsedMilliseconds / 1_000
        let hours = total / 3_600
        let minutes = (total % 3_600) / 60
        let seconds = total % 60
        return hours > 0
            ? String(format: "%d:%02d:%02d", hours, minutes, seconds)
            : String(format: "%d:%02d", minutes, seconds)
    }

    var statusLabel: String {
        switch state {
        case "paused": return "Paused"
        case "finalizing": return "Saving…"
        case "failed": return "Failed"
        default: return "Recording"
        }
    }

    func startPolling() {
        stopPolling()
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in self?.refresh() }
    }

    func stopPolling() {
        timer?.invalidate()
        timer = nil
    }

    func togglePause() { action(isPaused ? "record_resume" : "record_pause") }
    func toggleMute() {
        let muted = !microphoneMuted
        action("record_mute", ["muted": muted]) { self.microphoneMuted = muted }
    }

    func takeScreenshot() {
        guard canControl else { return }
        CaptureController.shared.show(kind: "image", target: "region")
    }

    func stop() {
        guard canControl else { return }
        busy = true
        state = "finalizing"
        Backend.shared.call("record_stop") { result in
            self.busy = false
            switch result {
            case .failure(let error): self.fail(error)
            case .success(let response):
                do {
                    let artifact = try Artifact(response: response)
                    AppStore.shared.addArtifact(artifact)
                    PreviewController.shared.refresh()
                    if AppStore.shared.settings.openEditorAfterRecording { AppStore.shared.open(artifact) }
                    RecordingHUDController.shared.close()
                } catch { self.fail(error) }
            }
        }
    }

    func restart() {
        confirm(
            title: "Restart recording?",
            message: "The current recording will be discarded and a new countdown will begin.",
            action: "Restart"
        ) { self.action("record_restart") }
    }

    func discard() {
        confirm(
            title: "Delete recording?",
            message: "This recording will be deleted permanently.",
            action: "Delete",
            destructive: true
        ) {
            self.busy = true
            Backend.shared.call("record_discard") { result in
                self.busy = false
                switch result {
                case .success: RecordingHUDController.shared.close()
                case .failure(let error): self.fail(error)
                }
            }
        }
    }

    private func refresh() {
        guard !statusInFlight else { return }
        statusInFlight = true
        Backend.shared.call("record_status") { result in
            self.statusInFlight = false
            switch result {
            case .failure(let error): self.fail(error)
            case .success(let value):
                self.state = value["state"] as? String ?? self.state
                self.elapsedMilliseconds = (value["elapsed_ms"] as? NSNumber)?.intValue ?? self.elapsedMilliseconds
                self.microphoneLevel = min(1, max(0, (value["microphone_level"] as? NSNumber)?.doubleValue ?? 0))
                self.microphoneMuted = value["microphone_muted"] as? Bool ?? self.microphoneMuted
                self.warning = value["warning"] as? String ?? ""
                if self.state == "idle" || self.state == "ready" { RecordingHUDController.shared.close() }
            }
        }
    }

    private func action(_ operation: String, _ fields: [String: Any] = [:], success: (() -> Void)? = nil) {
        guard !busy else { return }
        busy = true
        Backend.shared.call(operation, fields) { result in
            self.busy = false
            switch result {
            case .success:
                success?()
                self.refresh()
            case .failure(let error): self.fail(error)
            }
        }
    }

    private func fail(_ error: Error) {
        warning = error.localizedDescription
        AppStore.shared.report(error)
    }

    private func confirm(title: String, message: String, action: String, destructive: Bool = false, perform: @escaping () -> Void) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: action)
        alert.addButton(withTitle: "Cancel")
        if destructive { alert.buttons.first?.hasDestructiveAction = true }
        if alert.runModal() == .alertFirstButtonReturn { perform() }
    }
}

private struct RecordingHUDView: View {
    @ObservedObject var model: RecordingHUDModel

    var body: some View {
        VStack(spacing: 2) {
            Text(AppStore.shared.settings.excludeControls ? "Controls are hidden from the recording" : "These controls may appear in the recording")
                .font(.system(size: 9, weight: .medium))
                .foregroundColor(NativeTheme.glassMuted.opacity(0.72))
            HStack(spacing: 6) {
                HStack(spacing: 9) {
                    Circle()
                        .fill(model.isPaused ? NativeTheme.accent : NativeTheme.signal)
                        .frame(width: 10, height: 10)
                        .shadow(color: (model.isPaused ? NativeTheme.accent : NativeTheme.signal).opacity(0.3), radius: 4)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(model.elapsed).font(.system(size: 15, weight: .semibold, design: .rounded)).monospacedDigit()
                        Text(model.statusLabel.uppercased()).font(.system(size: 8, weight: .semibold)).foregroundColor(NativeTheme.glassMuted)
                    }
                }
                .frame(width: 96, alignment: .leading)

                HUDButton(symbol: "stop.fill", help: "Stop and save", tint: NativeTheme.signal, disabled: !model.canControl, action: model.stop)
                HUDButton(symbol: model.isPaused ? "play.fill" : "pause.fill", help: model.isPaused ? "Resume" : "Pause", disabled: !model.canControl, action: model.togglePause)
                HUDButton(symbol: "arrow.counterclockwise", help: "Restart", disabled: model.busy, action: model.restart)
                HUDButton(symbol: "camera", help: "Take a region screenshot", disabled: !model.canControl, action: model.takeScreenshot)

                if !AppStore.shared.settings.microphoneID.isEmpty {
                    GeometryReader { proxy in
                        ZStack(alignment: .leading) {
                            Capsule().fill(Color.black.opacity(0.4))
                            Capsule().fill(Color.green).frame(width: proxy.size.width * model.microphoneLevel)
                        }
                    }
                    .frame(width: 34, height: 4)
                }

                HUDButton(symbol: model.microphoneMuted ? "mic.slash" : "mic", help: model.microphoneMuted ? "Unmute" : "Mute", tint: model.microphoneMuted ? NativeTheme.accent : nil, disabled: !model.canControl || AppStore.shared.settings.microphoneID.isEmpty, action: model.toggleMute)
                HUDButton(symbol: "trash", help: "Delete recording", tint: NativeTheme.signal, disabled: model.busy || model.state == "finalizing", action: model.discard)
                HUDButton(symbol: "eye.slash", help: "Hide controls", disabled: model.busy, action: RecordingHUDController.shared.hide)
            }
            if !model.warning.isEmpty {
                Text(model.warning).lineLimit(1).font(.system(size: 9)).foregroundColor(NativeTheme.signal)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .frame(width: 418)
        .frame(minHeight: 70)
        .foregroundColor(NativeTheme.glassText)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 18))
        .background(NativeTheme.glass.opacity(0.76), in: RoundedRectangle(cornerRadius: 18))
        .overlay(RoundedRectangle(cornerRadius: 18).stroke(Color.white.opacity(0.14)))
        .shadow(color: .black.opacity(0.32), radius: 14, y: 6)
        .padding(6)
        .preferredColorScheme(.dark)
    }
}

private struct HUDButton: View {
    let symbol: String
    let help: String
    var tint: Color? = nil
    var disabled = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol).font(.system(size: 13, weight: .semibold)).frame(width: 30, height: 30)
        }
        .buttonStyle(.plain)
        .foregroundColor(tint ?? NativeTheme.glassMuted)
        .background((tint ?? NativeTheme.glassText).opacity(tint == nil ? 0.04 : 0.12), in: RoundedRectangle(cornerRadius: 7))
        .disabled(disabled)
        .opacity(disabled ? 0.32 : 1)
        .help(help)
    }
}

private extension View {
    @ViewBuilder func cursor(_ cursor: NSCursor) -> some View {
        self.onHover { inside in
            if inside { cursor.push() } else { NSCursor.pop() }
        }
    }
}
