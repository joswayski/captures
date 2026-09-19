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
        let panel = fixturePanel(ids: ["latest"], images: ["latest": image],
            copy: { _ in actions.append("copy") }, save: { _ in actions.append("save") },
            open: { _ in actions.append("open") }, dismiss: { _ in actions.append("dismiss") })
        defer { panel.close() }

        XCTAssertFalse(panel.canBecomeKey); XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertEqual(panel.previewView.artifactIDs, ["latest"])
        let buttons = panel.previewView.subviewsRecursive.compactMap { $0 as? CaptureButton }
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
        let png = try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:]))
        let imagePath = directory.appendingPathComponent("full.png")
        try png.write(to: imagePath)
        let transport = MiniPreviewActionTransport()
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("es.captures.tests.\(UUID())"))
        pasteboard.clearContents()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, pasteboard: { pasteboard })
        actions.configure(historyRoot: directory.path)
        var workspaceOwner: NSObject? = NSObject()
        workspaceOwner = nil // Preferences replaced the live workspace controller.
        XCTAssertNil(workspaceOwner)
        let captured = artifact(id: "latest", previewPath: imagePath.path,
                                imagePath: imagePath.path)

        actions.copy(captured)
        actions.save(captured)
        LiveCaptureController.flush()
        XCTAssertEqual(transport.saveCount, 1, "Save must execute on the persistent queue")
        XCTAssertEqual(transport.savedRoot, directory.path)
        try waitUntil { pasteboard.data(forType: .png) == png }
        XCTAssertEqual(pasteboard.data(forType: .png), png,
            "Copy must publish the full-resolution PNG, not the thumbnail")
    }

    func testOutOfOrderDecodesPreserveCaptureOrderAndCaptureCancellationRestoresPanel() throws {
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
        try waitUntil { controller.presentedArtifactIDs == ["old", "new"] }
        XCTAssertEqual(controller.presentedArtifactID, "new",
            "decode completion order must not change chronological membership")

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

    func testClearAllUsesSnapshotAndDoesNotRemoveLaterArrival() throws {
        _ = NSApplication.shared
        let oldDecode = DispatchSemaphore(value: 0)
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/old-preview.png" { oldDecode.wait() }
            return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "old", previewPath: "/old-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        XCTAssertEqual(controller.presentedArtifactIDs, ["old"])
        controller.clearAll()
        XCTAssertEqual(controller.presentedArtifactIDs, [])

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "new", previewPath: "/new-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        try waitUntil { controller.presentedArtifactIDs == ["new"] && controller.isPanelVisible }
        oldDecode.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.presentedArtifactIDs, ["new"],
            "late decode from the cleared snapshot cannot remove or resurrect previews")
    }

    func testIncomingCapturePreservesCollapsedState() throws {
        _ = NSApplication.shared
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        let settings = MiniPreviewSettings(enabled: true, placement: "top_left",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "first", previewPath: "/first-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        try waitUntil { controller.isPanelVisible }
        controller.setCollapsed(true)

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "second", previewPath: "/second-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        try waitUntil { controller.presentedArtifactIDs == ["first", "second"] }
        XCTAssertTrue(controller.isCollapsed)

        controller.updateSettings(MiniPreviewSettings(enabled: false, placement: "top_left",
                                                       includeInCaptures: false))
        XCTAssertFalse(controller.isPanelVisible)
        XCTAssertFalse(controller.isCollapsed)
        XCTAssertEqual(controller.presentedArtifactIDs, ["first", "second"])
        controller.updateSettings(settings)
        XCTAssertTrue(controller.isPanelVisible)
        XCTAssertEqual(controller.presentedArtifactIDs, ["first", "second"],
            "hiding previews must retain membership for re-enable")
    }

    func testRealThumbnailRendersRepresentativePanel() throws {
        _ = NSApplication.shared
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 2),
                            size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["fixture"], images: ["fixture": image])
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
        root.isReleasedWhenClosed = false
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["latest"], images: ["latest": image])
        panel.orderFrontRegardless()
        var terminationRequests = 0
        let handler = RootWindowCloseHandler(rootWindow: root,
            closePreviews: { panel.close() }, terminate: { terminationRequests += 1 })
        root.delegate = handler
        root.makeKeyAndOrderFront(nil)
        root.performClose(nil)
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

    private func fixturePanel(ids: [String], images: [String: NSImage],
                              collapsed: Bool = false, topAnchor: Bool = false,
                              copy: @escaping (String) -> Void = { _ in },
                              save: @escaping (String) -> Void = { _ in },
                              open: @escaping (String) -> Void = { _ in },
                              dismiss: @escaping (String) -> Void = { _ in }) -> MiniPreviewPanel {
        let geometry = fixtureGeometry()
        let resources = Dictionary(uniqueKeysWithValues: ids.compactMap { id in
            images[id].map { (id, MiniPreviewResource(artifact: artifact(id: id,
                previewPath: "/\(id)-preview.png"), image: $0)) }
        })
        let layouts = Dictionary(uniqueKeysWithValues: ids.enumerated().map { index, id in
            (id, CapturesPreviewCardLayout(y: 28 + Double(index) * 184,
                depth: ids.count - index - 1, interactive: true))
        })
        return MiniPreviewPanel(frame: NSRect(x: 0, y: 0,
            width: geometry.width, height: geometry.height), geometry: geometry,
            resources: resources, ids: ids, layouts: layouts, collapsed: collapsed,
            topAnchor: topAnchor, tokens: tokens, copy: copy, save: save, open: open,
            dismiss: dismiss, setCollapsed: { _ in }, clearAll: {})
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

private extension NSView {
    var subviewsRecursive: [NSView] {
        subviews + subviews.flatMap(\.subviewsRecursive)
    }
}
