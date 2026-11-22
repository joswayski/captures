import AppKit
import XCTest
@testable import CapturesNative

final class RecordingTransport: SettingsTransport {
    private let lock = NSLock()
    var requests: [[String: Any]] = []
    var error: Error?

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); requests.append(object); let failure = error; lock.unlock()
        if let failure { throw failure }
        switch object["operation"] as? String {
        case "load": return ["ok": true, "settings": ["theme": "mustard"]]
        case "save": return ["ok": true, "settings": object["settings"] as Any]
        case "theme": return ["ok": true, "colors": ["theme-accent": [1.0, 0.0, 0.0, 1.0]]]
        default: return ["ok": true, "path": "/fixture/settings.json"]
        }
    }

    func operations() -> [String] {
        lock.lock(); defer { lock.unlock() }
        return requests.compactMap { $0["operation"] as? String }
    }
}

final class SettingsStoreTests: XCTestCase {
    func testRealBridgePersistsSettingsAndRejectsInvalidSave() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let path = directory.appendingPathComponent("settings.json").path
        let bridge = SettingsBridge()
        var settings = try XCTUnwrap(bridge.request(["operation": "load", "path": path])["settings"] as? [String: Any])
        settings["appearance"] = "dark"
        settings["theme"] = "custom"
        settings["custom_theme"] = ["accent": "#AABBCC", "signal": "#DE4567"]
        settings["screenshot_countdown_seconds"] = 7
        _ = try bridge.request(["operation": "save", "path": path, "settings": settings])
        let reopened = try XCTUnwrap(bridge.request(["operation": "load", "path": path])["settings"] as? [String: Any])
        XCTAssertEqual(reopened.string("appearance"), "dark")
        XCTAssertEqual(reopened.int("screenshot_countdown_seconds"), 7)
        XCTAssertEqual((reopened["custom_theme"] as? [String: String])?["accent"], "#AABBCC")
        settings["screenshot_countdown_seconds"] = 11
        XCTAssertThrowsError(try bridge.request(["operation": "save", "path": path, "settings": settings]))
        let unchanged = try XCTUnwrap(bridge.request(["operation": "load", "path": path])["settings"] as? [String: Any])
        XCTAssertEqual(unchanged.int("screenshot_countdown_seconds"), 7)
    }

    func testRapidUpdatesCoalesceToNewestRevision() throws {
        let transport = RecordingTransport()
        let store = try SettingsStore(path: "/fixture.json", transport: transport, debounceInterval: 0.03)
        let completion = expectation(description: "newest save")
        var completedRevision = 0
        _ = store.save(["value": 1]) { _, _ in XCTFail("superseded save completed") }
        let newest = store.save(["value": 2]) { revision, result in
            if case .failure(let error) = result { XCTFail("Save failed: \(error)") }
            completedRevision = revision; completion.fulfill()
        }
        wait(for: [completion], timeout: 1)
        XCTAssertEqual(completedRevision, newest)
        XCTAssertEqual(transport.operations(), ["save"])
        XCTAssertEqual(transport.requests.last?["settings"] as? [String: Int], ["value": 2])
    }

    func testFlushPersistsPendingTextImmediately() throws {
        let transport = RecordingTransport()
        let store = try SettingsStore(path: "/fixture.json", transport: transport, debounceInterval: 60)
        _ = store.save(["output_directory": "/recent text"]) { _, _ in }
        store.flush()
        XCTAssertEqual(transport.operations(), ["save"])
        XCTAssertEqual((transport.requests.last?["settings"] as? [String: String])?["output_directory"], "/recent text")
    }

    func testCaptureFolderTypingPersistsWithoutLeavingTheField() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let path = directory.appendingPathComponent("settings.json").path
        let store = try SettingsStore(path: path, debounceInterval: 0.03)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 700)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame)
        window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let saved = expectation(description: "typing persisted while the field remains focused")
        let finalFolder = directory.appendingPathComponent("Edited captures").path
        var latestFolder = ""
        let controller = PreferencesController(root: root, store: store, tokens: { tokens },
            appearanceChanged: { _, _, _ in },
            settingsChanged: { latestFolder = $0.string("output_directory") },
            settingsPersisted: { if $0.string("output_directory") == finalFolder { saved.fulfill() } },
            showHistory: {})
        defer { withExtendedLifetime(controller) {} }
        let deadline = Date().addingTimeInterval(3)
        while controller.shortcutCard() == nil && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        let field = try XCTUnwrap(KeyViewLoop.candidates(in: root).first {
            $0.identifier?.rawValue == "setting.output_directory"
        } as? NSTextField)
        field.selectText(nil)
        let editor = try XCTUnwrap(field.currentEditor() as? NSTextView)
        editor.insertText(directory.appendingPathComponent("captures").path,
                          replacementRange: editor.selectedRange())
        let insertion = (directory.path as NSString).length + 1
        editor.setSelectedRange(NSRange(location: insertion, length: 0))
        editor.insertText("Edited ", replacementRange: editor.selectedRange())
        let caret = editor.selectedRange()
        XCTAssertEqual(latestFolder, finalFolder, "capture settings change before focus leaves the input")
        wait(for: [saved], timeout: 3)
        let reopened = try XCTUnwrap(SettingsBridge().request(["operation": "load", "path": path])["settings"] as? [String: Any])
        XCTAssertEqual(reopened.string("output_directory"), finalFolder)
        XCTAssertTrue(window.firstResponder === editor)
        XCTAssertTrue(field.currentEditor() === editor, "autosave must not replace the active input")
        XCTAssertEqual(editor.selectedRange(), caret)
        XCTAssertEqual(field.stringValue, finalFolder)
    }

    func testLoadAndPersistenceErrorsCrossTransportBoundary() throws {
        enum FixtureError: Error { case failed }
        let transport = RecordingTransport(); transport.error = FixtureError.failed
        let store = try SettingsStore(path: "/fixture.json", transport: transport, debounceInterval: 0)
        let loaded = expectation(description: "load error")
        store.load { result in
            if case .success = result { XCTFail("Expected load failure") }
            loaded.fulfill()
        }
        wait(for: [loaded], timeout: 1)
    }

    func testHexNormalizationAcceptsShippingForms() {
        XCTAssertEqual(PreferencesController.normalizeHex("#abc"), "#AABBCC")
        XCTAssertNil(PreferencesController.normalizeHex("abc"))
        XCTAssertEqual(PreferencesController.normalizeHex("#12ef90"), "#12EF90")
        XCTAssertNil(PreferencesController.normalizeHex("#12"))
    }

    /// Shipping `.custom-theme-color-control`: a token-drawn swatch beside a
    /// padded field whose hex value sits on the field's vertical centre.
    func testCustomThemeColorControlsCentreTheirValueBesideTokenSwatches() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("settings.json")
        let bridge = SettingsBridge()
        for appearance in ["dark", "light"] {
            var settings = try XCTUnwrap(bridge.request(["operation": "load", "path": file.path])["settings"] as? [String: Any])
            settings["appearance"] = appearance
            settings["theme"] = "custom"
            settings["custom_theme"] = ["accent": "#32D3FF", "signal": "#FF4FC3"]
            _ = try bridge.request(["operation": "save", "path": file.path, "settings": settings])
            let store = try SettingsStore(path: file.path, debounceInterval: 60)
            let frame = NSRect(x: 0, y: 0, width: 1000, height: 700)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame)
            window.contentView = root
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            let controller = PreferencesController(root: root, store: store, tokens: { tokens },
                appearanceChanged: { _, _, _ in }, showHistory: {})
            defer { withExtendedLifetime(controller) {} }
            var fields: [NSTextField] = []
            let deadline = Date().addingTimeInterval(3)
            while fields.count < 2 && Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01))
                fields = KeyViewLoop.candidates(in: root).compactMap { $0 as? NSTextField }
                    .filter { $0.identifier?.rawValue.hasPrefix("custom.") == true }
            }
            XCTAssertEqual(fields.count, 2)
            for field in fields {
                let well = try XCTUnwrap(field.superview?.subviews.first { $0 is ClosureColorWell } as? ClosureColorWell)
                XCTAssertNotNil(well.pickerTokens, "the swatch is drawn with tokens, not the stock well")
                let cell = try XCTUnwrap(field.cell as? TokenNumberFieldCell)
                let text = cell.drawingRect(forBounds: field.bounds)
                XCTAssertEqual(text.minX, tokens.number("s-3"), accuracy: 0.5)
                XCTAssertEqual(text.midY, field.bounds.midY, accuracy: 1.5)
            }
            guard let output = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"],
                  let editor = fields.first?.superview?.superview else { continue }
            window.display()
            let bitmap = try XCTUnwrap(editor.bitmapImageRepForCachingDisplay(in: editor.bounds))
            editor.cacheDisplay(in: editor.bounds, to: bitmap)
            let url = URL(fileURLWithPath: output).appendingPathComponent("custom-theme-editor-\(appearance).png")
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true)
            try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
        }
    }

    func testCustomColorEscapeDiscardsDraftButEnterCommitsAndFindStaysOpen() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("settings.json")
        let bridge = SettingsBridge()
        var settings = try XCTUnwrap(bridge.request(["operation": "load", "path": file.path])["settings"] as? [String: Any])
        settings["theme"] = "custom"
        settings["custom_theme"] = ["accent": "#12AB34", "signal": "#9876FE"]
        _ = try bridge.request(["operation": "save", "path": file.path, "settings": settings])
        let store = try SettingsStore(path: file.path, debounceInterval: 60)
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 700)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame)
        window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let controller = PreferencesController(root: root, store: store, tokens: { tokens },
            appearanceChanged: { _, _, _ in }, showHistory: {})
        defer { withExtendedLifetime(controller) {} }
        let deadline = Date().addingTimeInterval(3)
        while controller.shortcutCard() == nil && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        controller.showFind()
        func field(_ id: String) throws -> NSTextField {
            try XCTUnwrap(KeyViewLoop.candidates(in: root).first {
                $0.identifier?.rawValue == id
            } as? NSTextField)
        }
        func press(_ editor: NSTextView, key: UInt16, text: String) throws {
            let event = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber,
                context: nil, characters: text, charactersIgnoringModifiers: text,
                isARepeat: false, keyCode: key))
            editor.keyDown(with: event)
        }
        for (key, original) in [("accent", "#12AB34"), ("signal", "#9876FE")] {
            for draft in ["#FEDCBA", "#badhex"] {
                let before = try Data(contentsOf: file)
                let find = try field("find")
                let color = try field("custom.\(key)")
                color.selectText(nil)
                let editor = try XCTUnwrap(color.currentEditor() as? NSTextView)
                editor.insertText(draft, replacementRange: editor.selectedRange())
                try press(editor, key: 53, text: "\u{1b}")
                store.flush()
                XCTAssertEqual(color.stringValue, original)
                XCTAssertNil(color.currentEditor(), "Escape leaves the color field")
                XCTAssertTrue(try field("find") === find, "Escape must not close or rebuild Find")
                XCTAssertEqual(try Data(contentsOf: file), before, "cancelled drafts do not change persisted settings")
            }
            let color = try field("custom.\(key)")
            color.selectText(nil)
            let editor = try XCTUnwrap(color.currentEditor() as? NSTextView)
            editor.insertText("#abc", replacementRange: editor.selectedRange())
            try press(editor, key: 36, text: "\r")
            store.flush()
            let saved = try XCTUnwrap(bridge.request(["operation": "load", "path": file.path])["settings"] as? [String: Any])
            XCTAssertEqual((saved["custom_theme"] as? [String: String])?[key], "#AABBCC")
            XCTAssertNotNil(try field("find"))
        }
    }

    func testThemeResponseMergesAllDerivedTokens() {
        let transport = RecordingTransport()
        let base = Tokens.variants["light-mustard"]!
        let themed = base.applyingCustomTheme(["accent": "#ff0000", "signal": "#00ff00"], light: true, transport: transport)
        XCTAssertEqual(themed.colors["theme-accent"], [1, 0, 0, 1])
        XCTAssertEqual(transport.operations(), ["theme"])
    }
}
