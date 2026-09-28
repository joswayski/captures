import AppKit
import XCTest
@testable import CapturesNative

final class RecordingDisplayScreenshotTests: XCTestCase {
    func testDisplayRouteSharesTheShippingRecordingRule() throws {
        // `captures_app::capture_error::display_route` through the settings ABI.
        let bridge = SettingsBridge()
        XCTAssertEqual(try DisplayCaptureRoute(recordingState: nil, transport: bridge), .captureMenu)
        XCTAssertEqual(try DisplayCaptureRoute(recordingState: "failed", transport: bridge), .captureMenu)
        XCTAssertEqual(try DisplayCaptureRoute(recordingState: "recording", transport: bridge),
                       .captureDisplay)
        XCTAssertEqual(try DisplayCaptureRoute(recordingState: "paused", transport: bridge),
                       .captureDisplay)
        XCTAssertEqual(try DisplayCaptureRoute(recordingState: "countdown", transport: bridge), .ignore,
            "shipping `screenshot_capture_is_blocked` refuses a counting-down take")
        XCTAssertEqual(try DisplayCaptureRoute(recordingState: "finalizing", transport: bridge), .ignore)
    }

    func testEveryScreenshotGoesBesideARunningTake() {
        XCTAssertEqual(stillCaptureRoute(for: .display, displayRoute: .captureDisplay),
                       .recordingScreenshot(.display))
        XCTAssertEqual(stillCaptureRoute(for: .display, displayRoute: .ignore), .ignore)
        XCTAssertEqual(stillCaptureRoute(for: .display, displayRoute: .captureMenu), .menu(.display))
        XCTAssertEqual(stillCaptureRoute(for: .region, displayRoute: .captureDisplay),
                       .recordingScreenshot(.region))
        XCTAssertEqual(stillCaptureRoute(for: .window, displayRoute: .ignore), .ignore)
    }

    func testOneDisplayScreenshotAtATimeFromARunningOrPausedTake() {
        XCTAssertTrue(displayScreenshotAvailableDuringRecording(routeState: "recording",
            screenshotActive: false, lifecycleBusy: false))
        XCTAssertTrue(displayScreenshotAvailableDuringRecording(routeState: "paused",
            screenshotActive: false, lifecycleBusy: false))
        XCTAssertFalse(displayScreenshotAvailableDuringRecording(routeState: "recording",
            screenshotActive: true, lifecycleBusy: false), "a screenshot is already running")
        XCTAssertFalse(displayScreenshotAvailableDuringRecording(routeState: "recording",
            screenshotActive: false, lifecycleBusy: true), "pause, stop or restart owns the take")
        for state in [nil, "countdown", "finalizing", "failed"] as [String?] {
            XCTAssertFalse(displayScreenshotAvailableDuringRecording(routeState: state,
                screenshotActive: false, lifecycleBusy: false), "\(state ?? "no take")")
        }
    }

    func testNewCaptureSharesTheShippingRecordingRule() throws {
        // `captures_app::capture_error::new_capture_route` through the settings ABI.
        let bridge = SettingsBridge()
        let busy = NewCaptureRoute.inProgress(
            message: "Captures could not start the capture: capture already in progress")
        XCTAssertEqual(try NewCaptureRoute(recordingState: nil, controlsHidden: false,
                                           transport: bridge), .captureMenu)
        XCTAssertEqual(try NewCaptureRoute(recordingState: "failed", controlsHidden: false,
                                           transport: bridge), .captureMenu)
        XCTAssertEqual(try NewCaptureRoute(recordingState: "recording", controlsHidden: true,
                                           transport: bridge), .restoreControls)
        XCTAssertEqual(try NewCaptureRoute(recordingState: "paused", controlsHidden: true,
                                           transport: bridge), .restoreControls)
        XCTAssertEqual(try NewCaptureRoute(recordingState: "recording", controlsHidden: false,
                                           transport: bridge), busy,
                       "shipping `open_capture_controls` reports `CaptureInProgress`")
        XCTAssertEqual(try NewCaptureRoute(recordingState: "countdown", controlsHidden: false,
                                           transport: bridge), busy)
        XCTAssertEqual(try NewCaptureRoute(recordingState: "finalizing", controlsHidden: false,
                                           transport: bridge), busy)
    }

