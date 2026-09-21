import AppKit
import XCTest
@testable import CapturesNative

final class ScreenshotEditorTests: XCTestCase {
    func testViewportBridgeKeepsAsymmetricAnchorAfterPanAndRejectsInvalidInput() throws {
        let fit = CGRect(x: 17, y: 29, width: 503, height: 251.5)
        let canvas = CGSize(width: 1600, height: 800)
        let anchor = CGPoint(x: 411, y: 87)
        let panned = NativeEditorViewport(zoomPercent: 175, panX: -83, panY: 41)
        let before = try XCTUnwrap(panned.rect(fit: fit, canvas: canvas))
        let document = CGPoint(x: (anchor.x - before.minX) * canvas.width / before.width,
                               y: (anchor.y - before.minY) * canvas.height / before.height)
        let zoomed = try XCTUnwrap(panned.zoomed(to: 287.5, anchor: anchor,
                                                 fit: fit, canvas: canvas))
        let after = try XCTUnwrap(zoomed.rect(fit: fit, canvas: canvas))
        XCTAssertEqual(after.minX + document.x * after.width / canvas.width, anchor.x,
                       accuracy: 0.001)
        XCTAssertEqual(after.minY + document.y * after.height / canvas.height, anchor.y,
                       accuracy: 0.001)
        XCTAssertNil(NativeEditorViewport.wheelFactor(deltaPixels: .nan))
        XCTAssertNil(NativeEditorViewport().rect(fit: fit, canvas: .zero))
    }

    func testViewportControlsAreAccessibleAndDoNotMutateEditorState() throws {
        _ = NSApplication.shared
        let original = snapshot(id: "shot", unsaved: true, draft: true)
        let worker = FakeEditorWorker(snapshot: original)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        let labels = ["Fit screenshot in viewport", "Show screenshot at 100 percent",
                      "Zoom out", "Zoom in", "Recenter screenshot"]
        let controls = descendants(in: controller.root).compactMap { $0 as? CaptureButton }
            .filter { labels.contains($0.accessibilityLabel() ?? "") }
        XCTAssertEqual(controls.count, 5)
        controls.first { $0.accessibilityLabel() == "Show screenshot at 100 percent" }?.performClick(nil)
        XCTAssertEqual(controller.viewport.zoomPercent, 100)
        controls.first { $0.accessibilityLabel() == "Zoom in" }?.performClick(nil)
        XCTAssertEqual(controller.viewport.zoomPercent, 125)
        controls.first { $0.accessibilityLabel() == "Recenter screenshot" }?.performClick(nil)
        XCTAssertEqual(controller.viewport.zoomPercent, 125)
        controls.first { $0.accessibilityLabel() == "Fit screenshot in viewport" }?.performClick(nil)
        XCTAssertEqual(controller.viewport, NativeEditorViewport())
        XCTAssertEqual(controller.state.snapshot, original)
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertTrue(controls.allSatisfy { controller.root.bounds.contains($0.convert($0.bounds, to: controller.root)) })
    }

