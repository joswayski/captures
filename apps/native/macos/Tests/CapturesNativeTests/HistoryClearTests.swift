import AppKit
import XCTest
@testable import CapturesNative

final class HistoryClearTests: XCTestCase {
    func testMixedHistoryFiltersRetainSelectionAndHandleEmptyRefresh() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let image = PreviewView.fixtureImage(scale: 1)
        let path = directory.appendingPathComponent("fixture.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])).write(to: path)
        let settingsPath = directory.appendingPathComponent("settings.json").path
        let settingsBridge = SettingsBridge()
        var settings = try XCTUnwrap(settingsBridge.request(["operation": "load", "path": settingsPath])["settings"] as? [String: Any])
        settings["output_directory"] = directory.appendingPathComponent("exports").path
        settings["screenshot_format"] = "jpeg"
        _ = try settingsBridge.request(["operation": "save", "path": settingsPath, "settings": settings])
        for appearance in ["light", "dark"] {
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame); window.contentView = root
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            root.wantsLayer = true; root.layer!.backgroundColor = tokens.color("surface-canvas").cgColor
            window.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
            let transport = HistoryTransport(path: path.path, width: image.width, height: image.height,
                failPartway: false, kinds: ["video", "screenshot", "gif", "screenshot", "video"])
            let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
                historyRoot: directory.path, settingsPath: settingsPath, transport: transport,
                recoveryWorker: EmptyRecoveryWorker(), showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? NSTableView)
            func button(_ title: String) throws -> CaptureButton {
                try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }.first { $0.title == title })
            }
            func detailContains(_ text: String) -> Bool {
                root.subviews.compactMap { ($0 as? NSTextField)?.stringValue }.contains { $0.contains(text) }
            }
            try waitUntil { table.numberOfRows == 5 && detailContains("H.264 MP4") }
            XCTAssertEqual(try button("All 5").state, .on)
            try button("Screenshots 2").performClick(nil)
            try waitUntil { table.numberOfRows == 2 && detailContains("\(image.width + 1) × \(image.height) · PNG") }
            table.selectRowIndexes([1], byExtendingSelection: false)
            try waitUntil { detailContains("\(image.width + 3) × \(image.height) · PNG") }
            transport.promote(id: "item-3")
            try button("Refresh").performClick(nil)
            try waitUntil { table.selectedRow == 0 && detailContains("\(image.width + 3) × \(image.height) · PNG") }
            XCTAssertEqual(try button("Screenshots 2").state, .on)
            try button("Video 2").performClick(nil)
            try waitUntil { table.numberOfRows == 2 && detailContains("H.264 MP4") }
            XCTAssertFalse(try button("Copy image").isEnabled)
            XCTAssertTrue(try button("Edit recording").isEnabled)
            try button("GIF 1").performClick(nil)
            try waitUntil { table.numberOfRows == 1 && detailContains("GIF · Editor available") }
            XCTAssertEqual(try button("GIF 1").state, .on)
            XCTAssertTrue(try button("Edit recording").isEnabled)
            XCTAssertTrue(try button("Save file").isEnabled)
            XCTAssertFalse(try button("Show in Folder").isEnabled)
            try button("Save file").performClick(nil)
            try waitUntil { transport.saveCount == 1 && detailContains("Saved recording to") }
            XCTAssertTrue(try button("Show in Folder").isEnabled)
            XCTAssertFalse(try button("Copy image").isEnabled)
            XCTAssertEqual(transport.savedDirectory, settings["output_directory"] as? String)
            XCTAssertEqual(table.selectedRow, 0)
            for title in ["All 5", "Screenshots 2", "Video 2", "GIF 1"] {
                XCTAssertTrue(root.bounds.contains(try button(title).frame))
            }
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                let folder = URL(fileURLWithPath: output)
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                    .write(to: folder.appendingPathComponent("history-filter-\(appearance)-gif.png"))
            }
            transport.remove(kind: "gif")
            try button("Refresh").performClick(nil)
            try waitUntil { table.numberOfRows == 0 && detailContains("No captures match this filter") }
            XCTAssertEqual(table.selectedRow, -1)
            XCTAssertFalse(try button("GIF 0").isEnabled)
            XCTAssertEqual(try button("GIF 0").state, .on)
            XCTAssertFalse(try button("Delete from history").isEnabled)
            try button("All 4").performClick(nil)
            try waitUntil { table.numberOfRows == 4 }
            XCTAssertEqual(transport.clearCount, 0, "filtering never deletes files")
        }
    }

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
            let tokens = Tokens.variants["\(appearance)-mustard"]!
            let root = Surface(frame: frame); window.contentView = root
            root.wantsLayer = true; root.layer!.backgroundColor = tokens.color("surface-canvas").cgColor
            window.appearance = NSAppearance(named: failPartway ? .darkAqua : .aqua)
            let transport = HistoryTransport(path: path.path, width: image.width, height: image.height,
                failPartway: failPartway, kinds: failPartway ? ["screenshot", "video", "gif"] : ["video", "gif"])
            let controller = LiveCaptureController(root: root, window: window,
                tokens: tokens, historyRoot: directory.path,
                settingsPath: nil, transport: transport,
                recoveryWorker: EmptyRecoveryWorker(), showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            let clear = try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }.first { $0.title == "Clear history…" })
            let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? NSTableView)
            try waitUntil { table.numberOfRows == (failPartway ? 3 : 2) && clear.isEnabled }
            XCTAssertEqual(clear.accessibilityLabel(), "Clear history…")
            let gif = try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }.first { $0.title == "GIF 1" })
            gif.performClick(nil)
            try waitUntil { table.numberOfRows == 1 }

            clear.performClick(nil)
            try waitUntil { window.attachedSheet != nil }
            XCTAssertEqual(transport.clearCount, 0, "opening confirmation cannot delete anything")
            window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertSecondButtonReturn)
            try waitUntil { window.attachedSheet == nil }
            XCTAssertEqual(transport.clearCount, 0)
            XCTAssertEqual(table.numberOfRows, 1)

            clear.performClick(nil)
            try waitUntil { window.attachedSheet != nil }
            let blockedGate = failPartway ? nil : transport.blockNextHistoryAfterClear()
            window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertFirstButtonReturn)
            if !failPartway {
                try waitUntil { transport.blockedHistoryStarted.wait(timeout: .now()) == .success }
                controller.refreshHistory() // supersedes clear-owned reload before it returns
                blockedGate?.signal()
            }
            try waitUntil {
                transport.clearCount == 1 && table.numberOfRows == (failPartway ? 1 : 0)
                    && clear.isEnabled == failPartway
            }
            if !failPartway {
                let refresh = try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }
                    .first { $0.title == "Refresh" })
                try waitUntil { refresh.isEnabled }
            }
            if failPartway {
                XCTAssertTrue(root.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
                    .contains { $0.contains("Couldn’t clear history") && $0.contains("fixture deletion failed") })
            } else {
                XCTAssertEqual(table.selectedRow, -1)
                XCTAssertTrue(root.subviews.compactMap { $0 as? CaptureButton }
                    .filter { ["Edit screenshot", "Edit recording", "Copy image", "Save image", "Delete from history"].contains($0.title) }
                    .allSatisfy { !$0.isEnabled })
            }
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 500, y: 50)).alphaComponent, 1, accuracy: 0.01,
                    "render the same opaque canvas as the real workspace")
                let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                let folder = URL(fileURLWithPath: output)
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                try png.write(to: folder.appendingPathComponent("history-clear-\(appearance)-\(failPartway ? "error" : "empty").png"))
            }
            if failPartway {
                clear.performClick(nil)
                try waitUntil { window.attachedSheet != nil }
                window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertFirstButtonReturn)
                try waitUntil { transport.clearCount == 2 && table.numberOfRows == 0 && !clear.isEnabled }
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