    func testControlsLeaveTheScreenshotUnlessOptedIn() {
        XCTAssertTrue(recordingDisplayScreenshotHidesControls(includeControls: false))
        XCTAssertFalse(recordingDisplayScreenshotHidesControls(includeControls: true))
    }

    func testShortcutsStayRoutableWhileACaptureIsInFlight() {
        XCTAssertTrue(captureShortcutsEnabled(captureBusy: true, selectorGeneration: nil,
            captureRoutes: true), "the shared routes deliver every chord to the host")
        XCTAssertFalse(captureShortcutsEnabled(captureBusy: true, selectorGeneration: nil,
            captureRoutes: false))
        XCTAssertEqual(CaptureShortcut.recordWindow.captureAction, .record(.window))
        XCTAssertEqual(CaptureShortcut.display.captureAction, .screenshot(.display))
        XCTAssertEqual(CaptureShortcut.newCapture.captureAction, .newCapture)
    }

    func testBusyCaptureRouteSharesTheShippingRule() throws {
        // `captures_app::capture_error::busy_route` through the settings ABI.
        let bridge = SettingsBridge()
        let busy = BusyCaptureRoute.inProgress(
            message: "Captures could not start the capture: capture already in progress")
        XCTAssertEqual(try BusyCaptureRoute(action: .screenshot(.window), activity: .selector,
            recordingState: nil, controlsOnScreen: false, transport: bridge),
            .recaptureSelector(.window), "pressing a shortcut again recaptures the selector")
        XCTAssertEqual(try BusyCaptureRoute(action: .newCapture, activity: .selector,
            recordingState: nil, controlsOnScreen: false, transport: bridge),
            .recaptureMenu(record: false, target: .region))
        XCTAssertEqual(try BusyCaptureRoute(action: .record(.display), activity: .selector,
            recordingState: nil, controlsOnScreen: false, transport: bridge),
            .recaptureMenu(record: true, target: .display))
        XCTAssertEqual(try BusyCaptureRoute(action: .screenshot(.display), activity: .selector,
            recordingState: "recording", controlsOnScreen: false, transport: bridge),
            .recaptureDisplay)
        XCTAssertEqual(try BusyCaptureRoute(action: .screenshot(.region),
            activity: .menu(screenshotTarget: .region), recordingState: nil,
            controlsOnScreen: false, transport: bridge), .recaptureSelector(.region))
        XCTAssertEqual(try BusyCaptureRoute(action: .newCapture,
            activity: .menu(screenshotTarget: nil), recordingState: nil,
            controlsOnScreen: false, transport: bridge), .switchMenu(record: false, target: .region))
        XCTAssertEqual(try BusyCaptureRoute(action: .newCapture, activity: .busy,
            recordingState: nil, controlsOnScreen: false, transport: bridge), busy)
        XCTAssertEqual(try BusyCaptureRoute(action: .screenshot(.region), activity: .busy,
            recordingState: nil, controlsOnScreen: false, transport: bridge), .ignore)
        XCTAssertEqual(try BusyCaptureRoute(action: .newCapture, activity: .busy,
            recordingState: "paused", controlsOnScreen: false, transport: bridge),
            .restoreControls, "concealed controls come back like hidden ones")
        XCTAssertEqual(try BusyCaptureRoute(action: .newCapture, activity: .selector,
            recordingState: "recording", controlsOnScreen: true, transport: bridge), busy)
        XCTAssertEqual(try BusyCaptureRoute(action: .newCapture, activity: .idle,
            recordingState: nil, controlsOnScreen: false, transport: bridge), .idle)
    }

