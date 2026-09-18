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
    func testRapidUpdatesCoalesceToNewestRevision() throws {
        let transport = RecordingTransport()
        let store = try SettingsStore(path: "/fixture.json", transport: transport, debounceInterval: 0.03)
        let completion = expectation(description: "newest save")
        var completedRevision = 0
        _ = store.save(["value": 1]) { _, _ in XCTFail("superseded save completed") }
        let newest = store.save(["value": 2]) { revision, result in
            XCTAssertNoThrow(try result.get()); completedRevision = revision; completion.fulfill()
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
            XCTAssertThrowsError(try result.get()); loaded.fulfill()
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
