import AppKit

/// Actions the menu bar sends to the host rather than down the responder chain.
struct AppMainMenuActions {
    let target: AnyObject
    let quit: Selector
    let find: Selector
    let findNext: Selector
    let findPrevious: Selector
}

/// The application menu bar. Shipping runs with the regular activation policy
/// and `tauri::Builder::default()`, so macOS gets Tauri 2's default menu
/// (`Menu::default`): an app menu (About, Services, Hide, Hide Others, Quit),
/// File (Close Window), Edit (Undo, Redo, Cut, Copy, Paste, Select All), View
/// (full screen), Window (Minimize, Zoom, Close Window) and Help. Window and
/// editing items use the standard AppKit selectors with a nil target, so the
/// key window and its first responder handle them: ⌘W runs each window's own
/// `windowShouldClose` path, text fields get their field editor's undo, and
/// the screenshot canvas keeps its own ⌘Z handling in `performKeyEquivalent`.
/// Find, Find Next and Find Previous are shipping's Preferences find (⌘F, ⌘G,
/// ⇧⌘G), which the web UI handles in-page and AppKit routes through the menu.
struct AppMainMenu {
    let menu: NSMenu
    let servicesMenu: NSMenu
    let windowsMenu: NSMenu
    let helpMenu: NSMenu

    init(appName: String, quitTitle: String, actions: AppMainMenuActions) {
        menu = NSMenu()
        servicesMenu = NSMenu(title: "Services")
        windowsMenu = NSMenu(title: "Window")
        helpMenu = NSMenu(title: "Help")

        let appMenu = NSMenu(title: appName)
        appMenu.addItem(withTitle: "About \(appName)",
                        action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        let services = appMenu.addItem(withTitle: "Services", action: nil, keyEquivalent: "")
        services.submenu = servicesMenu
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Hide \(appName)", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let hideOthers = appMenu.addItem(withTitle: "Hide Others",
                                         action: #selector(NSApplication.hideOtherApplications(_:)),
                                         keyEquivalent: "h")
        hideOthers.keyEquivalentModifierMask = [.command, .option]
        appMenu.addItem(.separator())
        let quit = appMenu.addItem(withTitle: quitTitle, action: actions.quit, keyEquivalent: "q")
        quit.target = actions.target
        Self.attach(appMenu, to: menu)

        let fileMenu = NSMenu(title: "File")
        fileMenu.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        Self.attach(fileMenu, to: menu)

        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        let redo = editMenu.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "Z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenu.addItem(.separator())
        let find = editMenu.addItem(withTitle: "Find…", action: actions.find, keyEquivalent: "f")
        find.target = actions.target
        let next = editMenu.addItem(withTitle: "Find Next", action: actions.findNext, keyEquivalent: "g")
        next.target = actions.target
        let previous = editMenu.addItem(withTitle: "Find Previous", action: actions.findPrevious,
                                        keyEquivalent: "G")
        previous.keyEquivalentModifierMask = [.command, .shift]
        previous.target = actions.target
        Self.attach(editMenu, to: menu)

        let viewMenu = NSMenu(title: "View")
        let fullScreen = viewMenu.addItem(withTitle: "Enter Full Screen",
                                          action: #selector(NSWindow.toggleFullScreen(_:)), keyEquivalent: "f")
        fullScreen.keyEquivalentModifierMask = [.command, .control]
        Self.attach(viewMenu, to: menu)

        windowsMenu.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)),
                            keyEquivalent: "m")
        windowsMenu.addItem(withTitle: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: "")
        windowsMenu.addItem(.separator())
        windowsMenu.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)),
                            keyEquivalent: "w")
        Self.attach(windowsMenu, to: menu)

        Self.attach(helpMenu, to: menu)
    }

    /// Install as the menu bar, registering the Services, Window and Help
    /// menus with AppKit as Tauri does for its default menu.
    func install(in application: NSApplication) {
        application.mainMenu = menu
        application.servicesMenu = servicesMenu
        application.windowsMenu = windowsMenu
        application.helpMenu = helpMenu
    }

    private static func attach(_ submenu: NSMenu, to bar: NSMenu) {
        let item = NSMenuItem(title: submenu.title, action: nil, keyEquivalent: "")
        item.submenu = submenu
        bar.addItem(item)
    }
}
