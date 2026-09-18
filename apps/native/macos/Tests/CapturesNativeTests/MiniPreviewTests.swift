import AppKit
import XCTest
import CCapturesSettings
@testable import CapturesNative

final class MiniPreviewTests: XCTestCase {
    private let tokens = Tokens.variants["dark-mustard"]!

    func testCoordinateConversionPreservesNegativeOriginAndTopLeftGeometry() {
        let monitor = CapturesPreviewMonitor(work_x: -2880, work_y: 50,
            work_width: 2880, work_height: 1700, full_x: -2880, full_y: -100,
            full_width: 2880, full_height: 1800, scale_factor: 2)
        let geometry = CapturesPreviewGeometry(x: -1420, y: 25, width: 340, height: 240,
            card_height: 160, padding: 28, control_gutter: 52, anchor: 1)
        let frame = MiniPreviewController.appKitFrame(geometry: geometry, monitor: monitor,
            screenFrame: NSRect(x: -1440, y: 120, width: 1440, height: 900))
        XCTAssertEqual(frame, NSRect(x: -1420, y: 705, width: 340, height: 240))
    }

    func testPanelIsFixedGlassNonactivatingAndActionsStayBoundToArtifact() throws {
        _ = NSApplication.shared
        var actions: [String] = []
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let geometry = fixtureGeometry()
        let panel = MiniPreviewPanel(frame: NSRect(x: 0, y: 0,
            width: geometry.width, height: geometry.height), geometry: geometry,
            artifactID: "latest", image: image, tokens: tokens,
            copy: { actions.append("copy") }, save: { actions.append("save") },
            open: { actions.append("open") }, dismiss: { actions.append("dismiss") })
        defer { panel.close() }

        XCTAssertFalse(panel.canBecomeKey); XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertEqual(panel.previewView.artifactID, "latest")
        let buttons = panel.previewView.subviews.compactMap { $0 as? CaptureButton }
        XCTAssertEqual(buttons.map(\.title), ["Copy", "Save", "Open", "Dismiss"])
        XCTAssertTrue(buttons.allSatisfy(\.glass))
        buttons.forEach { $0.performClick(nil) }
        XCTAssertEqual(actions, ["copy", "save", "open", "dismiss"])
    }

    func testPendingFirstDecodeIsInvalidatedWhenHistoryIsCleared() throws {
        _ = NSApplication.shared
        let releaseDecode = DispatchSemaphore(value: 0)
        let decodeReturned = expectation(description: "blocked preview decode returned")
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in
            releaseDecode.wait(); decodeReturned.fulfill(); return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_left",
                                           includeInCaptures: false)
        let pending = artifact(id: "pending", previewPath: "/pending-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(pending, on: screenID(), settings: settings, generation: generation)
        controller.reconcileHistory(ids: [])
        releaseDecode.signal()
        wait(for: [decodeReturned], timeout: 5)
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertNil(controller.presentedArtifactID)
        XCTAssertFalse(controller.isPanelVisible)
    }

    func testCopyAndSaveRemainAvailableAfterWorkspaceSwitchesToPreferences() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let png = Data([137, 80, 78, 71, 13, 10, 26, 10])
        let imagePath = directory.appendingPathComponent("full.png")
        try png.write(to: imagePath)
        let transport = MiniPreviewActionTransport()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.configure(historyRoot: directory.path)
        var workspaceOwner: NSObject? = NSObject()
        workspaceOwner = nil // Preferences replaced the live workspace controller.
        XCTAssertNil(workspaceOwner)
        let captured = artifact(id: "latest", previewPath: imagePath.path,
                                imagePath: imagePath.path)

