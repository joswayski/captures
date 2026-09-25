import AppKit
import XCTest
import CCapturesSettings
@testable import CapturesNative

final class MiniPreviewTests: XCTestCase {
    private let tokens = Tokens.variants["dark-mustard"]!

    func testCoordinateConversionPreservesNegativeOriginAndTopLeftGeometry() {
        let monitor = CapturesPreviewMonitor(work_x: -2880, work_y: 50,
            work_width: 2880, work_height: 1700, full_x: -2880, full_y: -100,
            full_width: 2880, full_height: 1800, scale_factor: 2)
        let geometry = CapturesPreviewGeometry(x: -1420, y: 25, width: 340, height: 240,
            card_height: 160, padding: 28, control_gutter: 52, anchor: 1)
        let frame = MiniPreviewController.appKitFrame(geometry: geometry, monitor: monitor,
            screenFrame: NSRect(x: -1440, y: 120, width: 1440, height: 900))
        XCTAssertEqual(frame, NSRect(x: -1420, y: 705, width: 340, height: 240))
    }

    func testPanelIsFixedGlassNonactivatingAndActionsStayBoundToArtifact() throws {
        _ = NSApplication.shared
        var actions: [String] = []
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["latest"], images: ["latest": image],
            copy: { _ in actions.append("copy") }, save: { _ in actions.append("save") },
            open: { _ in actions.append("open") }, trash: { _ in actions.append("trash") },
            dismiss: { _ in actions.append("dismiss") })
        defer { panel.close() }

