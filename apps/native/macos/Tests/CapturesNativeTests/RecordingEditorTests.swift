import AppKit
import ImageIO
import XCTest
@testable import CapturesNative

final class RecordingEditorTests: XCTestCase {
    func testStagedTrimFormatFailureRetainsAcceptedFrameAndValues() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: presentation())
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let start = try field("Trim start milliseconds", in: controller.root)
        let end = try field("Trim end milliseconds", in: controller.root)
        start.stringValue = "250"; end.stringValue = "1250"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        let format = try popup("Recording export format", in: controller.root)
        format.selectItem(withTitle: "GIF"); _ = format.sendAction(format.action, to: format.target)
        XCTAssertFalse(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Save new copy", in: controller.root).isEnabled)
        worker.requestResult = .failure(AppBridgeError.backend("preview unavailable"))
        try button("Apply edits", in: controller.root).performClick(nil)
        let request = try XCTUnwrap(worker.requests.last)
        XCTAssertEqual(request["operation"] as? String, "update_preview")
        let edit = try XCTUnwrap(request["edit"] as? [String: Any])
        XCTAssertEqual((edit["trim_start_ms"] as? NSNumber)?.uint64Value, 250)
        XCTAssertEqual((edit["trim_end_ms"] as? NSNumber)?.uint64Value, 1250)
        XCTAssertNotNil(edit["audio"], "the host preserves shared edit fields it does not own")
        XCTAssertEqual((request["export"] as? [String: Any])?["format"] as? String, "gif")
        XCTAssertEqual(start.stringValue, "250"); XCTAssertEqual(end.stringValue, "1250")
        XCTAssertEqual(format.titleOfSelectedItem, "GIF")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("preview unavailable") })
        XCTAssertFalse(controller.prepareForTermination(), "staged recording edits have no draft")
    }

    func testAcceptedSeekEstimateSaveWarningAndDirtyLifecycle() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: presentation())
        var historyReloads = 0
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
            worker: worker, didSaveCopy: { historyReloads += 1 }, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let start = try field("Trim start milliseconds", in: controller.root)
        start.stringValue = "100"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        let accepted = presentation(start: 100, end: nil, position: 0, revision: 1)
        worker.requestResult = .success(accepted)
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertFalse(controller.prepareForTermination(), "accepted edits remain dirty until save")

        let seek = try slider("Recording frame position", in: controller.root)
        seek.doubleValue = 700
        worker.requestResult = .success(presentation(start: 100, end: nil, position: 700, revision: 2))
        _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertEqual(worker.requests.last?["operation"] as? String, "seek")
        XCTAssertEqual((worker.requests.last?["position_ms"] as? NSNumber)?.uint64Value, 700)

        worker.estimateResult = .success(RecordingEditorEstimate(sizeBytes: 1_500_000, exact: false))
        try button("Estimate size", in: controller.root).performClick(nil)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("≈") && $0.contains("MB") })
        worker.saveResult = .success(.savedWithoutHistory(path: "/Exports/edited.mp4",
                                                          warning: "History disk unavailable"))
        try button("Save new copy", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.saves.first?.destination, "/Exports/recording-edit-recordin.mp4")
        XCTAssertEqual(historyReloads, 0, "post-publication warning must not claim a History update")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("edited.mp4") })
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("History disk unavailable") })
        XCTAssertTrue(controller.prepareForTermination(), "the published path resolves dirty state")
        XCTAssertEqual(worker.closeCount, 1)
    }

    func testBusyCloseQuitCancellationAndRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeRecordingEditorWorker(presentation: presentation())
            let controller = RecordingEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                                                       worker: worker, confirmDiscard: { false })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                               outputDirectory: "/Exports")
            try render(controller.root, name: "recording-editor-normal-\(appearance)")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            XCTAssertTrue(controller.root.subviews.allSatisfy {
                $0.isHidden || controller.root.bounds.intersects($0.frame)
            })
            try render(controller.root, name: "recording-editor-minimum-\(appearance)")

            worker.deferSave = true
            try button("Save new copy", in: controller.root).performClick(nil)
            XCTAssertFalse(controller.windowShouldClose(controller.window))
            XCTAssertFalse(controller.prepareForTermination())
            let cancel = try button("Cancel operation", in: controller.root)
            XCTAssertFalse(cancel.isHiddenOrHasHiddenAncestor); cancel.performClick(nil)
            worker.completeSave(.failure(AppBridgeError.backend("Export cancelled")))
            XCTAssertTrue(labels(in: controller.root).contains { $0.contains("Export cancelled") })
            try render(controller.root, name: "recording-editor-error-\(appearance)")
        }
    }

    func testRealBridgeSeekTrimMp4GifCollisionAndImmutableOriginal() throws {
        let tools = try NativeMediaTools.locate()
        let fixture = try makeRecordingFixture(tools: tools)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let sourceBefore = try Data(contentsOf: fixture.source)
        let opened = try NativeRecordingEditorSession.open(historyRoot: fixture.history.path,
                                                            artifactID: fixture.id, tools: tools)
        let session = opened.0
        XCTAssertEqual(opened.1.snapshot.positionMilliseconds, 0)
        let sought = try session.request(["operation": "seek", "position_ms": 700])
        XCTAssertEqual(sought.snapshot.positionMilliseconds, 700)
        var edit = sought.snapshot.edit
        edit["trim_start_ms"] = 200; edit["trim_end_ms"] = 1_200
        var mp4 = sought.snapshot.export; mp4["format"] = "mp4"; mp4["quality"] = "standard"
        mp4["max_size_bytes"] = NSNull()
        let accepted = try session.request(["operation": "update_preview", "edit": edit,
                                            "export": mp4])
        XCTAssertEqual((accepted.snapshot.edit["trim_start_ms"] as? NSNumber)?.uint64Value, 200)
        let mp4Path = fixture.root.appendingPathComponent("trimmed.mp4").path
        let mp4Cancel = try XCTUnwrap(NativeRecordingEditorCancel())
        XCTAssertEqual(try session.save(destination: mp4Path, export: mp4,
                                        cancel: mp4Cancel, progress: { _ in }),
                       .saved(path: mp4Path))
        XCTAssertThrowsError(try session.save(destination: mp4Path, export: mp4,
                                               cancel: try XCTUnwrap(NativeRecordingEditorCancel()),
                                               progress: { _ in }))
        var gif = mp4; gif["format"] = "gif"
        _ = try session.request(["operation": "update_preview", "edit": edit, "export": gif])
        let gifPath = fixture.root.appendingPathComponent("trimmed.gif").path
        _ = try session.save(destination: gifPath, export: gif,
                             cancel: try XCTUnwrap(NativeRecordingEditorCancel()), progress: { _ in })
        XCTAssertTrue(FileManager.default.fileExists(atPath: gifPath))
        XCTAssertEqual(try Data(contentsOf: fixture.source), sourceBefore,
                       "every edit/export keeps the original byte-identical")
    }

    private func presentation(start: UInt64 = 0, end: UInt64? = nil,
                              position: UInt64 = 0, revision: UInt64 = 0) -> RecordingEditorPresentation {
        let endValue: Any = end.map { NSNumber(value: $0) } ?? NSNull()
        let snapshot = NativeRecordingEditorSnapshot([
            "artifact_id": "recording-id", "source": ["kind": "video", "mime_type": "video/mp4",
                "width": 320, "height": 180, "duration_ms": 2_000, "size_bytes": 1_024],
            "edit": ["trim_start_ms": start, "trim_end_ms": endValue,
                "crop": NSNull(), "output_width": NSNull(), "output_height": NSNull(),
                "audio": ["system_volume": 1.0, "microphone_volume": 1.0,
                          "mute_system_audio": false, "mute_microphone": false,
                          "mono_output": false, "source_has_system_audio": false,
                          "source_has_microphone_audio": false]],
            "preview_export": ["format": "mp4", "quality": "preserve",
                "max_size_bytes": NSNull(), "frames_per_second": NSNull(),
                "gif_max_colors": NSNull()],
            "position_ms": position, "revision": revision,
        ])!
        return RecordingEditorPresentation(snapshot: snapshot, image: fixtureImage())
    }

    private func recordingArtifact() -> CaptureArtifact {
        CaptureArtifact(["entry": ["id": "recording-id", "kind": "video", "width": 320,
            "height": 180, "created_at": "2026-09-22T00:00:00Z"],
            "preview_path": "/History/recording-id/preview.png",
            "media_path": "/History/recording-id/media.mp4"])!
    }

    private func makeRecordingFixture(tools: NativeMediaTools) throws
        -> (root: URL, history: URL, source: URL, id: String) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let history = root.appendingPathComponent("History")
        let id = UUID().uuidString.lowercased()
        let directory = history.appendingPathComponent(id)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let source = directory.appendingPathComponent("media.mp4")
        let process = Process(); process.executableURL = URL(fileURLWithPath: tools.ffmpeg)
        process.arguments = ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                             "testsrc2=size=160x90:rate=12:duration=2", "-pix_fmt", "yuv420p",
                             "-y", source.path]
        try process.run(); process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)
        let sourceSize = (try FileManager.default.attributesOfItem(atPath: source.path)[.size]
                          as? NSNumber)?.intValue ?? 0
        let metadata: [String: Any] = ["id": id, "kind": "video",
            "preview_url": "capture-history://\(id)/preview",
            "full_url": "capture-history://\(id)/full", "width": 160, "height": 90,
            "size_bytes": sourceSize,
            "created_at": "2026-09-22T00:00:00Z", "mode": "display",
            "mime_type": "video/mp4", "duration_ms": 2_000,
            "target": ["type": "display", "display_id": "fixture"]]
        try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys])
            .write(to: directory.appendingPathComponent("metadata.json"))
        return (root, history, source, id)
    }

    private func fixtureImage() -> CGImage {
        let bytes = [UInt8](repeating: 80, count: 16 * 9 * 4)
        let provider = CGDataProvider(data: Data(bytes) as CFData)!
        return CGImage(width: 16, height: 9, bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: 64, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: false,
                       intent: .defaultIntent)!
    }

    private func descendants(in view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap { descendants(in: $0) }
    }
    private func field(_ label: String, in view: NSView) throws -> NSTextField {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSTextField }
            .first { $0.accessibilityLabel() == label })
    }
    private func popup(_ label: String, in view: NSView) throws -> NSPopUpButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSPopUpButton }
            .first { $0.accessibilityLabel() == label })
    }
    private func slider(_ label: String, in view: NSView) throws -> NSSlider {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSSlider }
            .first { $0.accessibilityLabel() == label })
    }
    private func button(_ title: String, in view: NSView) throws -> CaptureButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? CaptureButton }.first { $0.title == title })
    }
    private func labels(in view: NSView) -> [String] {
        descendants(in: view).compactMap { ($0 as? NSTextField)?.stringValue }
    }
    private func render(_ view: NSView, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_NATIVE_PIXEL_DIR"] else { return }
        view.displayIfNeeded()
        let rep = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: rep)
        let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(name).png"))
    }
}