        actions.copy(captured)
        actions.save(captured)
        LiveCaptureController.flush()
        try waitUntil {
            transport.saveCount == 1 && NSPasteboard.general.data(forType: .png) == png
        }
        XCTAssertEqual(transport.savedRoot, directory.path)
        XCTAssertEqual(NSPasteboard.general.data(forType: .png), png)
    }

    func testReplacementRejectsLateDecodeAndCaptureCancellationRestoresPanel() throws {
        _ = NSApplication.shared
        let oldDecode = DispatchSemaphore(value: 0)
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/old-preview.png" { oldDecode.wait() }
            return image
        })
        let visible = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                          includeInCaptures: false)
        let old = artifact(id: "old", previewPath: "/old-preview.png")
        let new = artifact(id: "new", previewPath: "/new-preview.png")

        let first = try XCTUnwrap(controller.beginCapture(settings: visible))
        controller.present(old, on: screenID(), settings: visible, generation: first)
        let second = try XCTUnwrap(controller.beginCapture(settings: visible))
        controller.present(new, on: screenID(), settings: visible, generation: second)
        try waitUntil { controller.presentedArtifactID == "new" && controller.isPanelVisible }
        oldDecode.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.presentedArtifactID, "new",
            "an obsolete decode cannot replace the current screenshot")

        let cancelled = try XCTUnwrap(controller.beginCapture(settings: visible))
        XCTAssertFalse(controller.isPanelVisible)
        controller.restoreCapture(generation: cancelled)
        XCTAssertTrue(controller.isPanelVisible)
        let included = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: true)
        let includedCapture = try XCTUnwrap(controller.beginCapture(settings: included))
        XCTAssertTrue(controller.isPanelVisible,
            "the explicit inclusion preference keeps the preview visible during capture")
        controller.restoreCapture(generation: includedCapture)
        controller.updateSettings(MiniPreviewSettings(enabled: false, placement: "top_left",
                                                       includeInCaptures: false))
        XCTAssertFalse(controller.isPanelVisible)
        controller.reconcileHistory(ids: [])
        XCTAssertNil(controller.presentedArtifactID)
    }

    func testRealThumbnailRendersRepresentativePanel() throws {
        _ = NSApplication.shared
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 2),
                            size: NSSize(width: 284, height: 160))
        let geometry = fixtureGeometry()
        let panel = MiniPreviewPanel(frame: NSRect(x: 0, y: 0,
            width: geometry.width, height: geometry.height), geometry: geometry,
            artifactID: "fixture", image: image, tokens: tokens,
            copy: {}, save: {}, open: {}, dismiss: {})
        defer { panel.close() }
        panel.display(); panel.previewView.layoutSubtreeIfNeeded()
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let view = panel.previewView
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 10, y: 10)).alphaComponent,
                       tokens.color("glass-strong").alphaComponent,
                       accuracy: 0.01)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("mini-preview-dark-latest.png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }

    func testClosingRootWindowClosesOpenPanelAndRequestsTermination() {
        _ = NSApplication.shared
        let root = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 500, height: 400),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        let geometry = fixtureGeometry()
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let panel = MiniPreviewPanel(frame: NSRect(x: 0, y: 0,
            width: geometry.width, height: geometry.height), geometry: geometry,
            artifactID: "latest", image: image, tokens: tokens,
            copy: {}, save: {}, open: {}, dismiss: {})
        panel.orderFrontRegardless()
        var terminationRequests = 0
        let handler = RootWindowCloseHandler(rootWindow: root,
            closePreviews: { panel.close() }, terminate: { terminationRequests += 1 })
        root.delegate = handler
        root.makeKeyAndOrderFront(nil)
        root.close()
        XCTAssertFalse(panel.isVisible)
        XCTAssertEqual(terminationRequests, 1)
        withExtendedLifetime(handler) {}
    }

    private func artifact(id: String, previewPath: String,
                          imagePath: String? = nil) -> CaptureArtifact {
        CaptureArtifact(["entry": ["id": id, "width": 800, "height": 600,
            "created_at": "2026-09-18T00:00:00Z"],
            "image_path": imagePath ?? "/\(id).png", "preview_path": previewPath])!
    }

    private func fixtureGeometry() -> CapturesPreviewGeometry {
        CapturesPreviewGeometry(x: 0, y: 0, width: 340, height: 240,
            card_height: 160, padding: 28, control_gutter: 52, anchor: 0)
    }

    private func preferences() throws -> CapturePreferences {
        try CapturePreferences(["auto_copy_to_clipboard": false,
            "show_cursor_in_screenshots": false, "freeze_screen": true,
            "auto_start_on_selection": false, "show_mini_previews": true,
            "mini_preview_placement": "bottom_right",
            "include_mini_previews_in_captures": false,
            "output_directory": "/exports", "screenshot_format": "png",
            "screenshot_countdown_seconds": 0])
    }

    private func screenID() -> String {
        NSScreen.screens.first.flatMap(MiniPreviewController.displayID(for:)) ?? "0"
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), "mini preview operation did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}

private final class MiniPreviewActionTransport: AppTransport {
    private let lock = NSLock()
    private var saves = 0
    private var root: String?
    var saveCount: Int { lock.lock(); defer { lock.unlock() }; return saves }
    var savedRoot: String? { lock.lock(); defer { lock.unlock() }; return root }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        guard object["operation"] as? String == "save_screenshot" else {
            throw AppBridgeError.invalidResponse
        }
        saves += 1; root = object["root"] as? String
        return ["path": "/exports/latest.png"]
    }
}
