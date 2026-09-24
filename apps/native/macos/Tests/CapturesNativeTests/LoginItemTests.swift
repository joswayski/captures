import AppKit
import XCTest
@testable import CapturesNative

private final class LoginTransport: SettingsTransport {
    var requests: [[String: Any]] = []
    var enabled = false
    var failure: Error?
    func request(_ object: [String: Any]) throws -> [String: Any] {
        requests.append(object)
        if let failure { throw failure }
        switch object["operation"] as? String {
        case "default_path": return ["ok": true, "path": "/profiles/native/settings.json"]
        case "login_item":
            if let requested = object["enabled"] as? Bool { enabled = requested }
            return ["ok": true, "enabled": enabled]
        default: throw SettingsStoreError.invalidResponse
        }
    }
}

private struct LoginAppTransport: AppTransport {
    func request(_ object: [String: Any]) throws -> [String: Any] {
        guard object["operation"] as? String == "default_history_root" else {
            throw AppBridgeError.invalidResponse
        }
        return ["ok": true, "path": "/profiles/native/history"]
    }
}

private final class LoginSettingsTransport: SettingsTransport {
    func request(_ object: [String: Any]) throws -> [String: Any] {
        switch object["operation"] as? String {
        case "load": return ["ok": true, "settings": ["appearance": "dark", "theme": "mustard"]]
        case "save": return ["ok": true, "settings": object["settings"] as? [String: Any] ?? [:]]
        default: return ["ok": true, "path": "/fixture/settings.json"]
        }
    }
}

private final class FakeLoginItemService: LoginItemServicing {
    var calls: [Bool?] = []
    var results: [Result<Bool, Error>]
    init(_ results: [Result<Bool, Error>]) { self.results = results }
    func setEnabled(_ enabled: Bool?, completion: @escaping (Result<Bool, Error>) -> Void) {
        calls.append(enabled)
        let result = results.removeFirst()
        DispatchQueue.main.async { completion(result) }
    }
}

final class LoginItemTests: XCTestCase {
    func testQueryResolvesProfileRootsAndOmitsEnabled() {
        let transport = LoginTransport()
        let service = NativeLoginItemService(historyRoot: nil, settingsFile: nil, transport: transport,
                                             appTransport: LoginAppTransport())
        let completed = expectation(description: "query")
        service.setEnabled(nil) { result in
            XCTAssertEqual(try? result.get(), false); completed.fulfill()
        }
        wait(for: [completed], timeout: 1)
        let request = transport.requests.last
        XCTAssertEqual(request?["operation"] as? String, "login_item")
        XCTAssertEqual(request?["history_root"] as? String, "/profiles/native/history")
        XCTAssertEqual(request?["settings_file"] as? String, "/profiles/native/settings.json")
        XCTAssertNil(request?["enabled"])
    }

    func testToggleUsesExplicitRootsAndReturnsAuthoritativeState() {
        let transport = LoginTransport()
        let service = NativeLoginItemService(historyRoot: "/custom/history",
            settingsFile: "/custom/settings.json", transport: transport)
        let completed = expectation(description: "toggle")
        service.setEnabled(true) { result in
            XCTAssertEqual(try? result.get(), true); completed.fulfill()
        }
        wait(for: [completed], timeout: 1)
        XCTAssertEqual(transport.requests.count, 1)
        XCTAssertEqual(transport.requests[0]["enabled"] as? Bool, true)
    }

    func testBackendFailureRemainsRetryable() {
        let transport = LoginTransport()
        transport.failure = SettingsStoreError.backend("profile collision")
        let service = NativeLoginItemService(historyRoot: "/history", settingsFile: "/settings",
                                             transport: transport)
        let failed = expectation(description: "failure")
        service.setEnabled(true) { result in
            XCTAssertThrowsError(try result.get())
            failed.fulfill()
        }
        wait(for: [failed], timeout: 1)
        transport.failure = nil
        let retried = expectation(description: "retry")
        service.setEnabled(true) { result in
            XCTAssertEqual(try? result.get(), true); retried.fulfill()
        }
        wait(for: [retried], timeout: 1)
    }