private final class FakeRecordingEditorWorker: RecordingEditorWorking {
    var initial: RecordingEditorPresentation
    var requests: [[String: Any]] = []
    var requestResult: Result<RecordingEditorPresentation, Error>?
    var estimateResult: Result<RecordingEditorEstimate, Error> = .failure(AppBridgeError.backend("estimate unavailable"))
    var saveResult: Result<RecordingEditorSaveResult, Error> = .failure(AppBridgeError.backend("save unavailable"))
    var saves: [(destination: String, export: [String: Any])] = []
    var deferSave = false
    var closeCount = 0
    private var pendingSave: ((Result<RecordingEditorSaveResult, Error>) -> Void)?

    init(presentation: RecordingEditorPresentation) { initial = presentation }
    func open(historyRoot: String, artifactID: String,
              completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void) {
        completion(.success(initial))
    }
    func request(_ object: [String: Any],
                 completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void) {
        requests.append(object); completion(requestResult ?? .success(initial))
    }
    func estimate(cancel: NativeRecordingEditorCancel,
                  completion: @escaping (Result<RecordingEditorEstimate, Error>) -> Void) {
        completion(estimateResult)
    }
    func save(destination: String, export: [String: Any], cancel: NativeRecordingEditorCancel,
              progress: @escaping (RecordingEditorProgress) -> Void,
              completion: @escaping (Result<RecordingEditorSaveResult, Error>) -> Void) {
        saves.append((destination, export))
        if deferSave { pendingSave = completion }
        else { completion(saveResult) }
    }
    func completeSave(_ result: Result<RecordingEditorSaveResult, Error>) {
        let completion = pendingSave; pendingSave = nil; completion?(result)
    }
    func close() { closeCount += 1 }
}
