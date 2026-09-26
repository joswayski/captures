import AppKit
import XCTest
import CCapturesSettings
@testable import CapturesNative

private final class CaptureMenuSettingsTransport: SettingsTransport {
    func request(_ object: [String: Any]) throws -> [String: Any] {
        switch object["operation"] as? String {
        case "load": return ["ok": true, "settings": ["appearance": "dark", "theme": "mustard"]]
        case "save": return ["ok": true, "settings": object["settings"] as? [String: Any] ?? [:]]
        default: return ["ok": true, "path": "/fixture/settings.json"]
        }
    }
}

final class CaptureControlsTests: XCTestCase {
    private let frame = NSRect(x: 0, y: 0, width: 1000, height: 720)
    private let targets = [
        WindowSelectionTarget(id: "front", title: "Draft", appName: "Editor",
            rect: NSRect(x: 72, y: 64, width: 492, height: 332), cornerRadius: 24),
        WindowSelectionTarget(id: "back", title: "Reference", appName: "Browser",
            rect: NSRect(x: 406, y: 176, width: 478, height: 372), cornerRadius: 10),
    ]

    func testGuidanceAndNoteUseShippingCopy() {
        XCTAssertEqual(CaptureGuidanceCopy.directHint(CaptureGuidanceCopy.regionTitle,
            CaptureGuidanceCopy.regionHint, confirm: false),
            "Drag to select a region · Shift for square · Esc to cancel")
        XCTAssertEqual(CaptureGuidanceCopy.directHint(CaptureGuidanceCopy.windowTitle,
            CaptureGuidanceCopy.hint, confirm: true),
            "Select a window to continue · Esc to cancel · Press Enter to confirm")
        XCTAssertEqual(CaptureGuidanceCopy.displayTitle, "Click to capture this display")
        let state = RecordingControlState(framesPerSecond: 60, maxResolution: "original",
            showCursor: true, highlightClicks: false, systemAudio: false, microphoneDeviceID: nil)
        let screenshot = try? CaptureMenuPolicy.menu(mode: .screenshot, autoStart: false,
            canExcludeControls: true, controlsExcluded: true, state: state, availability: nil)
        XCTAssertEqual(screenshot?.note.text, "These controls won’t show in screenshots")
        XCTAssertEqual(screenshot?.note.emphasis, "won’t")
        XCTAssertEqual(screenshot?.note.setting, "include_recording_controls_in_captures")
        XCTAssertEqual(screenshot?.confirmText, "Press Enter to confirm")
        XCTAssertNil(screenshot?.confirmSetting)
        XCTAssertEqual(screenshot?.primaryLabel, "Capture")
        XCTAssertEqual(screenshot?.primaryAccessibilityLabel, "Take screenshot")
        XCTAssertEqual(screenshot?.primaryHidden, false)
        let recording = try? CaptureMenuPolicy.menu(mode: .record, autoStart: true,
            canExcludeControls: true, controlsExcluded: false, error: true, state: state,
            availability: RecordingControlAvailability(cursor: true, clicks: false,
                systemAudio: true, microphone: true))
        XCTAssertEqual(recording?.note.text, "These controls will show in recordings")
        XCTAssertEqual(recording?.confirmText,
            "Auto-capture is on. Selecting a target starts immediately.")
        XCTAssertEqual(recording?.confirmSetting, "auto_start_on_selection")
        XCTAssertEqual(recording?.primaryLabel, "Retry recording")
        XCTAssertEqual(recording?.primaryHidden, false, "a failed auto-start shows Retry")
        XCTAssertEqual(recording?.toggleStatus["show_cursor"], "On")
        XCTAssertEqual(recording?.toggleStatus["highlight_clicks"], "Unavailable")
        XCTAssertEqual(recording?.toggleStatus["system_audio"], "Off")
        let linux = try? CaptureMenuPolicy.menu(mode: .record, autoStart: false,
            canExcludeControls: false, controlsExcluded: false, state: state, availability: nil)
        XCTAssertEqual(linux?.note.text,
            "These controls will show in recordings · Use Hide controls to keep them out")
        XCTAssertNil(linux?.note.setting, "a platform that cannot exclude controls shows plain text")
        XCTAssertEqual(CaptureMenuPolicy.copy.fpsOptions, [60, 30, 15])
        XCTAssertEqual(CaptureMenuPolicy.copy.toggles.map(\.label),
            ["Show cursor", "Show clicks", "Desktop audio"])
        XCTAssertEqual(CaptureMenuPolicy.copy.highlightSeconds, 2.4, accuracy: 0.001)
        let coupled = try? CaptureMenuPolicy.coupled(changed: "highlight_clicks",
            showCursor: false, highlightClicks: true)
        XCTAssertEqual(coupled?.showCursor, true)
        let identity = try? CaptureMenuPolicy.displayIdentity(name: " ", width: 1512, height: 982,
            recordingFPS: 30)
        XCTAssertEqual(identity?.name, "Display")
        XCTAssertEqual(identity?.detail, "1512 × 982 · 30 FPS")
        let chip = NSRect(x: 100, y: 40, width: 200, height: 50)
        XCTAssertTrue(CaptureMenuPolicy.pointerOverGuidance(NSPoint(x: 72, y: 50), chip: chip,
            currentlyOver: false))
        XCTAssertFalse(CaptureMenuPolicy.pointerOverGuidance(NSPoint(x: 71, y: 50), chip: chip,
            currentlyOver: false))
        XCTAssertTrue(CaptureMenuPolicy.pointerOverGuidance(NSPoint(x: 61, y: 50), chip: chip,
            currentlyOver: true))
    }

