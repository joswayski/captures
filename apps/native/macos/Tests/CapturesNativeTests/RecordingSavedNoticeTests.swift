import AppKit
import XCTest
@testable import CapturesNative

final class RecordingSavedNoticeTests: XCTestCase {
    func testSaveGatingErrorRetryAndActualOutputPath() {
        let model = RecordingSavedNoticeModel()
        let firstGeneration = model.present(artifactID: "recording-42")
        let request = model.beginSave()
        XCTAssertEqual(request?.generation, firstGeneration)
        XCTAssertEqual(request?.artifactID, "recording-42")
        XCTAssertNil(model.beginSave(), "a pending save must not be sent twice")
        model.expire(generation: firstGeneration)
        XCTAssertEqual(model.state, .saving, "expiry cannot hide a pending save")

        model.finishSave(generation: firstGeneration,
            result: .failure(AppBridgeError.backend("Folder is unavailable")))
        XCTAssertEqual(model.state, .error(message: "Folder is unavailable", retry: .save))
        XCTAssertEqual(model.beginSave()?.artifactID, "recording-42")
        model.finishSave(generation: firstGeneration, result: .success("/Exports/Captures/actual.mp4"))
        XCTAssertEqual(model.state, .saved(path: "/Exports/Captures/actual.mp4"))
        XCTAssertEqual(model.beginReveal()?.path, "/Exports/Captures/actual.mp4")
    }

    func testMissingPathIsAnErrorAndCanRetry() {
        let model = RecordingSavedNoticeModel()
        let generation = model.present(artifactID: "recording")
        _ = model.beginSave()
        model.finishSave(generation: generation, result: .success("  "))
        guard let state = model.state, case .error(let message, .save) = state else {
            return XCTFail("missing paths must not report success")
        }
        XCTAssertTrue(message.contains("without a file path"))
        XCTAssertNotNil(model.beginSave())
    }

    func testReplacementDismissalAndTimeoutRejectStaleCompletion() {
        let model = RecordingSavedNoticeModel()
        let old = model.present(artifactID: "old")
        _ = model.beginSave()
        let replacement = model.present(artifactID: "new")
        model.finishSave(generation: old, result: .success("/old.mp4"))
        XCTAssertEqual(model.artifactID, "new")
        XCTAssertEqual(model.state, .ready)
        model.expire(generation: old)
        XCTAssertEqual(model.state, .ready, "an old timeout cannot dismiss a replacement")

        model.expire(generation: replacement)
        XCTAssertNil(model.state, "expiry dismisses the current notice")
        let dismissedGeneration = model.generation
        XCTAssertGreaterThan(dismissedGeneration, replacement)

        model.finishSave(generation: replacement, result: .success("/new.mp4"))
        XCTAssertNil(model.state, "a completion cannot reopen a dismissed notice")
        XCTAssertNil(model.artifactID, "dismissal does not mutate or retain artifact metadata")
    }

    func testRevealFailureRetriesAndSuccessOnlyDismissesNotice() {
        let model = RecordingSavedNoticeModel()
        let generation = model.present(artifactID: "kept-in-history")
        _ = model.beginSave()
        model.finishSave(generation: generation, result: .success("/saved/movie.mp4"))
        let reveal = model.beginReveal()
        model.finishReveal(generation: reveal!.generation, path: reveal!.path, succeeded: false)
        XCTAssertEqual(model.artifactID, "kept-in-history")
        XCTAssertEqual(model.beginReveal()?.path, "/saved/movie.mp4")
        model.finishReveal(generation: model.generation, path: "/saved/movie.mp4", succeeded: true)
        XCTAssertNil(model.state)
        XCTAssertEqual(RecordingSavedNoticeController.lifetime, 15.2)
    }

    func testHistorySaveUpdatesOnlyTheMatchingVisibleNotice() {
        let model = RecordingSavedNoticeModel()
        model.present(artifactID: "new")
        XCTAssertFalse(model.markSaved(artifactID: "old", path: "/old.mp4"))
        XCTAssertEqual(model.state, .ready)
        XCTAssertTrue(model.markSaved(artifactID: "new", path: "/new.mp4"))
        XCTAssertEqual(model.state, .saved(path: "/new.mp4"))
        model.dismiss()
        XCTAssertFalse(model.markSaved(artifactID: "new", path: "/new.mp4"))
        XCTAssertNil(model.state)
    }

    func testReadySavedAndErrorFixturesFitInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            let view = RecordingSavedNoticeView(
                frame: NSRect(x: 0, y: 0, width: 440, height: 116), tokens: tokens)
            let window = NSWindow(contentRect: view.bounds, styleMask: [.borderless],
                backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false; window.contentView = view
            defer { window.close() }
            XCTAssertTrue(view.subviews.allSatisfy { view.bounds.contains($0.frame) })
            XCTAssertEqual(view.primaryButton.accessibilityLabel(), "Save recording file")
            try render(view, window: window, name: "recording-ready-notice-\(appearance)-ready")
            view.update(.saved(path: "/Captures/movie.mp4"))
            XCTAssertEqual(view.primaryButton.title, "Show in Folder")
            try render(view, window: window, name: "recording-ready-notice-\(appearance)-saved")
            view.update(.error(message: "The Captures folder could not be reached. Choose another folder and retry.", retry: .save))
            XCTAssertEqual(view.primaryButton.title, "Retry save")
            try render(view, window: window, name: "recording-ready-notice-\(appearance)-error")
        }
    }

    func testNoticeIsOneRowOfCheckTileCopyAndIconButtonWithAQuietDismiss() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let view = RecordingSavedNoticeView(frame: NSRect(x: 0, y: 0, width: 440, height: 116), tokens: tokens)
        let padding = tokens.number("s-5")
        XCTAssertEqual(view.tile.frame, NSRect(x: padding, y: 39, width: 38, height: 38))
        XCTAssertEqual(view.tile.glyph, "check")
        XCTAssertFalse(view.subviews.contains { ($0 as? NSTextField)?.stringValue == "●" },
                       "the check tile replaces the text glyph")
        for state: RecordingSavedNoticeState in [.ready, .saved(path: "/Captures/movie.mp4"),
                                                 .error(message: "Folder is unavailable", retry: .save)] {
            view.update(state)
            let button = view.primaryButton.frame
            XCTAssertEqual(button.midY, view.bounds.midY, accuracy: 0.5, "one row: the button is centred")
            XCTAssertEqual(button.maxX, view.bounds.width - 38, accuracy: 0.5)
            XCTAssertEqual(button.height, tokens.number("h-md"))
            let copy = view.subviews.compactMap { $0 as? NSTextField }
            for label in copy {
                XCTAssertGreaterThanOrEqual(label.frame.minX, view.tile.frame.maxX + padding - 0.5)
                XCTAssertLessThanOrEqual(label.frame.maxX, button.minX - padding + 0.5,
                                         "\(label.stringValue) stays left of the button")
            }
            guard case .shipping(let name)? = view.primaryButton.icon else {
                return XCTFail("the action button leads with a shipping icon")
            }
            XCTAssertEqual(name, state == .ready || state == .error(message: "Folder is unavailable", retry: .save)
                ? "save" : "folder")
        }
        XCTAssertEqual(view.dismissButton.frame, NSRect(x: 406, y: 10, width: 24, height: 24))
        XCTAssertTrue(view.dismissButton.iconOnly)
        guard case .shipping("close")? = view.dismissButton.icon else {
            return XCTFail("dismiss is the shipping ×, not a Dismiss text button")
        }
        XCTAssertEqual(view.dismissButton.accessibilityLabel(), "Dismiss recording notice")
    }

    func testPanelIsTopRightNonactivatingAndNeverShared() throws {
        _ = NSApplication.shared
        let screen = try XCTUnwrap(NSScreen.main)
        let panel = RecordingSavedNoticePanel(screen: screen,
            tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]))
        defer { panel.close() }
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertFalse(panel.hidesOnDeactivate)
        XCTAssertEqual(panel.sharingType, .none)
        XCTAssertEqual(panel.frame.size, NSSize(width: 440, height: 116))
        XCTAssertEqual(panel.frame.maxX, screen.visibleFrame.maxX - 20, accuracy: 0.1)
        XCTAssertEqual(panel.frame.maxY, screen.visibleFrame.maxY - 20, accuracy: 0.1)
    }

    private func render(_ view: NSView, window: NSWindow, name: String) throws {
        window.display(); view.layoutSubtreeIfNeeded()
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("\(name).png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }
}
