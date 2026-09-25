import AppKit
import XCTest
@testable import CapturesNative

final class StartupNoticeTests: XCTestCase {
    private let monitor = NSRect(x: 0, y: 0, width: 1440, height: 900)
    private let workArea = NSRect(x: 0, y: 25, width: 1440, height: 800)

    func testTriggersMatchTheShippingLaunchRules() {
        XCTAssertEqual(startupNoticeTrigger(setupWasPresented: true, hiddenLaunch: false, openingMedia: true),
            .afterSetup, "completing setup always shows the notice")
        XCTAssertEqual(startupNoticeTrigger(setupWasPresented: false, hiddenLaunch: true, openingMedia: false),
            .quietLaunch)
        XCTAssertNil(startupNoticeTrigger(setupWasPresented: false, hiddenLaunch: true, openingMedia: true))
        XCTAssertNil(startupNoticeTrigger(setupWasPresented: false, hiddenLaunch: false, openingMedia: false))
        XCTAssertEqual(StartupNoticeTrigger.afterSetup.lifetime, 15)
        XCTAssertEqual(StartupNoticeTrigger.quietLaunch.lifetime, 5)
    }

    func testSharedPlacementAnchorsUnderTheMenuBarItem() throws {
        let layout = try XCTUnwrap(StartupNoticeLayout.resolve(monitor: monitor, workArea: workArea,
            tray: NSRect(x: 800, y: 0, width: 28, height: 24)))
        XCTAssertEqual(layout.caret, .top)
        XCTAssertEqual(layout.frame, NSRect(x: 814 - 176, y: 22, width: 352, height: 90))
        XCTAssertEqual(layout.caretX, 176)
        XCTAssertEqual(layout.card, NSRect(x: 28, y: 7, width: 296, height: 55))
        let caret = try XCTUnwrap(layout.caretTriangle)
        XCTAssertEqual(caret[0], NSPoint(x: 176, y: 0), "tip touches the menu bar item")
        XCTAssertEqual(caret[1].y, caret[2].y, "a flat-based triangle, not a rotated square")
        XCTAssertEqual(caret[2].x - caret[1].x, 12)
    }

    func testUnplacedStatusItemFallsBackWithoutACaret() throws {
        // AppKit can report the item at the Cocoa origin (bottom-left) before layout.
        let layout = try XCTUnwrap(StartupNoticeLayout.resolve(monitor: monitor, workArea: workArea,
            tray: NSRect(x: 0, y: 876, width: 28, height: 24)))
        XCTAssertEqual(layout.caret, .none)
        XCTAssertNil(layout.caretTriangle)
        XCTAssertEqual(layout.frame, NSRect(x: 1440 - 352 - 18, y: 35, width: 352, height: 110))
        let missing = try XCTUnwrap(StartupNoticeLayout.resolve(monitor: monitor, workArea: workArea, tray: nil))
        XCTAssertEqual(missing, layout)
    }

    func testAppKitCoordinatesFlipAtTheSharedBoundary() {
        // A menu bar item at the top of a 900pt primary display, in AppKit coordinates.
        let appKitItem = NSRect(x: 800, y: 876, width: 28, height: 24)
        XCTAssertEqual(StartupNoticeLayout.flip(appKitItem, primaryHeight: 900),
            NSRect(x: 800, y: 0, width: 28, height: 24))
        let secondary = NSRect(x: -1280, y: 900, width: 1280, height: 800)
        XCTAssertEqual(StartupNoticeLayout.flip(secondary, primaryHeight: 900),
            NSRect(x: -1280, y: -800, width: 1280, height: 800))
        let layout = StartupNoticeLayout.resolve(monitor: monitor, workArea: workArea,
            tray: StartupNoticeLayout.flip(appKitItem, primaryHeight: 900))!
        XCTAssertEqual(layout.appKitFrame(primaryHeight: 900),
            NSRect(x: 638, y: 900 - 22 - 90, width: 352, height: 90),
            "window hangs from the menu bar, overlapping the item by the caret overlap")
    }

    func testViewCarriesShippingCopyChipsAndCloseButton() throws {
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let layout = try XCTUnwrap(StartupNoticeLayout.resolve(monitor: monitor, workArea: workArea,
            tray: NSRect(x: 800, y: 0, width: 28, height: 24)))
        let view = StartupNoticeView(layout: layout, keys: ["⌘", "⇧", "Space"], tokens: tokens)
        XCTAssertEqual(view.accessibilityLabel(), "Captures is ready to use. Open New Capture with ⌘ ⇧ Space")
        let content = view.contentLayout()
        XCTAssertEqual(content.chips.count, 3)
        XCTAssertTrue(layout.card.contains(content.title))
        XCTAssertTrue(content.chips.allSatisfy { layout.card.contains($0) })
        XCTAssertGreaterThan(content.hint.minY, content.title.maxY - 0.5, "hint row sits below the title")
        XCTAssertEqual(abs(content.title.midX - layout.card.midX), 0, accuracy: 0.5)
        XCTAssertEqual(view.closeButton.accessibilityLabel(), "Close")
        XCTAssertEqual(view.closeButton.frame,
            NSRect(x: layout.card.maxX - 6 - 28, y: layout.card.midY - 14, width: 28, height: 28))
        // Light appearance still uses the fixed media glass.
        XCTAssertEqual(tokens.color("glass-strong-solid"),
            try XCTUnwrap(Tokens.variants["dark-mustard"]).color("glass-strong-solid"))
    }

    func testPanelIsNonactivatingFloatingAndDismissible() throws {
        _ = NSApplication.shared
        let controller = StartupNoticeController(tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]))
        let layout = try XCTUnwrap(StartupNoticeLayout.resolve(monitor: monitor, workArea: workArea,
            tray: NSRect(x: 800, y: 0, width: 28, height: 24)))
        controller.show(layout: layout, keys: ["⌘", "⇧", "Space"], lifetime: 60, primaryHeight: 900)
        let panel = try XCTUnwrap(controller.panel)
        XCTAssertEqual(panel.title, "Captures is running")
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertTrue(panel.styleMask.contains(.borderless))
        XCTAssertTrue(panel.becomesKeyOnlyIfNeeded)
        XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.noticeView.closeButton.acceptsFirstMouse(for: nil))
        XCTAssertFalse(panel.isOpaque)
        XCTAssertEqual(panel.level, .floating)
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllSpaces))
        XCTAssertTrue(panel.isVisible)
        XCTAssertFalse(panel.isKeyWindow)
        panel.noticeView.closeButton.performClick(nil)
        XCTAssertNil(controller.panel)
        XCTAssertFalse(panel.isVisible)
        XCTAssertFalse(controller.isVisible)
    }

    func testLifetimeExpiresAndStaleTimersCannotCloseAReplacement() throws {
        _ = NSApplication.shared
        let controller = StartupNoticeController(tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]))
        let layout = try XCTUnwrap(StartupNoticeLayout.resolve(monitor: monitor, workArea: workArea, tray: nil))
        controller.show(layout: layout, keys: [], lifetime: 0.05, primaryHeight: 900)
        XCTAssertTrue(controller.isVisible)
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        XCTAssertFalse(controller.isVisible, "the notice hides itself after its lifetime")

        controller.show(layout: layout, keys: [], lifetime: 0.05, primaryHeight: 900)
        controller.dismiss()
        controller.show(layout: layout, keys: [], lifetime: 60, primaryHeight: 900)
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        XCTAssertTrue(controller.isVisible, "a dismissed notice's timer cannot hide its replacement")
        controller.dismiss()
    }
}
