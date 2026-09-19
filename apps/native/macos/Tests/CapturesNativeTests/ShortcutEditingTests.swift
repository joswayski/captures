import AppKit
import XCTest
@testable import CapturesNative

private final class ShortcutSettingsTransport: SettingsTransport {
    private let lock = NSLock()
    private var value: [String: Any]
    private(set) var saves: [[String: Any]] = []

    init(appearance: String = "dark", newCaptureShortcut: String = "Command+Shift+Space") {
        value = [
            "appearance": appearance, "theme": "mustard", "custom_theme": [:],
            "output_directory": "/fixture/Captures",
            "new_capture_shortcut": newCaptureShortcut,
            "region_shortcut": "Command+Shift+Digit4",
            "window_shortcut": "Command+Shift+KeyW",
            "display_shortcut": "Command+Shift+Digit3",
            "recording": ["video_shortcut": "Command+Shift+Digit5",
                "window_shortcut": "Command+Shift+Digit6",
                "display_shortcut": "Command+Shift+Digit7"],
        ]
    }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        switch object["operation"] as? String {
        case "load": return ["ok": true, "settings": value]
        case "save":
            guard let settings = object["settings"] as? [String: Any] else {
                throw SettingsStoreError.invalidResponse
            }
            value = settings; saves.append(settings)
            return ["ok": true, "settings": settings]
        default: return ["ok": true, "path": "/fixture/settings.json"]
        }
    }

    func latestSave() -> [String: Any]? {
        lock.lock(); defer { lock.unlock() }
        return saves.last
    }

    func saveCount() -> Int {
        lock.lock(); defer { lock.unlock() }
        return saves.count
    }
}

final class ShortcutEditingTests: XCTestCase {
    private var windows: [NSWindow] = []

    override func tearDown() {
        windows.forEach { $0.close() }
        windows.removeAll()
        super.tearDown()
    }