    func testPreferencesFixtureDoesNotQueryWithoutInjectedService() throws {
        let (root, controller) = try preferencesFixture(service: nil)
        drainMainQueue()
        XCTAssertNil(find(root, identifier: "login-item"))
        withExtendedLifetime(controller) {}
    }

    func testPreferencesQueriesTogglesAndDisplaysRetryableError() throws {
        let service = FakeLoginItemService([.success(false),
            .failure(SettingsStoreError.backend("profile collision")), .success(true)])
        let (root, controller) = try preferencesFixture(service: service)
        waitForTitle("Off", in: root)
        XCTAssertEqual(service.calls.count, 1); XCTAssertNil(service.calls[0])
        let off = try XCTUnwrap(find(root, identifier: "login-item") as? NSButton)
        XCTAssertEqual(off.title, "Off")
        off.performClick(nil)
        let pending = try XCTUnwrap(find(root, identifier: "login-item") as? NSButton)
        XCTAssertEqual(pending.title, "Checking…"); XCTAssertFalse(pending.isEnabled)
        drainMainQueue()
        XCTAssertEqual(service.calls[1], true)
        let retry = try XCTUnwrap(find(root, identifier: "login-item") as? NSButton)
        XCTAssertEqual(retry.title, "Retry")
        XCTAssertTrue(labels(root).contains { $0.contains("profile collision") })
        retry.performClick(nil); drainMainQueue()
        XCTAssertNil(service.calls[2], "retry queries authoritative OS state instead of guessing")
        XCTAssertEqual((find(root, identifier: "login-item") as? NSButton)?.title, "On")
        withExtendedLifetime(controller) {}
    }

    func testRenderedPreferencesLoginItemControlLightAndDark() throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        for appearance in ["light", "dark"] {
            let service = FakeLoginItemService([.success(true)])
            let (root, controller) = try preferencesFixture(service: service, appearance: appearance)
            waitForTitle("On", in: root)
            let card = try XCTUnwrap(find(root, identifier: "preferences-card.about"))
            card.layoutSubtreeIfNeeded()
            let bitmap = try XCTUnwrap(card.bitmapImageRepForCachingDisplay(in: card.bounds))
            card.cacheDisplay(in: card.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            let url = URL(fileURLWithPath: directory)
                .appendingPathComponent("preferences-about-login-item-\(appearance).png")
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                    withIntermediateDirectories: true)
            try data.write(to: url)
            withExtendedLifetime(controller) {}
        }
    }

    private func preferencesFixture(service: LoginItemServicing?, appearance: String = "dark") throws
        -> (Surface, PreferencesController) {
        _ = NSApplication.shared
        let root = Surface(frame: NSRect(x: 0, y: 0, width: 1000, height: 720))
        let tokens = Tokens.variants["\(appearance)-mustard"]!
        let store = try SettingsStore(path: "/fixture/settings.json",
            transport: LoginSettingsTransport(), debounceInterval: 0)
        let controller = PreferencesController(root: root, store: store, tokens: { tokens },
            appearanceChanged: { _, _, _ in }, showHistory: {}, liveCaptureAvailable: true,
            loginItemService: service, initialAppearance: appearance)
        return (root, controller)
    }

    private func drainMainQueue() {
        let completed = expectation(description: "main queue")
        DispatchQueue.main.async { completed.fulfill() }
        wait(for: [completed], timeout: 1)
    }

    private func waitForTitle(_ title: String, in root: NSView) {
        let deadline = Date().addingTimeInterval(2)
        while (find(root, identifier: "login-item") as? NSButton)?.title != title,
              Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertEqual((find(root, identifier: "login-item") as? NSButton)?.title, title)
    }

    private func find(_ view: NSView, identifier: String) -> NSView? {
        if view.identifier?.rawValue == identifier { return view }
        return view.subviews.lazy.compactMap { find($0, identifier: identifier) }.first
    }

    private func labels(_ view: NSView) -> [String] {
        (view as? NSTextField).map { [$0.stringValue] } ?? view.subviews.flatMap(labels)
    }
}
