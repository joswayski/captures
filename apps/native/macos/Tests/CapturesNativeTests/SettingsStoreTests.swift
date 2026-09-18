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

    func testThemeResponseMergesAllDerivedTokens() {
        let transport = RecordingTransport()
        let base = Tokens.variants["light-mustard"]!
        let themed = base.applyingCustomTheme(["accent": "#ff0000", "signal": "#00ff00"], light: true, transport: transport)
        XCTAssertEqual(themed.colors["theme-accent"], [1, 0, 0, 1])
        XCTAssertEqual(transport.operations(), ["theme"])
    }
}
