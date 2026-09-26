import AppKit
import XCTest
@testable import CapturesNative

final class HistoryClearTests: XCTestCase {
    func testSharedHistoryPresentationMatchesShippingCopy() throws {
        let copy = try HistoryCopy(transport: SettingsBridge())
        XCTAssertEqual(copy.eyebrow, "On this device")
        XCTAssertEqual(copy.title, "Capture History")
        XCTAssertEqual(copy.emptyTitle, "No captures yet")
        XCTAssertEqual(copy.deleteAllConfirm, "Delete all forever")
        XCTAssertEqual(copy.label(.saveFile), "Save file")
        XCTAssertEqual(copy.busyLabel(.edit), "Opening…")
        XCTAssertEqual(copy.confirmTimeout, 4, accuracy: 0.001)
        XCTAssertEqual(copy.label(.restore), "Restore")
        XCTAssertEqual(copy.busyLabel(.restore), "Restoring…")
        XCTAssertEqual(copy.doneLabel(.restore), "Restored")
        XCTAssertEqual(copy.tooltip(.restore), "Bring this screenshot back as a floating preview")
        XCTAssertNil(copy.doneLabel(.saveFile))
        XCTAssertEqual(copy.feedbackDuration, 2.5, accuracy: 0.001)
        let entry: [String: Any] = ["id": "v", "kind": "video", "preview_url": "", "full_url": "",
            "width": 640, "height": 480, "size_bytes": 2_048, "created_at": "2026-09-26T15:04:05Z",
            "duration_ms": 65_000, "dropped_frames": 2]
        let cards = try HistoryCard.cards(for: [["entry": entry, "missing": false],
                                                ["entry": entry, "missing": true], ["entry": [String: Any]()]])
        XCTAssertEqual(cards.count, 3)
        XCTAssertEqual(cards[0]?.details, "640 × 480 · 2.0 KB · 1:05")
        XCTAssertEqual(cards[0]?.warning, "2 frames dropped while recording")
        XCTAssertEqual(cards[0]?.actions, [.edit, .saveFile])
        XCTAssertEqual(cards[1]?.missing, true)
        XCTAssertEqual(cards[1]?.actions, [])
        XCTAssertEqual(cards[1]?.deleteRequiresConfirmation, false)
        XCTAssertNil(cards[2], "a malformed entry has no card")
        let layout = try XCTUnwrap(HistoryGridLayout.make(width: 944))
        XCTAssertEqual(layout.columns, 3)
        XCTAssertEqual(layout.visibleItems(5, in: NSRect(x: 0, y: 0, width: 944, height: 100)), 0..<3)
        XCTAssertEqual(layout.step(1, count: 5, columns: 0, rows: 1), 4)
        XCTAssertEqual(layout.step(4, count: 5, columns: 1, rows: 0), 4)
    }

