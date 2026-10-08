import AppKit
import XCTest
@testable import CapturesNative

final class AppWindowsTests: XCTestCase {
    func testWindowsUseTheShippingTitlesSizesAndMinimums() {
        _ = NSApplication.shared
        let expected: [(AppWindowKind, String, NSSize, NSSize)] = [
            (.history, "Capture History", NSSize(width: 1020, height: 720), NSSize(width: 640, height: 440)),
            (.preferences, "Captures Preferences", NSSize(width: 880, height: 660), NSSize(width: 560, height: 440)),
            (.setup, "Captures", NSSize(width: 620, height: 560), NSSize(width: 480, height: 440)),
        ]
        for (kind, title, size, minimum) in expected {
            XCTAssertEqual(kind.title, title)
            XCTAssertEqual(kind.size, size)
            XCTAssertEqual(kind.minimumSize, minimum)
            // Built but never shown, so taller specs do not touch the screen.
            let made = AppWindows.makeWindow(kind)
            defer { made.close() }
            XCTAssertEqual(made.title, title)
            XCTAssertEqual(made.contentLayoutRect.size, size)
            XCTAssertEqual(made.contentMinSize, minimum)
            XCTAssertTrue(made.styleMask.contains(.resizable), "\(title) is resizable")
            XCTAssertTrue(made.styleMask.contains(.closable))
            XCTAssertFalse(made.isReleasedWhenClosed)
        }
        XCTAssertEqual(EditorWindowTitle.screenshot, "Captures Screenshot Editor")
        XCTAssertEqual(EditorWindowTitle.recording, "Captures Editor")
        XCTAssertFalse(AppWindowLayout.compact(width: 1020))
        XCTAssertTrue(AppWindowLayout.compact(width: 720))
        XCTAssertTrue(AppWindowLayout.short(height: 560))
        XCTAssertFalse(AppWindowLayout.short(height: 720))
    }

    func testPostUpdateRestoresPreferencesBesideTheNoticeWithoutChangingPriorities() {
        XCTAssertEqual(interactiveLaunch(onboardingComplete: true, launchedQuietly: true,
            openingFiles: false, restorePreferences: true), .startupNoticeAndPreferences)
        XCTAssertEqual(interactiveLaunch(onboardingComplete: true, launchedQuietly: true,
            openingFiles: false, restorePreferences: false), .startupNotice)
        XCTAssertEqual(interactiveLaunch(onboardingComplete: false, launchedQuietly: true,
            openingFiles: false, restorePreferences: true), .setup)
        for complete in [true, false] {
            XCTAssertNil(interactiveLaunch(onboardingComplete: complete, launchedQuietly: true,
                openingFiles: true, restorePreferences: true), "explicit media keeps priority")
        }
    }

