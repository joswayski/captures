import AppKit
import XCTest
@testable import CapturesNative

final class StatusItemTests: XCTestCase {
    func testStatusMenuContainsOnlyShippingActionsAndRoutesThem() {
        var captures: [StillCaptureKind] = []
        var actions: [String] = []
        let target = LiveStatusActions(capture: { captures.append($0) },
            history: { actions.append("history") },
            preferences: { actions.append("preferences") },
            outputFolder: { actions.append("folder") },
            quit: { actions.append("quit") })

        let menu = target.makeMenu()
        XCTAssertEqual(menu.items.map(\.title), ["Screenshot Region", "Screenshot Window",
            "Screenshot Display", "", "Capture History…", "Open Save Location",
            "Preferences…", "", "Quit Captures"])
        XCTAssertFalse(menu.items.contains { $0.title.localizedCaseInsensitiveContains("record") })
        XCTAssertFalse(menu.items.contains { $0.title.localizedCaseInsensitiveContains("update") })

        target.captureRegion(); target.captureWindow(); target.captureDisplay()
        target.showHistory(); target.showPreferences(); target.openOutputFolder(); target.quit()
        XCTAssertEqual(captures, [.region, .window, .display])
        XCTAssertEqual(actions, ["history", "preferences", "folder", "quit"])
    }

    func testLiveRootCloseHidesWithoutClosingPreviewsOrTerminating() {
        _ = NSApplication.shared
        let root = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 500, height: 400),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        root.isReleasedWhenClosed = false
        defer { root.delegate = nil; root.close() }
        var previewCloses = 0
        var terminationRequests = 0
        let handler = RootWindowCloseHandler(rootWindow: root,
            closePreviews: { previewCloses += 1 },
            terminate: { terminationRequests += 1 }, hidesRootWindow: true)
        root.delegate = handler
        root.makeKeyAndOrderFront(nil)

        root.performClose(nil)

        XCTAssertFalse(root.isVisible)
        XCTAssertEqual(previewCloses, 0)
        XCTAssertEqual(terminationRequests, 0)
        withExtendedLifetime(handler) {}
    }

    func testCaptureOnlyRestoresAWorkspaceThatWasPreviouslyVisible() {
        var restoration = CaptureWindowRestoration()
        restoration.begin(windowIsVisible: false)
        XCTAssertFalse(restoration.finish(restoreRequested: true),
            "background status-item captures must remain backgrounded")

        restoration.begin(windowIsVisible: true)
        XCTAssertTrue(restoration.finish(restoreRequested: true))

        restoration.begin(windowIsVisible: true)
        XCTAssertFalse(restoration.finish(restoreRequested: false),
            "termination and scene teardown must override restoration")
        XCTAssertFalse(restoration.finish(restoreRequested: true),
            "finishing consumes the prior visibility state")
    }

    func testReopenFocusesVisibleWorkspaceOtherwiseShowsPreferences() {
        XCTAssertEqual(liveReopenAction(hasVisibleWindows: true), .focusExisting)
        XCTAssertEqual(liveReopenAction(hasVisibleWindows: false), .showPreferences)
    }

    func testQuitFlushesCancelsClosesAndDrainsBeforeCleanup() {
        var events: [String] = []
        performTermination(flushPreferences: { events.append("flush-preferences") },
            cancelCapture: { events.append("cancel-capture") },
            closePreviews: { events.append("close-previews") },
            drainActions: { events.append("drain-actions") },
            removeExerciseDirectory: { events.append("remove-exercise-directory") })

        XCTAssertEqual(events, ["flush-preferences", "cancel-capture", "close-previews",
            "drain-actions", "remove-exercise-directory"])
    }
}