    func testRecapturedSelectionsFreezeWithoutCountdown() throws {
        let preferences = try CapturePreferences(["auto_copy_to_clipboard": false,
            "show_cursor_in_screenshots": true, "output_directory": "/tmp",
            "screenshot_format": "png", "freeze_screen": false, "auto_start_on_selection": false,
            "show_mini_previews": true, "mini_preview_placement": "bottom_left",
            "include_mini_previews_in_captures": false, "screenshot_countdown_seconds": 3])
        let recaptured = preferences.recapturing()
        XCTAssertEqual(recaptured.countdown, 0)
        XCTAssertTrue(recaptured.freezeScreen)
        XCTAssertEqual(preferences.countdown, 3)
        XCTAssertFalse(preferences.freezeScreen)
        XCTAssertEqual(recaptured.autoCopy, preferences.autoCopy)
    }

    func testDirectDisplayScreenshotNeedsARecording() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let settingsPath = folder.appendingPathComponent("settings.json").path
        let settingsBridge = SettingsBridge()
        var settings = try XCTUnwrap(settingsBridge.request([
            "operation": "load", "path": settingsPath])["settings"] as? [String: Any])
        settings["output_directory"] = folder.path
        _ = try settingsBridge.request(["operation": "save", "path": settingsPath, "settings": settings])
        let transport = FixtureDisplayTransport()
        var reported: [String] = []
        let frame = NSRect(x: 0, y: 0, width: 800, height: 560)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: folder.path, settingsPath: settingsPath, transport: transport,
            recoveryWorker: NoDraftsRecoveryWorker(),
            reportError: { reported.append($0) })
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let deadline = Date().addingTimeInterval(5)
        while !controller.captureReady && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(controller.captureReady)

        XCTAssertNil(controller.recordingRouteState)
        XCTAssertEqual(controller.displayCaptureRoute, .captureMenu)
        XCTAssertFalse(controller.recordingDisplayScreenshotAvailable)
        XCTAssertFalse(controller.captureDisplayWhileRecording(),
            "without a take, Screenshot Display opens the capture menu instead")
        XCTAssertFalse(controller.captureWhileRecording(.region),
            "without a take, Screenshot Region opens its own selector instead")
        XCTAssertFalse(controller.captureWhileRecording(.window))
        XCTAssertFalse(controller.captureWhileRecording(.display))
        XCTAssertEqual(controller.newCaptureRoute, .captureMenu)
        XCTAssertFalse(controller.captureInFlight)
        XCTAssertEqual(controller.captureActivity, .idle)
        XCTAssertEqual(controller.routeBusyCaptureAction(.screenshot(.region)), .idle,
            "with nothing open a shortcut starts the idle way")
        XCTAssertTrue(reported.isEmpty, "refusing a direct screenshot is silent")
        XCTAssertEqual(transport.captureRequests, 0)
    }
}

/// History is empty and one fixture display is listed; captures are counted.
private final class FixtureDisplayTransport: AppTransport {
    private let lock = NSLock()
    private var captures = 0
    var captureRequests: Int { lock.lock(); defer { lock.unlock() }; return captures }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        switch object["operation"] as? String {
        case "history":
            return ["artifacts": [[String: Any]]()]
        case "displays":
            return ["displays": [[
                "id": "fixture", "name": "Fixture", "width": 800, "height": 560,
                "x": 0, "y": 0, "scale_factor": 1, "is_primary": true,
            ]]]
        case "capture_display":
            lock.lock(); captures += 1; lock.unlock()
            throw AppBridgeError.backend("unexpected capture")
        default: throw AppBridgeError.invalidResponse
        }
    }
}

private final class NoDraftsRecoveryWorker: RecordingRecoveryWorking {
    func list(historyRoot: String, completion: @escaping (Result<[RecordingRecoveryDraft], Error>) -> Void) {
        completion(.success([]))
    }
    func recover(historyRoot: String, draft: RecordingRecoveryDraft, cancel: NativeRecordingEditorCancel,
                 progress: @escaping (String) -> Void,
                 completion: @escaping (Result<RecordingRecoveryResult, Error>) -> Void) {
        completion(.failure(AppBridgeError.invalidResponse))
    }
    func discard(historyRoot: String, draft: RecordingRecoveryDraft,
                 completion: @escaping (Result<Void, Error>) -> Void) {
        completion(.success(()))
    }
}
