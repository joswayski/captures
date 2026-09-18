import AppKit
import XCTest
@testable import CapturesNative

final class HistoryClearTests: XCTestCase {
    func testCancelConfirmationClearAndPartialFailureRefresh() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let image = PreviewView.fixtureImage(scale: 1)
        let png = try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]))
        let path = directory.appendingPathComponent("fixture.png")
        try png.write(to: path)

        for failPartway in [false, true] {
            let appearance = failPartway ? "dark" : "light"
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame); window.contentView = root
            let transport = HistoryTransport(path: path.path, width: image.width, height: image.height, failPartway: failPartway)
            let controller = LiveCaptureController(root: root, window: window,
                tokens: Tokens.variants["\(appearance)-mustard"]!, historyRoot: directory.path,
                settingsPath: nil, transport: transport, showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            let clear = try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }.first { $0.title == "Clear history…" })
            let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? NSTableView)
            try waitUntil { table.numberOfRows == 2 && clear.isEnabled }
            XCTAssertEqual(clear.accessibilityLabel(), "Clear history…")

            clear.performClick(nil)
            try waitUntil { window.attachedSheet != nil }
            XCTAssertEqual(transport.clearCount, 0, "opening confirmation cannot delete anything")
            window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertSecondButtonReturn)
            try waitUntil { window.attachedSheet == nil }
            XCTAssertEqual(transport.clearCount, 0)
            XCTAssertEqual(table.numberOfRows, 2)

            clear.performClick(nil)
            try waitUntil { window.attachedSheet != nil }
            window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertFirstButtonReturn)
            try waitUntil {
                transport.clearCount == 1 && table.numberOfRows == (failPartway ? 1 : 0)
                    && clear.isEnabled == failPartway
            }
            if failPartway {
                XCTAssertTrue(root.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
                    .contains { $0.contains("Couldn’t clear history") && $0.contains("fixture deletion failed") })
            } else {
                XCTAssertEqual(table.selectedRow, -1)
                XCTAssertTrue(root.subviews.compactMap { $0 as? CaptureButton }
                    .filter { ["Copy image", "Save image", "Delete from history"].contains($0.title) }
                    .allSatisfy { !$0.isEnabled })
            }
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                let folder = URL(fileURLWithPath: output)
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                try png.write(to: folder.appendingPathComponent("history-clear-\(appearance)-\(failPartway ? "error" : "empty").png"))
            }
        }
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        XCTAssertTrue(condition(), "native history action did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}

private final class HistoryTransport: AppTransport {
    private let lock = NSLock()
    private var artifacts: [[String: Any]]
    private var clears = 0
    private let failPartway: Bool

    init(path: String, width: Int, height: Int, failPartway: Bool) {
        self.failPartway = failPartway
        artifacts = ["one", "two"].map { id in
            ["entry": ["id": id, "width": width, "height": height, "created_at": "2026-09-18T00:00:00Z"],
             "image_path": path, "preview_path": path]
        }
    }

    var clearCount: Int { lock.lock(); defer { lock.unlock() }; return clears }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        switch object["operation"] as? String {
        case "history": return ["kind": "history", "artifacts": artifacts]
        case "displays": return ["kind": "displays", "displays": []]
        case "clear_history":
            clears += 1
            artifacts.removeFirst()
            if failPartway { throw AppBridgeError.backend("fixture deletion failed") }
            artifacts.removeAll()
            return ["kind": "history", "artifacts": artifacts]
        default: throw AppBridgeError.invalidResponse
        }
    }
}
