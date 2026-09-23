import AppKit
import XCTest
@testable import CapturesNative

final class RecordingRecoveryTests: XCTestCase {
    func testRowsConfirmationCancelAndSuccessfulLateRecovery() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let available = try draft("recoverable", identity: "original-identity", kind: "gif")
        let unavailable = try draft("unavailable", identity: nil, kind: nil,
                                    reason: "Manifest is unavailable; inspect the bundle manually.")
        let worker = RecoveryFixtureWorker(drafts: [available, unavailable])
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: folder.path, settingsPath: nil, transport: EmptyHistoryTransport(),
            recoveryWorker: worker, showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let panel = try recoveryPanel(root)
        let scroll = try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first)
        try waitUntil { worker.listCount > 0 && (scroll.documentView?.subviews.count ?? 0) > 0 }
        let content = try XCTUnwrap(scroll.documentView)
        XCTAssertEqual(buttons(content, title: "Recover").count, 1,
                       "unavailable rows have no destructive actions")
        XCTAssertEqual(buttons(content, title: "Discard…").count, 1)
        XCTAssertFalse(try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first)
            .isHidden, "History remains independent")

        buttons(content, title: "Discard…")[0].performClick(nil)
        try waitUntil { window.attachedSheet != nil }
        XCTAssertEqual(worker.discardCount, 0)
        window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertSecondButtonReturn)
        try waitUntil { window.attachedSheet == nil }
        XCTAssertEqual(worker.discardCount, 0)
        buttons(try XCTUnwrap(scroll.documentView), title: "Recover")[0].performClick(nil)
        XCTAssertEqual(worker.recoverCount, 1)
        XCTAssertFalse(controller.prepareEditorForTermination())
        worker.completeRecover(.failure(AppBridgeError.backend("The bundle changed.")))
        try waitUntil { controller.prepareEditorForTermination() }
        XCTAssertTrue(try XCTUnwrap(scroll.documentView).subviews.compactMap { ($0 as? NSTextField)?.stringValue }
            .contains { $0.contains("The bundle changed.") })
        buttons(try XCTUnwrap(scroll.documentView), title: "Recover")[0].performClick(nil)
        XCTAssertEqual(worker.recoverCount, 2, "ordinary failure retains the recovery action")
        let cancel = try XCTUnwrap(buttons(panel, title: "Cancel").first)
        cancel.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(worker.cancel).isCancelled)
        worker.completeRecover(.success(RecordingRecoveryResult(artifactID: "missing-artifact", warning: nil)))
        try waitUntil { controller.prepareEditorForTermination() }
        XCTAssertEqual(worker.recoveredIdentity, "original-identity")
        buttons(try XCTUnwrap(scroll.documentView), title: "Discard…")[0].performClick(nil)
        try waitUntil { window.attachedSheet != nil }
        window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertFirstButtonReturn)
        try waitUntil { worker.discardCount == 1 && window.attachedSheet == nil }
        XCTAssertEqual(worker.discardedIdentity, "original-identity")
    }

    func testMinimumLightDarkErrorAndUnavailableRowsRender() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: folder) }
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame); window.contentView = root
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            root.wantsLayer = true; root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
            window.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
            let worker = RecoveryFixtureWorker(drafts: [
                try draft("recoverable", identity: "identity", kind: "video"),
                try draft("unavailable", identity: nil, kind: nil,
                    reason: "The recording manifest is corrupt and cannot be recovered or discarded automatically."),
            ])
            let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
                historyRoot: folder.path, settingsPath: nil, transport: EmptyHistoryTransport(),
                recoveryWorker: worker, showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            let panel = try recoveryPanel(root)
            try waitUntil { worker.listCount > 0 && !panel.isHidden }
            XCTAssertEqual(panel.frame, NSRect(x: 28, y: 194, width: 320, height: 152))
            XCTAssertTrue(root.bounds.contains(panel.frame))
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                let path = URL(fileURLWithPath: output).appendingPathComponent("recording-recovery-\(appearance).png")
                try FileManager.default.createDirectory(at: path.deletingLastPathComponent(),
                                                        withIntermediateDirectories: true)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: path)
            }
            let longError = "Recovery root is temporarily unavailable. "
                + String(repeating: "The bundle must remain on disk for manual inspection. ", count: 5)
            worker.listError = AppBridgeError.backend(longError)
            try XCTUnwrap(buttons(root, title: "Refresh").first).performClick(nil)
            try waitUntil { tryRecoveryError(panel).contains("Recovery root is temporarily unavailable.") }
            let errorContent = try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView)
            let errorField = try XCTUnwrap(errorContent.subviews.compactMap { $0 as? NSTextField }
                .first { $0.stringValue.contains("Recovery root is temporarily unavailable.") })
            XCTAssertGreaterThan(errorField.frame.height, 49)
            XCTAssertEqual(errorField.toolTip, "Couldn’t list interrupted recordings: \(longError)")
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                let path = URL(fileURLWithPath: output)
                    .appendingPathComponent("recording-recovery-error-\(appearance).png")
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: path)
            }
            worker.listError = nil
            let content = try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView)
            try XCTUnwrap(buttons(content, title: "Retry list").first).performClick(nil)
            try waitUntil { !tryRecoveryError(panel).contains("Recovery root is temporarily unavailable.") }
            XCTAssertEqual(buttons(try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView),
                                   title: "Recover").count, 1)
        }
    }

    func testRecoveredHistoryArtifactIsSelectedAndOpensRealEditor() throws {
        _ = NSApplication.shared
        guard let tools = try? NativeMediaTools.locate() else {
            throw XCTSkip("ffmpeg and ffprobe are required")
        }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let worker = RecoveryFixtureWorker(drafts: [try draft("recoverable", identity: "accepted", kind: "video")])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: folder.path, settingsPath: nil, transport: EmptyHistoryTransport(),
            recoveryWorker: worker, showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let panel = try recoveryPanel(root)
        try waitUntil { (panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView?.subviews.count ?? 0) > 0 }
        let scroll = try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first)
        buttons(try XCTUnwrap(scroll.documentView), title: "Recover")[0].performClick(nil)
        let id = UUID().uuidString.lowercased()
        let directory = folder.appendingPathComponent(id)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let media = directory.appendingPathComponent("media.mp4")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tools.ffmpeg)
        process.arguments = ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                             "testsrc2=size=160x90:rate=12:duration=2", "-pix_fmt", "yuv420p", "-y", media.path]
        try process.run(); process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)
        let image = PreviewView.fixtureImage(scale: 1)
        try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]))
            .write(to: directory.appendingPathComponent("preview.png"))
        let metadata: [String: Any] = ["id": id, "kind": "video", "width": 160, "height": 90,
            "size_bytes": (try FileManager.default.attributesOfItem(atPath: media.path)[.size] as? NSNumber)?.intValue ?? 0,
            "created_at": "2026-09-22T00:00:00Z", "mode": "display", "mime_type": "video/mp4",
            "duration_ms": 2_000, "preview_url": "capture-history://\(id)/preview",
            "full_url": "capture-history://\(id)/full",
            "target": ["type": "display", "display_id": "fixture"]]
        try JSONSerialization.data(withJSONObject: metadata)
            .write(to: directory.appendingPathComponent("metadata.json"))
        worker.completeRecover(.success(RecordingRecoveryResult(artifactID: id, warning: nil)))
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? NSTableView)
        try waitUntil { table.numberOfRows == 1 && table.selectedRow == 0 }
        try waitUntil { NSApp.windows.contains { $0.title == "Recording editor" && $0.isVisible } }
        XCTAssertTrue(controller.prepareEditorForTermination())
        NSApp.windows.filter { $0.title == "Recording editor" }.forEach { $0.orderOut(nil) }
    }

    func testRecoveryDoesNotStealSelectionChangedDuringWork() throws {
        _ = NSApplication.shared
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let image = PreviewView.fixtureImage(scale: 1)
        let path = folder.appendingPathComponent("poster.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]))
            .write(to: path)
        let artifacts: [[String: Any]] = [
            ["entry": ["id": "newest", "kind": "screenshot", "width": image.width,
                       "height": image.height, "created_at": "2026-09-23T00:00:00Z"],
             "image_path": path.path, "preview_path": path.path],
            ["entry": ["id": "older", "kind": "screenshot", "width": image.width,
                       "height": image.height, "created_at": "2026-09-22T00:00:00Z"],
             "image_path": path.path, "preview_path": path.path],
        ]
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let worker = RecoveryFixtureWorker(drafts: [try draft("recoverable", identity: "accepted", kind: "video")])
        let controller = LiveCaptureController(root: root, window: window,
            tokens: try XCTUnwrap(Tokens.variants["light-mustard"]), historyRoot: folder.path,
            settingsPath: nil, transport: EmptyHistoryTransport(artifacts: artifacts),
            recoveryWorker: worker, showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? NSTableView)
        try waitUntil { table.numberOfRows == 2 && table.selectedRow == 0 }
        let panel = try recoveryPanel(root)
        let scroll = try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first)
        buttons(try XCTUnwrap(scroll.documentView), title: "Recover")[0].performClick(nil)
        table.selectRowIndexes([1], byExtendingSelection: false)
        XCTAssertEqual(table.selectedRow, 1)
        worker.completeRecover(.success(RecordingRecoveryResult(artifactID: "newest", warning: nil)))
        try waitUntil { controller.prepareEditorForTermination() }
        XCTAssertEqual(table.selectedRow, 1,
                       "recovery must not reselect its artifact after the user changes selection")
    }

    private func tryRecoveryError(_ panel: Surface) -> String {
        panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView?.subviews
            .compactMap { ($0 as? NSTextField)?.stringValue }.joined(separator: " ") ?? ""
    }

    private func draft(_ status: String, identity: String?, kind: String?, reason: String? = nil) throws
        -> RecordingRecoveryDraft {
        try XCTUnwrap(RecordingRecoveryDraft(["session_id": UUID().uuidString,
            "status": status, "identity": identity.map { $0 as Any } ?? NSNull(),
            "kind": kind.map { $0 as Any } ?? NSNull(),
            "created_at_ms": NSNumber(value: 1_780_000_000_000 as UInt64),
            "completed_duration_ms": 2_500,
            "reason": reason.map { $0 as Any } ?? NSNull()]))
    }

    private func recoveryPanel(_ root: Surface) throws -> Surface {
        try XCTUnwrap(root.subviews.compactMap { $0 as? Surface }.first { panel in
            panel.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
                .contains("Interrupted recordings")
        })
    }

    private func buttons(_ view: NSView, title: String) -> [CaptureButton] {
        view.subviews.compactMap { $0 as? CaptureButton }.filter { $0.title == title }
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        XCTAssertTrue(condition())
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}

