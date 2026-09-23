import AppKit
import ImageIO
import XCTest
@testable import CapturesNative

final class RecordingEditorTests: XCTestCase {
    func testPlaybackMailboxCoalescesNaturalEOFAndRejectsCancelledPendingFrame() throws {
        var cancelled = false
        var positions: [UInt64] = []
        var outcomes: [RecordingPlaybackCompletion] = []
        let natural = expectation(description: "natural EOF drains latest frame first")
        let delivery = RecordingPlaybackDelivery(shouldDiscardFrames: { cancelled },
            frame: { positions.append($0.positionMilliseconds) }, completion: { result in
                if case .success(let outcome) = result { outcomes.append(outcome) }
                natural.fulfill()
            })
        delivery.offer(RecordingPlaybackImage(positionMilliseconds: 100,
                                              image: try solidImage(red: 1, green: 2, blue: 3)))
        delivery.offer(RecordingPlaybackImage(positionMilliseconds: 200,
                                              image: try solidImage(red: 4, green: 5, blue: 6)))
        delivery.finish(.success(.eof))
        wait(for: [natural], timeout: 1)
        XCTAssertEqual(positions, [200], "one-slot delivery replaces a stale pending frame")
        XCTAssertEqual(outcomes, [.eof])

        let paused = expectation(description: "cancel rejects pending frame")
        let cancelledDelivery = RecordingPlaybackDelivery(shouldDiscardFrames: { cancelled },
            frame: { positions.append($0.positionMilliseconds) }, completion: { _ in paused.fulfill() })
        cancelledDelivery.offer(RecordingPlaybackImage(positionMilliseconds: 300,
            image: try solidImage(red: 7, green: 8, blue: 9)))
        cancelled = true
        cancelledDelivery.finish(.failure(AppBridgeError.backend("operation cancelled")))
        wait(for: [paused], timeout: 1)
        XCTAssertEqual(positions, [200], "Pause never presents a frame pending at cancellation")
    }

