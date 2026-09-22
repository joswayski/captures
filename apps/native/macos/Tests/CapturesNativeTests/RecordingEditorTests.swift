import AppKit
import ImageIO
import XCTest
@testable import CapturesNative

final class RecordingEditorTests: XCTestCase {
    func testSourceThumbnailsLoadOnceWithoutMutatingEditorState() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let timeline = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingTrimTimeline }.first)
        XCTAssertEqual(worker.thumbnailCalls, 1)
        XCTAssertEqual(timeline.thumbnailStateDescription,
                       "12 source-relative timeline thumbnails")
        XCTAssertFalse(controller.dirty)
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertTrue(worker.saves.isEmpty)

        let seek = try slider("Recording frame position", in: controller.root)
        seek.doubleValue = 500; _ = seek.sendAction(seek.action, to: seek.target)
        let start = try field("Trim start milliseconds", in: controller.root)
        start.stringValue = "100"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        worker.requestResult = .success(try presentation(start: 100, position: 500, revision: 2))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.thumbnailCalls, 1,
                       "seek and accepted edits never regenerate immutable source thumbnails")
    }

    func testThumbnailFailureCancelRetryAndNewSessionClearPriorStrip() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
        worker.thumbnailResult = .failure(AppBridgeError.backend("sprite unavailable"))
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { true })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let timeline = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingTrimTimeline }.first)
        let retry = try button("Retry", in: controller.root)
        XCTAssertEqual(timeline.thumbnailStateDescription, "Source thumbnails unavailable")
        XCTAssertFalse(retry.isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled,
                      "thumbnail failure leaves the rest of editing available")
        XCTAssertFalse(controller.dirty)

        worker.deferThumbnails = true
        retry.performClick(nil)
        XCTAssertEqual(timeline.thumbnailStateDescription, "Loading source thumbnails…")
        XCTAssertFalse(controller.windowShouldClose(controller.window),
                       "accepted thumbnail generation uses the existing busy close gate")
        let cancel = try button("Cancel operation", in: controller.root)
        cancel.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(worker.observedThumbnailCancel).isCancelled)
        XCTAssertEqual(timeline.thumbnailStateDescription, "Cancelling source thumbnails…")
        worker.completeThumbnails(.failure(AppBridgeError.backend("operation cancelled")))
        XCTAssertEqual(timeline.thumbnailStateDescription, "Source thumbnails cancelled")
        XCTAssertFalse(retry.isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)

        worker.deferThumbnails = false
        worker.thumbnailResult = .success(fakeTimelineImage())
        retry.performClick(nil)
        XCTAssertEqual(worker.thumbnailCalls, 3)
        XCTAssertEqual(timeline.thumbnailStateDescription,
                       "12 source-relative timeline thumbnails")
        worker.deferThumbnails = true
        controller.present(artifact: recordingArtifact(id: "another-recording"),
                           historyRoot: "/History", outputDirectory: "/Exports")
        XCTAssertEqual(worker.thumbnailCalls, 4)
        XCTAssertEqual(timeline.thumbnailStateDescription, "Loading source thumbnails…",
                       "a new item clears the prior recording's retained strip")
        worker.completeThumbnails(.success(fakeTimelineImage()))
        XCTAssertEqual(timeline.thumbnailStateDescription,
                       "12 source-relative timeline thumbnails")
    }

    func testCropAndResolutionGeometryBridgeUsesSharedBoundsAndPresets() throws {
        let source = NativeRecordingDimensions(width: 320, height: 180)
        let crop = NativeRecordingCropRect(x: 10, y: 60, width: 160, height: 90)
        XCTAssertEqual(NativeRecordingGeometry.resizeLocked(crop, source: source,
            axis: .width, value: 300),
            NativeRecordingCropRect(x: 10, y: 60, width: 213, height: 120))
        XCTAssertEqual(NativeRecordingGeometry.resizeLocked(crop, source: source,
            axis: .height, value: 0),
            NativeRecordingCropRect(x: 10, y: 60, width: 4, height: 2))

        XCTAssertEqual(NativeRecordingGeometry.constrain(
            NativeRecordingDimensions(width: 4_001, height: 2_003), preset: .p1080),
            NativeRecordingDimensions(width: 2_156, height: 1_080))
        XCTAssertEqual(NativeRecordingGeometry.constrain(
            NativeRecordingDimensions(width: 1_283, height: 721), preset: .p720),
            NativeRecordingDimensions(width: 1_280, height: 720))
        XCTAssertEqual(NativeRecordingGeometry.constrain(
            NativeRecordingDimensions(width: 1_001, height: 501), preset: .original),
            NativeRecordingDimensions(width: 1_000, height: 500))
    }

    func testTimelineBridgeThresholdWildSampleRecoveryAndFractionalTime() throws {
        XCTAssertEqual(try XCTUnwrap(NativeRecordingTimeline.ratio(milliseconds: 4_375,
            durationMilliseconds: 8_750)), 0.5, accuracy: 0.000_001)
        XCTAssertEqual(try XCTUnwrap(NativeRecordingTimeline.time(atX: 500, trackLeft: 0,
            trackWidth: 1_000, durationMilliseconds: 8_750)), 4_375, accuracy: 0.000_001)
        XCTAssertNil(NativeRecordingTimeline.ratio(milliseconds: .nan,
                                                    durationMilliseconds: 8_750))

        var drag = try XCTUnwrap(NativeRecordingTimelineDrag.begin(edge: .start,
            pointerX: 228.571_428_571_428_58, startMilliseconds: 2_000,
            endMilliseconds: 6_750, durationMilliseconds: 8_750))
        XCTAssertEqual(try XCTUnwrap(drag.update(pointerX: 231.570_428_571_428_58,
            trackLeft: 0, trackWidth: 1_000)), 2_000, accuracy: 0.000_001,
            "movement below the shared 3-point threshold does not stage")
        XCTAssertEqual(try XCTUnwrap(drag.update(pointerX: 9_000, trackLeft: 0,
            trackWidth: 1_000)), 2_000, accuracy: 0.000_001,
            "a wild sample is ignored without losing recoverability")
        XCTAssertEqual(try XCTUnwrap(drag.update(pointerX: 278.571_428_571_428_56,
            trackLeft: 0, trackWidth: 1_000)), 2_437.5, accuracy: 0.000_001)
    }

    func testTimelineDragStagesOnlyAndApplyPublishesOnce() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let timeline = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingTrimTimeline }.first)
        let start = try field("Trim start milliseconds", in: controller.root)
        let trackLeft: CGFloat = 10
        let trackWidth = timeline.bounds.width - 20

        XCTAssertTrue(timeline.beginDrag(edge: .start, at: trackLeft))
        timeline.continueDrag(at: trackLeft + 2.999)
        XCTAssertEqual(start.stringValue, "0")
        timeline.continueDrag(at: trackLeft + trackWidth * 0.125)
        XCTAssertEqual(start.stringValue, "250", "fractional shared time rounds only when staged")
        XCTAssertTrue(worker.requests.isEmpty, "pointer movement stages values without decoding")
        XCTAssertFalse(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Estimate size", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Save new copy", in: controller.root).isEnabled)
        timeline.endDrag()
        timeline.continueDrag(at: trackLeft + trackWidth * 0.25)
        XCTAssertEqual(start.stringValue, "250", "lost capture retains the staged value and ends the gesture")

        worker.requestResult = .success(try presentation(start: 250, revision: 1))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.count, 1)
        XCTAssertEqual(worker.requests.first?["operation"] as? String, "update_preview")
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)
    }

    func testTimelineWindowHitTestingPointerDispatchAndResignKey() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(start: 200, end: 1_800))
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let timeline = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingTrimTimeline }.first)
        let startHandle = try XCTUnwrap(descendants(in: timeline)
            .compactMap { $0 as? RecordingTrimHandle }
            .first { $0.edge == .start })
        let endHandle = try XCTUnwrap(descendants(in: timeline)
            .compactMap { $0 as? RecordingTrimHandle }
            .first { $0.edge == .end })
        let start = try field("Trim start milliseconds", in: controller.root)
        let end = try field("Trim end milliseconds", in: controller.root)

        for size in [NSSize(width: 960, height: 760), NSSize(width: 760, height: 540)] {
            controller.window.setContentSize(size)
            XCTAssertTrue(try windowHit(startHandle, in: controller) === startHandle)
            XCTAssertTrue(try windowHit(endHandle, in: controller) === endHandle)
        }

        start.stringValue = "1000"; end.stringValue = "1002"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        XCTAssertTrue(try windowHit(startHandle, in: controller) === startHandle,
                      "near-overlapping grips still select start at its displayed position")
        XCTAssertTrue(try windowHit(endHandle, in: controller) === endHandle,
                      "near-overlapping grips still select end at its displayed position")

        start.stringValue = "200"; end.stringValue = "1800"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        let originalStart = start.stringValue
        try dispatchMouse(.leftMouseDown, to: startHandle, in: controller)
        try dispatchMouse(.leftMouseDragged, to: startHandle, in: controller, deltaX: 30)
        try dispatchMouse(.leftMouseUp, to: startHandle, in: controller, deltaX: 30)
        XCTAssertNotEqual(start.stringValue, originalStart)
        XCTAssertTrue(worker.requests.isEmpty, "real pointer dispatch only stages the start handle")

        let originalEnd = end.stringValue
        try dispatchMouse(.leftMouseDown, to: endHandle, in: controller)
        try dispatchMouse(.leftMouseDragged, to: endHandle, in: controller, deltaX: -30)
        let stagedEnd = end.stringValue
        XCTAssertNotEqual(stagedEnd, originalEnd)
        controller.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification,
                                                   object: controller.window))
        try dispatchMouse(.leftMouseDragged, to: endHandle, in: controller, deltaX: -60)
        XCTAssertEqual(end.stringValue, stagedEnd,
                       "window deactivation drops capture without reverting the last stage")
        try dispatchMouse(.leftMouseUp, to: endHandle, in: controller, deltaX: -60)
        XCTAssertTrue(worker.requests.isEmpty, "real pointer dispatch never seeks or decodes")
    }

    func testTimelineFailureRetryDoesNotCoverRealHandleDispatchAtEitherSize() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(start: 200, end: 1_800))
        worker.thumbnailResult = .failure(AppBridgeError.backend("sprite unavailable"))
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let timeline = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingTrimTimeline }.first)
        let startHandle = try XCTUnwrap(descendants(in: timeline)
            .compactMap { $0 as? RecordingTrimHandle }.first { $0.edge == .start })
        let endHandle = try XCTUnwrap(descendants(in: timeline)
            .compactMap { $0 as? RecordingTrimHandle }.first { $0.edge == .end })
        let start = try field("Trim start milliseconds", in: controller.root)
        let end = try field("Trim end milliseconds", in: controller.root)

        for size in [NSSize(width: 960, height: 760), NSSize(width: 760, height: 540)] {
            controller.window.setContentSize(size)
            start.stringValue = "200"; end.stringValue = "1800"
            controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                         object: start))
            XCTAssertTrue(try windowHit(startHandle, in: controller) === startHandle)
            XCTAssertTrue(try windowHit(endHandle, in: controller) === endHandle,
                          "Retry stays outside the end-handle hit region")
            try dispatchMouse(.leftMouseDown, to: startHandle, in: controller)
            try dispatchMouse(.leftMouseDragged, to: startHandle, in: controller, deltaX: 8)
            try dispatchMouse(.leftMouseUp, to: startHandle, in: controller, deltaX: 8)
            XCTAssertNotEqual(start.stringValue, "200")

            start.stringValue = "200"; end.stringValue = "1800"
            controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                         object: start))
            try dispatchMouse(.leftMouseDown, to: endHandle, in: controller)
            try dispatchMouse(.leftMouseDragged, to: endHandle, in: controller, deltaX: -8)
            try dispatchMouse(.leftMouseUp, to: endHandle, in: controller, deltaX: -8)
            XCTAssertNotEqual(end.stringValue, "1800")
        }
        XCTAssertTrue(worker.requests.isEmpty, "thumbnail error trim dispatch only stages values")
    }

    func testTimelineKeyboardStepsBoundsAndNumericSynchronization() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let timeline = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingTrimTimeline }.first)
        let start = try field("Trim start milliseconds", in: controller.root)
        let end = try field("Trim end milliseconds", in: controller.root)
        timeline.nudge(edge: .start, direction: 1, page: false)
        XCTAssertEqual(start.stringValue, "1", "short recordings use one-millisecond arrows")
        timeline.nudge(edge: .start, direction: 1, page: true)
        XCTAssertEqual(start.stringValue, "1001", "Page Up stages one second")
        timeline.nudge(edge: .end, direction: -1, page: true)
        XCTAssertEqual(end.stringValue, "1002", "handles retain the shared one-millisecond span")

        start.stringValue = "200"; end.stringValue = "1500"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        XCTAssertEqual(timeline.startMilliseconds, 200)
        XCTAssertEqual(timeline.endMilliseconds, 1500)
        start.stringValue = "invalid"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        XCTAssertFalse(timeline.editingEnabled)
        XCTAssertEqual(timeline.startMilliseconds, 200,
                       "partial numeric input does not corrupt the last drawable range")

        let longTimeline = RecordingTrimTimeline(tokens: Tokens.variants["light-mustard"]!)
        longTimeline.frame = NSRect(x: 0, y: 0, width: 500, height: 28)
        longTimeline.setValues(start: 20, end: 70_000, duration: 70_000)
        longTimeline.setEditingEnabled(true)
        var staged: UInt64?
        longTimeline.onStage = { _, value in staged = value }
        longTimeline.nudge(edge: .start, direction: 1, page: false)
        XCTAssertEqual(staged, 30, "recordings at or above 60 seconds use ten-millisecond arrows")
    }

    func testStagedTrimFormatFailureRetainsAcceptedFrameAndValues() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
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
        let export = try XCTUnwrap(request["export"] as? [String: Any])
        XCTAssertEqual(export["format"] as? String, "gif")
        XCTAssertEqual((export["max_size_bytes"] as? NSNumber)?.uint64Value, 4_000_000,
                       "the host preserves accepted export fields it does not own")
        XCTAssertEqual(start.stringValue, "250"); XCTAssertEqual(end.stringValue, "1250")
        XCTAssertEqual(format.titleOfSelectedItem, "GIF")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("preview unavailable") })
        XCTAssertFalse(controller.prepareForTermination(), "staged recording edits have no draft")
    }

    func testCropTypedCommitLockAndRelockUseCurrentRatioWithoutEarlyStaging() throws {
        _ = NSApplication.shared
        let initialCrop = NativeRecordingCropRect(x: 10, y: 60, width: 160, height: 90)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(crop: initialCrop))
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let width = try field("Recording crop width", in: controller.root)
        let height = try field("Recording crop height", in: controller.root)
        let lock = try checkbox("Lock recording crop aspect ratio", in: controller.root)
        XCTAssertEqual(lock.state, .on)
        XCTAssertFalse(controller.dirty, "the default lock is UI-only")

        width.stringValue = "300"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: width))
        XCTAssertTrue(controller.dirty, "pending text participates in lifecycle gates")
        XCTAssertEqual(height.stringValue, "90", "typing alone does not change the coupled ratio")
        _ = width.sendAction(width.action, to: width.target)
        XCTAssertEqual(width.stringValue, "213")
        XCTAssertEqual(height.stringValue, "120",
                       "locked dimensions use the shared ratio and remaining origin bounds")
        XCTAssertTrue(controller.dirty)

        lock.state = .off; _ = lock.sendAction(lock.action, to: lock.target)
        width.stringValue = "111"; _ = width.sendAction(width.action, to: width.target)
        XCTAssertEqual(width.stringValue, "111"); XCTAssertEqual(height.stringValue, "120")
        lock.state = .on; _ = lock.sendAction(lock.action, to: lock.target)
        XCTAssertTrue(controller.dirty, "relocking does not itself publish or discard geometry")
        height.stringValue = "60"; _ = height.sendAction(height.action, to: height.target)
        XCTAssertEqual(width.stringValue, "56")
        XCTAssertEqual(height.stringValue, "60", "relock uses the adjusted 111:120 ratio")
    }

    func testPendingCropFieldEditorBuffersGateLifecycleAndApplyCommitsOnce() throws {
        _ = NSApplication.shared
        let initialCrop = NativeRecordingCropRect(x: 10, y: 60, width: 160, height: 90)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(crop: initialCrop))
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let width = try field("Recording crop width", in: controller.root)
        let height = try field("Recording crop height", in: controller.root)
        let apply = try button("Apply edits", in: controller.root)
        let seek = try slider("Recording frame position", in: controller.root)
        let outputMode = try popup("Recording output size", in: controller.root)
        let estimate = try button("Estimate size", in: controller.root)
        let save = try button("Save new copy", in: controller.root)

        width.selectText(nil)
        let editor = try XCTUnwrap(controller.window.fieldEditor(false, for: width) as? NSTextView)
        XCTAssertTrue(controller.window.firstResponder === editor,
                      "the real AppKit field editor, not the NSTextField, owns the pending buffer")
        editor.insertText("300", replacementRange: NSRange(location: 0, length: editor.string.utf16.count))
        XCTAssertEqual(width.stringValue, "300")
        XCTAssertEqual(height.stringValue, "90", "ratio math remains deferred until commit")
        XCTAssertTrue(controller.dirty)
        XCTAssertTrue(apply.isEnabled, "valid pending text can be applied")
        XCTAssertFalse(seek.isEnabled)
        XCTAssertFalse(estimate.isEnabled)
        XCTAssertFalse(save.isEnabled)
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        XCTAssertFalse(controller.prepareForTermination())

        worker.requestResult = .success(try presentation(revision: 1,
            crop: NativeRecordingCropRect(x: 10, y: 60, width: 213, height: 120)))
        apply.performClick(nil)
        XCTAssertEqual(worker.requests.count, 1,
                       "Apply commits the active field editor and sends one atomic update")
        let edit = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
        let crop = try XCTUnwrap(edit["crop"] as? [String: Any])
        XCTAssertEqual((crop["width"] as? NSNumber)?.uint32Value, 213)
        XCTAssertEqual((crop["height"] as? NSNumber)?.uint32Value, 120)
        XCTAssertEqual(width.stringValue, "213"); XCTAssertEqual(height.stringValue, "120")
        XCTAssertFalse(apply.isEnabled)

        width.selectText(nil)
        let invalidEditor = try XCTUnwrap(controller.window.fieldEditor(false, for: width)
            as? NSTextView)
        invalidEditor.insertText("-", replacementRange: NSRange(
            location: 0, length: invalidEditor.string.utf16.count))
        XCTAssertTrue(controller.dirty)
        XCTAssertFalse(apply.isEnabled, "partial input cannot publish stale crop geometry")
        XCTAssertFalse(seek.isEnabled); XCTAssertFalse(estimate.isEnabled)
        let requestCount = worker.requests.count
        _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertEqual(worker.requests.count, requestCount)
        XCTAssertEqual(width.stringValue, "-", "blocked seek cannot replace the pending buffer")
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        XCTAssertFalse(controller.prepareForTermination())
        controller.window.makeFirstResponder(nil)
        XCTAssertEqual(width.stringValue, "-", "invalid end editing remains available for correction")
        XCTAssertTrue(controller.dirty)
        outputMode.selectItem(withTitle: "720p maximum")
        _ = outputMode.sendAction(outputMode.action, to: outputMode.target)
        XCTAssertEqual(width.stringValue, "-",
                       "an unrelated output preset cannot discard ended invalid crop input")
        XCTAssertFalse(apply.isEnabled); XCTAssertFalse(seek.isEnabled)
        XCTAssertFalse(estimate.isEnabled); XCTAssertFalse(save.isEnabled)

        width.selectText(nil)
        let validEditor = try XCTUnwrap(controller.window.fieldEditor(false, for: width)
            as? NSTextView)
        validEditor.insertText("100", replacementRange: NSRange(
            location: 0, length: validEditor.string.utf16.count))
        outputMode.selectItem(withTitle: "1080p maximum")
        _ = outputMode.sendAction(outputMode.action, to: outputMode.target)
        XCTAssertTrue(controller.window.firstResponder === validEditor,
                      "selecting a popup item does not guarantee field-editor resignation")
        XCTAssertEqual(width.stringValue, "100",
                       "a preset change retains valid active crop text until explicit commit")
        XCTAssertTrue(apply.isEnabled); XCTAssertFalse(seek.isEnabled)
        XCTAssertFalse(estimate.isEnabled); XCTAssertFalse(save.isEnabled)
    }

    func testCropPresetCustomOriginalAndFailureRetentionShareAtomicGates() throws {
        _ = NSApplication.shared
        let crop = NativeRecordingCropRect(x: 20, y: 30, width: 1_001, height: 1_501)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            sourceWidth: 4_001, sourceHeight: 2_003, crop: crop))
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let mode = try popup("Recording output size", in: controller.root)
        let outputWidth = try field("Recording output width", in: controller.root)
        let outputHeight = try field("Recording output height", in: controller.root)
        let cropHeight = try field("Recording crop height", in: controller.root)
        let lock = try checkbox("Lock recording crop aspect ratio", in: controller.root)
        let preview = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSImageView }.first)
        let acceptedFrame = preview.image

        mode.selectItem(withTitle: "720p maximum")
        _ = mode.sendAction(mode.action, to: mode.target)
        XCTAssertEqual(outputWidth.stringValue, "480")
        XCTAssertEqual(outputHeight.stringValue, "720")
        XCTAssertFalse(outputWidth.isEnabled); XCTAssertFalse(outputHeight.isEnabled)
        XCTAssertFalse(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Estimate size", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Save new copy", in: controller.root).isEnabled)

        worker.requestResult = .failure(AppBridgeError.backend("crop preview unavailable"))
        try button("Apply edits", in: controller.root).performClick(nil)
        let failed = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
        let failedCrop = try XCTUnwrap(failed["crop"] as? [String: Any])
        XCTAssertEqual((failedCrop["x"] as? NSNumber)?.uint32Value, 20)
        XCTAssertEqual((failedCrop["y"] as? NSNumber)?.uint32Value, 30)
        XCTAssertEqual((failed["output_width"] as? NSNumber)?.uint32Value, 480)
        XCTAssertEqual((failed["output_height"] as? NSNumber)?.uint32Value, 720)
        XCTAssertTrue(preview.image === acceptedFrame)
        XCTAssertEqual(mode.titleOfSelectedItem, "720p maximum")

        worker.requestResult = .success(try presentation(revision: 1,
            sourceWidth: 4_001, sourceHeight: 2_003, crop: crop,
            output: NativeRecordingDimensions(width: 480, height: 720)))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(mode.titleOfSelectedItem, "720p maximum",
                       "acceptance retains the preset rather than inferring Custom")
        XCTAssertFalse(try button("Apply edits", in: controller.root).isEnabled)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Estimate size", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Save new copy", in: controller.root).isEnabled)

        let seek = try slider("Recording frame position", in: controller.root)
        worker.requestResult = .success(try presentation(position: 500, revision: 2,
            sourceWidth: 4_001, sourceHeight: 2_003, crop: crop,
            output: NativeRecordingDimensions(width: 480, height: 720)))
        seek.doubleValue = 500; _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertEqual(mode.titleOfSelectedItem, "720p maximum",
                       "source-relative seek retains the host preset")
        XCTAssertFalse(try button("Apply edits", in: controller.root).isEnabled)

        lock.state = .off; _ = lock.sendAction(lock.action, to: lock.target)
        cropHeight.stringValue = "501"
        _ = cropHeight.sendAction(cropHeight.action, to: cropHeight.target)
        XCTAssertEqual(outputWidth.stringValue, "1000")
        XCTAssertEqual(outputHeight.stringValue, "500",
                       "the selected preset recomputes from the staged crop")

        mode.selectItem(withTitle: "Custom"); _ = mode.sendAction(mode.action, to: mode.target)
        XCTAssertTrue(outputWidth.isEnabled); XCTAssertTrue(outputHeight.isEnabled)
        XCTAssertEqual(outputWidth.stringValue, "1000"); XCTAssertEqual(outputHeight.stringValue, "500")
        outputWidth.stringValue = "81"; outputHeight.stringValue = "61"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: outputWidth))
        worker.requestResult = .success(try presentation(position: 500, revision: 3,
            sourceWidth: 4_001, sourceHeight: 2_003,
            crop: NativeRecordingCropRect(x: 20, y: 30, width: 1_001, height: 501),
            output: NativeRecordingDimensions(width: 81, height: 61)))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(mode.titleOfSelectedItem, "Custom")
        XCTAssertEqual(outputWidth.stringValue, "81"); XCTAssertEqual(outputHeight.stringValue, "61")

        mode.selectItem(withTitle: "Original"); _ = mode.sendAction(mode.action, to: mode.target)
        worker.requestResult = .success(try presentation(position: 500, revision: 4,
            sourceWidth: 4_001, sourceHeight: 2_003,
            crop: NativeRecordingCropRect(x: 20, y: 30, width: 1_001, height: 501)))
        try button("Apply edits", in: controller.root).performClick(nil)
        let original = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
        XCTAssertTrue(original["output_width"] is NSNull)
        XCTAssertTrue(original["output_height"] is NSNull,
                      "Original omits explicit dimensions instead of publishing helper normalization")
    }

    func testAudioStagesAsymmetricTracksAndApplyPublishesTrustedIdentity() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            hasSystemAudio: true, hasMicrophoneAudio: true,
            systemVolume: 0.8, microphoneVolume: 1.2))
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let system = try field("System audio volume percent", in: controller.root)
        let microphone = try field("Microphone volume percent", in: controller.root)
        XCTAssertEqual(system.stringValue, "80%")
        XCTAssertEqual(microphone.stringValue, "120%")
        system.stringValue = "25"; microphone.stringValue = "175"
        let muteMicrophone = try checkbox("Mute microphone", in: controller.root)
        let monoOutput = try checkbox("Mono audio output", in: controller.root)
        muteMicrophone.state = .on; monoOutput.state = .on
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: system))
        XCTAssertFalse(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Estimate size", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Save new copy", in: controller.root).isEnabled)
        XCTAssertTrue(controller.dirty, "audio-only staging participates in dirty lifecycle")

        worker.requestResult = .success(try presentation(revision: 1,
            hasSystemAudio: true, hasMicrophoneAudio: true,
            systemVolume: 0.25, microphoneVolume: 1.75,
            muteMicrophone: true, monoOutput: true))
        try button("Apply edits", in: controller.root).performClick(nil)
        let request = try XCTUnwrap(worker.requests.last)
        let edit = try XCTUnwrap(request["edit"] as? [String: Any])
        let audio = try XCTUnwrap(edit["audio"] as? [String: Any])
        XCTAssertEqual((audio["system_volume"] as? NSNumber)?.doubleValue, 0.25)
        XCTAssertEqual((audio["microphone_volume"] as? NSNumber)?.doubleValue, 1.75)
        XCTAssertEqual(audio["mute_system_audio"] as? Bool, false)
        XCTAssertEqual(audio["mute_microphone"] as? Bool, true)
        XCTAssertEqual(audio["mono_output"] as? Bool, true)
        XCTAssertEqual(audio["source_has_system_audio"] as? Bool, true)
        XCTAssertEqual(audio["source_has_microphone_audio"] as? Bool, true)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)
    }

    func testAudioPercentagePresentationUsesPlainExactValues() throws {
        _ = NSApplication.shared
        for percent in [0.0, 25, 80, 100, 120, 175, 200] {
            let worker = FakeRecordingEditorWorker(presentation: try presentation(
                hasSystemAudio: true, systemVolume: Double(Float(percent / 100))))
            let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                       worker: worker, confirmDiscard: { false })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                               outputDirectory: "/Exports")
            let expected = percent.rounded() == percent ? String(Int(percent)) : String(percent)
            XCTAssertEqual(try field("System audio volume percent", in: controller.root).stringValue,
                           "\(expected)%")
        }
    }

    func testAudioPercentagesRoundTripAtF32BoundaryWithoutRegating() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            hasSystemAudio: true, hasMicrophoneAudio: true))
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let system = try field("System audio volume percent", in: controller.root)
        let microphone = try field("Microphone volume percent", in: controller.root)
        system.stringValue = "12.34%"; microphone.stringValue = "33.3%"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: system))
        worker.requestResult = .success(try presentation(revision: 1,
            hasSystemAudio: true, hasMicrophoneAudio: true,
            systemVolume: Double(Float(0.1234)), microphoneVolume: Double(Float(0.333))))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(system.stringValue, "12.34%")
        XCTAssertEqual(microphone.stringValue, "33.3%")
        XCTAssertFalse(try button("Apply edits", in: controller.root).isEnabled)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Estimate size", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Save new copy", in: controller.root).isEnabled)

        let seek = try slider("Recording frame position", in: controller.root)
        worker.requestResult = .success(try presentation(position: 500, revision: 2,
            hasSystemAudio: true, hasMicrophoneAudio: true,
            systemVolume: Double(Float(0.1234)), microphoneVolume: Double(Float(0.333))))
        seek.doubleValue = 500; _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertEqual(system.stringValue, "12.34%")
        XCTAssertEqual(microphone.stringValue, "33.3%")
        XCTAssertFalse(try button("Apply edits", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Save new copy", in: controller.root).isEnabled)

        microphone.stringValue = "29%"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: microphone))
        worker.requestResult = .success(try presentation(position: 500, revision: 3,
            hasSystemAudio: true, hasMicrophoneAudio: true,
            systemVolume: Double(Float(0.1234)), microphoneVolume: Double(Float(0.29))))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(system.stringValue, "12.34%")
        XCTAssertEqual(microphone.stringValue, "29%")
        XCTAssertFalse(try button("Apply edits", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Estimate size", in: controller.root).isEnabled)

        let start = try field("Trim start milliseconds", in: controller.root)
        start.stringValue = "100"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        let format = try popup("Recording export format", in: controller.root)
        format.selectItem(withTitle: "GIF"); _ = format.sendAction(format.action, to: format.target)
        let muteSystem = try checkbox("Mute system audio", in: controller.root)
        format.selectItem(withTitle: "MP4"); _ = format.sendAction(format.action, to: format.target)
        muteSystem.state = .on; _ = muteSystem.sendAction(muteSystem.action, to: muteSystem.target)
        worker.requestResult = .success(try presentation(start: 100, position: 500, revision: 4,
            hasSystemAudio: true, hasMicrophoneAudio: true,
            systemVolume: Double(Float(0.1234)), microphoneVolume: Double(Float(0.29)),
            muteSystem: true))
        try button("Apply edits", in: controller.root).performClick(nil)
        let request = try XCTUnwrap(worker.requests.last)
        let edit = try XCTUnwrap(request["edit"] as? [String: Any])
        let audio = try XCTUnwrap(edit["audio"] as? [String: Any])
        XCTAssertEqual((audio["system_volume"] as? NSNumber)?.floatValue, Float(0.1234))
        XCTAssertEqual((audio["microphone_volume"] as? NSNumber)?.floatValue, Float(0.29))
        XCTAssertFalse(try button("Apply edits", in: controller.root).isEnabled)
        XCTAssertTrue(try button("Save new copy", in: controller.root).isEnabled)
    }

    func testAudioFailureGifRetentionAndUnavailableTracks() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            hasSystemAudio: true, hasMicrophoneAudio: true))
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let system = try field("System audio volume percent", in: controller.root)
        let microphone = try field("Microphone volume percent", in: controller.root)
        let preview = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSImageView }.first)
        let acceptedFrame = preview.image
        system.stringValue = "30"; microphone.stringValue = "160"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: system))
        worker.requestResult = .failure(AppBridgeError.backend("audio preview unavailable"))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(system.stringValue, "30"); XCTAssertEqual(microphone.stringValue, "160")
        XCTAssertTrue(preview.image === acceptedFrame, "failed Apply retains the accepted frame")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("audio preview unavailable") })
        XCTAssertFalse(try button("Save new copy", in: controller.root).isEnabled)

        let format = try popup("Recording export format", in: controller.root)
        format.selectItem(withTitle: "GIF"); _ = format.sendAction(format.action, to: format.target)
        XCTAssertFalse(system.isEnabled); XCTAssertFalse(microphone.isEnabled)
        XCTAssertFalse(try checkbox("Mute system audio", in: controller.root).isEnabled)
        XCTAssertFalse(try checkbox("Mono audio output", in: controller.root).isEnabled)
        XCTAssertEqual(system.stringValue, "30"); XCTAssertEqual(microphone.stringValue, "160")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("MP4 kept") })
        format.selectItem(withTitle: "MP4"); _ = format.sendAction(format.action, to: format.target)
        XCTAssertTrue(system.isEnabled); XCTAssertTrue(microphone.isEnabled)
        XCTAssertEqual(system.stringValue, "30"); XCTAssertEqual(microphone.stringValue, "160")

        let silentWorker = FakeRecordingEditorWorker(presentation: try presentation())
        let silent = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                               worker: silentWorker, confirmDiscard: { false })
        defer { silent.window.orderOut(nil) }
        silent.present(artifact: recordingArtifact(), historyRoot: "/History",
                       outputDirectory: "/Exports")
        XCTAssertTrue(labels(in: silent.root).contains { $0 == "No audio tracks." })
        XCTAssertTrue(try field("System audio volume percent", in: silent.root).isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(try field("Microphone volume percent", in: silent.root).isHiddenOrHasHiddenAncestor)

        let systemOnlyWorker = FakeRecordingEditorWorker(presentation: try presentation(hasSystemAudio: true))
        let systemOnly = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: systemOnlyWorker, confirmDiscard: { false })
        defer { systemOnly.window.orderOut(nil) }
        systemOnly.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        XCTAssertFalse(try field("System audio volume percent", in: systemOnly.root).isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(try field("Microphone volume percent", in: systemOnly.root).isHiddenOrHasHiddenAncestor)
    }

    func testAcceptedSeekEstimateSaveWarningAndDirtyLifecycle() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
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
        let accepted = try presentation(start: 100, end: nil, position: 0, revision: 1)
        worker.requestResult = .success(accepted)
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)
        XCTAssertFalse(controller.prepareForTermination(), "accepted edits remain dirty until save")

        let seek = try slider("Recording frame position", in: controller.root)
        seek.doubleValue = 700
        worker.requestResult = .success(try presentation(start: 100, end: nil, position: 700, revision: 2))
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

    func testSameArtifactReentryPreservesEditsAndInFlightSave() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
        var historyReloads = 0
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
            worker: worker, didSaveCopy: { historyReloads += 1 }, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        let artifact = recordingArtifact()
        controller.present(artifact: artifact, historyRoot: "/History", outputDirectory: "/Exports")
        XCTAssertEqual(worker.openCount, 1)

        let start = try field("Trim start milliseconds", in: controller.root)
        start.stringValue = "100"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: start))
        controller.present(artifact: artifact, historyRoot: "/History", outputDirectory: "/Elsewhere")
        XCTAssertEqual(worker.openCount, 1)
        XCTAssertEqual(start.stringValue, "100", "same-item focus retains staged values")

        worker.requestResult = .success(try presentation(start: 100, revision: 1))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertTrue(controller.dirty)
        controller.present(artifact: artifact, historyRoot: "/History", outputDirectory: "/Elsewhere")
        XCTAssertEqual(worker.openCount, 1)
        XCTAssertEqual(start.stringValue, "100", "same-item focus retains accepted dirty edits")

        worker.deferSave = true
        try button("Save new copy", in: controller.root).performClick(nil)
        XCTAssertNotNil(worker.observedSaveCancel)
        controller.present(artifact: artifact, historyRoot: "/History", outputDirectory: "/Elsewhere")
        controller.present(artifact: recordingArtifact(id: "another-recording"),
                           historyRoot: "/History", outputDirectory: "/Elsewhere")
        XCTAssertEqual(worker.openCount, 1, "busy same/different-item requests cannot reopen the session")
        XCTAssertNotNil(worker.observedSaveCancel, "same-item focus retains the independent cancel token")
        XCTAssertFalse(try button("Cancel operation", in: controller.root).isHiddenOrHasHiddenAncestor)
        worker.completeSave(.success(.saved(path: "/Exports/recording-edit-recordin.mp4")))
        XCTAssertEqual(historyReloads, 1, "accepted save completion still publishes to History")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("recording-edit-recordin.mp4") })
        XCTAssertFalse(controller.dirty)
    }

    func testNewOpenClearsPreviousFrameBeforeFailure() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation())
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { true })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let image = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSImageView }.first)
        XCTAssertNotNil(image.image)

        worker.deferOpen = true
        controller.present(artifact: recordingArtifact(id: "another-recording"),
                           historyRoot: "/History", outputDirectory: "/Exports")
        XCTAssertNil(image.image, "a genuinely new open cannot display the previous recording frame")
        worker.completeOpen(.failure(AppBridgeError.backend("source missing")))
        XCTAssertNil(image.image)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("source missing") })
    }

    func testBusyCloseQuitCancellationAndRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeRecordingEditorWorker(presentation: try presentation(
                hasSystemAudio: true, hasMicrophoneAudio: true))
            let controller = RecordingEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                                                       worker: worker, confirmDiscard: { false })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                               outputDirectory: "/Exports")
            try render(controller.root, name: "recording-editor-normal-\(appearance)")
            let systemVolume = try field("System audio volume percent", in: controller.root)
            let microphoneVolume = try field("Microphone volume percent", in: controller.root)
            systemVolume.stringValue = "25%"; microphoneVolume.stringValue = "175%"
            controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                         object: systemVolume))
            try render(controller.root, name: "recording-editor-audio-staged-\(appearance)")
            systemVolume.stringValue = "100"; microphoneVolume.stringValue = "100"
            controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                         object: systemVolume))
            let timeline = try XCTUnwrap(descendants(in: controller.root)
                .compactMap { $0 as? RecordingTrimTimeline }.first)
            timeline.nudge(edge: .start, direction: 1, page: true)
            let startHandle = try XCTUnwrap(descendants(in: timeline)
                .first { $0.accessibilityLabel() == "Recording trim start handle" })
            XCTAssertTrue(controller.window.makeFirstResponder(startHandle))
            try render(controller.root, name: "recording-editor-trim-staged-\(appearance)")
            timeline.nudge(edge: .start, direction: -1, page: true)
            let format = try popup("Recording export format", in: controller.root)
            format.selectItem(withTitle: "GIF"); _ = format.sendAction(format.action, to: format.target)
            try render(controller.root, name: "recording-editor-gif-audio-disabled-\(appearance)")
            format.selectItem(withTitle: "MP4"); _ = format.sendAction(format.action, to: format.target)
            let cropEnabled = try checkbox("Crop recording", in: controller.root)
            cropEnabled.state = .on; _ = cropEnabled.sendAction(cropEnabled.action,
                                                                 to: cropEnabled.target)
            let cropWidth = try field("Recording crop width", in: controller.root)
            let cropX = try field("Recording crop X", in: controller.root)
            let cropY = try field("Recording crop Y", in: controller.root)
            cropWidth.stringValue = "160"; _ = cropWidth.sendAction(cropWidth.action,
                                                                    to: cropWidth.target)
            cropX.stringValue = "20"; _ = cropX.sendAction(cropX.action, to: cropX.target)
            cropY.stringValue = "20"; _ = cropY.sendAction(cropY.action, to: cropY.target)
            let outputMode = try popup("Recording output size", in: controller.root)
            outputMode.selectItem(withTitle: "720p maximum")
            _ = outputMode.sendAction(outputMode.action, to: outputMode.target)
            try render(controller.root, name: "recording-editor-crop-output-staged-\(appearance)")
            cropEnabled.state = .off; _ = cropEnabled.sendAction(cropEnabled.action,
                                                                  to: cropEnabled.target)
            outputMode.selectItem(withTitle: "Original")
            _ = outputMode.sendAction(outputMode.action, to: outputMode.target)
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            XCTAssertTrue(controller.root.subviews.allSatisfy {
                $0.isHidden || controller.root.bounds.intersects($0.frame)
            })
            try render(controller.root, name: "recording-editor-minimum-\(appearance)")
            outputMode.selectItem(withTitle: "Custom")
            _ = outputMode.sendAction(outputMode.action, to: outputMode.target)
            let outputWidth = try field("Recording output width", in: controller.root)
            let outputHeight = try field("Recording output height", in: controller.root)
            outputWidth.stringValue = "641"; outputHeight.stringValue = "359"
            controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                         object: outputWidth))
            try render(controller.root, name: "recording-editor-custom-output-minimum-\(appearance)")
            outputMode.selectItem(withTitle: "Original")
            _ = outputMode.sendAction(outputMode.action, to: outputMode.target)

            worker.deferSave = true
            try button("Save new copy", in: controller.root).performClick(nil)
            XCTAssertFalse(systemVolume.isEnabled)
            XCTAssertFalse(try checkbox("Mute system audio", in: controller.root).isEnabled)
            XCTAssertFalse(controller.windowShouldClose(controller.window))
            XCTAssertFalse(controller.prepareForTermination())
            let cancel = try button("Cancel operation", in: controller.root)
            XCTAssertFalse(cancel.isHiddenOrHasHiddenAncestor); cancel.performClick(nil)
            worker.completeSave(.failure(AppBridgeError.backend("Export cancelled")))
            XCTAssertTrue(systemVolume.isEnabled)
            XCTAssertTrue(labels(in: controller.root).contains { $0.contains("Export cancelled") })
            try render(controller.root, name: "recording-editor-error-\(appearance)")

            let loadingWorker = FakeRecordingEditorWorker(presentation: try presentation(
                hasSystemAudio: true, hasMicrophoneAudio: true))
            loadingWorker.deferThumbnails = true
            let loadingController = RecordingEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: loadingWorker,
                confirmDiscard: { false })
            defer { loadingController.window.orderOut(nil) }
            loadingController.present(artifact: recordingArtifact(), historyRoot: "/History",
                                      outputDirectory: "/Exports")
            try render(loadingController.root,
                       name: "recording-editor-thumbnails-loading-\(appearance)")

            let failedWorker = FakeRecordingEditorWorker(presentation: try presentation(
                hasSystemAudio: true, hasMicrophoneAudio: true))
            failedWorker.thumbnailResult = .failure(AppBridgeError.backend("sprite unavailable"))
            let failedController = RecordingEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: failedWorker,
                confirmDiscard: { false })
            defer { failedController.window.orderOut(nil) }
            failedController.present(artifact: recordingArtifact(), historyRoot: "/History",
                                     outputDirectory: "/Exports")
            failedController.window.setContentSize(NSSize(width: 760, height: 540))
            try render(failedController.root,
                       name: "recording-editor-thumbnails-error-minimum-\(appearance)")
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

    func testRealBridgeThumbnailOwnerOutlivesSessionWithoutSnapshotMutation() throws {
        let tools = try NativeMediaTools.locate()
        let fixture = try makeRecordingFixture(tools: tools)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        var retainedImage: CGImage?
        do {
            let opened = try NativeRecordingEditorSession.open(historyRoot: fixture.history.path,
                                                                artifactID: fixture.id, tools: tools)
            let session = opened.0
            let before = try session.request(["operation": "snapshot"]).snapshot
            let owner = try session.thumbnails(cancel: try XCTUnwrap(NativeRecordingEditorCancel()))
            XCTAssertEqual(owner.frameCount, 12)
            XCTAssertEqual(owner.frameWidth, 160); XCTAssertEqual(owner.frameHeight, 90)
            XCTAssertEqual(owner.spriteWidth, 1_920); XCTAssertEqual(owner.spriteHeight, 90)
            retainedImage = try owner.image()
            let after = try session.request(["operation": "snapshot"]).snapshot
            XCTAssertEqual(after.revision, before.revision)
            XCTAssertEqual(after.positionMilliseconds, before.positionMilliseconds)
            XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.edit, options: [.sortedKeys]),
                           try JSONSerialization.data(withJSONObject: before.edit, options: [.sortedKeys]))
            XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.export, options: [.sortedKeys]),
                           try JSONSerialization.data(withJSONObject: before.export, options: [.sortedKeys]))
        }
        let image = try XCTUnwrap(retainedImage)
        XCTAssertEqual(image.width, 1_920); XCTAssertEqual(image.height, 90)
        XCTAssertEqual(CFDataGetLength(try XCTUnwrap(image.dataProvider?.data)), 1_920 * 90 * 4,
                       "the retained pixel provider remains readable after session and owner release")
    }

    func testRealBridgeCropOutputDimensionsContentAndImmutableOriginal() throws {
        let tools = try NativeMediaTools.locate()
        let fixture = try makeCropRecordingFixture(tools: tools)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let sourceBefore = try Data(contentsOf: fixture.source)
        let opened = try NativeRecordingEditorSession.open(historyRoot: fixture.history.path,
                                                            artifactID: fixture.id, tools: tools)
        let session = opened.0
        var edit = opened.1.snapshot.edit
        edit["crop"] = ["x": 60, "y": 24, "width": 80, "height": 48]
        edit["output_width"] = 40; edit["output_height"] = 24
        var export = opened.1.snapshot.export
        export["quality"] = "standard"; export["max_size_bytes"] = NSNull()
        let accepted = try session.request(["operation": "update_preview", "edit": edit,
                                            "export": export])
        let acceptedCrop = try XCTUnwrap(accepted.snapshot.edit["crop"] as? [String: Any])
        XCTAssertEqual((acceptedCrop["x"] as? NSNumber)?.uint32Value, 60)
        XCTAssertEqual((acceptedCrop["y"] as? NSNumber)?.uint32Value, 24)
        XCTAssertEqual((accepted.snapshot.edit["output_width"] as? NSNumber)?.uint32Value, 40)
        XCTAssertEqual((accepted.snapshot.edit["output_height"] as? NSNumber)?.uint32Value, 24)

        let path = fixture.root.appendingPathComponent("cropped.mp4")
        _ = try session.save(destination: path.path, export: export,
                             cancel: try XCTUnwrap(NativeRecordingEditorCancel()), progress: { _ in })
        XCTAssertEqual(try videoDimensions(path, tools: tools),
                       NativeRecordingDimensions(width: 40, height: 24))
        XCTAssertEqual(try historyDimensions(for: path.path, history: fixture.history),
                       NativeRecordingDimensions(width: 40, height: 24))
        let pixels = try decodedRGB(path, tools: tools, width: 40, height: 24)
        let green = rgb(pixels, width: 40, x: 3, y: 3)
        XCTAssertGreaterThan(green.1, green.0 + 30)
        XCTAssertGreaterThan(green.1, green.2 + 30,
                             "the nonzero crop origin keeps the green crop corner")
        let blue = rgb(pixels, width: 40, x: 25, y: 14)
        XCTAssertGreaterThan(blue.2, blue.0 + 30)
        XCTAssertGreaterThan(blue.2, blue.1 + 30,
                             "the scaled export retains the crop's blue interior marker")
        XCTAssertEqual(try Data(contentsOf: fixture.source), sourceBefore,
                       "crop and output-size export keeps the original byte-identical")
    }

    func testRealBridgeIndependentAudioGainMuteMonoHistoryAndImmutableOriginal() throws {
        let tools = try NativeMediaTools.locate()
        let fixture = try makeIndependentAudioFixture(tools: tools)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let sourceBefore = try Data(contentsOf: fixture.source)
        let opened = try NativeRecordingEditorSession.open(historyRoot: fixture.history.path,
                                                            artifactID: fixture.id, tools: tools)
        let session = opened.0
        XCTAssertTrue(opened.1.snapshot.hasSystemAudio)
        XCTAssertTrue(opened.1.snapshot.hasMicrophoneAudio)
        var export = opened.1.snapshot.export
        export["quality"] = "standard"; export["max_size_bytes"] = NSNull()

        var gainEdit = opened.1.snapshot.edit
        gainEdit["trim_start_ms"] = 100; gainEdit["trim_end_ms"] = 1_900
        var gainAudio = try XCTUnwrap(gainEdit["audio"] as? [String: Any])
        gainAudio["system_volume"] = Float(0.1234); gainAudio["microphone_volume"] = Float(0.333)
        gainEdit["audio"] = gainAudio
        let gain = try session.request(["operation": "update_preview", "edit": gainEdit,
                                        "export": export])
        XCTAssertTrue(gain.snapshot.hasSystemAudio); XCTAssertTrue(gain.snapshot.hasMicrophoneAudio)
        let acceptedGain = try XCTUnwrap(gain.snapshot.edit["audio"] as? [String: Any])
        XCTAssertEqual((acceptedGain["system_volume"] as? NSNumber)?.floatValue, Float(0.1234))
        XCTAssertEqual((acceptedGain["microphone_volume"] as? NSNumber)?.floatValue, Float(0.333))
        let gainPath = fixture.root.appendingPathComponent("gain.mp4")
        _ = try session.save(destination: gainPath.path, export: export,
                             cancel: try XCTUnwrap(NativeRecordingEditorCancel()), progress: { _ in })
        XCTAssertEqual(try audioChannelCount(gainPath, tools: tools), 2)
        try assertTones(gainPath, tools: tools, channels: 2,
                        expected: [[0.01234, 0.02664], [0.00617, 0.02664]])

        var systemEdit = gain.snapshot.edit
        var systemAudio = try XCTUnwrap(systemEdit["audio"] as? [String: Any])
        systemAudio["system_volume"] = Float(0.29)
        systemAudio["mute_microphone"] = true; systemEdit["audio"] = systemAudio
        let system = try session.request(["operation": "update_preview", "edit": systemEdit,
                                          "export": export])
        let acceptedSystem = try XCTUnwrap(system.snapshot.edit["audio"] as? [String: Any])
        XCTAssertEqual((acceptedSystem["system_volume"] as? NSNumber)?.floatValue, Float(0.29))
        let systemPath = fixture.root.appendingPathComponent("system-only.mp4")
        _ = try session.save(destination: systemPath.path, export: export,
                             cancel: try XCTUnwrap(NativeRecordingEditorCancel()), progress: { _ in })
        XCTAssertEqual(try audioChannelCount(systemPath, tools: tools), 2)
        try assertTones(systemPath, tools: tools, channels: 2,
                        expected: [[0.029, 0], [0.0145, 0]])

        var microphoneEdit = system.snapshot.edit
        var microphoneAudio = try XCTUnwrap(microphoneEdit["audio"] as? [String: Any])
        microphoneAudio["mute_system_audio"] = true
        microphoneAudio["mute_microphone"] = false
        microphoneAudio["mono_output"] = true
        microphoneEdit["audio"] = microphoneAudio
        _ = try session.request(["operation": "update_preview", "edit": microphoneEdit,
                                 "export": export])
        let microphonePath = fixture.root.appendingPathComponent("microphone-mono.mp4")
        _ = try session.save(destination: microphonePath.path, export: export,
                             cancel: try XCTUnwrap(NativeRecordingEditorCancel()), progress: { _ in })
        XCTAssertEqual(try audioChannelCount(microphonePath, tools: tools), 1)
        try assertTones(microphonePath, tools: tools, channels: 1,
                        expected: [[0, 0.02664 * sqrt(2.0)]])

        XCTAssertEqual(try historyAudioIdentity(for: systemPath.path, history: fixture.history),
                       AudioIdentity(system: true, microphone: false))
        XCTAssertEqual(try historyAudioIdentity(for: microphonePath.path, history: fixture.history),
                       AudioIdentity(system: false, microphone: true))
        XCTAssertEqual(try Data(contentsOf: fixture.source), sourceBefore,
                       "audio exports keep the original byte-identical")
    }

    private func presentation(start: UInt64 = 0, end: UInt64? = nil,
                              position: UInt64 = 0, revision: UInt64 = 0,
                              sourceWidth: Int = 320, sourceHeight: Int = 180,
                              crop: NativeRecordingCropRect? = nil,
                              output: NativeRecordingDimensions? = nil,
                              hasSystemAudio: Bool = false, hasMicrophoneAudio: Bool = false,
                              systemVolume: Double = 1, microphoneVolume: Double = 1,
                              muteSystem: Bool = false, muteMicrophone: Bool = false,
                              monoOutput: Bool = false) throws
        -> RecordingEditorPresentation {
        let endValue: Any = end.map { NSNumber(value: $0) } ?? NSNull()
        let cropValue: Any = crop == nil ? NSNull() : crop!.dictionary
        let outputWidth: Any = output.map { NSNumber(value: $0.width) } ?? NSNull()
        let outputHeight: Any = output.map { NSNumber(value: $0.height) } ?? NSNull()
        let snapshot = try XCTUnwrap(NativeRecordingEditorSnapshot([
            "artifact_id": "recording-id", "source": ["kind": "video", "mime_type": "video/mp4",
                "width": sourceWidth, "height": sourceHeight,
                "duration_ms": 2_000, "size_bytes": 1_024],
            "edit": ["trim_start_ms": start, "trim_end_ms": endValue,
                "crop": cropValue, "output_width": outputWidth, "output_height": outputHeight,
                "audio": ["system_volume": systemVolume, "microphone_volume": microphoneVolume,
                          "mute_system_audio": muteSystem, "mute_microphone": muteMicrophone,
                          "mono_output": monoOutput, "source_has_system_audio": hasSystemAudio,
                          "source_has_microphone_audio": hasMicrophoneAudio]],
            "preview_export": ["format": "mp4", "quality": "preserve",
                "max_size_bytes": 4_000_000, "frames_per_second": NSNull(),
                "gif_max_colors": NSNull()],
            "position_ms": position, "revision": revision,
            "has_system_audio": hasSystemAudio,
            "has_microphone_audio": hasMicrophoneAudio,
        ]))
        return RecordingEditorPresentation(snapshot: snapshot, image: try fixtureImage())
    }

    private func recordingArtifact(id: String = "recording-id") -> CaptureArtifact {
        CaptureArtifact(["entry": ["id": id, "kind": "video", "width": 320,
            "height": 180, "created_at": "2026-09-22T00:00:00Z"],
            "preview_path": "/History/\(id)/preview.png",
            "media_path": "/History/\(id)/media.mp4"])!
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

    private func makeIndependentAudioFixture(tools: NativeMediaTools) throws
        -> (root: URL, history: URL, source: URL, id: String) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let history = root.appendingPathComponent("History")
        let id = UUID().uuidString.lowercased()
        let directory = history.appendingPathComponent(id)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let source = directory.appendingPathComponent("media.mp4")
        try run(tools.ffmpeg, ["-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=12:duration=2",
            "-f", "lavfi", "-i", "aevalsrc=0.1*sin(2*PI*440*t)|0.05*sin(2*PI*440*t):s=48000:d=2",
            "-f", "lavfi", "-i", "aevalsrc=0.04*sin(2*PI*880*t)|0.12*sin(2*PI*880*t):s=48000:d=2",
            "-filter_complex", "[1:a]asplit=2[system][s];[2:a]asplit=2[mic][m];[s][m]amix=inputs=2:normalize=0[mixed]",
            "-map", "0:v", "-map", "[mixed]", "-map", "[system]", "-map", "[mic]",
            "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "256k", "-y", source.path])
        let sourceSize = (try FileManager.default.attributesOfItem(atPath: source.path)[.size]
                          as? NSNumber)?.intValue ?? 0
        let metadata: [String: Any] = ["id": id, "kind": "video",
            "preview_url": "capture-history://\(id)/preview",
            "full_url": "capture-history://\(id)/full", "width": 160, "height": 90,
            "size_bytes": sourceSize, "created_at": "2026-09-22T00:00:00Z",
            "mode": "display", "mime_type": "video/mp4", "duration_ms": 2_000,
            "target": ["type": "display", "display_id": "fixture"],
            "has_system_audio": true, "has_microphone_audio": true, "dropped_frames": 0]
        try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys])
            .write(to: directory.appendingPathComponent("metadata.json"))
        return (root, history, source, id)
    }

    private func makeCropRecordingFixture(tools: NativeMediaTools) throws
        -> (root: URL, history: URL, source: URL, id: String) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let history = root.appendingPathComponent("History")
        let id = UUID().uuidString.lowercased()
        let directory = history.appendingPathComponent(id)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let source = directory.appendingPathComponent("media.mp4")
        try run(tools.ffmpeg, ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
            "color=c=red:size=160x96:rate=12:duration=2,drawbox=x=60:y=24:w=80:h=48:color=green:t=fill,drawbox=x=100:y=42:w=20:h=20:color=blue:t=fill",
            "-c:v", "mpeg4", "-q:v", "2", "-pix_fmt", "yuv420p", "-y", source.path])
        let sourceSize = (try FileManager.default.attributesOfItem(atPath: source.path)[.size]
                          as? NSNumber)?.intValue ?? 0
        let metadata: [String: Any] = ["id": id, "kind": "video",
            "preview_url": "capture-history://\(id)/preview",
            "full_url": "capture-history://\(id)/full", "width": 160, "height": 96,
            "size_bytes": sourceSize, "created_at": "2026-09-22T00:00:00Z",
            "mode": "display", "mime_type": "video/mp4", "duration_ms": 2_000,
            "target": ["type": "display", "display_id": "fixture"]]
        try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys])
            .write(to: directory.appendingPathComponent("metadata.json"))
        return (root, history, source, id)
    }

    private struct AudioIdentity: Equatable {
        let system: Bool
        let microphone: Bool
    }

    private func historyAudioIdentity(for path: String, history: URL) throws -> AudioIdentity {
        for directory in try FileManager.default.contentsOfDirectory(at: history,
            includingPropertiesForKeys: nil) {
            let metadata = directory.appendingPathComponent("metadata.json")
            guard let data = try? Data(contentsOf: metadata),
                  let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  value["saved_path"] as? String == path else { continue }
            return AudioIdentity(system: value["has_system_audio"] as? Bool ?? false,
                                 microphone: value["has_microphone_audio"] as? Bool ?? false)
        }
        throw AppBridgeError.invalidResponse
    }

    private func historyDimensions(for path: String, history: URL) throws
        -> NativeRecordingDimensions {
        for directory in try FileManager.default.contentsOfDirectory(at: history,
            includingPropertiesForKeys: nil) {
            let metadata = directory.appendingPathComponent("metadata.json")
            guard let data = try? Data(contentsOf: metadata),
                  let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  value["saved_path"] as? String == path,
                  let width = (value["width"] as? NSNumber)?.uint32Value,
                  let height = (value["height"] as? NSNumber)?.uint32Value else { continue }
            return NativeRecordingDimensions(width: width, height: height)
        }
        throw AppBridgeError.invalidResponse
    }

    private func videoDimensions(_ path: URL, tools: NativeMediaTools) throws
        -> NativeRecordingDimensions {
        let data = try run(tools.ffprobe, ["-v", "error", "-select_streams", "v:0",
            "-show_entries", "stream=width,height", "-of", "json", path.path])
        let value = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let streams = try XCTUnwrap(value["streams"] as? [[String: Any]])
        let stream = try XCTUnwrap(streams.first)
        return NativeRecordingDimensions(
            width: try XCTUnwrap((stream["width"] as? NSNumber)?.uint32Value),
            height: try XCTUnwrap((stream["height"] as? NSNumber)?.uint32Value))
    }

    private func decodedRGB(_ path: URL, tools: NativeMediaTools, width: Int, height: Int) throws
        -> [UInt8] {
        let data = try run(tools.ffmpeg, ["-v", "error", "-i", path.path,
            "-frames:v", "1", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        XCTAssertEqual(data.count, width * height * 3)
        return Array(data)
    }

    private func rgb(_ pixels: [UInt8], width: Int, x: Int, y: Int)
        -> (Int, Int, Int) {
        let offset = (y * width + x) * 3
        return (Int(pixels[offset]), Int(pixels[offset + 1]), Int(pixels[offset + 2]))
    }

    private func audioChannelCount(_ path: URL, tools: NativeMediaTools) throws -> Int {
        let data = try run(tools.ffprobe, ["-v", "error", "-select_streams", "a:0",
            "-show_entries", "stream=channels", "-of", "json", path.path])
        let value = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let streams = try XCTUnwrap(value["streams"] as? [[String: Any]])
        return try XCTUnwrap((streams.first?["channels"] as? NSNumber)?.intValue)
    }

    private func assertTones(_ path: URL, tools: NativeMediaTools, channels: Int,
                             expected: [[Double]], file: StaticString = #filePath,
                             line: UInt = #line) throws {
        let data = try run(tools.ffmpeg, ["-v", "error", "-ss", "0.3", "-i", path.path,
            "-map", "0:a:0", "-t", "0.2", "-ar", "48000", "-f", "f32le", "-"])
        let samples = data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
        XCTAssertEqual(samples.count, 9_600 * channels, file: file, line: line)
        guard samples.count == 9_600 * channels else { return }
        for channel in 0..<channels {
            for (index, frequency) in [440.0, 880.0].enumerated() {
                var real = 0.0, imaginary = 0.0
                for sample in 0..<9_600 {
                    let value = Double(samples[sample * channels + channel])
                    let phase = 2 * Double.pi * frequency * Double(sample) / 48_000
                    real += value * cos(phase); imaginary += value * sin(phase)
                }
                let measured = 2 * hypot(real, imaginary) / 9_600
                let target = expected[channel][index]
                XCTAssertEqual(measured, target, accuracy: max(0.001, target * 0.15),
                               "channel \(channel), \(Int(frequency)) Hz", file: file, line: line)
            }
        }
    }

    @discardableResult private func run(_ executable: String, _ arguments: [String]) throws -> Data {
        let process = Process(); let output = Pipe(); let errors = Pipe()
        process.executableURL = URL(fileURLWithPath: executable); process.arguments = arguments
        process.standardOutput = output; process.standardError = errors
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        let error = errors.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw AppBridgeError.backend(String(decoding: error, as: UTF8.self))
        }
        return data
    }

    private func fixtureImage() throws -> CGImage {
        let bytes = [UInt8](repeating: 80, count: 16 * 9 * 4)
        let provider = try XCTUnwrap(CGDataProvider(data: Data(bytes) as CFData))
        return try XCTUnwrap(CGImage(width: 16, height: 9, bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: 64, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: false,
                       intent: .defaultIntent))
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
    private func checkbox(_ label: String, in view: NSView) throws -> NSButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == label })
    }
    private func button(_ title: String, in view: NSView) throws -> CaptureButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? CaptureButton }.first { $0.title == title })
    }
    private func windowHit(_ handle: RecordingTrimHandle,
                           in controller: RecordingEditorController) throws -> NSView {
        let point = handle.convert(NSPoint(x: handle.bounds.midX, y: handle.bounds.midY), to: nil)
        return try XCTUnwrap(controller.window.contentView?.hitTest(point))
    }
    private func dispatchMouse(_ type: NSEvent.EventType, to handle: RecordingTrimHandle,
                               in controller: RecordingEditorController,
                               deltaX: CGFloat = 0) throws {
        var point = handle.convert(NSPoint(x: handle.bounds.midX, y: handle.bounds.midY), to: nil)
        point.x += deltaX
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point,
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: controller.window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .leftMouseDown: try XCTUnwrap(controller.window.contentView?.hitTest(point)).mouseDown(with: event)
        case .leftMouseDragged: handle.mouseDragged(with: event)
        case .leftMouseUp: handle.mouseUp(with: event)
        default: XCTFail("Unsupported pointer event")
        }
    }
    private func labels(in view: NSView) -> [String] {
        descendants(in: view).compactMap { ($0 as? NSTextField)?.stringValue }
    }
    private func render(_ view: NSView, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        view.displayIfNeeded()
        let rep = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: rep)
        let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(name).png"))
    }
}