    func testTargetSwitchesRetainSettledRegionAndWindowIndependently() throws {
        _ = NSApplication.shared
        var confirmed: [WindowSelectionChoice] = []
        let view = makeView(confirm: { confirmed.append($0) })

        XCTAssertEqual(view.target, .region)
        XCTAssertNil(view.choice, "an empty region must keep Capture disabled")
        let capture = try XCTUnwrap(buttons(in: view).first { $0.title == "Capture" })
        XCTAssertFalse(capture.isEnabled)

        view.beginRegion(NSPoint(x: 91, y: 77))
        XCTAssertFalse(view.isGuidanceVisible,
            "shipping guidance hides while a region gesture is active")
        view.dragRegion(NSPoint(x: 432, y: 268))
        view.endRegion()
        XCTAssertTrue(view.isGuidanceVisible)
        let region = try XCTUnwrap(view.choice)
        guard case .region(let rect) = region else { return XCTFail("expected region") }
        XCTAssertEqual(rect.x, 91); XCTAssertEqual(rect.y, 77)
        XCTAssertEqual(rect.width, 341); XCTAssertEqual(rect.height, 191)
        XCTAssertTrue(capture.isEnabled)

        view.setTarget(.window)
        XCTAssertNil(view.choice, "a region is not a window selection")
        view.selectWindow(NSPoint(x: 24, y: 40))
        XCTAssertEqual(view.choice, .window(index: 1, id: "back"))
        view.setTarget(.region)
        XCTAssertEqual(view.choice, region, "explicit target toggles retain the settled region")
        view.setTarget(.window)
        XCTAssertEqual(view.choice, .window(index: 1, id: "back"),
            "explicit target toggles retain the selected window")
        view.confirmSelection()
        XCTAssertEqual(confirmed, [.window(index: 1, id: "back")])
        view.setTarget(.display)
        XCTAssertEqual(view.choice, .display)
    }

    func testDisplayReplacementPreservesControlsButClearsDisplayLocalSelections() {
        _ = NSApplication.shared
        let initial = makeView()
        XCTAssertEqual(initial.controlsState, .initial)

        initial.setAspect(4)
        initial.beginRegion(NSPoint(x: 90, y: 70))
        initial.dragRegion(NSPoint(x: 410, y: 250)); initial.endRegion()
        let regionState = initial.controlsState
        XCTAssertNotNil(initial.choice)

        let replacementRegion = makeView()
        replacementRegion.restoreControls(regionState)
        XCTAssertEqual(replacementRegion.target, .region)
        XCTAssertEqual(replacementRegion.aspectIndex, 4)
        XCTAssertNil(replacementRegion.choice,
            "an actual display change clears the old display's region")

        initial.setTarget(.window)
        initial.selectWindow(NSPoint(x: 24, y: 40))
        let windowState = initial.controlsState
        XCTAssertNotNil(initial.choice)
        let replacementWindow = makeView()
        replacementWindow.restoreControls(windowState)
        XCTAssertEqual(replacementWindow.target, .window)
        XCTAssertEqual(replacementWindow.aspectIndex, 4)
        XCTAssertNil(replacementWindow.choice,
            "an actual display change clears the old display's window")

        var confirmed: [WindowSelectionChoice] = []
        let automaticDisplay = makeView(autoStart: true, confirm: { confirmed.append($0) })
        automaticDisplay.restoreControls(
            UnifiedCaptureControlsState(mode: .screenshot, target: .display, aspectIndex: 4))
        XCTAssertEqual(automaticDisplay.target, .display)
        XCTAssertEqual(confirmed, [.display],
            "display auto-capture is scheduled against the replacement session")
    }

    func testShellOrEmptyDesktopClickSelectsFullScreenSegment() {
        _ = NSApplication.shared
        let view = makeView()
        view.setTarget(.window)

        view.selectWindow(NSPoint(x: 700, y: 400))

        XCTAssertEqual(view.target, .display)
        XCTAssertEqual(view.controls.target, .display)
        XCTAssertEqual(view.choice, .display)
    }

