import AppKit
import XCTest
@testable import CapturesNative

final class ScreenshotEditorTests: XCTestCase {
    func testToolRailSelectionMenuFocusBusyGatesAndMinimumLayout() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                worker: worker, writeClipboard: { _ in true })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            controller.windowDidResize(Notification(name: NSWindow.didResizeNotification))
            let labels = ["Select & move (V)", "Crop (C)", "Text (T)", "Shapes", "Arrow (A)", "Freehand (P)", "Background removal (B)"]
            let rail = try labels.map { label in
                try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? CaptureButton }
                    .first { $0.accessibilityLabel() == label })
            }
            XCTAssertEqual(rail.map { $0.frame.minY }, rail.map { $0.frame.minY }.sorted())
            for button in rail {
                XCTAssertTrue(controller.root.bounds.contains(button.frame))
                XCTAssertFalse(button.isHidden)
                XCTAssertTrue(button.isEnabled)
                XCTAssertEqual(button.frame.size, NSSize(width: 38, height: 38))
                XCTAssertEqual(button.toolTip, button.accessibilityLabel())
            }
            let sections = try segmented("Editor section", in: controller.root)
            rail[2].performClick(nil)
            XCTAssertEqual(sections.selectedSegment, 2)
            XCTAssertEqual(controller.drawOverlay.shape, .text)
            XCTAssertTrue(controller.window.firstResponder === controller.drawOverlay)
            XCTAssertTrue(rail[2].selected)
            rail[1].performClick(nil)
            XCTAssertTrue(controller.cropOverlay.croppingEnabled)
            XCTAssertTrue(rail[1].selected)
            rail[0].performClick(nil)
            XCTAssertFalse(controller.cropOverlay.croppingEnabled)
            XCTAssertTrue(controller.selectionOverlay.selectionEnabled)
            XCTAssertTrue(rail[0].selected)
            XCTAssertEqual(rail.filter(\.selected).count, 1)
            let menu = try XCTUnwrap(rail[3].menu)
            XCTAssertEqual(menu.items.map(\.title), ["Rectangle (R)", "Ellipse (O)", "Line (L)", "Triangle", "Diamond (D)", "Star (S)"])
            for (index, shape) in [EditorDrawOverlay.Shape.rectangle, .ellipse, .line, .triangle, .diamond, .star].enumerated() {
                menu.performActionForItem(at: index)
                XCTAssertEqual(controller.drawOverlay.shape, shape)
                XCTAssertTrue(rail[3].selected)
                XCTAssertEqual(menu.items[index].state, .on)
            }
            for (index, shape) in [(4, EditorDrawOverlay.Shape.arrow), (5, .pen), (6, .wand)] {
                rail[index].performClick(nil)
                XCTAssertEqual(controller.drawOverlay.shape, shape)
                XCTAssertTrue(rail[index].selected)
            }
            try showOutput(in: controller.root)
            XCTAssertEqual(rail.filter(\.selected).count, 0)
            XCTAssertTrue(rail.allSatisfy { $0.isEnabled && !$0.isHidden })
            rail[4].performClick(nil)
            try render(controller.root, name: "screenshot-editor-tool-rail-minimum-\(appearance)")
            worker.deferEncodes = true
            try button("Copy image", in: controller.root).performClick(nil)
            XCTAssertTrue(rail.allSatisfy { !$0.isEnabled })
            rail[5].performClick(nil)
            XCTAssertEqual(controller.drawOverlay.shape, .arrow)
            worker.completePendingEncode()
            XCTAssertTrue(rail.allSatisfy(\.isEnabled))
            worker.failEncode = true
            try button("Copy image", in: controller.root).performClick(nil)
            XCTAssertEqual(worker.encodes.count, 2)
            XCTAssertTrue(rail.allSatisfy(\.isEnabled))
            try render(controller.root, name: "screenshot-editor-tool-rail-error-\(appearance)")
            XCTAssertTrue(worker.requests.isEmpty && worker.saves.isEmpty)
        }
    }

    func testToolKeysSelectExistingToolsPreserveRepeatsAndCancelUnfinishedGestures() throws {
        _ = NSApplication.shared
        let original = snapshot(id: "shot", unsaved: true, draft: true)
        let worker = FakeEditorWorker(snapshot: original)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        controller.window.makeFirstResponder(nil)
        let sections = try segmented("Editor section", in: controller.root)
        for (key, shape) in [("T", EditorDrawOverlay.Shape.text), ("r", .rectangle),
                             ("o", .ellipse), ("l", .line), ("d", .diamond), ("s", .star),
                             ("a", .arrow), ("p", .pen), ("b", .wand)] {
            controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: key))
            XCTAssertEqual(sections.selectedSegment, 2)
            XCTAssertEqual(controller.drawOverlay.shape, shape)
            XCTAssertTrue(controller.drawOverlay.drawingEnabled)
        }
        let tool = try popup("Drawing tool", in: controller.root)
        for mode in ["Erase", "Restore"] {
            tool.selectItem(withTitle: mode); _ = tool.sendAction(tool.action, to: tool.target)
            controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: "p"))
            controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: "b"))
            XCTAssertEqual(tool.titleOfSelectedItem, mode, "B recalls the previous background-removal mode")
        }
        controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: "r"))
        let image = controller.presentedImageRect
        controller.drawOverlay.begin(at: NSPoint(x: image.minX + 40, y: image.minY + 50))
        let start = controller.drawOverlay.startPoint
        XCTAssertNotNil(start)
        controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: "r"))
        XCTAssertEqual(controller.drawOverlay.startPoint, start, "same tool does not cancel a gesture")
        controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: "c"))
        XCTAssertNil(controller.drawOverlay.startPoint)
        XCTAssertEqual(sections.selectedSegment, 0)
        XCTAssertTrue(controller.cropOverlay.croppingEnabled)
        let fields = try ["Crop X", "Crop Y", "Crop width", "Crop height"].map { try field($0, in: controller.root) }
        let previous = fields.map(\.stringValue)
        controller.cropOverlay.begin(at: NSPoint(x: image.minX + 40, y: image.minY + 50))
        controller.cropOverlay.end(at: NSPoint(x: image.minX + 180, y: image.minY + 120))
        let candidate = fields.map(\.stringValue)
        XCTAssertNotEqual(candidate, previous)
        controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: "c"))
        XCTAssertTrue(controller.cropOverlay.croppingEnabled)
        XCTAssertEqual(fields.map(\.stringValue), candidate)
        controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 0, characters: "v"))
        XCTAssertEqual(sections.selectedSegment, 1)
        XCTAssertFalse(controller.cropOverlay.croppingEnabled)
        XCTAssertEqual(fields.map(\.stringValue), previous, "switching tools cancels, not applies, crop")
        XCTAssertEqual(controller.state.snapshot, original)
        XCTAssertTrue(worker.requests.isEmpty && worker.encodes.isEmpty && worker.saves.isEmpty)
    }

    func testToolKeysRespectFieldsControlsModifiersAndPendingWork() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!,
            worker: worker, writeClipboard: { _ in true })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        let sections = try segmented("Editor section", in: controller.root)
        let crop = try field("Crop X", in: controller.root)
        XCTAssertTrue(controller.window.makeFirstResponder(crop))
        let pen = try keyEvent(window: controller.window, keyCode: 35, characters: "p")
        XCTAssertFalse(controller.window.performKeyEquivalent(with: pen))
        XCTAssertEqual(sections.selectedSegment, 0)
        try showDraw(in: controller.root)
        let popup = try popup("Drawing tool", in: controller.root)
        XCTAssertTrue(controller.window.makeFirstResponder(popup))
        XCTAssertFalse(controller.window.performKeyEquivalent(with: pen))
        XCTAssertEqual(controller.drawOverlay.shape, .rectangle)
        XCTAssertTrue(controller.window.makeFirstResponder(sections))
        XCTAssertFalse(controller.window.performKeyEquivalent(with: pen))
        controller.window.makeFirstResponder(nil)
        for flags in [NSEvent.ModifierFlags.command, .control, .option] {
            let event = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags,
                timestamp: 0, windowNumber: controller.window.windowNumber, context: nil,
                characters: "p", charactersIgnoringModifiers: "p", isARepeat: false, keyCode: 35))
            _ = controller.window.performKeyEquivalent(with: event)
            XCTAssertEqual(controller.drawOverlay.shape, .rectangle)
        }
        worker.deferEncodes = true
        try button("Copy image", in: controller.root).performClick(nil)
        XCTAssertTrue(controller.state.busy)
        XCTAssertTrue(controller.window.performKeyEquivalent(with: pen))
        XCTAssertEqual(controller.drawOverlay.shape, .rectangle)
        worker.completePendingEncode()
        XCTAssertTrue(controller.window.performKeyEquivalent(with: pen))
        XCTAssertEqual(controller.drawOverlay.shape, .pen)
        XCTAssertTrue(worker.requests.isEmpty && worker.saves.isEmpty)
        XCTAssertEqual(worker.encodes.count, 1)
    }

    func testArrowNudgesKeepTypingLocksAndOneAcceptedCommand() throws {
        _ = NSApplication.shared
        let hidden = layer(id: "hidden", name: "Hidden", x: 8, y: 21,
                           visible: false, locked: false, opacity: 60)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [hidden]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)
        func arrow(_ code: UInt16, shift: Bool = false) throws -> NSEvent {
            let characters = [123: "\u{f702}", 124: "\u{f703}", 125: "\u{f701}", 126: "\u{f700}"][Int(code)]!
            return try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: shift ? .shift : [], timestamp: 0,
                windowNumber: controller.window.windowNumber, context: nil,
                characters: characters, charactersIgnoringModifiers: characters, isARepeat: true, keyCode: code))
        }
        XCTAssertTrue(controller.window.makeFirstResponder(try field("Layer name", in: controller.root)))
        controller.window.sendEvent(try arrow(123))
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertTrue(controller.window.makeFirstResponder(try table("Screenshot layers", in: controller.root)))
        controller.window.sendEvent(try arrow(126))
        XCTAssertTrue(worker.requests.isEmpty, "table navigation must not move document content")
        XCTAssertTrue(controller.window.makeFirstResponder(try button("Duplicate", in: controller.root)))
        worker.deferRequests = true
        let cases: [(UInt16, Bool, Double, Double)] = [
            (123, false, -1, 0), (124, true, 10, 0), (126, true, 0, -10), (125, false, 0, 1),
        ]
        for (index, item) in cases.enumerated() {
            controller.window.sendEvent(try arrow(item.0, shift: item.1))
            controller.window.sendEvent(try arrow(item.0, shift: item.1))
            XCTAssertEqual(worker.requests.count, index + 1, "busy repeats cannot queue movement")
            XCTAssertEqual(worker.requests.last?["id"] as? String, "hidden")
            let edit = try XCTUnwrap(worker.requests.last?["edit"] as? [String: Any])
            XCTAssertEqual(edit["action"] as? String, "translate", "keyboard movement must not snap or expand")
            XCTAssertEqual(edit["delta_x"] as? Double, item.2)
            XCTAssertEqual(edit["delta_y"] as? Double, item.3)
            let result = index == 3
                ? layer(id: "hidden", name: "Hidden", x: 8, y: 21, visible: false, locked: true, opacity: 60)
                : hidden
            worker.completePending(with: snapshot(id: "shot", layers: [result]))
        }
        controller.window.sendEvent(try arrow(123, shift: true))
        XCTAssertEqual(worker.requests.count, 4, "locked selection cannot be nudged")
    }

    func testLayerShortcutsRespectTypingLocksAcceptedSelectionAndBusyCommands() throws {
        _ = NSApplication.shared
        let original = layer(id: "original", name: "Original", x: 8, y: 21,
                             visible: false, locked: true, opacity: 60)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)
        func event(_ key: String, _ code: UInt16, _ modifiers: NSEvent.ModifierFlags = []) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers,
                timestamp: 0, windowNumber: controller.window.windowNumber, context: nil,
                characters: key, charactersIgnoringModifiers: key, isARepeat: true, keyCode: code))
        }
        let nameField = try field("Layer name", in: controller.root)
        XCTAssertTrue(controller.window.makeFirstResponder(nameField))
        _ = controller.window.performKeyEquivalent(with: try event("d", 2, .command))
        controller.window.sendEvent(try event("\u{7f}", 51))
        XCTAssertTrue(worker.requests.isEmpty, "typing/deletion must stay in the field")
        let duplicate = try button("Duplicate", in: controller.root)
        XCTAssertTrue(controller.window.makeFirstResponder(duplicate))
        controller.window.sendEvent(try event("\u{7f}", 51))
        controller.window.sendEvent(try event("\u{f728}", 117))
        XCTAssertTrue(worker.requests.isEmpty, "locked layers cannot be deleted with either key")
        worker.deferRequests = true
        XCTAssertTrue(controller.window.performKeyEquivalent(with: try event("d", 2, .control)))
        XCTAssertEqual(worker.requests.count, 1)
        XCTAssertEqual(worker.requests[0]["id"] as? String, "original")
        let edit = try XCTUnwrap(worker.requests[0]["edit"] as? [String: Any])
        XCTAssertEqual(edit["action"] as? String, "duplicate")
        let newID = try XCTUnwrap(edit["new_id"] as? String)
        XCTAssertNotNil(UUID(uuidString: newID))
        XCTAssertNotEqual(newID, "original")
        _ = controller.window.performKeyEquivalent(with: try event("d", 2, .command))
        controller.window.sendEvent(try event("\u{7f}", 51))
        XCTAssertEqual(worker.requests.count, 1, "busy shortcuts cannot queue commands")
        let copy = layer(id: newID, name: "Copy", x: 32, y: 45, visible: true, locked: false, opacity: 60)
        worker.completePending(with: snapshot(id: "shot", unsaved: true, layers: [original, copy]))
        XCTAssertTrue(controller.window.makeFirstResponder(nameField))
        controller.window.sendEvent(try event("\u{7f}", 51))
        _ = controller.window.performKeyEquivalent(with: try event("d", 2, .command))
        XCTAssertEqual(worker.requests.count, 1, "typing must protect an unlocked selection too")
        XCTAssertTrue(controller.window.makeFirstResponder(duplicate))
        controller.window.sendEvent(try event("\u{f728}", 117))
        XCTAssertEqual(worker.requests.count, 2)
        XCTAssertEqual(worker.requests.last?["id"] as? String, newID, "accepted copy becomes selected")
        XCTAssertEqual((worker.requests.last?["edit"] as? [String: Any])?["action"] as? String, "delete")
        worker.completePending(with: snapshot(id: "shot", unsaved: true, layers: [original]))
        worker.deferRequests = false; worker.failLayerAction = "duplicate"
        _ = controller.window.performKeyEquivalent(with: try event("d", 2, .command))
        XCTAssertEqual(worker.requests.count, 3)
        XCTAssertEqual((try field("Layer name", in: controller.root)).stringValue, "Original",
                       "failed duplication preserves the original selection")
        controller.window.sendEvent(try event("\u{7f}", 51))
        XCTAssertEqual(worker.requests.count, 3, "fallback selection is still locked")
    }

    func testHistoryShortcutsUseAcceptedCommandsAndLeaveFieldUndoAlone() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        func event(_ modifiers: NSEvent.ModifierFlags) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers,
                timestamp: 0, windowNumber: controller.window.windowNumber, context: nil,
                characters: "z", charactersIgnoringModifiers: "z", isARepeat: true, keyCode: 6))
        }
        let undo = try button("Undo", in: controller.root)
        let redo = try button("Redo", in: controller.root)
        XCTAssertTrue(undo.keyEquivalent.isEmpty && redo.keyEquivalent.isEmpty)
        let field = try field("Crop X", in: controller.root)
        XCTAssertTrue(controller.window.makeFirstResponder(field))
        _ = controller.window.performKeyEquivalent(with: try event(.command))
        _ = controller.window.performKeyEquivalent(with: try event([.control, .shift]))
        XCTAssertTrue(worker.requests.isEmpty, "field editors must not dispatch document history")

        controller.window.makeFirstResponder(undo)
        worker.deferRequests = true
        XCTAssertTrue(controller.window.performKeyEquivalent(with: try event(.control)))
        XCTAssertEqual(worker.requests.map { $0["operation"] as? String }, ["undo"])
        XCTAssertTrue(controller.state.busy)
        _ = controller.window.performKeyEquivalent(with: try event(.command))
        _ = controller.window.performKeyEquivalent(with: try event([.command, .shift]))
        XCTAssertEqual(worker.requests.count, 1, "repeats do not queue behind accepted work")
        worker.completePending(with: snapshot(id: "shot", canRedo: true))
        XCTAssertTrue(controller.window.performKeyEquivalent(with: try event([.command, .shift])))
        XCTAssertEqual(worker.requests.map { $0["operation"] as? String }, ["undo", "redo"])
        worker.completePending(with: snapshot(id: "shot"))
        _ = controller.window.performKeyEquivalent(with: try event(.command))
        _ = controller.window.performKeyEquivalent(with: try event([.control, .shift]))
        XCTAssertEqual(worker.requests.count, 2, "disabled history actions stay disabled")
    }

    func testZoomSliderUsesSharedLogScaleAndRetainsDocumentAndOutput() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let original = snapshot(id: "shot", width: 160, height: 90, unsaved: true, draft: true)
            let worker = FakeEditorWorker(snapshot: original)
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            let slider = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSSlider }
                .first { $0.accessibilityLabel() == "Canvas zoom" })
            XCTAssertEqual(slider.doubleValue, 0.5902718572045537, accuracy: 1e-12,
                           "Fit uses the small image's actual 100%, not the zero sentinel")
            XCTAssertTrue(slider.isContinuous)
            try showOutput(in: controller.root)
            try button("Preview output", in: controller.root).performClick(nil)
            let mode = try segmented("Output preview image", in: controller.root)
            for (position, percent) in [(0.0, 5.0), (0.5, 63.2), (0.75, 224.9), (1.0, 800.0)] {
                slider.doubleValue = position
                _ = slider.sendAction(slider.action, to: slider.target)
                XCTAssertEqual(controller.viewport.zoomPercent, percent)
                XCTAssertEqual(controller.state.snapshot, original)
                XCTAssertEqual(mode.selectedSegment, 1)
                XCTAssertTrue(mode.isEnabled)
                XCTAssertEqual(slider.accessibilityValueDescription(), "\(percent == floor(percent) ? String(Int(percent)) : String(percent))%")
            }
            XCTAssertTrue(worker.requests.isEmpty)
            XCTAssertEqual(worker.encodes.count, 1)
            try render(controller.root, name: "screenshot-editor-zoom-slider-maximum-\(appearance)")
            try button("Fit", in: controller.root).performClick(nil)
            XCTAssertEqual(controller.viewport, NativeEditorViewport())
            XCTAssertEqual(slider.doubleValue, 0.5902718572045537, accuracy: 1e-12)
            try render(controller.root, name: "screenshot-editor-zoom-slider-fit-\(appearance)")
        }
        XCTAssertNil(NativeEditorViewport.sliderZoom(position: .nan))
        XCTAssertNil(NativeEditorViewport.sliderPosition(percent: .infinity))
    }

    func testReplaceOriginalRequiresConfirmationAndUsesImmutableRequest() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", originalExportPath: "/exports/original.png"))
        var confirmation: ((Bool) -> Void)?
        var confirmedPath: String?
        var replaced: [String] = []
        let controller = ScreenshotEditorController(
            tokens: Tokens.variants["light-mustard"]!, worker: worker,
            didReplaceOriginal: { replaced.append($0) },
            confirmReplaceOriginal: { _, path, completion in
                confirmedPath = path; confirmation = completion
            })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        let replace = try button("Replace original…", in: controller.root)
        replace.scrollToVisible(replace.bounds)
        try render(controller.root, name: "screenshot-editor-replace-original-light")
        replace.performClick(nil)
        XCTAssertEqual(confirmedPath, "/exports/original.png")
        XCTAssertTrue(worker.originalSaves.isEmpty)
        XCTAssertFalse(controller.state.busy)
        XCTAssertFalse(replace.isEnabled, "a second confirmation must not overlap")

        confirmation?(false)
        XCTAssertTrue(worker.originalSaves.isEmpty)
        XCTAssertFalse(controller.state.busy)
        replace.performClick(nil)
        // Simulate a late control callback while the native sheet is open.
        (try field("Output filename", in: controller.root)).stringValue = "different.webp"
        let format = try popup("Output format", in: controller.root)
        format.selectItem(withTitle: "WebP")
        _ = format.sendAction(format.action, to: format.target)
        confirmation?(true)
        XCTAssertEqual(worker.originalSaves.count, 1)
        XCTAssertEqual(worker.originalSaves[0]["destination"] as? String, "/exports/original.png")
        XCTAssertEqual((worker.originalSaves[0]["options"] as? [String: Any])?["format"] as? String, "png")
        XCTAssertEqual(replaced, ["shot"])
    }

    func testReplaceOriginalResultScopeAndStaleConfirmation() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "first", originalExportPath: "/exports/first.png"))
        var confirmation: ((Bool) -> Void)?
        var errors: [String] = [], replaced: [String] = []
        let controller = ScreenshotEditorController(
            tokens: Tokens.variants["dark-mustard"]!, worker: worker,
            reportError: { errors.append($0) }, didReplaceOriginal: { replaced.append($0) },
            confirmReplaceOriginal: { _, _, completion in confirmation = completion })
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "first"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        try button("Replace original…", in: controller.root).performClick(nil)
        worker.snapshot = snapshot(id: "second", originalExportPath: "/exports/second.png")
        controller.present(artifact: artifact(id: "second"), historyRoot: "/native/History")
        confirmation?(true)
        XCTAssertTrue(worker.originalSaves.isEmpty)
        XCTAssertTrue(errors.last?.contains("changed before replacement") == true)

        try button("Preview output", in: controller.root).performClick(nil)
        let outputMode = try segmented("Output preview image", in: controller.root)
        let snapshotBeforeSave = controller.state.snapshot
        worker.saveOriginalResult = .success(.savedWithoutHistory(path: "/exports/second.png", warning: "database locked"))
        try button("Replace original…", in: controller.root).performClick(nil)
        confirmation?(true)
        XCTAssertEqual(controller.state.snapshot, snapshotBeforeSave)
        XCTAssertTrue(outputMode.isEnabled)
        XCTAssertEqual(outputMode.selectedSegment, 1)
        XCTAssertEqual(replaced, ["second"], "partial publication still notifies only the replaced artifact")
        XCTAssertTrue(errors.last?.contains("couldn’t update History: database locked") == true)
        let replace = try button("Replace original…", in: controller.root)
        replace.scrollToVisible(replace.bounds)
        try render(controller.root, name: "screenshot-editor-replace-original-history-error-dark")

        worker.saveOriginalResult = .failure(AppBridgeError.backend("denied"))
        try button("Replace original…", in: controller.root).performClick(nil)
        confirmation?(true)
        XCTAssertEqual(replaced, ["second"], "failed publication must not notify")
        XCTAssertTrue(errors.last?.contains("Couldn’t replace original: denied") == true)
    }

    func testReplaceOriginalActionOnlyAppearsForSavedOriginal() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        XCTAssertTrue(try button("Replace original…", in: controller.root).isHidden)
        try render(controller.root, name: "screenshot-editor-no-original-minimum-light")
    }

    func testReplaceOriginalNativeSheetHasExplicitCancelAndTarget() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", originalExportPath: "/exports/My capture.png"))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showOutput(in: controller.root)
            try button("Replace original…", in: controller.root).performClick(nil)
            let sheet = try XCTUnwrap(controller.window.attachedSheet)
            settle(sheet)
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
            let content = try XCTUnwrap(sheet.contentView)
            let buttons = descendants(in: content).compactMap { $0 as? NSButton }
            XCTAssertTrue(buttons.contains { $0.title == "Replace" })
            XCTAssertTrue(buttons.contains { $0.title == "Cancel" })
            XCTAssertTrue(worker.originalSaves.isEmpty)
            if let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                // NSAlert uses WindowServer-composited material and controls;
                // cacheDisplay produces blank/incomplete images for its content view.
                let url = URL(fileURLWithPath: directory)
                    .appendingPathComponent("screenshot-editor-replace-confirm-\(appearance).png")
                try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                        withIntermediateDirectories: true)
                let capture = Process()
                capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                capture.arguments = ["-x", "-o", "-l", String(sheet.windowNumber), url.path]
                try capture.run(); capture.waitUntilExit()
                XCTAssertEqual(capture.terminationStatus, 0, "Native sheet capture must be composited")
            }
            controller.window.endSheet(sheet, returnCode: .alertSecondButtonReturn)
            sheet.orderOut(nil)
            XCTAssertTrue(worker.originalSaves.isEmpty)
        }
    }

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

    func testFitCapsSmallImagesAndPreservesManualZoomWithoutDocumentWork() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let original = snapshot(id: "shot", width: 160, height: 90, unsaved: true, draft: true)
            let worker = FakeEditorWorker(snapshot: original)
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            let input = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? EditorViewportGestureView }
                .first { $0.accessibilityLabel() == "Screenshot viewport" })
            XCTAssertEqual(controller.presentedImageRect.size, NSSize(width: 160, height: 90))
            XCTAssertEqual(controller.presentedImageRect.midX, input.bounds.midX, accuracy: 0.001)
            XCTAssertEqual(controller.presentedImageRect.midY, input.bounds.midY, accuracy: 0.001)
            try render(controller.root, name: "screenshot-editor-fit-small-\(appearance)")
            try button("+", in: controller.root).performClick(nil)
            XCTAssertEqual(controller.viewport.zoomPercent, 125, "step starts from capped Fit")
            XCTAssertEqual(controller.presentedImageRect.size, NSSize(width: 200, height: 112.5))
            try chooseZoomPreset("200%", in: controller.root)
            XCTAssertEqual(controller.presentedImageRect.size, NSSize(width: 320, height: 180))
            input.onViewportPan?(NSPoint(x: 37, y: -21))
            try chooseZoomPreset("Fit", in: controller.root)
            XCTAssertEqual(controller.viewport, NativeEditorViewport())
            XCTAssertEqual(controller.presentedImageRect.size, NSSize(width: 160, height: 90))
            XCTAssertEqual(controller.state.snapshot, original)
            XCTAssertTrue(worker.requests.isEmpty); XCTAssertTrue(worker.encodes.isEmpty)
        }
    }

    func testViewportControlsAreAccessibleAndDoNotMutateEditorState() throws {
        _ = NSApplication.shared
        let original = snapshot(id: "shot", unsaved: true, draft: true)
        let worker = FakeEditorWorker(snapshot: original)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        let labels = ["Fit screenshot in viewport",
                      "Zoom out", "Zoom in", "Recenter screenshot"]
        let controls = descendants(in: controller.root).compactMap { $0 as? CaptureButton }
            .filter { labels.contains($0.accessibilityLabel() ?? "") }
        XCTAssertEqual(controls.count, 4)
        try chooseZoomPreset("100%", in: controller.root)
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

    func testWindowResizeKeepsInspectorAndZoomControlsVisibleAndFitCentered() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let original = snapshot(id: "shot", width: 640, height: 360, unsaved: true, draft: true)
            let worker = FakeEditorWorker(snapshot: original)
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            XCTAssertTrue(controller.window.styleMask.contains(.resizable))
            XCTAssertEqual(controller.window.contentMinSize, NSSize(width: 760, height: 540))
            let input = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? EditorViewportGestureView }
                .first { $0.accessibilityLabel() == "Screenshot viewport" })
            let section = try segmented("Editor section", in: controller.root)
            let inspector = try XCTUnwrap(descendants(in: controller.root)
                .first { $0.accessibilityLabel() == "Geometry controls" })
            let undo = try button("Undo", in: controller.root)
            // Exercise both axes without exceeding the CI display's height.
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            waitUntil { input.bounds.size == NSSize(width: 300, height: 318) }
            controller.window.setContentSize(NSSize(width: 1200, height: 600))
            controller.root.layoutSubtreeIfNeeded()
            waitUntil { controller.presentedImageRect == NSRect(x: 50, y: 27, width: 640, height: 360) }
            XCTAssertEqual(input.bounds.size, NSSize(width: 740, height: 414))
            XCTAssertEqual(controller.root.bounds.size, NSSize(width: 1200, height: 600))
            XCTAssertEqual(section.frame.minX, 888)
            XCTAssertEqual(inspector.frame, NSRect(x: 888, y: 66, width: 272, height: 246))
            XCTAssertEqual(undo.frame.origin, NSPoint(x: 888, y: 326))
            XCTAssertEqual(controller.presentedImageRect, NSRect(x: 50, y: 27, width: 640, height: 360))
            for control in descendants(in: controller.root) where
                ["Canvas zoom", "Canvas zoom preset", "Edited canvas dimensions", "Screenshot editor status"]
                    .contains(control.accessibilityLabel() ?? "") {
                XCTAssertTrue(controller.root.bounds.contains(control.convert(control.bounds, to: controller.root)))
            }
            try render(controller.root, name: "screenshot-editor-resized-fit-\(appearance)")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            controller.root.layoutSubtreeIfNeeded()
            waitUntil { controller.presentedImageRect.width == 300 }
            XCTAssertEqual(input.bounds.size, NSSize(width: 300, height: 318))
            XCTAssertEqual(section.frame.minX, 448)
            XCTAssertEqual(inspector.frame, NSRect(x: 448, y: 66, width: 272, height: 186))
            XCTAssertEqual(undo.frame.origin, NSPoint(x: 448, y: 266))
            XCTAssertEqual(controller.presentedImageRect.minX, 0, accuracy: 1e-7)
            XCTAssertEqual(controller.presentedImageRect.minY, 74.625, accuracy: 1e-7)
            XCTAssertEqual(controller.presentedImageRect.width, 300, accuracy: 1e-7)
            XCTAssertEqual(controller.presentedImageRect.height, 168.75, accuracy: 1e-7)
            try render(controller.root, name: "screenshot-editor-resized-minimum-\(appearance)")
            XCTAssertEqual(controller.viewport, NativeEditorViewport())
            XCTAssertEqual(controller.state.snapshot, original)
            XCTAssertTrue(worker.requests.isEmpty)
            XCTAssertTrue(worker.encodes.isEmpty)
        }
    }

    func testExportActionsStayVisibleAcrossSectionsAndDisableTogetherDuringWork() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: true))
            worker.deferEncodes = true
            var copies = 0
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                worker: worker, writeClipboard: { _ in copies += 1; return true })
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History",
                               outputDirectory: "/native/Exports")
            let copy = try button("Copy image", in: controller.root)
            let save = try button("Save new copy", in: controller.root)
            let sections = try segmented("Editor section", in: controller.root)
            for size in [NSSize(width: 1200, height: 600), NSSize(width: 760, height: 540)] {
                controller.window.setContentSize(size)
                controller.root.layoutSubtreeIfNeeded()
                XCTAssertEqual(copy.frame, NSRect(x: size.width - 312, y: size.height - 70, width: 100, height: 34))
                XCTAssertEqual(save.frame, NSRect(x: size.width - 192, y: size.height - 70, width: 152, height: 34))
                for index in 0..<4 {
                    sections.selectedSegment = index
                    _ = sections.sendAction(sections.action, to: sections.target)
                    for action in [copy, save] {
                        XCTAssertTrue(action.superview === controller.root)
                        XCTAssertNil(action.enclosingScrollView)
                        XCTAssertFalse(action.isHiddenOrHasHiddenAncestor)
                        XCTAssertTrue(action.isEnabled)
                        XCTAssertTrue(controller.root.bounds.contains(action.frame))
                    }
                }
            }
            try showDraw(in: controller.root)
            copy.performClick(nil)
            XCTAssertTrue(controller.state.busy)
            XCTAssertFalse(copy.isEnabled); XCTAssertFalse(save.isEnabled)
            save.performClick(nil)
            XCTAssertTrue(worker.saves.isEmpty)
            try render(controller.root, name: "screenshot-editor-pinned-export-pending-\(appearance)")
            worker.completePendingEncode()
            XCTAssertEqual(copies, 1)
            XCTAssertTrue(copy.isEnabled); XCTAssertTrue(save.isEnabled)
            XCTAssertTrue(worker.requests.isEmpty)
        }
    }

    func testWindowResizeCancelsGesturesWithoutResettingManualViewportOrDraft() throws {
        _ = NSApplication.shared
        let original = snapshot(id: "shot", unsaved: true, draft: true)
        let worker = FakeEditorWorker(snapshot: original)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try button("Draw crop", in: controller.root).performClick(nil)
        let image = controller.presentedImageRect
        controller.cropOverlay.begin(at: NSPoint(x: image.minX + 60, y: image.minY + 70))
        controller.cropOverlay.drag(to: NSPoint(x: image.minX + 240, y: image.minY + 180))
        XCTAssertTrue(controller.cropOverlay.croppingEnabled)
        controller.windowDidResize(Notification(name: NSWindow.didResizeNotification, object: controller.window))
        XCTAssertTrue(controller.cropOverlay.croppingEnabled, "same-size notifications preserve the gesture")
        controller.window.setContentSize(NSSize(width: 1100, height: 580))
        waitUntil { !controller.cropOverlay.croppingEnabled }
        XCTAssertEqual(try field("Crop X", in: controller.root).stringValue, "0")
        XCTAssertEqual(try field("Crop width", in: controller.root).stringValue, "640")
        try chooseZoomPreset("200%", in: controller.root)
        let input = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? EditorViewportGestureView }
            .first { $0.accessibilityLabel() == "Screenshot viewport" })
        input.onViewportPan?(NSPoint(x: 37, y: -21))
        let viewport = controller.viewport
        try showDraw(in: controller.root)
        controller.drawOverlay.begin(at: NSPoint(x: 120, y: 140))
        XCTAssertNotNil(controller.drawOverlay.startPoint)
        controller.window.setContentSize(NSSize(width: 1200, height: 600))
        waitUntil { controller.drawOverlay.startPoint == nil && input.bounds.width == 740
            && controller.presentedImageRect.midX == 370 + CGFloat(viewport.panX) }
        controller.drawOverlay.end(at: NSPoint(x: 260, y: 220))
        XCTAssertEqual(controller.viewport, viewport)
        XCTAssertEqual(controller.presentedImageRect.midX, 370 + CGFloat(viewport.panX), accuracy: 1e-7)
        XCTAssertEqual(controller.presentedImageRect.midY, 207 + CGFloat(viewport.panY), accuracy: 1e-7)
        let point = NSPoint(x: 160, y: 180)
        let down = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseDown,
            location: controller.drawOverlay.convert(point, to: nil), modifierFlags: [.command],
            timestamp: 0, windowNumber: controller.window.windowNumber, context: nil,
            eventNumber: 0, clickCount: 1, pressure: 1))
        controller.drawOverlay.mouseDown(with: down)
        XCTAssertTrue(controller.drawOverlay.isViewportPanning)
        controller.window.setContentSize(NSSize(width: 1000, height: 700))
        waitUntil { !controller.drawOverlay.isViewportPanning }
        XCTAssertEqual(controller.viewport, viewport)
        XCTAssertEqual(controller.state.snapshot, original)
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertTrue(worker.encodes.isEmpty)
    }

    func testCompactWindowWrapsFooterAndKeepsEveryInspectorReachable() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let original = snapshot(id: "shot", width: 640, height: 360, unsaved: true, draft: true)
            let worker = FakeEditorWorker(snapshot: original)
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            let input = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? EditorViewportGestureView }
                .first { $0.accessibilityLabel() == "Screenshot viewport" })
            let dimensions = try field("Edited canvas dimensions", in: controller.root)
            let zoom = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSSlider }
                .first { $0.accessibilityLabel() == "Canvas zoom" })
            let sections = try segmented("Editor section", in: controller.root)
            // Both sides of the wrapping threshold, and a return from compact
            // to wide, catch stale autoresizing offsets and cumulative drift.
            for width in [CGFloat(999), 1000, 760, 1000] {
                controller.window.setContentSize(NSSize(width: width, height: 540))
                controller.root.layoutSubtreeIfNeeded()
                waitUntil { input.bounds.width == width - 460 }
                XCTAssertEqual(zoom.frame.origin.y, width < 1000 ? 454 : 490)
                XCTAssertEqual(dimensions.frame.origin, NSPoint(x: width < 1000 ? 24 : 440, y: 494))
                XCTAssertEqual(input.bounds.height, width < 1000 ? 318 : 354)
                XCTAssertFalse(zoom.frame.intersects(dimensions.frame))
                XCTAssertLessThanOrEqual(zoom.frame.maxX, sections.frame.minX - 24)
            }
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            waitUntil { controller.presentedImageRect.width == 300 }
            XCTAssertEqual(controller.presentedImageRect, NSRect(x: 0, y: 74.625, width: 300, height: 168.75))
            XCTAssertEqual(zoom.frame.width, 92)
            XCTAssertEqual(sections.frame, NSRect(x: 448, y: 24, width: 272, height: 28))
            for control in controller.root.subviews where !control.isHidden {
                XCTAssertTrue(controller.root.bounds.contains(control.frame), "\(control) must fit at minimum size")
            }
            try render(controller.root, name: "screenshot-editor-compact-fit-\(appearance)")
            let controls: [NSView] = [try button("Trim edges", in: controller.root),
                try button("Add image…", in: controller.root),
                try popup("Drawing tool", in: controller.root),
                try button("Change…", in: controller.root)]
            for (index, control) in controls.enumerated() {
                sections.selectedSegment = index
                _ = sections.sendAction(sections.action, to: sections.target)
                let scroll = try XCTUnwrap(control.enclosingScrollView)
                control.scrollToVisible(control.bounds)
                controller.root.layoutSubtreeIfNeeded()
                XCTAssertTrue(scroll.contentView.bounds.contains(control.convert(control.bounds, to: scroll.contentView)),
                              "Section \(index) remains reachable in a compact inspector")
                XCTAssertFalse(control.isHiddenOrHasHiddenAncestor)
                try render(controller.root, name: "screenshot-editor-compact-section-\(index)-\(appearance)")
            }
            sections.selectedSegment = 0
            _ = sections.sendAction(sections.action, to: sections.target)
            let crop = try button("Apply crop", in: controller.root)
            crop.scrollToVisible(crop.bounds)
            try field("Crop width", in: controller.root).stringValue = "0"
            crop.performClick(nil)
            XCTAssertTrue(labels(in: controller.root).contains("Crop values must be finite numbers with positive width and height."))
            try render(controller.root, name: "screenshot-editor-compact-error-\(appearance)")
            XCTAssertEqual(controller.state.snapshot, original)
            XCTAssertTrue(worker.requests.isEmpty)
            XCTAssertTrue(worker.encodes.isEmpty)
        }
    }

    func testZoomPresetsTrackCustomZoomAndFitWithoutDocumentWork() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let original = snapshot(id: "shot", width: 640, height: 360, unsaved: true, draft: true)
            let worker = FakeEditorWorker(snapshot: original)
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            let preset = try popup("Canvas zoom preset", in: controller.root)
            XCTAssertEqual(preset.itemTitles, ["Fit", "50%", "100%", "200%"])
            XCTAssertEqual(preset.titleOfSelectedItem, "Fit")
            try chooseZoomPreset("50%", in: controller.root)
            XCTAssertEqual(controller.presentedImageRect.size, NSSize(width: 320, height: 180))
            try render(controller.root, name: "screenshot-editor-zoom-preset-50-\(appearance)")
            try chooseZoomPreset("200%", in: controller.root)
            XCTAssertEqual(controller.presentedImageRect.size, NSSize(width: 1280, height: 720))
            try chooseZoomPreset("100%", in: controller.root)
            try button("+", in: controller.root).performClick(nil)
            try button("+", in: controller.root).performClick(nil)
            XCTAssertEqual(preset.titleOfSelectedItem, "156.3%")
            XCTAssertEqual(preset.itemTitles, ["Fit", "156.3%", "50%", "100%", "200%"])
            try render(controller.root, name: "screenshot-editor-zoom-preset-custom-minimum-\(appearance)")
            let input = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? EditorViewportGestureView }
                .first { $0.accessibilityLabel() == "Screenshot viewport" })
            input.onViewportPan?(NSPoint(x: -71, y: 39))
            try chooseZoomPreset("Fit", in: controller.root)
            XCTAssertEqual(controller.viewport, NativeEditorViewport())
            XCTAssertEqual(preset.itemTitles, ["Fit", "50%", "100%", "200%"])
            input.onViewportPan?(NSPoint(x: 37, y: -21))
            XCTAssertNotEqual(controller.viewport.panX, 0)
            XCTAssertEqual(preset.titleOfSelectedItem, "Fit")
            try chooseZoomPreset("Fit", in: controller.root)
            XCTAssertEqual(controller.viewport, NativeEditorViewport())
            XCTAssertTrue(controller.root.bounds.contains(preset.convert(preset.bounds, to: controller.root)))
            XCTAssertEqual(controller.state.snapshot, original)
            XCTAssertTrue(worker.requests.isEmpty); XCTAssertTrue(worker.encodes.isEmpty)
        }
    }

    func testZoomShortcutsUseActualSizeAndWorkInFieldsWithoutEditing() throws {
        _ = NSApplication.shared
        let original = snapshot(id: "shot", width: 640, height: 360)
        let worker = FakeEditorWorker(snapshot: original)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        func event(_ key: String, code: UInt16, flags: NSEvent.ModifierFlags) -> NSEvent {
            NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags,
                timestamp: 0, windowNumber: controller.window.windowNumber, context: nil,
                characters: key, charactersIgnoringModifiers: key, isARepeat: true, keyCode: code)!
        }
        try showDraw(in: controller.root)
        controller.drawOverlay.begin(at: NSPoint(x: 120, y: 140))
        XCTAssertNotNil(controller.drawOverlay.startPoint)
        XCTAssertTrue(controller.window.performKeyEquivalent(with: event("0", code: 29, flags: .command)))
        XCTAssertEqual(controller.viewport.zoomPercent, 100, "zero is actual size, not Fit")
        XCTAssertNil(controller.drawOverlay.startPoint)
        let section = try segmented("Editor section", in: controller.root)
        section.selectedSegment = 0; _ = section.sendAction(section.action, to: section.target)
        let field = try field("Canvas width", in: controller.root)
        field.selectText(nil)
        let content = field.stringValue
        controller.window.sendEvent(event("=", code: 24, flags: .control))
        XCTAssertEqual(controller.viewport.zoomPercent, 125)
        XCTAssertTrue(controller.window.performKeyEquivalent(with: event("+", code: 69, flags: .command)))
        XCTAssertEqual(controller.viewport.zoomPercent, 156.3)
        XCTAssertTrue(controller.window.performKeyEquivalent(with: event("_", code: 27, flags: [.command, .shift])))
        XCTAssertEqual(controller.viewport.zoomPercent, 125)
        XCTAssertTrue(controller.window.performKeyEquivalent(with: event("-", code: 78, flags: .command)))
        XCTAssertEqual(controller.viewport.zoomPercent, 100)
        XCTAssertEqual(field.stringValue, content)
        XCTAssertFalse(controller.window.performKeyEquivalent(with: event("+", code: 69, flags: [])))
        for _ in 0..<20 { controller.window.sendEvent(event("+", code: 69, flags: .command)) }
        XCTAssertEqual(controller.viewport.zoomPercent, 800)
        for _ in 0..<30 { controller.window.sendEvent(event("-", code: 78, flags: .command)) }
        XCTAssertEqual(controller.viewport.zoomPercent, 5)
        XCTAssertTrue(controller.window.performKeyEquivalent(with: event("0", code: 82, flags: .control)))
        XCTAssertEqual(controller.viewport.zoomPercent, 100)
        let sheet = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 120, height: 80),
                             styleMask: [.titled], backing: .buffered, defer: false)
        controller.window.beginSheet(sheet)
        _ = controller.window.performKeyEquivalent(with: event("+", code: 69, flags: .command))
        XCTAssertEqual(controller.viewport.zoomPercent, 100, "a sheet owns keyboard input")
        controller.window.endSheet(sheet)
        XCTAssertEqual(controller.state.snapshot, original)
        XCTAssertTrue(worker.requests.isEmpty)
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
        try chooseZoomPreset("100%", in: controller.root)
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
            try chooseZoomPreset("100%", in: controller.root)
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

    func testCropOverlayMapsScaledImageCoordinatesAndLatchesSharedAspect() {
        let overlay = EditorCropOverlay(frame: NSRect(x: 0, y: 0, width: 700, height: 400))
        overlay.canvasSize = NSSize(width: 1200, height: 480)
        overlay.imageRect = { NSRect(x: 30, y: 40, width: 600, height: 240) }
        overlay.croppingEnabled = true
        var values: [NSRect] = []
        overlay.onChange = { values.append($0) }
        overlay.begin(at: NSPoint(x: 40, y: 55))
        overlay.end(at: NSPoint(x: 41, y: 55))
        XCTAssertTrue(values.isEmpty, "a click must not replace the candidate with a 1px crop")
        overlay.begin(at: NSPoint(x: 40, y: 55))
        overlay.drag(to: NSPoint(x: 100, y: 75))
        XCTAssertEqual(values.last, NSRect(x: 20, y: 30, width: 120, height: 40))
        overlay.updateModifier(shift: true)
        overlay.drag(to: NSPoint(x: 160, y: 85), shift: true)
        XCTAssertEqual(values.last, NSRect(x: 20, y: 30, width: 240, height: 80))
        overlay.updateModifier(shift: false)
        XCTAssertEqual(values.last, NSRect(x: 20, y: 30, width: 240, height: 60))
        overlay.aspect = 1
        XCTAssertEqual(values.last, NSRect(x: 20, y: 30, width: 240, height: 240))
        overlay.cancelGesture()
        let count = values.count
        overlay.end(at: NSPoint(x: 300, y: 200))
        XCTAssertEqual(values.count, count, "cancelled input cannot later update the fields")
        overlay.aspect = 0
        overlay.begin(at: NSPoint(x: 100, y: 100))
        overlay.end(at: NSPoint(x: -80, y: 600))
        XCTAssertEqual(values.last, NSRect(x: 0, y: 120, width: 140, height: 360))
    }

    func testCropCandidateCancelFailureAndApplyUseOneWorkerTransaction() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        try button("Preview output", in: controller.root).performClick(nil)
        let sections = try segmented("Editor section", in: controller.root)
        sections.selectedSegment = 0; _ = sections.sendAction(sections.action, to: sections.target)
        let fields = try ["Crop X", "Crop Y", "Crop width", "Crop height"].map { try field($0, in: controller.root) }
        let previous = ["12", "7", "320", "180"]
        for (field, value) in zip(fields, previous) { field.stringValue = value }
        try button("Draw crop", in: controller.root).performClick(nil)
        let image = controller.presentedImageRect
        func drag() {
            controller.cropOverlay.begin(at: NSPoint(x: image.minX + image.width * 0.75, y: image.minY + image.height * 0.8))
            controller.cropOverlay.end(at: NSPoint(x: image.minX + image.width * 0.25, y: image.minY + image.height * 0.3))
        }
        drag()
        XCTAssertEqual(fields.map(\.stringValue), ["160", "108", "320", "180"])
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertFalse(controller.state.snapshot?.unsavedChanges ?? true)
        XCTAssertTrue(try segmented("Output preview image", in: controller.root).isEnabled)
        controller.cropOverlay.keyDown(with: try keyEvent(window: controller.window, keyCode: 53, characters: "\u{1b}"))
        XCTAssertEqual(fields.map(\.stringValue), previous)
        XCTAssertFalse(controller.cropOverlay.croppingEnabled)
        try button("Draw crop", in: controller.root).performClick(nil)
        drag()
        fields[0].selectText(nil)
        controller.window.sendEvent(try keyEvent(window: controller.window, keyCode: 53, characters: "\u{1b}"))
        XCTAssertEqual(fields.map(\.stringValue), previous, "Escape works with a numeric field focused")
        XCTAssertTrue(try segmented("Output preview image", in: controller.root).isEnabled)
        try button("Draw crop", in: controller.root).performClick(nil)
        drag()
        worker.failOperation = "crop"
        try button("Apply crop", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.count, 1)
        XCTAssertEqual(controller.state.snapshot?.width, 640)
        XCTAssertEqual(fields.map(\.stringValue), ["160", "108", "320", "180"])
        XCTAssertFalse(controller.cropOverlay.croppingEnabled)
        worker.failOperation = nil; worker.deferRequests = true
        try button("Apply crop", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.count, 2)
        XCTAssertEqual(worker.requests.last?["rect"] as? [String: Double],
                       ["x": 160, "y": 108, "width": 320, "height": 180])
        XCTAssertFalse(try button("Draw crop", in: controller.root).isEnabled)
        worker.completePending(with: snapshot(id: "shot", width: 320, height: 180, unsaved: true))
        XCTAssertEqual(fields.map(\.stringValue), ["0", "0", "320", "180"])
        XCTAssertTrue(controller.state.snapshot?.canUndo == true)
    }

    func testCropFieldsViewportCancellationAndMinimumRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try button("Draw crop", in: controller.root).performClick(nil)
            let aspect = try popup("Crop aspect", in: controller.root)
            XCTAssertEqual(aspect.itemTitles, ["Free", "1:1", "4:3", "3:2", "16:9"])
            aspect.selectItem(withTitle: "4:3"); _ = aspect.sendAction(aspect.action, to: aspect.target)
            let image = controller.presentedImageRect
            controller.cropOverlay.begin(at: NSPoint(x: image.minX + image.width * 0.1, y: image.minY + image.height * 0.2))
            controller.cropOverlay.drag(to: NSPoint(x: image.minX + image.width * 0.5, y: image.minY + image.height * 0.6))
            XCTAssertEqual(controller.cropOverlay.selection, NSRect(x: 64, y: 72, width: 256, height: 192))
            try render(controller.root, name: "screenshot-editor-crop-drag-\(appearance)")
            try button("+", in: controller.root).performClick(nil)
            let candidate = controller.cropOverlay.selection
            controller.cropOverlay.end(at: NSPoint(x: 500, y: 400))
            XCTAssertEqual(controller.cropOverlay.selection, candidate, "zoom cancels only the active pointer gesture")
            let x = try field("Crop X", in: controller.root)
            x.stringValue = "120"
            controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: x))
            XCTAssertEqual(controller.cropOverlay.selection?.minX, 120)
            controller.window.setContentSize(NSSize(width: 1000, height: 700))
            let apply = try button("Apply crop", in: controller.root)
            let scroll = try XCTUnwrap(apply.enclosingScrollView)
            XCTAssertTrue(scroll.contentView.bounds.contains(apply.convert(apply.bounds, to: scroll.contentView)))
            try render(controller.root, name: "screenshot-editor-crop-minimum-\(appearance)")
            controller.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification, object: controller.window))
            XCTAssertFalse(controller.cropOverlay.croppingEnabled)
            XCTAssertEqual(x.stringValue, "0")
            XCTAssertTrue(worker.requests.isEmpty)
            try button("Draw crop", in: controller.root).performClick(nil)
            try showDraw(in: controller.root)
            XCTAssertFalse(controller.cropOverlay.croppingEnabled, "leaving Geometry cancels crop mode")
        }
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
        XCTAssertEqual(controller.cropOverlay.canvasSize, NSSize(width: 640, height: 360),
                       "pointer crops use the rendered pixels, like wgpu, rather than fractional document dimensions")

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

    func testOutputSizingControlsSendOptionsLockAspectAndRetainDocument() throws {
        _ = NSApplication.shared
        let original = snapshot(id: "shot", width: 641, height: 359, unsaved: true, draft: true)
        let worker = FakeEditorWorker(snapshot: original)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        let size = try popup("Output size", in: controller.root)
        let width = try field("Custom output width", in: controller.root)
        let height = try field("Custom output height", in: controller.root)
        let lock = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == "Lock output aspect ratio" })
        let previewMode = try segmented("Output preview image", in: controller.root)

        size.selectItem(withTitle: "75%"); _ = size.sendAction(size.action, to: size.target)
        XCTAssertTrue(labels(in: controller.root).contains("Output: 481 × 269 pixels"))
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertEqual((worker.encodes.last?["size"] as? [String: Any])?["mode"] as? String, "percent")
        XCTAssertEqual((worker.encodes.last?["size"] as? [String: Any])?["percent"] as? Int, 75)
        XCTAssertEqual(previewMode.selectedSegment, 1)

        size.selectItem(withTitle: "Custom"); _ = size.sendAction(size.action, to: size.target)
        XCTAssertEqual(width.stringValue, "641"); XCTAssertEqual(height.stringValue, "359")
        XCTAssertFalse(previewMode.isEnabled, "size changes invalidate an encoded preview without encoding")
        XCTAssertEqual(worker.encodes.count, 1)
        width.stringValue = "320"
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: width))
        XCTAssertEqual(height.stringValue, "179", "locked output dimensions use the document ratio")
        lock.performClick(nil)
        height.stringValue = "123"
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: height))
        XCTAssertEqual(width.stringValue, "320", "unlocked axes change independently")
        try button("Preview output", in: controller.root).performClick(nil)
        let custom = try XCTUnwrap(worker.encodes.last?["size"] as? [String: Any])
        XCTAssertEqual(custom["mode"] as? String, "custom")
        XCTAssertEqual(custom["width"] as? UInt64, 320); XCTAssertEqual(custom["height"] as? UInt64, 123)
        XCTAssertEqual(controller.state.snapshot, original)
        XCTAssertTrue(worker.requests.isEmpty)

        width.stringValue = "16385"
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: width))
        let count = worker.encodes.count
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.encodes.count, count)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("100 million pixels") })
        XCTAssertEqual(controller.state.snapshot, original)
        lock.performClick(nil)
        width.stringValue = String(UInt64.max)
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: width))
        XCTAssertTrue(labels(in: controller.root).contains("Invalid output dimensions."))
        XCTAssertEqual(worker.encodes.count, count, "invalid text must not overflow or encode")
    }

    func testOutputCompressionPresetsMapExactValuesClearPngOverrideAndTrackCustomEdits() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true, draft: true))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        let mode = try popup("Output quality mode", in: controller.root)
        let preset = try popup("Output compression preset", in: controller.root)
        let quality = try field("Output quality value", in: controller.root)
        let palette = try field("PNG maximum colors", in: controller.root)
        XCTAssertTrue(preset.isHidden)
        mode.selectItem(withTitle: "Compress"); _ = mode.sendAction(mode.action, to: mode.target)
        XCTAssertEqual(preset.itemTitles, ["Tiny", "Smaller", "Balanced", "High", "Highest"])
        XCTAssertEqual(preset.titleOfSelectedItem, "Highest")

        preset.selectItem(withTitle: "Tiny"); _ = preset.sendAction(preset.action, to: preset.target)
        XCTAssertEqual(quality.stringValue, "55")
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.encodes.last?["quality_value"] as? UInt64, 55)

        palette.stringValue = "64"
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification,
                                                      object: palette))
        XCTAssertEqual(preset.titleOfSelectedItem, "Custom")
        XCTAssertFalse((try segmented("Output preview image", in: controller.root)).isEnabled,
                       "manual palette edits invalidate a stale encoded preview")
        let format = try popup("Output format", in: controller.root)
        format.selectItem(withTitle: "JPEG"); _ = format.sendAction(format.action, to: format.target)
        XCTAssertEqual(preset.titleOfSelectedItem, "Tiny", "inactive PNG overrides do not change JPEG quality")
        XCTAssertEqual(palette.stringValue, "64")
        format.selectItem(withTitle: "PNG"); _ = format.sendAction(format.action, to: format.target)
        XCTAssertEqual(preset.titleOfSelectedItem, "Custom")
        preset.selectItem(withTitle: "Highest"); _ = preset.sendAction(preset.action, to: preset.target)
        XCTAssertEqual(quality.stringValue, "98")
        XCTAssertEqual(palette.stringValue, "", "a preset must clear explicit PNG quantization")
        try button("Preview output", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.encodes.last?["quality_value"] as? UInt64, 98)
        XCTAssertTrue((worker.encodes.last?["png"] as? [String: Any])?.isEmpty == true)

        quality.stringValue = "73"
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification,
                                                      object: quality))
        XCTAssertEqual(preset.titleOfSelectedItem, "Custom")
        mode.selectItem(withTitle: "Preserve"); _ = mode.sendAction(mode.action, to: mode.target)
        XCTAssertTrue(preset.isHidden)
        mode.selectItem(withTitle: "Compress"); _ = mode.sendAction(mode.action, to: mode.target)
        XCTAssertEqual(quality.stringValue, "73", "mode changes preserve a custom numeric value")
        XCTAssertEqual(preset.titleOfSelectedItem, "Custom")
        XCTAssertEqual(controller.state.snapshot?.hasDraft, true)
        XCTAssertTrue(controller.state.snapshot?.unsavedChanges == true)
    }

    func testTrimControlsUseSharedGeometryAndInvalidateOutput() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let fixture = try makeHistoryFixture()
            defer { try? FileManager.default.removeItem(at: fixture.root) }
            let worker = EditorWorker()
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil); worker.close(); EditorWorker.flush() }
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot != nil && !controller.state.busy }
            let initial = try XCTUnwrap(controller.state.snapshot)
            (try field("Canvas width", in: controller.root)).stringValue = "10"
            (try field("Canvas height", in: controller.root)).stringValue = "5"
            try button("Resize canvas", in: controller.root).performClick(nil)
            waitUntil { controller.state.snapshot?.width == 10 && !controller.state.busy }
            try showOutput(in: controller.root)
            try button("Preview output", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy }
            let output = try segmented("Output preview image", in: controller.root)
            XCTAssertTrue(output.isEnabled)
            let section = try segmented("Editor section", in: controller.root)
            section.selectedSegment = 0; _ = section.sendAction(section.action, to: section.target)
            let trim = try button("Trim edges", in: controller.root)
            let scroll = try XCTUnwrap(trim.enclosingScrollView)
            trim.scrollToVisible(trim.bounds)
            scroll.reflectScrolledClipView(scroll.contentView)
            XCTAssertTrue(scroll.documentVisibleRect.contains(trim.frame))
            try render(controller.root, name: "screenshot-editor-trim-minimum-\(appearance)")
            trim.performClick(nil)
            XCTAssertFalse(trim.isEnabled, "one shared command owns busy state")
            waitUntil { !controller.state.busy }
            XCTAssertEqual(controller.state.snapshot?.width, initial.width)
            XCTAssertEqual(controller.state.snapshot?.height, initial.height)
            XCTAssertEqual(controller.state.snapshot?.layers, initial.layers)
            XCTAssertFalse(output.isEnabled, "trim invalidates encoded output")
            try render(controller.root, name: "screenshot-editor-trim-applied-\(appearance)")
            try button("Undo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy }
            XCTAssertEqual(controller.state.snapshot?.width, 10)
            XCTAssertEqual(controller.state.snapshot?.height, 5)
            try button("Redo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy }
            XCTAssertEqual(controller.state.snapshot?.width, initial.width)
        }
    }

    func testTrimFailureRetainsAcceptedStateAndCanRetry() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let initial = snapshot(id: "shot", width: 640, height: 360)
            let worker = FakeEditorWorker(snapshot: initial)
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            let trim = try button("Trim edges", in: controller.root)
            let scroll = try XCTUnwrap(trim.enclosingScrollView)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: 356))
            scroll.reflectScrolledClipView(scroll.contentView)
            worker.failOperation = "trim_canvas"
            worker.failureMessage = "The trimmed canvas exceeds the image dimension limit. The accepted pixels and draft are unchanged."
            trim.performClick(nil)
            XCTAssertEqual(controller.state.snapshot, initial)
            XCTAssertTrue(trim.isEnabled)
            try render(controller.root, name: "screenshot-editor-trim-error-minimum-\(appearance)")
            worker.failOperation = nil
            trim.performClick(nil)
            XCTAssertEqual(worker.requests.filter { $0["operation"] as? String == "trim_canvas" }.count, 2)
        }
    }

    func testCanvasBackgroundControlsCommitRestoreAndCopyTransparentPixels() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let fixture = try makeHistoryFixture()
            defer { try? FileManager.default.removeItem(at: fixture.root) }
            let worker = EditorWorker()
            var copied: Data?
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                worker: worker, writeClipboard: { copied = $0; return true })
            defer { controller.window.orderOut(nil); worker.close(); EditorWorker.flush() }
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot != nil && !controller.state.busy }
            (try field("Canvas width", in: controller.root)).stringValue = "10"
            (try field("Canvas height", in: controller.root)).stringValue = "5"
            try button("Resize canvas", in: controller.root).performClick(nil)
            waitUntil { controller.state.snapshot?.width == 10 && !controller.state.busy }
            let layers = controller.state.snapshot?.layers
            try showOutput(in: controller.root)
            try button("Preview output", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy }
            let output = try segmented("Output preview image", in: controller.root)
            XCTAssertTrue(output.isEnabled)
            let section = try segmented("Editor section", in: controller.root)
            section.selectedSegment = 0; _ = section.sendAction(section.action, to: section.target)
            let mode = try popup("Canvas background mode", in: controller.root)
            let color = try field("Canvas background color", in: controller.root)
            let apply = try button("Apply background", in: controller.root)
            let scroll = try XCTUnwrap(color.enclosingScrollView)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: 226))
            scroll.reflectScrolledClipView(scroll.contentView)
            color.stringValue = "#21436580"
            XCTAssertEqual(controller.state.snapshot?.background, "#f7f7f5", "fields are not edits")
            apply.performClick(nil)
            XCTAssertFalse(apply.isEnabled); XCTAssertFalse(mode.isEnabled)
            waitUntil { !controller.state.busy }
            XCTAssertEqual(controller.state.snapshot?.background, "#21436580")
            XCTAssertFalse(output.isEnabled, "background commits invalidate encoded output")
            XCTAssertEqual(controller.state.snapshot?.layers, layers)
            try render(controller.root, name: "screenshot-editor-background-solid-\(appearance)")

            color.stringValue = "invalid"; apply.performClick(nil)
            waitUntil { !controller.state.busy }
            XCTAssertEqual(controller.state.snapshot?.background, "#21436580")
            XCTAssertEqual(color.stringValue, "#21436580")
            XCTAssertTrue(labels(in: controller.root).contains { $0.contains("Editor action failed") })
            try render(controller.root, name: "screenshot-editor-background-error-minimum-\(appearance)")
            mode.selectItem(withTitle: "Transparent"); _ = mode.sendAction(mode.action, to: mode.target)
            XCTAssertFalse(color.isEnabled)
            color.stringValue = "invalid-but-disabled"; apply.performClick(nil)
            waitUntil { !controller.state.busy }
            XCTAssertNil(controller.state.snapshot?.background)
            XCTAssertEqual(color.stringValue, "#21436580", "remember the last accepted solid color")
            try render(controller.root, name: "screenshot-editor-background-transparent-\(appearance)")
            try button("Undo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy }
            XCTAssertEqual(mode.titleOfSelectedItem, "Solid")
            XCTAssertEqual(controller.state.snapshot?.background, "#21436580")
            try button("Redo", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy }
            XCTAssertEqual(mode.titleOfSelectedItem, "Transparent")
            try showOutput(in: controller.root)
            try button("Copy image", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && copied != nil }
            let bitmap = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(copied)))
            XCTAssertEqual(bitmap.pixelsWide, 10); XCTAssertEqual(bitmap.pixelsHigh, 5)
            XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 9, y: 4)).alphaComponent, 0)
            XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 2, y: 1)).alphaComponent, 1)
            XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.drafts.path))
            try button("Save draft", in: controller.root).performClick(nil)
            waitUntil { !controller.state.busy && controller.state.snapshot?.hasDraft == true }
            _ = controller.windowShouldClose(controller.window)
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot != nil && !controller.state.busy }
            XCTAssertNil(controller.state.snapshot?.background)
            XCTAssertEqual(controller.state.snapshot?.layers, layers)
        }
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
        // Copy is available while editing geometry, without switching to Output.
        try button("Copy image", in: controller.root).performClick(nil)
        waitUntil { !controller.state.busy && pasteboard.data(forType: .png) != nil }
        let png = try XCTUnwrap(pasteboard.data(forType: .png))
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: png))
        XCTAssertEqual(bitmap.pixelsWide, 4); XCTAssertEqual(bitmap.pixelsHigh, 2)
        // Check encoded samples, not AppKit's display-profile conversion of an
        // untagged PNG. The independent fixture's source pixel (2,1) is RGBA.
        var pixel = [UInt](repeating: 0, count: bitmap.samplesPerPixel)
        bitmap.getPixel(&pixel, atX: 0, y: 0)
        XCTAssertEqual(pixel, [62, 71, 19, 255])
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
        try showDraw(in: controller.root)
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
        let created = shapeLayer(id: "fresh-shape", x: 142.222, y: -99.556)
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
        controller.window.setContentSize(NSSize(width: 1000, height: 600))
        XCTAssertEqual(controller.root.bounds.size, NSSize(width: 1000, height: 600))
        try showOutput(in: controller.root)
        try button("Preview output", in: controller.root).performClick(nil)
        let outputMode = try segmented("Output preview image", in: controller.root)
        XCTAssertEqual(outputMode.selectedSegment, 1)
        try showDraw(in: controller.root)

        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(at: 1); _ = tool.sendAction(tool.action, to: tool.target)
        let overlay = controller.drawOverlay
        XCTAssertEqual(overlay.presentedImageRect,
                       NSRect(x: 0, y: 72, width: 540, height: 270))
        overlay.begin(at: NSPoint(x: 500, y: 300))
        overlay.drag(to: NSPoint(x: 60, y: 30))
        XCTAssertTrue(worker.requests.isEmpty, "transient drawing never mutates the document")
        overlay.end(at: NSPoint(x: 60, y: 30))

        let request = try XCTUnwrap(worker.requests.last)
        XCTAssertEqual(request["operation"] as? String, "create_closed_shape")
        XCTAssertEqual(request["shape"] as? String, "ellipse")
        let start = try XCTUnwrap(request["start"] as? [String: CGFloat])
        let end = try XCTUnwrap(request["end"] as? [String: CGFloat])
        XCTAssertEqual(try XCTUnwrap(start["x"]), 1_185.185, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(start["y"]), 540.444, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(end["x"]), 142.222, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(end["y"]), -99.556, accuracy: 0.001,
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

    func testPolygonToolsUseSharedVerticesAndCommitOnlyOnRelease() throws {
        _ = NSApplication.shared
        for (title, count) in [("Triangle", 3), ("Diamond", 4), ("Star", 10)] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", width: 1280, height: 640))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            controller.window.setContentSize(NSSize(width: 1000, height: 600))
            XCTAssertEqual(controller.root.bounds.size, NSSize(width: 1000, height: 600))
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(withTitle: title); _ = tool.sendAction(tool.action, to: tool.target)
            let overlay = controller.drawOverlay
            let geometry = try XCTUnwrap(NativeEditorDrawGeometry(
                kind: try XCTUnwrap(overlay.shape.polygonGeometryKind),
                samples: [CGPoint(x: 90, y: 70), CGPoint(x: 10, y: 20)]))
            XCTAssertEqual(geometry.points.count, count)
            XCTAssertEqual(geometry.points[0].x, 50, accuracy: 0.00001)
            XCTAssertEqual(geometry.points[0].y, 20, accuracy: 0.00001)
            overlay.begin(at: NSPoint(x: 500, y: 300)); overlay.drag(to: NSPoint(x: 60, y: 30))
            XCTAssertTrue(worker.requests.isEmpty)
            overlay.end(at: NSPoint(x: 60, y: 30))
            XCTAssertEqual(worker.requests.count, 1)
            let request = try XCTUnwrap(worker.requests.last)
            XCTAssertEqual(request["operation"] as? String, "create_closed_shape")
            XCTAssertEqual(request["shape"] as? String, title.lowercased())
            let end = try XCTUnwrap(request["end"] as? [String: CGFloat])
            XCTAssertEqual(try XCTUnwrap(end["y"]), -99.556, accuracy: 0.001)
            overlay.begin(at: NSPoint(x: 300, y: 200)); overlay.end(at: NSPoint(x: 300, y: 350))
            overlay.begin(at: NSPoint(x: 300, y: 200)); overlay.end(at: NSPoint(x: 400, y: 200))
            overlay.begin(at: NSPoint(x: 200, y: 200)); overlay.drag(to: NSPoint(x: 260, y: 260))
            overlay.keyDown(with: try keyEvent(window: controller.window, keyCode: 53, characters: "\u{1b}"))
            overlay.end(at: NSPoint(x: 260, y: 260))
            overlay.begin(at: NSPoint(x: 200, y: 200))
            tool.selectItem(withTitle: "Rectangle"); _ = tool.sendAction(tool.action, to: tool.target)
            overlay.end(at: NSPoint(x: 260, y: 260))
            XCTAssertEqual(worker.requests.count, 1, "degenerate and cancelled gestures do not commit")
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
            let cropLabel = try XCTUnwrap(descendants(in: geometry).compactMap { $0 as? NSTextField }
                .first { $0.stringValue == "X" })
            let cropField = try field("Crop X", in: geometry)
            XCTAssertNotNil(cropField.enclosingScrollView)
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
            let preset = try popup("Output compression preset", in: controller.root)
            XCTAssertTrue(preset.isHidden)
            try render(controller.root, name: "screenshot-editor-output-normal-\(appearance)")

            let size = try popup("Output size", in: controller.root)
            size.selectItem(withTitle: "Custom"); _ = size.sendAction(size.action, to: size.target)
            let width = try field("Custom output width", in: controller.root)
            width.stringValue = "420"
            controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: width))
            try render(controller.root, name: "screenshot-editor-output-size-custom-\(appearance)")

            let quality = try popup("Output quality mode", in: controller.root)
            quality.selectItem(withTitle: "Compress")
            _ = quality.sendAction(quality.action, to: quality.target)
            XCTAssertFalse(preset.isHidden)
            XCTAssertTrue(controller.root.bounds.contains(preset.convert(preset.bounds, to: controller.root)))
            try button("Preview output", in: controller.root).performClick(nil)
            try render(controller.root, name: "screenshot-editor-output-preview-\(appearance)")

            quality.selectItem(withTitle: "Maximum file size")
            _ = quality.sendAction(quality.action, to: quality.target)
            XCTAssertTrue(preset.isHidden)
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

    func testPolygonRenderedStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", width: 960, height: 540))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            for title in ["Triangle", "Diamond", "Star"] {
                tool.selectItem(withTitle: title); _ = tool.sendAction(tool.action, to: tool.target)
                controller.drawOverlay.begin(at: NSPoint(x: 500, y: 390))
                controller.drawOverlay.drag(to: NSPoint(x: 90, y: 125))
                try render(controller.root, name: "screenshot-editor-polygon-\(title.lowercased())-\(appearance)")
                controller.drawOverlay.cancelGesture()
            }
            worker.failOperation = "create_closed_shape"
            worker.failureMessage = "The star could not be created. The previous draft, selection and undo history remain recoverable."
            controller.drawOverlay.begin(at: NSPoint(x: 500, y: 390))
            controller.drawOverlay.end(at: NSPoint(x: 90, y: 125))
            try render(controller.root, name: "screenshot-editor-polygon-error-minimum-\(appearance)")
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
        let resized = expectation(description: "encode asymmetric custom output")
        worker.encode(["format": "png", "quality": "preserve", "quality_value": 100, "png": [:],
                       "size": ["mode": "custom", "width": 5, "height": 2]]) { result in
            let output = try? result.get()
            XCTAssertEqual(output?.image.width, 5); XCTAssertEqual(output?.image.height, 2)
            resized.fulfill()
        }
        wait(for: [resized], timeout: 5)
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
                        "png": [:], "size": ["mode": "custom", "width": 4, "height": 6]],
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
        XCTAssertEqual(metadata?["width"] as? Int, 4); XCTAssertEqual(metadata?["height"] as? Int, 6)
        let savedSource = try XCTUnwrap(CGImageSourceCreateWithURL(output as CFURL, nil))
        let savedImage = try XCTUnwrap(CGImageSourceCreateImageAtIndex(savedSource, 0, nil))
        XCTAssertEqual(savedImage.width, 4); XCTAssertEqual(savedImage.height, 6)

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

        overlay.rotationSnapDegrees = 37
        overlay.begin(at: grip.handle); overlay.drag(to: current)
        overlay.flagsChanged(with: try flags(.shift))
        XCTAssertEqual(try XCTUnwrap(overlay.rotationPreview?.radians), 37 * Double.pi / 180, accuracy: 1e-12)
        overlay.end(at: current, snap: true)
        XCTAssertEqual(rotations[2].1, 37 * Double.pi / 180, accuracy: 1e-12)
        overlay.begin(at: grip.handle); overlay.drag(to: current, snap: true)
        overlay.rotationSnapDegrees = 45
        overlay.end(at: current, snap: true)
        XCTAssertEqual(rotations.count, 3, "changing the increment cancels an active gesture")
    }

    func testRotationSnapControlsDoNotEditDocumentOrInvalidateOutput() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [
                layer(id: "image", name: "Image", x: 10, y: 20, visible: true, locked: false, opacity: 100),
            ]))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!,
                worker: worker, numberLocale: Locale(identifier: "fr_FR"))
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showOutput(in: controller.root)
            try button("Preview output", in: controller.root).performClick(nil)
            let outputMode = try segmented("Output preview image", in: controller.root)
            let snapshot = controller.state.snapshot
            try showLayers(in: controller.root)
            let field = try field("Shift rotation snap", in: controller.root)
            XCTAssertEqual(field.stringValue, "15")
            for (input, expected) in [("0", 1.0), ("181", 180.0), ("37,5", 38.0), ("invalid", 38.0)] {
                field.stringValue = input
                _ = field.sendAction(field.action, to: field.target)
                XCTAssertEqual(controller.selectionOverlay.rotationSnapDegrees, expected)
            }
            field.stringValue = "37"
            controller.controlTextDidEndEditing(Notification(name: NSControl.textDidEndEditingNotification, object: field))
            XCTAssertEqual(controller.selectionOverlay.rotationSnapDegrees, 37)
            XCTAssertTrue(worker.requests.isEmpty)
            XCTAssertEqual(controller.state.snapshot, snapshot)
            XCTAssertEqual(outputMode.selectedSegment, 1)
            XCTAssertTrue(outputMode.isEnabled)
            field.scrollToVisible(field.bounds)
            try render(controller.root, name: "screenshot-editor-rotation-snap-\(appearance)")
        }
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
            "initial_text_size": 39,
            "selection_outlines": ["rotated": [
                ["x": 12, "y": 4], ["x": 26, "y": 18], ["x": 12, "y": 32], ["x": -2, "y": 18],
            ]],
            "can_undo": false, "can_redo": false, "unsaved_changes": false, "has_draft": false,
        ]
        let parsed = try XCTUnwrap(NativeEditorSnapshot(value))
        XCTAssertEqual(parsed.initialTextSize, 39, "capture size must not be inferred from draft dimensions")
        XCTAssertEqual(parsed.layers[0].selectionOutline,
                       [CGPoint(x: 12, y: 4), CGPoint(x: 26, y: 18),
                        CGPoint(x: 12, y: 32), CGPoint(x: -2, y: 18)])
        XCTAssertEqual(parsed.layers[0].rotation, Double.pi / 3)
        XCTAssertTrue(parsed.documentJSON.hasPrefix("{\"elements\""), "document JSON uses sorted keys")
        XCTAssertNil(snapshot(id: "old", layers: [element]).layers[0].selectionOutline)
    }

    func testSnapshotPasteCapabilityDefaultsAndRejectsMalformedValue() throws {
        XCTAssertFalse(snapshot(id: "legacy").canPasteLayer)
        var value: [String: Any] = [
            "artifact_id": "shot", "document": ["width": 200, "height": 100, "elements": []],
            "initial_text_size": 24, "can_undo": false, "can_redo": false,
            "unsaved_changes": false, "has_draft": false, "can_paste_layer": true,
        ]
        XCTAssertTrue(try XCTUnwrap(NativeEditorSnapshot(value)).canPasteLayer)
        value["can_paste_layer"] = "true"
        XCTAssertNil(NativeEditorSnapshot(value))
    }

    func testSnapshotParsesAndValidatesActiveTextInputToken() throws {
        var value: [String: Any] = [
            "artifact_id": "shot", "document": ["width": 200, "height": 100,
                "elements": [textLayer(id: "preview", text: "pending")]],
            "initial_text_size": 24, "can_undo": false, "can_redo": false,
            "unsaved_changes": false, "has_draft": false,
            "active_text_input": ["input_id": "host-token", "layer_id": "preview", "is_new": true],
        ]
        let parsed = try XCTUnwrap(NativeEditorSnapshot(value))
        XCTAssertEqual(parsed.activeTextInput,
                       NativeActiveTextInput(["input_id": "host-token", "layer_id": "preview",
                                              "is_new": true]))
        value["active_text_input"] = ["input_id": "", "layer_id": "preview", "is_new": true]
        XCTAssertNil(NativeEditorSnapshot(value))
        value["active_text_input"] = "host-token"
        XCTAssertNil(NativeEditorSnapshot(value))
    }

    func testLayerCopyPasteShortcutsDispatchAndPreserveOrInvalidateOutput() throws {
        _ = NSApplication.shared
        let original = layer(id: "source", name: "Source", x: 10, y: 20,
                             visible: true, locked: false, opacity: 100)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)
        let layers = try table("Screenshot layers", in: controller.root)
        layers.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        controller.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification,
                                                              object: layers))
        try showOutput(in: controller.root); try button("Preview output", in: controller.root).performClick(nil)
        let output = try segmented("Output preview image", in: controller.root)
        XCTAssertEqual(output.selectedSegment, 1)

        _ = controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 9, characters: "v", modifiers: .command))
        XCTAssertTrue(worker.requests.isEmpty, "paste is rejected until the shared snapshot enables it")

        worker.response = { request in
            guard request["operation"] as? String == "copy_layer" else { return nil }
            return self.snapshot(id: "shot", layers: [original], canPaste: true)
        }
        XCTAssertTrue(controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 8, characters: "c", modifiers: .command)))
        XCTAssertEqual(worker.requests.last?["operation"] as? String, "copy_layer")
        XCTAssertEqual(worker.requests.last?["id"] as? String, "source")
        XCTAssertTrue(controller.state.snapshot?.canPasteLayer == true)
        XCTAssertEqual(output.selectedSegment, 1); XCTAssertTrue(output.isEnabled,
            "copy changes capability only and keeps encoded pixels available")

        try showDraw(in: controller.root)
        worker.response = { request in
            guard request["operation"] as? String == "paste_layer",
                  let id = request["new_id"] as? String else { return nil }
            let pasted = self.layer(id: id, name: "Source copy", x: 34, y: 44,
                                    visible: true, locked: false, opacity: 100)
            return self.snapshot(id: "shot", unsaved: true, layers: [original, pasted], canPaste: true)
        }
        XCTAssertTrue(controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 9, characters: "v", modifiers: .control)))
        let paste = try XCTUnwrap(worker.requests.last)
        XCTAssertEqual(paste["operation"] as? String, "paste_layer")
        XCTAssertEqual(paste["after_id"] as? String, "source")
        let newID = try XCTUnwrap(paste["new_id"] as? String)
        XCTAssertNotEqual(newID, "source")
        XCTAssertEqual(controller.state.snapshot?.layers.first?.id, newID)
        XCTAssertEqual(try segmented("Editor section", in: controller.root).selectedSegment, 1,
                       "accepted paste returns to Select & move")
        XCTAssertFalse(output.isEnabled, "paste invalidates stale encoded output")
    }

    func testLayerShortcutsRespectFocusedControlPendingRejectedAndClosedStates() throws {
        _ = NSApplication.shared
        let source = layer(id: "source", name: "Source", x: 10, y: 20,
                           visible: true, locked: false, opacity: 100)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [source], canPaste: true))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showLayers(in: controller.root)
        let layers = try table("Screenshot layers", in: controller.root)
        layers.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        controller.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification,
                                                              object: layers))
        let focused = try field("Layer name", in: controller.root)
        controller.window.makeFirstResponder(focused)
        XCTAssertFalse(controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 8, characters: "c", modifiers: .command)))
        XCTAssertTrue(worker.requests.isEmpty, "native controls retain their own copy and paste")
        controller.window.makeFirstResponder(nil)

        worker.deferRequests = true
        _ = controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 9, characters: "v", modifiers: .command))
        _ = controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 9, characters: "v", modifiers: .command))
        XCTAssertEqual(worker.requests.count, 1, "one in-flight command rejects another shortcut")
        worker.completePending(with: snapshot(id: "shot", layers: [source], canPaste: true))
        worker.deferRequests = false; worker.failOperation = "paste_layer"
        try showDraw(in: controller.root)
        _ = controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 9, characters: "v", modifiers: .command))
        XCTAssertEqual(try segmented("Editor section", in: controller.root).selectedSegment, 2,
                       "rejected paste does not transition tools")
        controller.window.performClose(nil)
        let count = worker.requests.count
        _ = controller.window.performKeyEquivalent(with: try keyEvent(
            window: controller.window, keyCode: 9, characters: "v", modifiers: .command))
        XCTAssertEqual(worker.requests.count, count, "closed sessions reject shortcuts")
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

    func testTextClickCreatesAtCanvasPointAndSelectsFreshID() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        worker.response = { request in
            guard request["operation"] as? String == "begin_text_input",
                  let inputID = request["input_id"] as? String else { return nil }
            return self.snapshot(id: "shot", layers: [self.textLayer(id: "fresh-text", text: "")],
                activeTextInput: ["input_id": inputID, "layer_id": "fresh-text", "is_new": true])
        }
        try showOutput(in: controller.root); try button("Preview output", in: controller.root).performClick(nil)
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let image = controller.presentedImageRect
        let click = NSPoint(x: image.minX + image.width * 0.25, y: image.minY + image.height * 0.75)
        controller.drawOverlay.begin(at: click); controller.drawOverlay.end(at: click)
        let request = try XCTUnwrap(worker.requests.last)
        XCTAssertEqual(request["operation"] as? String, "begin_text_input")
        XCTAssertFalse((request["input_id"] as? String)?.isEmpty ?? true)
        let target = try XCTUnwrap(request["target"] as? [String: Any])
        XCTAssertEqual(target["kind"] as? String, "new")
        let create = try XCTUnwrap(target["create"] as? [String: Any])
        let point = try XCTUnwrap(create["point"] as? [String: CGFloat])
        XCTAssertEqual(try XCTUnwrap(point["x"]), 160, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(point["y"]), 270, accuracy: 0.001)
        XCTAssertEqual(create["text"] as? String, "")
        XCTAssertEqual(create["fontSize"] as? Double, 24)
        XCTAssertEqual(create["color"] as? String, "#ff3b5c")
        XCTAssertNil(create["stylePreset"], "drafts without named presets keep the plain family request")
        XCTAssertEqual(controller.state.snapshot?.layers.first?.id, "fresh-text")
        XCTAssertFalse(try segmented("Output preview image", in: controller.root).isEnabled,
                       "output controls stay blocked while composition is unresolved")
        XCTAssertEqual(try textView("Inline screenshot text", in: controller.root).string, "")
        let inlineEditor = try textView("Inline screenshot text", in: controller.root)
        XCTAssertTrue(controller.window.firstResponder === inlineEditor)
    }

    func testNewTextDefaultsUsePinnedPresetAndRetainInputAcrossFailureAndSnapshots() throws {
        _ = NSApplication.shared
        let fonts = ["sans": "Liberation Sans", "mono": "Liberation Mono"]
        let initial = snapshot(id: "shot", initialTextSize: 39, fonts: fonts)
        let worker = FakeEditorWorker(snapshot: initial)
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker,
                                                    numberLocale: Locale(identifier: "fr_FR"))
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root); try button("Preview output", in: controller.root).performClick(nil)
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let preset = try popup("New text style", in: controller.root)
        XCTAssertEqual(preset.itemTitles, ["Plain", "Standard", "Outlined", "Mono", "Box", "Mono box"])
        XCTAssertEqual(preset.titleOfSelectedItem, "Standard")
        preset.selectItem(withTitle: "Mono box")
        let size = try field("New text size", in: controller.root)
        let color = try field("New text color", in: controller.root)
        XCTAssertEqual(size.stringValue, "39", "use shared capture size, not the restored canvas")
        size.stringValue = "48,5"; color.stringValue = "#12abef"
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertTrue(try segmented("Output preview image", in: controller.root).isEnabled,
                      "staging creation defaults must not invalidate encoded output")

        worker.failOperation = "begin_text_input"
        let click = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: click); controller.drawOverlay.end(at: click)
        var create = try XCTUnwrap((worker.requests.last?["target"] as? [String: Any])?["create"] as? [String: Any])
        XCTAssertEqual(create["stylePreset"] as? String, "mono-box")
        XCTAssertEqual(create["fontSize"] as? Double, 48.5)
        XCTAssertEqual(create["color"] as? String, "#12abef")
        XCTAssertEqual(preset.titleOfSelectedItem, "Mono box")
        XCTAssertEqual(size.stringValue, "48,5"); XCTAssertEqual(color.stringValue, "#12abef")

        worker.failOperation = nil
        var inputID = ""
        worker.response = { request in
            switch request["operation"] as? String {
            case "begin_text_input":
                inputID = request["input_id"] as! String
                return self.snapshot(id: "shot",
                    layers: [self.textLayer(id: "fresh-default-text", text: "")], fonts: fonts,
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh-default-text", "is_new": true])
            case "finish_text_input":
                return self.snapshot(id: "shot", unsaved: true,
                    layers: [self.textLayer(id: "fresh-default-text", text: "")], fonts: fonts)
            default: return nil
            }
        }
        try button("Done", in: controller.root).performClick(nil)
        let retriedBegin = worker.requests.last { $0["operation"] as? String == "begin_text_input" }
        create = try XCTUnwrap((retriedBegin?["target"] as? [String: Any])?["create"] as? [String: Any])
        XCTAssertEqual(create["stylePreset"] as? String, "mono-box")
        XCTAssertEqual(controller.state.snapshot?.layers.first?.id, "fresh-default-text")
        XCTAssertEqual(preset.titleOfSelectedItem, "Mono box", "accepted snapshots must retain creation defaults")
        XCTAssertEqual(size.stringValue, "48,5"); XCTAssertEqual(color.stringValue, "#12abef")

        let freshWorker = FakeEditorWorker(snapshot: snapshot(id: "fresh", fonts: fonts))
        let fresh = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: freshWorker)
        defer { fresh.window.orderOut(nil) }
        fresh.present(artifact: artifact(id: "fresh"), historyRoot: "/native/History")
        XCTAssertEqual(try popup("New text style", in: fresh.root).titleOfSelectedItem, "Standard")
        XCTAssertEqual(try field("New text size", in: fresh.root).stringValue, "24")
        XCTAssertEqual(try field("New text color", in: fresh.root).stringValue, "#ff3b5c")
    }

    func testRoundedBoxCreationDefaultRequiresOfferedFontAndRetainsUserChoice() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let fonts = ["sans": "Liberation Sans", "rounded": "Nunito"]
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", initialTextSize: 39, fonts: fonts))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
            let preset = try popup("New text style", in: controller.root)
            XCTAssertEqual(preset.itemTitles, ["Plain", "Standard", "Rounded", "Outlined", "Box", "Rounded box"])
            XCTAssertEqual(preset.titleOfSelectedItem, "Rounded box")
            let size = try field("New text size", in: controller.root)
            let color = try field("New text color", in: controller.root)
            XCTAssertEqual(size.stringValue, "39")
            XCTAssertEqual(color.stringValue, "#ff3b5c")
            controller.window.setContentSize(NSSize(width: 1200, height: 820))
            try render(controller.root, name: "screenshot-editor-text-default-rounded-normal-\(appearance)")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            color.scrollToVisible(color.bounds)
            controller.root.layoutSubtreeIfNeeded()
            let scroll = try XCTUnwrap(color.enclosingScrollView)
            for control in [preset, size, color] {
                XCTAssertTrue(scroll.contentView.bounds.contains(control.convert(control.bounds, to: scroll.contentView)),
                              "New text defaults must be reachable at minimum size")
            }
            try render(controller.root, name: "screenshot-editor-text-default-rounded-minimum-\(appearance)")

            worker.failOperation = "begin_text_input"
            let click = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
            controller.drawOverlay.begin(at: click); controller.drawOverlay.end(at: click)
            let first = try XCTUnwrap((worker.requests.last?["target"] as? [String: Any])?["create"] as? [String: Any])
            XCTAssertEqual(first["stylePreset"] as? String, "rounded-box")
            XCTAssertEqual(first["fontSize"] as? Double, 39)
            XCTAssertEqual(first["color"] as? String, "#ff3b5c")
            XCTAssertEqual(preset.titleOfSelectedItem, "Rounded box", "failure must retain the creation default")

            preset.selectItem(withTitle: "Standard")
            size.stringValue = "52"; color.stringValue = "#12abef"
            worker.failOperation = nil
            worker.response = { request in
                switch request["operation"] as? String {
                case "begin_text_input":
                    return self.snapshot(id: "shot", layers: [self.textLayer(id: "new", text: "")],
                        fonts: fonts,
                        activeTextInput: ["input_id": request["input_id"] as! String,
                                          "layer_id": "new", "is_new": true])
                case "finish_text_input": return self.snapshot(id: "shot", unsaved: true,
                    layers: [self.textLayer(id: "new", text: "")], fonts: fonts)
                default: return nil
                }
            }
            try button("Done", in: controller.root).performClick(nil)
            let retry = try XCTUnwrap(worker.requests.last { $0["operation"] as? String == "begin_text_input" })
            let create = try XCTUnwrap((retry["target"] as? [String: Any])?["create"] as? [String: Any])
            XCTAssertEqual(create["stylePreset"] as? String, "rounded-box",
                           "retry keeps the pending creation target, not later picker changes")
            XCTAssertEqual(create["fontSize"] as? Double, 39)
            XCTAssertEqual(create["color"] as? String, "#ff3b5c")
            XCTAssertEqual(preset.titleOfSelectedItem, "Standard", "accepted snapshot must not reset user choice")
            XCTAssertEqual(size.stringValue, "52")
            XCTAssertEqual(color.stringValue, "#12abef")
        }

        let sans = ["sans": "Liberation Sans"]
        for includeStandard in [true, false] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "old", fonts: sans,
                includeStandardPreset: includeStandard))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "old"), historyRoot: "/native/History")
            let preset = try popup("New text style", in: controller.root)
            XCTAssertFalse(preset.itemTitles.contains("Rounded box"), "missing rounded font is not substituted")
            XCTAssertEqual(preset.titleOfSelectedItem, includeStandard ? "Standard" : "Plain")
        }
    }

    func testInlineTextCoalescesRapidUnicodeTypingAndFlushesBeforeOneCommit() throws {
        _ = NSApplication.shared
        let fresh = textLayer(id: "fresh", text: "")
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        var inputID = ""
        worker.response = { request in
            switch request["operation"] as? String {
            case "begin_text_input":
                inputID = request["input_id"] as! String
                return self.snapshot(id: "shot", layers: [fresh],
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
            case "finish_text_input":
                return self.snapshot(id: "shot", unsaved: true,
                    layers: [self.textLayer(id: "fresh", text: "Ω\n漢字🙂")])
            default: return nil
            }
        }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root); try button("Preview output", in: controller.root).performClick(nil)
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        let editor = try textView("Inline screenshot text", in: controller.root)
        XCTAssertTrue(controller.window.firstResponder === editor)

        worker.deferRequests = true
        editor.string = "Ω"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        editor.string = "Ω\n漢"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        editor.string = "Ω\n漢字🙂"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        XCTAssertEqual(worker.requests.filter { $0["operation"] as? String == "update_text_input" }.count, 1,
                       "typing remains local while one preview is in flight")
        XCTAssertTrue(editor.isEditable)

        worker.completePending(with: snapshot(id: "shot", layers: [textLayer(id: "fresh", text: "Ω")],
            activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true]))
        let updates = worker.requests.filter { $0["operation"] as? String == "update_text_input" }
        XCTAssertEqual(updates.count, 2)
        XCTAssertEqual(updates.last?["text"] as? String, "Ω\n漢字🙂",
                       "only the newest buffered replacement follows the accepted preview")
        worker.deferRequests = false
        worker.completePending(with: snapshot(id: "shot", layers: [textLayer(id: "fresh", text: "Ω\n漢字🙂")],
            activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true]))

        worker.deferRequests = true
        editor.keyDown(with: try keyEvent(window: controller.window, keyCode: 53, characters: "\u{1b}"))
        let finish = try XCTUnwrap(worker.requests.last)
        XCTAssertEqual(finish["operation"] as? String, "finish_text_input")
        XCTAssertEqual(finish["input_id"] as? String, inputID)
        XCTAssertEqual(finish["commit"] as? Bool, true)
        XCTAssertFalse(editor.isEditable)
        XCTAssertFalse(try button("Done", in: controller.root).isEnabled)
        XCTAssertFalse(try button("Cancel", in: controller.root).isEnabled)
        let requestCount = worker.requests.count
        editor.string = "typing after accepted Finish must not win"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.count, requestCount,
                       "typing and Cancel cannot race an accepted Finish request")
        worker.completePending(with: snapshot(id: "shot", unsaved: true,
            layers: [textLayer(id: "fresh", text: "Ω\n漢字🙂")]))
        XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle?.text, "Ω\n漢字🙂")
        XCTAssertEqual(controller.state.snapshot?.canUndo, true,
                       "the shared finish publishes the grouped text transaction")
        XCTAssertFalse(try segmented("Output preview image", in: controller.root).isEnabled,
                       "only accepted commit invalidates encoded output")
    }

    func testInlineTextDelayedBeginPreservesSelectionAndMarkedEscapeStaysNative() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        worker.deferRequests = true
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        let editor = try textView("Inline screenshot text", in: controller.root)
        editor.string = "abcdef"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        editor.setSelectedRange(NSRange(location: 1, length: 3))
        let inputID = try XCTUnwrap(worker.requests.last?["input_id"] as? String)
        worker.completePending(with: snapshot(id: "shot", layers: [textLayer(id: "fresh", text: "")],
            activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true]))
        XCTAssertEqual(editor.selectedRange(), NSRange(location: 1, length: 3),
                       "delayed Begin completion must not reset a live native selection")

        worker.deferRequests = false
        worker.completePending(with: snapshot(id: "shot", layers: [textLayer(id: "fresh", text: "abcdef")],
            activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true]))
        let finishesBeforeMarkedEscape = worker.requests.filter {
            $0["operation"] as? String == "finish_text_input"
        }.count
        editor.setMarkedText("候補", selectedRange: NSRange(location: 2, length: 0),
                             replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertTrue(editor.hasMarkedText())
        editor.keyDown(with: try keyEvent(window: controller.window, keyCode: 53, characters: "\u{1b}"))
        XCTAssertEqual(worker.requests.filter { $0["operation"] as? String == "finish_text_input" }.count,
                       finishesBeforeMarkedEscape,
                       "Escape belongs to the native input context while marked text is active")
        editor.unmarkText()
        try button("Cancel", in: controller.root).performClick(nil)
    }

    func testInlineTextQuitDrainsAcceptedFinishWithoutSendingAStaleToken() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        var inputID = ""
        worker.response = { request in
            switch request["operation"] as? String {
            case "begin_text_input":
                inputID = request["input_id"] as! String
                return self.snapshot(id: "shot", layers: [self.textLayer(id: "fresh", text: "")],
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
            case "update_text_input":
                return self.snapshot(id: "shot",
                    layers: [self.textLayer(id: "fresh", text: request["text"] as! String)],
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
            default: return nil
            }
        }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        let editor = try textView("Inline screenshot text", in: controller.root)
        editor.string = "accepted before quit"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        worker.deferRequests = true
        try button("Done", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["operation"] as? String, "finish_text_input")

        XCTAssertTrue(controller.prepareForTermination())
        XCTAssertNil(worker.terminationTextInputs.last!,
                     "termination drains the accepted Finish instead of reusing its now-stale token")
        worker.completePending(with: snapshot(id: "shot", unsaved: true,
            layers: [textLayer(id: "fresh", text: "accepted before quit")]))
        XCTAssertNil(controller.state.snapshot)
    }

    func testInlineTextQuitPreservesCancelRequestedDuringUpdate() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        var inputID = ""
        worker.response = { request in
            guard request["operation"] as? String == "begin_text_input" else { return nil }
            inputID = request["input_id"] as! String
            return self.snapshot(id: "shot", layers: [self.textLayer(id: "fresh", text: "")],
                activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
        }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        let editor = try textView("Inline screenshot text", in: controller.root)
        worker.deferRequests = true
        editor.string = "preview that must be cancelled"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["operation"] as? String, "update_text_input")

        XCTAssertTrue(controller.prepareForTermination())
        XCTAssertEqual(worker.terminationTextInputs.last!, EditorTerminationTextInput(
            inputID: inputID, text: "preview that must be cancelled", commit: false))
        worker.completePending(with: snapshot(id: "shot",
            layers: [textLayer(id: "fresh", text: "preview that must be cancelled")],
            activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true]))
        XCTAssertNil(controller.state.snapshot)
    }

    func testInlineTextBeginFailureRetainsLocalBufferAndOffersRetryOrCancel() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        worker.deferRequests = true
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        let editor = try textView("Inline screenshot text", in: controller.root)
        editor.string = "typed while Begin renders\nΩ🙂"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        XCTAssertEqual(worker.requests.count, 1)
        XCTAssertTrue(editor.isEditable)

        worker.completePendingFailure("font preview unavailable")
        XCTAssertEqual(editor.string, "typed while Begin renders\nΩ🙂")
        XCTAssertFalse(editor.isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("Retry or Cancel") })
        let done = try button("Done", in: controller.root)
        let cancel = try button("Cancel", in: controller.root)
        XCTAssertTrue(done.isEnabled && cancel.isEnabled)
        cancel.performClick(nil)
        XCTAssertTrue(editor.isHiddenOrHasHiddenAncestor)
        XCTAssertEqual(worker.requests.count, 1, "cancelling a rejected Begin needs no stale shared token")
        XCTAssertFalse(controller.state.snapshot?.unsavedChanges ?? true)
    }

    func testInlineTextCancelPreservesOutputAndTerminationFailureRetainsLatestBuffer() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        var inputID = ""
        worker.response = { request in
            switch request["operation"] as? String {
            case "begin_text_input":
                inputID = request["input_id"] as! String
                return self.snapshot(id: "shot", layers: [self.textLayer(id: "fresh", text: "")],
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
            case "update_text_input":
                return self.snapshot(id: "shot",
                    layers: [self.textLayer(id: "fresh", text: request["text"] as! String)],
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
            case "finish_text_input": return self.snapshot(id: "shot")
            default: return nil
            }
        }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root); try button("Preview output", in: controller.root).performClick(nil)
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        let editor = try textView("Inline screenshot text", in: controller.root)
        editor.string = "latest\n🙂"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
        worker.terminationResult = .failure(AppBridgeError.backend("disk unavailable"))
        XCTAssertFalse(controller.prepareForTermination())
        XCTAssertEqual(worker.terminationTextInputs.last!,
                       EditorTerminationTextInput(inputID: inputID, text: "latest\n🙂", commit: true))
        XCTAssertFalse(editor.isHiddenOrHasHiddenAncestor)
        XCTAssertTrue(controller.window.firstResponder === editor)

        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["operation"] as? String, "finish_text_input")
        XCTAssertEqual(worker.requests.last?["commit"] as? Bool, false)
        XCTAssertTrue(try segmented("Output preview image", in: controller.root).isEnabled,
                      "cancel restores committed pixels without discarding the encoded preview")
        XCTAssertFalse(controller.state.snapshot?.unsavedChanges ?? true,
                       "blank new composition never enters committed state")
    }

    func testInlineTextTerminationAcceptsFinishedInputWhenDraftSaveFailsThenRetriesPersistence() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        var inputID = ""
        worker.response = { request in
            guard request["operation"] as? String == "begin_text_input" else { return nil }
            inputID = request["input_id"] as! String
            return self.snapshot(id: "shot", layers: [self.textLayer(id: "fresh", text: "")],
                activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
        }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        let editor = try textView("Inline screenshot text", in: controller.root)
        editor.string = "accepted before disk failure"
        controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))

        let accepted = snapshot(id: "shot", unsaved: true,
            layers: [textLayer(id: "fresh", text: "accepted before disk failure")])
        worker.terminationResult = .failure(EditorTerminationFailure(
            cause: AppBridgeError.backend("disk unavailable"),
            acceptedPresentation: EditorPresentation(snapshot: accepted,
                image: CGImage.fixture(width: Int(accepted.width), height: Int(accepted.height)))))
        XCTAssertFalse(controller.prepareForTermination())
        XCTAssertEqual(worker.terminationTextInputs.last!,
                       EditorTerminationTextInput(inputID: inputID,
                           text: "accepted before disk failure", commit: true))
        XCTAssertTrue(editor.isHiddenOrHasHiddenAncestor,
                      "the accepted Finish consumes the shared token despite later persistence failure")
        XCTAssertNil(controller.state.snapshot?.activeTextInput)
        XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle?.text,
                       "accepted before disk failure")

        worker.terminationResult = .success(())
        XCTAssertTrue(controller.prepareForTermination())
        XCTAssertNil(worker.terminationTextInputs.last!,
                     "retry saves the accepted state without reusing the consumed input token")
    }

    func testInlineTextRenderedNormalMinimumAndFailureStates() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            var inputID = ""
            worker.response = { request in
                guard request["operation"] as? String == "begin_text_input" else { return nil }
                inputID = request["input_id"] as! String
                return self.snapshot(id: "shot", layers: [self.textLayer(id: "fresh", text: "")],
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
            }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
            let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
            controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
            let editor = try textView("Inline screenshot text", in: controller.root)
            XCTAssertTrue(try button("Cancel", in: controller.root).glass,
                          "the cancel action needs a media-safe surface over arbitrary pixels")
            editor.string = "Native multiline\nΩ and 日本語"
            try render(controller.root, name: "screenshot-editor-inline-text-normal-\(appearance)")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            XCTAssertTrue(controller.presentedImageRect.intersects(editor.enclosingScrollView!.frame))
            XCTAssertGreaterThanOrEqual(editor.enclosingScrollView!.frame.width, 220,
                                        "the minimum composing field retains Unicode input")
            XCTAssertGreaterThanOrEqual(editor.enclosingScrollView!.frame.height, 96,
                                        "the minimum composing field retains multiline input")
            try render(controller.root, name: "screenshot-editor-inline-text-minimum-\(appearance)")
            worker.failOperation = "update_text_input"
            controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
            XCTAssertFalse(editor.isHiddenOrHasHiddenAncestor)
            try render(controller.root, name: "screenshot-editor-inline-text-error-\(appearance)")
        }
    }

    func testInlineTextFramesStayFiniteAndRecoverableWhenPreviewMovesOffscreen() throws {
        _ = NSApplication.shared
        for (width, height) in [(640.0, 360.0), (1.0, 2_000.0)] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", width: width, height: height))
            var inputID = ""
            worker.response = { request in
                guard request["operation"] as? String == "begin_text_input" else { return nil }
                inputID = request["input_id"] as! String
                return self.snapshot(id: "shot", width: width, height: height,
                    layers: [self.textLayer(id: "fresh", text: "")],
                    activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true])
            }
            let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
                                                        worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
            let point = NSPoint(x: controller.presentedImageRect.midX,
                                y: controller.presentedImageRect.midY)
            controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
            let editor = try textView("Inline screenshot text", in: controller.root)
            let scroll = try XCTUnwrap(editor.enclosingScrollView)
            let done = try button("Done", in: controller.root)
            let cancel = try button("Cancel", in: controller.root)
            let viewport = try XCTUnwrap(descendants(in: controller.root)
                .compactMap { $0 as? EditorViewportGestureView }
                .first { $0.accessibilityLabel() == "Screenshot viewport" })
            func finite(_ rect: NSRect) -> Bool {
                [rect.minX, rect.minY, rect.width, rect.height].allSatisfy(\.isFinite)
            }

            viewport.onViewportZoom?(8, NSPoint(x: viewport.bounds.midX, y: viewport.bounds.midY))
            viewport.onViewportPan?(NSPoint(x: viewport.bounds.width * 20,
                                            y: -viewport.bounds.height * 20))
            XCTAssertFalse(controller.presentedImageRect.intersects(viewport.bounds),
                           "the regression requires the shared preview to be wholly offscreen")
            for view in [scroll, done, cancel] {
                XCTAssertTrue(finite(view.frame))
                XCTAssertFalse(view.frame.isNull)
                XCTAssertTrue(viewport.bounds.contains(view.frame),
                              "the native composing controls remain recoverable inside the viewport")
            }
            XCTAssertGreaterThanOrEqual(scroll.frame.width, 220)
            XCTAssertGreaterThanOrEqual(scroll.frame.height, 96)
            XCTAssertTrue(controller.window.firstResponder === editor)

            editor.string = "retained while preview is offscreen"
            worker.deferRequests = true
            controller.textDidChange(Notification(name: NSText.didChangeNotification, object: editor))
            done.performClick(nil)
            worker.completePending(with: snapshot(id: "shot", width: width, height: height,
                layers: [textLayer(id: "fresh", text: editor.string)],
                activeTextInput: ["input_id": inputID, "layer_id": "fresh", "is_new": true]))
            XCTAssertEqual(worker.requests.last?["operation"] as? String, "finish_text_input")
            viewport.onViewportPan?(NSPoint(x: -viewport.bounds.width * 40,
                                            y: viewport.bounds.height * 40))
            for view in [scroll, done, cancel] {
                XCTAssertTrue(finite(view.frame))
                XCTAssertTrue(viewport.bounds.contains(view.frame),
                              "accepted Finish retains finite UI until its callback resolves")
            }
            XCTAssertFalse(editor.isEditable)
        }
    }

    func testTextApplyCancelFailureAndUnsupportedFamilyRetention() throws {
        _ = NSApplication.shared
        var original = textLayer(id: "copy", text: "accepted", family: "draft-custom")
        original["locked"] = true
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let family = try popup("Text font", in: controller.root)
        XCTAssertEqual(family.titleOfSelectedItem, "Saved font: draft-custom")
        XCTAssertFalse(family.isEnabled)
        let editor = try textView("Text content", in: controller.root)
        XCTAssertTrue(editor.isEditable, "locking prevents movement, not property edits")
        editor.string = "pending\nsecond line"
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        XCTAssertFalse(controller.prepareForTermination())
        XCTAssertTrue(worker.requests.isEmpty)
        worker.failOperation = "edit_text"; worker.failureMessage = "missing glyph"
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(editor.string, "pending\nsecond line", "failed Apply retains staged multiline input")
        XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle?.text, "accepted")
        let patch = try XCTUnwrap(worker.requests.last?["patch"] as? [String: Any])
        XCTAssertEqual(Set(patch.keys), ["text"], "unchanged typography must not refit or replace the saved font")
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(editor.string, "accepted"); XCTAssertEqual(worker.requests.count, 1)

        editor.string = "applied"
        worker.failOperation = nil
        try button("Save draft", in: controller.root).performClick(nil)
        XCTAssertEqual(editor.string, "applied", "saving accepted pixels must not erase pending typing")
        worker.response = { request in
            guard request["operation"] as? String == "edit_text" else { return nil }
            return self.snapshot(id: "shot", unsaved: true,
                                 layers: [self.textLayer(id: "copy", text: "applied", family: "draft-custom")])
        }
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(controller.state.snapshot?.layers.first?.id, "copy")
        XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle?.text, "applied")
    }

    func testTextFamilyChangesAreStagedAndUseOnlySessionFonts() throws {
        _ = NSApplication.shared
        let families = ["sans": "Liberation Sans", "serif": "Liberation Serif", "mono": "Liberation Mono"]
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot",
            layers: [textLayer(id: "copy", text: "accepted")], fonts: families))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let family = try popup("Text font", in: controller.root)
        XCTAssertEqual(family.itemTitles, ["Liberation Mono", "Liberation Sans", "Liberation Serif"])
        XCTAssertEqual(family.titleOfSelectedItem, "Liberation Sans")
        family.selectItem(withTitle: "Liberation Serif")
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        XCTAssertTrue(worker.requests.isEmpty, "choosing a font must not commit an edit")
        worker.failOperation = "edit_text"
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(family.titleOfSelectedItem, "Liberation Serif", "failed Apply retains staged font")
        XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle?.fontFamily, "sans")
        let patch = try XCTUnwrap(worker.requests.last?["patch"] as? [String: String])
        XCTAssertEqual(patch, ["fontFamily": "serif"])
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(family.titleOfSelectedItem, "Liberation Sans")
        XCTAssertEqual(worker.requests.count, 1)
        worker.failOperation = nil
        family.selectItem(withTitle: "Liberation Mono")
        worker.deferRequests = true
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertFalse(family.isEnabled, "in-flight text edits freeze the family picker")
        worker.completePending(with: snapshot(id: "shot", unsaved: true,
            layers: [textLayer(id: "copy", text: "accepted", family: "mono")], fonts: families))
        XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle?.fontFamily, "mono")
        XCTAssertEqual(family.titleOfSelectedItem, "Liberation Mono")
        XCTAssertTrue(family.isEnabled)

        let pinned = FakeEditorWorker(snapshot: snapshot(id: "pinned",
            layers: [textLayer(id: "copy", text: "old")], fonts: ["sans": "Liberation Sans"]))
        let old = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: pinned)
        defer { old.window.orderOut(nil) }
        old.present(artifact: artifact(id: "pinned"), historyRoot: "/native/History")
        let oldFamily = try popup("Text font", in: old.root)
        XCTAssertEqual(oldFamily.itemTitles, ["Liberation Sans"])
        XCTAssertFalse(oldFamily.isEnabled, "host defaults must not expand a saved font set")
    }

    func testNamedTextStylesStagePreserveCustomValuesAndCancelAfterFailure() throws {
        _ = NSApplication.shared
        let fonts = ["sans": "Liberation Sans", "serif": "Liberation Serif", "mono": "Liberation Mono"]
        var original = textLayer(id: "copy", text: "keep this", family: "serif")
        original["background"] = "#abcdef"; original["roundedBackground"] = true
        original["dropShadow"] = true; original["bold"] = true; original["align"] = "right"
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original], fonts: fonts))
        worker.failOperation = "edit_text"
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let picker = try popup("Text style preset", in: controller.root)
        XCTAssertEqual(picker.itemTitles, ["Style…", "Standard", "Outlined", "Mono", "Box", "Mono box"])
        func choose(_ name: String) {
            picker.selectItem(withTitle: name); _ = picker.sendAction(picker.action, to: picker.target)
        }
        choose("Mono box")
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertEqual(try field("Text plate color", in: controller.root).stringValue, "#abcdef")
        XCTAssertEqual(try popup("Text font", in: controller.root).titleOfSelectedItem, "Liberation Mono")
        XCTAssertFalse(controller.prepareForTermination())
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["patch"] as? NSDictionary,
                       ["fontFamily": "mono", "roundedBackground": false] as NSDictionary)
        XCTAssertEqual(try popup("Text font", in: controller.root).titleOfSelectedItem, "Liberation Mono")
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(try popup("Text plate", in: controller.root).indexOfSelectedItem, 2)
        choose("Outlined")
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["patch"] as? NSDictionary,
                       ["fontFamily": "sans", "background": NSNull(), "outlined": true,
                        "roundedBackground": false] as NSDictionary)
        choose("Box")
        XCTAssertEqual(try field("Text plate color", in: controller.root).stringValue, "#111318")
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(try field("Text plate color", in: controller.root).stringValue, "#abcdef")
        XCTAssertTrue(controller.prepareForTermination())
    }

    func testExplicitSelectedTextStyleCarriesOnlyPresetToFutureCreation() throws {
        _ = NSApplication.shared
        let fonts = ["sans": "Liberation Sans", "serif": "Liberation Serif", "rounded": "Nunito"]
        let original = textLayer(id: "selected", text: "Existing")
        for appearance in ["light", "dark"] {
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", initialTextSize: 39,
                layers: [original], fonts: fonts))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showDraw(in: controller.root)
            let future = try popup("New text style", in: controller.root)
            let futureSize = try field("New text size", in: controller.root)
            let futureColor = try field("New text color", in: controller.root)
            let selected = try popup("Text style preset", in: controller.root)
            let selectedSize = try field("Text size", in: controller.root)
            let selectedColor = try field("Text color", in: controller.root)
            let family = try popup("Text font", in: controller.root)
            XCTAssertEqual(future.titleOfSelectedItem, "Rounded box",
                           "merely selecting a Standard-styled label must not carry its style")
            XCTAssertEqual(futureSize.stringValue, "39")
            XCTAssertEqual(futureColor.stringValue, "#ff3b5c")
            func choose(_ name: String) {
                selected.selectItem(withTitle: name)
                _ = selected.sendAction(selected.action, to: selected.target)
            }

            choose("Standard")
            XCTAssertEqual(future.titleOfSelectedItem, "Standard",
                           "an explicit same-label choice still changes the future style")
            XCTAssertTrue(worker.requests.isEmpty)
            selectedSize.stringValue = "71"; selectedColor.stringValue = "#21abcd"
            family.selectItem(withTitle: "Liberation Serif")
            XCTAssertEqual(future.titleOfSelectedItem, "Standard")
            XCTAssertEqual(futureSize.stringValue, "39")
            XCTAssertEqual(futureColor.stringValue, "#ff3b5c")
            choose("Box")
            XCTAssertEqual(future.titleOfSelectedItem, "Box")
            XCTAssertEqual(selectedSize.stringValue, "71")
            XCTAssertEqual(selectedColor.stringValue, "#21abcd")
            controller.window.setContentSize(NSSize(width: 1200, height: 820))
            try render(controller.root, name: "screenshot-editor-future-text-style-normal-\(appearance)")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            futureColor.scrollToVisible(futureColor.bounds)
            controller.root.layoutSubtreeIfNeeded()
            try render(controller.root, name: "screenshot-editor-future-text-style-minimum-\(appearance)")

            worker.failOperation = "edit_text"
            try button("Apply", in: controller.root).performClick(nil)
            XCTAssertEqual(worker.requests.last?["operation"] as? String, "edit_text")
            XCTAssertEqual(future.titleOfSelectedItem, "Box")
            XCTAssertEqual(selectedSize.stringValue, "71", "failed Apply retains selected-text staging")
            try button("Cancel", in: controller.root).performClick(nil)
            XCTAssertEqual(selectedSize.stringValue, "32")
            XCTAssertEqual(selectedColor.stringValue, "#111111")
            XCTAssertEqual(future.titleOfSelectedItem, "Box", "Cancel must not roll back future style")

            choose("Outlined")
            worker.failOperation = nil
            worker.response = { request in
                switch request["operation"] as? String {
                case "edit_text":
                    var accepted = original; accepted["outlined"] = true
                    return self.snapshot(id: "shot", unsaved: true, initialTextSize: 39,
                        layers: [accepted], fonts: fonts)
                case "undo":
                    return self.snapshot(id: "shot", initialTextSize: 39,
                        layers: [original], fonts: fonts)
                case "begin_text_input":
                    let inputID = request["input_id"] as! String
                    return self.snapshot(id: "shot", initialTextSize: 39,
                        layers: [original, self.textLayer(id: "new", text: "")], fonts: fonts,
                        activeTextInput: ["input_id": inputID, "layer_id": "new", "is_new": true])
                default: return nil
                }
            }
            try button("Apply", in: controller.root).performClick(nil)
            XCTAssertTrue(controller.state.snapshot?.layers.first?.textStyle?.outlined == true)
            XCTAssertEqual(future.titleOfSelectedItem, "Outlined", "accepted edit must not reset future style")
            try button("Undo", in: controller.root).performClick(nil)
            XCTAssertEqual(future.titleOfSelectedItem, "Outlined", "undo must not restore prior creation defaults")

            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
            let point = NSPoint(x: controller.presentedImageRect.maxX - 20,
                                y: controller.presentedImageRect.midY)
            controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
            let begin = try XCTUnwrap(worker.requests.last)
            XCTAssertEqual(begin["operation"] as? String, "begin_text_input")
            let create = try XCTUnwrap((begin["target"] as? [String: Any])?["create"] as? [String: Any])
            XCTAssertEqual(create["stylePreset"] as? String, "outlined")
            XCTAssertEqual(create["fontSize"] as? Double, 39)
            XCTAssertEqual(create["color"] as? String, "#ff3b5c")
            XCTAssertNil(create["bold"], "selected-label traits do not carry into new text")
            XCTAssertEqual(controller.state.snapshot?.activeTextInput?.layerID, "new")
        }
    }

    func testExplicitCreationPresetWinsAfterSelectedStyleAndNewEditorResets() throws {
        _ = NSApplication.shared
        let fonts = ["sans": "Liberation Sans", "rounded": "Nunito"]
        let original = textLayer(id: "selected", text: "Existing")
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original], fonts: fonts))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let future = try popup("New text style", in: controller.root)
        let selected = try popup("Text style preset", in: controller.root)
        selected.selectItem(withTitle: "Box"); _ = selected.sendAction(selected.action, to: selected.target)
        XCTAssertEqual(future.titleOfSelectedItem, "Box")
        future.selectItem(withTitle: "Plain")
        XCTAssertEqual(future.titleOfSelectedItem, "Plain")
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(future.titleOfSelectedItem, "Plain")
        XCTAssertTrue(worker.requests.isEmpty)

        let freshWorker = FakeEditorWorker(snapshot: snapshot(id: "fresh", fonts: fonts))
        let fresh = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: freshWorker)
        defer { fresh.window.orderOut(nil) }
        fresh.present(artifact: artifact(id: "fresh"), historyRoot: "/native/History")
        XCTAssertEqual(try popup("New text style", in: fresh.root).titleOfSelectedItem, "Rounded box")
    }

    func testTextShadowIsStagedAndOnlyPatchesTheEnabledFlag() throws {
        _ = NSApplication.shared
        let original = textLayer(id: "copy", text: "accepted")
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let shadow = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == "Text drop shadow" })
        XCTAssertEqual(shadow.state, .off, "older drafts omit the enabled flag")
        shadow.performClick(nil)
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertFalse(controller.windowShouldClose(controller.window))
        worker.failOperation = "edit_text"
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["patch"] as? [String: Bool], ["dropShadow": true])
        XCTAssertEqual(shadow.state, .on, "failed Apply preserves staged state")
        XCTAssertFalse(controller.state.snapshot?.layers.first?.textStyle?.dropShadow ?? true)
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(shadow.state, .off)
        shadow.performClick(nil)
        worker.failOperation = nil
        worker.response = { _ in
            var accepted = original; accepted["dropShadow"] = true
            return self.snapshot(id: "shot", unsaved: true, layers: [accepted])
        }
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertTrue(controller.state.snapshot?.layers.first?.textStyle?.dropShadow == true)
        XCTAssertEqual(shadow.state, .on)
        XCTAssertTrue(controller.prepareForTermination(), "successful Apply clears staging")
    }

    func testTextShadowSettingsPreservePrecisionCancelAndIgnoreDisabledInput() throws {
        _ = NSApplication.shared
        var original = textLayer(id: "copy", text: "accepted")
        original["dropShadow"] = true
        let resolved: [String: Any] = ["color": "#123456", "opacity": 61.234567,
                                      "blur": 14.96, "offsetX": -2.25, "offsetY": 6.75]
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original],
                                                        textShadows: ["copy": resolved]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let y = try field("Text shadow y", in: controller.root)
        let blur = try field("Text shadow blur", in: controller.root)
        let shadow = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == "Text drop shadow" })
        let apply = try button("Apply", in: controller.root)
        XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle?.shadowStyle?.opacity, 61.234567)
        XCTAssertEqual(y.stringValue, "6.75")
        y.stringValue = "-12.75"
        XCTAssertFalse(controller.prepareForTermination())
        worker.failOperation = "edit_text"
        apply.performClick(nil)
        XCTAssertEqual(worker.requests.last?["patch"] as? NSDictionary,
                       ["dropShadowStyle": ["offsetY": -12.75]] as NSDictionary)
        XCTAssertEqual(y.stringValue, "-12.75", "failure retains pending settings")
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(y.stringValue, "6.75")
        let count = worker.requests.count
        blur.stringValue = "invalid"
        apply.performClick(nil)
        XCTAssertEqual(worker.requests.count, count, "invalid numbers never enter the worker")
        blur.stringValue = "14.96"
        try field("Text shadow color", in: controller.root).stringValue = "invalid"
        apply.performClick(nil)
        XCTAssertEqual(worker.requests.count, count, "invalid colors never enter the worker")
        shadow.performClick(nil)
        XCTAssertTrue(try XCTUnwrap(blur.superview).isHidden)
        apply.performClick(nil)
        XCTAssertEqual(worker.requests.last?["patch"] as? [String: Bool], ["dropShadow": false])
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(blur.stringValue, "14.96")
        XCTAssertFalse(try XCTUnwrap(blur.superview).isHidden)
        XCTAssertTrue(controller.prepareForTermination())
    }

    func testTextShadowNumbersUseTheDisplayedLocale() throws {
        _ = NSApplication.shared
        var original = textLayer(id: "copy", text: "accepted")
        original["dropShadow"] = true
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!,
            worker: worker, numberLocale: Locale(identifier: "fr_FR"))
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let blur = try field("Text shadow blur", in: controller.root)
        XCTAssertEqual(blur.stringValue, "5,984")
        blur.stringValue = "7,25"
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["patch"] as? NSDictionary,
                       ["dropShadowStyle": ["blur": 7.25]] as NSDictionary)
    }

    func testTextOutlineStagesCancelsAndKeepsFailedInput() throws {
        _ = NSApplication.shared
        let original = textLayer(id: "copy", text: "accepted")
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [original]))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let outline = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == "Text outline" })
        XCTAssertEqual(outline.state, .off)
        outline.performClick(nil)
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertFalse(controller.prepareForTermination())
        worker.failOperation = "edit_text"
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertEqual(worker.requests.last?["patch"] as? [String: Bool], ["outlined": true])
        XCTAssertEqual(outline.state, .on)
        XCTAssertFalse(controller.state.snapshot?.layers.first?.textStyle?.outlined ?? true)
        try button("Cancel", in: controller.root).performClick(nil)
        XCTAssertEqual(outline.state, .off)
        outline.performClick(nil)
        worker.failOperation = nil
        worker.response = { _ in
            var accepted = original; accepted["outlined"] = true
            return self.snapshot(id: "shot", unsaved: true, layers: [accepted])
        }
        try button("Apply", in: controller.root).performClick(nil)
        XCTAssertTrue(controller.state.snapshot?.layers.first?.textStyle?.outlined == true)
        XCTAssertEqual(outline.state, .on)
        XCTAssertTrue(controller.prepareForTermination())
    }

    func testTextControlsRenderedAtNormalAndMinimumSizes() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            var label = textLayer(id: "copy", text: "First line\nSecond line", family: "serif")
            label["dropShadow"] = true
            label["outlined"] = true
            let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", unsaved: true,
                layers: [label],
                fonts: ["sans": "Liberation Sans", "serif": "Liberation Serif", "mono": "Liberation Mono"]))
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: worker)
            defer { controller.window.orderOut(nil) }
            controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
            try showDraw(in: controller.root)
            let tool = try popup("Drawing tool", in: controller.root)
            tool.selectItem(withTitle: "Text"); _ = tool.sendAction(tool.action, to: tool.target)
            let editor = try textView("Text content", in: controller.root)
            let inputScroll = try XCTUnwrap(editor.enclosingScrollView)
            let creationColor = try field("New text color", in: controller.root)
            XCTAssertGreaterThan(inputScroll.frame.minY, creationColor.frame.maxY,
                                 "the selected-text inspector must not overlap creation defaults")
            let apply = try button("Apply", in: controller.root)
            let cancel = try button("Cancel", in: controller.root)
            let scroll = try XCTUnwrap(apply.enclosingScrollView)
            let document = try XCTUnwrap(scroll.documentView)
            controller.window.setContentSize(NSSize(width: 1200, height: 820))
            try render(controller.root, name: "screenshot-editor-text-normal-\(appearance)")
            controller.window.setContentSize(NSSize(width: 1000, height: 700))
            scroll.contentView.scroll(to: NSPoint(x: 0, y: document.bounds.height - scroll.contentView.bounds.height))
            scroll.reflectScrolledClipView(scroll.contentView)
            controller.root.layoutSubtreeIfNeeded()
            for control in [apply, cancel] {
                XCTAssertTrue(scroll.contentView.bounds.contains(control.convert(control.bounds, to: scroll.contentView)),
                              "Text actions must be reachable at minimum size")
            }
            try render(controller.root, name: "screenshot-editor-text-minimum-\(appearance)")
            worker.failOperation = "edit_text"
            worker.failureMessage = "The supplied fonts cannot shape every glyph. Your accepted pixels and pending text are unchanged."
            editor.string = "Unaccepted text"
            apply.performClick(nil)
            XCTAssertEqual(editor.string, "Unaccepted text")
            try render(controller.root, name: "screenshot-editor-text-error-minimum-\(appearance)")
        }
    }

    func testWandUsesViewportMappingAndRejectsDragAndOffCanvasClicks() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(at: 5); _ = tool.sendAction(tool.action, to: tool.target)
        XCTAssertEqual(try field("Wand color tolerance", in: controller.root).stringValue, "36")
        let contiguous = try XCTUnwrap(descendants(in: controller.root).compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == "Wand contiguous only" })
        XCTAssertEqual(contiguous.state, .on)

        let image = controller.presentedImageRect
        let click = NSPoint(x: image.minX + image.width * 0.25,
                            y: image.minY + image.height * 0.75)
        controller.drawOverlay.begin(at: click); controller.drawOverlay.end(at: click)
        let request = try XCTUnwrap(worker.requests.last)
        XCTAssertEqual(request["operation"] as? String, "remove_image_background")
        XCTAssertEqual(request["tolerance"] as? Double, 36)
        XCTAssertEqual(request["contiguous"] as? Bool, true)
        let point = try XCTUnwrap(request["point"] as? [String: CGFloat])
        XCTAssertEqual(try XCTUnwrap(point["x"]), 160, accuracy: 0.001)
        XCTAssertEqual(try XCTUnwrap(point["y"]), 270, accuracy: 0.001)

        let count = worker.requests.count
        controller.drawOverlay.begin(at: click)
        controller.drawOverlay.end(at: NSPoint(x: click.x + 12, y: click.y))
        controller.drawOverlay.begin(at: NSPoint(x: image.minX - 1, y: image.midY))
        controller.drawOverlay.end(at: NSPoint(x: image.minX - 1, y: image.midY))
        XCTAssertEqual(worker.requests.count, count)
    }

    func testWandFailurePreservesAcceptedStateAndAllowsRetry() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        tool.selectItem(at: 5); _ = tool.sendAction(tool.action, to: tool.target)
        let accepted = controller.state.snapshot
        worker.failOperation = "remove_image_background"
        worker.failureMessage = "No matching pixels were found. Try a higher tolerance."
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        XCTAssertEqual(controller.state.snapshot, accepted); XCTAssertFalse(controller.state.busy)
        XCTAssertTrue(labels(in: controller.root).contains { $0.contains("No matching pixels") })
        worker.failOperation = nil
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        XCTAssertEqual(worker.requests.filter { $0["operation"] as? String == "remove_image_background" }.count, 2)
    }

    func testBackgroundBrushMapsEveryEventIncludesReleaseAndCancelsSafely() {
        _ = NSApplication.shared
        let overlay = EditorDrawOverlay(frame: NSRect(x: 0, y: 0, width: 200, height: 120))
        overlay.canvasSize = NSSize(width: 640, height: 360); overlay.drawingEnabled = true
        overlay.imageRect = { NSRect(x: 20, y: 10, width: 160, height: 90) }
        overlay.shape = .erase
        var strokes: [[NSPoint]] = []
        overlay.onBackgroundBrush = { mode, points in
            XCTAssertEqual(mode, .erase); strokes.append(points)
        }
        overlay.begin(at: NSPoint(x: 60, y: 77.5))
        overlay.drag(to: NSPoint(x: 190, y: 55)) // outside the image; Rust filters this sample.
        overlay.drag(to: NSPoint(x: 140, y: 32.5))
        overlay.end(at: NSPoint(x: 100, y: 55))
        XCTAssertEqual(strokes.count, 1)
        XCTAssertEqual(strokes[0], [NSPoint(x: 160, y: 270), NSPoint(x: 680, y: 180),
                                    NSPoint(x: 480, y: 90), NSPoint(x: 320, y: 180)])

        overlay.begin(at: NSPoint(x: 60, y: 77.5)); overlay.end(at: NSPoint(x: 60, y: 77.5))
        XCTAssertEqual(strokes.last, [NSPoint(x: 160, y: 270), NSPoint(x: 160, y: 270)],
                       "shipping stamps both down and release, including stationary soft edges")
        let count = strokes.count
        overlay.begin(at: NSPoint(x: 19, y: 55)); overlay.end(at: NSPoint(x: 100, y: 55))
        overlay.begin(at: NSPoint(x: 60, y: 55)); overlay.cancelGesture()
        overlay.end(at: NSPoint(x: 100, y: 55))
        XCTAssertEqual(strokes.count, count, "off-image starts and cancellation never edit")
    }

    func testBackgroundBrushOptionsIssueOneRetryableSerializedCommand() throws {
        _ = NSApplication.shared
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot"))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showDraw(in: controller.root)
        let tool = try popup("Drawing tool", in: controller.root)
        XCTAssertEqual(tool.itemTitles, ["Rectangle", "Ellipse", "Line", "Arrow", "Pen", "Wand",
                                         "Erase", "Restore", "Text", "Triangle", "Diamond", "Star"])
        tool.selectItem(at: 6); _ = tool.sendAction(tool.action, to: tool.target)
        XCTAssertEqual(try field("Brush diameter", in: controller.root).stringValue, "28")
        XCTAssertEqual(try field("Brush softness", in: controller.root).stringValue, "18")
        let point = NSPoint(x: controller.presentedImageRect.midX, y: controller.presentedImageRect.midY)
        worker.failOperation = "paint_image_background"
        let accepted = controller.state.snapshot
        controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
        XCTAssertEqual(controller.state.snapshot, accepted); XCTAssertFalse(controller.state.busy)
        worker.failOperation = nil
        controller.drawOverlay.begin(at: point); controller.drawOverlay.drag(to: NSPoint(x: point.x + 9, y: point.y + 7))
        controller.drawOverlay.end(at: NSPoint(x: point.x + 12, y: point.y + 11))
        let requests = worker.requests.filter { $0["operation"] as? String == "paint_image_background" }
        XCTAssertEqual(requests.count, 2, "each completed gesture has exactly one retryable command")
        XCTAssertEqual(requests.last?["mode"] as? String, "erase")
        XCTAssertEqual(requests.last?["size"] as? Double, 28)
        XCTAssertEqual(requests.last?["softness"] as? Double, 18)
        XCTAssertEqual((requests.last?["points"] as? [[String: CGFloat]])?.count, 3,
                       "press, last movement, and release are serialized")
    }

    func testLayerContextMenuTargetsClickedStableLayerWithoutChangingSelection() throws {
        _ = NSApplication.shared
        let first = layer(id: "first", name: "First", x: 0, y: 0,
                          visible: true, locked: false, opacity: 100)
        let locked = layer(id: "locked", name: "Locked", x: 10, y: 10,
                           visible: false, locked: true, opacity: 80)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [first, locked], canPaste: true))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["light-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        try showOutput(in: controller.root)
        try button("Preview output", in: controller.root).performClick(nil)
        let output = try segmented("Output preview image", in: controller.root)
        try showLayers(in: controller.root)
        let table = try table("Screenshot layers", in: controller.root)
        // The native table reverses the document's back-to-front order.
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        controller.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification))
        XCTAssertEqual(table.selectedRow, 1)

        let point = table.convert(NSPoint(x: 20, y: table.rect(ofRow: 0).midY), to: nil)
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: .rightMouseDown, location: point,
            modifierFlags: [], timestamp: 0, windowNumber: controller.window.windowNumber,
            context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
        let menu = try XCTUnwrap(table.menu(for: event))
        XCTAssertEqual(table.selectedRow, 1, "opening and cancelling a menu must not retarget selection")
        XCTAssertEqual(output.selectedSegment, 1)
        XCTAssertTrue(worker.requests.isEmpty)
        XCTAssertEqual(menu.items.filter { !$0.isSeparatorItem }.map(\.title),
                       ["Copy layer", "Paste layer", "Duplicate", "Delete"])
        XCTAssertTrue(menu.item(withTitle: "Copy layer")!.isEnabled, "hidden locked layers remain copyable")
        XCTAssertTrue(menu.item(withTitle: "Paste layer")!.isEnabled)
        XCTAssertTrue(menu.item(withTitle: "Duplicate")!.isEnabled)
        XCTAssertFalse(menu.item(withTitle: "Delete")!.isEnabled)

        menu.performActionForItem(at: 0)
        XCTAssertEqual(worker.requests.last?["operation"] as? String, "copy_layer")
        XCTAssertEqual(worker.requests.last?["id"] as? String, "locked")
        XCTAssertEqual(table.selectedRow, 1)
        XCTAssertEqual(output.selectedSegment, 1, "copy preserves encoded output")
        let pasteIndex = try XCTUnwrap(menu.items.firstIndex { $0.title == "Paste layer" })
        menu.performActionForItem(at: pasteIndex)
        XCTAssertEqual(worker.requests.last?["operation"] as? String, "paste_layer")
        XCTAssertEqual(worker.requests.last?["after_id"] as? String, "locked")
        XCTAssertNotNil(UUID(uuidString: try XCTUnwrap(worker.requests.last?["new_id"] as? String)))

        let blank = try XCTUnwrap(controller.layerContextMenu(row: -1))
        XCTAssertEqual(blank.items.map(\.title), ["Paste layer"])
        blank.performActionForItem(at: 0)
        XCTAssertNil(worker.requests.last?["after_id"])
    }

    func testLayerContextMenuCapabilityBusyClosedAndStaleGuards() throws {
        _ = NSApplication.shared
        let target = layer(id: "target", name: "Target", x: 0, y: 0,
                           visible: true, locked: false, opacity: 100)
        let worker = FakeEditorWorker(snapshot: snapshot(id: "shot", layers: [target], canPaste: false))
        let controller = ScreenshotEditorController(tokens: Tokens.variants["dark-mustard"]!, worker: worker)
        defer { controller.window.orderOut(nil) }
        controller.present(artifact: artifact(id: "shot"), historyRoot: "/native/History")
        let stale = try XCTUnwrap(controller.layerContextMenu(row: 0))
        XCTAssertFalse(stale.item(withTitle: "Paste layer")!.isEnabled)

        worker.deferRequests = true
        stale.performActionForItem(at: try XCTUnwrap(stale.items.firstIndex { $0.title == "Duplicate" }))
        XCTAssertEqual((worker.requests.last?["edit"] as? [String: Any])?["action"] as? String, "duplicate")
        let busy = try XCTUnwrap(controller.layerContextMenu(row: 0))
        XCTAssertTrue(busy.items.filter { !$0.isSeparatorItem }.allSatisfy { !$0.isEnabled })
        let count = worker.requests.count
        busy.performActionForItem(at: 0)
        XCTAssertEqual(worker.requests.count, count)

        worker.completePending(with: snapshot(id: "shot", layers: [], canPaste: true))
        stale.performActionForItem(at: 0)
        stale.performActionForItem(at: try XCTUnwrap(stale.items.firstIndex { $0.title == "Delete" }))
        XCTAssertEqual(worker.requests.count, count, "stable menu IDs must reject removed targets")
        XCTAssertEqual(controller.layerContextMenu(row: -1)?.items.map(\.title), ["Paste layer"])

        XCTAssertTrue(controller.prepareForTermination())
        XCTAssertNil(controller.layerContextMenu(row: -1), "closed editors expose no context actions")
        stale.performActionForItem(at: try XCTUnwrap(stale.items.firstIndex { $0.title == "Paste layer" }))
        XCTAssertEqual(worker.requests.count, count)
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
            XCTAssertEqual(tool.itemTitles, ["Rectangle", "Ellipse", "Line", "Arrow", "Pen", "Wand",
                                             "Erase", "Restore", "Text", "Triangle", "Diamond", "Star"])
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
            worker.failOperation = nil
            controller.drawOverlay.begin(at: NSPoint(x: 100, y: 220))
            controller.drawOverlay.end(at: NSPoint(x: 240, y: 210))
            tool.selectItem(at: 5); _ = tool.sendAction(tool.action, to: tool.target)
            try render(controller.root, name: "screenshot-editor-wand-minimum-\(appearance)")
            worker.failOperation = "remove_image_background"
            worker.failureMessage = "No matching pixels were found. Try a higher tolerance."
            let point = NSPoint(x: controller.presentedImageRect.midX,
                                y: controller.presentedImageRect.midY)
            controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
            try render(controller.root, name: "screenshot-editor-wand-error-minimum-\(appearance)")
            worker.failOperation = nil
            tool.selectItem(at: 6); _ = tool.sendAction(tool.action, to: tool.target)
            controller.drawOverlay.begin(at: point); controller.drawOverlay.end(at: point)
            try render(controller.root, name: "screenshot-editor-erase-minimum-\(appearance)")
            controller.drawOverlay.begin(at: NSPoint(x: point.x - 60, y: point.y - 30))
            controller.drawOverlay.drag(to: point)
            try render(controller.root, name: "screenshot-editor-erase-active-\(appearance)")
            worker.failOperation = "paint_image_background"
            worker.failureMessage = "The brush stroke could not be applied. The accepted pixels and undo history are unchanged."
            controller.drawOverlay.end(at: NSPoint(x: point.x + 30, y: point.y + 20))
            try render(controller.root, name: "screenshot-editor-erase-error-minimum-\(appearance)")
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

    func testRealBridgeFontFamiliesRenderAndReopenExactPixels() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker(); defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open text fixture")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in XCTAssertNotNil(try? result.get()); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        func request(_ object: [String: Any]) throws -> EditorPresentation {
            let done = expectation(description: "text request")
            var response: Result<EditorPresentation, Error>?
            worker.request(object) { response = $0; done.fulfill() }
            wait(for: [done], timeout: 5)
            return try XCTUnwrap(response).get()
        }
        _ = try request(["operation": "resize_canvas", "width": 640, "height": 360])
        let sans = try request(["operation": "create_text", "point": ["x": 50, "y": 80],
            "text": "Native Ωé", "fontFamily": "sans", "fontSize": 64, "color": "#111111"])
        XCTAssertEqual(sans.snapshot.fontFamilies,
            ["sans": "Liberation Sans", "serif": "Liberation Serif", "mono": "Liberation Mono",
             "rounded": "Nunito"])
        let id = try XCTUnwrap(sans.snapshot.layers.first?.id)
        let serif = try request(["operation": "edit_text", "id": id, "patch": ["fontFamily": "serif"]])
        let mono = try request(["operation": "edit_text", "id": id, "patch": ["fontFamily": "mono"]])
        func pixels(_ value: EditorPresentation) throws -> Data {
            try XCTUnwrap(value.image.dataProvider?.data) as Data
        }
        XCTAssertNotEqual(try pixels(sans), try pixels(serif))
        XCTAssertNotEqual(try pixels(serif), try pixels(mono))
        let undone = try request(["operation": "undo"])
        XCTAssertEqual(try pixels(undone), try pixels(serif))
        let redone = try request(["operation": "redo"])
        XCTAssertEqual(try pixels(redone), try pixels(mono))
        _ = try request(["operation": "save_draft", "updated_at_ms": 9753])
        worker.close(); EditorWorker.flush()
        let reopened = expectation(description: "reopen saved font bytes")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in
            do {
                let value = try result.get()
                XCTAssertEqual(value.snapshot.fontFamilies, mono.snapshot.fontFamilies)
                XCTAssertEqual(value.snapshot.layers.first?.textStyle?.fontFamily, "mono")
                XCTAssertEqual(try pixels(value), try pixels(mono))
            } catch { XCTFail("\(error)") }
            reopened.fulfill()
        }
        wait(for: [reopened], timeout: 5)
    }

    func testRealBridgeRoundedBoxUsesPinnedFontAndReopensExactPixels() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker(); defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open rounded text fixture")
        var original: EditorPresentation?
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in original = try? result.get(); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let fonts = try XCTUnwrap(original).snapshot.fontFamilies
        XCTAssertEqual(fonts["rounded"], "Nunito")
        XCTAssertTrue(try XCTUnwrap(original).snapshot.textStylePresets.contains { $0.id == "rounded-box" })
        func request(_ object: [String: Any]) throws -> EditorPresentation {
            let done = expectation(description: "rounded text request")
            var response: Result<EditorPresentation, Error>?
            worker.request(object) { response = $0; done.fulfill() }
            wait(for: [done], timeout: 5)
            return try XCTUnwrap(response).get()
        }
        _ = try request(["operation": "resize_canvas", "width": 640, "height": 360])
        let create: [String: Any] = ["operation": "create_text", "point": ["x": 190, "y": 110],
                                     "text": "Native Ωé", "fontSize": 48,
                                     "fontFamily": "sans", "color": "#ff3b5c"]
        let plain = try request(create)
        _ = try request(["operation": "undo"])
        var roundedCreate = create; roundedCreate["stylePreset"] = "rounded-box"
        let rounded = try request(roundedCreate)
        let style = try XCTUnwrap(rounded.snapshot.layers.first?.textStyle)
        XCTAssertEqual(style.fontFamily, "rounded")
        XCTAssertEqual(style.fontSize, 48)
        XCTAssertEqual(style.color, "#ff3b5c")
        XCTAssertEqual(style.background, "#111318")
        XCTAssertTrue(style.roundedBackground)
        XCTAssertFalse(style.bold); XCTAssertFalse(style.italic); XCTAssertFalse(style.outlined)
        XCTAssertEqual(style.align, "center")
        let pixels = try XCTUnwrap(rounded.image.dataProvider?.data) as Data
        XCTAssertNotEqual(pixels, try XCTUnwrap(plain.image.dataProvider?.data) as Data)
        _ = try request(["operation": "save_draft", "updated_at_ms": 9876])
        worker.close(); EditorWorker.flush()

        let verification = EditorWorker()
        let reopenedDraft = expectation(description: "reopen rounded text bytes")
        verification.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                          artifactID: fixture.id) { result in
            do {
                let value = try result.get()
                XCTAssertEqual(value.snapshot.layers.first?.textStyle, style)
                XCTAssertEqual(try XCTUnwrap(value.image.dataProvider?.data) as Data, pixels)
            } catch { XCTFail("\(error)") }
            reopenedDraft.fulfill()
        }
        wait(for: [reopenedDraft], timeout: 5)
        verification.close(); EditorWorker.flush()

        for appearance in ["light", "dark"] {
            let reopened = EditorWorker()
            let controller = ScreenshotEditorController(tokens: Tokens.variants["\(appearance)-mustard"]!, worker: reopened)
            controller.present(artifact: artifact(id: fixture.id), historyRoot: fixture.history.path)
            waitUntil { controller.state.snapshot?.hasDraft == true && !controller.state.busy }
            XCTAssertEqual(controller.state.snapshot?.layers.first?.textStyle, style)
            XCTAssertEqual(try popup("New text style", in: controller.root).titleOfSelectedItem, "Rounded box")
            try showDraw(in: controller.root)
            controller.window.setContentSize(NSSize(width: 1200, height: 820))
            try render(controller.root, name: "screenshot-editor-rounded-box-saved-normal-\(appearance)")
            controller.window.setContentSize(NSSize(width: 760, height: 540))
            try render(controller.root, name: "screenshot-editor-rounded-box-saved-minimum-\(appearance)")
            controller.window.orderOut(nil); reopened.close(); EditorWorker.flush()
        }
    }

    func testRealBridgeTextInputTransactionPreviewCancelBlankAndOneUndo() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker(); defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open text input fixture")
        var original: EditorPresentation?
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in original = try? result.get(); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let originalLayerCount = try XCTUnwrap(original).snapshot.layers.count
        func result(_ object: [String: Any]) -> Result<EditorPresentation, Error> {
            let done = expectation(description: "text input request")
            var response: Result<EditorPresentation, Error>!
            worker.request(object) { response = $0; done.fulfill() }
            wait(for: [done], timeout: 5)
            return response
        }
        func request(_ object: [String: Any]) throws -> EditorPresentation { try result(object).get() }
        _ = try request(["operation": "resize_canvas", "width": 640, "height": 360])
        _ = try request(["operation": "undo"])
        let inputID = "appkit-new"
        let begun = try request(["operation": "begin_text_input", "input_id": inputID,
            "target": ["kind": "new", "create": ["point": ["x": 180, "y": 120],
                "text": "", "fontSize": 48, "fontFamily": "sans", "color": "#111111"]]])
        XCTAssertEqual(begun.snapshot.activeTextInput?.inputID, inputID)
        XCTAssertEqual(begun.snapshot.activeTextInput?.isNew, true)
        XCTAssertFalse(begun.snapshot.unsavedChanges)
        XCTAssertFalse(begun.snapshot.canUndo)
        let layerID = try XCTUnwrap(begun.snapshot.activeTextInput?.layerID)

        let preview = try request(["operation": "update_text_input", "input_id": inputID,
                                   "text": "Native Ωé\nПривет"])
        XCTAssertEqual(preview.snapshot.layers.first?.textStyle?.text, "Native Ωé\nПривет")
        XCTAssertFalse(preview.snapshot.unsavedChanges)
        XCTAssertFalse(preview.snapshot.canUndo)
        XCTAssertThrowsError(try worker.prepareForTermination(textInput: nil).get(),
                             "termination cannot drop an unresolved shared input")
        XCTAssertThrowsError(try result(["operation": "resize_canvas", "width": 10, "height": 10]).get())
        XCTAssertThrowsError(try result(["operation": "update_text_input", "input_id": "stale",
                                         "text": "must not win"]).get())
        let encoded = expectation(description: "active text blocks raw pixel encode")
        worker.encode(["format": "png"]) { value in
            if case .success = value { XCTFail("active text unexpectedly encoded raw preview pixels") }
            encoded.fulfill()
        }
        let saved = expectation(description: "active text blocks save new")
        worker.saveNew([:]) { value in
            if case .success = value { XCTFail("active text unexpectedly saved preview pixels") }
            saved.fulfill()
        }
        wait(for: [encoded, saved], timeout: 5)
        let committed = try request(["operation": "finish_text_input", "input_id": inputID, "commit": true])
        XCTAssertNil(committed.snapshot.activeTextInput)
        XCTAssertTrue(committed.snapshot.unsavedChanges)
        XCTAssertTrue(committed.snapshot.canUndo)
        XCTAssertEqual(committed.snapshot.layers.first?.id, layerID)

        let undone = try request(["operation": "undo"])
        XCTAssertEqual(undone.snapshot.layers.count, originalLayerCount,
                       "Begin and every replacement commit as one document undo step")
        let restored = try request(["operation": "redo"])
        XCTAssertEqual(restored.snapshot.layers.first?.textStyle?.text, "Native Ωé\nПривет")
        let existingID = "appkit-existing"
        _ = try request(["operation": "begin_text_input", "input_id": existingID,
                         "target": ["kind": "existing", "id": layerID]])
        _ = try request(["operation": "update_text_input", "input_id": existingID, "text": "preview only"])
        let cancelled = try request(["operation": "finish_text_input", "input_id": existingID, "commit": false])
        XCTAssertEqual(cancelled.snapshot.layers.first?.textStyle?.text, "Native Ωé\nПривет")
        XCTAssertEqual(cancelled.snapshot.canUndo, restored.snapshot.canUndo)

        let blankID = "appkit-blank-existing"
        _ = try request(["operation": "begin_text_input", "input_id": blankID,
                         "target": ["kind": "existing", "id": layerID]])
        _ = try request(["operation": "update_text_input", "input_id": blankID, "text": "\n  "])
        let removed = try request(["operation": "finish_text_input", "input_id": blankID, "commit": true])
        XCTAssertEqual(removed.snapshot.layers.count, originalLayerCount)
        let restoredBlank = try request(["operation": "undo"])
        XCTAssertEqual(restoredBlank.snapshot.layers.first?.textStyle?.text, "Native Ωé\nПривет")

        let blankNewID = "appkit-blank-new"
        _ = try request(["operation": "begin_text_input", "input_id": blankNewID,
            "target": ["kind": "new", "create": ["point": ["x": 80, "y": 70],
                "text": "", "fontSize": 32, "fontFamily": "sans", "color": "#111111"]]])
        let blankNew = try request(["operation": "finish_text_input", "input_id": blankNewID, "commit": true])
        XCTAssertEqual(blankNew.snapshot.layers.count, originalLayerCount + 1,
                       "blank new composition is discarded")
    }

    func testRealBridgeTerminationDrainsUpdateThenCancelsWithoutPersistingPreview() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker(); defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open cancel-on-quit fixture")
        var original: EditorPresentation?
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in original = try? result.get(); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let originalLayerCount = try XCTUnwrap(original).snapshot.layers.count
        let inputID = "appkit-cancel-on-quit"
        let began = expectation(description: "begin cancel-on-quit input")
        worker.request(["operation": "begin_text_input", "input_id": inputID,
            "target": ["kind": "new", "create": ["point": ["x": 80, "y": 70],
                "text": "", "fontSize": 32, "fontFamily": "sans", "color": "#111111"]]]) {
            result in
            if case .failure(let error) = result { XCTFail("Begin failed: \(error)") }
            began.fulfill()
        }
        wait(for: [began], timeout: 5)

        let updated = expectation(description: "queued preview completes before cancellation")
        worker.request(["operation": "update_text_input", "input_id": inputID,
                        "text": "preview that must not persist"]) { result in
            if case .failure(let error) = result { XCTFail("Update failed: \(error)") }
            updated.fulfill()
        }
        XCTAssertNoThrow(try worker.prepareForTermination(textInput: EditorTerminationTextInput(
            inputID: inputID, text: "preview that must not persist", commit: false)).get())
        wait(for: [updated], timeout: 5)

        let reopenedWorker = EditorWorker(); defer { reopenedWorker.close(); EditorWorker.flush() }
        let reopened = expectation(description: "reopen after cancel-on-quit")
        var restored: EditorPresentation?
        reopenedWorker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                            artifactID: fixture.id) { result in
            restored = try? result.get(); reopened.fulfill()
        }
        wait(for: [reopened], timeout: 5)
        XCTAssertEqual(try XCTUnwrap(restored).snapshot.layers.count, originalLayerCount)
        XCTAssertNil(restored?.snapshot.activeTextInput)
        XCTAssertFalse(restored?.snapshot.unsavedChanges ?? true)
    }

    func testRealBridgeTerminationRetainsAcceptedFinishAcrossDraftPersistenceFailure() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker(); defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open persistence failure fixture")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                    artifactID: fixture.id) { result in
            if case .failure(let error) = result { XCTFail("Open failed: \(error)") }
            opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        let inputID = "appkit-persistence-retry"
        let began = expectation(description: "begin persistence retry input")
        worker.request(["operation": "begin_text_input", "input_id": inputID,
            "target": ["kind": "new", "create": ["point": ["x": 3, "y": 1],
                "text": "", "fontSize": 32, "fontFamily": "sans", "color": "#111111"]]]) {
            result in
            if case .failure(let error) = result { XCTFail("Begin failed: \(error)") }
            began.fulfill()
        }
        wait(for: [began], timeout: 5)

        try? FileManager.default.removeItem(at: fixture.drafts)
        try Data("not a directory".utf8).write(to: fixture.drafts)
        let first = worker.prepareForTermination(textInput: EditorTerminationTextInput(
            inputID: inputID, text: "committed despite disk failure", commit: true))
        guard case .failure(let error) = first,
              let failure = error as? EditorTerminationFailure else {
            return XCTFail("expected structured draft persistence failure")
        }
        let accepted = try XCTUnwrap(failure.acceptedPresentation)
        XCTAssertNil(accepted.snapshot.activeTextInput,
                     "Finish succeeded before draft persistence failed")
        XCTAssertEqual(accepted.snapshot.layers.first?.textStyle?.text,
                       "committed despite disk failure")
        XCTAssertTrue(accepted.snapshot.unsavedChanges)

        try FileManager.default.removeItem(at: fixture.drafts)
        XCTAssertNoThrow(try worker.prepareForTermination(textInput: nil).get(),
                         "retry persists the accepted commit without replaying its consumed token")
        let reopenedWorker = EditorWorker(); defer { reopenedWorker.close(); EditorWorker.flush() }
        let reopened = expectation(description: "reopen recovered draft")
        var restored: EditorPresentation?
        reopenedWorker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path,
                            artifactID: fixture.id) { result in
            restored = try? result.get(); reopened.fulfill()
        }
        wait(for: [reopened], timeout: 5)
        XCTAssertEqual(restored?.snapshot.layers.first?.textStyle?.text,
                       "committed despite disk failure")
        XCTAssertTrue(restored?.snapshot.hasDraft ?? false)
    }

    func testRealBridgeWandEditsAsymmetricPixelAndUndoRedo() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker(); defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open wand fixture")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in XCTAssertNotNil(try? result.get()); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        func request(_ object: [String: Any]) throws -> EditorPresentation {
            let done = expectation(description: "wand request")
            var response: Result<EditorPresentation, Error>?
            worker.request(object) { response = $0; done.fulfill() }
            wait(for: [done], timeout: 5)
            return try XCTUnwrap(response).get()
        }
        let removed = try request(["operation": "remove_image_background",
                                   "point": ["x": 2.5, "y": 1.5],
                                   "tolerance": 0, "contiguous": true])
        XCTAssertEqual(rgba(removed.image, x: 2, y: 1)[3], 0)
        XCTAssertEqual(rgba(removed.image, x: 1, y: 1)[3], 255)
        let undone = try request(["operation": "undo"])
        XCTAssertEqual(rgba(undone.image, x: 2, y: 1)[3], 255)
        let redone = try request(["operation": "redo"])
        XCTAssertEqual(rgba(redone.image, x: 2, y: 1)[3], 0)
    }

    func testRealBridgeBrushEraseRestoreAndUndo() throws {
        _ = NSApplication.shared
        let fixture = try makeHistoryFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }
        let worker = EditorWorker(); defer { worker.close(); EditorWorker.flush() }
        let opened = expectation(description: "open brush fixture")
        worker.open(historyRoot: fixture.history.path, draftsRoot: fixture.drafts.path, artifactID: fixture.id) {
            result in XCTAssertNotNil(try? result.get()); opened.fulfill()
        }
        wait(for: [opened], timeout: 5)
        func request(_ object: [String: Any]) throws -> EditorPresentation {
            let done = expectation(description: "brush request")
            var response: Result<EditorPresentation, Error>?
            worker.request(object) { response = $0; done.fulfill() }
            wait(for: [done], timeout: 5)
            return try XCTUnwrap(response).get()
        }
        let stroke: [String: Any] = ["operation": "paint_image_background",
                                     "points": [["x": 2.5, "y": 1.5]],
                                     "size": 4, "softness": 0]
        var erase = stroke; erase["mode"] = "erase"
        let erased = try request(erase)
        XCTAssertEqual(rgba(erased.image, x: 2, y: 1)[3], 0)
        var restore = stroke; restore["mode"] = "restore"
        let restored = try request(restore)
        XCTAssertEqual(rgba(restored.image, x: 2, y: 1)[3], 255)
        let undone = try request(["operation": "undo"])
        XCTAssertEqual(rgba(undone.image, x: 2, y: 1)[3], 0,
                       "restore publishes one undo step back to erased pixels")
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
                          canRedo: Bool = false,
                          originalExportPath: String? = nil,
                          initialTextSize: Double = 24,
                          layers: [[String: Any]] = [],
                          annotations: [String: [String: Any]] = [:],
                          textShadows: [String: [String: Any]]? = nil,
                          fonts: [String: String] = [:],
                          includeStandardPreset: Bool = true,
                          canPaste: Bool = false,
                          activeTextInput: [String: Any]? = nil) -> NativeEditorSnapshot {
        var value: [String: Any] = [
            "artifact_id": id, "document": ["width": width, "height": height,
                                                  "elements": layers],
            "initial_text_size": initialTextSize,
            "font_families": fonts,
            "text_style_presets": ([
                ["id": "standard", "label": "Standard", "fontFamily": "sans", "background": NSNull(), "outlined": false, "roundedBackground": false],
                ["id": "rounded", "label": "Rounded", "fontFamily": "rounded", "background": NSNull(), "outlined": false, "roundedBackground": false],
                ["id": "outlined", "label": "Outlined", "fontFamily": "sans", "background": NSNull(), "outlined": true, "roundedBackground": false],
                ["id": "mono", "label": "Mono", "fontFamily": "mono", "background": NSNull(), "outlined": false, "roundedBackground": false],
                ["id": "box", "label": "Box", "fontFamily": "sans", "background": "#111318", "outlined": false, "roundedBackground": false],
                ["id": "mono-box", "label": "Mono box", "fontFamily": "mono", "background": "#111318", "outlined": false, "roundedBackground": false],
                ["id": "rounded-box", "label": "Rounded box", "fontFamily": "rounded", "background": "#111318", "outlined": false, "roundedBackground": true],
            ] as [[String: Any]]).filter {
                fonts[$0["fontFamily"] as! String] != nil
                    && (includeStandardPreset || $0["id"] as? String != "standard")
            },
            "annotation_controls": annotations,
            "text_shadow_styles": textShadows ?? Dictionary(uniqueKeysWithValues: layers.compactMap { layer in
                guard layer["kind"] as? String == "text", let id = layer["id"] as? String else { return nil }
                return (id, ["color": "#000000", "opacity": 30.0, "blur": 5.984,
                             "offsetX": 0.0, "offsetY": 2.0] as [String: Any])
            }),
            "can_undo": unsaved, "can_redo": canRedo,
            "can_paste_layer": canPaste,
            "unsaved_changes": unsaved, "has_draft": draft,
            "active_text_input": activeTextInput ?? NSNull(),
        ]
        if let originalExportPath { value["original_export_path"] = originalExportPath }
        return NativeEditorSnapshot(value)!
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

    private func textLayer(id: String, text: String, family: String = "sans") -> [String: Any] {
        ["kind": "text", "id": id, "x": 20.0, "y": 24.0,
         "visible": true, "locked": false, "opacity": 100.0,
         "text": text, "fontSize": 32.0, "width": 256.0, "fontFamily": family,
         "bold": false, "italic": false, "align": "left", "color": "#111111",
         "background": NSNull(), "outlined": false, "roundedBackground": false]
    }

    private func artifact(id: String, mode: String = "region") -> CaptureArtifact {
        CaptureArtifact(["entry": [
            "id": id, "kind": "screenshot", "width": 640, "height": 360,
            "created_at": "2026-09-20T00:00:00Z", "mode": mode,
        ], "image_path": "/native/History/\(id)/capture.png",
            "preview_path": "/native/History/\(id)/preview.png"])!
    }

    private func button(_ title: String, in view: NSView) throws -> CaptureButton {
        let matches = descendants(in: view).compactMap { $0 as? CaptureButton }.filter { $0.title == title }
        return try XCTUnwrap(matches.first { !$0.isHiddenOrHasHiddenAncestor } ?? matches.first)
    }

    private func field(_ label: String, in view: NSView) throws -> NSTextField {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSTextField }
            .first { $0.accessibilityLabel() == label })
    }

    private func textView(_ label: String, in view: NSView) throws -> NSTextView {
        try XCTUnwrap(descendants(in: view).compactMap { $0 as? NSTextView }
            .first { $0.accessibilityLabel() == label })
    }

    private func showLayers(in view: NSView) throws {
        let sections = try segmented("Editor section", in: view)
        sections.selectedSegment = 1
        _ = sections.sendAction(sections.action, to: sections.target)
    }

    private func chooseZoomPreset(_ title: String, in view: NSView) throws {
        let control = try popup("Canvas zoom preset", in: view)
        XCTAssertNotNil(control.item(withTitle: title))
        control.selectItem(withTitle: title)
        _ = control.sendAction(control.action, to: control.target)
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
        let document = try XCTUnwrap(scroll.documentView)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, document.bounds.height - scroll.contentView.bounds.height)))
        scroll.reflectScrolledClipView(scroll.contentView)
        view.layoutSubtreeIfNeeded()
        XCTAssertTrue(scroll.contentView.bounds.contains(filename.convert(filename.bounds, to: scroll.contentView)),
                      "Export filename remains reachable after output sizing controls")
        let save = try button("Save new copy", in: view)
        XCTAssertTrue(save.superview === view)
        XCTAssertTrue(view.bounds.contains(save.frame))
        XCTAssertFalse(save.isHiddenOrHasHiddenAncestor)
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

    private func keyEvent(window: NSWindow, keyCode: UInt16, characters: String,
                          modifiers: NSEvent.ModifierFlags = []) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers,
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
    var originalSaves: [[String: Any]] = []
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
    var saveOriginalResult: Result<EditorSavePresentation, Error> = .success(.saved(path: "/exports/original.png"))
    var importLayerID = "imported-layer"
    var failImport = false
    var importedSnapshot: NativeEditorSnapshot?
    var response: (([String: Any]) -> NativeEditorSnapshot?)?
    var terminationResult: Result<Void, Error> = .success(())
    var terminationTextInputs: [EditorTerminationTextInput?] = []
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

    func completePendingFailure(_ message: String = "fixture request failed") {
        let completion = pendingCompletion
        pendingCompletion = nil
        completion?(.failure(AppBridgeError.backend(message)))
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

    func saveOriginal(_ request: [String: Any],
                      completion: @escaping (Result<EditorSavePresentation, Error>) -> Void) {
        originalSaves.append(request)
        completion(saveOriginalResult)
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
    func prepareForTermination(textInput: EditorTerminationTextInput? = nil) -> Result<Void, Error> {
        terminationTextInputs.append(textInput)
        return terminationResult
    }
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
