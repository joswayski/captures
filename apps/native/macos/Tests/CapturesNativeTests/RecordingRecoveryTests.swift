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
        let cancel = try XCTUnwrap(buttons(panel, title: "Cancel").first)
        cancel.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(worker.cancel).isCancelled)
        worker.completeRecover(.success(RecordingRecoveryResult(artifactID: "missing-artifact", warning: nil)))
        try waitUntil { controller.prepareEditorForTermination() }
        XCTAssertEqual(worker.recoveredIdentity, "original-identity")
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
        }
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
    func request(_ object: [String: Any]) throws -> [String: Any] {
        switch object["operation"] as? String {
        case "history": return ["artifacts": []]
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
    var cancel: NativeRecordingEditorCancel?
    private var pendingRecover: ((Result<RecordingRecoveryResult, Error>) -> Void)?
    init(drafts: [RecordingRecoveryDraft]) { self.drafts = drafts }
    func list(historyRoot: String, completion: @escaping (Result<[RecordingRecoveryDraft], Error>) -> Void) {
        listCount += 1; completion(.success(drafts))
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
        discardCount += 1; completion(.success(()))
    }
}
