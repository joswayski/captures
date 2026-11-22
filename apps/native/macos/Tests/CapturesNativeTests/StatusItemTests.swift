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

    func testAvailableAndInstallingUpdatesArePinnedFirstLikeShipping() {
        _ = NSApplication.shared
        var checks = 0
        let target = LiveStatusActions(newCapture: {}, capture: { _ in }, history: {}, preferences: {},
                                       checkUpdates: { checks += 1 }, outputFolder: {}, quit: {})
        target.updateItem = ("Update Available — 3 changes", true, true)
        var menu = target.makeMenu()
        XCTAssertEqual(menu.items.map(\.title), ["Update Available — 3 changes", "", "New Capture…",
            "Screenshot Region", "Screenshot Window", "Screenshot Display", "Record Region",
            "Record Window", "Record Display", "", "Capture History…", "Open Save Location",
            "Preferences", "Send Feedback…", "", "Quit Captures"])
        menu.performActionForItem(at: 0)
        XCTAssertEqual(checks, 1)
        target.updateItem = ("Installing Update…", false, true)
        menu = target.makeMenu()
        XCTAssertEqual(menu.items[0].title, "Installing Update…")
        XCTAssertFalse(menu.items[0].isEnabled)
        target.updateItem = ("Checking for Updates…", false, false)
        menu = target.makeMenu()
        XCTAssertEqual(menu.items[12].title, "Checking for Updates…", "an ordinary check stays in place")
        XCTAssertFalse(menu.items[12].isEnabled)
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

    func testStatusTemplateKeepsOnlyLightNeutralForegroundAndItsAlpha() throws {
        _ = NSApplication.shared
        let bitmap = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil,
            pixelsWide: 22, pixelsHigh: 22, bitsPerSample: 8, samplesPerPixel: 4,
            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
            bitmapFormat: .alphaNonpremultiplied, bytesPerRow: 22 * 4, bitsPerPixel: 32))
        let input = try XCTUnwrap(bitmap.bitmapData)
        for y in 0..<22 {
            for x in 0..<22 {
                // Explicit straight RGBA avoids NSBitmapImageRep.setColor's unsupported
                // grayscale-to-RGB conversion on the macOS 26 test runner.
                let rgba: [UInt8] = x < 6 ? [0, 0, 0, 255]
                    : x < 11 ? [255, 255, 255, y < 11 ? 128 : 64]
                    : x < 16 ? [255, 0, 0, 255] : [173, 173, 173, 255]
                let offset = y * bitmap.bytesPerRow + x * 4
                for channel in 0..<4 { input[offset + channel] = rgba[channel] }
            }
        }
        // Verify the fixture before invoking the conversion under test.
        XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 8, y: 10)).alphaComponent, 0.5, accuracy: 0.01)
        XCTAssertEqual(try XCTUnwrap(bitmap.colorAt(x: 8, y: 16)).alphaComponent, 0.25, accuracy: 0.01)
        let source = NSImage(size: NSSize(width: 22, height: 22))
        source.addRepresentation(bitmap)
        let template = try XCTUnwrap(statusItemTemplate(source: source))
        let pixels = try XCTUnwrap(template.representations.first as? NSBitmapImageRep)
        for x in [2, 13, 19] {
            XCTAssertEqual(try XCTUnwrap(pixels.colorAt(x: x, y: 10)).alphaComponent, 0,
                           "dark background, saturated color and sub-threshold gray are transparent")
        }
        let foreground = try XCTUnwrap(pixels.colorAt(x: 8, y: 10)?.usingColorSpace(.deviceRGB))
        XCTAssertEqual(foreground.alphaComponent, 0.5, accuracy: 0.01)
        XCTAssertEqual(foreground.redComponent, 1, accuracy: 0.01)
        let faint = try XCTUnwrap(pixels.colorAt(x: 8, y: 16)?.usingColorSpace(.deviceRGB))
        XCTAssertEqual(faint.alphaComponent, 0.25, accuracy: 0.01)
        XCTAssertEqual(faint.redComponent, 1, accuracy: 0.01)
        XCTAssertTrue(template.isTemplate)
    }

    func testStatusItemUsesProductTemplateIconAndCanEmitPixelEvidence() throws {
        _ = NSApplication.shared
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        defer { NSStatusBar.system.removeStatusItem(item) }
        let button = try XCTUnwrap(item.button)
        configureStatusItemButton(button)
        XCTAssertEqual(button.accessibilityLabel(), "Captures")
        let image = try XCTUnwrap(button.image, "the product resource must load, not use the fallback")
        XCTAssertTrue(image.isTemplate)
        XCTAssertEqual(image.size, NSSize(width: 22, height: 22))
        let pixels = try XCTUnwrap(image.representations.first as? NSBitmapImageRep)
        XCTAssertEqual(try XCTUnwrap(pixels.colorAt(x: 0, y: 0)).alphaComponent, 0)
        XCTAssertGreaterThan(try XCTUnwrap(pixels.colorAt(x: 11, y: 11)).alphaComponent, 0,
                             "the product's central spark is present")

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

    func testCaptureFromAnotherAppKeepsDocumentsBehindIt() throws {
        // Hand activation back to the session's frontmost app, so the test
        // leaves activation where it found it.
        guard let other = NSWorkspace.shared.frontmostApplication,
              other.processIdentifier != ProcessInfo.processInfo.processIdentifier else {
            throw XCTSkip("needs another frontmost app")
        }
        let editor = NSWindow(contentRect: NSRect(x: 40, y: 40, width: 240, height: 160),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        editor.isReleasedWhenClosed = false
        let preview = NSPanel(contentRect: NSRect(x: 300, y: 40, width: 120, height: 80),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        editor.orderFront(nil); preview.orderFrontRegardless()
        defer { editor.orderOut(nil); preview.orderOut(nil) }

        var handoff = CaptureActivationHandoff()
        handoff.begin(frontmost: other, capturesActive: true)
        XCTAssertNil(handoff.previousApp, "a capture started inside Captures keeps its focus")
        handoff.begin(frontmost: other, capturesActive: false)
        XCTAssertEqual(handoff.previousApp?.processIdentifier, other.processIdentifier)

        handoff.activate(windows: [editor, preview])
        XCTAssertFalse(editor.isVisible, "the capture surface must not raise a background editor")
        XCTAssertTrue(preview.isVisible, "floating previews are not documents")

        handoff.finish()
        XCTAssertTrue(editor.isVisible, "the editor comes back once the capture ends")
        XCTAssertNil(handoff.previousApp)
        XCTAssertTrue(handoff.concealed.isEmpty)
    }

    func testReopenFollowsShippingWindowPriorityOtherwiseShowsPreferences() {
        XCTAssertEqual(appReactivation(onboardingComplete: false, restoreRecordingControls: true,
                                       visible: [.history]), .showSetup)
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: true,
                                       visible: [.history]), .restoreRecordingControls)
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: false,
                                       visible: [.preferences, .history]), .focus(.history))
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: false,
                                       visible: [.preferences]), .focus(.preferences))
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: false,
                                       visible: []), .showPreferences)
    }

    func testReopenFocusesAnOpenEditorBeforeHistoryAndPreferences() {
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: false,
                                       visible: [.preferences, .history], editorVisible: true), .focusEditor)
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: false,
                                       visible: [], editorVisible: true), .focusEditor)
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: false,
                                       visible: [.setup, .history], editorVisible: true), .focus(.setup))
        XCTAssertEqual(appReactivation(onboardingComplete: false, restoreRecordingControls: false,
                                       visible: [], editorVisible: true), .showSetup)
        XCTAssertEqual(appReactivation(onboardingComplete: true, restoreRecordingControls: true,
                                       visible: [], editorVisible: true), .restoreRecordingControls)
        XCTAssertLessThan(AppWindowKind.setup.reactivationPriority, editorReactivationPriority)
        XCTAssertLessThan(editorReactivationPriority, AppWindowKind.history.reactivationPriority)
    }

    func testInteractiveLaunchFollowsShipping() {
        XCTAssertEqual(interactiveLaunch(onboardingComplete: false, launchedQuietly: false,
                                         openingFiles: false), .setup)
        XCTAssertEqual(interactiveLaunch(onboardingComplete: false, launchedQuietly: true,
                                         openingFiles: false), .setup)
        XCTAssertEqual(interactiveLaunch(onboardingComplete: true, launchedQuietly: true,
                                         openingFiles: false), .startupNotice)
        XCTAssertEqual(interactiveLaunch(onboardingComplete: true, launchedQuietly: false,
                                         openingFiles: false), .preferences)
        for complete in [false, true] {
            for quiet in [false, true] {
                XCTAssertNil(interactiveLaunch(onboardingComplete: complete, launchedQuietly: quiet,
                                               openingFiles: true))
            }
        }
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
        for focused in [false, true] {
            XCTAssertFalse(captureShortcutsSuspended(preferencesFocused: focused, recordingShortcut: false),
                "Preferences alone never suppresses capture shortcuts")
        }
        XCTAssertFalse(captureShortcutsSuspended(preferencesFocused: false, recordingShortcut: true),
            "blur restores registered shortcuts")
        XCTAssertTrue(captureShortcutsSuspended(preferencesFocused: true, recordingShortcut: true),
            "only an active focused recorder releases OS grabs")
        XCTAssertTrue(preferencesWindowFocused(visible: true, key: true, attachedSheetKey: false))
        XCTAssertTrue(preferencesWindowFocused(visible: true, key: false, attachedSheetKey: true))
        XCTAssertFalse(preferencesWindowFocused(visible: true, key: false, attachedSheetKey: false),
            "unfocused Preferences allows shortcuts")
        XCTAssertFalse(preferencesWindowFocused(visible: false, key: true, attachedSheetKey: false),
            "hidden Preferences allows shortcuts")
    }

    func testCaptureCanStartAndRepeatWhileGlobalShortcutsAreInstalled() throws {
        _ = NSApplication.shared
        XCTAssertTrue(Thread.isMainThread)
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("captures-shortcuts-\(UUID().uuidString).json").path
        var settings = try XCTUnwrap(SettingsBridge().request([
            "operation": "load", "path": path])["settings"] as? [String: Any])
        // Exercise real Carbon registration without taking over macOS's
        // screenshot keys or modifying the test user's settings.
        for (index, field) in ["new_capture_shortcut", "region_shortcut",
                               "window_shortcut", "display_shortcut"].enumerated() {
            settings[field] = "Control+Alt+Shift+F\(13 + index)"
        }
        var recording = try XCTUnwrap(settings["recording"] as? [String: Any])
        for (index, field) in ["video_shortcut", "window_shortcut", "display_shortcut"].enumerated() {
            recording[field] = "Control+Alt+Shift+F\(17 + index)"
        }
        settings["recording"] = recording
        var shortcuts = try NativeCaptureShortcuts(settings: settings)
        defer { shortcuts.close() }

        for attempt in 0..<3 {
            let response = try AppBridge.flow(["operation": "begin", "seconds": 0])
            let generation = try XCTUnwrap(response["generation"] as? NSNumber).uint64Value
            defer { _ = try? AppBridge.flow(["operation": "finish", "generation": generation]) }
            XCTAssertGreaterThan(generation, 0)
            // Preferences can suspend/resume its chords without releasing
            // capture Escape. A recreated host owner must coexist with it too.
            try shortcuts.setSuspended(true)
            try shortcuts.setSuspended(false)
            if attempt == 1 {
                shortcuts.close()
                shortcuts = try NativeCaptureShortcuts(settings: settings)
            }
        }
    }

    func testQuitInvocationLetsNestedRunLoopDrainMainQueueReplies() {
        _ = NSApplication.shared
        var insideCaller = false
        var replyReceived = false
        var invocations = 0
        let finished = expectation(description: "Quit receives its main-queue worker reply")
        DispatchQueue.main.async {
            insideCaller = true
            requestApplicationTermination {
                invocations += 1
                XCTAssertTrue(Thread.isMainThread)
                XCTAssertFalse(insideCaller, "Quit must not nest inside its initiating queue block")
                // Model terminateLater's nested wait without terminating XCTest.
                // DispatchQueue.main.async for Quit itself would starve this reply.
                DispatchQueue.main.async { replyReceived = true }
                let deadline = Date().addingTimeInterval(1)
                while !replyReceived && Date() < deadline {
                    RunLoop.current.run(until: Date().addingTimeInterval(0.01))
                }
                XCTAssertTrue(replyReceived, "the nested Quit loop must drain main-queue callbacks")
                finished.fulfill()
            }
            insideCaller = false
        }
        wait(for: [finished], timeout: 5)
        XCTAssertEqual(invocations, 1)
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
