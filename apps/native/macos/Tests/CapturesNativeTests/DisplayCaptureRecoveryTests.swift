import AppKit
import XCTest
@testable import CapturesNative

final class DisplayCaptureRecoveryTests: XCTestCase {
    func testScreenshotDisplayOpensTheCaptureMenuUnlessRecording() {
        // Shipping `start_capture_from_tray` and the display shortcut.
        XCTAssertEqual(stillCaptureRoute(for: .display, displayRoute: .captureMenu), .menu(.display))
        XCTAssertEqual(stillCaptureRoute(for: .display, displayRoute: .captureDisplay),
            .recordingScreenshot, "a running take captures the display directly")
        XCTAssertEqual(stillCaptureRoute(for: .region, displayRoute: .captureMenu), .capture(.region))
        XCTAssertEqual(stillCaptureRoute(for: .window, displayRoute: .captureDisplay), .capture(.window))
    }

    func testDisplayListIsListedAgainWhenEmptyOrStale() {
        XCTAssertTrue(displayListNeedsRefresh(listed: [], pointer: nil))
        XCTAssertTrue(displayListNeedsRefresh(listed: [], pointer: "1"))
        XCTAssertTrue(displayListNeedsRefresh(listed: ["1"], pointer: "2"),
            "a display attached since the last list is listed again")
        XCTAssertFalse(displayListNeedsRefresh(listed: ["1", "2"], pointer: "2"))
        XCTAssertFalse(displayListNeedsRefresh(listed: ["1"], pointer: nil))
    }

    func testPendingCapturesRetryLikeShipping() {
        XCTAssertEqual(PendingDisplayCapture.still(.display).retryKind, .display)
        XCTAssertEqual(PendingDisplayCapture.still(.window).retryKind, .window)
        XCTAssertEqual(PendingDisplayCapture.menu(recordingTarget: nil, screenshotTarget: .display).retryKind,
                       .region, "New Capture retries as Region")
        XCTAssertNotEqual(PendingDisplayCapture.menu(recordingTarget: .window, screenshotTarget: nil),
                          PendingDisplayCapture.menu(recordingTarget: nil, screenshotTarget: .window))
    }

    func testCaptureFailuresShareTheShippingDialogCopy() throws {
        let copy = try CaptureErrorCopy(transport: SettingsBridge())
        XCTAssertEqual(copy.title, "Captures")
        XCTAssertEqual(copy.button, "OK")
        XCTAssertEqual(copy, CaptureErrorCopy.current)
    }

    func testFailedFirstDisplayListRecoversWhenACaptureStarts() throws {
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
        let transport = RecoveringDisplayTransport()
        var reported: [String] = []
        let frame = NSRect(x: 0, y: 0, width: 800, height: 560)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: folder.path, settingsPath: settingsPath, transport: transport,
            recoveryWorker: IdleRecoveryWorker(),
            reportError: { reported.append($0) })
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        try waitUntil { transport.displayRequests == 1 && transport.historyRequests >= 1 }
        // Let the failed list's completion reach the main queue.
        let settle = Date().addingTimeInterval(0.3)
        while Date() < settle { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        XCTAssertFalse(controller.captureReady, "the first list failed")
        XCTAssertTrue(reported.isEmpty, "listing ahead of a capture is not a capture failure")
        XCTAssertNil(controller.historyError, "History shows only load and delete errors")

        // The capture lists the displays again instead of reporting "still loading".
        XCTAssertTrue(controller.capture(.region))
        try waitUntil { transport.displayRequests == 2 }
        // The retried capture ran on the recovered list: the fixture display
        // has no NSScreen, so it fails there and reports in the dialog.
        try waitUntil { reported.contains { $0.contains("no longer available") } }
        XCTAssertEqual(reported.count, 1)
        XCTAssertNil(controller.historyError, "capture failures are dialogs, not History errors")
        try waitUntil { controller.captureReady }
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        XCTAssertTrue(condition(), "display recovery did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}

/// History is empty; the first display list fails and later lists succeed.
private final class RecoveringDisplayTransport: AppTransport {
    private let lock = NSLock()
    private var displays = 0
    private var histories = 0
    var displayRequests: Int { lock.lock(); defer { lock.unlock() }; return displays }
    var historyRequests: Int { lock.lock(); defer { lock.unlock() }; return histories }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        switch object["operation"] as? String {
        case "history":
            lock.lock(); histories += 1; lock.unlock()
            return ["artifacts": [[String: Any]]()]
        case "displays":
            lock.lock(); displays += 1; let count = displays; lock.unlock()
            if count == 1 { throw AppBridgeError.backend("monitor query failed") }
            return ["displays": [[
                "id": "fixture", "name": "Fixture", "width": 800, "height": 560,
                "x": 0, "y": 0, "scale_factor": 1, "is_primary": true,
            ]]]
        default: throw AppBridgeError.invalidResponse
        }
    }
}

private final class IdleRecoveryWorker: RecordingRecoveryWorking {
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