    func testAllRowsRenderAndValidChordPersistsAtCorrectBoundary() throws {
        let transport = ShortcutSettingsTransport()
        let (controller, window) = try fixture(transport: transport) { code, control, shift, alt, meta in
            if code == "MetaLeft" { return ["kind": "waiting", "keys": ["Cmd"]] }
            return ["kind": "complete", "keys": ["Ctrl", "Shift", "Option", "Cmd", "Q"],
                "shortcut": "Control+Shift+Alt+Super+KeyQ"]
        }
        let identifiers = ["new_capture_shortcut", "region_shortcut", "window_shortcut",
            "display_shortcut", "recording.video_shortcut", "recording.window_shortcut",
            "recording.display_shortcut"]
        for identifier in identifiers { XCTAssertNotNil(controller.shortcutRecorder(identifier: identifier)) }

        let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: "region_shortcut"))
        recorder.performClick(nil)
        XCTAssertTrue(recorder.recording)
        XCTAssertTrue(window.firstResponder === recorder)
        XCTAssertTrue(recorder.isAccessibilitySelected())
        controller.handleShortcutInput(code: "MetaLeft", control: false, shift: false,
            alt: false, meta: true)
        XCTAssertEqual(recorder.keys, ["Cmd"])
        controller.handleShortcutInput(code: "KeyQ", control: true, shift: true,
            alt: true, meta: true)
        XCTAssertFalse(recorder.recording)
        XCTAssertEqual(recorder.keys, ["Ctrl", "Shift", "Option", "Cmd", "Q"])
        controller.flush()
        try waitUntil { transport.latestSave() != nil }
        XCTAssertEqual(transport.latestSave()?.string("region_shortcut"),
            "Control+Shift+Alt+Super+KeyQ")
        let recording = transport.latestSave()?["recording"] as? [String: Any]
        XCTAssertEqual(recording?.string("video_shortcut"), "Command+Shift+Digit5")
    }

    func testInvalidStaysRecordingAndEscapeOrBlurCancelsWithoutSaving() throws {
        let transport = ShortcutSettingsTransport()
        let (controller, window) = try fixture(transport: transport) { code, _, _, _, _ in
            if code == "Escape" { return ["kind": "cancel"] }
            return ["kind": "invalid", "keys": ["A"],
                "message": "Include a modifier, or use Print Screen."]
        }
        let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: "display_shortcut"))
        recorder.performClick(nil)
        controller.flush()
        let baselineSaveCount = transport.saveCount()
        controller.handleShortcutInput(code: "KeyA", control: false, shift: false,
            alt: false, meta: false)
        XCTAssertTrue(recorder.recording)
        XCTAssertEqual(recorder.error, "Include a modifier, or use Print Screen.")
        XCTAssertEqual(recorder.accessibilityHelp(), recorder.error)
        controller.handleShortcutInput(code: "Escape", control: true, shift: true,
            alt: false, meta: false)
        XCTAssertFalse(recorder.recording, "modified Escape still cancels")
        controller.flush()
        XCTAssertEqual(transport.saveCount(), baselineSaveCount)

        recorder.performClick(nil)
        XCTAssertTrue(window.makeFirstResponder(nil))
        XCTAssertFalse(recorder.recording, "blur cancels recording")
        recorder.performClick(nil)
        NotificationCenter.default.post(name: NSWindow.didResignKeyNotification, object: window)
        XCTAssertFalse(recorder.recording, "window focus loss cancels recording")
        controller.flush()
        XCTAssertEqual(transport.saveCount(), baselineSaveCount)
    }

    func testRealBridgeCancelWithoutKeysStopsControllerRecording() throws {
        let transport = ShortcutSettingsTransport()
        let (controller, _) = try fixture(transport: transport)
        let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: "region_shortcut"))
        recorder.performClick(nil)
        controller.flush()
        let baselineSaveCount = transport.saveCount()

        let response = try NativeCaptureShortcuts.record(code: "Escape", control: true,
            shift: true, alt: true, meta: true)
        XCTAssertEqual(response["kind"] as? String, "cancel")
        XCTAssertNil(response["keys"], "shipping cancel responses intentionally omit keys")
        controller.handleShortcutInput(code: "Escape", control: true, shift: true,
            alt: true, meta: true)

        XCTAssertFalse(recorder.recording)
        controller.flush()
        XCTAssertEqual(transport.saveCount(), baselineSaveCount)
    }

    func testRealBridgeLongestChordFitsAndPersists() throws {
        let transport = ShortcutSettingsTransport()
        let (controller, _) = try fixture(transport: transport)
        let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: "new_capture_shortcut"))
        recorder.performClick(nil)
        controller.handleShortcutInput(code: "MediaTrackPrevious", control: true, shift: true,
            alt: true, meta: true)

        XCTAssertEqual(recorder.keys, ["Ctrl", "Shift", "Option", "Cmd", "MediaTrackPrevious"])
        XCTAssertFalse(recorder.recording)
        XCTAssertGreaterThanOrEqual(recorder.displayedChipFontSize(), 9)
        XCTAssertLessThanOrEqual(try XCTUnwrap(recorder.displayedChipFrames().last).maxX,
            recorder.bounds.maxX - 10)
        controller.flush()
        try waitUntil { transport.latestSave() != nil }
        XCTAssertEqual(transport.latestSave()?.string("new_capture_shortcut"),
            "Control+Shift+Alt+Super+MediaTrackPrevious")
    }

    func testEveryRecorderPersistsItsOwnStoredField() throws {
        let transport = ShortcutSettingsTransport()
        let (controller, _) = try fixture(transport: transport) { code, _, _, _, _ in
            ["kind": "complete", "keys": ["Ctrl", code],
             "shortcut": "Control+\(code)"]
        }
        let identifiers = ["new_capture_shortcut", "region_shortcut", "window_shortcut",
            "display_shortcut", "recording.video_shortcut", "recording.window_shortcut",
            "recording.display_shortcut"]
        try XCTUnwrap(controller.shortcutRecorder(identifier: identifiers[0])).performClick(nil)
        controller.flush()
        let baselineSaveCount = transport.saveCount()
        for (index, identifier) in identifiers.enumerated() {
            let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: identifier))
            recorder.performClick(nil)
            controller.handleShortcutInput(code: "F\(index + 1)", control: true,
                shift: false, alt: false, meta: false)
            controller.flush()
            XCTAssertEqual(transport.saveCount(), baselineSaveCount + index + 1)
        }
        let saved = try XCTUnwrap(transport.latestSave())
        XCTAssertEqual(saved.string("new_capture_shortcut"), "Control+F1")
        XCTAssertEqual(saved.string("region_shortcut"), "Control+F2")
        XCTAssertEqual(saved.string("window_shortcut"), "Control+F3")
        XCTAssertEqual(saved.string("display_shortcut"), "Control+F4")
        let recording = try XCTUnwrap(saved["recording"] as? [String: Any])
        XCTAssertEqual(recording.string("video_shortcut"), "Control+F5")
        XCTAssertEqual(recording.string("window_shortcut"), "Control+F6")
        XCTAssertEqual(recording.string("display_shortcut"), "Control+F7")
    }

    func testCommandKeyEquivalentsAreConsumedByFocusedRecorder() throws {
        let transport = ShortcutSettingsTransport()
        var events: [(String, Bool)] = []
        let (controller, window) = try fixture(transport: transport) { code, _, _, _, meta in
            events.append((code, meta))
            return ["kind": "invalid", "keys": ["Cmd", String(code.suffix(1))],
                "message": "Keep recording"]
        }
        let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: "window_shortcut"))
        XCTAssertTrue(window.makeFirstResponder(recorder))
        let activate = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber,
            context: nil, characters: " ", charactersIgnoringModifiers: " ",
            isARepeat: false, keyCode: 49))
        recorder.keyDown(with: activate)
        XCTAssertTrue(recorder.recording, "Space activates the focused recorder")
        for (keyCode, character) in [(UInt16(3), "f"), (UInt16(12), "q")] {
            let event = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: .command, timestamp: 0, windowNumber: window.windowNumber,
                context: nil, characters: character, charactersIgnoringModifiers: character,
                isARepeat: false, keyCode: keyCode))
            XCTAssertTrue(recorder.performKeyEquivalent(with: event))
        }
        XCTAssertEqual(events.map { $0.0 }, ["KeyF", "KeyQ"])
        XCTAssertTrue(events.allSatisfy { $0.1 })
        XCTAssertTrue(recorder.recording)
    }

    func testShortcutCardsRenderNormalRecordingAndInvalidInLightAndDark() throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        for appearance in ["light", "dark"] {
            let transport = ShortcutSettingsTransport(appearance: appearance,
                newCaptureShortcut: "Control+Shift+Alt+Super+MediaTrackPrevious")
            let (controller, _) = try fixture(transport: transport, appearance: appearance)
            let card = try XCTUnwrap(controller.shortcutCard())
            try render(card, name: "shortcuts-\(appearance)-normal.png", directory: directory)
            let recorder = try XCTUnwrap(controller.shortcutRecorder(identifier: "new_capture_shortcut"))
            XCTAssertEqual(recorder.keys,
                ["Ctrl", "Shift", "Option", "Cmd", "MediaTrackPrevious"])
            XCTAssertLessThanOrEqual(try XCTUnwrap(recorder.displayedChipFrames().last).maxX,
                recorder.bounds.maxX - 10)
            recorder.performClick(nil)
            try render(card, name: "shortcuts-\(appearance)-recording.png", directory: directory)
            controller.handleShortcutInput(code: "KeyA", control: false, shift: false,
                alt: false, meta: false)
            XCTAssertEqual(recorder.error,
                "Include Ctrl, Shift, Option, or Command, or use Print Screen.")
            try render(card, name: "shortcuts-\(appearance)-invalid.png", directory: directory)
        }
    }

    func testMacVirtualKeyTranslationUsesPhysicalDomCodes() throws {
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 0), "KeyA")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 18), "Digit1",
            "ANSI number row must not be confused with the keypad")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 83), "Numpad1")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 53), "Escape")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 64), "F17")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 79), "F18")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 80), "F19")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 90), "F20")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 123), "ArrowLeft")
        XCTAssertEqual(PreferencesController.domCode(forKeyCode: 255), "Unidentified")
    }

    private func fixture(transport: ShortcutSettingsTransport, appearance: String = "dark",
                         policy: PreferencesController.ShortcutPolicy? = nil)
        throws -> (PreferencesController, NSWindow) {
        _ = NSApplication.shared
        let root = Surface(frame: NSRect(x: 0, y: 0, width: 1000, height: 720))
        root.wantsLayer = true
        let window = NSWindow(contentRect: root.bounds, styleMask: [.titled, .closable],
            backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false; window.contentView = root
        windows.append(window)
        let tokens = Tokens.variants["\(appearance)-mustard"]!
        let store = try SettingsStore(path: "/fixture/settings.json", transport: transport,
            debounceInterval: 0)
        let controller: PreferencesController
        if let policy {
            controller = PreferencesController(root: root, store: store, tokens: { tokens },
                appearanceChanged: { _, _, _ in }, shortcutPolicy: policy,
                showHistory: {}, liveCaptureAvailable: true)
        } else {
            controller = PreferencesController(root: root, store: store, tokens: { tokens },
                appearanceChanged: { _, _, _ in }, showHistory: {}, liveCaptureAvailable: true)
        }
        try waitUntil { controller.shortcutRecorder(identifier: "region_shortcut") != nil }
        window.makeKeyAndOrderFront(nil)
        return (controller, window)
    }

    private func waitUntil(_ predicate: @escaping () -> Bool) throws {
        let deadline = Date().addingTimeInterval(2)
        while !predicate(), Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(predicate())
    }

    private func render(_ view: NSView, name: String, directory: String) throws {
        view.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let url = URL(fileURLWithPath: directory).appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true)
        try data.write(to: url)
    }
}