    func testMixedHistoryCardsFiltersSelectionAndSave() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let image = PreviewView.fixtureImage(scale: 1)
        let path = directory.appendingPathComponent("fixture.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])).write(to: path)
        let settingsPath = directory.appendingPathComponent("settings.json").path
        let settingsBridge = SettingsBridge()
        var settings = try XCTUnwrap(settingsBridge.request(["operation": "load", "path": settingsPath])["settings"] as? [String: Any])
        settings["output_directory"] = directory.appendingPathComponent("exports").path
        settings["screenshot_format"] = "jpeg"
        _ = try settingsBridge.request(["operation": "save", "path": settingsPath, "settings": settings])
        for appearance in ["light", "dark"] {
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 600)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame); window.contentView = root
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            root.wantsLayer = true; root.layer!.backgroundColor = tokens.color("surface-canvas").cgColor
            window.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
            let transport = HistoryTransport(path: path.path, width: image.width, height: image.height,
                failPartway: false, kinds: ["video", "screenshot", "gif", "screenshot", "video"])
            let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
                historyRoot: directory.path, settingsPath: settingsPath, transport: transport,
                recoveryWorker: EmptyRecoveryWorker(), showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            let grid = try historyGrid(root)
            func button(_ title: String) throws -> NSButton {
                try XCTUnwrap(root.subviews.compactMap { $0 as? NSButton }.first { $0.title == title })
            }
            func labels() -> [String] {
                root.subviews.compactMap { $0 as? NSTextField }.filter { !$0.isHidden }.map(\.stringValue)
            }
            func actions(_ row: Int) -> [String] {
                grid.card(at: row)?.actionButtons.filter { !$0.isHidden }.map(\.title) ?? []
            }
            try waitUntil { grid.numberOfRows == 5 && grid.visibleCards.count == 5 }
            XCTAssertEqual(grid.selectedRow, -1, "loading History never selects a card")
            XCTAssertTrue(labels().contains("ON THIS DEVICE"))
            XCTAssertTrue(labels().contains("Capture History"))
            XCTAssertTrue(labels().contains { $0.contains("appear here for 30 days") })
            XCTAssertEqual(try button("All 5").state, .on)
            XCTAssertEqual(try button("Delete all").accessibilityLabel(), "Delete all captures")
            let first = try XCTUnwrap(grid.card(at: 0))
            XCTAssertFalse(first.dateLabel.stringValue.isEmpty)
            XCTAssertTrue(first.detailsLabel.stringValue.hasPrefix("\(image.width) × \(image.height) · 2.0 KB"))
            XCTAssertEqual(actions(0), ["Edit", "Save file"])
            XCTAssertEqual(first.deleteButton.accessibilityLabel(), "Delete from History")
            try waitUntil { grid.visibleCards.allSatisfy { $0.thumbnail.image != nil } }

