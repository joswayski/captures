import AppKit
import XCTest
@testable import CapturesNative

private final class MenuActionTarget: NSObject {
    var calls: [String] = []
    @objc func quitAction(_ sender: Any?) { calls.append("quit") }
    @objc func findAction(_ sender: Any?) { calls.append("find") }
    @objc func findNextAction(_ sender: Any?) { calls.append("next") }
    @objc func findPreviousAction(_ sender: Any?) { calls.append("previous") }
}

final class MainMenuTests: XCTestCase {
    private func makeMenu(_ target: MenuActionTarget = MenuActionTarget()) -> AppMainMenu {
        AppMainMenu(appName: "Captures", quitTitle: "Quit Captures",
                    actions: AppMainMenuActions(target: target, quit: #selector(MenuActionTarget.quitAction(_:)),
                                                find: #selector(MenuActionTarget.findAction(_:)),
                                                findNext: #selector(MenuActionTarget.findNextAction(_:)),
                                                findPrevious: #selector(MenuActionTarget.findPreviousAction(_:))))
    }

    private func submenu(_ title: String, in built: AppMainMenu) throws -> NSMenu {
        try XCTUnwrap(built.menu.items.first { $0.submenu?.title == title }?.submenu, "missing \(title) menu")
    }

    private func item(_ title: String, in menu: NSMenu) throws -> NSMenuItem {
        try XCTUnwrap(menu.items.first { $0.title == title }, "missing \(title)")
    }

    private func titles(_ menu: NSMenu) -> [String] {
        menu.items.map { $0.isSeparatorItem ? "-" : $0.title }
    }

    private func waitFor(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), "state did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }

    func testMenuBarMatchesTheTauriDefaultMenu() throws {
        _ = NSApplication.shared
        let built = makeMenu()
        XCTAssertEqual(built.menu.items.compactMap { $0.submenu?.title },
                       ["Captures", "File", "Edit", "View", "Window", "Help"])
        let app = try submenu("Captures", in: built)
        XCTAssertEqual(titles(app), ["About Captures", "-", "Services", "-", "Hide Captures", "Hide Others",
                                     "-", "Quit Captures"])
        XCTAssertTrue(try item("Services", in: app).submenu === built.servicesMenu)
        XCTAssertEqual(titles(try submenu("File", in: built)), ["Close Window"])
        XCTAssertEqual(titles(try submenu("Edit", in: built)),
                       ["Undo", "Redo", "-", "Cut", "Copy", "Paste", "Select All", "-",
                        "Find…", "Find Next", "Find Previous"])
        XCTAssertEqual(titles(try submenu("View", in: built)), ["Enter Full Screen"])
        let window = try submenu("Window", in: built)
        XCTAssertTrue(window === built.windowsMenu)
        XCTAssertEqual(titles(window), ["Minimize", "Zoom", "-", "Close Window"])
        XCTAssertTrue(try submenu("Help", in: built) === built.helpMenu)
    }

    func testStandardItemsUseAppKitSelectorsAndShippingKeyEquivalents() throws {
        _ = NSApplication.shared
        let built = makeMenu()
        let app = try submenu("Captures", in: built)
        let edit = try submenu("Edit", in: built)
        let window = try submenu("Window", in: built)
        let file = try submenu("File", in: built)
        let view = try submenu("View", in: built)
        let expected: [(NSMenu, String, Selector, String, NSEvent.ModifierFlags)] = [
            (app, "About Captures", #selector(NSApplication.orderFrontStandardAboutPanel(_:)), "", [.command]),
            (app, "Hide Captures", #selector(NSApplication.hide(_:)), "h", [.command]),
            (app, "Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]),
            (file, "Close Window", #selector(NSWindow.performClose(_:)), "w", [.command]),
            (edit, "Undo", Selector(("undo:")), "z", [.command]),
            (edit, "Redo", Selector(("redo:")), "Z", [.command, .shift]),
            (edit, "Cut", #selector(NSText.cut(_:)), "x", [.command]),
            (edit, "Copy", #selector(NSText.copy(_:)), "c", [.command]),
            (edit, "Paste", #selector(NSText.paste(_:)), "v", [.command]),
            (edit, "Select All", #selector(NSText.selectAll(_:)), "a", [.command]),
            (view, "Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control]),
            (window, "Minimize", #selector(NSWindow.performMiniaturize(_:)), "m", [.command]),
            (window, "Zoom", #selector(NSWindow.performZoom(_:)), "", [.command]),
            (window, "Close Window", #selector(NSWindow.performClose(_:)), "w", [.command]),
        ]
        for (menu, title, action, key, modifiers) in expected {
            let entry = try item(title, in: menu)
            XCTAssertEqual(entry.action, action, title)
            XCTAssertNil(entry.target, "\(title) goes down the responder chain")
            XCTAssertEqual(entry.keyEquivalent, key, title)
            if !key.isEmpty { XCTAssertEqual(entry.keyEquivalentModifierMask, modifiers, title) }
        }
        // Text fields' field editors answer Undo and Redo themselves.
        XCTAssertTrue(NSTextView().responds(to: Selector(("undo:"))))
        XCTAssertTrue(NSTextView().responds(to: Selector(("redo:"))))
    }

    func testHostActionsTargetTheHost() throws {
        _ = NSApplication.shared
        let target = MenuActionTarget()
        let built = makeMenu(target)
        let app = try submenu("Captures", in: built)
        let edit = try submenu("Edit", in: built)
        let quit = try item("Quit Captures", in: app)
        XCTAssertEqual(quit.keyEquivalent, "q")
        XCTAssertEqual(quit.keyEquivalentModifierMask, [.command])
        XCTAssertEqual(try item("Find…", in: edit).keyEquivalent, "f")
        XCTAssertEqual(try item("Find Next", in: edit).keyEquivalent, "g")
        let previous = try item("Find Previous", in: edit)
        XCTAssertEqual(previous.keyEquivalent, "G")
        XCTAssertEqual(previous.keyEquivalentModifierMask, [.command, .shift])
        for title in ["Find…", "Find Next", "Find Previous"] {
            let entry = try item(title, in: edit)
            XCTAssertTrue(entry.target === target, title)
            _ = NSApp.sendAction(try XCTUnwrap(entry.action), to: entry.target, from: entry)
        }
        _ = NSApp.sendAction(try XCTUnwrap(quit.action), to: quit.target, from: quit)
        XCTAssertEqual(target.calls, ["find", "next", "previous", "quit"])
    }

    func testInstallRegistersServicesWindowAndHelpMenus() {
        let application = NSApplication.shared
        let previousMain = application.mainMenu
        let previousServices = application.servicesMenu
        let previousWindows = application.windowsMenu
        let previousHelp = application.helpMenu
        defer {
            application.mainMenu = previousMain
            application.servicesMenu = previousServices
            application.windowsMenu = previousWindows
            application.helpMenu = previousHelp
        }
        let built = makeMenu()
        built.install(in: application)
        XCTAssertTrue(application.mainMenu === built.menu)
        XCTAssertTrue(application.servicesMenu === built.servicesMenu)
        XCTAssertTrue(application.windowsMenu === built.windowsMenu)
        XCTAssertTrue(application.helpMenu === built.helpMenu)
    }

    func testCloseWindowRunsAppWindowCloseVetoesAndHistoryHides() throws {
        _ = NSApplication.shared
        let close = try item("Close Window", in: try submenu("Window", in: makeMenu()))
        let action = try XCTUnwrap(close.action)
        let windows = AppWindows()
        var asked: [AppWindowKind] = []
        var allow = false
        var closed: [AppWindowKind] = []
        windows.shouldClose = { kind in asked.append(kind); return allow }
        windows.didClose = { closed.append($0) }
        let preferences = windows.show(.preferences, activate: false) { made in
            made.setContentSize(NSSize(width: 560, height: 440))
        }
        defer { windows.shouldClose = nil; preferences.close() }
        try waitFor { preferences.isVisible }

        preferences.perform(action, with: close)
        XCTAssertEqual(asked, [.preferences], "⌘W asks the window's own close path")
        XCTAssertTrue(preferences.isVisible, "a vetoed close keeps the window")
        allow = true
        preferences.perform(action, with: close)
        try waitFor { !preferences.isVisible }
        XCTAssertEqual(closed, [.preferences])

        // History hides through the host's root close handler instead of quitting.
        let history = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 500, height: 400),
                               styleMask: AppWindowKind.styleMask, backing: .buffered, defer: false)
        history.isReleasedWhenClosed = false
        var terminated = false
        let handler = RootWindowCloseHandler(rootWindow: history, closePreviews: {},
                                             terminate: { terminated = true }, hidesRootWindow: true)
        history.delegate = handler
        defer { history.delegate = nil; history.orderOut(nil) }
        history.orderFront(nil)
        try waitFor { history.isVisible }
        history.perform(action, with: close)
        try waitFor { !history.isVisible }
        XCTAssertFalse(terminated)
    }
}
