import AppKit
import XCTest
@testable import CapturesNative

final class OpenImageTests: XCTestCase {
    func testQueuedFilesRetainErrorsAndSerializeCanonicalOpens() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let png = folder.appendingPathComponent("source.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:])).write(to: png)
        let settingsPath = folder.appendingPathComponent("settings.json").path
        let settingsBridge = SettingsBridge()
        var settings = try XCTUnwrap(settingsBridge.request([
            "operation": "load", "path": settingsPath])["settings"] as? [String: Any])
        settings["output_directory"] = folder.path
        _ = try settingsBridge.request(["operation": "save", "path": settingsPath, "settings": settings])
        let transport = OpenImageTransport(image: png.path)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["light-mustard"]),
            historyRoot: folder.path, settingsPath: settingsPath, transport: transport,
            showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        defer { NSApp.windows.filter { $0.title.hasPrefix("Edit screenshot") }.forEach { $0.orderOut(nil) } }
        window.makeKeyAndOrderFront(nil)
        controller.openImages(["/invalid.gif", png.path, png.path])
        XCTAssertFalse(controller.prepareEditorForTermination(), "queued startup opens block teardown")
        try waitUntil { transport.firstOpenStarted.wait(timeout: .now()) == .success }
        XCTAssertEqual(transport.requests.count, 1)
        XCTAssertTrue(controller.externalOpenPending)
        XCTAssertFalse(try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }
            .first { $0.title == "Edit screenshot" }).isEnabled)
        transport.releaseFirstOpen.signal()
        try waitUntil { transport.requests.count == 3 && !controller.externalOpenPending }
        XCTAssertTrue(controller.prepareEditorForTermination())
        let requests = transport.requests
        XCTAssertEqual(requests.compactMap { $0["path"] as? String },
                       ["/invalid.gif", png.path, png.path])
        XCTAssertTrue(requests.allSatisfy { ($0["root"] as? String) == folder.path })
        XCTAssertEqual(requests[0]["open_artifact_ids"] as? [String], [])
        XCTAssertEqual(requests[1]["open_artifact_ids"] as? [String], [])
        XCTAssertEqual(requests[2]["open_artifact_ids"] as? [String], ["opened-id"])
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
            .first?.documentView as? NSTableView)
        XCTAssertEqual(table.numberOfRows, 1)
        XCTAssertEqual(table.selectedRow, 0)
        XCTAssertTrue(root.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
            .contains { $0.contains("/invalid.gif") && $0.contains("Unsupported") })
        XCTAssertEqual(transport.operations.filter { $0 == "open_image" }.count, 3)
        XCTAssertFalse(transport.operations.contains("request_permission"))
    }

    func testSelectionChangedDuringOpenDoesNotStealFocus() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let png = folder.appendingPathComponent("source.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:])).write(to: png)
        let transport = OpenImageTransport(image: png.path, existing: true)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]),
            historyRoot: folder.path, settingsPath: nil, transport: transport,
            showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }
            .first?.documentView as? NSTableView)
        try waitUntil { table.numberOfRows == 2 }
        controller.openImages([png.path])
        try waitUntil { transport.firstOpenStarted.wait(timeout: .now()) == .success }
        table.selectRowIndexes([1], byExtendingSelection: false)
        transport.releaseFirstOpen.signal()
        try waitUntil { !controller.externalOpenPending }
        XCTAssertEqual(table.selectedRow, 1, "late History must keep the newer selection")
        XCTAssertFalse(NSApp.windows.contains { $0.title.hasPrefix("Edit screenshot") && $0.isVisible })
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline {
            if condition() { return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTFail("external open did not settle")
        throw AppBridgeError.invalidResponse
    }
}

private final class OpenImageTransport: AppTransport {
    let firstOpenStarted = DispatchSemaphore(value: 0)
    let releaseFirstOpen = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private let image: String
    private let existing: Bool
    private var opened = false
    private var seen: [[String: Any]] = []
    private var calls: [String] = []
    var requests: [[String: Any]] { lock.lock(); defer { lock.unlock() }; return seen }
    var operations: [String] { lock.lock(); defer { lock.unlock() }; return calls }

    init(image: String, existing: Bool = false) {
        self.image = image; self.existing = existing
    }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        let operation = object["operation"] as? String ?? ""
        lock.lock(); calls.append(operation); lock.unlock()
        switch operation {
        case "history":
            lock.lock(); let isOpened = opened; lock.unlock()
            let artifact = entry("opened-id")
            return ["artifacts": existing
                ? [artifact, entry("original-id")]
                : (isOpened ? [artifact] : [])]
        case "displays": return ["displays": []]
        case "open_image":
            lock.lock(); seen.append(object); let count = seen.count; lock.unlock()
            if count == 1 {
                firstOpenStarted.signal()
                _ = releaseFirstOpen.wait(timeout: .now() + 5)
            }
            if object["path"] as? String == "/invalid.gif" {
                throw AppBridgeError.backend("Unsupported image format")
            }
            lock.lock(); opened = true; lock.unlock()
            return ["artifact": entry("opened-id"), "already_open": count > 2 || existing]
        default: throw AppBridgeError.invalidResponse
        }
    }

    private func entry(_ id: String) -> [String: Any] {
        ["entry": ["id": id, "kind": "screenshot", "width": 284, "height": 160,
                   "created_at": id == "opened-id" ? "2026-09-24T00:00:00Z" : "2026-09-23T00:00:00Z"],
         "image_path": image, "preview_path": image]
    }
}