    func testViewportPanEventsUseReleasePointAndZoomCancelsDrawing() throws {
        _ = NSApplication.shared
        let original = snapshot(id: "shot", width: 640, height: 360, unsaved: false, draft: false)
        let worker = FakeEditorWorker(snapshot: original)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let overlay = controller.drawOverlay
        func event(_ type: NSEvent.EventType, _ point: CGPoint) -> NSEvent {
            NSEvent.mouseEvent(with: type, location: overlay.convert(point, to: nil),
                modifierFlags: [.command], timestamp: 0, windowNumber: controller.window.windowNumber,
                context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        }
        overlay.begin(at: CGPoint(x: 120, y: 140))
        XCTAssertNotNil(overlay.startPoint)
        overlay.mouseDown(with: event(.leftMouseDown, CGPoint(x: 120, y: 140)))
        XCTAssertNil(overlay.startPoint, "pan has priority over drawing")
        overlay.mouseDragged(with: event(.leftMouseDragged, CGPoint(x: 137, y: 129)))
        overlay.mouseUp(with: event(.leftMouseUp, CGPoint(x: 143, y: 133)))
        XCTAssertEqual(controller.viewport.panX, 23, accuracy: 1e-7)
        XCTAssertEqual(controller.viewport.panY, -7, accuracy: 1e-7)
        XCTAssertFalse(overlay.isViewportPanning)
        overlay.begin(at: CGPoint(x: 120, y: 140))
        try button("100%", in: controller.root).performClick(nil)
        XCTAssertNil(overlay.startPoint, "toolbar zoom cancels the original gesture")
        overlay.end(at: CGPoint(x: 240, y: 200))
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertEqual(controller.state.snapshot, original)
        XCTAssertTrue(overlay.superview?.layer?.masksToBounds == true,
                      "pixels and overlays clip to the same viewport, not the outer panel")
    }

    func testHighZoomUsesOneRectForDrawingAndSelectionAndCancelsGestures() {
        let rect = CGRect(x: -137, y: 53, width: 2400, height: 1200)
        let draw = EditorDrawOverlay(frame: CGRect(x: 0, y: 0, width: 604, height: 468))
        let selection = EditorSelectionOverlay(frame: draw.frame)
        draw.canvasSize = CGSize(width: 1600, height: 800)
        selection.canvasSize = draw.canvasSize
        draw.imageRect = { rect }; selection.imageRect = { rect }
        let viewPoint = CGPoint(x: 463, y: 353)
        XCTAssertEqual(draw.canvasPoint(for: viewPoint), selection.canvasPoint(for: viewPoint))
        XCTAssertEqual(draw.canvasPoint(for: viewPoint).x, 400, accuracy: 0.001)
        XCTAssertEqual(draw.canvasPoint(for: viewPoint).y, 200, accuracy: 0.001)
        draw.drawingEnabled = true; draw.begin(at: viewPoint)
        selection.selectionEnabled = true; selection.begin(at: viewPoint)
        draw.setFrameSize(NSSize(width: 500, height: 420))
        selection.setFrameSize(NSSize(width: 500, height: 420))
        XCTAssertNil(draw.startPoint); XCTAssertNil(selection.startPoint)
    }

    func testManualZoomAndPanRenderWithoutDraftOrPixelWork() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let original = snapshot(id: "shot", width: 1200, height: 500,
                                    unsaved: true, draft: true)
            let worker = FakeEditorWorker(snapshot: original)
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            let controls = descendants(in: controller.root).compactMap { $0 as? CaptureButton }
            controls.first { $0.accessibilityLabel() == "Show screenshot at 100 percent" }?.performClick(nil)
            let input = try XCTUnwrap(descendants(in: controller.root)
                .compactMap { $0 as? EditorViewportGestureView }
                .first { $0.accessibilityLabel() == "Screenshot viewport" })
            input.onViewportPan?(NSPoint(x: -117, y: 63))
            XCTAssertEqual(controller.viewport.panX, -117)
            XCTAssertEqual(controller.viewport.panY, 63)
            XCTAssertEqual(controller.state.snapshot, original)
            XCTAssertTrue(worker.requests.isEmpty)
            XCTAssertTrue(worker.encodes.isEmpty)
            try render(controller.root, name: "screenshot-editor-viewport-manual-\(appearance)")
        }
    }

    func testStateRejectsStaleOpenAndCommandCompletions() throws {
        var state = ScreenshotEditorState()
        let first = state.beginOpen(artifactID: "first")
        let second = state.beginOpen(artifactID: "second")
        XCTAssertFalse(state.complete(snapshot(id: "first"), generation: first))
        XCTAssertTrue(state.busy)
        XCTAssertTrue(state.complete(snapshot(id: "second"), generation: second))
        XCTAssertFalse(state.busy)
        let command = try XCTUnwrap(state.beginCommand())
        state.close()
        XCTAssertFalse(state.complete(snapshot(id: "second", unsaved: true), generation: command))
        XCTAssertNil(state.artifactID)
    }

    func testCloseWithoutSavingRetainsPersistedDraftAndDiscardIsExplicit() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: true))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        XCTAssertEqual(worker.draftsRoot, "/native/editor-drafts")

        XCTAssertFalse(controller.windowShouldClose(controller.window))
        let closeSheet = try XCTUnwrap(controller.window.attachedSheet)
        let closeTitles = try XCTUnwrap(closeSheet.contentView).subviews
            .flatMap { [$0] + descendants(in: $0) }
            .compactMap { ($0 as? NSButton)?.title }
        XCTAssertTrue(Set(["Save and Close", "Close Without Saving", "Cancel Close"])
            .isSubset(of: Set(closeTitles)))
        controller.window.endSheet(closeSheet, returnCode: .alertSecondButtonReturn)
        waitUntil { worker.closeCount == 1 }
        XCTAssertFalse(worker.requests.contains { $0["operation"] as? String == "discard_draft" })

        let discardWorker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: false, draft: true))
        let discardController = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: discardWorker)
        defer { discardController.window.orderOut(nil) }
        discardController.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try button("Discard edits…", in: discardController.root).performClick(nil)
        let discardSheet = try XCTUnwrap(discardController.window.attachedSheet)
        discardController.window.endSheet(discardSheet, returnCode: .alertFirstButtonReturn)
        waitUntil { discardWorker.requests.contains { $0["operation"] as? String == "discard_draft" } }
    }

    func testSaveFailureAndTerminationFailureKeepRecoverableWindow() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: false))
        worker.failOperation = "save_draft"
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try button("Save draft", in: controller.root).performClick(nil)
        waitUntil { !controller.state.busy }
        XCTAssertTrue(controller.state.snapshot?.unsavedChanges == true)
        XCTAssertTrue(controller.window.isVisible)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("fixture save failed") })

        XCTAssertFalse(controller.windowShouldClose(controller.window))
        let closeSheet = try XCTUnwrap(controller.window.attachedSheet)
        controller.window.endSheet(closeSheet, returnCode: .alertFirstButtonReturn)
        waitUntil { !controller.state.busy }
        XCTAssertTrue(controller.window.isVisible, "failed Save and Close keeps the draft recoverable")
        XCTAssertEqual(worker.closeCount, 0)

        worker.terminationResult = .failure(AppBridgeError.backend("fixture quit save failed"))
        XCTAssertFalse(controller.prepareForTermination())
        XCTAssertTrue(controller.window.isVisible)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("fixture quit save failed") })
        XCTAssertEqual(worker.closeCount, 0)
    }

    func testSaveAndClosePersistsBeforeFreeingSession() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: false))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")

        XCTAssertFalse(controller.windowShouldClose(controller.window))
        let closeSheet = try XCTUnwrap(controller.window.attachedSheet)
        controller.window.endSheet(closeSheet, returnCode: .alertFirstButtonReturn)
        waitUntil { worker.closeCount == 1 }

        XCTAssertEqual(worker.requests.count, 1)
        XCTAssertEqual(worker.requests[0]["operation"] as? String, "save_draft")
        XCTAssertNotNil(worker.requests[0]["updated_at_ms"] as? UInt64)
        XCTAssertNil(controller.state.artifactID)
        XCTAssertFalse(controller.window.isVisible)
    }

    func testPendingEditCannotBeReplacedAndDirtyCompletionRemainsRecoverable() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "first"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "first"), historyRoot: "/native/History")
        worker.deferRequests = true

        (try field("Crop X", in: controller.root)).stringValue = "13"
        (try field("Crop Y", in: controller.root)).stringValue = "7"
        (try field("Crop width", in: controller.root)).stringValue = "321"
        (try field("Crop height", in: controller.root)).stringValue = "199"
        try button("Apply crop", in: controller.root).performClick(nil)
        XCTAssertTrue(controller.state.busy)

        controller.present(artifact: artifact(id: "second"), historyRoot: "/native/History")
        XCTAssertEqual(worker.openArtifactIDs, ["first"], "a pending accepted edit must own the session")
        XCTAssertEqual(controller.state.artifactID, "first")

        worker.completePending(with: snapshot(id: "first", width: 321, height: 199,
                                              unsaved: true, draft: false))
        waitUntil { !controller.state.busy }
        XCTAssertEqual(controller.state.artifactID, "first")
        XCTAssertTrue(controller.state.snapshot?.unsavedChanges == true)
        XCTAssertTrue(controller.window.title.contains("Unsaved"))
        XCTAssertTrue(try button("Save draft", in: controller.root).isEnabled)
    }

    func testGeometryParsingAndFormattingUseTheSameCommaDecimalLocale() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", width: 640.5, height: 360.25))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
            worker: worker, numberLocale: Locale(identifier: "fr_FR"))
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        XCTAssertEqual((try field("Canvas width", in: controller.root)).stringValue, "640,5")
        XCTAssertEqual((try field("Canvas height", in: controller.root)).stringValue, "360,25")

        (try field("Crop X", in: controller.root)).stringValue = "1,5"
        (try field("Crop Y", in: controller.root)).stringValue = "2,25"
        (try field("Crop width", in: controller.root)).stringValue = "300,75"
        (try field("Crop height", in: controller.root)).stringValue = "150,5"
        try button("Apply crop", in: controller.root).performClick(nil)

        let rect = try XCTUnwrap(worker.requests.last?["rect"] as? [String: Double])
        XCTAssertEqual(rect, ["x": 1.5, "y": 2.25, "width": 300.75, "height": 150.5])
    }

    func testOutputPreviewUsesShippingOptionsAndInvalidatesAfterEditsAndOptions() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                     worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        let format = try popup("Output format", in: controller.root)
        let quality = try popup("Output quality mode", in: controller.root)
        let qualityValue = try field("Output quality value", in: controller.root)
        let palette = try field("PNG maximum colors", in: controller.root)
        let budget = try field("Output byte budget", in: controller.root)
        XCTAssertTrue(qualityValue.isHidden); XCTAssertTrue(palette.isHidden)
        XCTAssertTrue(budget.isHidden)

        quality.selectItem(withTitle: "Compress"); _ = quality.sendAction(quality.action, to: quality.target)
        format.selectItem(withTitle: "WebP"); _ = format.sendAction(format.action, to: format.target)
        qualityValue.stringValue = "1"
        format.selectItem(withTitle: "JPEG"); _ = format.sendAction(format.action, to: format.target)
        XCTAssertEqual(qualityValue.stringValue, "40", "JPEG UI clamps to the encoder's minimum")
        qualityValue.stringValue = "73"
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.encodes.count, 1)
        XCTAssertEqual(worker.encodes[0]["format"] as? String, "jpeg")
        XCTAssertEqual(worker.encodes[0]["quality"] as? String, "compress")
        XCTAssertEqual(worker.encodes[0]["quality_value"] as? UInt64, 73)
        XCTAssertNil(worker.encodes[0]["max_size_bytes"])
        XCTAssertTrue((worker.encodes[0]["png"] as? [String: Any])?.isEmpty == true)
        XCTAssertTrue(controller.state.snapshot?.unsavedChanges == true,
                      "encoding does not mutate draft state")
        let previewMode = try segmented("Output preview image", in: controller.root)
        XCTAssertEqual(previewMode.selectedSegment, 1)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("Exact encoded size") })

        (try field("Crop X", in: controller.root)).stringValue = "3"
        (try field("Crop Y", in: controller.root)).stringValue = "5"
        (try field("Crop width", in: controller.root)).stringValue = "300"
        (try field("Crop height", in: controller.root)).stringValue = "200"
        try button("Apply crop", in: controller.root).performClick(nil)
        XCTAssertEqual(previewMode.selectedSegment, 0)
        XCTAssertFalse(previewMode.isEnabled, "an accepted document edit invalidates encoded output")

        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertEqual(previewMode.selectedSegment, 1)
        format.selectItem(withTitle: "WebP"); _ = format.sendAction(format.action, to: format.target)
        XCTAssertEqual(previewMode.selectedSegment, 0)
        XCTAssertFalse(previewMode.isEnabled, "changed options cannot leave stale output current")
    }

    func testCopyUsesEditedPixelsIgnoresExportOptionsAndPreservesOutputAndDraftState() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let original = snapshot(id: "shot", unsaved: true, draft: true)
            let worker = FakeEditorWorker(snapshot: original)
            var writes: [Data] = []; var clipboardAvailable = true
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                worker: worker, writeClipboard: { png in writes.append(png); return clipboardAvailable })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showOutput(in: controller.root)
            try button("Preview output", in: controller.root).performClick(nil)
            let mode = try segmented("Output preview image", in: controller.root)
            let copy = try button("Copy image", in: controller.root)
            XCTAssertEqual(copy.accessibilityLabel(), "Copy edited screenshot")
            copy.performClick(nil)
            XCTAssertEqual(mode.selectedSegment, 1, "Copy must not replace the encoded preview")
            XCTAssertTrue(mode.isEnabled)
            XCTAssertEqual(writes.count, 1)
            XCTAssertTrue(labels(in: controller.root).contains { $0.contains("Edited image copied") })
            try render(controller.root, name: "screenshot-editor-clipboard-success-\(appearance)")

            let format = try popup("Output format", in: controller.root)
            format.selectItem(withTitle: "JPEG"); _ = format.sendAction(format.action, to: format.target)
            let quality = try popup("Output quality mode", in: controller.root)
            quality.selectItem(withTitle: "Maximum file size"); _ = quality.sendAction(quality.action, to: quality.target)
            (try field("Output byte budget", in: controller.root)).stringValue = "invalid"
            clipboardAvailable = false
            copy.performClick(nil)
            let options = try XCTUnwrap(worker.encodes.last)
            XCTAssertEqual(options["format"] as? String, "png")
            XCTAssertEqual(options["quality"] as? String, "preserve")
            XCTAssertTrue(options["max_size_bytes"] is NSNull)
            XCTAssertTrue((options["png"] as? [String: Any])?["max_colors"] is NSNull)
            XCTAssertTrue(labels(in: controller.root).contains { $0.contains("clipboard is unavailable") })
            XCTAssertFalse(controller.state.busy); XCTAssertTrue(copy.isEnabled)
            try render(controller.root, name: "screenshot-editor-clipboard-error-minimum-\(appearance)")
            clipboardAvailable = true; copy.performClick(nil)
            XCTAssertEqual(writes.count, 3)
            worker.failEncode = true; copy.performClick(nil)
            XCTAssertEqual(writes.count, 3, "encoding failure must not touch the clipboard")
            XCTAssertFalse(controller.state.busy); XCTAssertTrue(copy.isEnabled)
            XCTAssertEqual(controller.state.snapshot, original)
            XCTAssertTrue(worker.requests.isEmpty && worker.saves.isEmpty)
        }
    }

    func testStaleClipboardCompletionCannotWriteAfterTermination() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        worker.deferEncodes = true
        var writes = 0
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
            worker: worker, writeClipboard: { _ in writes += 1; return true })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        let copy = try button("Copy image", in: controller.root)
        copy.performClick(nil)
        XCTAssertTrue(controller.state.busy); XCTAssertFalse(copy.isEnabled)
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        XCTAssertEqual(worker.closeCount, 0, "accepted worker work must not be freed during copy")
        XCTAssertTrue(controller.prepareForTermination())
        worker.completePendingEncode()
        XCTAssertEqual(writes, 0)
        XCTAssertNil(controller.state.artifactID)
    }

    func testRealClipboardContainsCroppedEditedPngAfterWorkerCloseWithoutCopyPersistence() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let originalURL = fixture.history.appendingPathComponent(fixture.id).appendingPathComponent("capture.png")
        let original = try Data(contentsOf: originalURL)
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("es.captures.tests.\(UUID())"))
        defer { pasteboard.releaseGlobally() }
        let worker = EditorWorker()
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
            worker: worker, writeClipboard: { png in
                pasteboard.clearContents(); return pasteboard.setData(png, forType: .png)
            })
        defer { controller.window.orderOut(nil); worker.close(); EditorWorker.flush() }
        controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
        waitUntil { controller.state.snapshot != nil && !controller.state.busy }
        for (label, value) in [("Crop X", "2"), ("Crop Y", "1"), ("Crop width", "4"), ("Crop height", "2")] {
            (try field(label, in: controller.root)).stringValue = value
        }
        try button("Apply crop", in: controller.root).performClick(nil)
        waitUntil { controller.state.snapshot?.width == 4 && !controller.state.busy }
        let edited = controller.state.snapshot
        try showOutput(in: controller.root)
        try button("Copy image", in: controller.root).performClick(nil)
        waitUntil { !controller.state.busy && pasteboard.data(forType: .png) != nil }
        let png = try XCTUnwrap(pasteboard.data(forType: .png))
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: png))
        XCTAssertEqual(bitmap.pixelsWide, 4); XCTAssertEqual(bitmap.pixelsHigh, 2)
        let color = try XCTUnwrap(bitmap.colorAt(x: 0, y: 0)?.usingColorSpace(.sRGB))
        XCTAssertEqual(color.redComponent, 62.0 / 255, accuracy: 1.0 / 255)
        XCTAssertEqual(color.greenComponent, 71.0 / 255, accuracy: 1.0 / 255)
        XCTAssertEqual(color.blueComponent, 19.0 / 255, accuracy: 1.0 / 255)
        XCTAssertEqual(controller.state.snapshot, edited)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.drafts.path))
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: fixture.history.path), [fixture.id])
        XCTAssertEqual(try Data(contentsOf: originalURL), original)
        worker.close(); EditorWorker.flush()
        XCTAssertEqual(pasteboard.data(forType: .png), png, "pasteboard owns bytes after the editor worker closes")
    }

    func testMaximumOutputRequiresLocaleParsedDefaultCapAndFailuresRemainRecoverable() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!,
            worker: worker, numberLocale: Locale(identifier: "fr_FR"))
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        let quality = try popup("Output quality mode", in: controller.root)
        quality.selectItem(withTitle: "Maximum file size")
        _ = quality.sendAction(quality.action, to: quality.target)
        let budget = try field("Output byte budget", in: controller.root)
        XCTAssertFalse(budget.isHidden); XCTAssertEqual(budget.stringValue, "10000000")
        XCTAssertTrue((try field("Output quality value", in: controller.root)).isHidden)
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.encodes.last?["max_size_bytes"] as? UInt64, 10_000_000)
        XCTAssertEqual(worker.encodes.last?["quality"] as? String, "maximum")

        budget.stringValue = "9 999"
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.state.busy)
        XCTAssertEqual(worker.encodes.count, 1)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("at least 10,000") })

        budget.stringValue = "10000"
        worker.failEncode = true
        worker.failureMessage = "fixture encode failed"
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.state.busy)
        XCTAssertNotNil(controller.state.snapshot)
        XCTAssertTrue(controller.window.isVisible)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("fixture encode failed") })
    }

    func testStaleOutputCompletionCannotReopenClosedEditor() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        worker.deferEncodes = true
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                     worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertTrue(controller.state.busy)
        XCTAssertTrue(controller.prepareForTermination())
        worker.completePendingEncode()
        XCTAssertNil(controller.state.artifactID)
        XCTAssertFalse(controller.window.isVisible)
    }

    func testSaveNewCopySendsExactDestinationOptionsAndModeWithoutChangingDraft() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: true))
        var refreshCount = 0
        let controller = ScreenshotEditorController(
            tokens: Tokens.variants["light-mustard"]!, worker: worker,
            didSaveCopy: { refreshCount += 1 })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot", mode: "window"),
                           historyRoot: "/native/History", outputDirectory: "/exports")
        try showOutput(in: controller.root)

        let format = try popup("Output format", in: controller.root)
        format.selectItem(withTitle: "JPEG")
        _ = format.sendAction(format.action, to: format.target)
        (try field("Output filename", in: controller.root)).stringValue = "asymmetric-edited.jpeg"
        try button("Save new copy", in: controller.root).performClick(nil)

        let request = try XCTUnwrap(worker.saves.last)
        XCTAssertEqual(request["history_root"] as? String, "/native/History")
        XCTAssertEqual(request["destination"] as? String, "/exports/asymmetric-edited.jpg")
        XCTAssertEqual(request["mode"] as? String, "window")
        let options = try XCTUnwrap(request["options"] as? [String: Any])
        XCTAssertEqual(options["format"] as? String, "jpeg")
        XCTAssertEqual(options["quality"] as? String, "preserve")
        XCTAssertEqual(refreshCount, 1)
        XCTAssertTrue(controller.state.snapshot?.unsavedChanges == true)
        XCTAssertTrue(controller.state.snapshot?.hasDraft == true)

        worker.saveResult = .failure(AppBridgeError.backend("destination already exists"))
        try button("Save new copy", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.state.busy)
        XCTAssertTrue(controller.window.isVisible)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("destination already exists") })

        worker.saveResult = .success(.savedWithoutHistory(
            path: "/exports/asymmetric-edited.jpg", warning: "fixture History failure"))
        try button("Save new copy", in: controller.root).performClick(nil)
        XCTAssertTrue(labels(in: controller.root).contains {
            $0.contains("Saved new copy") && $0.contains("fixture History failure")
        })
        XCTAssertEqual(refreshCount, 1, "partial publication does not claim a History refresh")
    }

    func testDirectoryPickerPreservesFilenameAndCancelOrStaleCompletionChangesNothing() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        var pickerCurrent: URL?
        var pickerCompletion: ((URL?) -> Void)?
        let controller = ScreenshotEditorController(
            tokens: Tokens.variants["light-mustard"]!, worker: worker,
            directoryPicker: { _, current, completion in
                pickerCurrent = current; pickerCompletion = completion
            })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History",
                           outputDirectory: "/first folder")
        try showOutput(in: controller.root)
        let filename = try field("Output filename", in: controller.root)
        filename.stringValue = "keep-this-name.png"
        try button("Change…", in: controller.root).performClick(nil)
        XCTAssertEqual(pickerCurrent?.path, "/first folder")
        pickerCompletion?(nil)
        XCTAssertEqual(filename.stringValue, "keep-this-name.png")
        XCTAssertEqual((try field("Output save location", in: controller.root)).stringValue,
                       "/first folder")
        XCTAssertFalse(controller.state.busy, "the folder panel does not occupy the editor worker")

        try button("Change…", in: controller.root).performClick(nil)
        pickerCompletion?(URL(fileURLWithPath: "/selected folder", isDirectory: true))
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual((try field("Output save location", in: controller.root)).stringValue,
                       "/selected folder")
        XCTAssertEqual(filename.stringValue, "keep-this-name.png")

        try button("Change…", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        pickerCompletion?(URL(fileURLWithPath: "/stale folder", isDirectory: true))
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual((try field("Output save location", in: controller.root)).stringValue,
                       "/selected folder", "a retired editor ignores the folder reply")

        let panel = ScreenshotEditorController.outputDirectoryPanel(
            current: URL(fileURLWithPath: "/first folder", isDirectory: true))
        XCTAssertEqual(panel.title, "Choose save location")
        XCTAssertEqual(panel.message, "Choose save location")
        XCTAssertTrue(panel.canChooseDirectories); XCTAssertFalse(panel.canChooseFiles)
        XCTAssertTrue(panel.canCreateDirectories); XCTAssertEqual(panel.directoryURL?.path, "/first folder")
    }

    func testImagePickerImportsStraightRgbaAndSelectsReturnedStableID() throws {
        _ = NSApplication.shared
        let background = layer(id: "background", name: "Original", x: 0, y: 0,
                               visible: true, locked: true, opacity: 100)
        let selected = layer(id: "selected", name: "Selected", x: 17, y: -9,
                             visible: false, locked: false, opacity: 63)
        let imported = layer(id: "returned-id", name: "Asymmetric import", x: 41, y: 23,
                             visible: true, locked: false, opacity: 100)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [background, selected]))
        worker.importLayerID = "returned-id"
        worker.importedSnapshot = snapshot(id: "shot", unsaved: true,
                                           layers: [background, selected, imported])
        var pickerCompletion: ((URL?) -> Void)?
        let bytes = Data([11, 29, 47, 61, 73, 89, 101, 127, 131, 149, 167, 191,
                          193, 211, 223, 239, 17, 37, 59, 83, 97, 109, 137, 251])
        let controller = ScreenshotEditorController(
            tokens: Tokens.variants["light-mustard"]!, worker: worker,
            imagePicker: { _, completion in pickerCompletion = completion },
            imageDecoder: { url in
                XCTAssertEqual(url.path, "/tmp/asymmetric.png")
                return EditorDecodedImage(data: bytes, width: 3, height: 2,
                                          bytesPerRow: 12, name: "Asymmetric import")
            })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)

        try button("Add image…", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.state.busy, "the picker does not occupy the session worker")
        pickerCompletion?(URL(fileURLWithPath: "/tmp/asymmetric.png"))
        waitUntil { worker.imports.count == 1 && !controller.state.busy }

        let call = try XCTUnwrap(worker.imports.first)
        XCTAssertEqual(call.image.data, bytes)
        XCTAssertEqual(call.image.width, 3); XCTAssertEqual(call.image.height, 2)
        XCTAssertEqual(call.image.bytesPerRow, 12); XCTAssertEqual(call.selectedID, "selected")
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue,
                       "Asymmetric import", "the FFI-returned stable ID is selected")
        XCTAssertTrue(controller.state.snapshot?.unsavedChanges == true)

        let panel = ScreenshotEditorController.imagePanel()
        XCTAssertEqual(panel.title, "Choose image")
        XCTAssertEqual(panel.message, "Choose an image to add as a new layer")
        XCTAssertTrue(panel.canChooseFiles); XCTAssertFalse(panel.canChooseDirectories)
        XCTAssertFalse(panel.allowsMultipleSelection)
    }

    func testDecodedImportWaitsBehindAcceptedEditAndLateOrCancelledPickerRepliesAreIgnored() throws {
        _ = NSApplication.shared
        let background = layer(id: "background", name: "Original", x: 0, y: 0,
                               visible: true, locked: true, opacity: 100)
        let selected = layer(id: "selected", name: "Selected", x: 17, y: -9,
                             visible: true, locked: false, opacity: 63)
        let imported = layer(id: "queued-import", name: "Queued", x: 8, y: 5,
                             visible: true, locked: false, opacity: 100)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [background, selected]))
        worker.importLayerID = "queued-import"
        worker.importedSnapshot = snapshot(id: "shot", unsaved: true,
                                           layers: [background, selected, imported])
        let decodeStarted = DispatchSemaphore(value: 0)
        let allowDecode = DispatchSemaphore(value: 0)
        var pickerCompletion: ((URL?) -> Void)?
        let controller = ScreenshotEditorController(
            tokens: Tokens.variants["light-mustard"]!, worker: worker,
            imagePicker: { _, completion in pickerCompletion = completion },
            imageDecoder: { _ in
                decodeStarted.signal(); _ = allowDecode.wait(timeout: .now() + 2)
                return EditorDecodedImage(data: Data(repeating: 0x7f, count: 8),
                                          width: 2, height: 1, bytesPerRow: 8, name: "Queued")
            })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)

        try button("Add image…", in: controller.root).performClick(nil)
        pickerCompletion?(nil)
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertTrue(worker.imports.isEmpty, "picker cancellation has no editor side effect")

        try button("Add image…", in: controller.root).performClick(nil)
        pickerCompletion?(URL(fileURLWithPath: "/tmp/queued.png"))
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(decodeStarted.wait(timeout: .now() + 2), .success)
        worker.deferRequests = true
        try button("Hide", in: controller.root).performClick(nil)
        XCTAssertTrue(controller.state.busy)
        allowDecode.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        XCTAssertTrue(worker.imports.isEmpty, "decoded pixels wait behind the accepted edit")
        worker.deferRequests = false
        worker.completePending(with: snapshot(id: "shot", unsaved: true,
                                              layers: [background, selected]))
        waitUntil { worker.imports.count == 1 && !controller.state.busy }

        let lateStarted = DispatchSemaphore(value: 0)
        let allowLate = DispatchSemaphore(value: 0)
        var lateCompletion: ((URL?) -> Void)?
        let lateWorker = FakeEditorWorker(snapshot: snapshot(id: "late"))
        let late = ScreenshotEditorController(
            tokens: Tokens.variants["dark-mustard"]!, worker: lateWorker,
            imagePicker: { _, completion in lateCompletion = completion },
            imageDecoder: { _ in
                lateStarted.signal(); _ = allowLate.wait(timeout: .now() + 2)
                return EditorDecodedImage(data: Data(repeating: 0xff, count: 4),
                                          width: 1, height: 1, bytesPerRow: 4, name: "Late")
            })
        defer { late.window.orderOut(nil) }
        late.present(artifact: artifact(id: "late"), historyRoot: "/native/History")
        try showLayers(in: late.root)
        try button("Add image…", in: late.root).performClick(nil)
        lateCompletion?(URL(fileURLWithPath: "/tmp/late.png"))
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(lateStarted.wait(timeout: .now() + 2), .success)
        XCTAssertFalse(late.windowShouldClose(late.window))
        allowLate.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        XCTAssertTrue(lateWorker.imports.isEmpty, "a closed editor rejects late decoded pixels")
    }

    func testLayerSnapshotOrderAndCommandsUseStableIDs() throws {
        _ = NSApplication.shared
        let background = layer(id: "background", name: "Original screenshot", x: 0, y: 0,
                               visible: true, locked: true, opacity: 100)
        let foreground = layer(id: "foreground", name: "A very long foreground image layer name",
                               x: 13.5, y: -7.25, visible: false, locked: false, opacity: 42.5)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [background, foreground]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                     worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)

        XCTAssertEqual(controller.state.snapshot?.layers.map(\.id), ["foreground", "background"])
        let nameField = try field("Layer name", in: controller.root)
        XCTAssertNil(nameField.formatter, "layer names must not use the numeric geometry formatter")
        XCTAssertTrue(nameField.isEditable); XCTAssertEqual(nameField.alignment, .left)
        XCTAssertEqual(nameField.placeholderString, "Layer name")
        XCTAssertEqual(nameField.stringValue,
                       "A very long foreground image layer name")
        XCTAssertEqual((try field("Layer X", in: controller.root)).stringValue, "13.5")
        XCTAssertEqual((try field("Layer Y", in: controller.root)).stringValue, "-7.25")
        XCTAssertEqual(try button("Show", in: controller.root).isEnabled, true,
                       "hidden layers remain editable")

        try button("Show", in: controller.root).performClick(nil)
        let visibility = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
        XCTAssertEqual(visibility["action"] as? String, "visibility")
        XCTAssertEqual(visibility["visible"] as? Bool, true)
        XCTAssertEqual(worker.requests.last?["id"] as? String, "foreground")

        (try field("Layer X", in: controller.root)).stringValue = "29.5"
        (try field("Layer Y", in: controller.root)).stringValue = "4.75"
        try button("Move", in: controller.root).performClick(nil)
        let move = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
        XCTAssertEqual(move["action"] as? String, "translate")
        XCTAssertEqual(move["delta_x"] as? Double, 16)
        XCTAssertEqual(move["delta_y"] as? Double, 12)

        (try field("Layer name", in: controller.root)).stringValue = "Foreground renamed"
        try button("Rename", in: controller.root).performClick(nil)
        XCTAssertEqual((worker.requests.last?["edit"] as? [String: Any])?["action"] as? String, "rename")
        XCTAssertEqual((worker.requests.last?["edit"] as? [String: Any])?["name"] as? String,
                       "Foreground renamed")
        (try field("Layer opacity", in: controller.root)).stringValue = "73.25"
        try button("Set", in: controller.root).performClick(nil)
        XCTAssertEqual((worker.requests.last?["edit"] as? [String: Any])?["opacity"] as? Double, 73.25)
        try button("Lock", in: controller.root).performClick(nil)
        XCTAssertEqual((worker.requests.last?["edit"] as? [String: Any])?["locked"] as? Bool, true)

        worker.response = { request in
            guard let edit = request["edit"] as? [String: Any],
                  edit["action"] as? String == "duplicate",
                  let newID = edit["new_id"] as? String else { return nil }
            return self.snapshot(id: "shot", unsaved: true,
                layers: [background, foreground,
                         self.layer(id: newID, name: "A very long foreground image layer name copy",
                                    x: 37.5, y: 16.75, visible: true, locked: false, opacity: 42.5)])
        }
        try button("Duplicate", in: controller.root).performClick(nil)
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue,
                       "A very long foreground image layer name copy")
        XCTAssertEqual((try field("Layer X", in: controller.root)).stringValue, "37.5")
        XCTAssertEqual((try field("Layer Y", in: controller.root)).stringValue, "16.75")
    }

    func testImageTransformCommandsKeepHiddenLockedSelectionAndInvalidateOutput() throws {
        _ = NSApplication.shared
        let transformed = layer(id: "stable-image", name: "Hidden locked image", x: 31.5, y: -14.25,
                                visible: false, locked: true, opacity: 67)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true,
                                                          layers: [transformed]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                     worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        try button("Preview output", in: controller.root).performClick(nil)
        let previewMode = try segmented("Output preview image", in: controller.root)
        XCTAssertEqual(previewMode.selectedSegment, 1)
        try showLayers(in: controller.root)

        for title in ["Rotate left", "Rotate right", "Flip horizontal", "Flip vertical"] {
            XCTAssertTrue(try button(title, in: controller.root).isEnabled,
                          "hidden and locked image layers remain transformable")
            try button(title, in: controller.root).performClick(nil)
            XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue,
                           "Hidden locked image", "stable selection survives each reply")
        }
        let transforms = worker.requests.compactMap { request -> String? in
            guard request["operation"] as? String == "layer",
                  request["id"] as? String == "stable-image",
                  let edit = request["edit"] as? [String: Any],
                  edit["action"] as? String == "image_transform" else { return nil }
            return edit["transform"] as? String
        }
        XCTAssertEqual(transforms, ["rotate-counterclockwise", "rotate-clockwise",
                                    "flip-horizontal", "flip-vertical"])
        XCTAssertEqual(previewMode.selectedSegment, 0)
        XCTAssertFalse(previewMode.isEnabled,
                       "an accepted image transform invalidates stale encoded output")
        XCTAssertTrue(controller.state.snapshot?.unsavedChanges == true)
    }

    func testShapeDragMapsPreviewCoordinatesSelectsFreshLayerAndCancelsWithoutEdits() throws {
        _ = NSApplication.shared
        let original = layer(id: "original", name: "Original", x: 0, y: 0,
                             visible: true, locked: true, opacity: 100)
        let created = shapeLayer(id: "fresh-shape", x: 127.152, y: -55.099)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", width: 1280, height: 640,
                                                          unsaved: true, layers: [original]))
        worker.response = { request in
            guard request["operation"] as? String == "create_closed_shape" else { return nil }
            return self.snapshot(id: "shot", width: 1280, height: 640,
                                 unsaved: true, draft: true, layers: [original, created])
        }
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                     worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        try button("Preview output", in: controller.root).performClick(nil)
        let outputMode = try segmented("Output preview image", in: controller.root)
        XCTAssertEqual(outputMode.selectedSegment, 1)
        try showDraw(in: controller.root)

        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(at: 1); _ = tool.sendAction(tool.action, to: tool.target)
        let overlay = controller.drawOverlay
        XCTAssertEqual(overlay.presentedImageRect,
                       NSRect(x: 0, y: 106, width: 604, height: 302))
        overlay.begin(at: NSPoint(x: 500, y: 350))
        overlay.drag(to: NSPoint(x: 60, y: 80))
        XCTAssertTrue(worker.requests.isEmpty, "transient drawing never mutates the document")
        overlay.end(at: NSPoint(x: 60, y: 80))

        let request = try XCTUnwrap(worker.requests.last)
        XCTAssertEqual(request["operation"] as? String, "create_closed_shape")
        XCTAssertEqual(request["shape"] as? String, "ellipse")
        let start = try XCTUnwrap(request["start"] as? [String: CGFloat])
        let end = try XCTUnwrap(request["end"] as? [String: CGFloat])
        XCTAssertEqual(try XCTUnwrap(start["x"]), 1_059.603, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(start["y"]), 517.086, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(end["x"]), 127.152, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(end["y"]), -55.099, accuracy: 0.001,
                       "preview whitespace maps to off-canvas document coordinates")
        XCTAssertNil(request["style"]); XCTAssertNil(request["opacity"])
        XCTAssertEqual(outputMode.selectedSegment, 0)
        XCTAssertFalse(outputMode.isEnabled, "accepted creation invalidates encoded output")
        try showLayers(in: controller.root)
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue, "Shape")
        XCTAssertEqual(try table("Screenshot layers", in: controller.root).selectedRow, 0,
                       "the returned fresh stable layer is selected")

        let acceptedCount = worker.requests.count
        try showDraw(in: controller.root)
        overlay.begin(at: NSPoint(x: 200, y: 200)); overlay.drag(to: NSPoint(x: 260, y: 260))
        overlay.keyDown(with: try keyEvent(window: controller.window, keyCode: 53,
                                           characters: "\u{1b}"))
        overlay.end(at: NSPoint(x: 260, y: 260))
        XCTAssertEqual(worker.requests.count, acceptedCount, "Escape cancels without an edit")

        overlay.begin(at: NSPoint(x: 210, y: 210)); overlay.drag(to: NSPoint(x: 270, y: 270))
        try showLayers(in: controller.root)
        overlay.end(at: NSPoint(x: 270, y: 270))
        XCTAssertEqual(worker.requests.count, acceptedCount, "leaving Draw cancels")

        try showDraw(in: controller.root)
        overlay.begin(at: NSPoint(x: 220, y: 220)); overlay.drag(to: NSPoint(x: 280, y: 280))
        controller.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification,
                                                   object: controller.window))
        overlay.end(at: NSPoint(x: 280, y: 280))
        XCTAssertEqual(worker.requests.count, acceptedCount, "focus loss cancels")

        overlay.begin(at: NSPoint(x: 300, y: 200)); overlay.end(at: NSPoint(x: 300, y: 350))
        XCTAssertEqual(worker.requests.count, acceptedCount, "zero-width shapes are host no-ops")

        overlay.begin(at: NSPoint(x: 230, y: 230)); overlay.drag(to: NSPoint(x: 290, y: 290))
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        overlay.end(at: NSPoint(x: 290, y: 290))
        XCTAssertEqual(worker.requests.count, acceptedCount, "closing cancels the transient drag")
        if let sheet = controller.window.attachedSheet {
            controller.window.endSheet(sheet, returnCode: .alertThirdButtonReturn)
        }
    }

    func testRejectedDuplicateAndDeletionKeepRecoverableStableSelection() throws {
        _ = NSApplication.shared
        let back = layer(id: "back", name: "Back", x: 0, y: 0, visible: true,
                         locked: true, opacity: 100)
        let middle = layer(id: "middle", name: "Middle", x: 8, y: 21, visible: true,
                           locked: false, opacity: 80)
        let front = layer(id: "front", name: "Front", x: -3, y: 5, visible: false,
                          locked: false, opacity: 60)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [back, middle, front]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)
        let table = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSTableView }.first)
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        controller.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification,
                                                              object: table))
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue, "Middle")

        try button("Move up", in: controller.root).performClick(nil)
        let reorder = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
        XCTAssertEqual(reorder["action"] as? String, "reorder")
        XCTAssertEqual(reorder["target_id"] as? String, "front")
        XCTAssertEqual(reorder["placement"] as? String, "before")

        worker.failLayerAction = "duplicate"
        try button("Duplicate", in: controller.root).performClick(nil)
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue, "Middle")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("fixture save failed") })

        worker.failLayerAction = nil
        worker.response = { request in
            guard let edit = request["edit"] as? [String: Any], edit["action"] as? String == "delete"
            else { return nil }
            return self.snapshot(id: "shot", unsaved: true, layers: [back, front])
        }
        try button("Delete", in: controller.root).performClick(nil)
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue, "Back",
                       "deletion selects the nearest surviving panel row")
        XCTAssertFalse(try button("Move", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Delete", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Hide", in: controller.root).isEnabled,
                      "locked layers still permit visibility")
        XCTAssertTrue(try button("Duplicate", in: controller.root).isEnabled,
                      "locked layers still permit duplication")

        worker.response = { request in
            guard request["operation"] as? String == "undo" else { return nil }
            return self.snapshot(id: "shot", unsaved: true, layers: [back, middle, front])
        }
        try button("Undo", in: controller.root).performClick(nil)
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue, "Back",
                       "undo retains a still-existing stable layer selection")
    }

    func testEditorControlsRenderAndSendSharedGeometryCommands() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", width: 640, height: 360))
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            let geometry = try XCTUnwrap(descendants(in: controller.root).first {
                $0.accessibilityLabel() == "Geometry controls"
            })
            XCTAssertTrue(geometry.isFlipped)
            let cropLabel = try XCTUnwrap(geometry.subviews.compactMap { $0 as? NSTextField }
                .first { $0.stringValue == "X" })
            let cropField = try field("Crop X", in: geometry)
            XCTAssertLessThan(cropLabel.frame.minY, cropField.frame.minY,
                              "top-down geometry places labels above fields")
            (try field("Crop X", in: controller.root)).stringValue = "13"
            (try field("Crop Y", in: controller.root)).stringValue = "7"
            (try field("Crop width", in: controller.root)).stringValue = "321"
            (try field("Crop height", in: controller.root)).stringValue = "199"
            try button("Apply crop", in: controller.root).performClick(nil)
            XCTAssertEqual(worker.requests.last?["operation"] as? String, "crop")
            let rect = try XCTUnwrap(worker.requests.last?["rect"] as? [String: Double])
            XCTAssertEqual(rect, ["x": 13, "y": 7, "width": 321, "height": 199])

            (try field("Canvas width", in: controller.root)).stringValue = "800"
            (try field("Canvas height", in: controller.root)).stringValue = "500"
            try button("Resize canvas", in: controller.root).performClick(nil)
            XCTAssertEqual(worker.requests.last?["operation"] as? String, "resize_canvas")
            XCTAssertEqual(worker.requests.last?["width"] as? Double, 800)
            XCTAssertEqual(worker.requests.last?["height"] as? Double, 500)
            XCTAssertFalse(try button("Undo", in: controller.root).isEnabled)
            XCTAssertFalse(try button("Redo", in: controller.root).isEnabled)
            try render(controller.root, name: "screenshot-editor-\(appearance)")

            worker.snapshot = snapshot(id: "shot", width: 640, height: 360,
                                       unsaved: true, draft: true)
            try button("Apply crop", in: controller.root).performClick(nil)
            XCTAssertFalse(controller.windowShouldClose(controller.window))
            let closeSheet = try XCTUnwrap(controller.window.attachedSheet)
            settle(closeSheet)
            try render(try XCTUnwrap(closeSheet.contentView),
                       name: "screenshot-editor-unsaved-close-\(appearance)")
            controller.window.endSheet(closeSheet, returnCode: .alertThirdButtonReturn)

            worker.failOperation = "save_draft"
            worker.failureMessage = "The draft could not be saved because the isolated editor-drafts location is unavailable. Your unsaved screenshot edits remain open and recoverable."
            try button("Save draft", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-save-error-\(appearance)")

            try button("Discard edits…", in: controller.root).performClick(nil)
            let discardSheet = try XCTUnwrap(controller.window.attachedSheet)
            settle(discardSheet)
            try render(try XCTUnwrap(discardSheet.contentView),
                       name: "screenshot-editor-discard-\(appearance)")
            controller.window.endSheet(discardSheet, returnCode: .alertSecondButtonReturn)
        }
    }

    func testLayerPanelRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let layers = [
                layer(id: "locked-background", name: "Original screenshot", x: 0, y: 0,
                      visible: true, locked: true, opacity: 100),
                layer(id: "hidden-locked", name: "Hidden locked reference", x: 12, y: 8,
                      visible: false, locked: true, opacity: 75),
                layer(id: "hidden-image", name: "A layer name long enough to require truncation in the panel",
                      x: 43.5, y: -12.25, visible: false, locked: false, opacity: 57.5),
            ]
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true,
                                                              draft: true, layers: layers))
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showLayers(in: controller.root)
            let layerPanel = try XCTUnwrap(descendants(in: controller.root).first {
                $0.accessibilityLabel() == "Layer controls"
            })
            XCTAssertTrue(layerPanel.isFlipped)
            let opacityLabel = try XCTUnwrap(descendants(in: layerPanel).compactMap { $0 as? NSTextField }
                .first { $0.stringValue == "Opacity (0–100)" })
            let opacityField = try field("Layer opacity", in: layerPanel)
            XCTAssertLessThan(opacityLabel.frame.minY, opacityField.frame.minY,
                              "top-down layer controls place labels above fields")
            let table = try XCTUnwrap(descendants(in: layerPanel).compactMap { $0 as? NSTableView }.first)
            controller.root.layoutSubtreeIfNeeded(); table.layoutSubtreeIfNeeded()
            let combinedStateCell = try XCTUnwrap(table.view(atColumn: 0, row: 1,
                makeIfNecessary: true) as? NSTableCellView)
            combinedStateCell.layoutSubtreeIfNeeded()
            let combinedState = try XCTUnwrap(combinedStateCell.subviews.compactMap {
                $0 as? NSTextField
            }.first { $0 !== combinedStateCell.textField })
            XCTAssertEqual(combinedState.stringValue, "Image · Hidden · Locked")
            XCTAssertLessThanOrEqual(combinedState.frame.maxX, combinedStateCell.visibleRect.maxX - 8,
                                     "metadata respects the clipped cell's visible trailing inset")
            XCTAssertLessThanOrEqual(combinedState.intrinsicContentSize.width,
                                     combinedState.frame.width,
                                     "combined Hidden and Locked metadata is not truncated")
            XCTAssertLessThanOrEqual(table.rect(ofRow: 2).maxY, table.visibleRect.maxY,
                                     "the initial three-layer fixture does not expose a partial row")
            try render(controller.root, name: "screenshot-editor-layers-\(appearance)")

            worker.failLayerAction = "duplicate"
            worker.failureMessage = "The selected layer could not be duplicated because its shared image asset is unavailable. The current draft remains open and recoverable."
            try button("Duplicate", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-layers-error-minimum-\(appearance)")

            let emptyWorker = FakeEditorWorker(snapshot: snapshot(id: "empty"))
            let empty = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: emptyWorker)
            defer { empty.window.orderOut(nil) }
            empty.present(artifact: artifact(id: "empty"), historyRoot: "/native/History")
            try showLayers(in: empty.root)
            try render(empty.root, name: "screenshot-editor-layers-empty-\(appearance)")
        }
    }

    func testOutputPanelRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true))
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showOutput(in: controller.root)
            try render(controller.root, name: "screenshot-editor-output-normal-\(appearance)")

            let quality = try popup("Output quality mode", in: controller.root)
            quality.selectItem(withTitle: "Compress")
            _ = quality.sendAction(quality.action, to: quality.target)
            try button("Preview output", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-output-preview-\(appearance)")

            quality.selectItem(withTitle: "Maximum file size")
            _ = quality.sendAction(quality.action, to: quality.target)
            (try field("Output byte budget", in: controller.root)).stringValue = "99"
            try button("Preview output", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-output-error-\(appearance)")

            (try field("Output byte budget", in: controller.root)).stringValue = "10000"
            worker.failEncode = true
            worker.failureMessage = "The encoded screenshot cannot meet this byte budget without exceeding the supported quality limits. The current draft and undo history remain unchanged and recoverable."
            try button("Preview output", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-output-error-minimum-\(appearance)")
        }
    }

    func testSaveNewCopyRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: true))
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History",
                               outputDirectory: "/Users/test/Pictures/Captures Export")
            try showOutput(in: controller.root)
            try scrollOutputSaveControlsVisible(in: controller.root)
            try render(controller.root, name: "screenshot-editor-save-copy-normal-\(appearance)")

            worker.saveResult = .success(.saved(
                path: "/Users/test/Pictures/Captures Export/Captures_2026-09-20_edited.png"))
            try button("Save new copy", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-save-copy-success-\(appearance)")

            worker.saveResult = .failure(AppBridgeError.backend("A file with this name already exists."))
            try button("Save new copy", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-save-copy-error-\(appearance)")

            worker.saveResult = .success(.savedWithoutHistory(
                path: "/Users/test/Pictures/Captures Export/Captures_2026-09-20_edited.png",
                warning: "The new file is safe, but the isolated native History location is unavailable. You can reveal the saved file and retry History publication later."))
            try button("Save new copy", in: controller.root).performClick(nil)
            try render(controller.root,
                       name: "screenshot-editor-save-copy-error-minimum-\(appearance)")
        }
    }

    func testImageImportRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let background = layer(id: "background", name: "Original screenshot", x: 0, y: 0,
                                   visible: true, locked: true, opacity: 100)
            let imported = layer(id: "imported", name: "Asymmetric transparent overlay", x: 37, y: -11,
                                 visible: true, locked: false, opacity: 100)
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [background]))
            worker.importLayerID = "imported"
            worker.importedSnapshot = snapshot(id: "shot", unsaved: true,
                                               layers: [background, imported])
            var pickerCompletion: ((URL?) -> Void)?
            var decodeError: String?
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker,
                imagePicker: { _, completion in pickerCompletion = completion },
                imageDecoder: { _ in
                    if let decodeError { throw AppBridgeError.backend(decodeError) }
                    return EditorDecodedImage(data: Data([
                        211, 17, 53, 255, 29, 197, 71, 127,
                        83, 37, 223, 191, 149, 101, 47, 255,
                    ]), width: 2, height: 2, bytesPerRow: 8,
                        name: "Asymmetric transparent overlay")
                })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showLayers(in: controller.root)
            try scrollImageImportVisible(in: controller.root)
            try render(controller.root, name: "screenshot-editor-import-normal-\(appearance)")

            try button("Add image…", in: controller.root).performClick(nil)
            pickerCompletion?(URL(fileURLWithPath: "/tmp/import.png"))
            waitUntil { worker.imports.count == 1 && !controller.state.busy }
            try scrollLayersTop(in: controller.root)
            try render(controller.root, name: "screenshot-editor-import-success-\(appearance)")

            try scrollImageImportVisible(in: controller.root)
            decodeError = "The selected file does not contain a decodable still image."
            try button("Add image…", in: controller.root).performClick(nil)
            pickerCompletion?(URL(fileURLWithPath: "/tmp/not-an-image.txt"))
            waitUntil { labels(in: controller.root).contains { $0.contains("decodable still image") } }
            try render(controller.root, name: "screenshot-editor-import-error-\(appearance)")

            decodeError = nil; worker.failImport = true
            worker.failureMessage = "The decoded image exceeds the retained editor asset budget. The current draft, layer selection, undo history, and previously imported pixels remain open and recoverable."
            try button("Add image…", in: controller.root).performClick(nil)
            pickerCompletion?(URL(fileURLWithPath: "/tmp/too-large.png"))
            waitUntil { !controller.state.busy && labels(in: controller.root).contains {
                $0.contains("retained editor asset budget")
            } }
            try render(controller.root,
                       name: "screenshot-editor-import-error-minimum-\(appearance)")
        }
    }

    func testImageTransformRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let transformed = layer(id: "stable-image", name: "Hidden locked asymmetric image",
                                    x: 37.5, y: -11.25, visible: false, locked: true, opacity: 64)
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true,
                                                              draft: true, layers: [transformed]))
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showLayers(in: controller.root)
            try scrollImageImportVisible(in: controller.root)
            try render(controller.root, name: "screenshot-editor-transform-controls-\(appearance)")

            worker.failLayerAction = "image_transform"
            worker.failureMessage = "The selected image transform could not be applied. The hidden locked layer, asymmetric pixels, draft, undo history, and selection remain recoverable."
            try button("Rotate right", in: controller.root).performClick(nil)
            try render(controller.root,
                       name: "screenshot-editor-transform-error-minimum-\(appearance)")
        }
    }

    func testClosedShapeRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", width: 960, height: 540,
                                                              unsaved: true, draft: true,
                                                              layers: [shapeLayer(id: "existing-shape",
                                                                                  x: 48, y: 32)]))
            let controller = ScreenshotEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(at: appearance == "light" ? 0 : 1)
            _ = tool.sendAction(tool.action, to: tool.target)
            controller.drawOverlay.begin(at: NSPoint(x: 500, y: 390))
            controller.drawOverlay.drag(to: NSPoint(x: 90, y: 125))
            try render(controller.root, name: "screenshot-editor-draw-preview-\(appearance)")

            worker.failOperation = "create_closed_shape"
            worker.failureMessage = "The closed shape could not be created. The previous draft, layer selection, pixels, and undo history remain open and recoverable."
            controller.drawOverlay.end(at: NSPoint(x: 90, y: 125))
            try render(controller.root,
                       name: "screenshot-editor-draw-error-minimum-\(appearance)")
        }
    }

    func testRealBridgeCropSaveReopenDiscardAndRetainedFrame() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker()
        let opened = expectation(description: "open")
        var original: EditorPresentation?
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            original = try? result.get(); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        XCTAssertEqual(original?.snapshot.width, 7)
        XCTAssertEqual(original?.snapshot.height, 3)

        let cropped = expectation(description: "crop")
        var edited: EditorPresentation?
        worker.request(["operation": "crop", "rect": [
            "x": 2, "y": 1, "width": 4, "height": 2,
        ]]) { result in edited = try? result.get(); cropped.fulfill() }
        wait(for: [cropped], timeout: 5)
        XCTAssertEqual(edited?.image.width, 4); XCTAssertEqual(edited?.image.height, 2)
        XCTAssertEqual(rgba(try XCTUnwrap(edited?.image), x: 0, y: 0), [62, 71, 19, 255])

        let saved = expectation(description: "save")
        worker.request(["operation": "save_draft", "updated_at_ms": 123]) { result in
            XCTAssertFalse((try? result.get().snapshot.unsavedChanges) ?? true); saved.fulfill()
        }
        wait(for: [saved], timeout: 5)
        worker.close(); EditorWorker.flush()
        XCTAssertEqual(rgba(try XCTUnwrap(original?.image), x: 6, y: 2), [186, 142, 19, 255],
                       "an old CGImage/provider must retain its immutable Rust frame after close")

        let reopenedWorker = EditorWorker()
        let reopened = expectation(description: "reopen")
        reopenedWorker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                            artifactID: fixture.id) { result in
            let value = try? result.get()
            XCTAssertEqual(value?.image.width, 4); XCTAssertTrue(value?.snapshot.hasDraft == true)
            reopened.fulfill()
        }
        wait(for: [reopened], timeout: 5)
        let discarded = expectation(description: "discard")
        reopenedWorker.request(["operation": "discard_draft"]) { result in
            let value = try? result.get()
            XCTAssertEqual(value?.image.width, 7); XCTAssertFalse(value?.snapshot.hasDraft ?? true)
            discarded.fulfill()
        }
        wait(for: [discarded], timeout: 5)
        reopenedWorker.close(); EditorWorker.flush()
    }

    func testRealBridgeEncodesPngJpegAndWebpWithIndependentBytesAndAlphaPolicy() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture(transparentOrigin: true)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let fixtureData = try Data(contentsOf: fixture.history.appendingPathComponent(fixture.id)
            .appendingPathComponent("capture.png"))
        let fixtureSource = try XCTUnwrap(CGImageSourceCreateWithData(fixtureData as CFData, nil))
        XCTAssertEqual(renderedAlphaRange(try XCTUnwrap(
            CGImageSourceCreateImageAtIndex(fixtureSource, 0, nil))).lowerBound, 0,
            "the real-bridge fixture itself retains transparency")
        let worker = EditorWorker()
        let opened = expectation(description: "open for export")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            if case .failure(let error) = result { XCTFail("open failed: \(error)") }
            opened.fulfill()
        }
        wait(for: [opened], timeout: 5)

        var outputs: [String: EditorOutputPresentation] = [:]
        for format in ["png", "jpeg", "webp"] {
            let encoded = expectation(description: "encode \(format)")
            worker.encode(["format": format, "quality": "compress", "quality_value": 85,
                           "png": format == "png" ? ["max_colors": 128] : [:]]) { result in
                outputs[format] = try? result.get(); encoded.fulfill()
            }
            wait(for: [encoded], timeout: 5)
        }
        XCTAssertEqual(Array(outputs["png"]?.data.prefix(4) ?? Data()), [137, 80, 78, 71])
        XCTAssertEqual(Array(outputs["jpeg"]?.data.prefix(2) ?? Data()), [255, 216])
        XCTAssertEqual(String(data: outputs["webp"]?.data.subdata(in: 8..<12) ?? Data(),
                              encoding: .ascii), "WEBP")
        XCTAssertEqual(outputs["png"]?.image.width, 7); XCTAssertEqual(outputs["webp"]?.image.height, 3)
        XCTAssertEqual(renderedAlphaRange(try XCTUnwrap(outputs["png"]?.image)), 255...255,
                       "the default editor background composites transparent source pixels")
        XCTAssertEqual(renderedAlphaRange(try XCTUnwrap(outputs["jpeg"]?.image)), 255...255,
                       "lossy output remains opaque")
        worker.close(); EditorWorker.flush()
        XCTAssertFalse(outputs["webp"]?.data.isEmpty ?? true,
                       "copied export bytes remain valid after session close")
    }

    func testRealBridgeSavesNewLossyCopyPublishesHistoryAndNeverOverwrites() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let output = fixture.root.appendingPathComponent("exports")
            .appendingPathComponent("edited-window.jpg")
        let worker = EditorWorker()
        let opened = expectation(description: "open for save")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            if case .failure(let error) = result { XCTFail("open failed: \(error)") }
            opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let request: [String: Any] = [
            "history_root": fixture.history.path,
            "destination": output.path,
            "options": ["format": "jpeg", "quality": "compress", "quality_value": 73,
                        "png": [:]],
            "mode": "display",
        ]
        let saved = expectation(description: "save new copy")
        worker.saveNew(request) { result in
            XCTAssertEqual(try? result.get(), .saved(path: output.path)); saved.fulfill()
        }
        wait(for: [saved], timeout: 5)
        let bytes = try Data(contentsOf: output)
        XCTAssertEqual(Array(bytes.prefix(2)), [255, 216])
        let historyEntries = try FileManager.default.contentsOfDirectory(at: fixture.history,
            includingPropertiesForKeys: nil).filter { $0.hasDirectoryPath }
        XCTAssertEqual(historyEntries.count, 2)
        let published = try XCTUnwrap(historyEntries.first { $0.lastPathComponent != fixture.id })
        let metadata = try JSONSerialization.jsonObject(with: Data(contentsOf:
            published.appendingPathComponent("metadata.json"))) as? [String: Any]
        XCTAssertEqual(metadata?["mode"] as? String, "display")
        XCTAssertEqual(metadata?["saved_path"] as? String, output.path)

        let collision = expectation(description: "reject overwrite")
        worker.saveNew(request) { result in
            if case .success = result { XCTFail("an existing export must not be overwritten") }
            collision.fulfill()
        }
        wait(for: [collision], timeout: 5)
        XCTAssertEqual(try Data(contentsOf: output), bytes)

        let drainedOutput = output.deletingLastPathComponent().appendingPathComponent("drained.jpg")
        var drainedRequest = request; drainedRequest["destination"] = drainedOutput.path
        let drained = expectation(description: "drained save callback")
        worker.saveNew(drainedRequest) { result in
            XCTAssertEqual(try? result.get(), .saved(path: drainedOutput.path)); drained.fulfill()
        }
        XCTAssertNoThrow(try worker.prepareForTermination().get(),
                         "quit drains an accepted publication before freeing the session")
        XCTAssertTrue(FileManager.default.fileExists(atPath: drainedOutput.path))
        wait(for: [drained], timeout: 5)
    }

    func testImageIODecoderAppliesExifOrientationAndProducesStraightSrgbRgba() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let sourcePixels: [[UInt8]] = [
            [10, 20, 30, 255], [40, 50, 60, 255],
            [70, 80, 90, 128], [100, 110, 120, 255],
            [130, 140, 150, 255], [160, 170, 180, 255],
        ]
        let oriented = root.appendingPathComponent("asymmetric-oriented.tiff")
        try writeTiff(url: oriented, width: 2, height: 3, bytes: sourcePixels.flatMap { $0 },
                      colorSpace: try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB)),
                      bitsPerPixel: 32, alpha: .last, orientation: 6)
        let decoded = try EditorImageDecoder.decode(oriented)
        XCTAssertEqual(decoded.width, 3); XCTAssertEqual(decoded.height, 2)
        XCTAssertEqual(decoded.bytesPerRow, 12); XCTAssertEqual(decoded.name, "asymmetric-oriented")
        let expected: [[UInt8]] = [sourcePixels[4], sourcePixels[2], sourcePixels[0],
                                   sourcePixels[5], sourcePixels[3], sourcePixels[1]]
        for (index, value) in expected.enumerated() {
            let actual = Array(decoded.data[(index * 4)..<(index * 4 + 4)])
            for channel in 0..<4 {
                XCTAssertEqual(Double(actual[channel]), Double(value[channel]), accuracy: 2,
                               "oriented pixel \(index), channel \(channel)")
            }
        }

        let p3URL = root.appendingPathComponent("display-p3.tiff")
        let p3 = try XCTUnwrap(CGColorSpace(name: CGColorSpace.displayP3))
        try writeTiff(url: p3URL, width: 1, height: 1, bytes: [180, 80, 40, 255],
                      colorSpace: p3, bitsPerPixel: 32, alpha: .last)
        let convertedP3 = try EditorImageDecoder.decode(p3URL)
        XCTAssertEqual(convertedP3.data[3], 255)
        XCTAssertNotEqual(Array(convertedP3.data.prefix(3)), [180, 80, 40],
                          "Display P3 samples are color-converted, not relabeled as sRGB")

        let cmykURL = root.appendingPathComponent("cmyk.tiff")
        try writeTiff(url: cmykURL, width: 1, height: 1, bytes: [0, 255, 255, 0],
                      colorSpace: CGColorSpaceCreateDeviceCMYK(), bitsPerPixel: 32, alpha: .none)
        let convertedCMYK = try EditorImageDecoder.decode(cmykURL)
        XCTAssertGreaterThan(convertedCMYK.data[0], 200)
        XCTAssertGreaterThan(Int(convertedCMYK.data[0]) - Int(convertedCMYK.data[1]), 150)
        XCTAssertGreaterThan(Int(convertedCMYK.data[0]) - Int(convertedCMYK.data[2]), 150)
        XCTAssertEqual(convertedCMYK.data[3], 255)
    }

    func testRealBridgeImportedPixelsSurviveSourceRemovalDraftSaveAndReopen() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let external = fixture.root.appendingPathComponent("external.png")
        let importPixels = Data([
            211, 17, 53, 255, 29, 197, 71, 127,
            83, 37, 223, 191, 149, 101, 47, 255,
        ])
        try importPixels.write(to: external)
        let decoded = EditorDecodedImage(data: importPixels, width: 2, height: 2,
                                         bytesPerRow: 8, name: "Detached source")
        let worker = EditorWorker()
        let opened = expectation(description: "open for import")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            if case .failure(let error) = result { XCTFail("open failed: \(error)") }
            opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let imported = expectation(description: "import")
        var importedLayerID: String?
        worker.importImage(decoded, selectedID: nil) { result in
            let value = try? result.get()
            importedLayerID = value?.layerID
            XCTAssertTrue(value?.presentation.snapshot.unsavedChanges == true)
            XCTAssertEqual(value?.presentation.snapshot.layers.first?.name, "Detached source")
            imported.fulfill()
        }
        wait(for: [imported], timeout: 5)
        try FileManager.default.removeItem(at: external)
        let undone = expectation(description: "undo imported layer")
        worker.request(["operation": "undo"]) { result in
            XCTAssertFalse((try? result.get().snapshot.layers.contains {
                $0.id == importedLayerID
            }) ?? true)
            undone.fulfill()
        }
        wait(for: [undone], timeout: 5)
        let redone = expectation(description: "redo imported layer")
        worker.request(["operation": "redo"]) { result in
            XCTAssertTrue((try? result.get().snapshot.layers.contains {
                $0.id == importedLayerID
            }) ?? false)
            redone.fulfill()
        }
        wait(for: [redone], timeout: 5)
        let saved = expectation(description: "save imported draft")
        worker.request(["operation": "save_draft", "updated_at_ms": 456]) { result in
            XCTAssertFalse((try? result.get().snapshot.unsavedChanges) ?? true); saved.fulfill()
        }
        wait(for: [saved], timeout: 5)
        worker.close(); EditorWorker.flush()

        let reopened = EditorWorker()
        let restored = expectation(description: "restore imported draft")
        reopened.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                      artifactID: fixture.id) { result in
            let value = try? result.get()
            XCTAssertEqual(value?.snapshot.layers.first?.id, importedLayerID)
            XCTAssertEqual(value?.snapshot.layers.first?.name, "Detached source")
            XCTAssertTrue(value?.snapshot.hasDraft == true)
            restored.fulfill()
        }
        wait(for: [restored], timeout: 5)
        reopened.close(); EditorWorker.flush()
    }

    func testRealBridgeImageTransformPixelsUndoRedoAndDraftReopen() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker()
        let opened = expectation(description: "open for transform")
        var layerID: String?
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            layerID = (try? result.get())?.snapshot.layers.first?.id
            opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let selectedID = try XCTUnwrap(layerID)
        let transform = expectation(description: "rotate clockwise")
        worker.request(["operation": "layer", "id": selectedID,
                        "edit": ["action": "image_transform", "transform": "rotate-clockwise"]]) {
            result in
            let value = try? result.get()
            XCTAssertEqual(value?.snapshot.artifactID, fixture.id)
            XCTAssertEqual(value?.snapshot.layers.first?.id, selectedID)
            XCTAssertEqual(value?.image.width, 3); XCTAssertEqual(value?.image.height, 7)
            if let image = value?.image {
                XCTAssertEqual(self.rgba(image, x: 0, y: 0), [0, 142, 19, 255])
                XCTAssertEqual(self.rgba(image, x: 2, y: 6), [186, 0, 19, 255])
            }
            transform.fulfill()
        }
        wait(for: [transform], timeout: 5)

        let undone = expectation(description: "undo transform")
        worker.request(["operation": "undo"]) { result in
            let value = try? result.get()
            XCTAssertEqual(value?.image.width, 7); XCTAssertEqual(value?.image.height, 3)
            if let image = value?.image {
                XCTAssertEqual(self.rgba(image, x: 6, y: 2), [186, 142, 19, 255])
            }
            undone.fulfill()
        }
        wait(for: [undone], timeout: 5)
        let redone = expectation(description: "redo transform")
        worker.request(["operation": "redo"]) { result in
            XCTAssertEqual((try? result.get().image.width), 3); redone.fulfill()
        }
        wait(for: [redone], timeout: 5)
        let saved = expectation(description: "save transformed draft")
        worker.request(["operation": "save_draft", "updated_at_ms": 789]) { result in
            XCTAssertFalse((try? result.get().snapshot.unsavedChanges) ?? true); saved.fulfill()
        }
        wait(for: [saved], timeout: 5)
        worker.close(); EditorWorker.flush()

        let reopened = EditorWorker()
        let restored = expectation(description: "reopen transformed draft")
        reopened.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                      artifactID: fixture.id) { result in
            let value = try? result.get()
            XCTAssertEqual(value?.snapshot.layers.first?.id, selectedID)
            XCTAssertTrue(value?.snapshot.hasDraft == true)
            XCTAssertEqual(value?.image.width, 3); XCTAssertEqual(value?.image.height, 7)
            restored.fulfill()
        }
        wait(for: [restored], timeout: 5)
        reopened.close(); EditorWorker.flush()
    }

    func testRealBridgeClosedShapePixelsClippingExpansionHistoryAndDraftReopen() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker()
        let opened = expectation(description: "open for shape creation")
        var original: EditorPresentation?
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            original = try? result.get(); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let originalID = try XCTUnwrap(original?.snapshot.layers.first?.id)

        let rectangle = expectation(description: "reverse partially clipped rectangle")
        var rectangleID: String?
        worker.request([
            "operation": "create_closed_shape", "shape": "rectangle",
            "start": ["x": 6.0, "y": 3.0], "end": ["x": 2.0, "y": -1.0],
        ]) { result in
            let value = try? result.get()
            rectangleID = value?.snapshot.layers.first?.id
            XCTAssertNotEqual(rectangleID, originalID)
            XCTAssertEqual(value?.snapshot.width, 7); XCTAssertEqual(value?.snapshot.height, 3,
                           "a partially clipped shape does not expand the canvas")
            if let image = value?.image {
                XCTAssertEqual(self.rgba(image, x: 3, y: 2), [255, 59, 92, 255])
            }
            rectangle.fulfill()
        }
        wait(for: [rectangle], timeout: 5)

        let undone = expectation(description: "undo shape")
        worker.request(["operation": "undo"]) { result in
            let value = try? result.get()
            XCTAssertEqual(value?.snapshot.layers.map(\.id), [originalID])
            if let image = value?.image {
                XCTAssertEqual(self.rgba(image, x: 3, y: 2), [93, 142, 19, 255])
            }
            undone.fulfill()
        }
        wait(for: [undone], timeout: 5)
        let redone = expectation(description: "redo shape")
        worker.request(["operation": "redo"]) { result in
            XCTAssertEqual((try? result.get())?.snapshot.layers.first?.id, rectangleID)
            redone.fulfill()
        }
        wait(for: [redone], timeout: 5)

        let ellipse = expectation(description: "fully outside ellipse expands")
        var expanded: EditorPresentation?
        worker.request([
            "operation": "create_closed_shape", "shape": "ellipse",
            "start": ["x": -20.0, "y": -15.0], "end": ["x": -16.0, "y": -11.0],
        ]) { result in
            expanded = try? result.get(); ellipse.fulfill()
        }
        wait(for: [ellipse], timeout: 5)
        let expandedValue = try XCTUnwrap(expanded)
        XCTAssertEqual(expandedValue.snapshot.width, 32)
        XCTAssertEqual(expandedValue.snapshot.height, 23)
        XCTAssertEqual(expandedValue.snapshot.layers.count, 3)
        XCTAssertNotEqual(expandedValue.snapshot.layers[0].id, rectangleID)
        XCTAssertEqual(rgba(expandedValue.image, x: 7, y: 7), [255, 59, 92, 255])
        XCTAssertEqual(rgba(expandedValue.image, x: 25, y: 20), [0, 0, 19, 255])

        let saved = expectation(description: "save shape draft")
        worker.request(["operation": "save_draft", "updated_at_ms": 987]) { result in
            XCTAssertFalse((try? result.get().snapshot.unsavedChanges) ?? true)
            saved.fulfill()
        }
        wait(for: [saved], timeout: 5)
        worker.close(); EditorWorker.flush()

        let reopened = EditorWorker()
        let restored = expectation(description: "reopen shape draft")
        reopened.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                      artifactID: fixture.id) { result in
            let value = try? result.get()
            XCTAssertTrue(value?.snapshot.hasDraft == true)
            XCTAssertEqual(value?.snapshot.width, 32); XCTAssertEqual(value?.snapshot.height, 23)
            XCTAssertEqual(value?.snapshot.layers.count, 3)
            if let image = value?.image {
                XCTAssertEqual(self.rgba(image, x: 7, y: 7), [255, 59, 92, 255])
                XCTAssertEqual(self.rgba(image, x: 25, y: 20), [0, 0, 19, 255])
            }
            restored.fulfill()
        }
        wait(for: [restored], timeout: 5)
        reopened.close(); EditorWorker.flush()
    }

    func testAnnotationFieldsPreservePrecisionAndLegacyValuesAndEmitMinimalPatch() throws {
        _ = NSApplication.shared
        let formatter = NumberFormatter()
        formatter.locale = Locale(identifier: "de_DE"); formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = false; formatter.maximumFractionDigits = 3
        let controls = EditorAnnotationControls(tokens: Tokens.variants["light-mustard"]!, formatter: formatter)
        var values = annotationStyle()
        values["color"] = "legacy-color"; values["strokeWidth"] = 8.123456
        let original = try XCTUnwrap(NativeAnnotationStyle(values))
        controls.setStyle(original); controls.setReady(true)
        var patches: [[String: Any]] = []
        var errors: [String] = []
        controls.apply = { patches.append($0) }; controls.reportError = { errors.append($0) }
        XCTAssertEqual(try field("Stroke width", in: controls).stringValue, "8,123")
        try button("Apply style", in: controls).performClick(nil)
        XCTAssertTrue(patches.isEmpty, "displaying resolved values is not an edit")
        try field("Shadow Y", in: controls).stringValue = "-12,75"
        try button("Apply style", in: controls).performClick(nil)
        XCTAssertEqual(patches.count, 1)
        XCTAssertEqual(patches[0] as NSDictionary, ["dropShadowStyle": ["offsetY": -12.75]] as NSDictionary)
        XCTAssertTrue(errors.isEmpty, "unchanged legacy colors are not revalidated")
        try field("Stroke color", in: controls).stringValue = "bad-new-color"
        try button("Apply style", in: controls).performClick(nil)
        XCTAssertEqual(patches.count, 1); XCTAssertEqual(errors.count, 1)
        try button("Reset fields", in: controls).performClick(nil)
        XCTAssertEqual(try field("Stroke color", in: controls).stringValue, "legacy-color")
        let picker = try XCTUnwrap(descendants(in: controls).compactMap { $0 as? ClosureColorWell }
            .first { $0.accessibilityLabel() == "Choose stroke color" })
        picker.color = NSColor(srgbRed: 0.2, green: 0.4, blue: 0.6, alpha: 1)
        _ = picker.sendAction(picker.action, to: picker.target)
        try button("Apply style", in: controls).performClick(nil)
        XCTAssertEqual(patches.last as NSDictionary?, ["color": "#336699"] as NSDictionary)

        controls.setStyle(original)
        try field("Shadow blur", in: controls).stringValue = "99"
        try annotationToggle("Shadow", in: controls).performClick(nil)
        try annotationToggle("Fill", in: controls).performClick(nil)
        try button("Apply style", in: controls).performClick(nil)
        XCTAssertEqual(patches.last as NSDictionary?, ["dropShadow": false, "fill": NSNull()] as NSDictionary)
        XCTAssertTrue(try field("Shadow blur", in: controls).isHiddenOrHasHiddenAncestor)

        values["closed"] = false
        controls.setStyle(try XCTUnwrap(NativeAnnotationStyle(values)))
        XCTAssertTrue(try annotationToggle("Stroke", in: controls).isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(try annotationToggle("Fill", in: controls).isHiddenOrHasHiddenAncestor)
        try field("Stroke width", in: controls).stringValue = "3,25"
        try button("Apply style", in: controls).performClick(nil)
        XCTAssertEqual(patches.last as NSDictionary?, ["strokeWidth": 3.25] as NSDictionary)
        controls.setReady(false)
        XCTAssertFalse(try button("Apply style", in: controls).isEnabled)
    }

    func testAnnotationSelectionBusyFailureAndOutputInvalidation() throws {
        _ = NSApplication.shared
        var hidden = shapeLayer(id: "shape", x: 5, y: 7)
        hidden["locked"] = true; hidden["visible"] = false
        let published = snapshot(id: "shot", layers: [
            layer(id: "image", name: "Original", x: 0, y: 0, visible: true, locked: true, opacity: 100), hidden,
        ], annotations: ["shape": annotationStyle()])
        let worker = FakeEditorWorker(snapshot: published)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        try button("Preview output", in: controller.root).performClick(nil)
        let preview = try segmented("Output preview image", in: controller.root)
        XCTAssertTrue(preview.isEnabled)
        try showLayers(in: controller.root)
        try button("Apply style", in: controller.root).performClick(nil)
        XCTAssertTrue(worker.requests.isEmpty); XCTAssertTrue(preview.isEnabled)
        worker.deferRequests = true
        try field("Fill color", in: controller.root).stringValue = "#2c4"
        try button("Apply style", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["id"] as? String, "shape")
        let edit = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
        XCTAssertEqual(edit as NSDictionary, ["action": "annotation_style", "patch": ["fill": "#22CC44"]] as NSDictionary)
        XCTAssertTrue(controller.state.busy); XCTAssertFalse(preview.isEnabled)
        XCTAssertFalse(try field("Fill color", in: controller.root).isEnabled)
        XCTAssertFalse(try table("Screenshot layers", in: controller.root).isEnabled)
        worker.completePending(with: published)
        XCTAssertEqual(try field("Fill color", in: controller.root).stringValue, "#E04090")
        worker.deferRequests = false; worker.failLayerAction = "annotation_style"
        try field("Fill color", in: controller.root).stringValue = "#123456"
        try button("Apply style", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.state.busy)
        XCTAssertEqual(try field("Fill color", in: controller.root).stringValue, "#E04090")
        XCTAssertEqual(try table("Screenshot layers", in: controller.root).selectedRow, 0)
        try field("Fill color", in: controller.root).stringValue = "#abcdef"
        let table = try table("Screenshot layers", in: controller.root)
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        XCTAssertTrue(try button("Apply style", in: controller.root).isHiddenOrHasHiddenAncestor)
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        XCTAssertEqual(try field("Fill color", in: controller.root).stringValue, "#E04090")
    }

    func testAnnotationRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: true,
                layers: [shapeLayer(id: "shape", x: 5, y: 7)], annotations: ["shape": annotationStyle()]))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showLayers(in: controller.root)
            let apply = try button("Apply style", in: controller.root)
            let scroll = try XCTUnwrap(apply.enclosingScrollView)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: 550))
            scroll.reflectScrolledClipView(scroll.contentView)
            controller.root.layoutSubtreeIfNeeded()
            for field in descendants(in: controller.root).compactMap({ $0 as? NSTextField })
                where ["Shadow opacity", "Stroke color", "Shadow color"].contains(field.stringValue) {
                XCTAssertLessThanOrEqual(field.intrinsicContentSize.width, field.frame.width)
            }
            try render(controller.root, name: "screenshot-editor-style-\(appearance)")
            let document = try XCTUnwrap(scroll.documentView)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: document.bounds.height - scroll.contentView.bounds.height))
            scroll.reflectScrolledClipView(scroll.contentView)
            XCTAssertTrue(scroll.contentView.bounds.contains(apply.convert(apply.bounds, to: scroll.contentView)),
                          "style actions remain reachable in the minimum window")
            worker.failLayerAction = "annotation_style"
            worker.failureMessage = "The annotation style could not be applied. The previous draft, layer selection, pixels and undo history remain recoverable."
            try field("Fill color", in: controller.root).stringValue = "#00aa44"
            try button("Apply style", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-style-error-minimum-\(appearance)")
            try annotationToggle("Shadow", in: controller.root).performClick(nil)
            try annotationToggle("Fill", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-style-disabled-\(appearance)")
        }
    }

    func testRealBridgeAnnotationPixelsUndoRedoAndDraftReopen() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker()
        defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open annotation fixture")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            XCTAssertNotNil(try? result.get()); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        func request(_ object: [String: Any]) throws -> EditorPresentation {
            let done = expectation(description: "annotation request")
            var response: Result<EditorPresentation, Error>?
            worker.request(object) { result in response = result; done.fulfill() }
            wait(for: [done], timeout: 5)
            return try XCTUnwrap(response).get()
        }
        _ = try request(["operation": "resize_canvas", "width": 32, "height": 24])
        let created = try request(["operation": "create_closed_shape", "shape": "rectangle",
                                  "start": ["x": 5, "y": 7], "end": ["x": 15, "y": 17]])
        let shape = try XCTUnwrap(created.snapshot.layers.first)
        let original = try XCTUnwrap(shape.annotation)
        XCTAssertFalse(original.strokeEnabled); XCTAssertFalse(original.dropShadow)
        var edited = original
        edited.fill = "#E04090"; edited.shadowColor = "#20C060"
        edited.shadowOpacity = 100; edited.shadowBlur = 0
        edited.shadowX = 8; edited.shadowY = -3; edited.dropShadow = true
        let styled = try request(["operation": "layer", "id": shape.id,
                                 "edit": ["action": "annotation_style", "patch": edited.patch(from: original)]])
        XCTAssertEqual(styled.snapshot.layers.first?.id, shape.id)
        XCTAssertEqual(styled.snapshot.layers.first?.annotation, edited)
        XCTAssertEqual(rgba(styled.image, x: 10, y: 12), [224, 64, 144, 255])
        XCTAssertEqual(rgba(styled.image, x: 20, y: 10), [32, 192, 96, 255])
        let undone = try request(["operation": "undo"])
        XCTAssertEqual(undone.snapshot.layers.first?.annotation, original)
        XCTAssertEqual(rgba(undone.image, x: 10, y: 12), [255, 59, 92, 255])
        // Outside the original 7×3 image, Undo reveals the #f7f7f5 canvas.
        XCTAssertEqual(rgba(undone.image, x: 20, y: 10), [247, 247, 245, 255])
        _ = try request(["operation": "redo"])
        _ = try request(["operation": "save_draft", "updated_at_ms": 1357])
        worker.close(); EditorWorker.flush()
        let reopened = expectation(description: "reopen styled draft")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            let value = try? result.get()
            XCTAssertEqual(value?.snapshot.layers.first?.annotation, edited)
            XCTAssertTrue(value?.snapshot.hasDraft == true)
            if let image = value?.image {
                XCTAssertEqual(self.rgba(image, x: 20, y: 10), [32, 192, 96, 255])
            }
            reopened.fulfill()
        }
        wait(for: [reopened], timeout: 5)
    }

    func testOpenDrawingThresholdsAndPenSamplingRestoreMouseCoalescing() throws {
        _ = NSApplication.shared
        let coalescing = NSEvent.isMouseCoalescingEnabled
        defer { NSEvent.isMouseCoalescingEnabled = coalescing }
        let overlay = EditorDrawOverlay(frame: NSRect(x: 0, y: 0, width: 200, height: 120))
        overlay.canvasSize = NSSize(width: 400, height: 200); overlay.drawingEnabled = true
        var completed: [(EditorDrawOverlay.Shape, NSPoint, NSPoint, [NSPoint])] = []
        overlay.onComplete = { completed.append(($0, $1, $2, $3)) }
        overlay.shape = .line
        for end in [NSPoint(x: 80, y: 30), NSPoint(x: 20, y: 80), NSPoint(x: 20, y: 30)] {
            overlay.begin(at: NSPoint(x: 20, y: 30)); overlay.end(at: end)
        }
        XCTAssertEqual(completed.count, 3, "axis-aligned and zero-length lines survive")
        XCTAssertEqual(completed[0].1, NSPoint(x: 40, y: 40))
        XCTAssertEqual(completed[0].2, NSPoint(x: 160, y: 40))
        overlay.shape = .arrow
        for (size, below, accepted) in [(NSSize(width: 400, height: 200), 2.99, 3.0),
                                         (NSSize(width: 50, height: 25), 5.99, 6.0)] {
            overlay.canvasSize = size
            let count = completed.count
            overlay.begin(at: NSPoint(x: 20, y: 30)); overlay.end(at: NSPoint(x: 20 + below, y: 30))
            XCTAssertEqual(completed.count, count)
            overlay.begin(at: NSPoint(x: 20, y: 30)); overlay.end(at: NSPoint(x: 20 + accepted, y: 30))
            XCTAssertEqual(completed.count, count + 1, "both screen and shared document minima apply")
        }
        overlay.canvasSize = NSSize(width: 400, height: 200); overlay.shape = .pen
        NSEvent.isMouseCoalescingEnabled = true
        overlay.begin(at: NSPoint(x: 20, y: 30))
        XCTAssertFalse(NSEvent.isMouseCoalescingEnabled)
        overlay.drag(to: NSPoint(x: 21.49, y: 30))
        overlay.drag(to: NSPoint(x: 21.5, y: 30))
        overlay.drag(to: NSPoint(x: -5, y: 50))
        overlay.end(at: NSPoint(x: 99, y: 99))
        XCTAssertEqual(completed.last?.3, [NSPoint(x: 40, y: 40), NSPoint(x: 43, y: 40), NSPoint(x: -10, y: 80)])
        XCTAssertTrue(NSEvent.isMouseCoalescingEnabled)
        overlay.begin(at: NSPoint(x: 30, y: 35)); overlay.end(at: NSPoint(x: 90, y: 90))
        XCTAssertEqual(completed.last?.3, [NSPoint(x: 60, y: 50)], "click-only Pen remains a dot")
        let count = completed.count
        for originallyCoalesced in [false, true] {
            NSEvent.isMouseCoalescingEnabled = originallyCoalesced
            overlay.begin(at: NSPoint(x: 20, y: 30)); overlay.shape = .arrow
            XCTAssertEqual(NSEvent.isMouseCoalescingEnabled, originallyCoalesced)
            overlay.end(at: NSPoint(x: 90, y: 90)); overlay.shape = .pen
            overlay.begin(at: NSPoint(x: 20, y: 30)); _ = overlay.resignFirstResponder()
            XCTAssertEqual(NSEvent.isMouseCoalescingEnabled, originallyCoalesced)
            overlay.end(at: NSPoint(x: 90, y: 90))
        }
        XCTAssertEqual(completed.count, count, "cancellation never commits partial samples")
        let geometry = try XCTUnwrap(NativeEditorDrawGeometry(arrow: false,
            samples: [CGPoint(x: 2, y: 3), CGPoint(x: 10, y: 19), CGPoint(x: 26, y: 7)]))
        XCTAssertEqual(geometry.points.count, 26)
        XCTAssertEqual(geometry.points[12], CGPoint(x: 10, y: 13.5))
        XCTAssertEqual(geometry.strokeWidth, 8)
    }

    func testCanvasSelectionMapsFittedCoordinatesAndDistinguishesClickFromMove() {
        _ = NSApplication.shared
        let overlay = EditorSelectionOverlay(frame: NSRect(x: 0, y: 0, width: 200, height: 120))
        overlay.canvasSize = NSSize(width: 400, height: 200)
        overlay.selectionEnabled = true
        var hits: [(CGPoint, Double)] = []
        var selections: [String?] = []
        var moves: [(String, CGFloat, CGFloat)] = []
        overlay.hitTestLayer = { point, tolerance in hits.append((point, tolerance)); return "front" }
        overlay.onSelect = { selections.append($0) }
        overlay.onMove = { id, dx, dy, _ in moves.append((id, dx, dy)) }
        let front = ["kind": "shape", "id": "front", "shape": "rectangle", "x": 20.0, "y": 20.0,
                     "endX": 200.0, "endY": 150.0, "controls": [], "locked": false, "visible": true,
                     "opacity": 100.0, "blendMode": "source-over",
                     "style": ["color": "#ff3b5c", "fill": "#ff3b5c", "strokeWidth": 8.0]] as [String: Any]
        overlay.documentJSON = String(decoding: try! JSONSerialization.data(withJSONObject:
            ["width": 400.0, "height": 200.0, "elements": [front]]), as: UTF8.self)

        // The non-square canvas is fitted to 200 × 100 with ten-point letterboxing.
        overlay.begin(at: CGPoint(x: 20, y: 30)); overlay.end(at: CGPoint(x: 22.9, y: 30))
        XCTAssertEqual(hits.count, 1); XCTAssertEqual(hits[0].0, CGPoint(x: 40, y: 40))
        XCTAssertEqual(hits[0].1, 16); XCTAssertEqual(selections.count, 1); XCTAssertTrue(moves.isEmpty)
        overlay.begin(at: CGPoint(x: 80, y: 60)); overlay.drag(to: CGPoint(x: 76, y: 67))
        overlay.end(at: CGPoint(x: 76, y: 67))
        XCTAssertEqual(hits.count, 2, "hit testing occurs only on press")
        XCTAssertEqual(moves.first?.0, "front"); XCTAssertEqual(moves.first?.1, -8)
        XCTAssertEqual(moves.first?.2, 14)
        overlay.begin(at: CGPoint(x: 20, y: 5)); overlay.end(at: CGPoint(x: 80, y: 80))
        XCTAssertEqual(hits.count, 2, "starts outside the fitted image are ignored")
    }

    func testCanvasSelectionEmptyErrorAndCancellationNeverCommit() {
        let overlay = EditorSelectionOverlay(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
        overlay.canvasSize = NSSize(width: 100, height: 100); overlay.selectionEnabled = true
        var selections: [String?] = []; var moves = 0; var errors = 0
        overlay.onSelect = { selections.append($0) }; overlay.onMove = { _, _, _, _ in moves += 1 }
        overlay.onError = { _ in errors += 1 }; overlay.hitTestLayer = { _, _ in nil }
        overlay.begin(at: CGPoint(x: 20, y: 20)); overlay.end(at: CGPoint(x: 20, y: 20))
        XCTAssertEqual(selections.count, 1); XCTAssertNil(selections[0])
        overlay.hitTestLayer = { _, _ in throw AppBridgeError.backend("unsupported geometry") }
        overlay.begin(at: CGPoint(x: 30, y: 30)); overlay.end(at: CGPoint(x: 50, y: 50))
        XCTAssertEqual(errors, 1); XCTAssertEqual(moves, 0); XCTAssertEqual(selections.count, 1)
        overlay.hitTestLayer = { _, _ in "layer" }
        let layer = ["kind": "shape", "id": "layer", "shape": "rectangle", "x": 10.0, "y": 10.0,
                     "endX": 80.0, "endY": 80.0, "controls": [], "locked": false, "visible": true,
                     "opacity": 100.0, "blendMode": "source-over",
                     "style": ["color": "#ff3b5c", "fill": "#ff3b5c", "strokeWidth": 8.0]] as [String: Any]
        overlay.documentJSON = String(decoding: try! JSONSerialization.data(withJSONObject:
            ["width": 100.0, "height": 100.0, "elements": [layer]]), as: UTF8.self)
        overlay.begin(at: CGPoint(x: 30, y: 30)); overlay.cancelGesture(); overlay.end(at: CGPoint(x: 60, y: 60))
        XCTAssertEqual(moves, 0)
        overlay.begin(at: CGPoint(x: 30, y: 30)); overlay.setFrameSize(NSSize(width: 120, height: 100))
        overlay.end(at: CGPoint(x: 60, y: 60))
        overlay.begin(at: CGPoint(x: 30, y: 30)); overlay.selectionEnabled = false
        overlay.end(at: CGPoint(x: 60, y: 60))
        XCTAssertEqual(moves, 0); XCTAssertEqual(selections.count, 1)
    }

    func testCanvasMovePreviewSnapsNearEdgesWithStrictThresholdAndMultipleGuides() throws {
        let moving: [String: Any] = [
            "kind": "image", "id": "moving", "source": "imported", "src": "draft-asset:image",
            "name": "image.png", "x": 41.25, "y": 37.5, "width": 83.5, "height": 46.25,
            "naturalWidth": 183.0, "naturalHeight": 86.5, "locked": false, "visible": true,
            "opacity": 73.0, "blendMode": "multiply",
        ]
        var sibling = moving
        sibling["id"] = "same-size"; sibling["x"] = 190.0; sibling["y"] = 130.0
        let data = try JSONSerialization.data(withJSONObject:
            ["width": 300.0, "height": 220.0, "elements": [moving, sibling]])
        let json = String(decoding: data, as: UTF8.self)
        let drag = try NativeEditorMoveDrag.begin(documentJSON: json, layerID: "moving", displayScale: 1)

        let exact = try XCTUnwrap(drag.preview(delta: CGPoint(x: -31.25, y: -27.5)))
        XCTAssertEqual(exact.outline[0], CGPoint(x: 10, y: 10),
                       "the exact ten-view-point threshold is strict")
        XCTAssertTrue(exact.guides.isEmpty)
        let nearEdge = try XCTUnwrap(drag.preview(delta: CGPoint(x: -31.251, y: -27.501)))
        XCTAssertEqual(nearEdge.outline[0], .zero)
        XCTAssertEqual(nearEdge.guides, [
            NativeEditorAlignmentGuide(orientation: .vertical, position: 0),
            NativeEditorAlignmentGuide(orientation: .horizontal, position: 0),
        ])
        let multiple = try XCTUnwrap(drag.preview(delta: CGPoint(x: 148.4, y: 92.4)))
        XCTAssertEqual(multiple.outline[0].x, 190, accuracy: 1e-7)
        XCTAssertEqual(multiple.outline[0].y, 130, accuracy: 1e-7)
        XCTAssertEqual(multiple.guides.count, 4)
        for (guide, expected) in zip(multiple.guides, [190.0, 273.5, 130, 176.25]) {
            XCTAssertEqual(guide.position, expected, accuracy: 1e-7)
        }

        // A layer already inside the magnetic range must not jump on a click,
        // or when a drag returns below the three-view-point movement threshold.
        var near = moving; near["x"] = 5.0
        let overlay = EditorSelectionOverlay(frame: NSRect(x: 0, y: 0, width: 300, height: 220))
        overlay.canvasSize = NSSize(width: 300, height: 220); overlay.selectionEnabled = true
        overlay.documentJSON = String(decoding: try JSONSerialization.data(withJSONObject:
            ["width": 300.0, "height": 220.0, "elements": [near]]), as: UTF8.self)
        overlay.hitTestLayer = { _, _ in "moving" }
        overlay.begin(at: CGPoint(x: 40, y: 50))
        XCTAssertNil(overlay.movePreview)
        overlay.drag(to: CGPoint(x: 42.9, y: 50))
        XCTAssertNil(overlay.movePreview)
        overlay.drag(to: CGPoint(x: 44, y: 50))
        XCTAssertEqual(overlay.movePreview?.outline[0].x, 0)
        overlay.drag(to: CGPoint(x: 41, y: 50))
        XCTAssertNil(overlay.movePreview)
    }

    func testCanvasRotationGripHasPriorityUsesReleaseAndNoOpDoesNotCommit() throws {
        let overlay = EditorSelectionOverlay(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
        overlay.canvasSize = NSSize(width: 300, height: 200); overlay.selectionEnabled = true
        let outline = [CGPoint(x: 100, y: 70), CGPoint(x: 160, y: 70),
                       CGPoint(x: 160, y: 120), CGPoint(x: 100, y: 120)]
        overlay.selectedOutline = outline; overlay.selectedLayerID = "selected"
        overlay.rotationEnabled = true
        let grip = try XCTUnwrap(NativeEditorRotationHandle(outline: outline, radians: 0,
            displayScale: 1, canvas: overlay.canvasSize))
        var bodyHits = 0; var rotations: [(String, Double)] = []
        overlay.hitTestLayer = { _, _ in bodyHits += 1; return "overlapping-front" }
        overlay.onRotate = { rotations.append(($0, $1)) }

        overlay.begin(at: grip.handle); overlay.end(at: grip.handle)
        XCTAssertEqual(bodyHits, 0, "the shared grip wins over an overlapping layer body")
        XCTAssertTrue(rotations.isEmpty, "a plain grip click does not create history")
        overlay.begin(at: grip.handle)
        overlay.drag(to: CGPoint(x: grip.handle.x + 24, y: grip.handle.y + 31), snap: true)
        overlay.end(at: CGPoint(x: grip.handle.x + 31, y: grip.handle.y + 24), snap: false)
        XCTAssertEqual(rotations.count, 1)
        XCTAssertEqual(rotations[0].0, "selected")
        XCTAssertEqual(rotations[0].1, atan2(31.0, 29.0), accuracy: 1e-12)
        overlay.begin(at: grip.handle)
        let angle = 40.0 * Double.pi / 180
        let current = CGPoint(x: 130 + 53 * sin(angle), y: 95 - 53 * cos(angle))
        overlay.drag(to: current)
        func flags(_ modifiers: NSEvent.ModifierFlags) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(with: .flagsChanged, location: .zero,
                modifierFlags: modifiers, timestamp: 0, windowNumber: 0, context: nil,
                characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: 56))
        }
        overlay.flagsChanged(with: try flags(.shift))
        XCTAssertEqual(try XCTUnwrap(overlay.rotationPreview?.radians), Double.pi / 4, accuracy: 1e-12)
        overlay.flagsChanged(with: try flags([]))
        XCTAssertEqual(try XCTUnwrap(overlay.rotationPreview?.radians), angle, accuracy: 1e-12)
        overlay.end(at: current)
        XCTAssertEqual(rotations[1].1, angle, accuracy: 1e-12)
    }

    func testCanvasRotationCancellationNeverCommits() throws {
        let overlay = EditorSelectionOverlay(frame: NSRect(x: 0, y: 0, width: 200, height: 200))
        overlay.canvasSize = NSSize(width: 200, height: 200); overlay.selectionEnabled = true
        let outline = [CGPoint(x: 70, y: 70), CGPoint(x: 130, y: 70),
                       CGPoint(x: 130, y: 120), CGPoint(x: 70, y: 120)]
        overlay.selectedOutline = outline; overlay.selectedLayerID = "layer"; overlay.rotationEnabled = true
        let grip = try XCTUnwrap(NativeEditorRotationHandle(outline: outline, radians: 0,
            displayScale: 1, canvas: overlay.canvasSize))
        var commits = 0; overlay.onRotate = { _, _ in commits += 1 }
        for cancel in [{ overlay.cancelGesture() },
                       { overlay.selectionEnabled = false },
                       { overlay.setFrameSize(NSSize(width: 201, height: 200)) }] {
            overlay.selectionEnabled = true; overlay.begin(at: grip.handle)
            overlay.drag(to: CGPoint(x: grip.handle.x + 20, y: grip.handle.y + 20)); cancel()
            overlay.end(at: CGPoint(x: grip.handle.x + 30, y: grip.handle.y + 20))
        }
        XCTAssertEqual(commits, 0)
    }

    func testCanvasResizeEightHandlesPrecedeBodyAndUseCornerShiftSemantics() throws {
        let overlay = EditorSelectionOverlay(frame: NSRect(x: 0, y: 0, width: 300, height: 220))
        overlay.canvasSize = NSSize(width: 300, height: 220); overlay.selectionEnabled = true
        let element: [String: Any] = [
            "kind": "shape", "id": "selected", "shape": "rectangle", "x": 100.0, "y": 70.0,
            "endX": 180.0, "endY": 130.0, "controls": [], "locked": false, "visible": true,
            "opacity": 100.0, "blendMode": "source-over",
            "style": ["color": "#ff3b5c", "fill": "#ff3b5c", "strokeWidth": 8.0],
        ]
        let document: [String: Any] = ["width": 300.0, "height": 220.0, "elements": [element]]
        overlay.documentJSON = String(decoding: try JSONSerialization.data(withJSONObject: document), as: UTF8.self)
        overlay.selectedLayerID = "selected"
        overlay.selectedOutline = [CGPoint(x: 95, y: 65), CGPoint(x: 185, y: 65),
                                   CGPoint(x: 185, y: 135), CGPoint(x: 95, y: 135)]
        overlay.resizeEnabled = true
        XCTAssertEqual(overlay.resizeHandlePoints.count, 8)
        var bodyHits = 0
        var commits: [(String, Bool)] = []
        overlay.hitTestLayer = { _, _ in bodyHits += 1; return "overlap" }
        overlay.onResize = { _, handle, _, _, lock in commits.append((handle, lock)) }
        for (index, grip) in overlay.resizeHandlePoints.enumerated() {
            overlay.begin(at: grip, snap: true)
            overlay.end(at: CGPoint(x: grip.x + 12, y: grip.y + 9), snap: true)
            XCTAssertEqual(commits[index].0, EditorSelectionOverlay.resizeHandleNames[index])
            XCTAssertEqual(commits[index].1, index.isMultiple(of: 2),
                           "Shift locks only corner resize handles")
        }
        XCTAssertEqual(bodyHits, 0, "resize grips win over an overlapping layer body")
    }

    func testCanvasResizeKeepsOriginalDragAcrossShiftAndCancellationOrClickNeverCommits() throws {
        let overlay = EditorSelectionOverlay(frame: NSRect(x: 0, y: 0, width: 300, height: 220))
        overlay.canvasSize = NSSize(width: 300, height: 220); overlay.selectionEnabled = true
        let element: [String: Any] = [
            "kind": "shape", "id": "selected", "shape": "rectangle", "x": 100.0, "y": 70.0,
            "endX": 180.0, "endY": 130.0, "controls": [], "locked": false, "visible": true,
            "opacity": 100.0, "blendMode": "source-over",
            "style": ["color": "#ff3b5c", "fill": "#ff3b5c", "strokeWidth": 8.0],
        ]
        let document: [String: Any] = ["width": 300.0, "height": 220.0, "elements": [element]]
        overlay.documentJSON = String(decoding: try JSONSerialization.data(withJSONObject: document), as: UTF8.self)
        overlay.selectedLayerID = "selected"
        overlay.selectedOutline = [CGPoint(x: 95, y: 65), CGPoint(x: 185, y: 65),
                                   CGPoint(x: 185, y: 135), CGPoint(x: 95, y: 135)]
        overlay.resizeEnabled = true
        let grip = try XCTUnwrap(overlay.resizeHandlePoints.first)
        let current = CGPoint(x: grip.x - 23, y: grip.y - 11)
        var commits = 0; overlay.onResize = { _, _, _, _, _ in commits += 1 }
        overlay.begin(at: grip); overlay.drag(to: current, snap: false)
        let unlocked = try XCTUnwrap(overlay.resizePreview).outline
        func flags(_ modifiers: NSEvent.ModifierFlags) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(with: .flagsChanged, location: .zero,
                modifierFlags: modifiers, timestamp: 0, windowNumber: 0, context: nil,
                characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: 56))
        }
        overlay.flagsChanged(with: try flags(.shift))
        XCTAssertNotEqual(overlay.resizePreview?.outline, unlocked)
        overlay.flagsChanged(with: try flags([]))
        XCTAssertEqual(overlay.resizePreview?.outline, unlocked,
                       "modifier changes reuse the immutable original drag")
        overlay.cancelGesture(); overlay.end(at: current)
        overlay.begin(at: grip); overlay.end(at: grip)
        XCTAssertEqual(commits, 0)
        overlay.begin(at: grip); overlay.drag(to: current); _ = overlay.resignFirstResponder(); overlay.end(at: current)
        XCTAssertEqual(commits, 0)
    }

    func testSnapshotParsesRotatedSelectionOutlineAndCachesSortedDocument() throws {
        let element = shapeLayer(id: "rotated", x: 10, y: 20)
        var rotatedElement = element
        rotatedElement["rotation"] = Double.pi / 3
        let value: [String: Any] = [
            "artifact_id": "shot", "document": ["height": 100, "elements": [rotatedElement], "width": 200],
            "selection_outlines": ["rotated": [
                ["x": 12, "y": 4], ["x": 26, "y": 18], ["x": 12, "y": 32], ["x": -2, "y": 18],
            ]],
            "can_undo": false, "can_redo": false, "unsaved_changes": false, "has_draft": false,
        ]
        let parsed = try XCTUnwrap(NativeEditorSnapshot(value))
        XCTAssertEqual(parsed.layers[0].selectionOutline,
                       [CGPoint(x: 12, y: 4), CGPoint(x: 26, y: 18),
                        CGPoint(x: 12, y: 32), CGPoint(x: -2, y: 18)])
        XCTAssertEqual(parsed.layers[0].rotation, Double.pi / 3)
        XCTAssertTrue(parsed.documentJSON.hasPrefix("{\"elements\""), "document JSON uses sorted keys")
        XCTAssertNil(snapshot(id: "old", layers: [element]).layers[0].selectionOutline)
    }

    func testCanvasSelectionControllerKeepsOutputUntilMoveAndPreservesSelectionOnFailure() throws {
        _ = NSApplication.shared
        func shape(_ id: String, x: Double, locked: Bool = false, visible: Bool = true) -> [String: Any] {
            ["kind": "shape", "id": id, "shape": "rectangle", "x": x, "y": 40.0,
             "endX": x + 60, "endY": 110.0, "controls": [], "locked": locked,
             "visible": visible, "opacity": 100.0, "blendMode": "source-over",
             "style": ["color": "#ff3b5c", "fill": "#ff3b5c", "strokeWidth": 8.0]]
        }
        let published = snapshot(id: "shot", layers: [shape("left", x: 40), shape("right", x: 240),
            shape("locked", x: 40, locked: true), shape("hidden", x: 40, visible: false)])
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: published)
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showOutput(in: controller.root)
            try button("Preview output", in: controller.root).performClick(nil)
            let output = try segmented("Output preview image", in: controller.root)
            try showLayers(in: controller.root)
            controller.root.layoutSubtreeIfNeeded()
            let overlay = controller.selectionOverlay
            let rect = overlay.presentedImageRect, scale = rect.width / 640
            func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: rect.minX + x * scale, y: rect.minY + y * scale) }
            overlay.begin(at: point(70, 70)); overlay.end(at: point(70, 70))
            XCTAssertEqual(try field("Layer X", in: controller.root).stringValue, "40")
            XCTAssertTrue(output.isEnabled); XCTAssertTrue(worker.requests.isEmpty)
            overlay.begin(at: point(550, 250)); overlay.end(at: point(550, 250))
            XCTAssertEqual(try table("Screenshot layers", in: controller.root).selectedRow, -1)
            XCTAssertTrue(output.isEnabled)
            XCTAssertEqual(try field("Layer X", in: controller.root).stringValue, "")
            XCTAssertFalse(try button("Move", in: controller.root).isEnabled)
            try render(controller.root, name: "screenshot-editor-canvas-empty-selection-\(appearance)")
            overlay.begin(at: point(70, 70)); overlay.end(at: point(70, 70))
            overlay.begin(at: point(270, 70)); overlay.drag(to: point(290, 90))
            try showOutput(in: controller.root)
            overlay.end(at: point(290, 90))
            XCTAssertTrue(worker.requests.isEmpty)
            try showLayers(in: controller.root)
            overlay.begin(at: point(270, 70)); overlay.drag(to: point(290, 90))
            controller.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification))
            overlay.end(at: point(290, 90)); XCTAssertTrue(worker.requests.isEmpty)
            worker.deferRequests = true
            overlay.begin(at: point(270, 70)); overlay.end(at: point(290, 100))
            XCTAssertEqual(worker.requests.count, 1)
            XCTAssertEqual(worker.requests[0]["id"] as? String, "right")
            let edit = try XCTUnwrap(worker.requests[0]["edit"] as? [String: Any])
            XCTAssertEqual(edit["action"] as? String, "drag_move")
            XCTAssertEqual(try XCTUnwrap(edit["delta_x"] as? Double), 20, accuracy: 1e-7)
            XCTAssertEqual(try XCTUnwrap(edit["delta_y"] as? Double), 30, accuracy: 1e-7)
            XCTAssertEqual(try XCTUnwrap(edit["display_scale"] as? Double), scale, accuracy: 1e-7)
            XCTAssertFalse(output.isEnabled); XCTAssertTrue(controller.state.busy)
            XCTAssertFalse(overlay.selectionEnabled)
            XCTAssertEqual(try field("Layer X", in: controller.root).stringValue, "40")
            worker.completePending(with: published)
            XCTAssertEqual(try field("Layer X", in: controller.root).stringValue, "240")
            worker.deferRequests = false; worker.failLayerAction = "drag_move"
            overlay.begin(at: point(70, 70)); overlay.end(at: point(90, 90))
            XCTAssertEqual(try field("Layer X", in: controller.root).stringValue, "240")
            XCTAssertFalse(controller.state.busy)
            try render(controller.root, name: "screenshot-editor-move-error-minimum-\(appearance)")
        }
    }

    func testRealCanvasMovePixelsUndoDraftAndRenderedOutlines() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let fixture = try makeHistoryFixture()
            defer { try? FileManager.default.removeItem(at: fixture.root) }
            let worker = EditorWorker()
            let opened = expectation(description: "canvas fixture")
            worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
                result in XCTAssertNotNil(try? result.get()); opened.fulfill()
            }
            wait(for: [opened], timeout: 5)
            func request(_ object: [String: Any], using worker: EditorWorker) throws -> EditorPresentation {
                let done = expectation(description: "canvas request")
                var response: Result<EditorPresentation, Error>?
                worker.request(object) { result in response = result; done.fulfill() }
                wait(for: [done], timeout: 5)
                return try XCTUnwrap(response).get()
            }
            _ = try request(["operation": "resize_canvas", "width": 640, "height": 360], using: worker)
            let created = try request(["operation": "create_closed_shape", "shape": "rectangle",
                "start": ["x": 100, "y": 80], "end": ["x": 200, "y": 160]], using: worker)
            let id = try XCTUnwrap(created.snapshot.layers.first?.id)
            _ = try request(["operation": "save_draft", "updated_at_ms": 4000], using: worker)
            worker.close(); EditorWorker.flush()
            let live = EditorWorker()
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: live)
            defer { controller.window.orderOut(nil); live.close(); EditorWorker.flush() }
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot != nil && !controller.state.busy }
            try showLayers(in: controller.root); controller.root.layoutSubtreeIfNeeded()
            let overlay = controller.selectionOverlay
            let rect = overlay.presentedImageRect, scale = rect.width / 640
            let start = CGPoint(x: rect.minX + 150 * scale, y: rect.minY + 120 * scale)
            let end = CGPoint(x: start.x - 95 * scale, y: start.y - 75 * scale)
            overlay.begin(at: start); overlay.drag(to: end)
            XCTAssertFalse(controller.state.snapshot!.unsavedChanges)
            XCTAssertEqual(overlay.movePreview?.outline.first, .zero,
                           "a raw near-edge move previews the independently derived snapped outline")
            try render(controller.root, name: "screenshot-editor-move-active-\(appearance)")
            overlay.end(at: end)
            waitUntil { !controller.state.busy && controller.state.snapshot!.unsavedChanges }
            let moved = try request(["operation": "snapshot"], using: live)
            XCTAssertEqual(moved.snapshot.layers.first?.id, id)
            // The default ten-pixel stroke extends five pixels outside the shape.
            XCTAssertEqual(try XCTUnwrap(moved.snapshot.layers.first?.x), 5, accuracy: 1e-7)
            XCTAssertEqual(try XCTUnwrap(moved.snapshot.layers.first?.y), 5, accuracy: 1e-7)
            XCTAssertEqual(rgba(moved.image, x: 110, y: 90), [247, 247, 245, 255])
            XCTAssertEqual(rgba(moved.image, x: 10, y: 10), [255, 59, 92, 255])
            try render(controller.root, name: "screenshot-editor-move-committed-\(appearance)")
            try button("Undo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.canRedo == true }
            let undone = try request(["operation": "snapshot"], using: live)
            XCTAssertEqual(rgba(undone.image, x: 110, y: 90), [255, 59, 92, 255])
            XCTAssertEqual(rgba(undone.image, x: 10, y: 10), [247, 247, 245, 255])
            try button("Redo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.canRedo == false }
            try button("Save draft", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.unsavedChanges == false }
            live.close(); EditorWorker.flush()
            let reopened = EditorWorker()
            let done = expectation(description: "reopen moved canvas")
            reopened.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) { result in
                if let value = try? result.get() {
                    XCTAssertEqual(value.snapshot.layers.first?.id, id)
                    XCTAssertEqual(self.rgba(value.image, x: 10, y: 10), [255, 59, 92, 255])
                } else { XCTFail("reopen failed") }
                done.fulfill()
            }
            wait(for: [done], timeout: 5); reopened.close(); EditorWorker.flush()
        }
    }

    func testRealCanvasRotationPixelsUndoDraftAndFailureFixtures() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let fixture = try makeHistoryFixture()
            defer { try? FileManager.default.removeItem(at: fixture.root) }
            let worker = EditorWorker()
            let opened = expectation(description: "rotation fixture")
            worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
                result in XCTAssertNotNil(try? result.get()); opened.fulfill()
            }
            wait(for: [opened], timeout: 5)
            func request(_ object: [String: Any], using worker: EditorWorker) throws -> EditorPresentation {
                let done = expectation(description: "rotation request")
                var response: Result<EditorPresentation, Error>?
                worker.request(object) { result in response = result; done.fulfill() }
                wait(for: [done], timeout: 5)
                return try XCTUnwrap(response).get()
            }
            _ = try request(["operation": "resize_canvas", "width": 640, "height": 360], using: worker)
            let created = try request(["operation": "create_closed_shape", "shape": "rectangle",
                "start": ["x": 100, "y": 80], "end": ["x": 220, "y": 140]], using: worker)
            let id = try XCTUnwrap(created.snapshot.layers.first?.id)
            _ = try request(["operation": "save_draft", "updated_at_ms": 5000], using: worker)
            worker.close(); EditorWorker.flush()
            let live = EditorWorker()
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: live)
            defer { controller.window.orderOut(nil); live.close(); EditorWorker.flush() }
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot != nil && !controller.state.busy }
            try showLayers(in: controller.root); controller.root.layoutSubtreeIfNeeded()
            let overlay = controller.selectionOverlay
            let rect = overlay.presentedImageRect, scale = rect.width / 640
            // Authored 120x60 rectangle, 5px selection padding, 28-view-point grip offset.
            let pivot = CGPoint(x: rect.minX + 160 * scale, y: rect.minY + 110 * scale)
            let start = CGPoint(x: pivot.x, y: rect.minY + 75 * scale - 28)
            let end = CGPoint(x: pivot.x + pivot.y - start.y, y: pivot.y)
            overlay.begin(at: start); overlay.drag(to: end, snap: true)
            XCTAssertEqual(try XCTUnwrap(overlay.rotationPreview?.radians), Double.pi / 2, accuracy: 1e-12)
            XCTAssertFalse(controller.state.snapshot!.unsavedChanges)
            try render(controller.root, name: "screenshot-editor-rotation-active-\(appearance)")
            overlay.end(at: end, snap: true)
            waitUntil { !controller.state.busy && controller.state.snapshot!.unsavedChanges }
            let rotated = try request(["operation": "snapshot"], using: live)
            XCTAssertEqual(rotated.snapshot.layers.first?.id, id)
            XCTAssertEqual(try XCTUnwrap(rotated.snapshot.layers.first?.rotation), Double.pi / 2, accuracy: 1e-12)
            // A quarter-turn about (160,110) leaves bounds x130..190, y50..170.
            XCTAssertEqual(rgba(rotated.image, x: 110, y: 110), [247, 247, 245, 255])
            XCTAssertEqual(rgba(rotated.image, x: 160, y: 60), [255, 59, 92, 255])
            try render(controller.root, name: "screenshot-editor-rotation-committed-\(appearance)")
            try button("Undo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.canRedo == true }
            let undone = try request(["operation": "snapshot"], using: live)
            XCTAssertEqual(rgba(undone.image, x: 110, y: 110), [255, 59, 92, 255])
            XCTAssertEqual(rgba(undone.image, x: 160, y: 60), [247, 247, 245, 255])
            try button("Redo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.canRedo == false }
            try button("Save draft", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.unsavedChanges == false }
            live.close(); EditorWorker.flush()
            let reopened = EditorWorker()
            let restored = expectation(description: "reopen rotated draft")
            reopened.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) { result in
                if let value = try? result.get() {
                    XCTAssertEqual(value.snapshot.layers.first?.id, id)
                    XCTAssertEqual(self.rgba(value.image, x: 160, y: 60), [255, 59, 92, 255])
                } else { XCTFail("rotated draft reopen failed") }
                restored.fulfill()
            }
            wait(for: [restored], timeout: 5); reopened.close(); EditorWorker.flush()

            let failureWorker = FakeEditorWorker(snapshot: rotated.snapshot)
            let failure = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: failureWorker)
            defer { failure.window.orderOut(nil) }
            failure.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            try showOutput(in: failure.root)
            try button("Preview output", in: failure.root).performClick(nil)
            let output = try segmented("Output preview image", in: failure.root)
            try showLayers(in: failure.root); failure.root.layoutSubtreeIfNeeded()
            let surface = failure.selectionOverlay
            let image = surface.presentedImageRect, displayScale = image.width / 640
            let grip = try XCTUnwrap(NativeEditorRotationHandle(outline: try XCTUnwrap(surface.selectedOutline),
                radians: surface.selectedRotation, displayScale: displayScale, canvas: surface.canvasSize))
            let press = CGPoint(x: image.minX + grip.handle.x * displayScale, y: image.minY + grip.handle.y * displayScale)
            let release = CGPoint(x: press.x + 20, y: press.y + 30)
            surface.begin(at: press); surface.end(at: press)
            XCTAssertTrue(failureWorker.requests.isEmpty); XCTAssertTrue(output.isEnabled)
            surface.begin(at: press); surface.drag(to: release)
            failure.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification))
            surface.end(at: release); XCTAssertTrue(failureWorker.requests.isEmpty)
            failureWorker.deferRequests = true
            surface.begin(at: press); surface.end(at: release)
            XCTAssertEqual(failureWorker.requests.count, 1); XCTAssertTrue(failure.state.busy)
            XCTAssertFalse(output.isEnabled); XCTAssertFalse(surface.selectionEnabled)
            XCTAssertEqual(failureWorker.requests[0]["id"] as? String, id)
            XCTAssertEqual((failureWorker.requests[0]["edit"] as? [String: Any])?["action"] as? String, "rotate")
            failureWorker.completePending(with: rotated.snapshot)
            XCTAssertEqual(surface.selectedLayerID, id)
            failureWorker.deferRequests = false; failureWorker.failLayerAction = "rotate"
            failureWorker.failureMessage = "Rotation failed. The previous pixels, selection, undo history and saved draft remain recoverable."
            surface.begin(at: press); surface.end(at: release)
            XCTAssertEqual(failure.state.snapshot, rotated.snapshot)
            XCTAssertFalse(failure.state.busy); XCTAssertEqual(surface.selectedLayerID, id)
            try render(failure.root, name: "screenshot-editor-rotation-error-minimum-\(appearance)")
        }
    }

    func testRealCanvasResizePixelsUndoRedoDraftReopenAndFailureFixtures() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let fixture = try makeHistoryFixture()
            defer { try? FileManager.default.removeItem(at: fixture.root) }
            func request(_ object: [String: Any], using worker: EditorWorker) throws -> EditorPresentation {
                let done = expectation(description: "resize request")
                var response: Result<EditorPresentation, Error>?
                worker.request(object) { response = $0; done.fulfill() }
                wait(for: [done], timeout: 5); return try XCTUnwrap(response).get()
            }
            let seed = EditorWorker()
            let opened = expectation(description: "resize fixture")
            seed.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
                XCTAssertNotNil(try? $0.get()); opened.fulfill()
            }
            wait(for: [opened], timeout: 5)
            _ = try request(["operation": "resize_canvas", "width": 640, "height": 360], using: seed)
            let created = try request(["operation": "create_closed_shape", "shape": "rectangle",
                "start": ["x": 100, "y": 80], "end": ["x": 220, "y": 140]], using: seed)
            let id = try XCTUnwrap(created.snapshot.layers.first?.id)
            _ = try request(["operation": "save_draft", "updated_at_ms": 5000], using: seed)
            seed.close(); EditorWorker.flush()

            let live = EditorWorker()
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: live)
            defer { controller.window.orderOut(nil); live.close(); EditorWorker.flush() }
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot != nil && !controller.state.busy }
            try showLayers(in: controller.root); controller.root.layoutSubtreeIfNeeded()
            let overlay = controller.selectionOverlay
            let image = overlay.presentedImageRect, scale = image.width / 640
            XCTAssertEqual(overlay.resizeHandlePoints.count, 8)
            let southeast = overlay.resizeHandlePoints[4]
            let press = CGPoint(x: image.minX + southeast.x * scale, y: image.minY + southeast.y * scale)
            let release = CGPoint(x: press.x + 60 * scale, y: press.y + 40 * scale)
            overlay.begin(at: press); overlay.drag(to: release)
            XCTAssertNotNil(overlay.resizePreview); XCTAssertFalse(controller.state.snapshot!.unsavedChanges)
            try render(controller.root, name: "screenshot-editor-resize-active-\(appearance)")
            overlay.end(at: release)
            waitUntil { !controller.state.busy && controller.state.snapshot?.unsavedChanges == true }
            let resized = try request(["operation": "snapshot"], using: live)
            XCTAssertEqual(resized.snapshot.layers.first?.id, id)
            XCTAssertEqual(rgba(resized.image, x: 260, y: 165), [255, 59, 92, 255])
            try render(controller.root, name: "screenshot-editor-resize-committed-\(appearance)")
            try button("Undo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.canRedo == true }
            XCTAssertEqual(rgba(try request(["operation": "snapshot"], using: live).image,
                                x: 260, y: 165), [247, 247, 245, 255])
            try button("Redo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.canRedo == false }
            try button("Save draft", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.unsavedChanges == false }
            live.close(); EditorWorker.flush()
            let reopened = EditorWorker(), restored = expectation(description: "reopen resized draft")
            reopened.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
                if let value = try? $0.get() {
                    XCTAssertEqual(self.rgba(value.image, x: 260, y: 165), [255, 59, 92, 255])
                } else { XCTFail("resized draft reopen failed") }
                restored.fulfill()
            }
            wait(for: [restored], timeout: 5); reopened.close(); EditorWorker.flush()

            let failingWorker = FakeEditorWorker(snapshot: resized.snapshot)
            let failure = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: failingWorker)
            defer { failure.window.orderOut(nil) }
            failure.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            try showOutput(in: failure.root); try button("Preview output", in: failure.root).performClick(nil)
            let output = try segmented("Output preview image", in: failure.root)
            try showLayers(in: failure.root); failure.root.layoutSubtreeIfNeeded()
            let surface = failure.selectionOverlay, fitted = surface.presentedImageRect
            XCTAssertEqual(surface.resizeHandlePoints.count, 8)
            let grip = surface.resizeHandlePoints[4]
            let start = CGPoint(x: fitted.minX + grip.x * fitted.width / 640,
                                y: fitted.minY + grip.y * fitted.width / 640)
            failingWorker.deferRequests = true
            surface.begin(at: start); surface.end(at: CGPoint(x: start.x + 20, y: start.y + 20))
            XCTAssertTrue(failure.state.busy); XCTAssertFalse(output.isEnabled)
            XCTAssertEqual((failingWorker.requests.last?["edit"] as? [String: Any])?["action"] as? String, "resize")
            failingWorker.completePending(with: resized.snapshot)
            XCTAssertEqual(surface.selectedLayerID, id)
            failingWorker.deferRequests = false; failingWorker.failLayerAction = "resize"
            failingWorker.failureMessage = "Resize failed; original pixels and selection remain recoverable."
            surface.begin(at: start); surface.end(at: CGPoint(x: start.x + 20, y: start.y + 20))
            XCTAssertFalse(failure.state.busy); XCTAssertEqual(surface.selectedLayerID, id)
            XCTAssertEqual(failure.state.snapshot, resized.snapshot)
            try render(failure.root, name: "screenshot-editor-resize-error-minimum-\(appearance)")
        }
    }

    func testOpenDrawingCommandsSelectFreshLayersInvalidateOutputAndCancelPen() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { worker.response = nil; controller.drawOverlay.cancelGesture(); controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        worker.response = { request in
            var layer = self.shapeLayer(id: "new-\(worker.requests.count)", x: 11, y: 17)
            if request["operation"] as? String == "create_freehand_path" { layer["kind"] = "path" }
            return self.snapshot(id: "shot", unsaved: true, layers: [layer])
        }
        let tool = try popup("Drawing tool", in: controller.root)
        let overlay = controller.drawOverlay
        for (index, operation) in [(2, "create_open_shape"), (3, "create_open_shape"), (4, "create_freehand_path")] {
            try showOutput(in: controller.root)
            try button("Preview output", in: controller.root).performClick(nil)
            try showDraw(in: controller.root)
            tool.selectItem(at: index); _ = tool.sendAction(tool.action, to: tool.target)
            overlay.begin(at: NSPoint(x: 350, y: 300)); overlay.drag(to: NSPoint(x: 80, y: 130))
            let count = worker.requests.count
            overlay.end(at: NSPoint(x: 40, y: 110))
            XCTAssertEqual(worker.requests.count, count + 1)
            XCTAssertEqual(worker.requests.last?["operation"] as? String, operation)
            XCTAssertEqual(controller.state.snapshot?.layers.first?.id, "new-\(count + 1)")
            XCTAssertFalse(try segmented("Output preview image", in: controller.root).isEnabled)
            if index == 4 {
                XCTAssertEqual((worker.requests.last?["points"] as? [[String: CGFloat]])?.count, 2)
            } else { XCTAssertEqual(worker.requests.last?["shape"] as? String, index == 2 ? "line" : "arrow") }
        }
        let count = worker.requests.count
        let coalescing = NSEvent.isMouseCoalescingEnabled
        overlay.begin(at: NSPoint(x: 40, y: 110))
        overlay.keyDown(with: try keyEvent(window: controller.window, keyCode: 53, characters: "\u{1b}"))
        overlay.end(at: NSPoint(x: 120, y: 150))
        XCTAssertEqual(worker.requests.count, count); XCTAssertEqual(NSEvent.isMouseCoalescingEnabled, coalescing)
        overlay.begin(at: NSPoint(x: 40, y: 110)); try showLayers(in: controller.root)
        overlay.end(at: NSPoint(x: 120, y: 150))
        XCTAssertEqual(worker.requests.count, count); XCTAssertEqual(NSEvent.isMouseCoalescingEnabled, coalescing)
        try showDraw(in: controller.root)
        worker.failOperation = "create_freehand_path"
        let published = controller.state.snapshot
        overlay.begin(at: NSPoint(x: 40, y: 110)); overlay.end(at: NSPoint(x: 40, y: 110))
        XCTAssertEqual(controller.state.snapshot, published)
        XCTAssertFalse(controller.state.busy); XCTAssertNil(overlay.startPoint)
        XCTAssertEqual(NSEvent.isMouseCoalescingEnabled, coalescing)
    }

    func testOpenDrawingRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.drawOverlay.cancelGesture(); controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            XCTAssertEqual(tool.itemTitles, ["Rectangle", "Ellipse", "Line", "Arrow", "Pen"])
            for (index, name) in [(2, "line"), (3, "arrow"), (4, "pen")] {
                tool.selectItem(at: index); _ = tool.sendAction(tool.action, to: tool.target)
                controller.drawOverlay.begin(at: NSPoint(x: 100, y: 220))
                controller.drawOverlay.drag(to: NSPoint(x: 250, y: 90))
                controller.drawOverlay.drag(to: NSPoint(x: 420, y: 320))
                try render(controller.root, name: "screenshot-editor-\(name)-preview-\(appearance)")
                controller.drawOverlay.cancelGesture()
            }
            controller.drawOverlay.begin(at: NSPoint(x: 240, y: 210))
            try render(controller.root, name: "screenshot-editor-pen-dot-\(appearance)")
            worker.failOperation = "create_freehand_path"
            worker.failureMessage = "The Pen stroke could not be created. The previous draft, selection, pixels and undo history remain recoverable."
            controller.drawOverlay.end(at: NSPoint(x: 240, y: 210))
            try render(controller.root, name: "screenshot-editor-pen-error-minimum-\(appearance)")
        }
    }

    func testRealBridgeOpenDrawingPixelsUndoAndDraftReopen() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker()
        defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open drawing fixture")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in XCTAssertNotNil(try? result.get()); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        func request(_ object: [String: Any]) throws -> EditorPresentation {
            let done = expectation(description: "drawing request")
            var response: Result<EditorPresentation, Error>?
            worker.request(object) { result in response = result; done.fulfill() }
            wait(for: [done], timeout: 5)
            return try XCTUnwrap(response).get()
        }
        _ = try request(["operation": "resize_canvas", "width": 640, "height": 360])
        let line = try request(["operation": "create_open_shape", "shape": "line",
                                "start": ["x": 450, "y": 120], "end": ["x": 50, "y": 120]])
        XCTAssertEqual(rgba(line.image, x: 300, y: 120), [255, 59, 92, 255])
        let arrow = try request(["operation": "create_open_shape", "shape": "arrow",
                                 "start": ["x": 70, "y": 300], "end": ["x": 550, "y": 300]])
        XCTAssertEqual(rgba(arrow.image, x: 400, y: 300), [255, 59, 92, 255])
        let pen = try request(["operation": "create_freehand_path", "points": [
            ["x": 80, "y": 80], ["x": 200, "y": 240], ["x": 400, "y": 80],
        ]])
        XCTAssertEqual(pen.snapshot.layers.first?.kind, .path)
        XCTAssertEqual(rgba(pen.image, x: 195, y: 180), [255, 59, 92, 255])
        let undone = try request(["operation": "undo"])
        XCTAssertEqual(rgba(undone.image, x: 195, y: 180), [247, 247, 245, 255])
        let redone = try request(["operation": "redo"])
        XCTAssertEqual(redone.snapshot.layers.first?.id, pen.snapshot.layers.first?.id)
        XCTAssertEqual(rgba(redone.image, x: 195, y: 180), [255, 59, 92, 255])
        _ = try request(["operation": "save_draft", "updated_at_ms": 2468])
        worker.close(); EditorWorker.flush()
        for appearance in ["light", "dark"] {
            let reopened = EditorWorker()
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: reopened)
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot?.hasDraft == true && !controller.state.busy }
            XCTAssertEqual(controller.state.snapshot?.layers.first?.id, pen.snapshot.layers.first?.id)
            XCTAssertEqual(controller.state.snapshot?.layers.count, 4)
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(at: 4); _ = tool.sendAction(tool.action, to: tool.target)
            try render(controller.root, name: "screenshot-editor-drawing-committed-\(appearance)")
            controller.window.orderOut(nil); reopened.close(); EditorWorker.flush()
        }
    }

    private func annotationStyle() -> [String: Any] {
        ["closed": true, "color": "#112233", "fill": "#E04090", "strokeWidth": 4.0,
         "strokeEnabled": true, "dropShadow": true,
         "dropShadowStyle": ["color": "#20C060", "opacity": 70.25, "blur": 2.5,
                             "offsetX": 6.0, "offsetY": -3.0]]
    }

    private func annotationToggle(_ label: String, in view: NSView) throws -> CaptureButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? CaptureButton }
            .first { $0.accessibilityLabel() == label })
    }

    private func snapshot(id: String, width: Double = 640, height: Double = 360,
                          unsaved: Bool = false, draft: Bool = false,
                          layers: [[String: Any]] = [],
                          annotations: [String: [String: Any]] = [:]) -> NativeEditorSnapshot {
        NativeEditorSnapshot([
            "artifact_id": id, "document": ["width": width, "height": height,
                                                  "elements": layers],
            "annotation_controls": annotations,
            "can_undo": unsaved, "can_redo": false,
            "unsaved_changes": unsaved, "has_draft": draft,
        ])!
    }

    private func layer(id: String, name: String, x: Double, y: Double, visible: Bool,
                       locked: Bool, opacity: Double) -> [String: Any] {
        ["kind": "image", "id": id, "name": name, "x": x, "y": y,
         "visible": visible, "locked": locked, "opacity": opacity]
    }

    private func shapeLayer(id: String, x: Double, y: Double) -> [String: Any] {
        ["kind": "shape", "id": id, "x": x, "y": y,
         "visible": true, "locked": false, "opacity": 100.0]
    }

    private func artifact(id: String, mode: String = "region") -> CaptureArtifact {
        CaptureArtifact(["entry": [
            "id": id, "kind": "screenshot", "width": 640, "height": 360,
            "created_at": "2026-09-20T00:00:00Z", "mode": mode,
        ], "image_path": "/native/History/\(id)/capture.png",
            "preview_path": "/native/History/\(id)/preview.png"])!
    }

    private func button(_ title: String, in view: NSView) throws -> CaptureButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? CaptureButton }.first { $0.title == title })
    }

    private func field(_ label: String, in view: NSView) throws -> NSTextField {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSTextField }
            .first { $0.accessibilityLabel() == label })
    }

    private func showLayers(in view: NSView) throws {
        let sections = try segmented("Editor section", in: view)
        sections.selectedSegment = 1
        _ = sections.sendAction(sections.action, to: sections.target)
    }

    private func showOutput(in view: NSView) throws {
        let sections = try segmented("Editor section", in: view)
        sections.selectedSegment = 3
        _ = sections.sendAction(sections.action, to: sections.target)
    }

    private func showDraw(in view: NSView) throws {
        let sections = try segmented("Editor section", in: view)
        sections.selectedSegment = 2
        _ = sections.sendAction(sections.action, to: sections.target)
    }

    private func scrollOutputSaveControlsVisible(in view: NSView) throws {
        let filename = try field("Output filename", in: view)
        let scroll = try XCTUnwrap(filename.enclosingScrollView)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: 238))
        scroll.reflectScrolledClipView(scroll.contentView)
        view.layoutSubtreeIfNeeded()
    }

    private func scrollImageImportVisible(in view: NSView) throws {
        let control = try button("Add image…", in: view)
        let scroll = try XCTUnwrap(control.enclosingScrollView)
        let document = try XCTUnwrap(scroll.documentView)
        let bottom = max(0, document.bounds.height - scroll.contentView.bounds.height)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: bottom))
        scroll.reflectScrolledClipView(scroll.contentView)
        view.layoutSubtreeIfNeeded()
    }

    private func scrollLayersTop(in view: NSView) throws {
        let control = try button("Add image…", in: view)
        let scroll = try XCTUnwrap(control.enclosingScrollView)
        scroll.contentView.scroll(to: .zero)
        scroll.reflectScrolledClipView(scroll.contentView)
        view.layoutSubtreeIfNeeded()
    }

    private func popup(_ label: String, in view: NSView) throws -> NSPopUpButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSPopUpButton }
            .first { $0.accessibilityLabel() == label })
    }

    private func segmented(_ label: String, in view: NSView) throws -> NSSegmentedControl {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSSegmentedControl }
            .first { $0.accessibilityLabel() == label })
    }

    private func table(_ label: String, in view: NSView) throws -> NSTableView {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSTableView }
            .first { $0.accessibilityLabel() == label })
    }

    private func keyEvent(window: NSWindow, keyCode: UInt16, characters: String) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: characters, charactersIgnoringModifiers: characters,
            isARepeat: false, keyCode: keyCode))
    }

    private func labels(in view: NSView) -> [String] {
        descendants(in: view).compactMap { ($0 as? NSTextField)?.stringValue }
    }

    private func descendants(in view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap { descendants(in: $0) }
    }

    private func waitUntil(_ predicate: () -> Bool) {
        let deadline = Date().addingTimeInterval(2)
        while !predicate() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(predicate())
    }

    private func settle(_ window: NSWindow) {
        waitUntil { window.isVisible }
        RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        window.display()
    }

    private func render(_ view: NSView, name: String) throws {
        view.window?.display(); view.layoutSubtreeIfNeeded()
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("\(name).png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }

    private func writeTiff(url: URL, width: Int, height: Int, bytes: [UInt8],
                           colorSpace: CGColorSpace, bitsPerPixel: Int,
                           alpha: CGImageAlphaInfo, orientation: Int? = nil) throws {
        let bytesPerRow = width * bitsPerPixel / 8
        let provider = try XCTUnwrap(CGDataProvider(data: Data(bytes) as CFData))
        let image = try XCTUnwrap(CGImage(width: width, height: height, bitsPerComponent: 8,
            bitsPerPixel: bitsPerPixel, bytesPerRow: bytesPerRow, space: colorSpace,
            bitmapInfo: CGBitmapInfo(rawValue: alpha.rawValue), provider: provider,
            decode: nil, shouldInterpolate: false, intent: .defaultIntent))
        let destination = try XCTUnwrap(CGImageDestinationCreateWithURL(
            url as CFURL, "public.tiff" as CFString, 1, nil))
        var properties: [CFString: Any] = [:]
        if let orientation { properties[kCGImagePropertyOrientation] = orientation }
        CGImageDestinationAddImage(destination, image, properties as CFDictionary)
        XCTAssertTrue(CGImageDestinationFinalize(destination))
    }

    private func makeHistoryFixture(transparentOrigin: Bool = false) throws
        -> (root: URL, history: URL, drafts: URL, id: String) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let history = root.appendingPathComponent("History")
        let drafts = root.appendingPathComponent("editor-drafts")
        let id = UUID().uuidString.lowercased()
        let entry = history.appendingPathComponent(id)
        try FileManager.default.createDirectory(at: entry, withIntermediateDirectories: true)
        let image = CGImage.fixture(width: 7, height: 3, transparentOrigin: transparentOrigin)
        let encoded = NSMutableData()
        let destination = try XCTUnwrap(CGImageDestinationCreateWithData(
            encoded, "public.png" as CFString, 1, nil))
        CGImageDestinationAddImage(destination, image, nil)
        XCTAssertTrue(CGImageDestinationFinalize(destination))
        let png = encoded as Data
        try png.write(to: entry.appendingPathComponent("capture.png"))
        try png.write(to: entry.appendingPathComponent("preview.png"))
        let metadata: [String: Any] = [
            "id": id, "kind": "screenshot", "preview_url": "capture-history://\(id)/preview",
            "full_url": "capture-history://\(id)/full", "width": 7, "height": 3,
            "size_bytes": png.count, "created_at": "2026-09-20T00:00:00Z",
            "mode": "region", "saved_path": NSNull(),
        ]
        try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys])
            .write(to: entry.appendingPathComponent("metadata.json"))
        return (root, history, drafts, id)
    }

    private func rgba(_ image: CGImage, x: Int, y: Int) -> [UInt8] {
        guard let data = image.dataProvider?.data,
              let pointer = CFDataGetBytePtr(data) else { return [] }
        let offset = y * image.bytesPerRow + x * 4
        return Array(UnsafeBufferPointer(start: pointer + offset, count: 4))
    }

    private func renderedAlphaRange(_ image: CGImage) -> ClosedRange<UInt8> {
        var grayAlpha = [UInt8](repeating: 0, count: image.width * image.height * 2)
        let rendered = grayAlpha.withUnsafeMutableBytes { storage -> Bool in
            guard let base = storage.baseAddress,
                  let context = CGContext(data: base, width: image.width, height: image.height,
                                          bitsPerComponent: 8, bytesPerRow: image.width * 2,
                                          space: CGColorSpaceCreateDeviceGray(),
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
            return true
        }
        guard rendered else { return 0...0 }
        let alpha = stride(from: 1, to: grayAlpha.count, by: 2).map { grayAlpha[$0] }
        return (alpha.min() ?? 0)...(alpha.max() ?? 0)
    }
}

