import AppKit
import XCTest
@testable import CapturesNative

final class UpdateNoticeTests: XCTestCase {
    func testSharedPresentationCopyForEveryFixture() throws {
        let model = UpdateNoticeModel()
        model.event = { _, _ in }
        for name in UpdateNoticeModel.fixtures {
            try model.load(fixture: name)
            XCTAssertFalse(try model.presentation().title.isEmpty, name)
        }
        XCTAssertThrowsError(try model.load(fixture: "unknown"))

        try model.load(fixture: "available")
        var p = try model.presentation()
        XCTAssertEqual(p.title, "Update available")
        XCTAssertEqual(p.description, "Version 2026.08.27.5 · 12.6 MB")
        XCTAssertEqual(p.notes?.stacked, true)
        XCTAssertEqual(p.notes?.groups.map(\.displayVersion), ["2026.08.27.5", "2026.08.27.3"])
        XCTAssertEqual(p.notes?.groups.first?.items.first?.pullNumber, 265)
        XCTAssertEqual(p.notes?.groups.first?.items.first?.pullURL, "https://github.com/joswayski/captures/pull/265")
        XCTAssertEqual(p.dismiss?.label, "Later")
        XCTAssertEqual(p.primary?.label, "Update now")
        XCTAssertNil(p.revealNotes)

        model.showChangelog = false
        p = try model.presentation()
        XCTAssertNil(p.notes)
        XCTAssertEqual(p.revealNotes?.label, "What’s new")
        XCTAssertEqual(p.cardHeight, 168)

        try model.load(fixture: "closing")
        XCTAssertEqual(try model.presentation().closeWarning,
            "Open captures will close. Unsaved edits are kept as drafts.")

        try model.load(fixture: "restarting")
        p = try model.presentation()
        XCTAssertEqual(p.title, "Updated")
        XCTAssertEqual(p.restartMessage, "Reopening in 3 seconds…")
        XCTAssertTrue(p.dismissBlocked)
        XCTAssertFalse(p.hasFooter)

        try model.load(fixture: "error")
        p = try model.presentation()
        XCTAssertEqual(p.title, "Update failed")
        XCTAssertEqual(p.primary?.label, "Try again")
        XCTAssertEqual(p.primary?.action, .install)
        XCTAssertEqual(p.error?.fallbackLink, "download from captur.es")
    }

    func testStubSourceSimulatesInstallWithoutSideEffectsAndBlocksEscape() throws {
        let model = UpdateNoticeModel()
        var events: [String] = []
        var persisted: [Bool] = []
        model.event = { name, _ in events.append(name) }
        model.persistShowChangelog = { persisted.append($0) }
        try model.load(fixture: "available")
        model.perform(.hideNotes)
        model.perform(.showNotes)
        XCTAssertEqual(persisted, [false, true])
        model.perform(.openPullRequest("https://github.com/joswayski/captures/pull/265"))
        XCTAssertTrue(events.contains("update-notice-open-url"))

        model.perform(.install)
        XCTAssertEqual(model.status?["state"] as? String, "downloading")
        XCTAssertEqual(model.tickInterval, 0.4)
        let download = try model.presentation().download
        XCTAssertEqual(download?.percent, 0)
        model.perform(.dismiss)
        XCTAssertTrue(model.visible, "Escape must not dismiss while downloading")
        XCTAssertTrue(events.contains("update-notice-dismiss-blocked"))

        var ticks = 0
        while model.status?["state"] as? String == "downloading", ticks < 20 { model.tick(); ticks += 1 }
        XCTAssertEqual(model.status?["state"] as? String, "restarting")
        model.tick(); model.tick(); model.tick()
        XCTAssertNil(model.status)
        XCTAssertFalse(model.visible)
        XCTAssertTrue(events.contains("update-notice-restart-simulated"))

        try model.load(fixture: "up-to-date")
        model.perform(.dismiss)
        XCTAssertFalse(model.visible)
    }

    func testTrayPlacementAndCoordinateConversion() throws {
        let model = UpdateNoticeModel()
        let monitor = CGRect(x: 0, y: 0, width: 1440, height: 900)
        let placed = try model.placement(monitor: monitor, workArea: monitor,
            tray: CGRect(x: 1200, y: 0, width: 24, height: 24), cardWidth: 400, cardHeight: 168)
        XCTAssertEqual(placed.caret, "top")
        XCTAssertEqual(placed.frame.minY, 22)
        XCTAssertEqual(placed.frame.minX + placed.caretX, 1212)
        XCTAssertEqual(placed.card.minX, 28)
        let fallback = try model.placement(monitor: monitor, workArea: monitor, tray: nil,
            cardWidth: 400, cardHeight: 168)
        XCTAssertEqual(fallback.caret, "none")
        let cocoa = UpdateNoticePlacement.cocoaFrame(CGRect(x: 10, y: 20, width: 100, height: 50), primaryHeight: 900)
        XCTAssertEqual(cocoa, NSRect(x: 10, y: 830, width: 100, height: 50))
        XCTAssertEqual(UpdateNoticePlacement.topLeft(cocoa, primaryHeight: 900), CGRect(x: 10, y: 20, width: 100, height: 50))
    }

    func testViewRendersEveryFixtureWithCaret() throws {
        let model = UpdateNoticeModel()
        let tokens = Tokens.variants["dark-mustard"]!
        let monitor = CGRect(x: 0, y: 0, width: 1440, height: 900)
        for name in UpdateNoticeModel.fixtures {
            try model.load(fixture: name)
            let p = try model.presentation()
            let placement = try model.placement(monitor: monitor, workArea: monitor,
                tray: CGRect(x: 1200, y: 0, width: 24, height: 24), cardWidth: p.cardWidth, cardHeight: p.cardHeight)
            let view = UpdateNoticeView(frame: NSRect(origin: .zero, size: placement.frame.size), tokens: tokens)
            var actions: [UpdateNoticeAction] = []
            view.onAction = { actions.append($0) }
            view.render(p, placement: placement)
            XCTAssertEqual(view.layer?.sublayers?.filter { $0.name == "update-caret" }.count, 1, name)
            let buttons = view.subviews.flatMap(\.subviews).compactMap { $0 as? CaptureButton }
            XCTAssertEqual(buttons.map(\.title), [p.dismiss?.label, p.primary?.label].compactMap { $0 }, name)
            buttons.last?.performClick(nil)
            if let primary = p.primary { XCTAssertEqual(actions.last, primary.action, name) }
        }
    }

    func testOptionsAcceptUpdateFixtureOnlyForTheUpdateScene() throws {
        let options = try Options(["--scene", "update", "--update-state", "error", "--update-tray", "none"])
        XCTAssertEqual(options.scene, "update")
        XCTAssertEqual(options.updateState, "error")
        XCTAssertEqual(options.updateTray, "none")
        XCTAssertThrowsError(try Options(["--update-state", "error"]))
        XCTAssertThrowsError(try Options(["--scene", "update", "--update-state", "unknown"]))
        XCTAssertThrowsError(try Options(["--scene", "update", "--exercise"]))
        XCTAssertThrowsError(try Options(["--live", "--scene", "update"]))
        XCTAssertEqual(Workbench.sceneTitle("update"), "Update notice")
    }
}