            try button("Screenshots 2").performClick(nil)
            try waitUntil { grid.numberOfRows == 2 }
            XCTAssertEqual(actions(0), ["Edit", "Restore"], "shipping screenshot cards offer Restore")
            grid.selectRowIndexes([1], byExtendingSelection: false)
            XCTAssertEqual(grid.selectedRow, 1)
            XCTAssertTrue(try XCTUnwrap(grid.card(at: 1)).selected)
            transport.promote(id: "item-3")
            try button("Refresh").performClick(nil)
            try waitUntil { grid.selectedRow == 0 && grid.card(at: 0)?.artifactID == "item-3" }
            XCTAssertEqual(try button("Screenshots 2").state, .on)
            // Keyboard: arrows move the explicit selection within the filter.
            window.makeFirstResponder(grid)
            grid.keyDown(with: try key(124, window))
            XCTAssertEqual(grid.selectedRow, 1)
            grid.keyDown(with: try key(126, window))
            XCTAssertEqual(grid.selectedRow, 0)
            try button("Video 2").performClick(nil)
            try waitUntil { grid.numberOfRows == 2 }
            XCTAssertEqual(grid.selectedRow, -1, "a hidden selection is cleared, not replaced")
            XCTAssertEqual(actions(1), ["Edit", "Save file"])
            try button("GIF 1").performClick(nil)
            try waitUntil { grid.numberOfRows == 1 }
            XCTAssertEqual(try button("GIF 1").state, .on)
            let gif = try XCTUnwrap(grid.card(at: 0))
            XCTAssertEqual(actions(0), ["Edit", "Save file"])
            gif.actionButtons[1].performClick(nil)
            try waitUntil { transport.saveCount == 1 && labels().contains { $0.contains("Saved recording to") } }
            try waitUntil { actions(0) == ["Edit", "Show in Folder"] }
            XCTAssertEqual(transport.savedDirectory, settings["output_directory"] as? String)
            XCTAssertEqual(grid.selectedRow, 0, "a card action selects its card")
            for title in ["All 5", "Screenshots 2", "Video 2", "GIF 1", "Delete all"] {
                XCTAssertTrue(root.bounds.contains(try button(title).frame))
            }
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                try button("All 5").performClick(nil)
                try waitUntil { grid.visibleCards.count == 5 && grid.visibleCards.allSatisfy { $0.thumbnail.image != nil } }
                try capture(root, to: URL(fileURLWithPath: output).appendingPathComponent("history-grid-\(appearance).png"))
                try button("GIF 1").performClick(nil)
                try waitUntil { grid.numberOfRows == 1 }
            }
            transport.remove(kind: "gif")
            try button("Refresh").performClick(nil)
            // The filtered-empty copy lives in the grid area: Refresh also reloads
            // displays, and that status message lands after History's.
            try waitUntil { grid.numberOfRows == 0 && labels().contains("No captures match this filter.") }
            XCTAssertEqual(grid.selectedRow, -1)
            XCTAssertFalse(try button("GIF 0").isEnabled)
            XCTAssertEqual(try button("GIF 0").state, .on)
            try button("All 4").performClick(nil)
            try waitUntil { grid.numberOfRows == 4 }
            XCTAssertFalse(labels().contains("No captures match this filter."))
            XCTAssertEqual(transport.clearCount, 0, "filtering never deletes files")
        }
    }

    func testRestoreBringsAScreenshotBackAsAFloatingPreview() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let image = PreviewView.fixtureImage(scale: 1)
        let path = directory.appendingPathComponent("fixture.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])).write(to: path)
        let settingsPath = directory.appendingPathComponent("settings.json").path
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 600)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let transport = HistoryTransport(path: path.path, width: image.width, height: image.height,
            failPartway: false, kinds: ["screenshot", "video"])
        let loader = RestoreImageLoader(NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height)))
        let previews = MiniPreviewController(tokens: tokens, imageLoader: { try loader.load($0) })
        defer { previews.close() }
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: directory.path, settingsPath: settingsPath, transport: transport,
            recoveryWorker: EmptyRecoveryWorker(), miniPreviews: previews, showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let grid = try historyGrid(root)
        try waitUntil { grid.numberOfRows == 2 && grid.visibleCards.count == 2 }
        func restoreTitle() -> String? { grid.card(at: 0)?.actionButtons[1].title }
        func clickRestore() throws { try XCTUnwrap(grid.card(at: 0)).actionButtons[1].performClick(nil) }
        XCTAssertEqual(grid.card(at: 0)?.artifactID, "item-0")
        XCTAssertEqual(restoreTitle(), "Restore")
        XCTAssertEqual(grid.card(at: 0)?.actionButtons[1].toolTip, "Bring this screenshot back as a floating preview")
        XCTAssertEqual(grid.card(at: 1)?.actionButtons.filter { !$0.isHidden }.map(\.title) ?? [], ["Edit", "Save file"],
                       "recordings never offer Restore")
        XCTAssertTrue(previews.presentedArtifactIDs.isEmpty)

        try clickRestore()
        try waitUntil { previews.decodedArtifactIDs == ["item-0"] && restoreTitle() == "Restored" }
        XCTAssertTrue(previews.isPanelVisible)
        XCTAssertFalse(previews.isClipboardCurrent(for: "item-0"), "Restore never copies")
        // "✓ Restored" reverts after the shipping 2.5 s.
        try waitUntil { restoreTitle() == "Restore" }

        // Already showing: no duplicate or reorder, but still confirmed.
        try clickRestore()
        try waitUntil { restoreTitle() == "Restored" }
        XCTAssertEqual(previews.presentedArtifactIDs, ["item-0"])

        // An unreadable preview reports the error and leaves no card.
        previews.dismiss("item-0")
        loader.setFailing(true)
        try clickRestore()
        try waitUntil {
            restoreTitle() == "Restore" && root.subviews.contains {
                ($0 as? NSTextField)?.stringValue.hasPrefix("Couldn’t restore screenshot") == true
            }
        }
        XCTAssertTrue(previews.presentedArtifactIDs.isEmpty)
        XCTAssertEqual(transport.saveCount, 0)
        XCTAssertEqual(transport.deletedIDs, [], "Restore never changes History")
    }

    func testCardDeleteNeedsSecondClickExceptMissingRecordings() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let image = PreviewView.fixtureImage(scale: 1)
        let path = directory.appendingPathComponent("fixture.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])).write(to: path)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 600)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let transport = HistoryTransport(path: path.path, width: image.width, height: image.height,
            failPartway: false, kinds: ["screenshot", "video", "screenshot"], missing: ["item-1"])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: directory.path, settingsPath: nil, transport: transport,
            recoveryWorker: EmptyRecoveryWorker(), showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        window.makeKeyAndOrderFront(nil)
        let grid = try historyGrid(root)
        try waitUntil { grid.numberOfRows == 3 && grid.visibleCards.count == 3 }
        let missing = try XCTUnwrap(grid.card(at: 1))
        XCTAssertEqual(missing.artifactID, "item-1")
        XCTAssertFalse(missing.missingLabel.isHidden)
        XCTAssertEqual(missing.missingLabel.stringValue, "File missing")
        XCTAssertTrue(missing.actionButtons.allSatisfy(\.isHidden), "a missing recording has no actions")
        XCTAssertEqual(missing.deleteButton.accessibilityLabel(), "Remove missing entry")

        let screenshot = try XCTUnwrap(grid.card(at: 0))
        screenshot.deleteButton.performClick(nil)
        XCTAssertEqual(transport.deletedIDs, [], "the first click only arms deletion")
        XCTAssertEqual(try XCTUnwrap(grid.card(at: 0)).deleteButton.accessibilityLabel(), "Confirm permanent deletion")
        XCTAssertEqual(try XCTUnwrap(grid.card(at: 0)).deleteButton.toolTip, "Delete forever")
        window.makeFirstResponder(grid)
        grid.keyDown(with: try key(53, window))
        XCTAssertEqual(try XCTUnwrap(grid.card(at: 0)).deleteButton.accessibilityLabel(), "Delete from History",
                       "Escape backs out without deleting")
        try XCTUnwrap(grid.card(at: 0)).deleteButton.performClick(nil)
        try XCTUnwrap(grid.card(at: 0)).deleteButton.performClick(nil)
        try waitUntil { transport.deletedIDs == ["item-0"] && grid.numberOfRows == 2 }
        try XCTUnwrap(grid.card(at: 0)).deleteButton.performClick(nil)
        try waitUntil { transport.deletedIDs == ["item-0", "item-1"] && grid.numberOfRows == 1 }
        XCTAssertEqual(grid.card(at: 0)?.artifactID, "item-2")
    }

    func testCancelConfirmationClearAndPartialFailureRefresh() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let image = PreviewView.fixtureImage(scale: 1)
        let png = try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]))
        let path = directory.appendingPathComponent("fixture.png")
        try png.write(to: path)

        for failPartway in [false, true] {
            let appearance = failPartway ? "dark" : "light"
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 600)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let tokens = Tokens.variants["\(appearance)-mustard"]!
            let root = Surface(frame: frame); window.contentView = root
            root.wantsLayer = true; root.layer!.backgroundColor = tokens.color("surface-canvas").cgColor
            window.appearance = NSAppearance(named: failPartway ? .darkAqua : .aqua)
            let transport = HistoryTransport(path: path.path, width: image.width, height: image.height,
                failPartway: failPartway, kinds: failPartway ? ["screenshot", "video", "gif"] : ["video", "gif"])
            let controller = LiveCaptureController(root: root, window: window,
                tokens: tokens, historyRoot: directory.path,
                settingsPath: nil, transport: transport,
                recoveryWorker: EmptyRecoveryWorker(), showPreferences: {})
            defer { withExtendedLifetime(controller) {} }
            window.makeKeyAndOrderFront(nil)
            let clear = try XCTUnwrap(root.subviews.compactMap { $0 as? HistoryButton }
                .first { $0.accessibilityLabel() == "Delete all captures" })
            let cancel = try XCTUnwrap(root.subviews.compactMap { $0 as? HistoryButton }
                .first { $0.accessibilityLabel() == "Cancel delete all captures" })
            let grid = try historyGrid(root)
            let empty = try XCTUnwrap(root.subviews.compactMap { $0 as? HistoryEmptyView }.first)
            try waitUntil { grid.numberOfRows == (failPartway ? 3 : 2) && clear.isEnabled && !clear.isHidden }
            XCTAssertEqual(clear.title, "Delete all")
            XCTAssertTrue(cancel.isHidden)
            XCTAssertTrue(empty.isHidden)
            let gif = try XCTUnwrap(root.subviews.compactMap { $0 as? NSButton }.first { $0.title == "GIF 1" })
            gif.performClick(nil)
            try waitUntil { grid.numberOfRows == 1 }

            clear.performClick(nil)
            XCTAssertEqual(clear.title, "Delete all forever")
            XCTAssertEqual(clear.accessibilityLabel(), "Confirm delete all captures")
            XCTAssertFalse(cancel.isHidden)
            XCTAssertEqual(transport.clearCount, 0, "arming confirmation cannot delete anything")
            cancel.performClick(nil)
            XCTAssertEqual(clear.title, "Delete all")
            XCTAssertTrue(cancel.isHidden)
            XCTAssertEqual(transport.clearCount, 0)
            XCTAssertEqual(grid.numberOfRows, 1)

            clear.performClick(nil)
            let blockedGate = failPartway ? nil : transport.blockNextHistoryAfterClear()
            clear.performClick(nil)
            if !failPartway {
                var historyStarted = false
                try waitUntil {
                    historyStarted = historyStarted || transport.blockedHistoryStarted.wait(timeout: .now()) == .success
                    return historyStarted
                }
                controller.refreshHistory() // supersedes clear-owned reload before it returns
                blockedGate?.signal()
            }
            try waitUntil {
                transport.clearCount == 1 && grid.numberOfRows == (failPartway ? 1 : 0)
                    && clear.isEnabled == failPartway && clear.isHidden == !failPartway
            }
            if !failPartway {
                let refresh = try XCTUnwrap(root.subviews.compactMap { $0 as? CaptureButton }
                    .first { $0.title == "Refresh" })
                try waitUntil { refresh.isEnabled }
                XCTAssertFalse(empty.isHidden)
                XCTAssertEqual(empty.titleLabel.stringValue, "No captures yet")
                XCTAssertEqual(empty.bodyLabel.stringValue, "New screenshots, videos, and GIFs appear here automatically.")
                XCTAssertTrue(root.subviews.compactMap { $0 as? HistoryFilterPill }.allSatisfy(\.isHidden),
                              "filters are hidden while History is empty")
            } else {
                XCTAssertTrue(root.subviews.compactMap { ($0 as? NSTextField)?.stringValue }
                    .contains { $0.contains("Couldn’t delete capture history") && $0.contains("fixture deletion failed") })
            }
            if let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                window.display(); root.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in: root.bounds))
                root.cacheDisplay(in: root.bounds, to: bitmap)
                XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 500, y: 50)).alphaComponent, 1, accuracy: 0.01,
                    "render the same opaque canvas as the real workspace")
                let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                let folder = URL(fileURLWithPath: output)
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                try png.write(to: folder.appendingPathComponent("history-clear-\(appearance)-\(failPartway ? "error" : "empty").png"))
            }
            if failPartway {
                clear.performClick(nil)
                clear.performClick(nil)
                try waitUntil { transport.clearCount == 2 && grid.numberOfRows == 0 && clear.isHidden }
            }
        }
    }

    func testHistoryTabOrderFollowsShippingHeaderFiltersRecoveryThenGrid() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let image = PreviewView.fixtureImage(scale: 1)
        let path = directory.appendingPathComponent("fixture.png")
        try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])).write(to: path)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 600)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame); window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let transport = HistoryTransport(path: path.path, width: image.width, height: image.height,
            failPartway: false, kinds: ["screenshot", "video"], missing: [])
        let controller = LiveCaptureController(root: root, window: window, tokens: tokens,
            historyRoot: directory.path, settingsPath: nil, transport: transport,
            recoveryWorker: EmptyRecoveryWorker(), showPreferences: {})
        defer { withExtendedLifetime(controller) {} }
        let grid = try historyGrid(root)
        try waitUntil { grid.numberOfRows == 2 }
        XCTAssertTrue(window.initialFirstResponder === grid, "The grid starts focused for its arrow keys")
        let order = KeyViewLoop.order(from: grid)
        XCTAssertFalse(order.contains { $0.isDescendant(of: grid) && $0 !== grid },
                       "Tab visits the grid once, not each card control")
        let copy = HistoryCopy.current
        let cancel = try XCTUnwrap(order.firstIndex { $0.accessibilityLabel() == copy.cancelLabel })
        let deleteAll = try XCTUnwrap(order.firstIndex { $0.accessibilityLabel() == copy.deleteAllLabel })
        XCTAssertLessThan(cancel, deleteAll, "Cancel precedes Delete all, like shipping")
        let pills = order.indices.filter { order[$0] is HistoryFilterPill }
        XCTAssertEqual(pills.count, CaptureHistoryFilter.allCases.count)
        XCTAssertGreaterThan(pills.first ?? 0, deleteAll)
        let retry = try XCTUnwrap(keyViewIndex(order, "Retry list"))
        XCTAssertGreaterThan(retry, pills.last ?? 0, "Interrupted recordings follow the filters")
        XCTAssertTrue(order.last?.nextKeyView === grid, "The grid ends the loop after interrupted recordings")
    }

    private func historyGrid(_ root: NSView) throws -> HistoryGridView {
        try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first?.documentView as? HistoryGridView)
    }

    private func key(_ code: UInt16, _ window: NSWindow) throws -> NSEvent {
        let characters: String
        switch code {
        case 53: characters = "\u{1b}"
        case 123: characters = "\u{F702}"
        case 124: characters = "\u{F703}"
        case 125: characters = "\u{F701}"
        default: characters = "\u{F700}"
        }
        return try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: characters,
            charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code))
    }

    private func capture(_ view: NSView, to url: URL) throws {
        view.window?.display(); view.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        XCTAssertTrue(condition(), "native history action did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}

private final class EmptyRecoveryWorker: RecordingRecoveryWorking {
    func list(historyRoot: String, completion: @escaping (Result<[RecordingRecoveryDraft], Error>) -> Void) {
        completion(.success([]))
    }
    func recover(historyRoot: String, draft: RecordingRecoveryDraft, cancel: NativeRecordingEditorCancel,
                 progress: @escaping (String) -> Void,
                 completion: @escaping (Result<RecordingRecoveryResult, Error>) -> Void) {
        XCTFail("History-only fixture must not recover a recording")
    }
    func discard(historyRoot: String, draft: RecordingRecoveryDraft,
                 completion: @escaping (Result<Void, Error>) -> Void) {
        XCTFail("History-only fixture must not discard a recording")
    }
}

/// Mini-preview image loading that can be switched to fail.
private final class RestoreImageLoader {
    private let lock = NSLock()
    private let image: NSImage
    private var failing = false

    init(_ image: NSImage) { self.image = image }
    func setFailing(_ value: Bool) { lock.lock(); failing = value; lock.unlock() }
    func load(_ path: String) throws -> NSImage {
        lock.lock(); defer { lock.unlock() }
        if failing { throw AppBridgeError.invalidResponse }
        return image
    }
}

private final class HistoryTransport: AppTransport {
    private let lock = NSLock()
    private var artifacts: [[String: Any]]
    private var clears = 0
    private var deletes: [String] = []
    private var blockedHistory: DispatchSemaphore?
    let blockedHistoryStarted = DispatchSemaphore(value: 0)
    private var failPartway: Bool
    private var saves = 0
    private var exportDirectory: String?

    init(path: String, width: Int, height: Int, failPartway: Bool,
         kinds: [String] = ["screenshot", "screenshot"], missing: Set<String> = []) {
        self.failPartway = failPartway
        artifacts = kinds.enumerated().map { index, kind in
            ["entry": ["id": "item-\(index)", "kind": kind, "width": width + index, "height": height,
                       "preview_url": "", "full_url": "", "size_bytes": 2_048,
                       "created_at": "2026-09-18T00:00:0\(kinds.count - index)Z"],
             "image_path": path, "preview_path": path, "missing": missing.contains("item-\(index)")]
        }
    }

    func remove(kind: String) {
        lock.lock(); defer { lock.unlock() }
        artifacts.removeAll { ($0["entry"] as? [String: Any])?["kind"] as? String == kind }
    }

    func promote(id: String) {
        lock.lock(); defer { lock.unlock() }
        for index in artifacts.indices {
            guard var entry = artifacts[index]["entry"] as? [String: Any], entry["id"] as? String == id else { continue }
            entry["created_at"] = "2026-09-19T00:00:00Z"; artifacts[index]["entry"] = entry
        }
    }

    var clearCount: Int { lock.lock(); defer { lock.unlock() }; return clears }
    var deletedIDs: [String] { lock.lock(); defer { lock.unlock() }; return deletes }
    var saveCount: Int { lock.lock(); defer { lock.unlock() }; return saves }
    var savedDirectory: String? { lock.lock(); defer { lock.unlock() }; return exportDirectory }

    func blockNextHistoryAfterClear() -> DispatchSemaphore {
        let gate = DispatchSemaphore(value: 0)
        lock.lock(); blockedHistory = gate; lock.unlock()
        return gate
    }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        switch object["operation"] as? String {
        case "history":
            if clears > 0, let blockedHistory {
                blockedHistoryStarted.signal()
                _ = blockedHistory.wait(timeout: .now() + 5)
                self.blockedHistory = nil
            }
            return ["kind": "history", "artifacts": artifacts]
        case "displays": return ["kind": "displays", "displays": []]
        case "delete":
            guard let id = object["id"] as? String else { throw AppBridgeError.invalidResponse }
            deletes.append(id)
            artifacts.removeAll { ($0["entry"] as? [String: Any])?["id"] as? String == id }
            return ["kind": "deleted", "id": id]
        case "save_recording":
            guard let directory = object["directory"] as? String,
                  let index = artifacts.firstIndex(where: { ($0["entry"] as? [String: Any])?["id"] as? String == object["id"] as? String }),
                  var entry = artifacts[index]["entry"] as? [String: Any],
                  entry["kind"] as? String == "gif" else { throw AppBridgeError.invalidResponse }
            let path = URL(fileURLWithPath: directory).appendingPathComponent("saved.gif").path
            entry["saved_path"] = path; artifacts[index]["entry"] = entry
            saves += 1; exportDirectory = directory
            return ["kind": "saved", "artifact": artifacts[index], "path": path]
        case "clear_history":
            clears += 1
            artifacts.removeFirst()
            if failPartway { failPartway = false; throw AppBridgeError.backend("fixture deletion failed") }
            artifacts.removeAll()
            return ["kind": "history", "artifacts": artifacts]
        default: throw AppBridgeError.invalidResponse
        }
    }
}
