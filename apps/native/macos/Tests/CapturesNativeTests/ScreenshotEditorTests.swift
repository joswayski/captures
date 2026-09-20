import AppKit
import XCTest
@testable import CapturesNative

final class ScreenshotEditorTests: XCTestCase {
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
            let opacityLabel = try XCTUnwrap(layerPanel.subviews.compactMap { $0 as? NSTextField }
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

    private func snapshot(id: String, width: Double = 640, height: Double = 360,
                          unsaved: Bool = false, draft: Bool = false,
                          layers: [[String: Any]] = []) -> NativeEditorSnapshot {
        NativeEditorSnapshot([
            "artifact_id": id, "document": ["width": width, "height": height,
                                                  "elements": layers],
            "can_undo": unsaved, "can_redo": false,
            "unsaved_changes": unsaved, "has_draft": draft,
        ])!
    }

    private func layer(id: String, name: String, x: Double, y: Double, visible: Bool,
                       locked: Bool, opacity: Double) -> [String: Any] {
        ["kind": "image", "id": id, "name": name, "x": x, "y": y,
         "visible": visible, "locked": locked, "opacity": opacity]
    }

    private func artifact(id: String) -> CaptureArtifact {
        CaptureArtifact(["entry": [
            "id": id, "kind": "screenshot", "width": 640, "height": 360,
            "created_at": "2026-09-20T00:00:00Z",
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
        sections.selectedSegment = 2
        _ = sections.sendAction(sections.action, to: sections.target)
    }

    private func popup(_ label: String, in view: NSView) throws -> NSPopUpButton {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSPopUpButton }
            .first { $0.accessibilityLabel() == label })
    }

    private func segmented(_ label: String, in view: NSView) throws -> NSSegmentedControl {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSSegmentedControl }
            .first { $0.accessibilityLabel() == label })
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
    var openArtifactIDs: [String] = []
    var closeCount = 0
    var draftsRoot: String?
    var failOperation: String?
    var failLayerAction: String?
    var failureMessage = "fixture save failed"
    var deferRequests = false
    var deferEncodes = false
    var failEncode = false
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