private final class FakeEditorWorker: EditorWorking {
    var snapshot: NativeEditorSnapshot
    var requests: [[String: Any]] = []
    var encodes: [[String: Any]] = []
    var saves: [[String: Any]] = []
    var imports: [(image: EditorDecodedImage, selectedID: String?)] = []
    var openArtifactIDs: [String] = []
    var closeCount = 0
    var draftsRoot: String?
    var failOperation: String?
    var failLayerAction: String?
    var failureMessage = "fixture save failed"
    var deferRequests = false
    var deferEncodes = false
    var failEncode = false
    var saveResult: Result<EditorSavePresentation, Error> = .success(.saved(path: "/output/edited.png"))
    var importLayerID = "imported-layer"
    var failImport = false
    var importedSnapshot: NativeEditorSnapshot?
    var response: (([String: Any]) -> NativeEditorSnapshot?)?
    var terminationResult: Result<Void, Error> = .success(())
    private var pendingCompletion: ((Result<EditorPresentation, Error>) -> Void)?
    private var pendingEncodeCompletion: ((Result<EditorOutputPresentation, Error>) -> Void)?

    init(snapshot: NativeEditorSnapshot) { self.snapshot = snapshot }

    func open(historyRoot: String, draftsRoot: String, artifactID: String,
              completion: @escaping (Result<EditorPresentation, Error>) -> Void) {
        self.draftsRoot = draftsRoot
        openArtifactIDs.append(artifactID)
        completion(.success(EditorPresentation(snapshot: snapshot,
            image: CGImage.fixture(width: Int(snapshot.width), height: Int(snapshot.height)))))
    }