    func testSilentPlaybackPauseResumeEOFAndAcceptedStateGates() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            start: 200, end: 1_800, position: 400))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let play = try button("Play", in: controller.root)
        let seek = try slider("Recording frame position", in: controller.root)
        let save = try button("Save new copy", in: controller.root)
        let estimate = try button("Estimate size", in: controller.root)
        let trimStart = try field("Trim start milliseconds", in: controller.root)

        play.performClick(nil)
        XCTAssertEqual(worker.playbackStarts, [400])
        XCTAssertEqual(play.title, "Pause")
        XCTAssertFalse(seek.isEnabled); XCTAssertFalse(save.isEnabled)
        XCTAssertFalse(estimate.isEnabled); XCTAssertFalse(trimStart.isEnabled)
        XCTAssertFalse(controller.dirty)
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 650,
                                                         image: try solidImage(red: 20, green: 210, blue: 30)))
        XCTAssertEqual(seek.doubleValue, 650)
        XCTAssertTrue(worker.requests.isEmpty); XCTAssertTrue(worker.saves.isEmpty)
        XCTAssertFalse(controller.dirty, "motion frames never mutate accepted editor identity")

        play.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(worker.observedPlaybackCancel).isCancelled)
        XCTAssertEqual(play.title, "Pausing…")
        XCTAssertFalse(play.isEnabled, "controls stay gated until decoder teardown finishes")
        worker.completePlayback(.success(.cancelled))
        XCTAssertEqual(play.title, "Play"); XCTAssertTrue(play.isEnabled)
        XCTAssertEqual(seek.doubleValue, 650, "Pause retains the last transient playhead")

        play.performClick(nil)
        XCTAssertEqual(worker.playbackStarts, [400, 650], "Play resumes the last presented source time")
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 1_700,
                                                         image: try solidImage(red: 30, green: 40, blue: 220)))
        worker.completePlayback(.success(.eof))
        XCTAssertEqual(seek.doubleValue, 1_700)
        play.performClick(nil)
        XCTAssertEqual(worker.playbackStarts, [400, 650, 200],
                       "Play after EOF restarts at accepted trim start")
    }

    func testLoopToggleWrapDisableAndPauseCancellationStayOnePlaybackOperation() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            start: 211, end: 1_611, position: 537))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let play = try button("Play", in: controller.root)
        let loop = try checkbox("Loop silent recording preview", in: controller.root)
        let seek = try slider("Recording frame position", in: controller.root)

        XCTAssertEqual(loop.state, .off)
        loop.performClick(nil)
        XCTAssertFalse(controller.dirty)
        play.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(worker.observedPlaybackLoop).isEnabled)
        XCTAssertEqual(worker.playbackStarts, [537])

        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 1_403,
            image: try solidImage(red: 10, green: 190, blue: 30)))
        worker.completePlayback(.success(.eof))
        XCTAssertEqual(worker.playbackStarts, [537, 211])
        XCTAssertEqual(play.title, "Pause", "looping never exposes an idle export gate")
        XCTAssertFalse(seek.isEnabled)

        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 389,
            image: try solidImage(red: 30, green: 40, blue: 210)))
        loop.performClick(nil)
        XCTAssertEqual(loop.state, .off)
        worker.completePlayback(.success(.eof))
        XCTAssertEqual(worker.playbackStarts, [537, 211],
                       "turning Loop off finishes the current lap without another decoder")
        XCTAssertEqual(play.title, "Play")
        XCTAssertEqual(seek.doubleValue, 389)

        loop.performClick(nil); play.performClick(nil)
        XCTAssertEqual(worker.playbackStarts.last, 211,
                       "Play after completed EOF restarts at the accepted trim start")
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 1_500,
            image: try solidImage(red: 200, green: 30, blue: 40)))
        worker.completePlayback(.success(.eof))
        XCTAssertEqual(Array(worker.playbackStarts.suffix(2)), [211, 211],
                       "the next nonempty lap reopens at the same accepted trim start")
        play.performClick(nil)
        XCTAssertEqual(play.title, "Pausing…")
        XCTAssertFalse(loop.isEnabled, "Loop cannot change while decoder cancellation is pending")
        worker.completePlayback(.success(.eof))
        XCTAssertEqual(play.title, "Play")
        XCTAssertEqual(Array(worker.playbackStarts.suffix(2)), [211, 211],
                       "Pause at a wrap never restarts playback")
        XCTAssertEqual(loop.state, .on, "Pause retains the item-local Loop preference")
    }

    func testLoopEmptyErrorApplySeekAndNewItemLifecycle() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            start: 200, end: 1_800, position: 400))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let loop = try checkbox("Loop silent recording preview", in: controller.root)
        let play = try button("Play", in: controller.root)
        let estimate = try button("Estimate size", in: controller.root)
        let save = try button("Save new copy", in: controller.root)
        loop.performClick(nil)
        XCTAssertFalse(controller.dirty); XCTAssertTrue(estimate.isEnabled); XCTAssertTrue(save.isEnabled)

        play.performClick(nil)
        worker.completePlayback(.success(.empty))
        XCTAssertEqual(worker.playbackStarts, [400], "zero-frame EOF cannot spin")
        XCTAssertEqual(play.title, "Play"); XCTAssertEqual(loop.state, .on)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("without frames") })

        play.performClick(nil)
        worker.completePlayback(.failure(AppBridgeError.backend("loop decoder failed")))
        XCTAssertEqual(worker.playbackStarts, [400, 200])
        XCTAssertEqual(loop.state, .on, "playback errors retain the item-local preference")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("loop decoder failed") })

        let trimEnd = try field("Trim end milliseconds", in: controller.root)
        trimEnd.stringValue = "1600"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: trimEnd))
        worker.requestResult = .success(try presentation(start: 200, end: 1_600,
                                                         position: 400, revision: 1))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertEqual(loop.state, .on)
        let acceptedDirty = controller.dirty
        let requestCount = worker.requests.count
        worker.estimateResult = .success(RecordingEditorEstimate(sizeBytes: 1_500_000,
                                                                  exact: false))
        estimate.performClick(nil)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("≈") && $0.contains("MB") })
        loop.performClick(nil); loop.performClick(nil)
        XCTAssertEqual(controller.dirty, acceptedDirty,
                       "Loop never changes accepted or saved editor identity")
        XCTAssertEqual(worker.requests.count, requestCount,
                       "Loop is host-only and never enters the session snapshot")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("≈") && $0.contains("MB") },
                      "Loop leaves the accepted-settings estimate intact")
        XCTAssertTrue(worker.saves.isEmpty); XCTAssertEqual(worker.openCount, 1)
        XCTAssertTrue(estimate.isEnabled); XCTAssertTrue(save.isEnabled)

        let seek = try slider("Recording frame position", in: controller.root)
        worker.requestResult = .success(try presentation(start: 200, end: 1_600,
                                                         position: 733, revision: 2))
        seek.doubleValue = 733; _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertEqual(loop.state, .on, "Apply and Seek retain Loop for the current item")

        let cleanWorker = FakeRecordingEditorWorker(presentation: try presentation())
        let cleanController = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                        worker: cleanWorker, confirmDiscard: { false })
        defer { cleanController.window.orderOut(nil) }
        cleanController.present(artifact: recordingArtifact(), historyRoot: "/History",
                                outputDirectory: "/Exports")
        let cleanLoop = try checkbox("Loop silent recording preview", in: cleanController.root)
        cleanLoop.performClick(nil)
        cleanController.present(artifact: recordingArtifact(id: "next-recording"),
                                historyRoot: "/History", outputDirectory: "/Exports")
        XCTAssertEqual(cleanLoop.state, .off, "each History item defaults Loop off")
        XCTAssertFalse(cleanController.dirty)
    }

    func testSilentPlaybackErrorRestoresAcceptedFrameAndAllowsRetry() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            start: 100, end: 1_500, position: 300))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let play = try button("Play", in: controller.root)
        let seek = try slider("Recording frame position", in: controller.root)
        play.performClick(nil)
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 700,
                                                         image: try solidImage(red: 1, green: 240, blue: 2)))
        XCTAssertEqual(seek.doubleValue, 700)
        worker.completePlayback(.failure(AppBridgeError.backend("decoder stopped")))
        XCTAssertEqual(seek.doubleValue, 300)
        XCTAssertEqual(play.title, "Play"); XCTAssertTrue(play.isEnabled)
        XCTAssertFalse(controller.dirty)
        XCTAssertTrue(labels(in: controller.root).contains {
            $0.contains("accepted preview was restored")
        })
        play.performClick(nil)
        XCTAssertEqual(worker.playbackStarts, [300, 300], "an error retries from accepted position")
    }

    func testSeekPreservesRequestedTargetAtIdleAndAfterTransientPlayback() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(position: 100))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let seek = try slider("Recording frame position", in: controller.root)

        worker.requestResult = .success(try presentation(position: 777, revision: 1))
        seek.doubleValue = 777; _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertEqual((worker.requests.last?["position_ms"] as? NSNumber)?.uint64Value, 777,
                       "ordinary seek keeps the newly requested asymmetric target")

        let play = try button("Play", in: controller.root)
        play.performClick(nil)
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 913,
            image: try solidImage(red: 10, green: 20, blue: 30)))
        play.performClick(nil); worker.completePlayback(.success(.cancelled))
        XCTAssertEqual(seek.doubleValue, 913)
        worker.requestResult = .success(try presentation(position: 1_237, revision: 2))
        seek.doubleValue = 1_237; _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertEqual((worker.requests.last?["position_ms"] as? NSNumber)?.uint64Value, 1_237,
                       "restoring the accepted still cannot replace a post-playback seek target")
        XCTAssertEqual(seek.doubleValue, 1_237)
    }

    func testPauseBeforeDelayedPlaybackStartRetainsAcceptedDisplayedPosition() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            start: 200, end: 1_800, position: 1_937))
        worker.deferPlayback = true
        worker.deferPlaybackStarted = true
        worker.playbackMetadata = RecordingPlaybackMetadata(startPositionMilliseconds: 200,
                                                             width: 640, height: 360,
                                                             framesPerSecond: 24)
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let play = try button("Play", in: controller.root)
        let seek = try slider("Recording frame position", in: controller.root)
        let preview = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSImageView }.first)
        let acceptedFrame = preview.image

        play.performClick(nil)
        XCTAssertEqual(worker.playbackStarts, [1_937])
        play.performClick(nil)
        XCTAssertEqual(play.title, "Pausing…")
        XCTAssertEqual(seek.doubleValue, 1_937)

        worker.completePlaybackStarted()
        XCTAssertEqual(play.title, "Pausing…", "late metadata cannot replace the Pausing state")
        XCTAssertTrue(labels(in: controller.root).contains("Pausing silent playback…"))
        XCTAssertEqual(seek.doubleValue, 1_937,
                       "zero-frame Pause retains the accepted still that was actually displayed")
        XCTAssertTrue(preview.image === acceptedFrame)
        worker.completePlayback(.success(.cancelled))

        play.performClick(nil)
        XCTAssertEqual(worker.playbackStarts, [1_937, 1_937],
                       "the next Play resumes the accepted displayed source position")
    }

    func testPlaybackControlsDoNotOverlapAtSupportedWidths() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(position: 0))
        let tokens = Tokens.variants["light-mustard"]!
        let controller = RecordingEditorController(tokens: tokens,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let play = try button("Play", in: controller.root)
        let seek = try slider("Recording frame position", in: controller.root)
        let loop = try checkbox("Loop silent recording preview", in: controller.root)
        let timestamp = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSTextField }
            .first { !$0.isEditable && $0.stringValue.contains(" / ") })

        XCTAssertEqual(seek.doubleValue, 0, "layout coverage keeps the playhead at source start")
        for size in [NSSize(width: 760, height: 540), NSSize(width: 960, height: 760)] {
            controller.window.setContentSize(size)
            XCTAssertFalse(play.isHidden); XCTAssertFalse(loop.isHidden)
            XCTAssertFalse(seek.isHidden); XCTAssertFalse(timestamp.isHidden)
            XCTAssertGreaterThan(seek.frame.width, 0)
            XCTAssertTrue(controller.root.bounds.intersects(play.frame))
            XCTAssertTrue(controller.root.bounds.intersects(loop.frame))
            XCTAssertTrue(controller.root.bounds.intersects(seek.frame))
            XCTAssertTrue(controller.root.bounds.intersects(timestamp.frame))
            let gap = tokens.number("s-2")
            XCTAssertLessThanOrEqual(play.frame.maxX + gap, loop.frame.minX,
                                     "Play must not overlap Loop at width \(size.width)")
            XCTAssertLessThanOrEqual(loop.frame.maxX + gap, seek.frame.minX,
                                     "Loop must not overlap the seek slider at width \(size.width)")
            XCTAssertLessThanOrEqual(seek.frame.maxX + gap, timestamp.frame.minX,
                                     "the seek slider must not overlap its timestamp at width \(size.width)")
        }
    }

    func testPreviewFitActualSizeLifecycleIsDisplayOnlyAndItemLocal() throws {
        _ = NSApplication.shared
        let accepted = try presentation(position: 417, sourceWidth: 1_600, sourceHeight: 900,
                                        previewWidth: 1_200, previewHeight: 800)
        let worker = FakeRecordingEditorWorker(presentation: accepted)
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        controller.window.setContentSize(NSSize(width: 760, height: 540))
        let fit = try button("Fit", in: controller.root)
        let actual = try button("100%", in: controller.root)
        let image = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSImageView }.first)
        let scroll = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSScrollView }
            .first { $0.accessibilityLabel() == "Recording preview viewport" })
        XCTAssertTrue(fit.selected); XCTAssertFalse(actual.selected)
        XCTAssertLessThan(image.frame.width, 1_200)

        worker.estimateResult = .success(RecordingEditorEstimate(sizeBytes: 4_567, exact: true))
        try button("Estimate size", in: controller.root).performClick(nil)
        let estimateText = labels(in: controller.root).first { $0.contains("KB") }
        let dirty = controller.dirty
        let requests = worker.requests.count
        let acceptedImage = image.image
        actual.performClick(nil)
        XCTAssertFalse(fit.selected); XCTAssertTrue(actual.selected)
        XCTAssertEqual(image.frame.size, NSSize(width: 1_200, height: 800),
                       "100% is one decoded image pixel per logical point")
        XCTAssertTrue(scroll.hasHorizontalScroller); XCTAssertTrue(scroll.hasVerticalScroller)
        XCTAssertTrue(image.image === acceptedImage)
        XCTAssertEqual(controller.dirty, dirty); XCTAssertEqual(worker.requests.count, requests)
        XCTAssertEqual(labels(in: controller.root).first { $0.contains("KB") }, estimateText,
                       "display scale leaves the accepted estimate intact")

        let play = try button("Play", in: controller.root)
        play.performClick(nil)
        XCTAssertTrue(actual.isEnabled, "display scale remains available during playback")
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 733,
            image: try fixtureImage(width: 640, height: 360)))
        XCTAssertTrue(actual.selected)
        XCTAssertEqual(image.frame.size, NSSize(width: 640, height: 360),
                       "100% follows the current decoded motion-frame dimensions")
        fit.performClick(nil); actual.performClick(nil)
        XCTAssertEqual(worker.playbackStarts, [417])
        XCTAssertEqual(worker.requests.count, requests,
                       "scale changes never acquire a worker operation")
        play.performClick(nil); worker.completePlayback(.success(.cancelled))
        XCTAssertTrue(actual.selected, "Pause retains the item-local display preference")

        let trimEnd = try field("Trim end milliseconds", in: controller.root)
        trimEnd.stringValue = "1700"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: trimEnd))
        worker.requestResult = .success(try presentation(end: 1_700, position: 417, revision: 1,
            sourceWidth: 1_600, sourceHeight: 900, previewWidth: 480, previewHeight: 270))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertTrue(actual.selected, "Apply preserves 100%")

        worker.requestResult = .success(try presentation(end: 1_700, position: 811, revision: 2,
            sourceWidth: 1_600, sourceHeight: 900, previewWidth: 320, previewHeight: 120))
        let seek = try slider("Recording frame position", in: controller.root)
        seek.doubleValue = 811; _ = seek.sendAction(seek.action, to: seek.target)
        XCTAssertTrue(actual.selected, "Seek preserves 100%")
        XCTAssertEqual(image.frame.size, NSSize(width: 320, height: 120))
        XCTAssertEqual(image.frame.midX, scroll.documentView!.bounds.midX, accuracy: 0.5)
        XCTAssertEqual(image.frame.midY, scroll.documentView!.bounds.midY, accuracy: 0.5,
                       "smaller decoded images remain centered")

        let cleanWorker = FakeRecordingEditorWorker(presentation: accepted)
        let cleanController = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                        worker: cleanWorker,
                                                        confirmDiscard: { false })
        defer { cleanController.window.orderOut(nil) }
        cleanController.present(artifact: recordingArtifact(), historyRoot: "/History",
                                outputDirectory: "/Exports")
        try button("100%", in: cleanController.root).performClick(nil)
        cleanController.present(artifact: recordingArtifact(id: "next-recording"),
                                historyRoot: "/History", outputDirectory: "/Exports")
        XCTAssertTrue(try button("Fit", in: cleanController.root).selected)
        XCTAssertFalse(try button("100%", in: cleanController.root).selected,
                       "each History item defaults to Fit")
    }

    func testActualSizeCropUsesScrolledImageRectAndEndsGestureOnViewportChanges() throws {
        _ = NSApplication.shared
        let crop = NativeRecordingCropRect(x: 140, y: 90, width: 500, height: 300)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            position: 433, sourceWidth: 1_200, sourceHeight: 800, crop: crop,
            previewWidth: 600, previewHeight: 400))
        worker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 433,
            image: try fixtureImage(width: 1_200, height: 800)))
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        controller.window.setContentSize(NSSize(width: 760, height: 540))
        try button("100%", in: controller.root).performClick(nil)
        try button("Adjust crop", in: controller.root).performClick(nil)
        let overlay = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingCropOverlay }.first)
        let image = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSImageView }.first)
        let scroll = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSScrollView }
            .first { $0.accessibilityLabel() == "Recording preview viewport" })
        XCTAssertEqual(overlay.fittedImageRect, image.frame)
        XCTAssertEqual(image.frame.size, NSSize(width: 1_200, height: 800))

        scroll.contentView.scroll(to: NSPoint(x: 113, y: 71))
        scroll.reflectScrolledClipView(scroll.contentView)
        let beforeX = try field("Recording crop X", in: controller.root).stringValue
        let start = NSPoint(x: overlay.displayedCropRect.midX,
                            y: overlay.displayedCropRect.midY)
        overlay.beginDrag(.move, at: start)
        scroll.contentView.scroll(to: NSPoint(x: 151, y: 109))
        scroll.reflectScrolledClipView(scroll.contentView)
        overlay.continueDrag(at: NSPoint(x: start.x + 23, y: start.y + 17))
        XCTAssertEqual(try field("Recording crop X", in: controller.root).stringValue, beforeX,
                       "scrolling ends an active crop gesture")

        try dispatchCropOverlayMouse(.leftMouseDown, at: start, to: overlay, in: controller)
        try dispatchCropOverlayMouse(.leftMouseDragged, at: start, to: overlay,
                                     in: controller, deltaX: 13, deltaY: 7)
        try dispatchCropOverlayMouse(.leftMouseUp, at: start, to: overlay,
                                     in: controller, deltaX: 13, deltaY: 7)
        XCTAssertEqual(try field("Recording crop X", in: controller.root).stringValue, "153")
        XCTAssertEqual(try field("Recording crop Y", in: controller.root).stringValue, "97",
                       "actual-size crop mapping is one source pixel per point after scrolling")
        XCTAssertTrue(worker.requests.isEmpty)

        overlay.beginDrag(.move, at: NSPoint(x: overlay.displayedCropRect.midX,
                                             y: overlay.displayedCropRect.midY))
        try button("Fit", in: controller.root).performClick(nil)
        let staged = try field("Recording crop X", in: controller.root).stringValue
        overlay.continueDrag(at: NSPoint(x: overlay.displayedCropRect.midX + 30,
                                         y: overlay.displayedCropRect.midY + 20))
        XCTAssertEqual(try field("Recording crop X", in: controller.root).stringValue, staged,
                       "scale changes end active crop gestures")
        XCTAssertTrue(controller.dirty, "the staged crop remains pending without publishing")
        XCTAssertTrue(worker.requests.isEmpty)
    }

    func testPlaybackPauseCompletesBeforeSessionSwitchAndTerminationRetry() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(position: 250))
        worker.deferPlayback = true
        var terminationRequests = 0
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
            worker: worker, confirmDiscard: { false },
            requestTermination: { terminationRequests += 1 })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        try button("Play", in: controller.root).performClick(nil)
        controller.present(artifact: recordingArtifact(id: "next-recording"),
                           historyRoot: "/History", outputDirectory: "/Exports")
        XCTAssertTrue(try XCTUnwrap(worker.observedPlaybackCancel).isCancelled)
        XCTAssertEqual(worker.openCount, 1, "new session waits for decoder teardown")
        worker.completePlayback(.success(.cancelled))
        XCTAssertEqual(worker.openCount, 2)

        try button("Play", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.prepareForTermination())
        XCTAssertEqual(terminationRequests, 0)
        worker.completePlayback(.success(.cancelled))
        XCTAssertEqual(terminationRequests, 1,
                       "quit retries only after playback process teardown completes")
    }

    func testPlaybackFocusLossAndMiniaturizePauseWithoutPostStopFrames() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(position: 100))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let play = try button("Play", in: controller.root)
        let seek = try slider("Recording frame position", in: controller.root)
        play.performClick(nil)
        controller.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification,
                                                   object: controller.window))
        XCTAssertTrue(try XCTUnwrap(worker.observedPlaybackCancel).isCancelled)
        worker.completePlayback(.success(.cancelled))
        let stoppedPosition = seek.doubleValue
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 900,
                                                         image: try solidImage(red: 2, green: 3, blue: 4)))
        XCTAssertEqual(seek.doubleValue, stoppedPosition,
                       "completed playback drops queued or stale frame delivery")

        play.performClick(nil)
        controller.windowDidMiniaturize(Notification(name: NSWindow.didMiniaturizeNotification,
                                                     object: controller.window))
        XCTAssertEqual(play.title, "Pausing…")
    }

    func testPlaybackCloseWaitsForDecoderTeardownBeforeClosingSession() throws {
        _ = NSApplication.shared
        let worker = FakeRecordingEditorWorker(presentation: try presentation(position: 100))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { true })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        XCTAssertFalse(controller.window.isReleasedWhenClosed,
                       "ARC retains the controller's window across close and reopen")
        try button("Play", in: controller.root).performClick(nil)
        controller.window.performClose(nil)
        XCTAssertTrue(try XCTUnwrap(worker.observedPlaybackCancel).isCancelled)
        XCTAssertEqual(worker.closeCount, 0)
        worker.completePlayback(.success(.cancelled))
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.01))
        XCTAssertEqual(worker.closeCount, 1,
                       "session close is queued only after playback Drop finishes")
        XCTAssertFalse(controller.window.isVisible,
                       "the completed close is not reentrant with playback teardown")

        controller.present(artifact: recordingArtifact(id: "reopened-recording"),
                           historyRoot: "/History", outputDirectory: "/Exports")
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.01))
        XCTAssertTrue(controller.window.isVisible)
        XCTAssertEqual(worker.openCount, 2,
                       "the retained controller and window reopen after actual close")
    }

    func testSilentPlaybackRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeRecordingEditorWorker(presentation: try presentation(
                start: 200, end: 1_800, position: 400,
                hasSystemAudio: true, hasMicrophoneAudio: true))
            worker.deferPlayback = true
            let controller = RecordingEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker,
                confirmDiscard: { false })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                               outputDirectory: "/Exports")
            let play = try button("Play", in: controller.root)
            let loop = try checkbox("Loop silent recording preview", in: controller.root)
            loop.state = .on; _ = loop.sendAction(loop.action, to: loop.target)
            play.performClick(nil)
            worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 700,
                image: try solidImage(red: 20, green: 210, blue: 30)))
            try render(controller.root, name: "recording-editor-looping-\(appearance)")

            play.performClick(nil); worker.completePlayback(.success(.cancelled))
            try render(controller.root, name: "recording-editor-loop-paused-\(appearance)")

            controller.window.setContentSize(NSSize(width: 760, height: 540))
            play.performClick(nil)
            worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 1_100,
                image: try solidImage(red: 30, green: 40, blue: 220)))
            try render(controller.root, name: "recording-editor-looping-minimum-\(appearance)")
            play.performClick(nil); worker.completePlayback(.success(.cancelled))
            try render(controller.root, name: "recording-editor-loop-paused-minimum-\(appearance)")
            play.performClick(nil)
            worker.completePlayback(.failure(AppBridgeError.backend("decoder stopped")))
            try render(controller.root, name: "recording-editor-playback-error-minimum-\(appearance)")
        }
    }

    func testPreviewScaleRenderedFitActualPausedAndSourceStates() throws {
        _ = NSApplication.shared
        let crop = NativeRecordingCropRect(x: 140, y: 90, width: 500, height: 300)
        for appearance in ["light", "dark"] {
            let worker = FakeRecordingEditorWorker(presentation: try presentation(
                position: 433, sourceWidth: 1_200, sourceHeight: 800,
                previewWidth: 1_200, previewHeight: 800, crop: crop))
            worker.deferPlayback = true
            worker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 433,
                image: try fixtureImage(width: 1_200, height: 800)))
            let controller = RecordingEditorController(
                tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker,
                confirmDiscard: { false })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                               outputDirectory: "/Exports")
            for (suffix, size) in [("normal", NSSize(width: 960, height: 760)),
                                   ("minimum", NSSize(width: 760, height: 540))] {
                controller.window.setContentSize(size)
                try render(controller.root,
                           name: "recording-editor-preview-fit-\(appearance)-\(suffix)")
                try button("100%", in: controller.root).performClick(nil)
                try render(controller.root,
                           name: "recording-editor-preview-actual-\(appearance)-\(suffix)")
                try button("Fit", in: controller.root).performClick(nil)
            }

            controller.window.setContentSize(NSSize(width: 760, height: 540))
            try button("100%", in: controller.root).performClick(nil)
            let play = try button("Play", in: controller.root)
            play.performClick(nil)
            worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 733,
                image: try fixtureImage(width: 640, height: 360)))
            play.performClick(nil); worker.completePlayback(.success(.cancelled))
            try render(controller.root,
                       name: "recording-editor-preview-actual-paused-\(appearance)-minimum")

            try button("Adjust crop", in: controller.root).performClick(nil)
            try render(controller.root,
                       name: "recording-editor-preview-actual-source-\(appearance)-minimum")
        }
    }

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
        let loop = try checkbox("Loop silent recording preview", in: controller.root)
        XCTAssertEqual(timeline.thumbnailStateDescription, "Source thumbnails unavailable")
        XCTAssertFalse(retry.isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(loop.isEnabled)
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled,
                      "thumbnail failure leaves the rest of editing available")
        XCTAssertFalse(controller.dirty)

        worker.deferThumbnails = true
        retry.performClick(nil)
        XCTAssertEqual(timeline.thumbnailStateDescription, "Loading source thumbnails…")
        XCTAssertFalse(loop.isEnabled,
                       "Loop cannot change while another operation owns the serialized worker")
        XCTAssertFalse(controller.windowShouldClose(controller.window),
                       "accepted thumbnail generation uses the existing busy close gate")
        let cancel = try button("Cancel operation", in: controller.root)
        cancel.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(worker.observedThumbnailCancel).isCancelled)
        XCTAssertEqual(timeline.thumbnailStateDescription, "Cancelling source thumbnails…")
        worker.completeThumbnails(.failure(AppBridgeError.backend("operation cancelled")))
        XCTAssertEqual(timeline.thumbnailStateDescription, "Source thumbnails cancelled")
        XCTAssertFalse(retry.isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(loop.isEnabled)
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

    func testGraphicalCropMapsFlippedLetterboxCoordinatesThroughSharedGeometry() throws {
        _ = NSApplication.shared
        let overlay = RecordingCropOverlay(tokens: Tokens.variants["light-mustard"]!)
        overlay.frame = NSRect(x: 0, y: 0, width: 424, height: 224)
        overlay.sourceSize = NativeRecordingDimensions(width: 400, height: 200)
        overlay.crop = NativeRecordingCropRect(x: 40, y: 20, width: 200, height: 100)
        overlay.isHidden = false; overlay.lockAspect = false; overlay.setEditingEnabled(true)
        XCTAssertTrue(overlay.isFlipped)
        XCTAssertEqual(overlay.fittedImageRect, NSRect(x: 12, y: 12, width: 400, height: 200))
        XCTAssertEqual(overlay.displayedCropRect, NSRect(x: 52, y: 32, width: 200, height: 100))

        var staged: [NativeRecordingCropRect] = []
        overlay.onStage = { staged.append($0) }
        let southEast = NSPoint(x: overlay.displayedCropRect.maxX,
                                y: overlay.displayedCropRect.maxY)
        overlay.beginDrag(.southEast, at: southEast)
        overlay.continueDrag(at: NSPoint(x: southEast.x + 40, y: southEast.y + 20))
        overlay.endDrag()
        XCTAssertEqual(staged.last,
                       NativeRecordingCropRect(x: 40, y: 20, width: 240, height: 120))

        let movedFrom = NSPoint(x: overlay.displayedCropRect.midX,
                                y: overlay.displayedCropRect.midY)
        overlay.beginDrag(.move, at: movedFrom)
        overlay.continueDrag(at: NSPoint(x: movedFrom.x - 20, y: movedFrom.y + 20))
        overlay.endDrag()
        XCTAssertEqual(staged.last,
                       NativeRecordingCropRect(x: 20, y: 40, width: 240, height: 120),
                       "positive flipped-view Y maps to positive top-down source Y")

        overlay.lockAspect = true
        overlay.nudge(.northWest, deltaX: -10, deltaY: -10)
        XCTAssertEqual(staged.last,
                       NativeRecordingCropRect(x: 0, y: 30, width: 260, height: 130))
    }

    func testGraphicalCropSourceModeCachesStagesAndRestoresPriorDisplay() throws {
        _ = NSApplication.shared
        let initialCrop = NativeRecordingCropRect(x: 40, y: 20, width: 160, height: 90)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            position: 400, crop: initialCrop))
        let source = try solidImage(width: 320, height: 180, red: 18, green: 90, blue: 170)
        worker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 400,
                                                             image: source))
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let image = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSImageView }.first)
        let acceptedImage = image.image
        worker.estimateResult = .success(RecordingEditorEstimate(sizeBytes: 1_234_567,
                                                                 exact: false))
        try button("Estimate size", in: controller.root).performClick(nil)
        let acceptedEstimate = try XCTUnwrap(labels(in: controller.root).first {
            $0.contains("≈") && $0.contains("MB")
        })
        var adjust = try button("Adjust crop", in: controller.root)
        let play = try button("Play", in: controller.root)
        let overlay = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingCropOverlay }.first)

        adjust.performClick(nil)
        XCTAssertEqual(worker.sourceCalls, 1)
        XCTAssertEqual(adjust.title, "Done cropping")
        XCTAssertFalse(overlay.isHidden); XCTAssertTrue(overlay.editingEnabled)
        XCTAssertFalse(play.isEnabled)
        XCTAssertEqual(image.image?.size, NSSize(width: 320, height: 180))
        XCTAssertFalse(controller.dirty, "source display mode is transient host state")
        XCTAssertTrue(labels(in: controller.root).contains(acceptedEstimate),
                      "source display mode retains the accepted estimate")
        XCTAssertTrue(worker.requests.isEmpty)

        overlay.lockAspect = false
        overlay.nudge(.east, deltaX: 11, deltaY: 0)
        XCTAssertTrue(controller.dirty)
        XCTAssertEqual(try field("Recording crop width", in: controller.root).stringValue, "171")
        XCTAssertEqual(try field("Recording output width", in: controller.root).stringValue, "171",
                       "Original reflects the staged crop without explicit output rounding")
        let outputMode = try popup("Recording output size", in: controller.root)
        outputMode.selectItem(withTitle: "Custom")
        _ = outputMode.sendAction(outputMode.action, to: outputMode.target)
        let outputWidth = try field("Recording output width", in: controller.root)
        let outputHeight = try field("Recording output height", in: controller.root)
        outputWidth.stringValue = "122"; outputHeight.stringValue = "78"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: outputWidth))
        overlay.nudge(.east, deltaX: 5, deltaY: 0)
        XCTAssertEqual(outputWidth.stringValue, "122")
        XCTAssertEqual(outputHeight.stringValue, "78",
                       "graphical crop preserves independent Custom output values")

        adjust = try button("Done cropping", in: controller.root)
        adjust.performClick(nil)
        XCTAssertTrue(overlay.isHidden)
        XCTAssertTrue(image.image === acceptedImage,
                      "Done restores the exact accepted display object without publishing")
        XCTAssertEqual(worker.requests.count, 0)

        try button("Adjust crop", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.sourceCalls, 1, "same-item accepted-position source frame is cached")
    }

    func testGraphicalCropPendingInputApplyFailureSuccessAndMotionRestore() throws {
        _ = NSApplication.shared
        let initialCrop = NativeRecordingCropRect(x: 40, y: 20, width: 160, height: 90)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            position: 400, crop: initialCrop))
        worker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 400,
            image: try solidImage(width: 320, height: 180, red: 180, green: 70, blue: 25)))
        worker.deferPlayback = true
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let width = try field("Recording crop width", in: controller.root)
        let adjust = try button("Adjust crop", in: controller.root)
        width.stringValue = "-"
        controller.controlTextDidChange(Notification(name: NSText.didChangeNotification,
                                                     object: width))
        XCTAssertFalse(adjust.isEnabled)
        adjust.performClick(nil)
        XCTAssertEqual(worker.sourceCalls, 0)
        XCTAssertEqual(width.stringValue, "-", "graphical input cannot discard partial text")
        width.stringValue = "160"; _ = width.sendAction(width.action, to: width.target)

        let play = try button("Play", in: controller.root)
        let image = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSImageView }.first)
        play.performClick(nil)
        worker.sendPlaybackFrame(RecordingPlaybackImage(positionMilliseconds: 733,
            image: try solidImage(red: 10, green: 200, blue: 30)))
        let motionImage = image.image
        play.performClick(nil); worker.completePlayback(.success(.cancelled))
        adjust.performClick(nil)
        XCTAssertTrue(labels(in: controller.root).contains {
            $0 == "Full source · 0:00.400"
        }, "crop mode labels the accepted source time, not the paused motion time")
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("0:00.733 / 0:02.000") },
                      "crop mode does not change the transient playback resume position")
        try button("Done cropping", in: controller.root).performClick(nil)
        XCTAssertTrue(image.image === motionImage,
                      "Done restores the exact previously presented motion frame")

        adjust.performClick(nil)
        let overlay = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingCropOverlay }.first)
        let lock = try checkbox("Lock recording crop aspect ratio", in: controller.root)
        lock.state = .off; _ = lock.sendAction(lock.action, to: lock.target)
        overlay.nudge(.east, deltaX: 20, deltaY: 0)
        let sourceImage = image.image
        worker.requestResult = .failure(AppBridgeError.backend("crop preview unavailable"))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertFalse(overlay.isHidden)
        XCTAssertTrue(image.image === sourceImage,
                      "failed Apply preserves the full-source display and pending crop")
        XCTAssertEqual(width.stringValue, "180")

        worker.requestResult = .success(try presentation(position: 400, revision: 1,
            crop: NativeRecordingCropRect(x: 40, y: 20, width: 180, height: 90)))
        try button("Apply edits", in: controller.root).performClick(nil)
        XCTAssertTrue(overlay.isHidden, "successful Apply leaves source adjustment mode")
        XCTAssertFalse(image.image === sourceImage)
        XCTAssertEqual(width.stringValue, "180")
    }

    func testGraphicalCropLoadCancelErrorRetryAndSeekInvalidation() throws {
        _ = NSApplication.shared
        let crop = NativeRecordingCropRect(x: 20, y: 10, width: 200, height: 120)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(
            position: 400, crop: crop))
        worker.deferSource = true
        let controller = RecordingEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        let preview = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSImageView }.first)
        let acceptedImage = preview.image
        let acceptedWidth = try field("Recording crop width", in: controller.root).stringValue
        try button("Adjust crop", in: controller.root).performClick(nil)
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        XCTAssertFalse(controller.prepareForTermination())
        let cancel = try button("Cancel operation", in: controller.root)
        cancel.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(worker.observedSourceCancel).isCancelled)
        worker.completeSource(.success(RecordingSourceImage(positionMilliseconds: 400,
            image: try solidImage(width: 320, height: 180, red: 220, green: 30, blue: 20))))
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("cancelled") })
        XCTAssertTrue(try button("Adjust crop", in: controller.root).isEnabled)
        XCTAssertTrue(preview.image === acceptedImage,
                      "a late success after cancellation cannot replace the accepted image")
        XCTAssertEqual(try field("Recording crop width", in: controller.root).stringValue,
                       acceptedWidth)
        XCTAssertTrue(descendants(in: controller.root)
            .compactMap { $0 as? RecordingCropOverlay }.first?.isHidden == true)

        try button("Adjust crop", in: controller.root).performClick(nil)
        worker.completeSource(.failure(AppBridgeError.backend("decoder unavailable")))
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("decoder unavailable") })
        XCTAssertTrue(try slider("Recording frame position", in: controller.root).isEnabled)

        try button("Adjust crop", in: controller.root).performClick(nil)
        worker.completeSource(.success(RecordingSourceImage(positionMilliseconds: 400,
            image: try solidImage(width: 320, height: 180, red: 40, green: 80, blue: 160))))
        try button("Done cropping", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.sourceCalls, 3)

        let seek = try slider("Recording frame position", in: controller.root)
        worker.requestResult = .success(try presentation(position: 913, revision: 1, crop: crop))
        seek.doubleValue = 913; _ = seek.sendAction(seek.action, to: seek.target)
        worker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 913,
            image: try solidImage(width: 320, height: 180, red: 70, green: 120, blue: 30)))
        worker.deferSource = false
        try button("Adjust crop", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.sourceCalls, 4,
                       "accepted seek position changes invalidate the full-source cache")

        try button("Done cropping", in: controller.root).performClick(nil)
        controller.present(artifact: recordingArtifact(id: "next-recording"),
                           historyRoot: "/History", outputDirectory: "/Exports")
        XCTAssertEqual(try button("Adjust crop", in: controller.root).title, "Adjust crop")
        worker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 400,
            image: try solidImage(width: 320, height: 180, red: 120, green: 30, blue: 80)))
        try button("Adjust crop", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.sourceCalls, 5, "a new History item cannot reuse the prior source cache")
    }

    func testGraphicalCropWindowDispatchHitsEveryHandleAtSupportedSizes() throws {
        _ = NSApplication.shared
        let crop = NativeRecordingCropRect(x: 0, y: 0, width: 320, height: 180)
        let worker = FakeRecordingEditorWorker(presentation: try presentation(position: 400,
                                                                              crop: crop))
        worker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 400,
            image: try solidImage(width: 320, height: 180, red: 80, green: 100, blue: 150)))
        let controller = RecordingEditorController(tokens: Tokens.variants["dark-mustard"]!,
                                                   worker: worker, confirmDiscard: { false })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
        try button("Adjust crop", in: controller.root).performClick(nil)
        let overlay = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? RecordingCropOverlay }.first)
        let handles = descendants(in: overlay).compactMap { $0 as? RecordingCropHandle }
        XCTAssertEqual(handles.count, 8)

        for size in [NSSize(width: 960, height: 760), NSSize(width: 760, height: 540)] {
            controller.window.setContentSize(size)
            for handle in handles {
                XCTAssertTrue(try windowHit(handle, in: controller) === handle,
                              "root dispatch reaches \(handle.kind) at \(size.width)px")
            }
        }
        let widthField = try field("Recording crop width", in: controller.root)
        widthField.selectText(nil)
        let editor = try XCTUnwrap(controller.window.fieldEditor(false, for: widthField)
            as? NSTextView)
        editor.insertText("300", replacementRange: NSRange(
            location: 0, length: editor.string.utf16.count))
        XCTAssertTrue(overlay.interceptsPendingInput)
        let pendingPoint = NSPoint(x: overlay.displayedCropRect.midX,
                                   y: overlay.displayedCropRect.midY)
        try dispatchCropOverlayMouse(.leftMouseDown, at: pendingPoint, to: overlay,
                                     in: controller)
        try dispatchCropOverlayMouse(.leftMouseDragged, at: pendingPoint, to: overlay,
                                     in: controller, deltaX: 12, deltaY: 6)
        try dispatchCropOverlayMouse(.leftMouseUp, at: pendingPoint, to: overlay,
                                     in: controller, deltaX: 12, deltaY: 6)
        XCTAssertEqual(widthField.stringValue, "300")
        XCTAssertEqual(try field("Recording crop X", in: controller.root).stringValue, "0",
                       "the first overlay click commits text without beginning a drag")
        XCTAssertTrue(overlay.editingEnabled)
        let movePoint = NSPoint(x: overlay.displayedCropRect.midX,
                                y: overlay.displayedCropRect.midY)
        try dispatchCropOverlayMouse(.leftMouseDown, at: movePoint, to: overlay, in: controller)
        try dispatchCropOverlayMouse(.leftMouseDragged, at: movePoint, to: overlay,
                                     in: controller, deltaX: 6, deltaY: 4)
        try dispatchCropOverlayMouse(.leftMouseUp, at: movePoint, to: overlay,
                                     in: controller, deltaX: 6, deltaY: 4)
        let movedX = try XCTUnwrap(UInt32(try field("Recording crop X",
                                                   in: controller.root).stringValue))
        XCTAssertNotEqual(movedX, 0,
                          "the next overlay gesture moves the committed crop")
        XCTAssertTrue(controller.window.firstResponder === overlay)
        try dispatchCropKey(124, to: overlay, in: controller)
        XCTAssertEqual(try XCTUnwrap(UInt32(try field("Recording crop X",
                                                     in: controller.root).stringValue)), movedX + 1)
        try dispatchCropKey(123, to: overlay, in: controller, modifiers: [.shift])
        let clampedX = UInt32(max(0, Int(movedX) - 9))
        XCTAssertEqual(try XCTUnwrap(UInt32(try field("Recording crop X",
                                                     in: controller.root).stringValue)),
                       clampedX,
                       "focused interior movement clamps a ten-pixel Shift nudge")
        try dispatchCropKey(124, to: overlay, in: controller, modifiers: [.shift])
        XCTAssertEqual(try XCTUnwrap(UInt32(try field("Recording crop X",
                                                     in: controller.root).stringValue)), clampedX + 10,
                       "the opposite in-bounds Shift nudge moves ten source pixels")

        let southEast = try XCTUnwrap(handles.first { $0.kind == .southEast })
        let before = try field("Recording crop width", in: controller.root).stringValue
        try dispatchCropMouse(.leftMouseDown, to: southEast, in: controller)
        try dispatchCropMouse(.leftMouseDragged, to: southEast, in: controller,
                              deltaX: -12, deltaY: -6)
        try dispatchCropMouse(.leftMouseUp, to: southEast, in: controller,
                              deltaX: -12, deltaY: -6)
        XCTAssertNotEqual(try field("Recording crop width", in: controller.root).stringValue, before)
        XCTAssertTrue(worker.requests.isEmpty, "crop pointer dispatch only stages numeric geometry")
        let releasedWidth = try field("Recording crop width", in: controller.root).stringValue
        try dispatchCropMouse(.leftMouseDragged, to: southEast, in: controller,
                              deltaX: -40, deltaY: -20)
        XCTAssertEqual(try field("Recording crop width", in: controller.root).stringValue,
                       releasedWidth, "pointer release ends the crop gesture")

        // The locked southeast resize above leaves the crop against the source's top edge.
        // Move it down before expanding the east edge so the coupled height has room to grow.
        XCTAssertTrue(controller.window.makeFirstResponder(overlay))
        try dispatchCropKey(125, to: overlay, in: controller, modifiers: [.shift])
        let east = try XCTUnwrap(handles.first { $0.kind == .east })
        XCTAssertTrue(controller.window.makeFirstResponder(east))
        let keyboardWidth = try XCTUnwrap(UInt32(try field("Recording crop width",
                                                          in: controller.root).stringValue))
        try dispatchCropKey(124, to: east, in: controller)
        XCTAssertEqual(try XCTUnwrap(UInt32(try field("Recording crop width",
                                                     in: controller.root).stringValue)),
                       keyboardWidth + 1)
        try dispatchCropKey(124, to: east, in: controller, modifiers: [.shift])
        XCTAssertEqual(try XCTUnwrap(UInt32(try field("Recording crop width",
                                                     in: controller.root).stringValue)),
                       keyboardWidth + 11,
                       "focused handles use one source pixel, or ten with Shift")

        let staged = try field("Recording crop width", in: controller.root).stringValue
        overlay.beginDrag(.east, at: NSPoint(x: overlay.displayedCropRect.maxX,
                                             y: overlay.displayedCropRect.midY))
        controller.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification,
                                                   object: controller.window))
        overlay.continueDrag(at: NSPoint(x: overlay.displayedCropRect.maxX + 30,
                                         y: overlay.displayedCropRect.midY))
        XCTAssertEqual(try field("Recording crop width", in: controller.root).stringValue, staged,
                       "focus loss ends the gesture while retaining its last staged value")
        overlay.beginDrag(.east, at: NSPoint(x: overlay.displayedCropRect.maxX,
                                             y: overlay.displayedCropRect.midY))
        try dispatchCropKey(53, to: east, in: controller)
        overlay.continueDrag(at: NSPoint(x: overlay.displayedCropRect.maxX + 30,
                                         y: overlay.displayedCropRect.midY))
        XCTAssertEqual(try field("Recording crop width", in: controller.root).stringValue, staged,
                       "Escape ends the gesture without rolling back the staged value")
        overlay.beginDrag(.east, at: NSPoint(x: overlay.displayedCropRect.maxX,
                                             y: overlay.displayedCropRect.midY))
        controller.window.setContentSize(NSSize(width: 960, height: 760))
        overlay.continueDrag(at: NSPoint(x: overlay.displayedCropRect.maxX + 30,
                                         y: overlay.displayedCropRect.midY))
        XCTAssertEqual(try field("Recording crop width", in: controller.root).stringValue, staged,
                       "layout changes end the gesture while retaining staged geometry")
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
        let explanation = try XCTUnwrap(descendants(in: controller.root)
            .compactMap { $0 as? NSTextField }
            .first { !$0.isEditable && $0.stringValue.hasPrefix("Apply before") })

        for size in [NSSize(width: 960, height: 760), NSSize(width: 760, height: 540)] {
            controller.window.setContentSize(size)
            XCTAssertLessThanOrEqual(explanation.intrinsicContentSize.width, explanation.frame.width,
                                     "Retry must not clip the adjacent edit-gating explanation")
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
        let play = try button("Play", in: controller.root)

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
        XCTAssertFalse(play.isEnabled)
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
        XCTAssertTrue(play.isEnabled)

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
        XCTAssertFalse(play.isEnabled)

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

    func testGraphicalCropRenderedSourceLoadingErrorAndAcceptedStates() throws {
        _ = NSApplication.shared
        let crop = NativeRecordingCropRect(x: 40, y: 20, width: 160, height: 90)
        func renderSizes(_ controller: RecordingEditorController, _ name: String) throws {
            for (suffix, size) in [("normal", NSSize(width: 960, height: 760)),
                                   ("minimum", NSSize(width: 760, height: 540))] {
                controller.window.setContentSize(size)
                try render(controller.root, name: "recording-editor-crop-\(name)-\(suffix)")
            }
        }
        for appearance in ["light", "dark"] {
            let activeWorker = FakeRecordingEditorWorker(presentation: try presentation(
                position: 400, crop: crop))
            activeWorker.sourceResult = .success(RecordingSourceImage(positionMilliseconds: 400,
                image: try solidImage(width: 320, height: 180,
                                      red: 48, green: 92, blue: 164)))
            let active = RecordingEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                                                   worker: activeWorker, confirmDiscard: { false })
            defer { active.window.orderOut(nil) }
            active.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
            try button("Adjust crop", in: active.root).performClick(nil)
            try renderSizes(active, "source-\(appearance)")

            let loadingWorker = FakeRecordingEditorWorker(presentation: try presentation(
                position: 400, crop: crop)); loadingWorker.deferSource = true
            let loading = RecordingEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                worker: loadingWorker, confirmDiscard: { false })
            defer { loading.window.orderOut(nil) }
            loading.present(artifact: recordingArtifact(), historyRoot: "/History",
                            outputDirectory: "/Exports")
            try button("Adjust crop", in: loading.root).performClick(nil)
            try renderSizes(loading, "loading-\(appearance)")

            let errorWorker = FakeRecordingEditorWorker(presentation: try presentation(
                position: 400, crop: crop))
            errorWorker.sourceResult = .failure(AppBridgeError.backend("Full-source frame unavailable"))
            let failed = RecordingEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                worker: errorWorker, confirmDiscard: { false })
            defer { failed.window.orderOut(nil) }
            failed.present(artifact: recordingArtifact(), historyRoot: "/History",
                           outputDirectory: "/Exports")
            try button("Adjust crop", in: failed.root).performClick(nil)
            try renderSizes(failed, "error-\(appearance)")

            let overlay = try XCTUnwrap(descendants(in: active.root)
                .compactMap { $0 as? RecordingCropOverlay }.first)
            let lock = try checkbox("Lock recording crop aspect ratio", in: active.root)
            lock.state = .off; _ = lock.sendAction(lock.action, to: lock.target)
            overlay.nudge(.east, deltaX: 20, deltaY: 0)
            activeWorker.requestResult = .success(try presentation(position: 400, revision: 1,
                crop: NativeRecordingCropRect(x: 40, y: 20, width: 180, height: 90)))
            try button("Apply edits", in: active.root).performClick(nil)
            try renderSizes(active, "accepted-\(appearance)")
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

    func testRealBridgeSourceFrameIsFullSourceAtAcceptedPositionAndImmutable() throws {
        let tools = try NativeMediaTools.locate()
        let fixture = try makeCropRecordingFixture(tools: tools)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let sourceBefore = try Data(contentsOf: fixture.source)
        var retainedImage: CGImage?
        do {
            let opened = try NativeRecordingEditorSession.open(historyRoot: fixture.history.path,
                                                                artifactID: fixture.id, tools: tools)
            let session = opened.0
            let sought = try session.request(["operation": "seek", "position_ms": 733])
            var edit = sought.snapshot.edit
            edit["trim_start_ms"] = 211; edit["trim_end_ms"] = 1_289
            edit["crop"] = ["x": 60, "y": 24, "width": 80, "height": 48]
            edit["output_width"] = 40; edit["output_height"] = 24
            var export = sought.snapshot.export
            export["format"] = "gif"; export["quality"] = "standard"
            let accepted = try session.request(["operation": "update_preview", "edit": edit,
                                                "export": export])
            XCTAssertEqual(accepted.image.width, 40); XCTAssertEqual(accepted.image.height, 24)
            let before = try session.request(["operation": "snapshot"]).snapshot
            let source = try session.sourceFrame(cancel: try XCTUnwrap(NativeRecordingEditorCancel()))
            XCTAssertEqual(source.positionMilliseconds, 733)
            XCTAssertEqual(source.image.width, 160); XCTAssertEqual(source.image.height, 96)
            XCTAssertEqual(try pixels(source.image).count, 160 * 96 * 4)
            retainedImage = source.image

            let cancelled = try XCTUnwrap(NativeRecordingEditorCancel()); cancelled.cancel()
            XCTAssertThrowsError(try session.sourceFrame(cancel: cancelled))
            let after = try session.request(["operation": "snapshot"]).snapshot
            XCTAssertEqual(after.revision, before.revision)
            XCTAssertEqual(after.positionMilliseconds, before.positionMilliseconds)
            XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.edit, options: [.sortedKeys]),
                           try JSONSerialization.data(withJSONObject: before.edit, options: [.sortedKeys]))
            XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.export, options: [.sortedKeys]),
                           try JSONSerialization.data(withJSONObject: before.export, options: [.sortedKeys]))
        }
        let retained = try XCTUnwrap(retainedImage)
        XCTAssertEqual(try pixels(retained).count, 160 * 96 * 4,
                       "source-frame pixels outlive their frame owner and editor session")
        XCTAssertEqual(try Data(contentsOf: fixture.source), sourceBefore,
                       "source preview leaves the recording byte-identical")
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

    func testRealBridgeSilentPlaybackMotionCancellationAndImmutableAcceptedState() throws {
        let tools = try NativeMediaTools.locate()
        let fixture = try makeRecordingFixture(tools: tools)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let sourceBefore = try Data(contentsOf: fixture.source)
        var retainedImage: CGImage?
        do {
            let opened = try NativeRecordingEditorSession.open(historyRoot: fixture.history.path,
                                                                artifactID: fixture.id, tools: tools)
            let session = opened.0
            var edit = opened.1.snapshot.edit
            edit["trim_start_ms"] = 200; edit["trim_end_ms"] = 800
            edit["crop"] = ["x": 20, "y": 10, "width": 120, "height": 60]
            edit["output_width"] = 80; edit["output_height"] = 40
            var export = opened.1.snapshot.export
            export["format"] = "gif"
            let accepted = try session.request(["operation": "update_preview", "edit": edit,
                                                "export": export])
            let before = try session.request(["operation": "snapshot"]).snapshot
            let cancel = try XCTUnwrap(NativeRecordingEditorCancel())
            let playback = try session.playback(positionMilliseconds: 800, cancel: cancel)
            XCTAssertEqual(playback.metadata.startPositionMilliseconds, 200,
                           "trim end normalizes to accepted trim start")
            XCTAssertEqual(playback.metadata.width, 80); XCTAssertEqual(playback.metadata.height, 40)
            XCTAssertLessThanOrEqual(playback.metadata.framesPerSecond, 30)
            let first = try XCTUnwrap(playback.nextFrame())
            var later = try XCTUnwrap(playback.nextFrame())
            while later.positionMilliseconds == first.positionMilliseconds {
                later = try XCTUnwrap(playback.nextFrame())
            }
            XCTAssertNotEqual(try pixels(first.image), try pixels(later.image),
                              "persistent playback presents temporal motion frames")
            retainedImage = later.image
            while try playback.nextFrame() != nil {}
            XCTAssertNil(try playback.nextFrame(), "EOF remains deterministic")
            XCTAssertFalse(cancel.isCancelled, "natural EOF does not cancel the caller token")

            let after = try session.request(["operation": "snapshot"]).snapshot
            XCTAssertEqual(after.revision, before.revision)
            XCTAssertEqual(after.positionMilliseconds, before.positionMilliseconds)
            XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.edit, options: [.sortedKeys]),
                           try JSONSerialization.data(withJSONObject: before.edit, options: [.sortedKeys]))
            XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.export, options: [.sortedKeys]),
                           try JSONSerialization.data(withJSONObject: before.export, options: [.sortedKeys]))
            XCTAssertEqual(accepted.snapshot.revision, before.revision)

            let cancelled = try XCTUnwrap(NativeRecordingEditorCancel())
            let cancelledPlayback = try session.playback(positionMilliseconds: 200,
                                                          cancel: cancelled)
            cancelled.cancel()
            XCTAssertThrowsError(try cancelledPlayback.nextFrame())
        }
        XCTAssertEqual(try XCTUnwrap(retainedImage).width, 80)
        XCTAssertEqual(try pixels(try XCTUnwrap(retainedImage)).count, 80 * 40 * 4,
                       "retained playback frame remains readable after stream/session release")
        XCTAssertEqual(try Data(contentsOf: fixture.source), sourceBefore,
                       "playback keeps the original recording byte-identical")
    }

    func testRealWorkerLoopsAsymmetricTrimThenDisablesAndCancelsWithoutMutation() throws {
        let tools = try NativeMediaTools.locate()
        let fixture = try makeRecordingFixture(tools: tools)
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let sourceBefore = try Data(contentsOf: fixture.source)
        let worker = RecordingEditorWorker()
        defer { worker.close(); RecordingEditorWorker.flush() }

        let openedExpectation = expectation(description: "open recording worker")
        var openedResult: Result<RecordingEditorPresentation, Error>?
        worker.open(historyRoot: fixture.history.path, artifactID: fixture.id) {
            openedResult = $0; openedExpectation.fulfill()
        }
        wait(for: [openedExpectation], timeout: 10)
        let opened = try XCTUnwrap(openedResult).get()
        var edit = opened.snapshot.edit
        edit["trim_start_ms"] = 233; edit["trim_end_ms"] = 977
        let acceptedExpectation = expectation(description: "accept asymmetric trim")
        var acceptedResult: Result<RecordingEditorPresentation, Error>?
        worker.request(["operation": "update_preview", "edit": edit,
                        "export": opened.snapshot.export]) {
            acceptedResult = $0; acceptedExpectation.fulfill()
        }
        wait(for: [acceptedExpectation], timeout: 10)
        let accepted = try XCTUnwrap(acceptedResult).get()

        let finishLoop = expectation(description: "loop finishes after disable")
        let loop = RecordingPlaybackLoopControl(enabled: true)
        let loopCancel = try XCTUnwrap(NativeRecordingEditorCancel())
        var firstPositions: [UInt64] = []
        var firstStartedCount = 0
        var firstResult: Result<RecordingPlaybackCompletion, Error>?
        worker.playback(positionMilliseconds: 701, loopStartMilliseconds: 233,
            loop: loop, cancel: loopCancel, started: { _ in firstStartedCount += 1 }, frame: { value in
                if let previous = firstPositions.last, value.positionMilliseconds < previous {
                    loop.isEnabled = false
                }
                firstPositions.append(value.positionMilliseconds)
            }, completion: {
                firstResult = $0; finishLoop.fulfill()
            })
        wait(for: [finishLoop], timeout: 10)
        XCTAssertEqual(try XCTUnwrap(firstResult).get(), .eof)
        XCTAssertEqual(firstStartedCount, 1,
                       "loop laps reuse one playback operation without queued metadata callbacks")
        XCTAssertTrue(firstPositions.allSatisfy { (233..<977).contains($0) },
                      "all presented positions remain inside the accepted half-open trim")
        XCTAssertTrue(zip(firstPositions, firstPositions.dropFirst()).contains { pair in
            pair.0 > pair.1
        },
                      "a nonempty EOF reopens at the asymmetric source-relative trim start")
        XCTAssertFalse(loopCancel.isCancelled)

        let cancelLoop = expectation(description: "loop cancellation finishes")
        let secondLoop = RecordingPlaybackLoopControl(enabled: true)
        let secondCancel = try XCTUnwrap(NativeRecordingEditorCancel())
        var secondPositions: [UInt64] = []
        var secondResult: Result<RecordingPlaybackCompletion, Error>?
        worker.playback(positionMilliseconds: 701, loopStartMilliseconds: 233,
            loop: secondLoop, cancel: secondCancel, started: { _ in }, frame: { value in
                if let previous = secondPositions.last, value.positionMilliseconds < previous {
                    secondCancel.cancel()
                }
                secondPositions.append(value.positionMilliseconds)
            }, completion: {
                secondResult = $0; cancelLoop.fulfill()
            })
        wait(for: [cancelLoop], timeout: 10)
        XCTAssertEqual(try XCTUnwrap(secondResult).get(), .cancelled)
        XCTAssertTrue(secondPositions.allSatisfy { (233..<977).contains($0) })
        XCTAssertTrue(zip(secondPositions, secondPositions.dropFirst()).contains { pair in
            pair.0 > pair.1
        })

        let snapshotExpectation = expectation(description: "read immutable snapshot")
        var snapshotResult: Result<RecordingEditorPresentation, Error>?
        worker.request(["operation": "snapshot"]) {
            snapshotResult = $0; snapshotExpectation.fulfill()
        }
        wait(for: [snapshotExpectation], timeout: 10)
        let after = try XCTUnwrap(snapshotResult).get().snapshot
        XCTAssertEqual(after.revision, accepted.snapshot.revision)
        XCTAssertEqual(after.positionMilliseconds, accepted.snapshot.positionMilliseconds)
        XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.edit, options: [.sortedKeys]),
                       try JSONSerialization.data(withJSONObject: accepted.snapshot.edit,
                                                  options: [.sortedKeys]))
        XCTAssertEqual(try JSONSerialization.data(withJSONObject: after.export, options: [.sortedKeys]),
                       try JSONSerialization.data(withJSONObject: accepted.snapshot.export,
                                                  options: [.sortedKeys]))
        XCTAssertEqual(try Data(contentsOf: fixture.source), sourceBefore,
                       "looped playback keeps the source byte-identical")
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
                              previewWidth: Int = 16, previewHeight: Int = 9,
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
        return RecordingEditorPresentation(snapshot: snapshot,
                                           image: try fixtureImage(width: previewWidth,
                                                                   height: previewHeight))
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

    private func fixtureImage(width: Int = 16, height: Int = 9) throws -> CGImage {
        let bytes = [UInt8](repeating: 80, count: width * height * 4)
        let provider = try XCTUnwrap(CGDataProvider(data: Data(bytes) as CFData))
        return try XCTUnwrap(CGImage(width: width, height: height,
                       bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: false,
                       intent: .defaultIntent))
    }

    private func solidImage(red: UInt8, green: UInt8, blue: UInt8) throws -> CGImage {
        let bytes = [red, green, blue, 255, red, green, blue, 255]
        let provider = try XCTUnwrap(CGDataProvider(data: Data(bytes) as CFData))
        return try XCTUnwrap(CGImage(width: 2, height: 1, bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: 8, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false,
            intent: .defaultIntent))
    }

    private func solidImage(width: Int, height: Int, red: UInt8, green: UInt8,
                            blue: UInt8) throws -> CGImage {
        let pixel = [red, green, blue, 255]
        let provider = try XCTUnwrap(CGDataProvider(
            data: Data(Array(repeating: pixel, count: width * height).flatMap { $0 }) as CFData))
        return try XCTUnwrap(CGImage(width: width, height: height, bitsPerComponent: 8,
            bitsPerPixel: 32, bytesPerRow: width * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false,
            intent: .defaultIntent))
    }

    private func pixels(_ image: CGImage) throws -> Data {
        try XCTUnwrap(image.dataProvider?.data) as Data
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
    private func windowHit(_ handle: RecordingCropHandle,
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
    private func dispatchCropMouse(_ type: NSEvent.EventType, to handle: RecordingCropHandle,
                                   in controller: RecordingEditorController,
                                   deltaX: CGFloat = 0, deltaY: CGFloat = 0) throws {
        var point = handle.convert(NSPoint(x: handle.bounds.midX, y: handle.bounds.midY), to: nil)
        point.x += deltaX; point.y += deltaY
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point,
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: controller.window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .leftMouseDown:
            try XCTUnwrap(controller.window.contentView?.hitTest(point)).mouseDown(with: event)
        case .leftMouseDragged: handle.mouseDragged(with: event)
        case .leftMouseUp: handle.mouseUp(with: event)
        default: XCTFail("Unsupported pointer event")
        }
    }
    private func dispatchCropOverlayMouse(_ type: NSEvent.EventType, at localPoint: NSPoint,
                                          to overlay: RecordingCropOverlay,
                                          in controller: RecordingEditorController,
                                          deltaX: CGFloat = 0, deltaY: CGFloat = 0) throws {
        var point = overlay.convert(localPoint, to: nil)
        point.x += deltaX; point.y += deltaY
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point,
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: controller.window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .leftMouseDown:
            try XCTUnwrap(controller.window.contentView?.hitTest(point)).mouseDown(with: event)
        case .leftMouseDragged: overlay.mouseDragged(with: event)
        case .leftMouseUp: overlay.mouseUp(with: event)
        default: XCTFail("Unsupported pointer event")
        }
    }
    private func dispatchCropKey(_ keyCode: UInt16, to handle: RecordingCropHandle,
                                 in controller: RecordingEditorController,
                                 modifiers: NSEvent.ModifierFlags = []) throws {
        let event = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: modifiers, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: controller.window.windowNumber, context: nil,
            characters: "", charactersIgnoringModifiers: "", isARepeat: false,
            keyCode: keyCode))
        XCTAssertTrue(controller.window.firstResponder === handle)
        controller.window.sendEvent(event)
    }
    private func dispatchCropKey(_ keyCode: UInt16, to overlay: RecordingCropOverlay,
                                 in controller: RecordingEditorController,
                                 modifiers: NSEvent.ModifierFlags = []) throws {
        let event = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: modifiers, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: controller.window.windowNumber, context: nil,
            characters: "", charactersIgnoringModifiers: "", isARepeat: false,
            keyCode: keyCode))
        XCTAssertTrue(controller.window.firstResponder === overlay)
        controller.window.sendEvent(event)
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
    var playbackMetadata = RecordingPlaybackMetadata(startPositionMilliseconds: 0,
                                                      width: 2, height: 1,
                                                      framesPerSecond: 10)
    var playbackStarts: [UInt64] = []
    var playbackFrames: [RecordingPlaybackImage] = []
    var playbackResult: Result<RecordingPlaybackCompletion, Error> = .success(.eof)
    var deferPlayback = false
    var deferPlaybackStarted = false
    var sourceResult: Result<RecordingSourceImage, Error> =
        .failure(AppBridgeError.backend("source frame unavailable"))
    var sourceCalls = 0
    var deferSource = false
    var thumbnailResult: Result<CGImage, Error> = .success(fakeTimelineImage())
    var thumbnailCalls = 0
    var deferThumbnails = false
    var saveResult: Result<RecordingEditorSaveResult, Error> = .failure(AppBridgeError.backend("save unavailable"))
    var saves: [(destination: String, export: [String: Any])] = []
    var deferSave = false
    var closeCount = 0
    weak var observedSaveCancel: NativeRecordingEditorCancel?
    weak var observedThumbnailCancel: NativeRecordingEditorCancel?
    weak var observedSourceCancel: NativeRecordingEditorCancel?
    weak var observedPlaybackCancel: NativeRecordingEditorCancel?
    weak var observedPlaybackLoop: RecordingPlaybackLoopControl?
    private var pendingOpen: ((Result<RecordingEditorPresentation, Error>) -> Void)?
    private var pendingSave: ((Result<RecordingEditorSaveResult, Error>) -> Void)?
    private var pendingThumbnails: ((Result<CGImage, Error>) -> Void)?
    private var pendingSource: ((Result<RecordingSourceImage, Error>) -> Void)?
    private var pendingPlaybackStarted: ((RecordingPlaybackMetadata) -> Void)?
    private var pendingPlaybackFrame: ((RecordingPlaybackImage) -> Void)?
    private var pendingPlaybackCompletion: ((Result<RecordingPlaybackCompletion, Error>) -> Void)?
    private var pendingPlaybackLoopStart: UInt64?
    private var playbackLapFrameCount = 0

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
    func playback(positionMilliseconds: UInt64, loopStartMilliseconds: UInt64,
                  loop: RecordingPlaybackLoopControl, cancel: NativeRecordingEditorCancel,
                  started: @escaping (RecordingPlaybackMetadata) -> Void,
                  frame: @escaping (RecordingPlaybackImage) -> Void,
                  completion: @escaping (Result<RecordingPlaybackCompletion, Error>) -> Void) {
        playbackStarts.append(positionMilliseconds); observedPlaybackCancel = cancel
        observedPlaybackLoop = loop; pendingPlaybackLoopStart = loopStartMilliseconds
        playbackLapFrameCount = 0
        if deferPlaybackStarted { pendingPlaybackStarted = started }
        else { started(playbackMetadata) }
        if deferPlayback {
            pendingPlaybackFrame = frame; pendingPlaybackCompletion = completion
        } else {
            playbackFrames.forEach(frame); completion(playbackResult)
        }
    }
    func completePlaybackStarted() {
        let started = pendingPlaybackStarted; pendingPlaybackStarted = nil
        started?(playbackMetadata)
    }
    func sendPlaybackFrame(_ value: RecordingPlaybackImage) {
        playbackLapFrameCount += 1; pendingPlaybackFrame?(value)
    }
    func completePlayback(_ result: Result<RecordingPlaybackCompletion, Error>) {
        if case .success(.eof) = result,
           observedPlaybackCancel?.isCancelled == false,
           playbackLapFrameCount > 0,
           observedPlaybackLoop?.isEnabled == true,
           let loopStart = pendingPlaybackLoopStart {
            playbackStarts.append(loopStart); playbackLapFrameCount = 0
            if !deferPlaybackStarted { pendingPlaybackStarted?(playbackMetadata) }
            return
        }
        let completion = pendingPlaybackCompletion
        pendingPlaybackStarted = nil; pendingPlaybackFrame = nil; pendingPlaybackCompletion = nil
        pendingPlaybackLoopStart = nil; playbackLapFrameCount = 0
        completion?(observedPlaybackCancel?.isCancelled == true ? .success(.cancelled) : result)
    }
    func sourceFrame(cancel: NativeRecordingEditorCancel,
                     completion: @escaping (Result<RecordingSourceImage, Error>) -> Void) {
        sourceCalls += 1; observedSourceCancel = cancel
        if deferSource { pendingSource = completion }
        else { completion(sourceResult) }
    }
    func completeSource(_ result: Result<RecordingSourceImage, Error>) {
        let completion = pendingSource; pendingSource = nil; completion?(result)
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
