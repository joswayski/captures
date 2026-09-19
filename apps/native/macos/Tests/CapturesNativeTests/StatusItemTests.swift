import AppKit
import XCTest
@testable import CapturesNative

final class StatusItemTests: XCTestCase {
    func testStatusMenuContainsOnlyShippingActionsAndRoutesThem() {
        _ = NSApplication.shared
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

        for index in [0, 1, 2, 4, 5, 6, 8] {
            menu.performActionForItem(at: index)
        }
        XCTAssertEqual(captures, [.region, .window, .display])
        XCTAssertEqual(actions, ["history", "folder", "preferences", "quit"])
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

    func testStatusItemUsesNativeTemplateIconAndCanEmitPixelEvidence() throws {
        _ = NSApplication.shared
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        defer { NSStatusBar.system.removeStatusItem(item) }
        let button = try XCTUnwrap(item.button)
        configureStatusItemButton(button)
        XCTAssertEqual(button.accessibilityLabel(), "Captures")
        XCTAssertTrue(button.image?.isTemplate == true || button.title == "C")

        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        button.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(button.bitmapImageRepForCachingDisplay(in: button.bounds))
        button.cacheDisplay(in: button.bounds, to: bitmap)
        let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let url = URL(fileURLWithPath: directory).appendingPathComponent("status-item-button.png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true)
        try data.write(to: url)
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

    func testShortcutHostPolicyUsesOnlyCaptureBindingsAndSuppressesBlockedScenes() {
        let settings: [String: Any] = ["region_shortcut": "Command+Shift+4",
            "window_shortcut": "Command+Shift+W", "display_shortcut": "Command+Shift+3",
            "appearance": "dark"]
        XCTAssertEqual(captureShortcutSignature(settings),
            ["Command+Shift+4", "Command+Shift+W", "Command+Shift+3"])
        var unrelated = settings
        unrelated["appearance"] = "light"
        XCTAssertEqual(captureShortcutSignature(unrelated), captureShortcutSignature(settings))
        XCTAssertEqual(stillCaptureKind(for: .region), .region)
        XCTAssertEqual(stillCaptureKind(for: .window), .window)
        XCTAssertEqual(stillCaptureKind(for: .display), .display)
        XCTAssertTrue(captureShortcutsEnabled(scene: "live", captureBusy: false))
        XCTAssertFalse(captureShortcutsEnabled(scene: "preferences", captureBusy: false))
        XCTAssertFalse(captureShortcutsEnabled(scene: "live", captureBusy: true))
    }

    func testQuitFlushesCancelsClosesAndDrainsBeforeCleanup() {
        var events: [String] = []
        performTermination(flushPreferences: { events.append("flush-preferences") },
            cancelCapture: { events.append("cancel-capture") },
            closeShortcuts: { events.append("close-shortcuts") },
            closePreviews: { events.append("close-previews") },
            drainActions: { events.append("drain-actions") },
            removeExerciseDirectory: { events.append("remove-exercise-directory") })

        XCTAssertEqual(events, ["flush-preferences", "cancel-capture", "close-shortcuts",
            "close-previews", "drain-actions", "remove-exercise-directory"])
    }
}