private final class EmptyRecoveryWorker: RecordingRecoveryWorking {
    func list(historyRoot: String, completion: @escaping (Result<[RecordingRecoveryDraft], Error>) -> Void) {
        completion(.success([]))
    }
    func recover(historyRoot: String, draft: RecordingRecoveryDraft, cancel: NativeRecordingEditorCancel,
                 progress: @escaping (String) -> Void,
                 completion: @escaping (Result<RecordingRecoveryResult, Error>) -> Void) {
        XCTFail("History-only fixture must not recover a recording")
    }
    func discard(historyRoot: String, draft: RecordingRecoveryDraft,
                 completion: @escaping (Result<Void, Error>) -> Void) {
        XCTFail("History-only fixture must not discard a recording")
    }
}

private final class HistoryTransport: AppTransport {
    private let lock = NSLock()
    private var artifacts: [[String: Any]]
    private var clears = 0
    private var blockedHistory: DispatchSemaphore?
    let blockedHistoryStarted = DispatchSemaphore(value: 0)
    private var failPartway: Bool
    private var saves = 0
    private var exportDirectory: String?

    init(path: String, width: Int, height: Int, failPartway: Bool,
         kinds: [String] = ["screenshot", "screenshot"]) {
        self.failPartway = failPartway
        artifacts = kinds.enumerated().map { index, kind in
            ["entry": ["id": "item-\(index)", "kind": kind, "width": width + index, "height": height,
                       "created_at": "2026-09-18T00:00:0\(kinds.count - index)Z"],
             "image_path": path, "preview_path": path]
        }
    }