    func testKeyboardTargetSwitchesMatchShippingSelectionAndAutoStartBoundaries() {
        _ = NSApplication.shared
        var confirmed: [WindowSelectionChoice] = []
        let view = makeView(autoStart: true, confirm: { confirmed.append($0) })

        view.setTargetFromShortcut(.display)
        XCTAssertEqual(view.target, .display)
        XCTAssertTrue(confirmed.isEmpty,
            "a target shortcut changes mode but never arms automatic capture")

        view.setTarget(.window)
        view.selectWindow(NSPoint(x: 24, y: 40))
        XCTAssertEqual(confirmed, [.window(index: 1, id: "back")])
        confirmed.removeAll()
        XCTAssertTrue(view.hoverWindow(NSPoint(x: 100, y: 240)))
        XCTAssertGreaterThanOrEqual(view.hoveredWindowIndex, 0)
        view.setTarget(.region)
        XCTAssertEqual(view.selectedWindowIndex, 1,
            "pointer target buttons retain the settled window")

        view.setTargetFromShortcut(.window)
        XCTAssertEqual(view.selectedWindowIndex, 1,
            "the Window shortcut retains the settled window")
        XCTAssertEqual(view.hoveredWindowIndex, -1)
        XCTAssertTrue(confirmed.isEmpty)

        view.setTargetFromShortcut(.region)
        XCTAssertNil(view.selectedWindowIndex)
        XCTAssertEqual(view.hoveredWindowIndex, -1)
        view.setAspect(4)
        view.beginRegion(NSPoint(x: 90, y: 70))
        view.dragRegion(NSPoint(x: 410, y: 250)); view.endRegion()
        XCTAssertEqual(confirmed.count, 1,
            "pointer selection still honors automatic capture")
        confirmed.removeAll()
        let region = view.region.rect

        view.setTarget(.window)
        view.selectWindow(NSPoint(x: 24, y: 40))
        XCTAssertEqual(view.selectedWindowIndex, 1)
        confirmed.removeAll()

        view.setTargetFromShortcut(.display)
        XCTAssertNil(view.selectedWindowIndex)
        XCTAssertEqual(view.hoveredWindowIndex, -1)
        XCTAssertEqual(view.region.rect.x, region.x)
        XCTAssertEqual(view.region.rect.y, region.y)
        XCTAssertEqual(view.region.rect.width, region.width)
        XCTAssertEqual(view.region.rect.height, region.height,
            "keyboard target changes retain the settled region and aspect")
        XCTAssertEqual(view.aspectIndex, 4)
        XCTAssertTrue(confirmed.isEmpty,
            "the Display shortcut must not itself trigger automatic capture")
        for target in UnifiedCaptureTarget.allCases {
            view.setTargetFromShortcut(target, mode: .record)
            XCTAssertEqual(view.mode, .record)
            XCTAssertEqual(view.target, target)
            XCTAssertEqual(view.region.rect.x, region.x)
            XCTAssertEqual(view.region.rect.y, region.y)
            XCTAssertEqual(view.region.rect.width, region.width)
            XCTAssertEqual(view.region.rect.height, region.height)
            XCTAssertEqual(view.aspectIndex, 4)
            XCTAssertTrue(confirmed.isEmpty, "recording keys must not auto-start")
        }
        view.setTargetFromShortcut(.region)
        XCTAssertEqual(view.mode, .screenshot, "screenshot keys leave Record mode")
        XCTAssertTrue(confirmed.isEmpty)
    }

    func testEnterEscapeAndAutoStartRespectSelectionBoundaries() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: frame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        var confirmed: [WindowSelectionChoice] = []
        var cancelled = 0
        let manual = makeView(confirm: { confirmed.append($0) }, cancel: { cancelled += 1 })
        window.contentView = manual
        manual.keyDown(with: try keyEvent(window: window, keyCode: 36, characters: "\r"))
        XCTAssertTrue(confirmed.isEmpty)
        manual.beginRegion(NSPoint(x: 120, y: 92)); manual.dragRegion(NSPoint(x: 380, y: 246))
        manual.endRegion()
        manual.keyDown(with: try keyEvent(window: window, keyCode: 36, characters: "\r"))
        XCTAssertEqual(confirmed.count, 1)
        manual.keyDown(with: try keyEvent(window: window, keyCode: 53, characters: "\u{1b}"))
        XCTAssertEqual(cancelled, 1)

