import AppKit
import XCTest
@testable import CapturesNative

final class StatusItemTests: XCTestCase {
    func testStatusMenuMatchesShippingOrderAndRoutesActions() {
        _ = NSApplication.shared
        var captures: [StillCaptureKind] = []
        var records: [UnifiedCaptureTarget] = []
        var actions: [String] = []
        let target = LiveStatusActions(newCapture: { actions.append("new") },
            capture: { captures.append($0) },
            record: { records.append($0) },
            history: { actions.append("history") },
            preferences: { actions.append("preferences") },
            feedback: { actions.append("feedback") },
            outputFolder: { actions.append("folder") },
            quit: { actions.append("quit") })
        target.shortcuts = ["CommandOrControl+Shift+Space", "Ctrl+Shift+F7", "Alt+KeyW",
                            "Shift+Digit4", "", "Nonsense+Key", "CmdOrCtrl+Alt+D"]

        let menu = target.makeMenu()
        XCTAssertEqual(menu.items.map(\.title), ["New Capture…", "Screenshot Region",
            "Screenshot Window", "Screenshot Display", "Record Region", "Record Window",
            "Record Display", "", "Capture History…", "Open Save Location", "Preferences",
            "Send Feedback…", "Check for Updates…", "", "Quit Captures"])
        XCTAssertFalse(menu.items[12].isEnabled, "updates are not connected yet")
        XCTAssertEqual(menu.items[0].keyEquivalent, " ")
        XCTAssertEqual(menu.items[0].keyEquivalentModifierMask, [.command, .shift])
        XCTAssertEqual(menu.items[2].keyEquivalent, "w")
        XCTAssertEqual(menu.items[2].keyEquivalentModifierMask, [.option])
        XCTAssertEqual(menu.items[3].keyEquivalent, "4")
        XCTAssertEqual(menu.items[4].keyEquivalent, "", "an empty shortcut has no key equivalent")
        XCTAssertEqual(menu.items[5].keyEquivalent, "", "an unknown key has no key equivalent")
        XCTAssertEqual(menu.items[6].keyEquivalentModifierMask, [.command, .option])

        for index in [0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 14] {
            menu.performActionForItem(at: index)
        }
        XCTAssertEqual(captures, [.region, .window, .display])
        XCTAssertEqual(records, [.region, .window, .display])
        XCTAssertEqual(actions, ["new", "history", "folder", "preferences", "feedback", "quit"])
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
        restoration.begin(windowIsVisible: false, windowIsKey: false)
        XCTAssertEqual(restoration.finish(restoreRequested: true), .none,
            "background status-item captures must remain backgrounded")

        restoration.begin(windowIsVisible: true, windowIsKey: false)
        XCTAssertEqual(restoration.finish(restoreRequested: true), .visible,
            "an unfocused visible workspace must reappear without activation")

        restoration.begin(windowIsVisible: true, windowIsKey: true)
        XCTAssertEqual(restoration.finish(restoreRequested: true), .key)

        restoration.begin(windowIsVisible: true, windowIsKey: true)
        XCTAssertEqual(restoration.finish(restoreRequested: false), .none,
            "termination and scene teardown must override restoration")
        XCTAssertEqual(restoration.finish(restoreRequested: true), .none,
            "finishing consumes the prior visibility state")
    }

    func testReopenFocusesVisibleWorkspaceOtherwiseShowsPreferences() {
        XCTAssertEqual(liveReopenAction(hasVisibleWindows: true), .focusExisting)
        XCTAssertEqual(liveReopenAction(hasVisibleWindows: false), .showPreferences)
    }

    func testShortcutHostPolicyUsesOnlyCaptureBindingsAndSuppressesBlockedScenes() {
        let settings: [String: Any] = ["new_capture_shortcut": "Command+Shift+Space",
            "region_shortcut": "Command+Shift+4",
            "window_shortcut": "Command+Shift+W", "display_shortcut": "Command+Shift+3",
            "recording": ["video_shortcut": "Control+F7", "window_shortcut": "Control+F8",
                "display_shortcut": "Control+F9"],
            "appearance": "dark"]
        XCTAssertEqual(captureShortcutSignature(settings),
            ["Command+Shift+Space", "Command+Shift+4", "Command+Shift+W", "Command+Shift+3",
                "Control+F7", "Control+F8", "Control+F9"])
        var unrelated = settings
        unrelated["appearance"] = "light"
        XCTAssertEqual(captureShortcutSignature(unrelated), captureShortcutSignature(settings))
        for field in ["video_shortcut", "window_shortcut", "display_shortcut"] {
            var edited = settings
            var recording = settings["recording"] as! [String: String]
            recording[field] = "Control+F12"
            edited["recording"] = recording
            XCTAssertNotEqual(captureShortcutSignature(edited), captureShortcutSignature(settings))
        }
        XCTAssertNil(stillCaptureKind(for: .newCapture))
        XCTAssertEqual(stillCaptureKind(for: .region), .region)
        XCTAssertEqual(stillCaptureKind(for: .window), .window)
        XCTAssertEqual(stillCaptureKind(for: .display), .display)
        for (wire, target) in [("record_region", UnifiedCaptureTarget.region),
                               ("record_window", .window), ("record_display", .display)] {
            let action = CaptureShortcut(rawValue: wire)
            XCTAssertEqual(action?.target, target)
            XCTAssertEqual(action?.mode, .record)
            XCTAssertNil(stillCaptureKind(for: action!))
        }
        XCTAssertTrue(captureShortcutsEnabled(captureBusy: false, selectorGeneration: nil))
        XCTAssertFalse(captureShortcutsEnabled(captureBusy: true, selectorGeneration: nil),
            "capture-busy suppression remains independent of registration suspension")
        XCTAssertTrue(captureShortcutsEnabled(captureBusy: true, selectorGeneration: nil,
            recordingControlsHidden: true),
            "only New Capture is routed while hidden controls need restoration")
        XCTAssertTrue(captureShortcutsEnabled(captureBusy: true, selectorGeneration: 42),
            "the exact active selector scope keeps target shortcuts enabled")
        XCTAssertFalse(captureShortcutsSuspended(preferencesFocused: false),
            "hidden or unfocused Preferences must restore registered shortcuts")
        XCTAssertTrue(captureShortcutsSuspended(preferencesFocused: true),
            "focused Preferences releases OS grabs before recorder input")
        XCTAssertTrue(preferencesWindowFocused(scene: "preferences", visible: true,
            key: true, attachedSheetKey: false))
        XCTAssertTrue(preferencesWindowFocused(scene: "preferences", visible: true,
            key: false, attachedSheetKey: true))
        XCTAssertFalse(preferencesWindowFocused(scene: "preferences", visible: true,
            key: false, attachedSheetKey: false), "unfocused Preferences allows shortcuts")
        XCTAssertFalse(preferencesWindowFocused(scene: "preferences", visible: false,
            key: true, attachedSheetKey: false), "hidden Preferences allows shortcuts")
        XCTAssertFalse(preferencesWindowFocused(scene: "live", visible: true,
            key: true, attachedSheetKey: false))
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