private final class EmptyHistoryTransport: AppTransport {
    let artifacts: [[String: Any]]
    init(artifacts: [[String: Any]] = []) { self.artifacts = artifacts }
    func request(_ object: [String: Any]) throws -> [String: Any] {
        switch object["operation"] as? String {
        case "history": return ["artifacts": artifacts]
        case "displays": return ["displays": []]
        default: throw AppBridgeError.invalidResponse
        }
    }
}

private final class RecoveryFixtureWorker: RecordingRecoveryWorking {
    var drafts: [RecordingRecoveryDraft]
    var listCount = 0
    var recoverCount = 0
    var discardCount = 0
    var recoveredIdentity: String?
    var discardedIdentity: String?
    var listError: Error?
    var cancel: NativeRecordingEditorCancel?
    private var pendingRecover: ((Result<RecordingRecoveryResult, Error>) -> Void)?
    init(drafts: [RecordingRecoveryDraft]) { self.drafts = drafts }
    func list(historyRoot: String, completion: @escaping (Result<[RecordingRecoveryDraft], Error>) -> Void) {
        listCount += 1
        if let listError { completion(.failure(listError)) }
        else { completion(.success(drafts)) }
    }
    func recover(historyRoot: String, draft: RecordingRecoveryDraft, cancel: NativeRecordingEditorCancel,
                 progress: @escaping (String) -> Void,
                 completion: @escaping (Result<RecordingRecoveryResult, Error>) -> Void) {
        recoverCount += 1; recoveredIdentity = draft.identity; self.cancel = cancel
        progress("assembling"); pendingRecover = completion
    }
    func completeRecover(_ result: Result<RecordingRecoveryResult, Error>) {
        let completion = pendingRecover; pendingRecover = nil; completion?(result)
    }
    func discard(historyRoot: String, draft: RecordingRecoveryDraft,
                 completion: @escaping (Result<Void, Error>) -> Void) {
        discardCount += 1; discardedIdentity = draft.identity; completion(.success(()))
    }
}