    func request(_ object: [String: Any],
                 completion: @escaping (Result<EditorPresentation, Error>) -> Void) {
        requests.append(object)
        if object["operation"] as? String == failOperation {
            completion(.failure(AppBridgeError.backend(failureMessage))); return
        }
        if let edit = object["edit"] as? [String: Any],
           edit["action"] as? String == failLayerAction {
            completion(.failure(AppBridgeError.backend(failureMessage))); return
        }
        if deferRequests {
            pendingCompletion = completion
            return
        }
        if let returnedSnapshot = response?(object) { snapshot = returnedSnapshot }
        completion(.success(EditorPresentation(snapshot: snapshot,
            image: CGImage.fixture(width: Int(snapshot.width), height: Int(snapshot.height)))))
    }

    func completePending(with snapshot: NativeEditorSnapshot) {
        self.snapshot = snapshot
        let completion = pendingCompletion
        pendingCompletion = nil
        completion?(.success(EditorPresentation(snapshot: snapshot,
            image: CGImage.fixture(width: Int(snapshot.width), height: Int(snapshot.height)))))
    }

    func encode(_ options: [String: Any],
                completion: @escaping (Result<EditorOutputPresentation, Error>) -> Void) {
        encodes.append(options)
        if failEncode {
            completion(.failure(AppBridgeError.backend(failureMessage))); return
        }
        if deferEncodes {
            pendingEncodeCompletion = completion; return
        }
        completion(.success(output()))
    }