        XCTAssertFalse(panel.canBecomeKey); XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertEqual(panel.previewView.artifactIDs, ["latest"])
        let buttons = panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
        XCTAssertEqual(buttons.map(\.title), ["Close", "Delete", "Edit", "Copy", "Save file"])
        XCTAssertEqual(buttons.first { $0.title == "Edit" }?.accessibilityLabel(), "Edit")
        XCTAssertTrue(buttons.allSatisfy(\.isHidden), "idle chrome must not leave click traps")
        buttons.forEach { $0.performClick(nil) }
        XCTAssertEqual(actions, ["dismiss", "dismiss", "open", "copy", "save"])
        try write(render(panel), name: "mini-preview-single-unsaved-edit-trash.png")
    }

    func testSavedCardUsesRevealWithShowInFolderAccessibility() throws {
        _ = NSApplication.shared
        let panel = fixturePanel(ids: ["saved"], images: ["saved": solidImage(.systemBlue)],
                                 savedPaths: ["saved": "/Exports/Café image.png"])
        defer { panel.close() }
        let buttons = panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
        let reveal = try XCTUnwrap(buttons.first { $0.title == "Show in Folder" })
        XCTAssertEqual(buttons.map(\.title), ["Close", "Delete", "Edit", "Copy", "Show in Folder"])
        XCTAssertEqual(reveal.accessibilityLabel(), "Show in Folder")
        XCTAssertEqual(reveal.toolTip, "Show in Folder")
        try write(render(panel), name: "mini-preview-single-saved-reveal.png")
    }

    func testHoverChromeMirrorsAndSaveUpdatesWithoutMovingCenterActions() throws {
        _ = NSApplication.shared
        for right in [false, true] {
            let panel = fixturePanel(ids: ["card"], images: ["card": solidImage(.white)],
                                     rightAnchor: right)
            defer { panel.close() }
            let card = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }.first)
            let controls = card.subviews.compactMap { $0 as? MiniPreviewButton }
            func button(_ name: String) throws -> MiniPreviewButton {
                try XCTUnwrap(controls.first { $0.title == name })
            }
            let event = try XCTUnwrap(NSEvent.mouseEvent(with: .mouseMoved, location: .zero,
                modifierFlags: [], timestamp: 1, windowNumber: panel.windowNumber, context: nil,
                eventNumber: 0, clickCount: 0, pressure: 0))
            XCTAssertTrue(controls.allSatisfy(\.isHidden))
            card.mouseEntered(with: event)
            XCTAssertEqual(controls.filter { !$0.isHidden }.map(\.title), ["Delete", "Edit", "Copy", "Save file"])
            XCTAssertEqual(try button("Delete").frame, NSRect(x: right ? 248 : 8, y: 8, width: 28, height: 28))
            XCTAssertEqual(try button("Edit").frame.minX, right ? 8 : 248)
            XCTAssertEqual(try button("Copy").frame, NSRect(x: 72, y: 45, width: 140, height: 32))
            let saveFrame = try button("Save file").frame
            XCTAssertEqual(saveFrame, NSRect(x: 72, y: 83, width: 140, height: 32))
            XCTAssertFalse(card.hasVisibleLabels)
            try write(render(panel), name: "mini-preview-hover-\(right ? "right" : "left")")
            card.updateSaveButton(saved: true)
            XCTAssertEqual(try button("Show in Folder").frame, saveFrame)
            XCTAssertFalse(try button("Close").isHidden)
            XCTAssertEqual(try button("Close").frame.minX, right ? 214 : 8)
            XCTAssertEqual(try button("Delete").frame.minX, right ? 248 : 42)
            card.mouseExited(with: event)
            XCTAssertTrue(controls.allSatisfy(\.isHidden))
            XCTAssertTrue(card.hasVisibleLabels)
        }
    }

    func testCollapsedPointerDragTracksDesktopWithoutExpandingAndClickStillExpands() throws {
        _ = NSApplication.shared
        var moves: [NSPoint] = []
        var expanded = 0
        let panel = fixturePanel(ids: ["one", "two"],
            images: ["one": solidImage(.red), "two": solidImage(.blue)], collapsed: true,
            setCollapsed: { if !$0 { expanded += 1 } }, move: { moves.append($0) })
        defer { panel.close() }
        panel.setFrameOrigin(NSPoint(x: 300, y: 200))
        let button = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == "Expand 2 previews" })
        let point = button.convert(NSPoint(x: 100, y: 80), to: nil)
        func event(_ type: NSEvent.EventType, _ point: NSPoint) throws -> NSEvent {
            try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [],
                timestamp: 1, windowNumber: panel.windowNumber, context: nil,
                eventNumber: 0, clickCount: 1, pressure: 1))
        }
        let rear = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }
            .first { $0.artifactID == "one" })
        let front = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }
            .first { $0.artifactID == "two" })
        let restY = rear.frame.minY, frontFrame = front.frame, windowFrame = panel.frame
        button.mouseEntered(with: try event(.leftMouseDown, point))
        try waitUntil { abs(rear.frame.minY - (restY - 2.946)) < 0.001 }
        XCTAssertEqual(front.frame, frontFrame)
        XCTAssertEqual(panel.frame, windowFrame)
        try write(render(panel), name: "mini-preview-stack-hovered.png")
        button.mouseDown(with: try event(.leftMouseDown, point))
        button.mouseExited(with: try event(.leftMouseDragged, point))
        XCTAssertTrue(panel.previewView.pileHovered, "Press retains hover outside the front card")
        button.mouseDragged(with: try event(.leftMouseDragged, NSPoint(x: point.x + 2, y: point.y - 1)))
        XCTAssertTrue(moves.isEmpty)
        button.mouseDragged(with: try event(.leftMouseDragged, NSPoint(x: point.x + 40, y: point.y - 25)))
        XCTAssertEqual(moves.last, NSPoint(x: 340, y: 175))
        panel.setFrameOrigin(try XCTUnwrap(moves.last))
        // The stationary desktop pointer returns to its original local position.
        button.mouseDragged(with: try event(.leftMouseDragged, point))
        XCTAssertEqual(moves.last, NSPoint(x: 340, y: 175))
        button.mouseUp(with: try event(.leftMouseUp, point))
        XCTAssertEqual(expanded, 0)
        button.mouseDown(with: try event(.leftMouseDown, point))
        button.mouseUp(with: try event(.leftMouseUp, point))
        XCTAssertEqual(expanded, 1)
        button.performClick(nil)
        XCTAssertEqual(expanded, 2, "Accessibility activation must remain a click")
        button.mouseExited(with: try event(.leftMouseDragged, point))
        try waitUntil { abs(rear.frame.minY - restY) < 0.001 }
        XCTAssertFalse(panel.previewView.pileHovered)
        XCTAssertFalse(panel.canBecomeKey)
    }

    func testOutboundCardDragIsCopyOnlyAndCompactPileRemainsMoveOnly() throws {
        _ = NSApplication.shared
        let image = solidImage(.systemBlue)
        let expanded = fixturePanel(ids: ["expanded"], images: ["expanded": image])
        let collapsed = fixturePanel(ids: ["older", "newer"],
            images: ["older": image, "newer": image], collapsed: true)
        defer { expanded.close(); collapsed.close() }
        let expandedCard = try XCTUnwrap(expanded.previewView.subviewsRecursive
            .compactMap { $0 as? MiniPreviewCardView }.first)
        expandedCard.preparedDragPath = "/tmp/exact-source.png"
        XCTAssertEqual(expandedCard.sourceOperationMask(for: .outsideApplication), .copy)
        XCTAssertEqual(expandedCard.sourceOperationMask(for: .withinApplication), .copy)
        XCTAssertTrue(expandedCard.isOutboundFileDragEnabled)

        let compactCards = collapsed.previewView.subviewsRecursive
            .compactMap { $0 as? MiniPreviewCardView }
        compactCards.forEach { $0.preparedDragPath = "/tmp/\($0.artifactID).png" }
        XCTAssertTrue(compactCards.allSatisfy { !$0.isOutboundFileDragEnabled },
            "compact cards remain move-only even after their files are prepared")
    }

    func testWorkspaceReconfigurationDoesNotDeleteLiveDragExports() {
        let transport = MiniPreviewActionTransport()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport)
        actions.configure(historyRoot: "/profile/history")
        LiveCaptureController.queue.sync {}
        XCTAssertEqual(transport.cleanupCount, 1)
        actions.configure(historyRoot: "/profile/history")
        LiveCaptureController.queue.sync {}
        XCTAssertEqual(transport.cleanupCount, 1)
    }

    func testStaleAsyncDragPreparationCannotAttachToReplacementIdentity() throws {
        _ = NSApplication.shared
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.blue) })
        defer { controller.close() }
        var preparations: [(CaptureArtifact, (String?) -> Void)] = []
        controller.prepareDrag = { preparations.append(($0, $1)) }
        let old = artifact(id: "same", previewPath: "/old-preview.png", imagePath: "/old.png")
        let oldGeneration = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(old, on: screenID(), settings: previewSettings(), generation: oldGeneration)
        try waitUntil { preparations.count == 1 }

        controller.dismiss(old.id)
        let replacement = artifact(id: "same", previewPath: "/new-preview.png", imagePath: "/new.png")
        let newGeneration = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(replacement, on: screenID(), settings: previewSettings(), generation: newGeneration)
        try waitUntil { preparations.count == 2 }
        preparations[0].1("/tmp/stale.png")
        // The dismissed panel is closed but may still be listed in NSApp.windows.
        let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView.artifactIDs == ["same"] })
        XCTAssertFalse(panel.previewView.containsPreparedDragPath("/tmp/stale.png"))
        preparations[1].1("/tmp/current.png")
        XCTAssertTrue(panel.previewView.containsPreparedDragPath("/tmp/current.png"))
        XCTAssertTrue(controller.updateSavedPath("/Exports/Café.png", for: replacement))
        XCTAssertEqual(preparations.count, 3)
        XCTAssertEqual(preparations[2].0.savedPath, "/Exports/Café.png")
        XCTAssertFalse(panel.previewView.containsPreparedDragPath("/tmp/current.png"))
        preparations[1].1("/tmp/late-unsaved.png")
        XCTAssertFalse(panel.previewView.containsPreparedDragPath("/tmp/late-unsaved.png"))
        preparations[2].1("/Exports/Café.png")
        XCTAssertTrue(panel.previewView.containsPreparedDragPath("/Exports/Café.png"))
    }

    func testDragCompletionRequiresExactSourceAndClassifiesDestinations() throws {
        _ = NSApplication.shared
        let controller = try presentedController(artifact(id: "source", previewPath: "/source.png"))
        defer { controller.close() }
        func card() throws -> (MiniPreviewPanel, MiniPreviewCardView) {
            // Closed panels stay in NSApp.windows while this test still holds them.
            let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
                .first { $0.isVisible && $0.previewView.artifactIDs == ["source"] })
            return (panel, try XCTUnwrap(panel.previewView.subviewsRecursive
                .compactMap { $0 as? MiniPreviewCardView }.first))
        }
        var current = try card()
        current.1.finishDrag(at: NSPoint(x: current.0.frame.midX, y: current.0.frame.midY),
                             operation: .copy)
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"], "own-panel drops are rejected")

        let ownWindow = NSWindow(contentRect: NSRect(x: 20_000, y: 20_000, width: 100, height: 100),
            styleMask: .borderless, backing: .buffered, defer: false)
        ownWindow.isReleasedWhenClosed = false; defer { ownWindow.close() }
        ownWindow.orderFrontRegardless()
        current.1.finishDrag(at: NSPoint(x: ownWindow.frame.midX, y: ownWindow.frame.midY),
                             operation: .copy)
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"], "another app window keeps the card")
        current.1.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: [])
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"], "cancelled external drags keep the card")

        let staleCard = current.1
        controller.dismiss("source")
        let replacement = artifact(id: "source", previewPath: "/replacement.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(replacement, on: screenID(), settings: previewSettings(), generation: generation)
        try waitUntil { controller.isPanelVisible }
        staleCard.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: .copy)
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"],
            "late completion from a different source identity cannot dismiss its replacement")

        current = try card()
        current.1.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: .copy)
        XCTAssertTrue(controller.presentedArtifactIDs.isEmpty,
            "an accepted external copy dismisses the exact source card")
    }

    func testPendingFirstDecodeIsInvalidatedWhenHistoryIsCleared() throws {
        _ = NSApplication.shared
        let releaseDecode = DispatchSemaphore(value: 0)
        let decodeReturned = expectation(description: "blocked preview decode returned")
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in
            releaseDecode.wait(); decodeReturned.fulfill(); return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_left",
                                           includeInCaptures: false)
        let pending = artifact(id: "pending", previewPath: "/pending-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(pending, on: screenID(), settings: settings, generation: generation)
        controller.reconcileHistory(ids: [])
        releaseDecode.signal()
        wait(for: [decodeReturned], timeout: 5)
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertNil(controller.presentedArtifactID)
        XCTAssertFalse(controller.isPanelVisible)
    }

    func testCopyAndSaveRemainAvailableAfterWorkspaceSwitchesToPreferences() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let png = try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:]))
        let imagePath = directory.appendingPathComponent("full.png")
        try png.write(to: imagePath)
        let transport = MiniPreviewActionTransport()
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("es.captures.tests.\(UUID())"))
        pasteboard.clearContents()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, pasteboard: { pasteboard })
        actions.configure(historyRoot: directory.path)
        var workspaceOwner: NSObject? = NSObject()
        workspaceOwner = nil // Preferences replaced the live workspace controller.
        XCTAssertNil(workspaceOwner)
        let captured = artifact(id: "latest", previewPath: imagePath.path,
                                imagePath: imagePath.path)

        actions.copy(captured)
        actions.save(captured)
        LiveCaptureController.flush()
        XCTAssertEqual(transport.saveCount, 1, "Save must execute on the persistent queue")
        XCTAssertEqual(transport.savedRoot, directory.path)
        try waitUntil { pasteboard.data(forType: .png) == png }
        XCTAssertEqual(pasteboard.data(forType: .png), png,
            "Copy must publish the full-resolution PNG, not the thumbnail")
    }

    func testSavedActionRevealsExactUnicodePathWithoutSavingAgain() throws {
        let path = "/Exports/Client shots/Café 東京.png"
        let transport = MiniPreviewActionTransport()
        var checkedPath: String?
        var revealed: [URL] = []
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, fileExists: {
                checkedPath = $0; return true
            }, revealFiles: { revealed = $0 })

        actions.save(artifact(id: "stable-id", previewPath: "/preview.png", savedPath: path))
        LiveCaptureController.flush()
        try waitUntil { !revealed.isEmpty }
        XCTAssertEqual(checkedPath, path)
        XCTAssertEqual(revealed, [URL(fileURLWithPath: path)])
        XCTAssertEqual(transport.saveCount, 0, "Reveal must not create another export")
    }

    func testSaveUpdatesExistingCardThenRevealAndMissingExportNeverSaveAgain() throws {
        _ = NSApplication.shared
        let image = solidImage(.systemBlue)
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        defer { controller.close() }
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let captured = artifact(id: "save-reveal", previewPath: "/save-reveal-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(captured, on: screenID(), settings: settings, generation: generation)
        try waitUntil { controller.isPanelVisible }
        let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.previewView.artifactIDs == [captured.id] })
        let button = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
            .first { $0.title == "Save file" })
        let originalFrame = panel.frame
        let transport = MiniPreviewActionTransport()
        transport.gate = DispatchSemaphore(value: 0)
        var exists = true
        var revealed: [URL] = []
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, fileExists: { path in
                XCTAssertFalse(Thread.isMainThread)
                XCTAssertEqual(path, "/exports/latest.png")
                return exists
            }, revealFiles: { revealed.append(contentsOf: $0) })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        controller.saveArtifact = { actions.save($0) }
        button.performClick(nil)
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        button.performClick(nil)
        actions.trash(captured)
        XCTAssertEqual(transport.saveCount, 1, "in-flight Save must not dispatch twice")
        XCTAssertEqual(transport.trashCount, 0, "Trash must not race an accepted Save")
        transport.gate?.signal()
        LiveCaptureController.flush()
        try waitUntil { button.title == "Show in Folder" }
        XCTAssertEqual(panel.frame, originalFrame)
        XCTAssertTrue(panel.isVisible, "Save updates the existing panel")
        XCTAssertEqual(button.accessibilityLabel(), "Show in Folder")
        controller.refreshArtifacts([captured])
        XCTAssertEqual(button.title, "Show in Folder", "an older unsaved snapshot cannot erase export state")
        button.performClick(nil)
        try waitUntil { revealed.count == 1 }
        XCTAssertEqual(revealed, [URL(fileURLWithPath: "/exports/latest.png")])
        exists = false
        button.performClick(nil)
        try waitUntil { controller.statusText(for: captured.id) == "Export missing" }
        let status = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? NSTextField }
            .first { $0.stringValue == "Export missing" })
        XCTAssertLessThanOrEqual(status.attributedStringValue.size().width, status.bounds.width,
                                 "the missing-export status must fit without truncation")
        XCTAssertEqual(revealed.count, 1)
        XCTAssertEqual(transport.saveCount, 1)
        XCTAssertEqual(button.title, "Show in Folder")
        XCTAssertEqual(controller.presentedArtifactIDs, [captured.id])
        try write(render(panel), name: "mini-preview-reveal-missing-export.png")
    }

    func testSaveFailureStaysSaveAndCompletionAfterDismissalIsRejected() throws {
        _ = NSApplication.shared
        let image = solidImage(.systemBlue)
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let captured = artifact(id: "stable", previewPath: "/stable-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(captured, on: screenID(), settings: settings, generation: generation)
        try waitUntil { controller.isPanelVisible }

        let failedTransport = MiniPreviewActionTransport(); failedTransport.fail = true
        let failedActions = MiniPreviewActions(settingsPath: nil, transport: failedTransport,
                                               loadPreferences: { try self.preferences() })
        failedActions.bind(previews: controller); failedActions.configure(historyRoot: "/History")
        failedActions.save(captured)
        LiveCaptureController.flush()
        try waitUntil { controller.statusText(for: captured.id) == "Save failed" }

        let blockedTransport = MiniPreviewActionTransport()
        blockedTransport.gate = DispatchSemaphore(value: 0)
        let blockedActions = MiniPreviewActions(settingsPath: nil, transport: blockedTransport,
                                                loadPreferences: { try self.preferences() })
        blockedActions.bind(previews: controller); blockedActions.configure(historyRoot: "/History")
        blockedActions.save(captured)
        XCTAssertEqual(blockedTransport.started.wait(timeout: .now() + 5), .success)
        controller.dismiss(captured.id)
        blockedTransport.gate?.signal()
        LiveCaptureController.flush()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.presentedArtifactIDs, [],
            "a dismissed card must not be recreated by a late save completion")
    }

    func testTrashSendsExactUnicodeSnapshotAndSerializesWithSave() throws {
        _ = NSApplication.shared
        let savedPath = "/Exports/客户/Café 東京.png"
        let captured = artifact(id: "截图-id", previewPath: "/preview.png", savedPath: savedPath)
        let controller = try presentedController(captured)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport()
        transport.gate = DispatchSemaphore(value: 0)
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/历史/History")

        actions.trash(captured)
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        actions.save(captured)
        XCTAssertEqual(transport.saveCount, 0, "Save and Trash share the artifact busy guard")
        XCTAssertEqual(transport.lastRequest?["operation"] as? String, "trash_preview")
        XCTAssertEqual(transport.lastRequest?["root"] as? String, "/历史/History")
        XCTAssertEqual(transport.lastRequest?["id"] as? String, "截图-id")
        XCTAssertEqual(transport.lastRequest?["saved_path"] as? String, savedPath)
        transport.gate?.signal(); LiveCaptureController.flush()
        try waitUntil { controller.presentedArtifactIDs.isEmpty }
        XCTAssertEqual(transport.trashCount, 1)
    }

    func testTrashFailureCanRetryAndKeepsReadableErrorDetail() throws {
        _ = NSApplication.shared
        let captured = artifact(id: "retry", previewPath: "/preview.png",
                                savedPath: "/Exports/retry.png")
        let controller = try presentedController(captured)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport(); transport.fail = true
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        actions.trash(captured); LiveCaptureController.flush()
        try waitUntil { controller.statusText(for: captured.id) == "Trash failed" }
        let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.previewView.artifactIDs == [captured.id] })
        let status = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? NSTextField }
            .first { $0.stringValue == "Trash failed" })
        XCTAssertFalse(try XCTUnwrap(status.toolTip).isEmpty)
        XCTAssertTrue(panel.isVisible)
        try write(render(panel), name: "mini-preview-trash-error.png")
        transport.fail = false
        actions.trash(captured); LiveCaptureController.flush()
        try waitUntil { controller.presentedArtifactIDs.isEmpty }
        XCTAssertEqual(transport.trashCount, 2)
    }

    func testTrashRejectsStaleSavedPathAndMalformedResponse() throws {
        _ = NSApplication.shared
        let original = artifact(id: "snapshot", previewPath: "/preview.png", savedPath: "/old.png")
        let controller = try presentedController(original)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        controller.refreshArtifacts([
            artifact(id: "snapshot", previewPath: "/preview.png", savedPath: "/new.png"),
        ])
        actions.trash(original)
        XCTAssertEqual(transport.requestCount, 0, "stale saved-path snapshots cannot dispatch")

        transport.malformed = true
        let current = artifact(id: "snapshot", previewPath: "/preview.png", savedPath: "/new.png")
        actions.trash(current); LiveCaptureController.flush()
        try waitUntil { controller.statusText(for: current.id) == "Trash failed" }
        XCTAssertEqual(controller.presentedArtifactIDs, [current.id],
            "malformed success responses must never dismiss")
    }

    func testUnsavedTrashOnlyDismissesAndLateSavedCompletionCannotRemoveReplacement() throws {
        _ = NSApplication.shared
        let unsaved = artifact(id: "draft", previewPath: "/draft-preview.png")
        let unsavedController = try presentedController(unsaved)
        let unsavedTransport = MiniPreviewActionTransport()
        let unsavedActions = MiniPreviewActions(settingsPath: nil, transport: unsavedTransport,
            loadPreferences: { try self.preferences() })
        unsavedActions.bind(previews: unsavedController); unsavedActions.configure(historyRoot: "/History")
        unsavedActions.trash(unsaved)
        LiveCaptureController.flush()
        try waitUntil { unsavedController.presentedArtifactIDs.isEmpty }
        XCTAssertEqual(unsavedTransport.requestCount, 1)
        XCTAssertEqual(unsavedTransport.lastRequest?["operation"] as? String, "trash_preview")
        XCTAssertTrue(unsavedTransport.lastRequest?["saved_path"] is NSNull,
            "unsaved Trash must never send a private file as the exported path")
        unsavedController.close()

        let saved = artifact(id: "same", previewPath: "/old-preview.png", savedPath: "/old.png")
        let controller = try presentedController(saved)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport(); transport.gate = DispatchSemaphore(value: 0)
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        actions.trash(saved)
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        let changed = artifact(id: "same", previewPath: "/old-preview.png", savedPath: "/changed.png")
        controller.refreshArtifacts([changed])
        transport.gate?.signal(); LiveCaptureController.flush()
        var completionDrained = false
        DispatchQueue.main.async { completionDrained = true }
        try waitUntil { completionDrained }
        XCTAssertEqual(controller.presentedArtifactIDs, ["same"],
            "a changed saved-path snapshot must reject late completion")

        transport.gate = DispatchSemaphore(value: 0)
        actions.trash(changed)
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        controller.dismiss(saved.id)
        let replacement = artifact(id: "same", previewPath: "/replacement-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(replacement, on: screenID(), settings: previewSettings(), generation: generation)
        try waitUntil { controller.decodedArtifactIDs == ["same"] }
        transport.gate?.signal(); LiveCaptureController.flush()
        completionDrained = false
        DispatchQueue.main.async { completionDrained = true }
        try waitUntil { completionDrained }
        XCTAssertEqual(controller.presentedArtifactIDs, ["same"],
            "late Trash completion must not remove an identity/path replacement")
    }

    func testOutOfOrderDecodesPreserveCaptureOrderAndCaptureCancellationRestoresPanel() throws {
        _ = NSApplication.shared
        let oldDecode = DispatchSemaphore(value: 0)
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/old-preview.png" { oldDecode.wait() }
            return image
        })
        let visible = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                          includeInCaptures: false)
        let old = artifact(id: "old", previewPath: "/old-preview.png")
        let new = artifact(id: "new", previewPath: "/new-preview.png")

        let first = try XCTUnwrap(controller.beginCapture(settings: visible))
        controller.present(old, on: screenID(), settings: visible, generation: first)
        let second = try XCTUnwrap(controller.beginCapture(settings: visible))
        controller.present(new, on: screenID(), settings: visible, generation: second)
        try waitUntil { controller.presentedArtifactID == "new" && controller.isPanelVisible }
        XCTAssertEqual(controller.decodedArtifactIDs, ["new"])
        oldDecode.signal()
        try waitUntil { controller.decodedArtifactIDs == ["old", "new"] }
        XCTAssertEqual(controller.presentedArtifactID, "new",
            "decode completion order must not change chronological membership")

        let cancelled = try XCTUnwrap(controller.beginCapture(settings: visible))
        XCTAssertFalse(controller.isPanelVisible)
        controller.restoreCapture(generation: cancelled)
        XCTAssertTrue(controller.isPanelVisible)
        let included = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: true)
        let includedCapture = try XCTUnwrap(controller.beginCapture(settings: included))
        XCTAssertTrue(controller.isPanelVisible,
            "the explicit inclusion preference keeps the preview visible during capture")
        controller.restoreCapture(generation: includedCapture)
        controller.updateSettings(MiniPreviewSettings(enabled: false, placement: "top_left",
                                                       includeInCaptures: false))
        XCTAssertFalse(controller.isPanelVisible)
        controller.reconcileHistory(ids: [])
        XCTAssertNil(controller.presentedArtifactID)
    }

    func testClearAllUsesSnapshotAndDoesNotRemoveLaterArrival() throws {
        _ = NSApplication.shared
        let oldDecode = DispatchSemaphore(value: 0)
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/old-preview.png" { oldDecode.wait() }
            return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "old", previewPath: "/old-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        XCTAssertEqual(controller.presentedArtifactIDs, ["old"])
        controller.clearAll()
        XCTAssertEqual(controller.presentedArtifactIDs, [])

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "new", previewPath: "/new-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        try waitUntil { controller.presentedArtifactIDs == ["new"] && controller.isPanelVisible }
        oldDecode.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.presentedArtifactIDs, ["new"],
            "late decode from the cleared snapshot cannot remove or resurrect previews")
    }

    func testIncomingCapturePreservesCollapsedState() throws {
        _ = NSApplication.shared
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        let settings = MiniPreviewSettings(enabled: true, placement: "top_left",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "first", previewPath: "/first-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        try waitUntil { controller.isPanelVisible }
        controller.setCollapsed(true)
        controller.moveStack(to: NSPoint(x: 240, y: 270))
        let moved = try XCTUnwrap(controller.stackOrigin)

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "second", previewPath: "/second-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        try waitUntil { controller.decodedArtifactIDs == ["first", "second"] }
        XCTAssertTrue(controller.isCollapsed)
        XCTAssertEqual(controller.stackOrigin?.x, moved.x)
        XCTAssertEqual(controller.stackOrigin?.edge, moved.edge)
        controller.setCollapsed(false)
        XCTAssertEqual(controller.stackOrigin?.edge, moved.edge)

        controller.updateSettings(MiniPreviewSettings(enabled: false, placement: "top_left",
                                                       includeInCaptures: false))
        XCTAssertNil(controller.stackOrigin)
        XCTAssertFalse(controller.isPanelVisible)
        XCTAssertFalse(controller.isCollapsed)
        XCTAssertEqual(controller.presentedArtifactIDs, ["first", "second"])
        controller.updateSettings(settings)
        XCTAssertTrue(controller.isPanelVisible)
        XCTAssertEqual(controller.presentedArtifactIDs, ["first", "second"],
            "hiding previews must retain membership for re-enable")
        controller.setCollapsed(true)
        controller.moveStack(to: NSPoint(x: 270, y: 250))
        XCTAssertNotNil(controller.stackOrigin)
        controller.clearAll()
        XCTAssertNil(controller.stackOrigin)
    }

    func testDeletingBlockedLatestDecodeRestoresVisibleOlderCard() throws {
        _ = NSApplication.shared
        let releaseLatest = DispatchSemaphore(value: 0)
        let image = solidImage(.systemBlue)
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/latest-preview.png" { releaseLatest.wait() }
            return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "older", previewPath: "/older-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        try waitUntil { controller.decodedArtifactIDs == ["older"] && controller.isPanelVisible }

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "latest", previewPath: "/latest-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        XCTAssertFalse(controller.isPanelVisible)
        controller.reconcileHistory(ids: ["older"])
        XCTAssertEqual(controller.presentedArtifactIDs, ["older"])
        XCTAssertTrue(controller.isPanelVisible,
            "deleting the pending latest capture must restore decoded survivors")
        releaseLatest.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.decodedArtifactIDs, ["older"])
    }

    func testCompactDepthShadePreservesFrontAndClearsOnExpansion() throws {
        _ = NSApplication.shared
        let panel = fixturePanel(ids: ["red"],
            images: ["red": solidImage(NSColor(srgbRed: 1, green: 0, blue: 0, alpha: 1))], collapsed: true)
        defer { panel.close() }
        let card = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }.first)
        func red() throws -> CGFloat {
            let bitmap = try render(panel)
            XCTAssertEqual(bitmap.colorSpace, .sRGB)
            XCTAssertEqual(bitmap.bitsPerSample, 8)
            // colorAt returns a calibrated NSColor even for this sRGB bitmap;
            // converting that color again changes the already-correct bytes.
            var pixel = [Int](repeating: 0, count: bitmap.samplesPerPixel)
            bitmap.getPixel(&pixel, atX: 170, y: 132)
            return CGFloat(pixel[0]) / 255
        }
        XCTAssertEqual(try red(), 1, accuracy: 0.01)
        card.setCompact(true, depth: 1)
        // Source red composited with (15,15,18) at CSS depth1 opacity .13748.
        XCTAssertEqual(try red(), 0.870607, accuracy: 0.015)
        try write(render(panel), name: "mini-preview-depth-one.png")
        card.setCompact(true, depth: 6)
        XCTAssertEqual(try red(), 0.322353, accuracy: 0.015, "deep shade caps at .72")
        card.setCompact(false, depth: 6)
        XCTAssertEqual(try red(), 1, accuracy: 0.01, "expanded images are never shaded")
        card.setCompact(true, depth: 0)
        XCTAssertEqual(try red(), 1, accuracy: 0.01, "new front image is never shaded")
    }

    func testRealThumbnailRendersExpandedAndCollapsedStacks() throws {
        _ = NSApplication.shared
        let ids = ["oldest", "middle", "newest"]
        let images = ["oldest": solidImage(.systemRed), "middle": solidImage(.systemGreen),
                      "newest": solidImage(.systemBlue)]
        let panels: [(MiniPreviewPanel, String)] = [
            (fixturePanel(ids: ids, images: images, topAnchor: true), "top-expanded"),
            (fixturePanel(ids: ids, images: images, collapsed: true, topAnchor: true),
             "top-collapsed"),
            (fixturePanel(ids: ids, images: images), "bottom-expanded"),
            (fixturePanel(ids: ids, images: images, collapsed: true), "bottom-collapsed")
        ]
        defer { panels.forEach { $0.0.close() } }

        for (panel, name) in panels {
            XCTAssertEqual(panel.previewView.cardPaintOrder, ids,
                "chronological subview order paints newest on top")
            let bitmap = try render(panel)
            XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 10, y: 10)).alphaComponent, 0,
                           accuracy: 0.01, "stack padding remains transparent")
            let newestSampleY = name == "bottom-expanded" ? 436 : 90
            let frontPixel = try XCTUnwrap(bitmap.colorAt(x: 170, y: newestSampleY))
            XCTAssertGreaterThan(frontPixel.alphaComponent, 0.9)
            XCTAssertGreaterThan(frontPixel.blueComponent, frontPixel.redComponent,
                "the newest blue capture must paint over older cards")
            try write(bitmap, name: "mini-preview-stack-\(name).png")
        }

        let asymmetric = NSImage(cgImage: PreviewView.fixtureImage(scale: 2),
                                 size: NSSize(width: 284, height: 160))
        let single = fixturePanel(ids: ["asymmetric"], images: ["asymmetric": asymmetric])
        defer { single.close() }
        try write(render(single), name: "mini-preview-single-asymmetric.png")

        let white = fixturePanel(ids: ["white"], images: ["white": solidImage(.white)])
        defer { white.close() }
        let whiteBitmap = try render(white)
        let whiteImagePixel = try XCTUnwrap(whiteBitmap.colorAt(x: 170, y: 90))
        XCTAssertGreaterThan(whiteImagePixel.brightnessComponent, 0.9)
        try write(whiteBitmap, name: "mini-preview-single-white.png")
    }

    func testCollapsedPileHidesActionsAndFrontHitTargetExpands() {
        _ = NSApplication.shared
        let ids = ["older", "newer"]
        let image = solidImage(.systemBlue)
        var expanded = false
        let panel = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }), collapsed: true,
            setCollapsed: { expanded = !$0 })
        defer { panel.close() }
        XCTAssertEqual(panel.previewView.visibleCardActionTitles, [])
        XCTAssertEqual(panel.previewView.visibleCardLabelCount, 0)
        panel.previewView.setStatus("Copied", for: "newer")
        XCTAssertEqual(panel.previewView.visibleCardLabelCount, 0,
            "late action feedback cannot restore compact labels")
        XCTAssertTrue(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
            .filter { !$0.isHidden }.isEmpty)
        XCTAssertEqual(panel.previewView.pileExpandAccessibilityLabel, "Expand 2 previews")
        panel.previewView.activatePileExpand()
        XCTAssertTrue(expanded)

        let expandedPanel = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }))
        defer { expandedPanel.close() }
        XCTAssertEqual(expandedPanel.previewView.visibleCardLabelCount, ids.count,
            "expanded cards retain the dimensions badge")
    }

    func testOverflowUsesSharedLayoutsAndReservesBottomControlGutter() throws {
        _ = NSApplication.shared
        let ids = (0..<8).map { "capture-\($0)" }
        let image = solidImage(.systemPurple)
        let panel = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }))
        defer { panel.close() }
        XCTAssertEqual(panel.previewView.documentHeight, 1_528)
        XCTAssertGreaterThan(panel.previewView.documentHeight, panel.previewView.bounds.height)
        XCTAssertEqual(panel.previewView.scrollOffsetY,
            panel.previewView.documentHeight - panel.previewView.bounds.height, accuracy: 0.5)

        let collapsed = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }), collapsed: true)
        defer { collapsed.close() }
        XCTAssertLessThanOrEqual(collapsed.previewView.documentHeight,
            collapsed.previewView.viewportHeight + 0.5,
            "collapsed piles never create a hidden scroll range")
        XCTAssertEqual(collapsed.previewView.scrollOffsetY, 0, accuracy: 0.5)

        let distinct = Dictionary(uniqueKeysWithValues: ids.enumerated().map { index, id in
            (id, solidImage(NSColor(calibratedHue: CGFloat(index) / CGFloat(ids.count),
                saturation: 0.8, brightness: 0.9, alpha: 1)))
        })
        let renderPanel = fixturePanel(ids: ids, images: distinct)
        defer { renderPanel.close() }
        XCTAssertEqual(renderPanel.previewView.scrollOffsetY,
            renderPanel.previewView.documentHeight - renderPanel.previewView.bounds.height,
            accuracy: 0.5, "bottom overflow opens with newest capture visible")
        try write(render(renderPanel), name: "mini-preview-stack-bottom-overflow.png")
    }

    func testClosingRootWindowClosesOpenPanelAndRequestsTermination() {
        _ = NSApplication.shared
        let root = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 500, height: 400),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        root.isReleasedWhenClosed = false
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["latest"], images: ["latest": image])
        panel.orderFrontRegardless()
        var terminationRequests = 0
        let handler = RootWindowCloseHandler(rootWindow: root,
            closePreviews: { panel.close() }, terminate: { terminationRequests += 1 })
        root.delegate = handler
        root.makeKeyAndOrderFront(nil)
        root.performClose(nil)
        XCTAssertFalse(panel.isVisible)
        XCTAssertEqual(terminationRequests, 1)
        withExtendedLifetime(handler) {}
    }

    private func artifact(id: String, previewPath: String,
                          imagePath: String? = nil, savedPath: String? = nil) -> CaptureArtifact {
        CaptureArtifact(["entry": ["id": id, "width": 800, "height": 600,
            "created_at": "2026-09-18T00:00:00Z",
            "saved_path": savedPath.map { $0 as Any } ?? NSNull()],
            "image_path": imagePath ?? "/\(id).png", "preview_path": previewPath])!
    }

    private func fixturePanel(ids: [String], images: [String: NSImage],
                              collapsed: Bool = false, topAnchor: Bool = false,
                              rightAnchor: Bool = false,
                              savedPaths: [String: String] = [:],
                              copy: @escaping (String) -> Void = { _ in },
                              save: @escaping (String) -> Void = { _ in },
                              open: @escaping (String) -> Void = { _ in },
                              trash: @escaping (String) -> Void = { _ in },
                              dismiss: @escaping (String) -> Void = { _ in },
                              setCollapsed: @escaping (Bool) -> Void = { _ in },
                              move: @escaping (NSPoint) -> Void = { _ in }) -> MiniPreviewPanel {
        let stack = NativePreviewStack()
        ids.forEach { XCTAssertTrue(stack.insert($0)) }
        if collapsed { stack.setCollapsed(true) }
        let placement = (topAnchor ? "top_" : "bottom_") + (rightAnchor ? "right" : "left")
        let monitor = CapturesPreviewMonitor(work_x: 0, work_y: 0,
            work_width: 800, work_height: 700, full_x: 0, full_y: 0,
            full_width: 800, full_height: 700, scale_factor: 1)
        let geometry = NativePreviewLayout.geometry(monitor: monitor, count: ids.count,
            collapsed: collapsed, placement: placement)!
        let resources = Dictionary(uniqueKeysWithValues: ids.compactMap { id in
            images[id].map { (id, MiniPreviewResource(artifact: artifact(id: id,
                previewPath: "/\(id)-preview.png", savedPath: savedPaths[id]), image: $0)) }
        })
        let layouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
            stack.cardLayout(index: index, topAnchor: topAnchor).map { (id, $0) }
        })
        let hoverLayouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
            stack.cardLayout(index: index, topAnchor: topAnchor, hovered: true).map { (id, $0) }
        })
        return MiniPreviewPanel(frame: NSRect(x: 0, y: 0,
            width: geometry.width, height: geometry.height), geometry: geometry,
            contentHeight: stack.contentHeight,
            resources: resources, ids: ids, layouts: layouts, hoverLayouts: hoverLayouts, collapsed: collapsed,
            topAnchor: topAnchor, rightAnchor: placement.hasSuffix("_right"), tokens: tokens,
            copy: copy, save: save, open: open,
            trash: trash, dismiss: dismiss, setCollapsed: setCollapsed, clearAll: {}, move: move)
    }

    private func previewSettings() -> MiniPreviewSettings {
        MiniPreviewSettings(enabled: true, placement: "bottom_right", includeInCaptures: false)
    }

    private func presentedController(_ captured: CaptureArtifact) throws -> MiniPreviewController {
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.systemBlue) })
        let settings = previewSettings()
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(captured, on: screenID(), settings: settings, generation: generation)
        try waitUntil { controller.isPanelVisible }
        return controller
    }

    private func solidImage(_ color: NSColor) -> NSImage {
        NSImage(size: NSSize(width: 284, height: 160), flipped: true) { rect in
            color.setFill(); rect.fill(); return true
        }
    }

    private func render(_ panel: MiniPreviewPanel) throws -> NSBitmapImageRep {
        panel.display(); panel.previewView.layoutSubtreeIfNeeded()
        let view = panel.previewView
        // Set the destination profile BEFORE drawing. Display-dependent blending
        // followed by conversion to sRGB does not give the same channel values.
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds)?
            .retagging(with: .sRGB))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return bitmap
    }

    private func write(_ bitmap: NSBitmapImageRep, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let url = URL(fileURLWithPath: directory).appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }

    private func preferences() throws -> CapturePreferences {
        try CapturePreferences(["auto_copy_to_clipboard": false,
            "show_cursor_in_screenshots": false, "freeze_screen": true,
            "auto_start_on_selection": false, "show_mini_previews": true,
            "mini_preview_placement": "bottom_right",
            "include_mini_previews_in_captures": false,
            "output_directory": "/exports", "screenshot_format": "png",
            "screenshot_countdown_seconds": 0])
    }

    private func screenID() -> String {
        NSScreen.screens.first.flatMap(MiniPreviewController.displayID(for:)) ?? "0"
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), "mini preview operation did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}