    func testShowCreatesOnceThenFocusesTheOpenWindow() throws {
        _ = NSApplication.shared
        let windows = AppWindows()
        var builds = 0
        let setup = windows.show(.setup, activate: false) { _ in builds += 1 }
        defer { setup.close() }
        XCTAssertEqual(builds, 1)
        try settle { setup.isVisible }
        XCTAssertTrue(windows.window(.setup) === setup)
        XCTAssertEqual(windows.visibleKinds, [.setup])

        // Another window takes the front; showing again reuses and raises it.
        let other = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
                             styleMask: [.titled], backing: .buffered, defer: false)
        other.isReleasedWhenClosed = false
        defer { other.close() }
        other.makeKeyAndOrderFront(nil)
        try settle { NSApp.orderedWindows.first === other }
        let again = windows.show(.setup, activate: false) { _ in builds += 1 }
        XCTAssertTrue(again === setup, "an open window is focused, not duplicated")
        XCTAssertEqual(builds, 1)
        try settle { NSApp.orderedWindows.first === setup }
    }

    func testClosingOneWindowLeavesTheOthersOpen() throws {
        _ = NSApplication.shared
        let windows = AppWindows()
        var closed: [AppWindowKind] = []
        windows.didClose = { closed.append($0) }
        // The host keeps History's own close handler; it is adopted, not owned.
        let history = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 500, height: 400),
                               styleMask: AppWindowKind.styleMask, backing: .buffered, defer: false)
        history.isReleasedWhenClosed = false
        defer { history.close() }
        windows.adopt(history, as: .history)
        history.orderFront(nil)
        let setup = windows.show(.setup, activate: false)
        defer { setup.close() }
        try settle { history.isVisible && setup.isVisible }
        XCTAssertEqual(Set(windows.visibleKinds), [.history, .setup])
        XCTAssertTrue(windows.kind(of: history) == .history)

        setup.performClose(nil)
        try settle { !setup.isVisible }
        XCTAssertEqual(closed, [.setup])
        XCTAssertNil(windows.window(.setup), "a closed window is forgotten and rebuilt next time")
        XCTAssertTrue(history.isVisible, "closing setup leaves History open")

        var rebuilt = 0
        let reopened = windows.show(.setup, activate: false) { _ in rebuilt += 1 }
        defer { reopened.close() }
        XCTAssertEqual(rebuilt, 1)
        XCTAssertFalse(reopened === setup)

        // Closing History (adopted) never forgets it or reports it closed.
        history.close()
        XCTAssertTrue(windows.window(.history) === history)
        XCTAssertEqual(closed, [.setup])
        XCTAssertTrue(reopened.isVisible, "closing History leaves setup open")
    }

    func testShouldCloseCanKeepAWindowOpen() throws {
        _ = NSApplication.shared
        let windows = AppWindows()
        var asked: [AppWindowKind] = []
        windows.shouldClose = { kind in asked.append(kind); return false }
        let setup = windows.show(.setup, activate: false)
        defer { windows.shouldClose = nil; setup.close() }
        try settle { setup.isVisible }
        setup.performClose(nil)
        XCTAssertEqual(asked, [.setup])
        XCTAssertTrue(setup.isVisible)
        XCTAssertTrue(windows.window(.setup) === setup)
    }

    func testPreferencesDropsItsNavAndReflowsInACompactWindow() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let spec = AppWindowKind.preferences
        let preferencesWindow = AppWindows.makeWindow(spec)
        defer { preferencesWindow.close() }
        // Test windows stay at most 600 pt tall.
        preferencesWindow.setContentSize(NSSize(width: spec.size.width, height: 600))
        let root = Surface(frame: NSRect(origin: .zero, size: preferencesWindow.contentLayoutRect.size))
        preferencesWindow.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let store = try SettingsStore(path: directory.appendingPathComponent("settings.json").path,
                                      debounceInterval: 0)
        let controller = PreferencesController(root: root, store: store, tokens: { tokens },
            appearanceChanged: { _, _, _ in }, showHistory: {})
        defer { withExtendedLifetime(controller) {} }
        try settle { controller.shortcutCard() != nil }
        let sections = PreferencesPolicy.sections.count
        func navButtons() -> [PreferenceNavButton] {
            descendants(of: root).compactMap { $0 as? PreferenceNavButton }.filter { !$0.isHiddenOrHasHiddenAncestor }
        }
        XCTAssertEqual(navButtons().count, sections, "the default window shows the section nav")

        preferencesWindow.setContentSize(spec.minimumSize)
        try settle { navButtons().isEmpty }
        let card = try XCTUnwrap(controller.shortcutCard())
        let cardFrame = try XCTUnwrap(card.superview).convert(card.frame, to: root)
        XCTAssertGreaterThanOrEqual(cardFrame.minX, 0)
        XCTAssertLessThanOrEqual(cardFrame.maxX, root.bounds.width + 0.5, "cards fit the minimum window")

        preferencesWindow.setContentSize(NSSize(width: spec.size.width, height: 600))
        try settle { navButtons().count == sections }
    }

    private func descendants(of view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap { descendants(of: $0) }
    }

    private func settle(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), "window state did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}
