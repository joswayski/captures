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
        let transport = EmptyHistoryTransport()
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: folder.path, settingsPath: nil, transport: transport,
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
        controller.openImages(["/unsupported-image.gif"])
        XCTAssertEqual(transport.openImageCount, 0, "confirmation must keep the image queue parked")
        window.endSheet(try XCTUnwrap(window.attachedSheet), returnCode: .alertSecondButtonReturn)
        try waitUntil { window.attachedSheet == nil }
        try waitUntil { transport.openImageCount == 1 && !controller.externalOpenPending }
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
        let listsBeforeSuccess = worker.listCount
        worker.completeRecover(.success(RecordingRecoveryResult(artifactID: "missing-artifact", warning: nil)))
        // Supersede the recovery-owned History load before its queued response.
        // Its stale selection/open callback must be suppressed, but cleanup must
        // still release the recovery busy/quit latch.
        controller.refreshHistory()
        try waitUntil { controller.prepareEditorForTermination() }
        try waitUntil { worker.listCount > listsBeforeSuccess }
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
                    reason: "The recording manifest is corrupt and cannot be recovered or discarded automatically. "
                        + String(repeating: "Inspect the retained bundle manually. ", count: 5)),
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
            let row = try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView)
            let reason = try XCTUnwrap(row.subviews.compactMap { $0 as? NSTextField }
                .first { $0.stringValue.contains("The recording manifest is corrupt") })
            XCTAssertGreaterThan(reason.frame.height, 49)
            XCTAssertEqual(reason.toolTip, reason.stringValue)
            XCTAssertEqual(reason.accessibilityHelp(), reason.stringValue)
            XCTAssertTrue(panel.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
                .contains("Scroll for full details."))
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                let path = URL(fileURLWithPath: output)
                    .appendingPathComponent("recording-recovery-\(appearance)-minimum.png")
                try FileManager.default.createDirectory(at: path.deletingLastPathComponent(),
                                                        withIntermediateDirectories: true)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: path)
                let scroll = try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first)
                scroll.contentView.scroll(to: NSPoint(x: 0, y: 96))
                scroll.reflectScrolledClipView(scroll.contentView)
                window.display()
                root.cacheDisplay(in: root.bounds, to: bitmap)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                    .write(to: URL(fileURLWithPath: output)
                        .appendingPathComponent("recording-recovery-unavailable-\(appearance)-minimum.png"))
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
            let retry = try XCTUnwrap(buttons(panel, title: "Retry list").first)
            XCTAssertFalse(retry.isHidden)
            XCTAssertTrue(retry.isEnabled)
            retry.performClick(nil)
            try waitUntil { !tryRecoveryError(panel).contains("Recovery root is temporarily unavailable.") }
            XCTAssertEqual(buttons(try XCTUnwrap(panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView),
                                   title: "Recover").count, 1)
        }
    }

    func testNormalLightDarkRecoveryRowsRender() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: folder) }
            let frame = NSRect(x: 0, y: 0, width: 1280, height: 800)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame); window.contentView = root
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            root.wantsLayer = true; root.layer?.backgroundColor = tokens.color("surface-canvas").cgColor
            window.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
            let worker = RecoveryFixtureWorker(drafts: [try draft("recoverable", identity: "identity", kind: "gif")])
            let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
                historyRoot: folder.path, settingsPath: nil, transport: EmptyHistoryTransport(),
                recoveryWorker: worker, showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            let panel = try recoveryPanel(root)
            try waitUntil { worker.listCount > 0 && !panel.isHidden }
            XCTAssertTrue(root.bounds.contains(panel.frame))
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                let path = URL(fileURLWithPath: output)
                    .appendingPathComponent("recording-recovery-\(appearance)-normal.png")
                try FileManager.default.createDirectory(at: path.deletingLastPathComponent(),
                                                        withIntermediateDirectories: true)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: path)
            }
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
        let settingsPath = folder.appendingPathComponent("settings.json").path
        let settingsBridge = SettingsBridge()
        var settings = try XCTUnwrap(settingsBridge.request([
            "operation": "load", "path": settingsPath])["settings"] as? [String: Any])
        settings["output_directory"] = folder.path
        _ = try settingsBridge.request(["operation": "save", "path": settingsPath, "settings": settings])
        let initialHistoryGate = DispatchSemaphore(value: 0)
        defer { initialHistoryGate.signal() }
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let worker = RecoveryFixtureWorker(drafts: [try draft("recoverable", identity: "accepted", kind: "video")])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: folder.path, settingsPath: settingsPath,
            transport: EmptyHistoryTransport(initialHistoryGate: initialHistoryGate),
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
        // The initial History response may arrive after Recover was clicked.
        // Its programmatic selection must not count as user selection intent.
        initialHistoryGate.signal()
        let table = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? NSTableView)
        try waitUntil { table.numberOfRows == 1 && table.selectedRow == 0 }
        XCTAssertEqual(worker.recoverCount, 1)
        worker.completeRecover(.success(RecordingRecoveryResult(artifactID: id, warning: nil)))
        try waitUntil { table.numberOfRows == 1 && table.selectedRow == 0 }
        try waitUntil { NSApp.windows.contains { $0.title == "Recording editor" && $0.isVisible } }
        let editor = try XCTUnwrap(NSApp.windows.first { $0.title == "Recording editor" && $0.isVisible })
        try waitUntil { editor.contentView.map { descendants($0).compactMap { $0 as? NSImageView }
            .contains { $0.accessibilityLabel() == "Decoded recording frame" && $0.image != nil } } == true }
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

    func testRealRecoveryBridgePublishesAndOpensSameHistoryRecording() throws {
        guard let tools = try? NativeMediaTools.locate() else {
            throw XCTSkip("ffmpeg and ffprobe are required")
        }
        let base = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let history = base.appendingPathComponent("history")
        let id = UUID().uuidString.lowercased()
        let bundle = base.appendingPathComponent("recording-recovery").appendingPathComponent(id)
        try FileManager.default.createDirectory(at: history, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let segment = bundle.appendingPathComponent("segment-000.mp4")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tools.ffmpeg)
        process.arguments = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
                             "color=c=red:size=64x48:rate=10:duration=1", "-c:v", "mpeg4", segment.path]
        try process.run(); process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)
        let size = (try FileManager.default.attributesOfItem(atPath: segment.path)[.size] as? NSNumber)?.intValue ?? 0
        let manifest: [String: Any] = ["schema_version": 1, "session_id": id,
            "created_at_ms": 1_780_000_000_000 as UInt64, "updated_at_ms": 1_780_000_000_000 as UInt64,
            "state": "failed", "options": ["kind": "video", "target": ["type": "display", "display_id": "fixture"],
                "frames_per_second": 15, "max_resolution": "original", "countdown_seconds": 0,
                "show_cursor": false],
            "segments": [["index": 0, "relative_path": "segment-000.mp4", "started_at_ms": 0,
                "duration_ms": 1_000, "width": 64, "height": 48, "size_bytes": size,
                "dropped_frames": 0, "complete": true]]]
        try JSONSerialization.data(withJSONObject: manifest)
            .write(to: bundle.appendingPathComponent("manifest.json"))
        let worker = RecordingRecoveryWorker()
        var listed: Result<[RecordingRecoveryDraft], Error>?
        worker.list(historyRoot: history.path) { listed = $0 }
        try waitUntil { listed != nil }
        let draft = try XCTUnwrap(try listed!.get().first)
        XCTAssertEqual(draft.sessionID, id)
        XCTAssertEqual(draft.status, "recoverable")
        let cancel = try XCTUnwrap(NativeRecordingEditorCancel())
        var result: Result<RecordingRecoveryResult, Error>?
        var stages: [String] = []
        worker.recover(historyRoot: history.path, draft: draft, cancel: cancel,
                       progress: { stages.append($0) }, completion: { result = $0 })
        try waitUntil { result != nil }
        let recovered = try result!.get()
        XCTAssertTrue(stages.contains("publishing"))
        let artifacts = try XCTUnwrap(NativeRecordingInfo.request([
            "operation": "history", "root": history.path])["recordings"] as? [[String: Any]])
        XCTAssertTrue(artifacts.contains {
            ($0["entry"] as? [String: Any])?["id"] as? String == recovered.artifactID
        })
        let (session, opened) = try NativeRecordingEditorSession.open(
            historyRoot: history.path, artifactID: recovered.artifactID, tools: tools)
        XCTAssertEqual(opened.snapshot.artifactID, recovered.artifactID)
        XCTAssertNotNil(try session.request(["operation": "seek", "position_ms": 200]).image.dataProvider?.data)
    }

    func testNativeSessionKeepsRecoveryLeaseUntilOwnerIsRetired() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let history = base.appendingPathComponent("history")
        let recoveryRoot = base.appendingPathComponent("recording-recovery")
        try FileManager.default.createDirectory(at: history, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: base) }
        let options: [String: Any] = ["kind": "video", "target": ["type": "display", "display_id": "fixture"],
                                      "frames_per_second": 15, "max_resolution": "original",
                                      "countdown_seconds": 0, "show_cursor": false]
        let display: [String: Any] = ["id": "fixture", "name": "Fixture", "x": 0, "y": 0,
                                      "width": 640, "height": 360, "scale_factor": 1.0, "is_primary": true]
        let id: String
        var session: NativeRecordingSession?
        do {
            let prepared = try NativeRecordingSession.prepare(
                recoveryRoot: recoveryRoot.path, options: options, display: display)
            id = prepared.1.id; session = prepared.0
        }
        let worker = RecordingRecoveryWorker()
        var result: Result<[RecordingRecoveryDraft], Error>?
        worker.list(historyRoot: history.path) { result = $0 }
        try waitUntil { result != nil }
        XCTAssertThrowsError(try result!.get(), "a live handle owns the lease even while idle")
        // Force the terminal discard persistence step to fail. Successful
        // discard intentionally releases the lease; a failed owner retains it.
        let manifest = recoveryRoot.appendingPathComponent(id).appendingPathComponent("manifest.json")
        try FileManager.default.removeItem(at: manifest)
        try FileManager.default.createDirectory(at: manifest, withIntermediateDirectories: true)
        try Data("keep".utf8).write(to: manifest.appendingPathComponent("marker"))
        XCTAssertThrowsError(try session?.discard())
        XCTAssertEqual(try session?.snapshot().state, "discarded")
        result = nil
        worker.list(historyRoot: history.path) { result = $0 }
        try waitUntil { result != nil }
        XCTAssertThrowsError(try result!.get(), "terminal state still owns the lease")
        session = nil
        result = nil
        worker.list(historyRoot: history.path) { result = $0 }
        try waitUntil { result != nil }
        XCTAssertEqual(try result!.get().first?.status, "unavailable")
    }

    private func tryRecoveryError(_ panel: Surface) -> String {
        panel.subviews.compactMap { $0 as? NSScrollView }.first?.documentView?.subviews
            .compactMap { ($0 as? NSTextField)?.stringValue }.joined(separator: " ") ?? ""
    }

    private func descendants(_ view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap(descendants)
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
    private let lock = NSLock()
    private var opens = 0
    var openImageCount: Int { lock.lock(); defer { lock.unlock() }; return opens }
    private var initialHistoryGate: DispatchSemaphore?
    init(artifacts: [[String: Any]] = [], initialHistoryGate: DispatchSemaphore? = nil) {
        self.artifacts = artifacts; self.initialHistoryGate = initialHistoryGate
    }
    func request(_ object: [String: Any]) throws -> [String: Any] {
        switch object["operation"] as? String {
        case "history":
            if let gate = initialHistoryGate {
                initialHistoryGate = nil
                gate.wait()
            }
            return ["artifacts": artifacts]
        case "displays": return ["displays": []]
        case "open_image":
            lock.lock(); opens += 1; lock.unlock()
            throw AppBridgeError.backend("unsupported image fixture")
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