    func completePendingEncode() {
        let completion = pendingEncodeCompletion
        pendingEncodeCompletion = nil
        completion?(.success(output()))
    }

    func saveNew(_ request: [String: Any],
                 completion: @escaping (Result<EditorSavePresentation, Error>) -> Void) {
        saves.append(request)
        completion(saveResult)
    }

    func importImage(_ image: EditorDecodedImage, selectedID: String?,
                     completion: @escaping (Result<EditorImportPresentation, Error>) -> Void) {
        imports.append((image, selectedID))
        if failImport {
            completion(.failure(AppBridgeError.backend(failureMessage))); return
        }
        if let importedSnapshot { snapshot = importedSnapshot }
        completion(.success(EditorImportPresentation(layerID: importLayerID,
            presentation: EditorPresentation(snapshot: snapshot,
                image: CGImage.fixture(width: Int(snapshot.width), height: Int(snapshot.height))))))
    }

    private func output() -> EditorOutputPresentation {
        EditorOutputPresentation(data: Data(repeating: 0x5a, count: 12_345),
            image: CGImage.fixture(width: Int(snapshot.width), height: Int(snapshot.height)))
    }

    func close() { closeCount += 1 }
    func prepareForTermination() -> Result<Void, Error> { terminationResult }
}

private extension CGImage {
    static func fixture(width: Int, height: Int, transparentOrigin: Bool = false) -> CGImage {
        let bytes = (0..<width * height).flatMap { index -> [UInt8] in
            let x = index % width, y = index / width
            return [UInt8(truncatingIfNeeded: x * 31), UInt8(truncatingIfNeeded: y * 71), 19,
                    transparentOrigin && index == 0 ? 0 : 255]
        }
        let provider = CGDataProvider(data: Data(bytes) as CFData)!
        return CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue), provider: provider,
            decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
    }
}