private final class MiniPreviewActionTransport: AppTransport {
    private let lock = NSLock()
    private var saves = 0
    private var root: String?
    private var trashes = 0
    private var requests = 0
    private var cleanups = 0
    private var recordedRequest: [String: Any]?
    var fail = false
    var malformed = false
    var gate: DispatchSemaphore?
    let started = DispatchSemaphore(value: 0)
    var saveCount: Int { lock.lock(); defer { lock.unlock() }; return saves }
    var savedRoot: String? { lock.lock(); defer { lock.unlock() }; return root }
    var trashCount: Int { lock.lock(); defer { lock.unlock() }; return trashes }
    var requestCount: Int { lock.lock(); defer { lock.unlock() }; return requests }
    var cleanupCount: Int { lock.lock(); defer { lock.unlock() }; return cleanups }
    var lastRequest: [String: Any]? { lock.lock(); defer { lock.unlock() }; return recordedRequest }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        let operation = object["operation"] as? String
        if operation == "clear_previous_preview_drags" {
            cleanups += 1
            return ["kind": "previous_preview_drags_cleared"]
        }
        requests += 1; recordedRequest = object
        guard operation == "save_screenshot" || operation == "trash_preview" else {
            throw AppBridgeError.invalidResponse
        }
        if operation == "save_screenshot" { saves += 1 } else { trashes += 1 }
        root = object["root"] as? String; started.signal()
        let gate = self.gate
        lock.unlock(); gate?.wait(); lock.lock()
        if fail { throw AppBridgeError.invalidResponse }
        if operation == "trash_preview" {
            if malformed { return ["kind": "wrong", "id": object["id"] as Any] }
            return ["kind": "preview_trashed", "id": object["id"] as Any]
        }
        return ["path": "/exports/latest.png"]
    }
}

private extension NSView {
    var subviewsRecursive: [NSView] {
        subviews + subviews.flatMap(\.subviewsRecursive)
    }
}