        confirmed.removeAll()
        let automatic = makeView(autoStart: true, confirm: { confirmed.append($0) })
        automatic.beginRegion(NSPoint(x: 80, y: 60))
        automatic.dragRegion(NSPoint(x: 81, y: 61)); automatic.endRegion()
        XCTAssertTrue(confirmed.isEmpty, "a sub-minimum drag cannot auto-capture")
        automatic.beginRegion(NSPoint(x: 80, y: 60))
        automatic.dragRegion(NSPoint(x: 320, y: 210))
        XCTAssertTrue(confirmed.isEmpty, "auto-start waits for pointer release")
        automatic.endRegion()
        XCTAssertEqual(confirmed.count, 1)

        confirmed.removeAll(); automatic.setTarget(.window)
        automatic.selectWindow(NSPoint(x: 24, y: 40))
        XCTAssertEqual(confirmed, [.window(index: 1, id: "back")])
        confirmed.removeAll(); automatic.setTarget(.display)
        XCTAssertEqual(confirmed, [.display])
    }

    func testPreparationGateRejectsCancelledAndSupersededResults() {
        var gate = CapturePreparationGate()
        let first = gate.begin()
        XCTAssertTrue(gate.accepts(first))
        let replacement = gate.begin()
        XCTAssertFalse(gate.accepts(first), "a display switch must reject its stale preparation")
        XCTAssertTrue(gate.accepts(replacement))
        gate.invalidate()
        XCTAssertFalse(gate.accepts(replacement), "cancel must prevent a late preparation from reopening controls")
    }

    func testRecordModeFollowsShippingAutoStartAndPreservesLogicalRegionCoordinates() throws {
        _ = NSApplication.shared
        var confirmed: [WindowSelectionChoice] = []
        let automatic = makeView(autoStart: true, confirm: { confirmed.append($0) })
        automatic.setMode(.record)
        XCTAssertTrue(automatic.controls.primaryHidden,
            "shipping auto-start hides Start recording as well as Capture")
        automatic.beginRegion(NSPoint(x: 100.4, y: 49.6))
        automatic.dragRegion(NSPoint(x: 400, y: 300)); automatic.endRegion()
        XCTAssertEqual(confirmed.count, 1, "shipping auto-start applies to Record too")
        confirmed.removeAll()
        let tray = makeView(autoStart: true, confirm: { confirmed.append($0) })
        tray.restoreControls(UnifiedCaptureControlsState(mode: .record, target: .display,
            aspectIndex: 0), armAutoStart: false)
        XCTAssertEqual(tray.target, .display)
        XCTAssertTrue(confirmed.isEmpty, "a tray Record Full Screen request never auto-starts")
        tray.confirmSelection()
        XCTAssertEqual(confirmed, [.display], "Enter or a desktop click still starts it")

        confirmed.removeAll()
        let view = makeView(confirm: { confirmed.append($0) })
        view.setMode(.record)
        view.beginRegion(NSPoint(x: 100.4, y: 49.6))
        view.dragRegion(NSPoint(x: 900.2, y: 499.7)); view.endRegion()
        XCTAssertTrue(confirmed.isEmpty, "without auto-start Record waits for confirmation")
        let start = try XCTUnwrap(buttons(in: view.controls).first {
            $0.accessibilityLabel() == "Start recording"
        })
        XCTAssertFalse(start.isHidden)
        XCTAssertEqual(start.title, "Start recording")
        view.confirmSelection()
        XCTAssertEqual(confirmed.count, 1)

        let target = try nativeRecordingTarget(try XCTUnwrap(view.choice), displayID: "-1440")
        let rect = try XCTUnwrap(target["rect"] as? [String: Any])
        XCTAssertEqual(target["display_id"] as? String, "-1440")
        XCTAssertEqual(rect["x"] as? Int, 100)
        XCTAssertEqual(rect["y"] as? Int, 50)
        XCTAssertEqual(rect["width"] as? Int, 800)
        XCTAssertEqual(rect["height"] as? Int, 450,
            "2x and negative-origin displays still cross the bridge in display-local logical units")
    }

    func testRecordingControlsRespectCapabilitiesAndRenderNarrowly() throws {
        _ = NSApplication.shared
        let narrowFrame = NSRect(x: 0, y: 0, width: 768, height: 600)
        let window = NSWindow(contentRect: narrowFrame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false; defer { window.close() }
        let view = UnifiedCaptureSelectionView(frame: narrowFrame, image: nil, targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: true,
            hitTest: { _ in 0 }, displayTitles: ["Main display · 768 × 600"],
            selectedDisplay: 0, confirm: { _ in }, cancel: {}, changeDisplay: { _ in },
            recordingState: RecordingControlState(framesPerSecond: 60, maxResolution: "original",
                showCursor: true, highlightClicks: true, systemAudio: true,
                microphoneDeviceID: "stored-mic"),
            recordingAvailability: RecordingControlAvailability(cursor: true, clicks: false,
                systemAudio: true, microphone: true),
            microphoneDevices: [
                NativeMicrophoneDevice(["id": "built-in", "name": "MacBook Microphone",
                    "is_default": true])!,
            ])
        window.contentView = view; view.setMode(.record)
        let switches = recordingSwitches(in: view.controls)
        let cursor = try XCTUnwrap(switches.first { $0.accessibilityLabel() == "Show cursor" })
        let clicks = try XCTUnwrap(switches.first { $0.accessibilityLabel() == "Show clicks" })
        let audio = try XCTUnwrap(switches.first { $0.accessibilityLabel() == "Record desktop audio" })
        XCTAssertTrue(cursor.isEnabled); XCTAssertEqual(cursor.status, "On")
        XCTAssertFalse(clicks.isEnabled); XCTAssertEqual(clicks.status, "Unavailable")
        XCTAssertEqual(clicks.toolTip, "Click highlights are unavailable in this desktop session")
        XCTAssertTrue(audio.isEnabled); XCTAssertEqual(audio.status, "On")
        XCTAssertTrue(descendant(in: view.controls, accessibilityLabel: "Frames per second") is NSPopUpButton)
        XCTAssertEqual((descendant(in: view.controls, accessibilityLabel: "Frames per second")
            as? NSPopUpButton)?.itemTitles, ["60", "30", "15"])
        let microphone = try XCTUnwrap(descendant(in: view.controls,
            accessibilityLabel: "Microphone") as? NSPopUpButton)
        XCTAssertTrue(microphone.isEnabled)
        XCTAssertEqual(microphone.titleOfSelectedItem, "Selected microphone",
            "a stored device remains selected while temporarily absent from discovery")
        var changes: [RecordingControlState] = []
        view.controls.recordingControlsChanged = { changes.append($0) }
        view.controls.selectMicrophone(0, notify: true)
        XCTAssertNil(changes.last?.microphoneDeviceID)
        view.controls.selectMicrophone(2, notify: true)
        XCTAssertEqual(changes.last?.microphoneDeviceID, "built-in")
        XCTAssertEqual(view.controlsState.mode, .record)
        XCTAssertEqual(view.controls.frame.width, 736)
        XCTAssertEqual(view.controls.frame.height, 154)
        XCTAssertTrue(view.controls.subviews.allSatisfy {
            $0.frame.minX >= 0 && $0.frame.maxX <= view.controls.bounds.width
                && $0.frame.minY >= 0 && $0.frame.maxY <= view.controls.bounds.height
        }, "recording settings must not clip on a 768-point display")
        try render(view, window: window, name: "capture-controls-dark-narrow-recording")
    }

    func testBlankControlsSpaceDragsAndClampsWithoutStartingARegion() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: frame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let view = makeView()
        window.contentView = view
        let original = view.controls.frame

        let controlsPoint = NSPoint(x: view.controls.frame.midX, y: view.controls.frame.midY)
        view.mouseDown(with: try mouseEvent(.leftMouseDown, window: window,
            x: controlsPoint.x, y: controlsPoint.y))
        view.mouseDragged(with: try mouseEvent(.leftMouseDragged, window: window,
            x: 120, y: 100))
        view.mouseUp(with: try mouseEvent(.leftMouseUp, window: window, x: 120, y: 100))
        XCTAssertNil(view.choice)
        XCTAssertNil(view.region.mode)
        XCTAssertEqual(view.region.rect.width, 0,
            "a controls-origin gesture must never leak into region selection")

        view.controls.beginPanelDrag(at: NSPoint(x: original.midX, y: original.midY))
        view.controls.dragPanel(to: NSPoint(x: -500, y: -500))
        XCTAssertEqual(view.controls.frame.origin, NSPoint(x: 16, y: 16))
        view.controls.endPanelDrag()

        view.controls.beginPanelDrag(at: NSPoint(x: 26, y: 26))
        view.controls.dragPanel(to: NSPoint(x: 5_000, y: 5_000))
        XCTAssertEqual(view.controls.frame.maxX, frame.width - 16)
        XCTAssertEqual(view.controls.frame.maxY, frame.height - 16)
        view.controls.endPanelDrag()

        let footerPoint = view.convert(NSPoint(x: 20, y: 70), from: view.controls)
        XCTAssertTrue(view.controls.hitTest(footerPoint) === view.controls,
            "the blank footer is the drag target")
        let capture = try XCTUnwrap(buttons(in: view.controls).first { $0.title == "Capture" })
        let capturePoint = view.convert(NSPoint(x: capture.frame.midX,
            y: capture.frame.midY), from: view.controls)
        XCTAssertTrue(view.controls.hitTest(capturePoint) === capture,
            "interactive controls do not begin a panel drag")
    }

    func testNarrowMonitorKeepsPickerAndPrimaryActionVisibleWithoutOverlap() throws {
        _ = NSApplication.shared
        let narrowFrame = NSRect(x: 0, y: 0, width: 768, height: 600)
        let window = NSWindow(contentRect: narrowFrame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let view = UnifiedCaptureSelectionView(frame: narrowFrame, image: nil, targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: false,
            hitTest: { _ in 0 }, displayTitles: ["Main display · 768 × 600"],
            selectedDisplay: 0, confirm: { _ in }, cancel: {}, changeDisplay: { _ in })
        window.contentView = view
        XCTAssertEqual(view.controls.frame.width, 736)
        try render(view, window: window, name: "capture-controls-dark-narrow-region")
        view.setTarget(.display)
        try render(view, window: window, name: "capture-controls-dark-narrow-display")

        let capture = try XCTUnwrap(buttons(in: view.controls).first { $0.title == "Capture" })
        let picker = try XCTUnwrap(descendant(in: view.controls, accessibilityLabel: "Display"))
        XCTAssertTrue(view.controls.bounds.contains(capture.frame))
        XCTAssertTrue(view.controls.bounds.contains(picker.frame))
        XCTAssertFalse(capture.frame.intersects(picker.frame))
        for subview in view.controls.subviews where !subview.isHidden {
            XCTAssertTrue(view.controls.bounds.contains(subview.frame),
                "\(subview) clips on a 768-point monitor")
        }
    }

    func testPanelIsExcludedFromCapturedPixels() throws {
        _ = NSApplication.shared
        let screen = try XCTUnwrap(NSScreen.main)
        var cancelled = 0
        let panel = UnifiedCapturePanel(screen: screen, image: nil, targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: false,
            hitTest: { _ in -1 }, displayTitles: ["Main display"], selectedDisplay: 0,
            confirm: { _ in }, cancel: { cancelled += 1 }, changeDisplay: { _ in })
        defer { panel.close() }
        XCTAssertEqual(panel.title, "Captures Capture Controls")
        XCTAssertEqual(panel.sharingType, .none)
        XCTAssertEqual(panel.selector.target, .region)
        panel.cancelOperation(nil)
        XCTAssertEqual(cancelled, 1, "Escape command routing cancels even when a control owns focus")
    }

    func testFixedGlassEmptySelectedAndAutoStatesRenderInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in ["dark", "light"] {
            let window = NSWindow(contentRect: frame, styleMask: [.borderless],
                backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let empty = makeView(appearance: appearance)
            window.contentView = empty
            try render(empty, window: window, name: "capture-controls-\(appearance)-empty")

            empty.beginRegion(NSPoint(x: 114, y: 78))
            empty.dragRegion(NSPoint(x: 478, y: 283)); empty.endRegion()
            empty.setAspect(4)
            try render(empty, window: window, name: "capture-controls-\(appearance)-selected")

            let automatic = makeView(appearance: appearance, autoStart: true)
            window.contentView = automatic
            automatic.setTarget(.window)
            _ = automatic.hoverWindow(NSPoint(x: 24, y: 40))
            try render(automatic, window: window, name: "capture-controls-\(appearance)-auto")

            let controls = buttons(in: automatic.controls)
            XCTAssertEqual(controls.filter(\.selected).map(\.title).sorted(),
                ["Screenshot", "Window"])
            XCTAssertTrue(controls.allSatisfy(\.glass))
            XCTAssertTrue(controls.allSatisfy { $0.accessibilityLabel() != nil })
            XCTAssertTrue(controls.first { $0.title == "Capture" }?.isHidden == true)
            XCTAssertTrue(controls.first { $0.title == "Record" }?.isEnabled == true)
            XCTAssertEqual(automatic.controls.frame.width, 854)
            XCTAssertEqual(automatic.controls.frame.height, 86)
            XCTAssertEqual(automatic.controls.frame.minY, frame.height - 112)

            automatic.setMode(.record)
            XCTAssertEqual(automatic.controls.frame.width, 902)
            XCTAssertEqual(automatic.controls.frame.height, 154)
            try render(automatic, window: window,
                name: "capture-controls-\(appearance)-recording")
        }
    }

    func testNoteLinksOpenPreferencesAndFollowCapabilities() throws {
        _ = NSApplication.shared
        var opened: [String] = []
        let automatic = makeView(autoStart: true)
        automatic.openPreference = { opened.append($0) }
        XCTAssertEqual(automatic.controls.noteLinks.map(\.setting),
            ["include_recording_controls_in_captures", "auto_start_on_selection"])
        XCTAssertEqual(automatic.controls.noteText,
            "These controls won’t show in screenshots · Auto-capture is on. Selecting a target starts immediately.")
        automatic.controls.noteLinks.forEach { $0.performClick(nil) }
        XCTAssertEqual(opened, ["include_recording_controls_in_captures", "auto_start_on_selection"])
        for view in automatic.controls.noteLinks {
            XCTAssertTrue(automatic.controls.bounds.contains(view.frame), "\(view.text) clips")
        }

        let plain = makeView(visibility: CaptureControlsVisibility(canExclude: false, excluded: false))
        XCTAssertTrue(plain.controls.noteLinks.isEmpty,
            "Enter confirmation and non-excludable controls are plain text")
        XCTAssertEqual(plain.controls.noteText,
            "These controls will show in screenshots · Press Enter to confirm")
        plain.setMode(.record)
        XCTAssertEqual(plain.controls.noteText,
            "These controls will show in recordings · Use Hide controls to keep them out · Press Enter to confirm")
    }

    func testRecordingSwitchesCoupleCursorAndClicksLikeShipping() throws {
        _ = NSApplication.shared
        let view = UnifiedCaptureSelectionView(frame: frame, image: nil, targets: targets,
            tokens: Tokens.variants["dark-mustard"]!, autoStart: false, hitTest: { _ in 0 },
            displayTitles: ["Main display"], selectedDisplay: 0, confirm: { _ in }, cancel: {},
            changeDisplay: { _ in },
            recordingState: RecordingControlState(framesPerSecond: 30, maxResolution: "p720",
                showCursor: false, highlightClicks: false, systemAudio: false, microphoneDeviceID: nil),
            recordingAvailability: RecordingControlAvailability(cursor: true, clicks: true,
                systemAudio: false, microphone: false))
        view.setMode(.record)
        var changes: [RecordingControlState] = []
        view.controls.recordingControlsChanged = { changes.append($0) }
        view.controls.toggleRecordingSwitch("highlight_clicks")
        XCTAssertEqual(changes.last?.highlightClicks, true)
        XCTAssertEqual(changes.last?.showCursor, true, "showing clicks shows the cursor")
        view.controls.toggleRecordingSwitch("show_cursor")
        XCTAssertEqual(changes.last?.showCursor, false)
        XCTAssertEqual(changes.last?.highlightClicks, false, "hiding the cursor hides clicks")
        view.controls.toggleRecordingSwitch("system_audio")
        XCTAssertEqual(changes.count, 2, "an unavailable switch cannot change")
        let switches = recordingSwitches(in: view.controls)
        XCTAssertEqual(switches.map(\.status), ["Off", "Off", "Unavailable"])
        let microphone = try XCTUnwrap(descendant(in: view.controls,
            accessibilityLabel: "Microphone") as? NSPopUpButton)
        XCTAssertEqual(microphone.titleOfSelectedItem, "Unavailable")
        XCTAssertEqual((descendant(in: view.controls, accessibilityLabel: "Maximum resolution")
            as? NSPopUpButton)?.titleOfSelectedItem, "720p")
    }

    func testFullScreenIdentityAndGuidanceFollowShippingRules() throws {
        _ = NSApplication.shared
        // Keep asserted windows within the CI runner's clamped screen height.
        let compact = NSRect(x: 0, y: 0, width: 1000, height: 600)
        let window = NSWindow(contentRect: compact, styleMask: [.borderless],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false; defer { window.close() }
        let view = makeView(identity: CaptureDisplayIdentity(name: "Studio Display",
            width: 5120, height: 2880), viewFrame: compact)
        window.contentView = view

        XCTAssertEqual(view.guidanceText, "Drag to select a region · Shift for square · Esc to cancel")
        XCTAssertEqual(view.guidanceFrame.minY, (compact.height * 0.16).rounded(), accuracy: 1,
            "the shipping chip sits 16% from the top")
        let chip = view.guidanceFrame
        view.duckGuidance(at: NSPoint(x: chip.minX - 20, y: chip.midY))
        XCTAssertTrue(view.isGuidanceDucked, "the chip fades within 28 points of the pointer")
        view.duckGuidance(at: NSPoint(x: chip.minX - 35, y: chip.midY))
        XCTAssertTrue(view.isGuidanceDucked, "leave slack keeps a faded chip hidden")
        view.duckGuidance(at: NSPoint(x: chip.minX - 45, y: chip.midY))
        XCTAssertFalse(view.isGuidanceDucked)

        view.setTarget(.display)
        XCTAssertFalse(view.isGuidanceVisible, "Full screen shows the identity, not guidance")
        XCTAssertTrue(view.isDisplayIdentityVisible)
        XCTAssertEqual(view.displayIdentityText, "Studio Display · 5120 × 2880")
        try render(view, window: window, name: "capture-controls-light-display-identity")
        view.setMode(.record)
        XCTAssertEqual(view.displayIdentityText, "Studio Display · 5120 × 2880 · 60 FPS")

        view.setMode(.screenshot); view.setTarget(.window)
        XCTAssertFalse(view.isDisplayIdentityVisible)
        XCTAssertEqual(view.guidanceText, "Select a window to continue · Esc to cancel")
        XCTAssertTrue(view.hoverWindow(NSPoint(x: 700, y: 400)))
        XCTAssertEqual(view.guidanceText, "Click to capture this display · Esc to cancel")
        XCTAssertTrue(view.hoverWindow(NSPoint(x: 100, y: 200)))
        XCTAssertTrue(view.isGuidanceVisible, "guidance stays while only hovering a window")
        view.selectWindow(NSPoint(x: 100, y: 200))
        XCTAssertFalse(view.isGuidanceVisible, "guidance hides once a window is selected")
    }

    func testNoteLinkRevealsAndHighlightsItsPreferencesRow() throws {
        _ = NSApplication.shared
        let root = Surface(frame: NSRect(x: 0, y: 0, width: 1000, height: 600))
        let tokens = Tokens.variants["dark-mustard"]!
        let store = try SettingsStore(path: "/fixture/settings.json",
            transport: CaptureMenuSettingsTransport(), debounceInterval: 0)
        let controller = PreferencesController(root: root, store: store, tokens: { tokens },
            appearanceChanged: { _, _, _ in }, showHistory: {})
        controller.revealSetting("include_recording_controls_in_captures")
        let deadline = Date().addingTimeInterval(2)
        while controller.highlightedRowFrame == nil, Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        let row = try XCTUnwrap(controller.highlightedRowFrame)
        let scroll = try XCTUnwrap(scrollView(in: root))
        let visible = scroll.contentView.bounds
        XCTAssertGreaterThan(visible.minY, 0, "the Capture row starts below the first viewport")
        XCTAssertTrue(visible.contains(NSPoint(x: row.midX, y: row.midY)),
            "the linked row is scrolled into view: \(row) in \(visible)")
        XCTAssertEqual(controller.highlightedSetting, "include_recording_controls_in_captures")
        withExtendedLifetime(controller) {}
    }

    private func recordingSwitches(in view: NSView) -> [RecordingSwitchButton] {
        view.subviews.flatMap { subview in
            (subview as? RecordingSwitchButton).map { [$0] } ?? recordingSwitches(in: subview)
        }
    }

    private func scrollView(in view: NSView) -> NSScrollView? {
        if let scroll = view as? NSScrollView { return scroll }
        return view.subviews.lazy.compactMap { self.scrollView(in: $0) }.first
    }

    private func makeView(appearance: String = "light", autoStart: Bool = false,
                          confirm: @escaping (WindowSelectionChoice) -> Void = { _ in },
                          cancel: @escaping () -> Void = {},
                          visibility: CaptureControlsVisibility = .excludedByDefault,
                          identity: CaptureDisplayIdentity? = nil,
                          viewFrame: NSRect? = nil) -> UnifiedCaptureSelectionView {
        UnifiedCaptureSelectionView(frame: viewFrame ?? frame,
            image: PreviewView.fixtureImage(scale: 2048.0 / 284.0), targets: targets,
            tokens: Tokens.variants["\(appearance)-mustard"]!, autoStart: autoStart,
            hitTest: { point in point.x < 50 ? 1 : (point.x < 400 ? 0 : -1) },
            displayTitles: ["Main display · 1000 × 720", "External · 1920 × 1080"],
            selectedDisplay: 0, confirm: confirm, cancel: cancel, changeDisplay: { _ in },
            visibility: visibility, displayIdentity: identity)
    }

    private func buttons(in view: NSView) -> [CaptureButton] {
        view.subviews.flatMap { subview in
            (subview as? CaptureButton).map { [$0] } ?? buttons(in: subview)
        }
    }

    private func descendant(in view: NSView, accessibilityLabel: String) -> NSView? {
        if view.accessibilityLabel() == accessibilityLabel { return view }
        return view.subviews.lazy.compactMap {
            self.descendant(in: $0, accessibilityLabel: accessibilityLabel)
        }.first
    }

    private func mouseEvent(_ type: NSEvent.EventType, window: NSWindow,
                            x: CGFloat, y: CGFloat) throws -> NSEvent {
        try XCTUnwrap(NSEvent.mouseEvent(with: type,
            location: NSPoint(x: x, y: frame.height - y), modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
    }

    private func keyEvent(window: NSWindow, keyCode: UInt16, characters: String) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: characters, charactersIgnoringModifiers: characters,
            isARepeat: false, keyCode: keyCode))
    }

    private func render(_ view: NSView, window: NSWindow, name: String) throws {
        window.display(); view.layoutSubtreeIfNeeded()
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let url = URL(fileURLWithPath: directory).appendingPathComponent("\(name).png")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }
}