    func remove(kind: String) {
        lock.lock(); defer { lock.unlock() }
        artifacts.removeAll { ($0["entry"] as? [String: Any])?["kind"] as? String == kind }
    }

    func promote(id: String) {
        lock.lock(); defer { lock.unlock() }
        for index in artifacts.indices {
            guard var entry = artifacts[index]["entry"] as? [String: Any], entry["id"] as? String == id else { continue }
            entry["created_at"] = "2026-09-19T00:00:00Z"; artifacts[index]["entry"] = entry
        }
    }

    var clearCount: Int { lock.lock(); defer { lock.unlock() }; return clears }
    var saveCount: Int { lock.lock(); defer { lock.unlock() }; return saves }
    var savedDirectory: String? { lock.lock(); defer { lock.unlock() }; return exportDirectory }

    func blockNextHistoryAfterClear() -> DispatchSemaphore {
        let gate = DispatchSemaphore(value: 0)
        lock.lock(); blockedHistory = gate; lock.unlock()
        return gate
    }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        switch object["operation"] as? String {
        case "history":
            if clears > 0, let blockedHistory {
                blockedHistoryStarted.signal()
                _ = blockedHistory.wait(timeout: .now() + 5)
                self.blockedHistory = nil
            }
            return ["kind": "history", "artifacts": artifacts]
        case "displays": return ["kind": "displays", "displays": []]
        case "save_recording":
            guard let directory = object["directory"] as? String,
                  let index = artifacts.firstIndex(where: { ($0["entry"] as? [String: Any])?["id"] as? String == object["id"] as? String }),
                  var entry = artifacts[index]["entry"] as? [String: Any],
                  entry["kind"] as? String == "gif" else { throw AppBridgeError.invalidResponse }
            let path = URL(fileURLWithPath: directory).appendingPathComponent("saved.gif").path
            entry["saved_path"] = path; artifacts[index]["entry"] = entry
            saves += 1; exportDirectory = directory
            return ["kind": "saved", "artifact": artifacts[index], "path": path]
        case "clear_history":
            clears += 1
            artifacts.removeFirst()
            if failPartway { failPartway = false; throw AppBridgeError.backend("fixture deletion failed") }
            artifacts.removeAll()
            return ["kind": "history", "artifacts": artifacts]
        default: throw AppBridgeError.invalidResponse
        }
    }
}