private final class FakeRecordingEditorWorker: RecordingEditorWorking {
    var initial: RecordingEditorPresentation
    var openCount = 0
    var deferOpen = false
    var requests: [[String: Any]] = []
    var requestResult: Result<RecordingEditorPresentation, Error>?
    var estimateResult: Result<RecordingEditorEstimate, Error> = .failure(AppBridgeError.backend("estimate unavailable"))
    var thumbnailResult: Result<CGImage, Error> = .success(fakeTimelineImage())
    var thumbnailCalls = 0
    var deferThumbnails = false
    var saveResult: Result<RecordingEditorSaveResult, Error> = .failure(AppBridgeError.backend("save unavailable"))
    var saves: [(destination: String, export: [String: Any])] = []
    var deferSave = false
    var closeCount = 0
    weak var observedSaveCancel: NativeRecordingEditorCancel?
    weak var observedThumbnailCancel: NativeRecordingEditorCancel?
    private var pendingOpen: ((Result<RecordingEditorPresentation, Error>) -> Void)?
    private var pendingSave: ((Result<RecordingEditorSaveResult, Error>) -> Void)?
    private var pendingThumbnails: ((Result<CGImage, Error>) -> Void)?

    init(presentation: RecordingEditorPresentation) { initial = presentation }
    func open(historyRoot: String, artifactID: String,
              completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void) {
        openCount += 1
        if deferOpen { pendingOpen = completion }
        else { completion(.success(initial)) }
    }
    func completeOpen(_ result: Result<RecordingEditorPresentation, Error>) {
        let completion = pendingOpen; pendingOpen = nil; completion?(result)
    }
    func request(_ object: [String: Any],
                 completion: @escaping (Result<RecordingEditorPresentation, Error>) -> Void) {
        requests.append(object); completion(requestResult ?? .success(initial))
    }
    func estimate(cancel: NativeRecordingEditorCancel,
                  completion: @escaping (Result<RecordingEditorEstimate, Error>) -> Void) {
        completion(estimateResult)
    }
    func thumbnails(cancel: NativeRecordingEditorCancel,
                    completion: @escaping (Result<CGImage, Error>) -> Void) {
        thumbnailCalls += 1; observedThumbnailCancel = cancel
        if deferThumbnails { pendingThumbnails = completion }
        else { completion(thumbnailResult) }
    }
    func completeThumbnails(_ result: Result<CGImage, Error>) {
        let completion = pendingThumbnails; pendingThumbnails = nil; completion?(result)
    }
    func save(destination: String, export: [String: Any], cancel: NativeRecordingEditorCancel,
              progress: @escaping (RecordingEditorProgress) -> Void,
              completion: @escaping (Result<RecordingEditorSaveResult, Error>) -> Void) {
        saves.append((destination, export))
        observedSaveCancel = cancel
        if deferSave { pendingSave = completion }
        else { completion(saveResult) }
    }
    func completeSave(_ result: Result<RecordingEditorSaveResult, Error>) {
        let completion = pendingSave; pendingSave = nil; completion?(result)
    }
    func close() { closeCount += 1 }
}

private func fakeTimelineImage() -> CGImage {
    let width = 1_920, height = 90
    var bytes = [UInt8](repeating: 255, count: width * height * 4)
    for y in 0..<height {
        for x in 0..<width {
            let frame = x / 160
            let offset = (y * width + x) * 4
            bytes[offset] = UInt8(28 + frame * 17)
            bytes[offset + 1] = UInt8(190 - frame * 9)
            bytes[offset + 2] = UInt8(52 + frame * 11)
        }
    }
    let provider = CGDataProvider(data: Data(bytes) as CFData)!
    return CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                   bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                   bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                   provider: provider, decode: nil, shouldInterpolate: false,
                   intent: .defaultIntent)!
}
