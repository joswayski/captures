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
            if case .failure(let error) = result {
                XCTAssertEqual(error.localizedDescription, "profile collision")
            } else {
                XCTFail("Registration failure must reach the control")
            }
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

    func testPreferencesShowGifCardMicrophoneAndHighlightSectionInView() throws {
        let (root, controller) = try preferencesFixture(service: nil)
        // Settings load on the store's worker before the cards are built, so a
        // single main-queue drain can run first on a slow runner.
        let deadline = Date().addingTimeInterval(2)
        while !labels(root).contains("Palette colors"), Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        drainMainQueue()
        let text = labels(root)
        for title in ["Default microphone", "Frames per second", "Maximum width", "Palette colors"] {
            XCTAssertTrue(text.contains(title), title)
        }
        XCTAssertFalse(text.contains("GIF quality"), "the stub GIF card is replaced")
        XCTAssertEqual(controller.activeSection, "appearance")
        let scroll = try XCTUnwrap(root.subviews.compactMap { $0 as? NSScrollView }.first)
        let document = try XCTUnwrap(scroll.documentView)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: document.frame.height - scroll.contentSize.height))
        scroll.reflectScrolledClipView(scroll.contentView)
        drainMainQueue()
        XCTAssertEqual(controller.activeSection, "about", "the end of the page highlights the last section")
        withExtendedLifetime(controller) {}
    }

    func testPreferencesUseSharedCopyWholeRowSwitchesAndSegmentedAppearance() throws {
        var appearances: [String] = []
        let (root, controller) = try preferencesFixture(service: nil) { appearance in
            appearances.append(appearance)
        }
        let deadline = Date().addingTimeInterval(2)
        while find(root, identifier: "setting.freeze_screen") == nil, Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        let text = labels(root)
        for title in ["Interface theme", "Accent color", "Mini preview position", "Show what’s new on update notices",
                      PreferencesPolicy.text("shortcuts.system_title"), PreferencesPolicy.text("updates.title")] {
            XCTAssertTrue(text.contains(title), title)
        }
        XCTAssertEqual(PreferencesPolicy.text("shortcuts.system_title"), "macOS Screenshot shortcuts")

        // Shipping option labels, not raw persisted values.
        let menus = views(root, ClosurePopUpButton.self)
        let format = try XCTUnwrap(menus.first { $0.accessibilityLabel() == "Screenshot format" })
        XCTAssertEqual(format.itemTitles, ["PNG", "JPEG", "WebP"])
        let countdown = try XCTUnwrap(menus.first { $0.accessibilityLabel() == "Screenshot countdown" })
        XCTAssertEqual(countdown.itemTitles.prefix(3), ["Off", "1 second", "2 seconds"])
        XCTAssertNotNil(menus.first { $0.accessibilityLabel() == "GIF palette colors" })

        // The whole row is the switch; toggling rebuilds with the new state.
        let freeze = try XCTUnwrap(find(root, identifier: "setting.freeze_screen") as? PreferenceSwitchButton)
        XCTAssertFalse(freeze.isOn)
        XCTAssertGreaterThan(freeze.frame.width, freeze.switchRect.width * 10, "the row, not just the knob")
        freeze.performClick(nil)
        let toggled = try XCTUnwrap(find(root, identifier: "setting.freeze_screen") as? PreferenceSwitchButton)
        XCTAssertTrue(toggled.isOn)

        // Mini preview corners follow the Show mini previews switch.
        let corner = try XCTUnwrap(find(root, identifier: "mini-preview-placement.top_right") as? NSButton)
        XCTAssertFalse(corner.isEnabled)

        // System / Light / Dark segmented control in shipping order.
        let segments = views(root, PreferenceSegmentButton.self)
        XCTAssertEqual(segments.map(\.title), ["System", "Light", "Dark"])
        XCTAssertEqual(segments.filter(\.active).map(\.title), ["Dark"])
        segments[1].performClick(nil)
        XCTAssertEqual(appearances.last, "light")

        // Shared find policy: matches, count label and no results.
        controller.showFind()
        let field = try XCTUnwrap(find(root, identifier: "find") as? NSTextField)
        field.stringValue = "freeze screen when"
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: field))
        XCTAssertTrue(labels(root).contains("1 of 1"))
        field.stringValue = "no such preference"
        controller.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: field))
        XCTAssertTrue(labels(root).contains("No results"))
        controller.closeFind()
        XCTAssertNil(find(root, identifier: "find"))
        withExtendedLifetime(controller) {}
    }

    func testRenderedPreferencesCardsLightAndDark() throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        for appearance in ["light", "dark"] {
            let (root, controller) = try preferencesFixture(service: FakeLoginItemService([.success(false)]),
                appearance: appearance)
            waitForTitle("Off", in: root)
            for id in ["appearance", "capture", "shortcuts", "recording", "about"] {
                let card = try XCTUnwrap(find(root, identifier: "preferences-card.\(id)"))
                card.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(card.bitmapImageRepForCachingDisplay(in: card.bounds))
                card.cacheDisplay(in: card.bounds, to: bitmap)
                let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                let url = URL(fileURLWithPath: directory)
                    .appendingPathComponent("preferences-\(id)-\(appearance).png")
                try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                        withIntermediateDirectories: true)
                try data.write(to: url)
            }
            withExtendedLifetime(controller) {}
        }
    }

    private func preferencesFixture(service: LoginItemServicing?, appearance: String = "dark",
                                    appearanceChanged: @escaping (String) -> Void = { _ in }) throws
        -> (Surface, PreferencesController) {
        _ = NSApplication.shared
        let root = Surface(frame: NSRect(x: 0, y: 0, width: 1000, height: 600))
        let tokens = Tokens.variants["\(appearance)-mustard"]!
        let store = try SettingsStore(path: "/fixture/settings.json",
            transport: LoginSettingsTransport(), debounceInterval: 0)
        let controller = PreferencesController(root: root, store: store, tokens: { tokens },
            appearanceChanged: { value, _, _ in appearanceChanged(value) }, showHistory: {},
            liveCaptureAvailable: true,
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
        return view.subviews.lazy.compactMap { self.find($0, identifier: identifier) }.first
    }

    private func views<T: NSView>(_ view: NSView, _ type: T.Type) -> [T] {
        ((view as? T).map { [$0] } ?? []) + view.subviews.flatMap { views($0, type) }
    }

    private func labels(_ view: NSView) -> [String] {
        (view as? NSTextField).map { [$0.stringValue] } ?? view.subviews.flatMap(labels)
    }
}
